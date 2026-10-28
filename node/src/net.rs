//! Сеансы приложений (3.5): сеть между клиентами одного приложения в голосовом канале.
//!
//! - Сеанс живёт в канале: кто в том же канале, видит его и входит (`joinOrCreate` — войти в идущий или стать хостом).
//! - Хост — мост того, кто создал сеанс: он держит состав, общее состояние и раздаёт сообщения (звезда).
//! - Мосты связаны постоянным WebSocket (подписанный запрос, как всё между соседями); приложение — WebSocket
//!   к своему мосту на 127.0.0.1 (своё окно и служба — по токену приложения, веб-интерфейс — через панель).
//! - Доставка: надёжно (по порядку, без потерь, до 1 МБ), быстро (без повторов: при заторе отбрасывается, до 1200 байт),
//!   последнее по ключу (недоставленное заменяется свежим — позиции, курсоры).
//! - Свой сервер: `transport: "external"` — мост держит лобби и состав, трафик идёт мимо него; строка подключения
//!   от хоста и билет участника (хост проверяет, кто есть кто).
//!
//! Кадр приложения (двоичный): к мосту — [доставка u8][кому u16 LE][ключ u8, только для «последнего»][данные];
//! от моста — [доставка u8][от кого u16 LE][данные]. Кому: 0 — хост, N — участник, 0xFFFF — все, 0xFFFE — все кроме себя.
//! Управление — текстом (JSON): запрос `{op, re}` → ответ `{ev: "re", re, ok, v | error}`, события `{ev, …}`.

use crate::node::Node;
use axum::extract::ws::{Message as AxMsg, WebSocket};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, Notify};

pub const HOST: u16 = 0;
pub const ALL: u16 = 0xFFFF;
pub const OTHERS: u16 = 0xFFFE;
pub const REL: u8 = 0;
pub const FAST: u8 = 1;
pub const LATEST: u8 = 2;
const FAST_MAX: usize = 1200;
const REL_MAX: usize = 1 << 20;
const MAX_MEMBERS: u16 = 64;
/// Сосед молчит дольше — связь с ним считается потерянной.
const LINK_DEAD: Duration = Duration::from_secs(10);

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Clone, Default, Debug)]
pub struct Chan {
    pub room: String,
    pub title: String,
    pub members: Vec<String>,
}

pub enum Out {
    Text(String),
    Bin(Vec<u8>),
}

#[derive(Clone)]
struct Joined {
    sid: String,
    cid: u16,
    host: String,
    room: String,
}

struct Conn {
    app: String,
    perms: Vec<String>,
    tx: mpsc::UnboundedSender<Out>,
    sess: Option<Joined>,
    /// Как назвать участника: имя ПК (и метка экземпляра, если их несколько).
    label: String,
}

#[derive(Clone)]
struct Member {
    cid: u16,
    node: String,
    conn: u64,
    name: String,
    ticket: String,
    /// Ключ пользователя для издателя приложения (3.7) — со слов его моста; пусто без права identity.
    key: String,
}

struct Hosted {
    app: String,
    proto: Value,
    room: String,
    title: String,
    max: u16,
    transport: String,
    connect: String,
    created: i64,
    members: BTreeMap<u16, Member>,
    next_cid: u16,
    state: Value,
    rev: u64,
    meta: Value,
    leave_with_channel: bool,
}

/// Связь с соседом: одна на соседа; сокеты к ней пристёгиваются (новый вытесняет старый), очередь общая.
pub struct Link {
    peer: String,
    q: Mutex<LinkQ>,
    notify: Notify,
    gen: AtomicU64,
    connected: AtomicBool,
    connecting: AtomicBool,
    last_try: Mutex<Option<Instant>>,
    last_seen: Mutex<Instant>,
    rtt: AtomicU32,
    /// Смещение часов соседа относительно наших, мс.
    offset: AtomicI64,
}

#[derive(Default)]
struct LinkQ {
    rel: VecDeque<Out>,
    fast: VecDeque<Vec<u8>>,
    latest: BTreeMap<String, Vec<u8>>,
    latest_order: VecDeque<String>,
}

#[derive(Default)]
pub struct Net {
    chan: Mutex<Chan>,
    hosted: Mutex<BTreeMap<String, Hosted>>,
    remote: Mutex<BTreeMap<String, (Value, Instant)>>,
    conns: Mutex<BTreeMap<u64, Conn>>,
    next: AtomicU64,
    links: Mutex<BTreeMap<String, Arc<Link>>>,
    joins: Mutex<BTreeMap<u64, oneshot::Sender<Result<Value, String>>>>,
    tickets: Mutex<BTreeMap<String, Value>>,
    panel_tickets: Mutex<BTreeMap<String, (String, Instant)>>,
    dev: Mutex<BTreeMap<String, Value>>,
}

// ───────────── канал ─────────────

/// Панель сообщает, в каком канале этот ПК (пусто — ни в каком) и кто в нём.
pub fn set_channel(node: &Arc<Node>, room: &str, title: &str, members: Vec<String>) {
    let me = node.node_name();
    let members: Vec<String> = members.into_iter().filter(|m| !m.is_empty() && !bridge_core::eq_ic(m, &me)).collect();
    let changed_room = {
        let mut c = node.net.chan.lock();
        let changed = c.room != room;
        *c = Chan { room: room.to_string(), title: title.to_string(), members };
        changed
    };
    if changed_room {
        // вышли из канала — сеансы того канала: свои (хост) заканчиваются, в чужих — выходим
        let ended: Vec<String> = node.net.hosted.lock().iter().filter(|(_, h)| h.room != room && h.leave_with_channel).map(|(s, _)| s.clone()).collect();
        for sid in ended {
            end(node, &sid, "хост вышел из канала");
        }
        let leave: Vec<u64> = node.net.conns.lock().iter().filter(|(_, c)| c.sess.as_ref().map(|s| s.room != room).unwrap_or(false)).map(|(id, _)| *id).collect();
        for id in leave {
            let lwc = node.net.conns.lock().get(&id).map(|c| leave_with_channel(node, &c.app)).unwrap_or(true);
            if lwc {
                leave_session(node, id, "вы вышли из канала");
            }
        }
        for (id, c) in node.net.conns.lock().iter() {
            let _ = id;
            send_text(&c.tx, json!({ "ev": "channel", "room": room, "title": title }));
        }
        node.net.remote.lock().clear();
    }
    announce_all(node);
}

pub fn channel(node: &Node) -> Chan {
    node.net.chan.lock().clone()
}

/// Приложение `app` сейчас в сеансе в моём канале, а `peer` — в этом же канале: ему можно отдать пакет по просьбе («Взять у хоста»).
pub fn shared_in_channel(node: &Node, app: &str, peer: &str) -> bool {
    let ch = channel(node);
    if ch.room.is_empty() || !ch.members.iter().any(|m| bridge_core::eq_ic(m, peer)) {
        return false;
    }
    node.net.hosted.lock().values().any(|h| h.app == app && h.room == ch.room) || node.net.conns.lock().values().any(|c| c.app == app && c.sess.as_ref().map(|s| s.room == ch.room).unwrap_or(false))
}

fn leave_with_channel(node: &Node, app: &str) -> bool {
    crate::apps::get(node, app).map(|a| a.manifest["net"]["leaveWithChannel"] != false).unwrap_or(true)
}

// ───────────── подключения приложений ─────────────

fn send_text(tx: &mpsc::UnboundedSender<Out>, v: Value) {
    let _ = tx.send(Out::Text(v.to_string()));
}

fn conn_send(node: &Node, conn: u64, v: Value) {
    if let Some(c) = node.net.conns.lock().get(&conn) {
        send_text(&c.tx, v);
    }
}

/// Одноразовый пропуск для панели: веб-приложение `app` подключается к сети через неё.
pub fn panel_ticket(node: &Node, app: &str) -> String {
    let t = hex::encode(rand::random::<[u8; 16]>());
    let mut g = node.net.panel_tickets.lock();
    g.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(60));
    g.insert(t.clone(), (app.to_string(), Instant::now()));
    t
}

pub fn take_panel_ticket(node: &Node, t: &str) -> Option<String> {
    let mut g = node.net.panel_tickets.lock();
    g.remove(t).filter(|(_, at)| at.elapsed() < Duration::from_secs(60)).map(|(a, _)| a)
}

/// Подключение приложения `app` (права — уже проверенные разрешением хозяина).
pub async fn serve_app(node: Arc<Node>, ws: WebSocket, app: String, perms: Vec<String>, label: String) {
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let id = node.net.next.fetch_add(1, Ordering::SeqCst) + 1;
    node.net.conns.lock().insert(id, Conn { app: app.clone(), perms, tx: tx.clone(), sess: None, label });
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let r = match m {
                Out::Text(t) => sink.send(AxMsg::Text(t)).await,
                Out::Bin(b) => sink.send(AxMsg::Binary(b)).await,
            };
            if r.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let ch = channel(&node);
    send_text(&tx, json!({ "ev": "hello", "conn": id, "app": app, "node": node.node_name(), "channel": { "room": ch.room, "title": ch.title, "members": ch.members } }));
    while let Some(m) = stream.next().await {
        match m {
            Ok(AxMsg::Text(t)) => {
                let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                let n2 = node.clone();
                // join ждёт ответа хоста — не задерживаем остальные сообщения
                if v["op"] == "join" || v["op"] == "joinOrCreate" {
                    tokio::spawn(async move { conn_op(&n2, id, v).await });
                } else {
                    conn_op(&n2, id, v).await;
                }
            }
            Ok(AxMsg::Binary(b)) => conn_bin(&node, id, &b),
            Ok(AxMsg::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
    leave_session(&node, id, "");
    node.net.conns.lock().remove(&id);
    writer.abort();
}

fn has(node: &Node, conn: u64, perm: &str) -> bool {
    node.net.conns.lock().get(&conn).map(|c| c.perms.iter().any(|p| p == perm)).unwrap_or(false)
}

async fn conn_op(node: &Arc<Node>, conn: u64, v: Value) {
    let op = v["op"].as_str().unwrap_or("").to_string();
    let re = v["re"].clone();
    let r: Result<Value, String> = match op.as_str() {
        "channel" => {
            let c = channel(node);
            Ok(json!({ "room": c.room, "title": c.title, "members": c.members }))
        }
        "list" | "create" | "join" | "joinOrCreate" | "leave" | "presence" | "state" | "connect" | "kick" | "ticket" if !has(node, conn, "sessions") => Err("у приложения нет права «Сетевые сеансы в канале»".into()),
        "list" => Ok(json!(list_for(node, conn))),
        "create" => create(node, conn, &v),
        "join" => join(node, conn, v["sid"].as_str().unwrap_or("")).await,
        "joinOrCreate" => {
            let found = list_for(node, conn).into_iter().find(|s| s["joinable"] == true).and_then(|s| s["sid"].as_str().map(String::from));
            match found {
                Some(sid) => match join(node, conn, &sid).await {
                    Ok(v) => Ok(v),
                    Err(_) => create(node, conn, &v),
                },
                None => create(node, conn, &v),
            }
        }
        "leave" => {
            leave_session(node, conn, "");
            Ok(json!(true))
        }
        "presence" | "state" | "connect" | "kick" => to_host_ctl(node, conn, &op, &v),
        "activity.since" | "activity.stats" if !has(node, conn, "activity") => Err("у приложения нет права «Видеть вашу активность в мосте»".into()),
        "activity.since" => Ok(crate::activity::since(node, v["cursor"].as_u64().unwrap_or(0), v["limit"].as_u64().unwrap_or(500).min(2000) as usize)),
        "activity.stats" => Ok(crate::activity::stats(node, v["period"].as_str().unwrap_or("today"))),
        "time" => Ok(json!({ "now": now_ms() })),
        _ => Err(format!("нет такой команды: {}", op)),
    };
    if !re.is_null() {
        conn_send(node, conn, match r {
            Ok(v) => json!({ "ev": "re", "re": re, "ok": true, "v": v }),
            Err(e) => json!({ "ev": "re", "re": re, "ok": false, "error": e }),
        });
    }
}

/// Сеансы этого приложения в моём канале.
fn list_for(node: &Node, conn: u64) -> Vec<Value> {
    let Some(app) = node.net.conns.lock().get(&conn).map(|c| c.app.clone()) else { return vec![] };
    let room = channel(node).room;
    if room.is_empty() {
        return vec![];
    }
    let proto = crate::apps::get(node, &app).map(|a| a.manifest["net"]["proto"].clone()).unwrap_or(Value::Null);
    let mut out = Vec::new();
    for (sid, h) in node.net.hosted.lock().iter() {
        if h.app == app && h.room == room {
            out.push(summary(node, sid, h, &proto));
        }
    }
    for (_, (s, at)) in node.net.remote.lock().iter() {
        if s["app"] == app.as_str() && s["room"] == room.as_str() && at.elapsed() < Duration::from_secs(12) {
            let mut s = s.clone();
            s["joinable"] = json!(s["proto"] == proto && s["count"].as_u64().unwrap_or(0) < s["max"].as_u64().unwrap_or(0));
            out.push(s);
        }
    }
    out.sort_by_key(|s| s["created"].as_i64().unwrap_or(0));
    out
}

fn summary(node: &Node, sid: &str, h: &Hosted, proto: &Value) -> Value {
    json!({
        "sid": sid, "app": h.app, "proto": h.proto, "room": h.room, "title": h.title, "host": node.node_name(),
        "name": crate::apps::get(node, &h.app).map(|a| a.name).unwrap_or_default(), "version": crate::apps::get(node, &h.app).map(|a| a.version).unwrap_or_default(),
        "count": h.members.len(), "max": h.max, "transport": h.transport, "created": h.created, "meta": h.meta,
        "members": h.members.values().map(|m| m.name.clone()).collect::<Vec<_>>(),
        "joinable": h.proto == *proto && (h.members.len() as u16) < h.max,
    })
}

fn create(node: &Arc<Node>, conn: u64, v: &Value) -> Result<Value, String> {
    if node.remote.active().is_some() {
        return Err("пока этим ПК управляют по сети, сеансы не начинаются".into());
    }
    leave_session(node, conn, "");
    let ch = channel(node);
    if ch.room.is_empty() {
        return Err("сеанс начинается в голосовом канале — зайдите в канал".into());
    }
    let (app, label) = node.net.conns.lock().get(&conn).map(|c| (c.app.clone(), c.label.clone())).ok_or("подключение закрыто")?;
    let a = crate::apps::get(node, &app).ok_or("нет такого приложения")?;
    let net = &a.manifest["net"];
    let max = v["max"].as_u64().or(net["max"].as_u64()).unwrap_or(8).clamp(1, MAX_MEMBERS as u64) as u16;
    let transport = if v["transport"] == "external" || net["transport"] == "external" { "external" } else { "bridge" };
    let sid = hex::encode(rand::random::<[u8; 8]>());
    let ticket = hex::encode(rand::random::<[u8; 16]>());
    let me = node.node_name();
    let mut members = BTreeMap::new();
    let key = crate::identity::app_key(node, &app);
    members.insert(HOST, Member { cid: HOST, node: me.clone(), conn, name: label.clone(), ticket: ticket.clone(), key: key.clone() });
    node.net.tickets.lock().insert(ticket.clone(), json!({ "sid": sid, "app": app, "cid": HOST, "node": me, "name": label, "key": key }));
    let h = Hosted {
        app: app.clone(),
        proto: net["proto"].clone(),
        room: ch.room.clone(),
        title: ch.title.clone(),
        max,
        transport: transport.into(),
        connect: String::new(),
        created: now_ms(),
        members,
        next_cid: 1,
        state: if v["state"].is_object() { v["state"].clone() } else { json!({}) },
        rev: 0,
        meta: v["meta"].clone(),
        leave_with_channel: net["leaveWithChannel"] != false,
    };
    let snap = snapshot(&sid, &h, HOST, &me);
    node.net.hosted.lock().insert(sid.clone(), h);
    if let Some(c) = node.net.conns.lock().get_mut(&conn) {
        c.sess = Some(Joined { sid: sid.clone(), cid: HOST, host: me.clone(), room: ch.room.clone() });
    }
    node.info(&format!("[сеансы] «{}»: сеанс {} в канале «{}» (хост — этот ПК)", a.name, sid, ch.title));
    announce_all(node);
    Ok(snap)
}

fn snapshot(sid: &str, h: &Hosted, you: u16, host: &str) -> Value {
    let ticket = h.members.get(&you).map(|m| m.ticket.clone()).unwrap_or_default();
    json!({
        "sid": sid, "app": h.app, "room": h.room, "title": h.title, "host": host, "you": you, "isHost": you == HOST,
        "members": h.members.values().map(|m| json!({ "cid": m.cid, "node": m.node, "name": m.name, "key": m.key })).collect::<Vec<_>>(),
        "state": h.state, "rev": h.rev, "connect": h.connect, "transport": h.transport, "ticket": ticket, "max": h.max,
        "hostNow": now_ms(), "created": h.created, "meta": h.meta,
    })
}

async fn join(node: &Arc<Node>, conn: u64, sid: &str) -> Result<Value, String> {
    if node.remote.active().is_some() {
        return Err("пока этим ПК управляют по сети, сеансы недоступны".into());
    }
    leave_session(node, conn, "");
    let (app, label) = node.net.conns.lock().get(&conn).map(|c| (c.app.clone(), c.label.clone())).ok_or("подключение закрыто")?;
    let room = channel(node).room;
    let proto = crate::apps::get(node, &app).map(|a| a.manifest["net"]["proto"].clone()).unwrap_or(Value::Null);
    let me = node.node_name();
    // свой сеанс (второй экземпляр на этом же ПК)
    if node.net.hosted.lock().contains_key(sid) {
        let snap = host_join(node, sid, &me, conn, &label, &app, &proto, &room, &crate::identity::app_key(node, &app))?;
        if let Some(c) = node.net.conns.lock().get_mut(&conn) {
            c.sess = Some(Joined { sid: sid.into(), cid: snap["you"].as_u64().unwrap_or(0) as u16, host: me.clone(), room: room.clone() });
        }
        return Ok(snap);
    }
    let host = node.net.remote.lock().get(sid).and_then(|(s, _)| s["host"].as_str().map(String::from)).ok_or("сеанс закончился или недоступен")?;
    let (tx, rx) = oneshot::channel();
    node.net.joins.lock().insert(conn, tx);
    link_text(node, &host, json!({ "t": "join", "sid": sid, "conn": conn, "name": label, "app": app, "proto": proto, "room": room, "key": crate::identity::app_key(node, &app) }));
    let r = match tokio::time::timeout(Duration::from_secs(8), rx).await {
        Ok(Ok(r)) => r,
        _ => {
            node.net.joins.lock().remove(&conn);
            Err("хост сеанса не ответил".into())
        }
    }?;
    if let Some(c) = node.net.conns.lock().get_mut(&conn) {
        c.sess = Some(Joined { sid: sid.into(), cid: r["you"].as_u64().unwrap_or(0) as u16, host: host.clone(), room: room.clone() });
    }
    Ok(r)
}

/// У хоста: впустить участника (с этого ПК или с соседа).
#[allow(clippy::too_many_arguments)]
fn host_join(node: &Arc<Node>, sid: &str, from_node: &str, conn: u64, name: &str, app: &str, proto: &Value, room: &str, key: &str) -> Result<Value, String> {
    let me = node.node_name();
    let (snap, new_member) = {
        let mut g = node.net.hosted.lock();
        let h = g.get_mut(sid).ok_or("сеанс закончился")?;
        if h.app != app {
            return Err("это сеанс другого приложения".into());
        }
        if h.proto != *proto {
            return Err("версия протокола приложения не совпадает с хостом — обновите приложение («Взять у хозяина»)".into());
        }
        if h.room != room {
            return Err("войти можно только из того же канала".into());
        }
        if h.members.len() as u16 >= h.max {
            return Err(format!("сеанс полон ({} из {})", h.members.len(), h.max));
        }
        let mut cid = h.next_cid;
        while h.members.contains_key(&cid) || cid == HOST || cid >= OTHERS {
            cid = cid.wrapping_add(1).max(1);
        }
        h.next_cid = cid.wrapping_add(1).max(1);
        let ticket = hex::encode(rand::random::<[u8; 16]>());
        let key: String = key.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(43).collect();
        let m = Member { cid, node: from_node.to_string(), conn, name: name.to_string(), ticket: ticket.clone(), key };
        node.net.tickets.lock().insert(ticket, json!({ "sid": sid, "app": app, "cid": cid, "node": from_node, "name": name, "key": m.key }));
        h.members.insert(cid, m.clone());
        (snapshot(sid, h, cid, &me), m)
    };
    broadcast(node, sid, json!({ "ev": "member", "op": "join", "m": { "cid": new_member.cid, "node": new_member.node, "name": new_member.name, "key": new_member.key } }), Some(new_member.cid));
    announce_all(node);
    Ok(snap)
}

/// Выйти из сеанса (закрыли приложение, вышли из канала, сменили сеанс).
fn leave_session(node: &Arc<Node>, conn: u64, why: &str) {
    let s = node.net.conns.lock().get_mut(&conn).and_then(|c| c.sess.take());
    let Some(s) = s else { return };
    if !why.is_empty() {
        conn_send(node, conn, json!({ "ev": "ended", "why": why }));
    }
    if s.host == node.node_name() {
        host_leave(node, &s.sid, s.cid, "");
    } else {
        link_text(node, &s.host, json!({ "t": "leave", "sid": s.sid, "cid": s.cid }));
    }
}

fn host_leave(node: &Arc<Node>, sid: &str, cid: u16, why: &str) {
    if cid == HOST {
        end(node, sid, if why.is_empty() { "хост вышел из сеанса" } else { why });
        return;
    }
    let removed = {
        let mut g = node.net.hosted.lock();
        g.get_mut(sid).and_then(|h| h.members.remove(&cid))
    };
    if let Some(m) = removed {
        node.net.tickets.lock().remove(&m.ticket);
        broadcast(node, sid, json!({ "ev": "member", "op": "leave", "cid": cid, "why": why }), None);
        announce_all(node);
    }
}

/// Закончить свой сеанс: всем участникам — «сеанс закончился».
fn end(node: &Arc<Node>, sid: &str, why: &str) {
    let h = node.net.hosted.lock().remove(sid);
    let Some(h) = h else { return };
    let me = node.node_name();
    for m in h.members.values() {
        node.net.tickets.lock().remove(&m.ticket);
        if m.node == me {
            let mut g = node.net.conns.lock();
            if let Some(c) = g.get_mut(&m.conn) {
                if c.sess.as_ref().map(|s| s.sid == sid).unwrap_or(false) {
                    c.sess = None;
                    send_text(&c.tx, json!({ "ev": "ended", "why": why }));
                }
            }
        } else {
            link_text(node, &m.node, json!({ "t": "ended", "sid": sid, "why": why, "conn": m.conn }));
        }
    }
    for p in channel(node).members {
        link_text(node, &p, json!({ "t": "gone", "sid": sid }));
    }
    node.info(&format!("[сеансы] сеанс {} закончился: {}", sid, why));
}

/// Событие всем участникам сеанса (кроме `except`).
fn broadcast(node: &Arc<Node>, sid: &str, ev: Value, except: Option<u16>) {
    let members: Vec<Member> = node.net.hosted.lock().get(sid).map(|h| h.members.values().cloned().collect()).unwrap_or_default();
    let me = node.node_name();
    for m in members {
        if Some(m.cid) == except {
            continue;
        }
        if m.node == me {
            conn_send(node, m.conn, ev.clone());
        } else {
            link_text(node, &m.node, json!({ "t": "ev", "sid": sid, "conn": m.conn, "ev": ev }));
        }
    }
}

/// Присутствие, общее состояние, строка подключения, выгнать — через хоста.
fn to_host_ctl(node: &Arc<Node>, conn: u64, op: &str, v: &Value) -> Result<Value, String> {
    let s = node.net.conns.lock().get(&conn).and_then(|c| c.sess.clone()).ok_or("вы не в сеансе")?;
    if s.host == node.node_name() {
        host_ctl(node, &s.sid, s.cid, op, v)
    } else {
        link_text(node, &s.host, json!({ "t": "ctl", "sid": s.sid, "cid": s.cid, "op": op, "v": v }));
        Ok(json!(true))
    }
}

fn host_ctl(node: &Arc<Node>, sid: &str, cid: u16, op: &str, v: &Value) -> Result<Value, String> {
    match op {
        "presence" => {
            let data = v["data"].clone();
            if data.to_string().len() > 4096 {
                return Err("присутствие — до 4 КБ".into());
            }
            broadcast(node, sid, json!({ "ev": "presence", "cid": cid, "data": data }), Some(cid));
            Ok(json!(true))
        }
        "state" => {
            let ops = v["ops"].as_array().cloned().unwrap_or_default();
            let rev = {
                let mut g = node.net.hosted.lock();
                let h = g.get_mut(sid).ok_or("сеанс закончился")?;
                for o in &ops {
                    apply_op(&mut h.state, o);
                }
                if h.state.to_string().len() > 4 << 20 {
                    return Err("общее состояние больше 4 МБ".into());
                }
                h.rev += 1;
                h.rev
            };
            broadcast(node, sid, json!({ "ev": "state", "ops": ops, "rev": rev, "by": cid }), None);
            Ok(json!({ "rev": rev }))
        }
        "connect" => {
            if cid != HOST {
                return Err("строку подключения задаёт хост".into());
            }
            let value: String = v["value"].as_str().unwrap_or("").chars().take(1000).collect();
            if let Some(h) = node.net.hosted.lock().get_mut(sid) {
                h.connect = value.clone();
            }
            broadcast(node, sid, json!({ "ev": "connect", "value": value }), None);
            Ok(json!(true))
        }
        "kick" => {
            if cid != HOST {
                return Err("выгнать может только хост".into());
            }
            let who = v["cid"].as_u64().unwrap_or(0) as u16;
            if who == HOST {
                return Err("себя не выгнать".into());
            }
            let m = node.net.hosted.lock().get(sid).and_then(|h| h.members.get(&who).cloned());
            if let Some(m) = m {
                let ev = json!({ "ev": "ended", "why": "хост убрал вас из сеанса" });
                if m.node == node.node_name() {
                    if let Some(c) = node.net.conns.lock().get_mut(&m.conn) {
                        c.sess = None;
                        send_text(&c.tx, ev);
                    }
                } else {
                    link_text(node, &m.node, json!({ "t": "ended", "sid": sid, "why": "хост убрал вас из сеанса", "conn": m.conn }));
                }
                host_leave(node, sid, who, "убран хостом");
            }
            Ok(json!(true))
        }
        _ => Err("нет такой команды".into()),
    }
}

/// Изменение общего состояния: {op: set|merge|del, path: "a.b" | ["a","b"], value}.
pub fn apply_op(state: &mut Value, o: &Value) {
    let path: Vec<String> = match &o["path"] {
        Value::Array(a) => a.iter().map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).collect(),
        Value::String(s) if !s.is_empty() => s.split('.').map(String::from).collect(),
        _ => vec![],
    };
    if !state.is_object() {
        *state = json!({});
    }
    if path.is_empty() {
        match o["op"].as_str() {
            Some("set") => *state = if o["value"].is_object() { o["value"].clone() } else { json!({}) },
            Some("merge") => merge(state, &o["value"]),
            _ => {}
        }
        return;
    }
    let mut cur = state;
    for k in &path[..path.len() - 1] {
        if !cur.is_object() {
            *cur = json!({});
        }
        cur = cur.as_object_mut().unwrap().entry(k.clone()).or_insert(json!({}));
    }
    if !cur.is_object() {
        *cur = json!({});
    }
    let last = path.last().unwrap().clone();
    let obj = cur.as_object_mut().unwrap();
    match o["op"].as_str() {
        Some("set") => {
            obj.insert(last, o["value"].clone());
        }
        Some("merge") => {
            let e = obj.entry(last).or_insert(json!({}));
            merge(e, &o["value"]);
        }
        Some("del") => {
            obj.remove(&last);
        }
        _ => {}
    }
}

fn merge(a: &mut Value, b: &Value) {
    match (a.as_object_mut(), b.as_object()) {
        (Some(ao), Some(bo)) => {
            for (k, v) in bo {
                if v.is_null() {
                    ao.remove(k);
                } else {
                    merge(ao.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        _ => *a = b.clone(),
    }
}

// ───────────── данные ─────────────

fn conn_bin(node: &Arc<Node>, conn: u64, b: &[u8]) {
    if b.len() < 3 {
        return;
    }
    let delivery = b[0].min(LATEST);
    let to = u16::from_le_bytes([b[1], b[2]]);
    let (key, payload) = if delivery == LATEST {
        if b.len() < 4 {
            return;
        }
        (b[3], &b[4..])
    } else {
        (0, &b[3..])
    };
    let lim = if delivery == REL { REL_MAX } else { FAST_MAX };
    if payload.len() > lim {
        conn_send(node, conn, json!({ "ev": "error", "error": format!("сообщение больше {} байт — для «{}» это предел", lim, if delivery == REL { "надёжно" } else { "быстро" }) }));
        return;
    }
    let Some(s) = node.net.conns.lock().get(&conn).and_then(|c| c.sess.clone()) else { return };
    if s.host == node.node_name() {
        host_route(node, &s.sid, s.cid, to, delivery, key, payload);
    } else {
        link_data(node, &s.host, &s.sid, s.cid, to, delivery, key, payload);
    }
}

fn app_frame(delivery: u8, from: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(payload.len() + 3);
    f.push(delivery);
    f.extend_from_slice(&from.to_le_bytes());
    f.extend_from_slice(payload);
    f
}

/// У хоста: раздать сообщение адресатам.
fn host_route(node: &Arc<Node>, sid: &str, from: u16, to: u16, delivery: u8, key: u8, payload: &[u8]) {
    let targets: Vec<Member> = {
        let g = node.net.hosted.lock();
        let Some(h) = g.get(sid) else { return };
        match to {
            ALL => h.members.values().cloned().collect(),
            OTHERS => h.members.values().filter(|m| m.cid != from).cloned().collect(),
            n => h.members.get(&n).cloned().into_iter().collect(),
        }
    };
    let me = node.node_name();
    for m in targets {
        if m.node == me {
            if let Some(c) = node.net.conns.lock().get(&m.conn) {
                let _ = c.tx.send(Out::Bin(app_frame(delivery, from, payload)));
            }
        } else {
            link_data(node, &m.node, sid, from, m.cid, delivery, key, payload);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn link_data(node: &Arc<Node>, peer: &str, sid: &str, from: u16, to: u16, delivery: u8, key: u8, payload: &[u8]) {
    let mut f = Vec::with_capacity(payload.len() + sid.len() + 8);
    f.push(sid.len() as u8);
    f.extend_from_slice(sid.as_bytes());
    f.extend_from_slice(&from.to_le_bytes());
    f.extend_from_slice(&to.to_le_bytes());
    f.push(delivery);
    f.push(key);
    f.extend_from_slice(payload);
    let l = link(node, peer);
    {
        let mut q = l.q.lock();
        match delivery {
            REL => q.rel.push_back(Out::Bin(f)),
            FAST => {
                if q.fast.len() >= 256 {
                    q.fast.pop_front();
                }
                q.fast.push_back(f);
            }
            _ => {
                let k = format!("{}/{}/{}/{}", sid, from, to, key);
                if q.latest.insert(k.clone(), f).is_none() {
                    q.latest_order.push_back(k);
                }
            }
        }
    }
    l.notify.notify_one();
}

/// Двоичный кадр от соседа.
fn peer_bin(node: &Arc<Node>, peer: &str, b: &[u8]) {
    let Some(&n) = b.first() else { return };
    let n = n as usize;
    if b.len() < 1 + n + 6 {
        return;
    }
    let Ok(sid) = std::str::from_utf8(&b[1..1 + n]) else { return };
    let o = 1 + n;
    let from = u16::from_le_bytes([b[o], b[o + 1]]);
    let to = u16::from_le_bytes([b[o + 2], b[o + 3]]);
    let delivery = b[o + 4];
    let key = b[o + 5];
    let payload = &b[o + 6..];
    // я хост: проверить, что отправитель — участник с того ПК
    let hosted = node.net.hosted.lock().get(sid).map(|h| h.members.get(&from).map(|m| m.node == peer).unwrap_or(false));
    match hosted {
        Some(true) => host_route(node, sid, from, to, delivery, key, payload),
        Some(false) => {}
        None => {
            // я участник: кадр от хоста — своему экземпляру
            let target = node.net.conns.lock().iter().find(|(_, c)| c.sess.as_ref().map(|s| s.sid == sid && s.cid == to && s.host == peer).unwrap_or(false)).map(|(_, c)| c.tx.clone());
            if let Some(tx) = target {
                let _ = tx.send(Out::Bin(app_frame(delivery, from, payload)));
            }
        }
    }
}

/// Текст от соседа.
fn peer_text(node: &Arc<Node>, peer: &str, v: Value) {
    let sid = v["sid"].as_str().unwrap_or("").to_string();
    match v["t"].as_str().unwrap_or("") {
        "ping" => link_text(node, peer, json!({ "t": "pong", "ts": v["ts"], "now": now_ms() })),
        "pong" => {
            let l = link(node, peer);
            let ts = v["ts"].as_i64().unwrap_or(0);
            let rtt = (now_ms() - ts).max(0);
            l.rtt.store(rtt as u32, Ordering::Relaxed);
            l.offset.store(v["now"].as_i64().unwrap_or(0) + rtt / 2 - now_ms(), Ordering::Relaxed);
        }
        "announce" => {
            let s = v["s"].clone();
            if s["host"] != peer {
                return;
            }
            if let Some(sid) = s["sid"].as_str() {
                node.net.remote.lock().insert(sid.to_string(), (s.clone(), Instant::now()));
            }
        }
        "gone" => {
            node.net.remote.lock().remove(&sid);
        }
        "join" => {
            let conn = v["conn"].as_u64().unwrap_or(0);
            let r = host_join(node, &sid, peer, conn, v["name"].as_str().unwrap_or(peer), v["app"].as_str().unwrap_or(""), &v["proto"], v["room"].as_str().unwrap_or(""), v["key"].as_str().unwrap_or(""));
            link_text(node, peer, match r {
                Ok(snap) => json!({ "t": "joined", "conn": conn, "snap": snap }),
                Err(e) => json!({ "t": "denied", "conn": conn, "error": e }),
            });
        }
        "joined" | "denied" => {
            let conn = v["conn"].as_u64().unwrap_or(0);
            if let Some(tx) = node.net.joins.lock().remove(&conn) {
                let _ = tx.send(if v["t"] == "joined" { Ok(v["snap"].clone()) } else { Err(v["error"].as_str().unwrap_or("отказано").to_string()) });
            }
        }
        "leave" => {
            let cid = v["cid"].as_u64().unwrap_or(0) as u16;
            let ok = node.net.hosted.lock().get(&sid).and_then(|h| h.members.get(&cid)).map(|m| m.node == peer).unwrap_or(false);
            if ok {
                host_leave(node, &sid, cid, "");
            }
        }
        "ctl" => {
            let cid = v["cid"].as_u64().unwrap_or(0) as u16;
            let ok = node.net.hosted.lock().get(&sid).and_then(|h| h.members.get(&cid)).map(|m| m.node == peer).unwrap_or(false);
            if ok {
                if let Err(e) = host_ctl(node, &sid, cid, v["op"].as_str().unwrap_or(""), &v["v"]) {
                    let conn = node.net.hosted.lock().get(&sid).and_then(|h| h.members.get(&cid).map(|m| m.conn)).unwrap_or(0);
                    link_text(node, peer, json!({ "t": "ev", "sid": sid, "conn": conn, "ev": { "ev": "error", "error": e } }));
                }
            }
        }
        "ev" => {
            let conn = v["conn"].as_u64().unwrap_or(0);
            let ok = node.net.conns.lock().get(&conn).and_then(|c| c.sess.as_ref().map(|s| s.sid == sid && s.host == peer)).unwrap_or(false);
            if ok {
                conn_send(node, conn, v["ev"].clone());
            }
        }
        "ended" => {
            let conn = v["conn"].as_u64().unwrap_or(0);
            let mut g = node.net.conns.lock();
            if let Some(c) = g.get_mut(&conn) {
                if c.sess.as_ref().map(|s| s.sid == sid && s.host == peer).unwrap_or(false) {
                    c.sess = None;
                    send_text(&c.tx, json!({ "ev": "ended", "why": v["why"] }));
                }
            }
        }
        _ => {}
    }
}

// ───────────── связи между мостами ─────────────

fn link(node: &Arc<Node>, peer: &str) -> Arc<Link> {
    let l = {
        let mut g = node.net.links.lock();
        g.entry(peer.to_string())
            .or_insert_with(|| {
                Arc::new(Link {
                    peer: peer.to_string(),
                    q: Mutex::new(LinkQ::default()),
                    notify: Notify::new(),
                    gen: AtomicU64::new(0),
                    connected: AtomicBool::new(false),
                    connecting: AtomicBool::new(false),
                    last_try: Mutex::new(None),
                    last_seen: Mutex::new(Instant::now()),
                    rtt: AtomicU32::new(0),
                    offset: AtomicI64::new(0),
                })
            })
            .clone()
    };
    if !l.connected.load(Ordering::SeqCst) {
        dial(node, &l);
    }
    l
}

fn link_text(node: &Arc<Node>, peer: &str, v: Value) {
    let l = link(node, peer);
    {
        let mut q = l.q.lock();
        if q.rel.len() > 20000 {
            q.rel.pop_front();
        }
        q.rel.push_back(Out::Text(v.to_string()));
    }
    l.notify.notify_one();
}

/// Подключиться к соседу (не чаще раза в 2 с).
fn dial(node: &Arc<Node>, l: &Arc<Link>) {
    if l.connecting.swap(true, Ordering::SeqCst) {
        return;
    }
    {
        let mut t = l.last_try.lock();
        if t.map(|t| t.elapsed() < Duration::from_secs(2)).unwrap_or(false) {
            l.connecting.store(false, Ordering::SeqCst);
            return;
        }
        *t = Some(Instant::now());
    }
    let (node, l) = (node.clone(), l.clone());
    tokio::spawn(async move {
        let pc = node.cfg.read().peer(&l.peer).cloned();
        let r: Result<(), String> = async {
            let pc = pc.ok_or("нет такого соседа")?;
            if !pc.enabled {
                return Err("сосед выключен".into());
            }
            let client = node.client_for(&pc);
            let rel = format!("{}/peer/net", bridge_core::API_PREFIX);
            let (url, auth) = client.ws_target(&rel);
            use tokio_tungstenite::tungstenite::client::IntoClientRequest;
            let mut req = url.into_client_request().map_err(|e| e.to_string())?;
            req.headers_mut().insert("Authorization", auth.parse().map_err(|_| "подпись".to_string())?);
            let (ws, _) = tokio::time::timeout(Duration::from_secs(6), tokio_tungstenite::connect_async(req)).await.map_err(|_| "нет ответа".to_string())?.map_err(|e| e.to_string())?;
            run_client_link(node.clone(), l.clone(), ws);
            Ok(())
        }
        .await;
        if let Err(e) = r {
            node.info_once(&format!("netlink|{}", l.peer), &format!("[сеансы] нет связи с «{}»: {}", l.peer, e), 60);
        }
        l.connecting.store(false, Ordering::SeqCst);
    });
}

fn next_out(l: &Link) -> Option<Out> {
    let mut q = l.q.lock();
    if let Some(m) = q.rel.pop_front() {
        return Some(m);
    }
    while let Some(k) = q.latest_order.pop_front() {
        if let Some(b) = q.latest.remove(&k) {
            return Some(Out::Bin(b));
        }
    }
    q.fast.pop_front().map(Out::Bin)
}

fn attach(l: &Arc<Link>) -> u64 {
    let g = l.gen.fetch_add(1, Ordering::SeqCst) + 1;
    l.connected.store(true, Ordering::SeqCst);
    *l.last_seen.lock() = Instant::now();
    l.notify.notify_waiters();
    g
}

fn detach(node: &Arc<Node>, l: &Arc<Link>, gen: u64) {
    if l.gen.load(Ordering::SeqCst) == gen {
        l.connected.store(false, Ordering::SeqCst);
        let _ = node;
    }
}

/// Входящая связь от соседа (он подключился к нам).
pub async fn serve_peer(node: Arc<Node>, peer: String, ws: WebSocket) {
    let l = link(&node, &peer);
    let gen = attach(&l);
    let (mut sink, mut stream) = ws.split();
    let (l2, n2) = (l.clone(), node.clone());
    let writer = tokio::spawn(async move {
        loop {
            if l2.gen.load(Ordering::SeqCst) != gen {
                break;
            }
            match next_out(&l2) {
                Some(Out::Text(t)) => {
                    if sink.send(AxMsg::Text(t)).await.is_err() {
                        break;
                    }
                }
                Some(Out::Bin(b)) => {
                    if sink.send(AxMsg::Binary(b)).await.is_err() {
                        break;
                    }
                }
                None => {
                    let _ = tokio::time::timeout(Duration::from_millis(500), l2.notify.notified()).await;
                }
            }
        }
        let _ = sink.close().await;
        let _ = n2;
    });
    while let Some(m) = stream.next().await {
        if l.gen.load(Ordering::SeqCst) != gen {
            break;
        }
        *l.last_seen.lock() = Instant::now();
        match m {
            Ok(AxMsg::Text(t)) => peer_text(&node, &peer, serde_json::from_str(&t).unwrap_or(Value::Null)),
            Ok(AxMsg::Binary(b)) => peer_bin(&node, &peer, &b),
            Ok(AxMsg::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
    writer.abort();
    detach(&node, &l, gen);
}

/// Исходящая связь (мы подключились к соседу).
fn run_client_link(node: Arc<Node>, l: Arc<Link>, ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>) {
    use tokio_tungstenite::tungstenite::Message as TMsg;
    let gen = attach(&l);
    let (mut sink, mut stream) = ws.split();
    let l2 = l.clone();
    let writer = tokio::spawn(async move {
        loop {
            if l2.gen.load(Ordering::SeqCst) != gen {
                break;
            }
            match next_out(&l2) {
                Some(Out::Text(t)) => {
                    if sink.send(TMsg::Text(t)).await.is_err() {
                        break;
                    }
                }
                Some(Out::Bin(b)) => {
                    if sink.send(TMsg::Binary(b)).await.is_err() {
                        break;
                    }
                }
                None => {
                    let _ = tokio::time::timeout(Duration::from_millis(500), l2.notify.notified()).await;
                }
            }
        }
        let _ = sink.close().await;
    });
    tokio::spawn(async move {
        let peer = l.peer.clone();
        while let Some(m) = stream.next().await {
            if l.gen.load(Ordering::SeqCst) != gen {
                break;
            }
            *l.last_seen.lock() = Instant::now();
            match m {
                Ok(TMsg::Text(t)) => peer_text(&node, &peer, serde_json::from_str(&t).unwrap_or(Value::Null)),
                Ok(TMsg::Binary(b)) => peer_bin(&node, &peer, &b),
                Ok(TMsg::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        writer.abort();
        detach(&node, &l, gen);
    });
}

// ───────────── фон: анонсы, часы, потерянные связи ─────────────

fn announce_all(node: &Arc<Node>) {
    let ch = channel(node);
    let list: Vec<Value> = node.net.hosted.lock().iter().filter(|(_, h)| h.room == ch.room).map(|(sid, h)| summary(node, sid, h, &h.proto)).collect();
    for p in &ch.members {
        for s in &list {
            link_text(node, p, json!({ "t": "announce", "s": s }));
        }
    }
    // приложениям — свежий список (кто ждёт сеансы своего приложения)
    let conns: Vec<(u64, String)> = node.net.conns.lock().iter().filter(|(_, c)| c.sess.is_none()).map(|(id, c)| (*id, c.app.clone())).collect();
    for (id, _) in conns {
        if has(node, id, "sessions") {
            let l = list_for(node, id);
            conn_send(node, id, json!({ "ev": "sessions", "list": l }));
        }
    }
}

pub fn schedule(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut tick: u64 = 0;
        loop {
            tokio::select! {
                _ = node.shutdown.cancelled() => {
                    let sids: Vec<String> = node.net.hosted.lock().keys().cloned().collect();
                    for s in sids { end(&node, &s, "мост хоста завершает работу"); }
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
            tick += 1;
            // пинг соседям, с которыми есть связь
            let links: Vec<Arc<Link>> = node.net.links.lock().values().cloned().collect();
            for l in &links {
                if l.connected.load(Ordering::SeqCst) && tick % 2 == 0 {
                    link_text(&node, &l.peer, json!({ "t": "ping", "ts": now_ms() }));
                }
            }
            if tick % 3 == 0 {
                announce_all(&node);
                node.net.remote.lock().retain(|_, (_, at)| at.elapsed() < Duration::from_secs(15));
            }
            if tick % 2 == 0 {
                clocks(&node);
            }
            // потерянные связи: у хоста — убрать участников с того ПК; у участника — сеанс закончился
            let dead: Vec<String> = links.iter().filter(|l| l.last_seen.lock().elapsed() > LINK_DEAD && (needs(&node, &l.peer))).map(|l| l.peer.clone()).collect();
            for p in dead {
                let gone: Vec<(String, u16)> = node.net.hosted.lock().iter().flat_map(|(sid, h)| h.members.values().filter(|m| m.node == p).map(|m| (sid.clone(), m.cid)).collect::<Vec<_>>()).collect();
                for (sid, cid) in gone {
                    host_leave(&node, &sid, cid, "пропала связь");
                }
                let lost: Vec<u64> = node.net.conns.lock().iter().filter(|(_, c)| c.sess.as_ref().map(|s| s.host == p).unwrap_or(false)).map(|(id, _)| *id).collect();
                for id in lost {
                    if let Some(c) = node.net.conns.lock().get_mut(&id) {
                        c.sess = None;
                        send_text(&c.tx, json!({ "ev": "ended", "why": format!("пропала связь с хостом ({})", p) }));
                    }
                }
                *link(&node, &p).last_seen.lock() = Instant::now();
            }
            if tick % 3600 == 1 {
                crate::activity::cleanup(&node);
            }
        }
    });
}

/// С соседом есть общие сеансы (его потеря что-то значит).
fn needs(node: &Node, peer: &str) -> bool {
    node.net.hosted.lock().values().any(|h| h.members.values().any(|m| m.node == peer)) || node.net.conns.lock().values().any(|c| c.sess.as_ref().map(|s| s.host == peer).unwrap_or(false))
}

/// Часы и задержки: участникам — смещение к часам хоста; хост рассылает задержки участников.
fn clocks(node: &Arc<Node>) {
    let me = node.node_name();
    let conns: Vec<(u64, Joined)> = node.net.conns.lock().iter().filter_map(|(id, c)| c.sess.clone().map(|s| (*id, s))).collect();
    for (id, s) in conns {
        let (offset, rtt) = if s.host == me {
            (0, 0)
        } else {
            let l = link(node, &s.host);
            (l.offset.load(Ordering::Relaxed), l.rtt.load(Ordering::Relaxed))
        };
        conn_send(node, id, json!({ "ev": "clock", "offset": offset, "rtt": rtt }));
    }
    let sids: Vec<String> = node.net.hosted.lock().keys().cloned().collect();
    for sid in sids {
        let members: Vec<Member> = node.net.hosted.lock().get(&sid).map(|h| h.members.values().cloned().collect()).unwrap_or_default();
        let mut rtt = serde_json::Map::new();
        for m in &members {
            let v = if m.node == me { 0 } else { link(node, &m.node).rtt.load(Ordering::Relaxed) };
            rtt.insert(m.cid.to_string(), json!(v));
        }
        broadcast(node, &sid, json!({ "ev": "rtt", "rtt": rtt }), None);
    }
}

// ───────────── билеты, редактор, активность ─────────────

/// Проверить билет участника (своему серверу приложения, на ПК хоста).
pub fn check_ticket(node: &Node, t: &str) -> Option<Value> {
    node.net.tickets.lock().get(t).cloned()
}

/// Новое событие активности — подключённым приложениям с правом `activity`.
pub fn activity_event(node: &Node, e: &Value) {
    for c in node.net.conns.lock().values() {
        if c.perms.iter().any(|p| p == "activity") {
            send_text(&c.tx, json!({ "ev": "activity", "e": e }));
        }
    }
}

/// Редактор (Unity и т. п.) просит подключиться как приложение `app`: панель спрашивает хозяина.
pub fn dev_request(node: &Node, app: &str, who: &str) -> Result<String, String> {
    let a = crate::apps::get(node, app).ok_or(format!("приложения «{}» нет в папке «приложения» этого ПК", app))?;
    if !crate::apps::consent_ok(node, &a) {
        return Err(format!("«{}» ещё не разрешено на этом ПК — разрешите его в панели", a.name));
    }
    if dev_pending(node).len() >= 3 {
        return Err("уже ждут ответа три запроса на подключение — ответьте в панели моста".into());
    }
    let req = hex::encode(rand::random::<[u8; 8]>());
    let who: String = who.chars().take(60).collect();
    node.net.dev.lock().insert(req.clone(), json!({ "app": app, "name": a.name, "who": who, "state": "wait", "at": now_ms() }));
    node.media.deliver(json!({ "media": "apps", "type": "dev", "req": req, "app": app, "name": a.name, "who": who }));
    node.info(&format!("[приложения] «{}» просит подключиться как «{}» — ждёт ответа в панели", who, a.name));
    Ok(req)
}

pub fn dev_answer(node: &Node, req: &str, ok: bool) -> Result<(), String> {
    let mut g = node.net.dev.lock();
    let r = g.get_mut(req).ok_or("запрос устарел")?;
    if ok {
        let app = r["app"].as_str().unwrap_or("").to_string();
        r["token"] = json!(crate::apps::issue_token(node, &app));
        r["state"] = json!("ok");
    } else {
        r["state"] = json!("no");
    }
    Ok(())
}

pub fn dev_state(node: &Node, req: &str) -> Value {
    let g = node.net.dev.lock();
    g.get(req).map(|r| json!({ "state": r["state"], "token": r["token"] })).unwrap_or(json!({ "state": "gone" }))
}

pub fn dev_pending(node: &Node) -> Vec<Value> {
    node.net.dev.lock().iter().filter(|(_, r)| r["state"] == "wait" && now_ms() - r["at"].as_i64().unwrap_or(0) < 120_000).map(|(k, r)| json!({ "req": k, "app": r["app"], "name": r["name"], "who": r["who"] })).collect()
}

/// Для панели: что сейчас идёт (свои и соседей) в моём канале.
pub fn status(node: &Node) -> Value {
    let ch = channel(node);
    let mut list: Vec<Value> = node.net.hosted.lock().iter().map(|(sid, h)| summary(node, sid, h, &h.proto)).collect();
    for (_, (s, at)) in node.net.remote.lock().iter() {
        if at.elapsed() < Duration::from_secs(12) && s["room"] == ch.room.as_str() {
            list.push(s.clone());
        }
    }
    let links: Vec<Value> = node.net.links.lock().values().map(|l| json!({ "peer": l.peer, "connected": l.connected.load(Ordering::SeqCst), "rtt": l.rtt.load(Ordering::Relaxed) })).collect();
    json!({ "channel": { "room": ch.room, "title": ch.title, "members": ch.members }, "sessions": list, "links": links, "conns": node.net.conns.lock().len(), "dev": dev_pending(node) })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_ops() {
        let mut s = json!({});
        apply_op(&mut s, &json!({ "op": "set", "path": "a.b", "value": 1 }));
        apply_op(&mut s, &json!({ "op": "merge", "path": ["a"], "value": { "c": 2, "b": null } }));
        assert_eq!(s, json!({ "a": { "c": 2 } }));
        apply_op(&mut s, &json!({ "op": "del", "path": "a.c" }));
        assert_eq!(s, json!({ "a": {} }));
        apply_op(&mut s, &json!({ "op": "set", "path": "", "value": { "x": 1 } }));
        assert_eq!(s, json!({ "x": 1 }));
    }
}
