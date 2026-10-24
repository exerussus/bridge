//! Чаты: личные (с соседом) и канала. Сообщения живут сутки. Личное идёт прямо соседу; сообщение канала — через
//! владельца канала: он проверяет, что отправитель участник, и рассылает остальным (так его видят и те, кто не сосед
//! отправителю). Кто не на связи — получит из очереди, пока сообщению меньше суток.
//! Беседа: «dm:<сосед>» или «ch:<владелец>:<канал>».

use crate::node::Node;
use bridge_core::{eq_ic, time};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

const TTL_SECS: i64 = 24 * 3600;
const TEXT_MAX: usize = 4000;
pub const FILE_MAX: usize = 10 * 1024 * 1024;
const FILES_MAX: usize = 25 * 1024 * 1024;

#[derive(Default)]
pub struct Chat {
    dir: Mutex<PathBuf>,
    msgs: Mutex<Vec<Value>>,
    n: AtomicU64,
    ids: Mutex<VecDeque<String>>,
    read: Mutex<BTreeMap<String, u64>>,
    /// (кому, сообщение, срок unix)
    outbox: Mutex<Vec<(String, Value, i64)>>,
    pub notify: Notify,
    pub kick: Notify,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn age_ok(v: &Value) -> bool {
    v["time"].as_str().and_then(time::parse).map(|t| now() - t.timestamp() < TTL_SECS).unwrap_or(false)
}

pub enum Conv {
    Dm(String),
    Ch(String, String),
}

pub fn parse_conv(c: &str) -> Option<Conv> {
    if let Some(p) = c.strip_prefix("dm:") {
        return (!p.is_empty()).then(|| Conv::Dm(p.to_string()));
    }
    let rest = c.strip_prefix("ch:")?;
    let (o, id) = rest.split_once(':')?;
    (!o.is_empty() && !id.is_empty()).then(|| Conv::Ch(o.to_string(), id.to_string()))
}

impl Chat {
    fn path(&self, n: &str) -> PathBuf {
        self.dir.lock().join(n)
    }

    pub fn load(&self, node: &Node) {
        let d = node.data_dir.join("chat");
        let _ = std::fs::create_dir_all(&d);
        *self.dir.lock() = d;
        let list: Vec<Value> = std::fs::read_to_string(self.path("messages.jsonl"))
            .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).filter(age_ok).collect())
            .unwrap_or_default();
        self.n.store(list.iter().filter_map(|v| v["n"].as_u64()).max().unwrap_or(0), Ordering::SeqCst);
        for v in &list {
            self.remember(v["id"].as_str().unwrap_or(""));
        }
        *self.msgs.lock() = list;
        if let Ok(t) = std::fs::read_to_string(self.path("state.json")) {
            if let Ok(v) = serde_json::from_str::<Value>(&t) {
                if let Ok(r) = serde_json::from_value(v["read"].clone()) {
                    *self.read.lock() = r;
                }
                if let Ok(o) = serde_json::from_value::<Vec<(String, Value, i64)>>(v["outbox"].clone()) {
                    *self.outbox.lock() = o;
                }
            }
        }
    }

    fn save(&self) {
        let mut t = String::new();
        for v in self.msgs.lock().iter() {
            t.push_str(&v.to_string());
            t.push('\n');
        }
        let _ = bridge_core::write_atomic(&self.path("messages.jsonl"), &t);
        self.save_state();
    }

    fn save_state(&self) {
        let v = json!({ "read": *self.read.lock(), "outbox": *self.outbox.lock() });
        let _ = bridge_core::write_atomic(&self.path("state.json"), &v.to_string());
    }

    fn remember(&self, id: &str) -> bool {
        if id.is_empty() {
            return true;
        }
        let mut g = self.ids.lock();
        if g.iter().any(|x| x == id) {
            return false;
        }
        g.push_back(id.into());
        while g.len() > 5000 {
            g.pop_front();
        }
        true
    }

    /// Сохранить сообщение у себя (новое — с номером; повтор id — None).
    fn store(&self, node: &Node, mut v: Value) -> Option<Value> {
        if !self.remember(v["id"].as_str().unwrap_or("")) {
            return None;
        }
        let n = self.n.fetch_add(1, Ordering::SeqCst) + 1;
        v["n"] = json!(n);
        {
            let mut g = self.msgs.lock();
            g.push(v.clone());
            g.retain(age_ok);
        }
        self.save();
        self.notify.notify_waiters();
        if v["mine"] != true {
            node.emit("chat-msg", json!({ "conv": v["conv"], "from": v["from"], "author": v["author"], "id": v["id"], "text": preview(&v), "dm": v["conv"].as_str().map(|c| c.starts_with("dm:")).unwrap_or(false) }));
        }
        Some(v)
    }

    fn set_status(&self, id: &str, status: &str) {
        let mut changed = false;
        {
            let mut g = self.msgs.lock();
            if let Some(i) = g.iter().position(|v| v["id"] == id) {
                let mut v = g.remove(i);
                v["status"] = json!(status);
                v["n"] = json!(self.n.fetch_add(1, Ordering::SeqCst) + 1);
                g.push(v);
                changed = true;
            }
        }
        if changed {
            self.save();
            self.notify.notify_waiters();
        }
    }

    pub fn after(&self, conv: &str, after: u64) -> Vec<Value> {
        self.msgs.lock().iter().filter(|v| (conv.is_empty() || v["conv"] == conv) && v["n"].as_u64().unwrap_or(0) > after && age_ok(v)).cloned().collect()
    }

    pub fn mark_read(&self, conv: &str) {
        let n = self.msgs.lock().iter().filter(|v| v["conv"] == conv).filter_map(|v| v["n"].as_u64()).max().unwrap_or(0);
        self.read.lock().insert(conv.into(), n);
        self.save_state();
        self.notify.notify_waiters();
    }

    /// Беседы с последним сообщением и непрочитанным.
    pub fn conversations(&self) -> BTreeMap<String, (Value, u64)> {
        let read = self.read.lock().clone();
        let mut out: BTreeMap<String, (Value, u64)> = BTreeMap::new();
        for v in self.msgs.lock().iter().filter(|v| age_ok(v)) {
            let c = v["conv"].as_str().unwrap_or("").to_string();
            let r = read.get(&c).copied().unwrap_or(0);
            let e = out.entry(c).or_insert((Value::Null, 0));
            if v["mine"] != true && v["first"].as_u64().or(v["n"].as_u64()).unwrap_or(0) > r {
                e.1 += 1;
            }
            let mut last = v.clone();
            last["text"] = json!(preview(v));
            e.0 = last;
        }
        out
    }

    pub fn unread_total(&self) -> u64 {
        self.conversations().values().map(|x| x.1).sum()
    }

    fn queue(&self, to: &str, msg: Value) {
        self.outbox.lock().push((to.into(), msg, now() + TTL_SECS));
        self.save_state();
        self.kick.notify_one();
    }
}

fn files_dir(node: &Node, id: &str) -> PathBuf {
    node.data_dir.join("chat").join("files").join(crate::mobile::folder_name(id))
}

/// Вложения: сохранить у себя → [{name, size, mime}] для сообщения. На входе — {name, mime, data(base64)} или {path} (файл с диска).
fn save_files(node: &Node, id: &str, list: &[Value]) -> Result<Vec<Value>, String> {
    use base64::Engine;
    let mut out = Vec::new();
    let mut total = 0usize;
    for f in list {
        let (name, bytes) = if let Some(p) = f["path"].as_str().filter(|p| !p.is_empty()) {
            let pb = std::path::PathBuf::from(p);
            let b = std::fs::read(&pb).map_err(|e| format!("{}: {}", p, e))?;
            (f["name"].as_str().map(String::from).unwrap_or_else(|| pb.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "файл".into())), b)
        } else {
            let b = base64::engine::general_purpose::STANDARD.decode(f["data"].as_str().unwrap_or("")).map_err(|_| "вложение: не base64".to_string())?;
            (f["name"].as_str().unwrap_or("файл").to_string(), b)
        };
        if bytes.len() > FILE_MAX {
            return Err(format!("«{}» больше 10 МБ", name));
        }
        total += bytes.len();
        if total > FILES_MAX {
            return Err("вложения больше 25 МБ на сообщение".into());
        }
        let name = crate::mobile::folder_name(&name);
        let dir = files_dir(node, id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(&name), &bytes).map_err(|e| e.to_string())?;
        let mime = f["mime"].as_str().filter(|m| !m.is_empty()).map(String::from).unwrap_or_else(|| mime_by_name(&name).into());
        let mut e = json!({ "name": name, "size": bytes.len(), "mime": mime });
        if f["sticker"].as_bool() == Some(true) && crate::avatar::mime_of(&bytes).is_some() {
            e["sticker"] = json!(true);
        }
        out.push(e);
    }
    Ok(out)
}

fn mime_by_name(n: &str) -> &'static str {
    let l = n.to_lowercase();
    let ext = l.rsplit('.').next().unwrap_or("");
    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// Вложения для пересылки соседу: те же записи + data (base64).
fn wire_files(node: &Node, id: &str, files: &[Value]) -> Vec<Value> {
    use base64::Engine;
    files
        .iter()
        .filter_map(|f| {
            let name = f["name"].as_str()?;
            let b = std::fs::read(files_dir(node, id).join(name)).ok()?;
            let mut e = json!({ "name": name, "size": b.len(), "mime": f["mime"], "data": base64::engine::general_purpose::STANDARD.encode(b) });
            if f["sticker"].as_bool() == Some(true) {
                e["sticker"] = json!(true);
            }
            Some(e)
        })
        .collect()
}

pub fn file_path(node: &Node, id: &str, name: &str) -> Option<PathBuf> {
    if name.contains(['/', '\\']) || name.contains("..") {
        return None;
    }
    let p = files_dir(node, id).join(name);
    p.is_file().then_some(p)
}

/// Убрать вложения сообщений старше суток.
fn sweep_files(node: &Node) {
    let keep: std::collections::HashSet<String> = node.chat.msgs.lock().iter().filter_map(|v| v["id"].as_str().map(crate::mobile::folder_name)).collect();
    let root = node.data_dir.join("chat").join("files");
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if !keep.contains(&n) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

fn new_id(node: &Node) -> String {
    format!("{}-{}", node.node_name(), uuid::Uuid::new_v4().simple().to_string().get(..12).unwrap_or(""))
}

async fn post(node: &Arc<Node>, to: &str, msg: &Value) -> Result<(), String> {
    let Some(p) = node.cfg.read().peer(to).cloned().filter(|p| p.enabled && p.has_url()) else { return Err(format!("«{}» не сосед этого ПК", to)) };
    let c = node.client_for(&p);
    match tokio::time::timeout(Duration::from_secs(10), c.post_json("/peer/chat/msg", msg)).await {
        Ok(Ok((code, _))) if (200..300).contains(&code) => Ok(()),
        Ok(Ok((code, t))) => Err(crate::client::error_text(&t, code)),
        Ok(Err(e)) => Err(crate::link::explain(&e, &p).0),
        Err(_) => Err("сосед не ответил вовремя".into()),
    }
}

/// Отправить или поставить в очередь (если сосед не на связи).
async fn deliver(node: &Arc<Node>, to: &str, msg: Value) -> bool {
    if node.peer_state(to).online || node.peer_state(to).last_seen.is_none() {
        if post(node, to, &msg).await.is_ok() {
            return true;
        }
    }
    node.chat.queue(to, msg);
    false
}

/// Участники канала этого узла (кроме самого узла).
fn own_members(node: &Node, id: &str) -> Option<Vec<String>> {
    let c = node.cfg.read();
    let ch = bridge_core::config::get_ic(&c.channels, id)?;
    Some(c.channel_members(ch))
}

/// Владелец разослал сообщение канала участникам (кроме отправителя).
async fn fanout(node: &Arc<Node>, id: &str, msg: &Value, except: &str) {
    for m in own_members(node, id).unwrap_or_default() {
        if eq_ic(&m, except) {
            continue;
        }
        let (n2, msg, m2) = (node.clone(), msg.clone(), m.clone());
        tokio::spawn(async move {
            deliver(&n2, &m2, msg).await;
        });
    }
}

/// Сообщение с этого ПК (панель, телефон, ассистент через API). `author` — подпись, например «Ассистент».
pub async fn send(node: &Arc<Node>, conv: &str, text: &str, author: &str) -> Result<Value, (u16, String)> {
    send_with(node, conv, text, author, &[]).await
}

pub async fn send_with(node: &Arc<Node>, conv: &str, text: &str, author: &str, attach: &[Value]) -> Result<Value, (u16, String)> {
    send_full(node, conv, text, author, attach, &Value::Null).await
}

/// Ответ на сообщение (3.2): {id, from, author?, text — начало или выделенная цитата, q — это цитата}. Неверное — Null.
pub fn clean_reply(v: &Value) -> Value {
    let id = v["id"].as_str().unwrap_or("");
    if id.is_empty() || id.len() > 80 {
        return Value::Null;
    }
    let cut = |k: &str, n: usize| v[k].as_str().unwrap_or("").chars().take(n).collect::<String>();
    json!({ "id": id, "from": cut("from", 64), "author": cut("author", 40), "text": cut("text", 300), "q": v["q"].as_bool().unwrap_or(false) })
}

/// Сообщение с ответом на другое (`reply` — см. clean_reply).
pub async fn send_full(node: &Arc<Node>, conv: &str, text: &str, author: &str, attach: &[Value], reply: &Value) -> Result<Value, (u16, String)> {
    let rp = clean_reply(reply);
    let text = text.trim();
    if text.is_empty() && attach.is_empty() {
        return Err((400, "пустое сообщение".into()));
    }
    if text.chars().count() > TEXT_MAX {
        return Err((400, format!("сообщение длиннее {} знаков", TEXT_MAX)));
    }
    let me = node.node_name();
    let author: String = author.trim().chars().take(40).collect();
    let id = new_id(node);
    let t = time::format(&time::now());
    let files = save_files(node, &id, attach).map_err(|e| (400, e))?;
    let wf = wire_files(node, &id, &files);
    match parse_conv(conv) {
        Some(Conv::Dm(peer)) => {
            let Some(p) = node.cfg.read().peer(&peer).cloned() else { return Err((404, format!("нет соседа «{}»", peer))) };
            let conv = format!("dm:{}", p.name);
            let local = node.chat.store(node, json!({ "id": id, "conv": conv, "from": me, "author": author, "time": t, "text": text, "files": files, "reply": rp, "mine": true, "status": "pending" })).unwrap_or_default();
            let wire = json!({ "id": id, "kind": "dm", "time": t, "text": text, "author": author, "files": wf, "reply": rp });
            let ok = deliver(node, &p.name, wire).await;
            node.chat.set_status(&id, if ok { "sent" } else { "queued" });
            Ok(json!({ "ok": true, "id": id, "delivered": ok, "message": local }))
        }
        Some(Conv::Ch(owner, cid)) => {
            let conv = format!("ch:{}:{}", owner, cid);
            let wire = json!({ "id": id, "kind": "ch", "conv": conv, "from": me, "time": t, "text": text, "author": author, "files": wf, "reply": rp });
            if eq_ic(&owner, &me) {
                if own_members(node, &cid).is_none() {
                    return Err((404, format!("нет канала «{}»", cid)));
                }
                let local = node.chat.store(node, json!({ "id": id, "conv": conv, "from": me, "author": author, "time": t, "text": text, "files": files, "reply": rp, "mine": true, "status": "sent" })).unwrap_or_default();
                fanout(node, &cid, &wire, &me).await;
                return Ok(json!({ "ok": true, "id": id, "delivered": true, "message": local }));
            }
            if !crate::media::all_channels(node).iter().any(|c| eq_ic(&c.owner, &owner) && eq_ic(&c.id, &cid)) {
                return Err((404, "канал не найден (нет в приветах соседей)".into()));
            }
            let local = node.chat.store(node, json!({ "id": id, "conv": conv, "from": me, "author": author, "time": t, "text": text, "files": files, "reply": rp, "mine": true, "status": "pending" })).unwrap_or_default();
            let ok = deliver(node, &owner, wire).await;
            node.chat.set_status(&id, if ok { "sent" } else { "queued" });
            Ok(json!({ "ok": true, "id": id, "delivered": ok, "message": local }))
        }
        None => Err((400, "беседа: dm:<сосед> или ch:<владелец>:<канал>".into())),
    }
}

/// Пришло от соседа (подпись проверена: `from` — имя соседа).
pub async fn on_peer(node: &Arc<Node>, from: &str, v: &Value) -> Result<(), (u16, String)> {
    let id = v["id"].as_str().unwrap_or("").to_string();
    let text: String = v["text"].as_str().unwrap_or("").chars().take(TEXT_MAX).collect();
    let attach = v["files"].as_array().cloned().unwrap_or_default().into_iter().filter(|f| f["path"].is_null()).collect::<Vec<_>>();
    if id.is_empty() || (text.trim().is_empty() && attach.is_empty()) {
        return Err((400, "нет id или текста".into()));
    }
    if node.chat.after("", 0).iter().any(|m| m["id"] == id) {
        return Ok(()); // повтор
    }
    let files = save_files(node, &id, &attach).map_err(|e| (400, e))?;
    // стикеры — отдельно в «Присланные» (живут дольше суток чата)
    let sender = if v["kind"] == "ch" { v["from"].as_str().unwrap_or(from) } else { from };
    for f in files.iter().filter(|f| f["sticker"] == true) {
        if let Some(b) = f["name"].as_str().and_then(|n| file_path(node, &id, n)).and_then(|p| std::fs::read(p).ok()) {
            crate::stickers::received(node, sender, &b);
        }
    }
    let t = v["time"].as_str().filter(|t| time::parse(t).is_some()).map(String::from).unwrap_or_else(|| time::format(&time::now()));
    let author = v["author"].as_str().unwrap_or("").chars().take(40).collect::<String>();
    let rp = clean_reply(&v["reply"]);
    match v["kind"].as_str() {
        Some("dm") => {
            node.chat.store(node, json!({ "id": id, "conv": format!("dm:{}", from), "from": from, "author": author, "time": t, "text": text, "files": files, "reply": rp }));
            Ok(())
        }
        Some("ch") => {
            let conv = v["conv"].as_str().unwrap_or("").to_string();
            let Some(Conv::Ch(owner, cid)) = parse_conv(&conv) else { return Err((400, "беседа".into())) };
            let me = node.node_name();
            if eq_ic(&owner, &me) {
                // участник пишет в наш канал: проверить и разослать остальным
                let allowed = {
                    let c = node.cfg.read();
                    bridge_core::config::get_ic(&c.channels, &cid).map(|ch| c.channel_has(ch, from)).unwrap_or(false)
                };
                if !allowed {
                    return Err((403, "не участник канала".into()));
                }
                let conv = format!("ch:{}:{}", me, cid);
                let wire = json!({ "id": id, "kind": "ch", "conv": conv, "from": from, "time": t, "text": text, "author": author, "files": v["files"], "reply": rp });
                if node.chat.store(node, json!({ "id": id, "conv": conv, "from": from, "author": author, "time": t, "text": text, "files": files, "reply": rp })).is_some() {
                    fanout(node, &cid, &wire, from).await;
                }
                Ok(())
            } else if eq_ic(&owner, from) {
                // владелец канала рассылает: отправитель — из сообщения (владелец его проверил)
                let sender = v["from"].as_str().unwrap_or(from).to_string();
                node.chat.store(node, json!({ "id": id, "conv": conv, "from": sender, "author": author, "time": t, "text": text, "files": files, "reply": rp }));
                Ok(())
            } else {
                Err((403, "сообщения канала рассылает только его владелец".into()))
            }
        }
        _ => Err((400, "kind: dm или ch".into())),
    }
}

/// Очередь: доставить тем, кто снова на связи; сутки — и всё.
pub fn schedule(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        loop {
            let kick = node.chat.kick.notified();
            flush(&node).await;
            sweep_files(&node);
            tokio::select! {
                _ = node.shutdown.cancelled() => break,
                _ = kick => { tokio::time::sleep(Duration::from_millis(300)).await; }
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
    });
}

async fn flush(node: &Arc<Node>) {
    let items: Vec<(String, Value, i64)> = {
        let mut g = node.chat.outbox.lock();
        g.retain(|x| x.2 > now());
        g.clone()
    };
    if items.is_empty() {
        return;
    }
    let mut done = Vec::new();
    for (to, msg, _) in &items {
        if !node.peer_state(to).online {
            continue;
        }
        if post(node, to, msg).await.is_ok() {
            done.push(msg["id"].as_str().unwrap_or("").to_string() + "|" + to);
            if let Some(id) = msg["id"].as_str() {
                if id.starts_with(&format!("{}-", node.node_name())) {
                    node.chat.set_status(id, "sent");
                }
            }
        }
    }
    if !done.is_empty() {
        node.chat.outbox.lock().retain(|(to, m, _)| !done.contains(&(m["id"].as_str().unwrap_or("").to_string() + "|" + to)));
        node.chat.save_state();
    }
}

/// Список бесед для панели: каналы (свои и соседей) и соседи, с последним сообщением и непрочитанным.
pub fn list(node: &Arc<Node>) -> Value {
    let convs = node.chat.conversations();
    let mut out = Vec::new();
    for c in crate::media::all_channels(node) {
        let key = format!("ch:{}:{}", c.owner, c.id);
        let (last, unread) = convs.get(&key).cloned().unwrap_or((Value::Null, 0));
        out.push(json!({ "conv": key, "kind": "channel", "title": c.title, "owner": c.owner, "last": last, "unread": unread }));
    }
    let peers: Vec<(String, String, bool)> = {
        let c = node.cfg.read();
        c.peers.iter().filter(|p| p.enabled && !p.pending).map(|p| (p.name.clone(), if p.title.is_empty() { p.name.clone() } else { p.title.clone() }, node.peer_state(&p.name).online)).collect()
    };
    for (name, title, online) in peers {
        let key = format!("dm:{}", name);
        let (last, unread) = convs.get(&key).cloned().unwrap_or((Value::Null, 0));
        out.push(json!({ "conv": key, "kind": "dm", "title": title, "peer": name, "online": online, "last": last, "unread": unread }));
    }
    json!({ "me": node.node_name(), "conversations": out, "unread": node.chat.unread_total(), "pending": node.chat.outbox.lock().len() })
}

/// Текст для списка бесед и уведомлений: у стикера и вложений без текста — что пришло.
pub fn preview(v: &Value) -> String {
    let t = v["text"].as_str().unwrap_or("").trim();
    if !t.is_empty() {
        return t.to_string();
    }
    let fs = v["files"].as_array().cloned().unwrap_or_default();
    if !fs.is_empty() && fs.iter().all(|f| f["sticker"] == true) {
        return "стикер".into();
    }
    match fs.first().and_then(|f| f["name"].as_str()) {
        Some(n) if fs.len() == 1 => format!("📎 {}", n),
        Some(_) => format!("📎 файлов: {}", fs.len()),
        None => String::new(),
    }
}
