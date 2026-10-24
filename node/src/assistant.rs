//! Ассистент (контракт v1, см. API.md). Ассистент — отдельная программа на одном ПК семьи («хозяин»). Он забирает
//! входящие у своего моста (`assistant/wait`) и отдаёт ответы (`assistant/reply`). Мосты остальных ПК («клиенты»)
//! показывают вкладку «Ассистент» и пересылают сообщения хозяину по протоколу соседей. Имя узла (`peer`) во входящем
//! ставит хозяин по подписи соседа — подделать его с клиента нельзя.

use crate::node::Node;
use bridge_core::time;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

pub const VERSION: u32 = 1;
const INBOUND_MAX: usize = 1000;
const INBOUND_DAYS: i64 = 7;
const HISTORY_MAX: usize = 500;
const FILE_MAX: usize = 10 * 1024 * 1024;
const ALIVE: Duration = Duration::from_secs(120);

pub struct Assistant {
    dir: Mutex<PathBuf>,
    // ── хозяин ──
    inbound: Mutex<Vec<Value>>,
    seq: AtomicU64,
    acked: AtomicU64,
    seen: AtomicBool,
    title: Mutex<String>,
    alive: Mutex<Option<Instant>>,
    offline: AtomicBool,
    in_notify: Notify,
    /// Ответы для соседей, которые пока не на связи: сосед → [(ответ, крайний срок unix)].
    queued: Mutex<BTreeMap<String, Vec<(Value, i64)>>>,
    ids: Mutex<VecDeque<String>>,
    // ── у всех: чат панели ──
    history: Mutex<Vec<Value>>,
    hist_n: AtomicU64,
    pub hist_notify: Notify,
    outbox: Mutex<Vec<Value>>,
    pub kick: Notify,
    typing: Mutex<Option<(String, Instant)>>,
    read_n: AtomicU64,
    reply_ids: Mutex<VecDeque<String>>,
    msg_n: AtomicU64,
}

impl Default for Assistant {
    fn default() -> Self {
        Assistant {
            dir: Mutex::new(PathBuf::new()),
            inbound: Mutex::new(Vec::new()),
            seq: AtomicU64::new(0),
            acked: AtomicU64::new(0),
            seen: AtomicBool::new(false),
            title: Mutex::new("Ассистент".into()),
            alive: Mutex::new(None),
            offline: AtomicBool::new(false),
            in_notify: Notify::new(),
            queued: Mutex::new(BTreeMap::new()),
            ids: Mutex::new(VecDeque::new()),
            history: Mutex::new(Vec::new()),
            hist_n: AtomicU64::new(0),
            hist_notify: Notify::new(),
            outbox: Mutex::new(Vec::new()),
            kick: Notify::new(),
            typing: Mutex::new(None),
            read_n: AtomicU64::new(0),
            reply_ids: Mutex::new(VecDeque::new()),
            msg_n: AtomicU64::new(0),
        }
    }
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

fn read_jsonl(p: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(p).map(|t| t.lines().filter_map(|l| serde_json::from_str(l.trim_start_matches('\u{feff}')).ok()).collect()).unwrap_or_default()
}

fn write_jsonl(p: &std::path::Path, items: &[Value]) {
    let mut t = String::new();
    for v in items {
        t.push_str(&v.to_string());
        t.push('\n');
    }
    let _ = bridge_core::write_atomic(p, &t);
}

fn remember(list: &Mutex<VecDeque<String>>, id: &str) -> bool {
    if id.is_empty() {
        return true;
    }
    let mut g = list.lock();
    if g.iter().any(|x| x == id) {
        return false;
    }
    g.push_back(id.to_string());
    while g.len() > 3000 {
        g.pop_front();
    }
    true
}

/// Размер сообщения для журнала (содержимое в журнал не пишем).
fn approx_bytes(v: &Value) -> usize {
    v.to_string().len()
}

impl Assistant {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.lock().join(name)
    }

    pub fn load(&self, node: &Node) {
        let dir = node.data_dir.join("assistant");
        let _ = std::fs::create_dir_all(&dir);
        *self.dir.lock() = dir;
        if let Ok(t) = std::fs::read_to_string(self.path("state.json")) {
            if let Ok(v) = serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')) {
                self.seq.store(v["seq"].as_u64().unwrap_or(0), Ordering::SeqCst);
                self.acked.store(v["acked"].as_u64().unwrap_or(0), Ordering::SeqCst);
                self.seen.store(v["seen"].as_bool().unwrap_or(false), Ordering::SeqCst);
                self.read_n.store(v["readN"].as_u64().unwrap_or(0), Ordering::SeqCst);
                self.msg_n.store(v["msgN"].as_u64().unwrap_or(0), Ordering::SeqCst);
                if let Some(t) = v["title"].as_str().filter(|t| !t.is_empty()) {
                    *self.title.lock() = t.into();
                }
            }
        }
        let inbound = read_jsonl(&self.path("inbound.jsonl"));
        for v in &inbound {
            remember(&self.ids, v["id"].as_str().unwrap_or(""));
        }
        *self.inbound.lock() = inbound;
        let hist = read_jsonl(&self.path("history.jsonl"));
        self.hist_n.store(hist.iter().filter_map(|v| v["n"].as_u64()).max().unwrap_or(0), Ordering::SeqCst);
        for v in &hist {
            if v["dir"] == "in" {
                remember(&self.reply_ids, v["id"].as_str().unwrap_or(""));
            }
        }
        *self.history.lock() = hist;
        *self.outbox.lock() = read_jsonl(&self.path("outbox.jsonl"));
        if let Ok(t) = std::fs::read_to_string(self.path("queued.json")) {
            if let Ok(v) = serde_json::from_str(&t) {
                *self.queued.lock() = v;
            }
        }
    }

    fn save_state(&self) {
        let v = json!({
            "seq": self.seq.load(Ordering::SeqCst), "acked": self.acked.load(Ordering::SeqCst), "seen": self.seen.load(Ordering::SeqCst),
            "readN": self.read_n.load(Ordering::SeqCst), "msgN": self.msg_n.load(Ordering::SeqCst), "title": *self.title.lock(),
        });
        let _ = bridge_core::write_atomic(&self.path("state.json"), &v.to_string());
    }

    fn save_inbound(&self) {
        write_jsonl(&self.path("inbound.jsonl"), &self.inbound.lock());
    }
    fn save_history(&self) {
        write_jsonl(&self.path("history.jsonl"), &self.history.lock());
    }
    fn save_outbox(&self) {
        write_jsonl(&self.path("outbox.jsonl"), &self.outbox.lock());
    }
    fn save_queued(&self) {
        let _ = bridge_core::write_atomic(&self.path("queued.json"), &serde_json::to_string(&*self.queued.lock()).unwrap_or_default());
    }

    // ───────────── хозяин ─────────────

    /// Ассистент хоть раз подключался к этому мосту — здесь хозяин.
    pub fn is_host(&self) -> bool {
        self.seen.load(Ordering::SeqCst)
    }

    pub fn online(&self) -> bool {
        !self.offline.load(Ordering::SeqCst) && self.alive.lock().map(|t| t.elapsed() < ALIVE).unwrap_or(false)
    }

    pub fn title(&self) -> String {
        self.title.lock().clone()
    }

    fn touch(&self) {
        *self.alive.lock() = Some(Instant::now());
        self.offline.store(false, Ordering::SeqCst);
        if !self.seen.swap(true, Ordering::SeqCst) {
            self.save_state();
        }
    }

    pub fn info(&self) -> Value {
        json!({ "version": VERSION, "seq": self.seq.load(Ordering::SeqCst) })
    }

    pub fn presence(&self, node: &Node, v: &Value) {
        let was = self.online();
        if let Some(t) = v["title"].as_str().filter(|t| !t.trim().is_empty()) {
            *self.title.lock() = t.trim().chars().take(40).collect();
        }
        if v["online"].as_bool() == Some(false) {
            *self.alive.lock() = None;
            self.offline.store(true, Ordering::SeqCst);
        } else {
            self.touch();
        }
        self.save_state();
        if was != self.online() {
            node.info(&format!("Ассистент {}", if self.online() { "на связи" } else { "не на связи" }));
            node.emit("assistant-state", json!({ "online": self.online() }));
            self.hist_notify.notify_waiters();
        }
    }

    /// Входящее в очередь хозяина. Повтор того же id — не дубль (Ok(None)).
    pub fn push_inbound(&self, node: &Node, mut item: Value, peer: &str) -> Result<Option<u64>, String> {
        if !item.is_object() {
            return Err("сообщение — объект JSON".into());
        }
        let id = item["id"].as_str().unwrap_or("").to_string();
        if id.is_empty() {
            return Err("нет id".into());
        }
        let kind = item["kind"].as_str().unwrap_or("").to_string();
        if !["text", "voice", "file", "action"].contains(&kind.as_str()) {
            return Err(format!("неизвестный kind «{}»", kind));
        }
        if let Some(files) = item["files"].as_array() {
            for f in files {
                if f["data"].as_str().map(|d| d.len() / 4 * 3).unwrap_or(0) > FILE_MAX + 4 {
                    return Err(format!("файл «{}» больше 10 МБ", f["name"].as_str().unwrap_or("")));
                }
            }
        }
        if !remember(&self.ids, &id) {
            return Ok(None);
        }
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let o = item.as_object_mut().unwrap();
        o.insert("seq".into(), json!(seq));
        o.insert("peer".into(), json!(peer));
        if !o.contains_key("time") {
            o.insert("time".into(), json!(time::format(&time::now())));
        }
        if !o.contains_key("replyTo") {
            o.insert("replyTo".into(), Value::Null);
        }
        let size = approx_bytes(&item);
        {
            let mut g = self.inbound.lock();
            g.push(item);
            let cut = now_unix() - INBOUND_DAYS * 86400;
            g.retain(|v| v["time"].as_str().and_then(time::parse).map(|t| t.timestamp() > cut).unwrap_or(true));
            let n = g.len();
            if n > INBOUND_MAX {
                g.drain(0..n - INBOUND_MAX);
            }
        }
        self.save_inbound();
        self.save_state();
        node.info(&format!("Сообщение ассистенту от {} ({}), {} байт", peer, kind, size));
        node.emit("assistant-in", json!({ "peer": peer, "kind": kind, "id": id, "seq": seq }));
        self.in_notify.notify_waiters();
        Ok(Some(seq))
    }

    /// Долгий опрос: всё с seq > after; `after` — подтверждение (старше можно чистить).
    pub async fn wait(&self, after: u64, timeout: Duration) -> Value {
        self.touch();
        if after > self.acked.load(Ordering::SeqCst) {
            self.acked.store(after, Ordering::SeqCst);
            let removed = {
                let mut g = self.inbound.lock();
                let n = g.len();
                g.retain(|v| v["seq"].as_u64().unwrap_or(0) > after);
                n != g.len()
            };
            if removed {
                self.save_inbound();
            }
            self.save_state();
        }
        let deadline = Instant::now() + timeout;
        loop {
            let notified = self.in_notify.notified();
            let items: Vec<Value> = self.inbound.lock().iter().filter(|v| v["seq"].as_u64().unwrap_or(0) > after).cloned().collect();
            if !items.is_empty() {
                return json!({ "seq": self.seq.load(Ordering::SeqCst), "items": items });
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return json!({ "seq": self.seq.load(Ordering::SeqCst), "items": [] });
            }
            let _ = tokio::time::timeout(left.min(Duration::from_secs(20)), notified).await;
            self.touch();
        }
    }

    // ───────────── у всех: чат ─────────────

    fn push_history(&self, mut v: Value) -> Value {
        let n = self.hist_n.fetch_add(1, Ordering::SeqCst) + 1;
        v["n"] = json!(n);
        v["first"] = json!(n);
        {
            let mut g = self.history.lock();
            g.push(v.clone());
            let len = g.len();
            if len > HISTORY_MAX {
                g.drain(0..len - HISTORY_MAX);
            }
        }
        self.save_history();
        self.hist_notify.notify_waiters();
        v
    }

    /// Поменять запись истории (статус, нажатая кнопка): она получает новый номер — панель увидит изменение.
    fn update_history(&self, id: &str, f: impl FnOnce(&mut Value)) {
        let changed = {
            let mut g = self.history.lock();
            match g.iter().position(|v| v["id"] == id) {
                Some(i) => {
                    let mut v = g.remove(i);
                    f(&mut v);
                    v["n"] = json!(self.hist_n.fetch_add(1, Ordering::SeqCst) + 1);
                    g.push(v);
                    true
                }
                None => false,
            }
        };
        if changed {
            self.save_history();
            self.hist_notify.notify_waiters();
        }
    }

    pub fn new_id(&self, node: &Node) -> String {
        let n = self.msg_n.fetch_add(1, Ordering::SeqCst) + 1;
        self.save_state();
        format!("{}-{}-{:04}", node.node_name(), chrono::Local::now().format("%Y%m%d"), n)
    }

    pub fn unread(&self) -> u64 {
        let r = self.read_n.load(Ordering::SeqCst);
        self.history.lock().iter().filter(|v| v["dir"] == "in" && v["first"].as_u64().or(v["n"].as_u64()).unwrap_or(0) > r).count() as u64
    }

    pub fn mark_read(&self) {
        self.read_n.store(self.hist_n.load(Ordering::SeqCst), Ordering::SeqCst);
        self.save_state();
        self.hist_notify.notify_waiters();
    }

    pub fn typing(&self) -> bool {
        self.typing.lock().as_ref().map(|(_, t)| t.elapsed() < Duration::from_secs(60)).unwrap_or(false)
    }

    pub fn history_after(&self, after: u64) -> Vec<Value> {
        self.history.lock().iter().filter(|v| v["n"].as_u64().unwrap_or(0) > after).cloned().collect()
    }

    pub fn pending(&self) -> usize {
        self.outbox.lock().len()
    }

    /// Ответ ассистента пришёл на этот ПК: в историю, файлы — в «приём/ассистент», уведомление.
    pub fn deliver_local(&self, node: &Node, body: &Value) -> Result<(), String> {
        let kind = body["kind"].as_str().unwrap_or("reply").to_string();
        let reply_to = body["replyTo"].as_str().unwrap_or("").to_string();
        if kind == "progress" {
            *self.typing.lock() = Some((reply_to, Instant::now()));
            self.hist_notify.notify_waiters();
            node.emit("assistant-typing", json!({}));
            return Ok(());
        }
        let id = body["id"].as_str().map(String::from).unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        if !remember(&self.reply_ids, &id) {
            return Ok(());
        }
        {
            let mut t = self.typing.lock();
            if t.as_ref().map(|(r, _)| r.is_empty() || *r == reply_to || reply_to.is_empty()).unwrap_or(false) {
                *t = None;
            }
        }
        let mut files = Vec::new();
        if let Some(list) = body["files"].as_array() {
            let dir = node.mailbox.in_root(node).join("ассистент");
            let _ = std::fs::create_dir_all(&dir);
            for f in list {
                use base64::Engine;
                let name = crate::mobile::folder_name(f["name"].as_str().unwrap_or("файл"));
                let Ok(data) = base64::engine::general_purpose::STANDARD.decode(f["data"].as_str().unwrap_or("")) else { continue };
                let mut full = dir.join(&name);
                if full.exists() {
                    let stem = full.file_stem().and_then(|s| s.to_str()).unwrap_or("файл").to_string();
                    let ext = full.extension().and_then(|s| s.to_str()).map(|e| format!(".{}", e)).unwrap_or_default();
                    full = dir.join(format!("{} ({}){}", stem, chrono::Local::now().format("%H%M%S"), ext));
                }
                if std::fs::write(&full, &data).is_ok() {
                    let fname = full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
                    files.push(json!({ "name": fname, "size": data.len(), "mime": f["mime"], "path": format!("ассистент/{}", fname) }));
                }
            }
        }
        let text = body["text"].as_str().unwrap_or("").to_string();
        let notice = kind == "notice";
        let alarm = notice && text.trim_start().starts_with('⏰');
        self.push_history(json!({
            "id": id, "time": time::format(&time::now()), "dir": "in", "kind": kind, "text": text, "files": files,
            "buttons": body["buttons"].clone(), "replyTo": if reply_to.is_empty() { Value::Null } else { json!(reply_to) }, "alarm": alarm,
        }));
        node.info(&format!("Ответ ассистента ({}), {} байт", kind, approx_bytes(body)));
        let short: String = text.chars().take(160).collect();
        node.emit("assistant-msg", json!({ "id": id, "kind": kind, "notice": notice, "alarm": alarm, "text": short, "unread": self.unread() }));
        Ok(())
    }

    /// Сообщение из панели (или телефона): в историю и хозяину — напрямую или через очередь.
    pub fn send(&self, node: &Arc<Node>, req: &Value) -> Result<Value, String> {
        let kind = req["kind"].as_str().unwrap_or("text").to_string();
        let id = self.new_id(node);
        let mut item = json!({ "id": id, "time": time::format(&time::now()), "kind": kind, "text": req["text"].as_str().unwrap_or("") });
        let mut hist = json!({ "id": id, "time": item["time"], "dir": "out", "kind": kind, "text": item["text"], "status": "pending" });
        if let Some(r) = req["replyTo"].as_str().filter(|r| !r.is_empty()) {
            item["replyTo"] = json!(r);
            hist["replyTo"] = json!(r);
        }
        match kind.as_str() {
            "text" => {
                if item["text"].as_str().unwrap_or("").trim().is_empty() {
                    return Err("пустое сообщение".into());
                }
            }
            "file" => {
                let files = req["files"].as_array().cloned().unwrap_or_default();
                if files.is_empty() {
                    return Err("нет файлов".into());
                }
                hist["files"] = json!(files.iter().map(|f| json!({ "name": f["name"], "size": f["size"] })).collect::<Vec<_>>());
                item["files"] = json!(files);
            }
            "voice" => {
                let v = req["voice"].clone();
                if v["data"].as_str().unwrap_or("").is_empty() {
                    return Err("нет записи".into());
                }
                hist["voice"] = json!({ "mime": v["mime"], "seconds": req["seconds"], "size": v["data"].as_str().map(|d| d.len() / 4 * 3) });
                item["voice"] = v;
            }
            "action" => {
                let a = req["action"].clone();
                if a["name"].as_str().unwrap_or("").is_empty() {
                    return Err("нет action.name".into());
                }
                hist["action"] = a.clone();
                hist["text"] = json!(req["label"].as_str().unwrap_or(a["name"].as_str().unwrap_or("")));
                item["action"] = a;
                // кнопки этого сообщения больше не нажимаются
                if let Some(r) = req["replyTo"].as_str() {
                    let label = hist["text"].clone();
                    self.update_history(r, |v| v["pressed"] = label);
                }
            }
            _ => return Err(format!("неизвестный kind «{}»", kind)),
        }
        let me = node.node_name();
        if self.is_host() {
            self.push_inbound(node, item, &me)?;
            hist["status"] = json!("sent");
            return Ok(self.push_history(hist));
        }
        if host_peer(node).is_none() {
            return Err("ассистента не видно ни на этом ПК, ни у соседей".into());
        }
        self.outbox.lock().push(item);
        self.save_outbox();
        let out = self.push_history(hist);
        self.kick.notify_one();
        Ok(out)
    }

    pub fn mark_sent(&self, id: &str, status: &str, error: &str) {
        self.update_history(id, |v| {
            v["status"] = json!(status);
            if !error.is_empty() {
                v["error"] = json!(error);
            }
        });
    }
}

/// Сосед-хозяин ассистента (из его привета): (имя, название, на связи).
pub fn host_peer(node: &Node) -> Option<(String, String, bool)> {
    let names: Vec<String> = node.cfg.read().peers.iter().filter(|p| p.enabled).map(|p| p.name.clone()).collect();
    let mut found = None;
    for n in names {
        let st = node.peer_state(&n);
        if let Some(a) = st.assistant {
            let cand = (n.clone(), if a.title.is_empty() { "Ассистент".into() } else { a.title }, a.online && st.online);
            if cand.2 {
                return Some(cand);
            }
            found.get_or_insert(cand);
        }
    }
    found
}

/// Где ассистент для панели этого ПК: (хозяин, название, на связи).
pub fn host_of(node: &Node) -> Option<(String, String, bool)> {
    if node.assistant.is_host() {
        return Some((node.node_name(), node.assistant.title(), node.assistant.online()));
    }
    host_peer(node)
}

/// Ответ от ассистента (POST assistant/reply): этому ПК или соседу; сосед не на связи — в очередь.
pub async fn reply(node: &Arc<Node>, body: Value) -> Result<Value, (u16, String)> {
    let to = body["to"].as_str().unwrap_or("").to_string();
    let kind = body["kind"].as_str().unwrap_or("reply").to_string();
    if !["reply", "notice", "progress"].contains(&kind.as_str()) {
        return Err((400, format!("неизвестный kind «{}»", kind)));
    }
    node.assistant.touch();
    let mut msg = body.clone();
    if let Some(o) = msg.as_object_mut() {
        o.remove("to");
    }
    if to.is_empty() || bridge_core::eq_ic(&to, &node.node_name()) {
        node.assistant.deliver_local(node, &msg).map_err(|e| (400, e))?;
        return Ok(json!({ "ok": true, "delivered": true }));
    }
    let Some(peer) = node.cfg.read().peer(&to).cloned() else { return Err((404, format!("нет соседа «{}»", to))) };
    if !peer.enabled || !peer.has_url() {
        return Err((404, format!("сосед «{}» выключен или без адреса", to)));
    }
    match send_reply(node, &peer, &msg).await {
        Ok(()) => Ok(json!({ "ok": true, "delivered": true })),
        Err(e) => {
            if kind == "progress" {
                return Ok(json!({ "ok": true, "delivered": false, "queued": false, "error": e }));
            }
            let ttl = if kind == "notice" { 24 * 3600 } else { 7 * 24 * 3600 };
            node.assistant.queued.lock().entry(peer.name.clone()).or_default().push((msg, now_unix() + ttl));
            node.assistant.save_queued();
            Ok(json!({ "ok": true, "delivered": false, "queued": true }))
        }
    }
}

async fn send_reply(node: &Arc<Node>, peer: &bridge_core::config::PeerConfig, msg: &Value) -> Result<(), String> {
    let client = node.client_for(peer);
    match tokio::time::timeout(Duration::from_secs(30), client.post_json("/peer/assistant/reply", msg)).await {
        Ok(Ok((c, _))) if (200..300).contains(&c) => Ok(()),
        Ok(Ok((c, t))) => Err(crate::client::error_text(&t, c)),
        Ok(Err(e)) => Err(crate::link::explain(&e, peer).0),
        Err(_) => Err("сосед не ответил вовремя".into()),
    }
}

/// Фоновая доставка: клиент — свои сообщения хозяину; хозяин — отложенные ответы соседям.
pub fn schedule(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        loop {
            let kick = node.assistant.kick.notified();
            flush_outbox(&node).await;
            flush_queued(&node).await;
            tokio::select! {
                _ = node.shutdown.cancelled() => break,
                _ = kick => {}
                _ = tokio::time::sleep(Duration::from_secs(3)) => {}
            }
        }
    });
}

async fn flush_outbox(node: &Arc<Node>) {
    if node.assistant.outbox.lock().is_empty() {
        return;
    }
    if node.assistant.is_host() {
        // ПК сам стал хозяином — переложить в свою очередь
        let items: Vec<Value> = std::mem::take(&mut *node.assistant.outbox.lock());
        node.assistant.save_outbox();
        let me = node.node_name();
        for it in items {
            let id = it["id"].as_str().unwrap_or("").to_string();
            let _ = node.assistant.push_inbound(node, it, &me);
            node.assistant.mark_sent(&id, "sent", "");
        }
        return;
    }
    let Some((host, _, _)) = host_peer(node) else { return };
    let Some(peer) = node.cfg.read().peer(&host).cloned() else { return };
    if !node.peer_state(&host).online {
        return;
    }
    let client = node.client_for(&peer);
    loop {
        let Some(item) = node.assistant.outbox.lock().first().cloned() else { break };
        let id = item["id"].as_str().unwrap_or("").to_string();
        match tokio::time::timeout(Duration::from_secs(60), client.post_json("/peer/assistant/msg", &item)).await {
            Ok(Ok((c, _))) if (200..300).contains(&c) => {
                node.assistant.outbox.lock().retain(|v| v["id"] != item["id"]);
                node.assistant.save_outbox();
                node.assistant.mark_sent(&id, "sent", "");
            }
            Ok(Ok((c, t))) if (400..500).contains(&c) && c != 408 && c != 429 => {
                let e = crate::client::error_text(&t, c);
                node.info(&format!("Ассистент на «{}» не принял сообщение: {}", host, e));
                node.assistant.outbox.lock().retain(|v| v["id"] != item["id"]);
                node.assistant.save_outbox();
                node.assistant.mark_sent(&id, "failed", &e);
            }
            _ => break,
        }
    }
}

async fn flush_queued(node: &Arc<Node>) {
    let peers: Vec<String> = {
        let now = now_unix();
        let mut g = node.assistant.queued.lock();
        for list in g.values_mut() {
            list.retain(|(_, until)| *until > now);
        }
        g.retain(|_, l| !l.is_empty());
        g.keys().cloned().collect()
    };
    for name in peers {
        if !node.peer_state(&name).online {
            continue;
        }
        let Some(peer) = node.cfg.read().peer(&name).cloned() else { continue };
        loop {
            let Some((msg, _)) = node.assistant.queued.lock().get(&name).and_then(|l| l.first().cloned()) else { break };
            if send_reply(node, &peer, &msg).await.is_err() {
                break;
            }
            if let Some(l) = node.assistant.queued.lock().get_mut(&name) {
                if !l.is_empty() {
                    l.remove(0);
                }
            }
            node.assistant.save_queued();
        }
    }
}

/// Для привета соседу: ассистент здесь, и соседу можно ему писать.
pub fn hello_info(node: &Node, peer: &str) -> Option<bridge_core::proto::AssistantInfo> {
    if !node.assistant.is_host() {
        return None;
    }
    let c = node.cfg.read();
    if !c.list_allows(&c.assistant.allow_from, peer) {
        return None;
    }
    Some(bridge_core::proto::AssistantInfo { host: c.node.clone(), title: node.assistant.title(), online: node.assistant.online(), version: VERSION })
}
