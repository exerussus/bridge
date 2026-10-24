//! Экран и голос: WebRTC в панели браузера, узел передаёт только сигналинг (предложение, ответ, ICE) между панелями
//! соседей по подписанному каналу моста. Звук и видео идут напрямую между ПК — по локальной сети или Radmin.

use crate::node::Node;
use bridge_core::eq_ic;
use bridge_core::proto::ChannelInfo;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::broadcast;

pub struct Media {
    pub tx: broadcast::Sender<Value>,
    /// Сколько панелей сейчас слушают (если 0 — входящий звонок некому показать: трей откроет панель).
    pub listeners: AtomicUsize,
    /// Последние приглашения (комната → приглашение): панель, открытая позже, увидит входящий звонок.
    pub invites: Mutex<BTreeMap<String, Value>>,
    /// Кто в каких каналах и в каком состоянии (микрофон, звук, показы, кого смотрит): «комната|узел» → (состояние, unix-время).
    pub presence: Mutex<BTreeMap<String, (Value, i64)>>,
    /// Команды панели от ассистента (API /local/ui): ждут ответа панели.
    pub ui_waiters: Mutex<BTreeMap<String, tokio::sync::oneshot::Sender<Value>>>,
    /// Какая панель взялась выполнить команду (если открыто несколько окон — выполняет одно).
    pub ui_claims: Mutex<BTreeMap<String, Vec<(String, i64)>>>,
    /// Открыть панель (трей ставит своё — окно-приложение; иначе — браузер по умолчанию).
    pub panel_opener: Mutex<Option<Arc<dyn Fn(&str) + Send + Sync>>>,
}

/// Состояние участника считается живым столько секунд без обновления (панели шлют его каждые 5 с).
pub const PRESENCE_TTL: i64 = 16;

impl Default for Media {
    fn default() -> Self {
        Self::new()
    }
}

impl Media {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(512);
        Media {
            tx,
            listeners: AtomicUsize::new(0),
            invites: Mutex::new(BTreeMap::new()),
            presence: Mutex::new(BTreeMap::new()),
            ui_waiters: Mutex::new(BTreeMap::new()),
            ui_claims: Mutex::new(BTreeMap::new()),
            panel_opener: Mutex::new(None),
        }
    }

    /// Сообщение от соседа — панелям этого ПК.
    pub fn deliver(&self, msg: Value) {
        let kind = msg.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
        let room = msg.get("room").and_then(|t| t.as_str()).unwrap_or("").to_string();
        match kind.as_str() {
            "invite" => {
                let mut g = self.invites.lock();
                g.insert(room.clone(), msg.clone());
                // держим только свежие
                let now = chrono::Utc::now().timestamp();
                g.retain(|_, v| now - v.get("ts").and_then(|t| t.as_i64()).unwrap_or(now) < 120);
            }
            "bye" | "decline" | "cancel" => {
                self.invites.lock().remove(&room);
            }
            _ => {}
        }
        if msg.get("media").and_then(|m| m.as_str()) == Some("room") {
            let from = msg.get("from").and_then(|t| t.as_str()).unwrap_or("").to_string();
            self.note_presence(&room, &from, &kind, msg.get("st"));
        }
        let _ = self.tx.send(msg);
    }

    /// Запомнить состояние участника (state) или его уход (leave).
    pub fn note_presence(&self, room: &str, who: &str, kind: &str, st: Option<&Value>) {
        if room.is_empty() || who.is_empty() {
            return;
        }
        let key = format!("{}|{}", room, who);
        let mut g = self.presence.lock();
        match (kind, st) {
            ("state", Some(st)) => {
                g.insert(key, (st.clone(), chrono::Utc::now().timestamp()));
            }
            ("leave", _) => {
                g.remove(&key);
            }
            _ => {}
        }
    }

    /// Живые участники: [{room, node, st, age}].
    pub fn presence_list(&self) -> Vec<Value> {
        let now = chrono::Utc::now().timestamp();
        let mut g = self.presence.lock();
        g.retain(|_, (_, t)| now - *t < PRESENCE_TTL * 4);
        g.iter()
            .filter(|(_, (_, t))| now - *t < PRESENCE_TTL)
            .map(|(k, (st, t))| {
                let (room, node) = k.split_once('|').unwrap_or((k, ""));
                json!({ "room": room, "node": node, "st": st, "age": now - t })
            })
            .collect()
    }

    pub fn has_listeners(&self) -> bool {
        self.listeners.load(Ordering::SeqCst) > 0
    }

    pub fn pending_invites(&self) -> Vec<Value> {
        let now = chrono::Utc::now().timestamp();
        self.invites.lock().values().filter(|v| now - v.get("ts").and_then(|t| t.as_i64()).unwrap_or(now) < 120).cloned().collect()
    }
}

/// Каналы этого узла, куда может зайти сосед `peer` (для его привета).
pub fn channels_for(node: &Node, peer: &str) -> Vec<ChannelInfo> {
    let cfg = node.cfg.read();
    let me = cfg.node.clone();
    cfg.channels
        .iter()
        .filter(|(_, c)| cfg.channel_has(c, peer))
        .map(|(id, c)| {
            let mut members = vec![me.clone()];
            members.extend(cfg.channel_members(c));
            ChannelInfo { owner: me.clone(), id: id.clone(), title: if c.title.is_empty() { id.clone() } else { c.title.clone() }, members, art: crate::chanart::hash(node, id) }
        })
        .collect()
}

/// Все каналы, видимые здесь: свои и соседей (из их приветов).
pub fn all_channels(node: &Node) -> Vec<ChannelInfo> {
    let mut out: Vec<ChannelInfo> = Vec::new();
    {
        let cfg = node.cfg.read();
        for (id, c) in &cfg.channels {
            let mut members = vec![cfg.node.clone()];
            members.extend(cfg.channel_members(c));
            out.push(ChannelInfo { owner: cfg.node.clone(), id: id.clone(), title: if c.title.is_empty() { id.clone() } else { c.title.clone() }, members, art: crate::chanart::hash(node, id) });
        }
    }
    let names: Vec<String> = node.cfg.read().peers.iter().filter(|p| p.enabled).map(|p| p.name.clone()).collect();
    for n in names {
        for c in node.peer_state(&n).channels {
            if !out.iter().any(|x| eq_ic(&x.owner, &c.owner) && eq_ic(&x.id, &c.id)) {
                out.push(c);
            }
        }
    }
    out
}

/// Переслать сигнал участнику своего канала: отправитель и получатель — участники, получатель — наш сосед.
pub async fn relay(node: &Arc<Node>, from: &str, to: &str, mut msg: Value) -> Result<(), (u16, String)> {
    let room = msg.get("room").and_then(|t| t.as_str()).unwrap_or("").to_string();
    let (owner, id) = room.split_once(':').unwrap_or(("", ""));
    if !eq_ic(owner, &node.node_name()) {
        return Err((403, "пересылать можно только в своих каналах".into()));
    }
    let peer = {
        let cfg = node.cfg.read();
        let Some(c) = bridge_core::config::get_ic(&cfg.channels, id) else { return Err((404, format!("нет канала «{}»", id))) };
        if !cfg.channel_has(c, from) || !cfg.channel_has(c, to) {
            return Err((403, "не участник канала".into()));
        }
        match cfg.peer(to) {
            Some(p) if p.enabled && p.has_url() => p.clone(),
            _ => return Err((404, format!("«{}» не подключён к владельцу канала", to))),
        }
    };
    if let Some(o) = msg.as_object_mut() {
        o.remove("relayTo");
        o.remove("from");
        o.insert("origin".into(), Value::String(from.to_string()));
    }
    let client = node.client_for(&peer);
    match tokio::time::timeout(std::time::Duration::from_secs(8), client.signal(&msg)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err((e.status().unwrap_or(502), crate::link::explain(&e, &peer).0)),
        Err(_) => Err((504, crate::link::explain_timeout(&peer).0)),
    }
}

/// Выполнить действие в панели (голос и экран живут в окне панели — это браузер). Панели нет — открыть и дождаться.
pub async fn ui_command(node: &Arc<Node>, action: &str, args: Value, timeout: std::time::Duration) -> Result<Value, (u16, String)> {
    if !node.media.has_listeners() {
        let opener = node.media.panel_opener.lock().clone();
        match opener {
            Some(f) => f("voice"),
            None => {
                let _ = crate::http_local::open_url(&format!("{}#voice", node.dashboard_url()));
            }
        }
        let t0 = std::time::Instant::now();
        while !node.media.has_listeners() {
            if t0.elapsed() > std::time::Duration::from_secs(25) {
                return Err((503, "панель не открылась — откройте её (трей → «Открыть мост») и повторите".into()));
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(800)).await; // панель подгружает каналы
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    node.media.ui_waiters.lock().insert(id.clone(), tx);
    let _ = node.media.tx.send(json!({ "media": "ui", "type": "cmd", "id": id, "action": action, "args": args }));
    let r = tokio::time::timeout(timeout, rx).await;
    node.media.ui_waiters.lock().remove(&id);
    node.media.ui_claims.lock().remove(&id);
    match r {
        Ok(Ok(v)) => {
            if v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) {
                Ok(v.get("data").cloned().unwrap_or(Value::Null))
            } else {
                Err((422, v.get("error").and_then(|x| x.as_str()).unwrap_or("панель не выполнила команду").to_string()))
            }
        }
        _ => Err((504, "панель не ответила вовремя (возможно, ждёт действия человека — например, выбора экрана для показа)".into())),
    }
}
