//! Доска канала: общая бесконечная доска для рисования, картинок и заметок. Живёт у владельца канала (как и чат
//! канала): он хранит объекты, нумерует изменения и рассылает их участникам, которые смотрят доску. Участник шлёт
//! свои изменения владельцу, а получает — от него (свои тоже, панель узнаёт их по номеру сеанса `cs`).
//!
//! Объект — JSON с `id` (буквы, цифры, `-`, `_`) и любыми полями панели (`t` — вид: path, rect, arrow, text, note, img).
//! Изменения: `put {o}` — создать или заменить, `del {ids}`, `clear`, `load {objs}` — заменить всё (стенд), `enable {on}`,
//! `edit {mode: all|owner}` (только владелец), `cur {...}` — курсор и вид участника (не хранится).
//! Стенд — сохранённая доска целиком (с картинками) в файле на этом ПК: bridge-data/stands/*.mostboard.

use crate::node::Node;
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

pub const LOG_MAX: usize = 5000;
pub const OBJ_MAX: usize = 50_000;
pub const OBJ_BYTES: usize = 512 * 1024;
pub const IMG_MAX: usize = 12 * 1024 * 1024;
pub const STAND_MAX: usize = 200 * 1024 * 1024;
const SUB_SECS: i64 = 60;

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Default)]
pub struct Boards {
    rooms: Mutex<HashMap<String, Arc<Room>>>,
    pub kick: Notify,
}

pub struct Room {
    pub key: String,
    pub owner: String,
    pub cid: String,
    st: Mutex<St>,
    notify: Notify,
}

#[derive(Default)]
struct Sub {
    until: i64,
    queue: Vec<Value>,
    busy: bool,
}

#[derive(Default)]
struct St {
    loaded: bool,
    enabled: bool,
    edit: String,
    seq: u64,
    objs: HashMap<String, Value>,
    /// фон: {c: "#rrggbb", l: яркость 20..200 %} (пусто — по умолчанию)
    bg: Value,
    /// слои снизу вверх: [{id, name}] (пусто — один слой)
    layers: Value,
    log: VecDeque<Value>,
    dirty: bool,
    /// у владельца: соседи, чьи панели смотрят доску
    subs: HashMap<String, Sub>,
    /// у участника: когда последний раз продлевали подписку у владельца
    last_sub: i64,
    resync: bool,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn parse_key(key: &str) -> Option<(String, String)> {
    let (o, c) = key.split_once(':')?;
    (!o.is_empty() && !c.is_empty() && !c.contains(':')).then(|| (o.to_string(), c.to_string()))
}

fn dir(node: &Node, cid: &str) -> PathBuf {
    node.data_dir.join("boards").join(crate::mobile::folder_name(cid))
}

fn cache_dir(node: &Node, owner: &str, cid: &str) -> PathBuf {
    node.data_dir.join("boards-cache").join(crate::mobile::folder_name(owner)).join(crate::mobile::folder_name(cid))
}

impl Room {
    fn is_mine(&self, node: &Node) -> bool {
        bridge_core::eq_ic(&self.owner, &node.node_name())
    }
}

/// Доска канала «владелец:id» (только каналы — не звонки без канала).
pub fn room(node: &Node, key: &str) -> Result<Arc<Room>, (u16, String)> {
    let (owner, cid) = parse_key(key).ok_or((400, "room: «владелец:канал»".to_string()))?;
    let me = node.node_name();
    let exists = if bridge_core::eq_ic(&owner, &me) {
        bridge_core::config::get_ic(&node.cfg.read().channels, &cid).is_some()
    } else {
        crate::media::all_channels(node).iter().any(|c| bridge_core::eq_ic(&c.owner, &owner) && bridge_core::eq_ic(&c.id, &cid))
    };
    if !exists {
        return Err((404, "доска есть только в канале — такого канала нет".into()));
    }
    let k = format!("{}:{}", owner, cid);
    let mut g = node.boards.rooms.lock();
    Ok(g.entry(k.to_lowercase()).or_insert_with(|| Arc::new(Room { key: k, owner, cid, st: Mutex::new(St::default()), notify: Notify::new() })).clone())
}

fn load(node: &Node, r: &Room, st: &mut St) {
    if st.loaded {
        return;
    }
    st.loaded = true;
    st.edit = "all".into();
    if let Ok(t) = std::fs::read_to_string(dir(node, &r.cid).join("board.json")) {
        if let Ok(v) = serde_json::from_str::<Value>(&t) {
            st.enabled = v["enabled"].as_bool().unwrap_or(false);
            st.edit = v["edit"].as_str().unwrap_or("all").to_string();
            st.seq = v["seq"].as_u64().unwrap_or(0);
            st.bg = v["bg"].clone();
            st.layers = v["layers"].clone();
            for o in v["objs"].as_array().cloned().unwrap_or_default() {
                if let Some(id) = o["id"].as_str() {
                    st.objs.insert(id.to_string(), o.clone());
                }
            }
        }
    }
}

fn snapshot_of(r: &Room, st: &St) -> Value {
    json!({ "room": r.key, "owner": r.owner, "enabled": st.enabled, "edit": st.edit, "seq": st.seq, "bg": st.bg, "layers": st.layers, "objs": st.objs.values().collect::<Vec<_>>() })
}

fn member_ok(node: &Node, r: &Room, from: &str) -> bool {
    if from.is_empty() || bridge_core::eq_ic(from, &node.node_name()) {
        return true;
    }
    let c = node.cfg.read();
    bridge_core::config::get_ic(&c.channels, &r.cid).map(|ch| c.channel_has(ch, from)).unwrap_or(false)
}

/// Фон доски: цвет #rrggbb (или пусто — по умолчанию) и яркость 20..200 %.
fn check_bg(v: &Value) -> Result<Value, String> {
    let c = v["c"].as_str().unwrap_or("");
    if !c.is_empty() && !(c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit())) {
        return Err("цвет фона — #rrggbb".into());
    }
    let l = v["l"].as_f64().unwrap_or(100.0).clamp(20.0, 200.0).round() as i64;
    Ok(json!({ "c": c, "l": l }))
}

/// Слои доски: до 64, у каждого id (латиница, цифры, - и _) и имя до 40 символов.
fn check_layers(v: &Value) -> Result<Value, String> {
    let list = v.as_array().ok_or("слои — список")?;
    if list.is_empty() || list.len() > 64 {
        return Err("слоёв — от 1 до 64".into());
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for l in list {
        let id = l["id"].as_str().unwrap_or("");
        if !valid_id(id) || !seen.insert(id.to_string()) {
            return Err(format!("неверный слой «{}»", id));
        }
        let name: String = l["name"].as_str().unwrap_or(id).chars().take(40).collect();
        out.push(json!({ "id": id, "name": name }));
    }
    Ok(Value::Array(out))
}

fn check_obj(o: &Value) -> Result<String, String> {
    let id = o["id"].as_str().filter(|i| valid_id(i)).ok_or("у объекта нет id")?;
    if o.to_string().len() > OBJ_BYTES {
        return Err("объект слишком большой".into());
    }
    Ok(id.to_string())
}

/// У владельца: применить изменения участника (`from` пусто — свои). Возвращает номер последнего изменения.
pub fn apply(node: &Node, r: &Room, from: &str, cs: &str, ops: &[Value]) -> Result<u64, (u16, String)> {
    if !r.is_mine(node) {
        return Err((400, "доска не здесь".into()));
    }
    if !member_ok(node, r, from) {
        return Err((403, "вы не участник этого канала".into()));
    }
    let me = node.node_name();
    let by = if from.is_empty() { me.clone() } else { from.to_string() };
    let owner_self = bridge_core::eq_ic(&by, &me);
    let mut st = r.st.lock();
    load(node, r, &mut st);
    let mut evs = Vec::new();
    for op in ops {
        let t = op["t"].as_str().unwrap_or("");
        if t != "cur" && t != "enable" && st.edit == "owner" && !owner_self {
            return Err((403, "доску правит только владелец канала".into()));
        }
        if t != "cur" && t != "enable" && t != "edit" && !st.enabled {
            return Err((409, "доска выключена".into()));
        }
        match t {
            "put" => {
                let mut o = op["o"].clone();
                let id = check_obj(&o).map_err(|e| (400, e))?;
                if !st.objs.contains_key(&id) && st.objs.len() >= OBJ_MAX {
                    return Err((413, format!("на доске уже {} объектов", OBJ_MAX)));
                }
                o["by"] = json!(by);
                st.objs.insert(id, o);
            }
            "del" => {
                for id in op["ids"].as_array().cloned().unwrap_or_default() {
                    if let Some(id) = id.as_str() {
                        st.objs.remove(id);
                    }
                }
            }
            "clear" => st.objs.clear(),
            "load" => {
                let list = op["objs"].as_array().cloned().unwrap_or_default();
                if list.len() > OBJ_MAX {
                    return Err((413, "слишком много объектов".into()));
                }
                let mut m = HashMap::new();
                for o in list {
                    let id = check_obj(&o).map_err(|e| (400, e))?;
                    m.insert(id, o);
                }
                st.objs = m;
                if !op["bg"].is_null() {
                    st.bg = check_bg(&op["bg"]).unwrap_or(Value::Null);
                }
                if !op["layers"].is_null() {
                    st.layers = check_layers(&op["layers"]).unwrap_or(Value::Null);
                }
            }
            "enable" => st.enabled = op["on"].as_bool().unwrap_or(false),
            "bg" => st.bg = check_bg(op).map_err(|e| (400, e))?,
            "layers" => st.layers = check_layers(&op["list"]).map_err(|e| (400, e))?,
            "edit" => {
                if !owner_self {
                    return Err((403, "кто правит доску, решает владелец канала".into()));
                }
                st.edit = if op["mode"] == "owner" { "owner".into() } else { "all".into() };
            }
            "cur" => {}
            _ => continue,
        }
        if t != "cur" {
            st.dirty = true;
        }
        st.seq += 1;
        let ev = json!({ "seq": st.seq, "by": by, "cs": cs, "op": op });
        st.log.push_back(ev.clone());
        evs.push(ev);
    }
    while st.log.len() > LOG_MAX {
        st.log.pop_front();
    }
    let now = now();
    st.subs.retain(|_, s| s.until > now);
    for s in st.subs.values_mut() {
        s.queue.extend(evs.iter().cloned());
    }
    let seq = st.seq;
    drop(st);
    r.notify.notify_waiters();
    if !evs.is_empty() {
        node.boards.kick.notify_one();
    }
    Ok(seq)
}

/// Снимок доски (у владельца — свой; у участника — у владельца, и с этого места начинается рассылка сюда).
pub async fn state(node: &Arc<Node>, r: &Arc<Room>) -> Result<Value, (u16, String)> {
    if r.is_mine(node) {
        let mut st = r.st.lock();
        load(node, r, &mut st);
        return Ok(snapshot_of(r, &st));
    }
    let p = node.cfg.read().peer(&r.owner).cloned().ok_or((404, "владельца канала нет среди соседей".to_string()))?;
    let c = node.client_for(&p);
    let (code, t) = tokio::time::timeout(Duration::from_secs(20), c.get_text(&format!("/peer/board/state?room={}", crate::client::esc(&r.key))))
        .await
        .map_err(|_| (504, "владелец канала не ответил".to_string()))?
        .map_err(|e| (502, format!("доска у владельца канала: {}", crate::link::explain(&e, &p).0)))?;
    if code != 200 {
        return Err((code, crate::client::error_text(&t, code)));
    }
    let v: Value = serde_json::from_str(&t).map_err(|_| (502, "непонятный ответ владельца".to_string()))?;
    let seq = v["seq"].as_u64().unwrap_or(0);
    let mut st = r.st.lock();
    st.loaded = true;
    st.enabled = v["enabled"].as_bool().unwrap_or(false);
    st.edit = v["edit"].as_str().unwrap_or("all").to_string();
    st.log.retain(|e| e["seq"].as_u64().unwrap_or(0) > seq);
    st.seq = st.log.back().and_then(|e| e["seq"].as_u64()).unwrap_or(seq).max(seq);
    st.resync = false;
    st.last_sub = now();
    Ok(v)
}

/// У владельца: снимок для соседа и подписка его на изменения.
pub fn peer_state(node: &Node, r: &Room, from: &str) -> Result<Value, (u16, String)> {
    if !r.is_mine(node) || !member_ok(node, r, from) {
        return Err((403, "вы не участник этого канала".into()));
    }
    let mut st = r.st.lock();
    load(node, r, &mut st);
    let s = st.subs.entry(from.to_string()).or_default();
    s.until = now() + SUB_SECS;
    s.queue.clear();
    Ok(snapshot_of(r, &st))
}

pub fn peer_sub(node: &Node, r: &Room, from: &str) -> Result<(), (u16, String)> {
    if !r.is_mine(node) || !member_ok(node, r, from) {
        return Err((403, "вы не участник этого канала".into()));
    }
    let mut st = r.st.lock();
    st.subs.entry(from.to_string()).or_default().until = now() + SUB_SECS;
    Ok(())
}

/// У участника: владелец прислал изменения.
pub fn on_push(node: &Node, r: &Room, from: &str, events: &[Value]) -> Result<(), (u16, String)> {
    if !bridge_core::eq_ic(from, &r.owner) || r.is_mine(node) {
        return Err((403, "изменения доски рассылает только владелец канала".into()));
    }
    let mut st = r.st.lock();
    for e in events {
        let s = e["seq"].as_u64().unwrap_or(0);
        if s <= st.seq {
            continue;
        }
        if s > st.seq + 1 && st.loaded {
            st.resync = true;
        }
        st.seq = s;
        match e["op"]["t"].as_str() {
            Some("enable") => st.enabled = e["op"]["on"].as_bool().unwrap_or(false),
            Some("edit") => st.edit = if e["op"]["mode"] == "owner" { "owner".into() } else { "all".into() },
            Some("bg") => st.bg = check_bg(&e["op"]).unwrap_or(Value::Null),
            Some("layers") => st.layers = check_layers(&e["op"]["list"]).unwrap_or(Value::Null),
            _ => {}
        }
        st.log.push_back(e.clone());
    }
    while st.log.len() > LOG_MAX {
        st.log.pop_front();
    }
    drop(st);
    r.notify.notify_waiters();
    Ok(())
}

/// Изменения после `after` (долгий опрос до `timeout` с). `resync` — пропуск: панели перечитать снимок.
pub async fn poll(node: &Arc<Node>, r: &Arc<Room>, after: u64, timeout: u64) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout.min(30));
    loop {
        let wait = r.notify.notified();
        {
            let mut st = r.st.lock();
            if !r.is_mine(node) && now() - st.last_sub > 20 {
                st.last_sub = now();
                let (n, rr) = (node.clone(), r.clone());
                tokio::spawn(async move {
                    let p = n.cfg.read().peer(&rr.owner).cloned();
                    if let Some(p) = p {
                        let c = n.client_for(&p);
                        let _ = tokio::time::timeout(Duration::from_secs(5), c.post_json("/peer/board/sub", &json!({ "room": rr.key }))).await;
                    }
                });
            }
            let first = st.log.front().and_then(|e| e["seq"].as_u64()).unwrap_or(st.seq + 1);
            if st.resync || (after < st.seq && first > after + 1) {
                return json!({ "resync": true, "seq": st.seq, "events": [] });
            }
            let evs: Vec<Value> = st.log.iter().filter(|e| e["seq"].as_u64().unwrap_or(0) > after).cloned().collect();
            if !evs.is_empty() || tokio::time::Instant::now() >= deadline {
                return json!({ "seq": st.seq, "events": evs, "enabled": st.enabled, "edit": st.edit });
            }
        }
        tokio::select! {
            _ = wait => {}
            _ = tokio::time::sleep_until(deadline) => {}
            _ = node.shutdown.cancelled() => return json!({ "seq": after, "events": [] }),
        }
    }
}

/// Изменения от панели: у себя — применить, у соседа-владельца — отправить ему.
pub async fn op(node: &Arc<Node>, r: &Arc<Room>, cs: &str, ops: &[Value]) -> Result<u64, (u16, String)> {
    if r.is_mine(node) {
        return apply(node, r, "", cs, ops);
    }
    let p = node.cfg.read().peer(&r.owner).cloned().ok_or((404, "владельца канала нет среди соседей".to_string()))?;
    let c = node.client_for(&p);
    let (code, t) = tokio::time::timeout(Duration::from_secs(20), c.post_json("/peer/board/op", &json!({ "room": r.key, "cs": cs, "ops": ops })))
        .await
        .map_err(|_| (504, "владелец канала не ответил".to_string()))?
        .map_err(|e| (502, crate::link::explain(&e, &p).0))?;
    if !(200..300).contains(&code) {
        return Err((code, crate::client::error_text(&t, code)));
    }
    Ok(serde_json::from_str::<Value>(&t).ok().and_then(|v| v["seq"].as_u64()).unwrap_or(0))
}

// ───────────── картинки ─────────────

fn img_id(b: &[u8]) -> Option<String> {
    let ext = match crate::avatar::mime_of(b)? {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "jpg",
    };
    Some(format!("{}.{}", hex::encode(&Sha256::digest(b)[..8]), ext))
}

fn valid_img(id: &str) -> bool {
    let (h, e) = id.split_once('.').unwrap_or(("", ""));
    h.len() == 16 && h.chars().all(|c| c.is_ascii_hexdigit()) && ["png", "gif", "webp", "jpg"].contains(&e)
}

/// У владельца: сохранить картинку доски.
pub fn store_image(node: &Node, r: &Room, from: &str, b: &[u8]) -> Result<String, (u16, String)> {
    if !r.is_mine(node) || !member_ok(node, r, from) {
        return Err((403, "вы не участник этого канала".into()));
    }
    if b.len() > IMG_MAX {
        return Err((413, "картинка больше 12 МБ".into()));
    }
    let id = img_id(b).ok_or((400, "нужна картинка: PNG, JPEG, GIF или WebP".to_string()))?;
    let d = dir(node, &r.cid).join("img");
    std::fs::create_dir_all(&d).map_err(|e| (500, e.to_string()))?;
    let p = d.join(&id);
    if !p.exists() {
        std::fs::write(&p, b).map_err(|e| (500, e.to_string()))?;
    }
    Ok(id)
}

/// Картинка с панели: у себя — сохранить, иначе — владельцу.
pub async fn put_image(node: &Arc<Node>, r: &Arc<Room>, b: &[u8]) -> Result<String, (u16, String)> {
    if r.is_mine(node) {
        return store_image(node, r, "", b);
    }
    use base64::Engine;
    let id = img_id(b).ok_or((400, "нужна картинка: PNG, JPEG, GIF или WebP".to_string()))?;
    let cache = cache_dir(node, &r.owner, &r.cid);
    let _ = std::fs::create_dir_all(&cache);
    let _ = std::fs::write(cache.join(&id), b);
    let p = node.cfg.read().peer(&r.owner).cloned().ok_or((404, "владельца канала нет среди соседей".to_string()))?;
    let c = node.client_for(&p);
    let (code, t) = tokio::time::timeout(Duration::from_secs(120), c.post_json("/peer/board/image", &json!({ "room": r.key, "data": base64::engine::general_purpose::STANDARD.encode(b) })))
        .await
        .map_err(|_| (504, "владелец канала не ответил".to_string()))?
        .map_err(|e| (502, crate::link::explain(&e, &p).0))?;
    if !(200..300).contains(&code) {
        return Err((code, crate::client::error_text(&t, code)));
    }
    Ok(id)
}

pub fn own_image(node: &Node, r: &Room, from: &str, id: &str) -> Option<Vec<u8>> {
    if !valid_img(id) || !member_ok(node, r, from) {
        return None;
    }
    std::fs::read(dir(node, &r.cid).join("img").join(id)).ok()
}

pub async fn get_image(node: &Arc<Node>, r: &Arc<Room>, id: &str) -> Option<Vec<u8>> {
    if !valid_img(id) {
        return None;
    }
    if r.is_mine(node) {
        return own_image(node, r, "", id);
    }
    let cache = cache_dir(node, &r.owner, &r.cid).join(id);
    if let Ok(b) = std::fs::read(&cache) {
        return Some(b);
    }
    let p = node.cfg.read().peer(&r.owner).cloned()?;
    let c = node.client_for(&p);
    let (code, b) = tokio::time::timeout(Duration::from_secs(60), c.get_bytes(&format!("/peer/board/image?room={}&id={}", crate::client::esc(&r.key), id))).await.ok()?.ok()?;
    if code != 200 || img_id(&b).as_deref() != Some(id) {
        return None;
    }
    let _ = std::fs::create_dir_all(cache.parent()?);
    let _ = std::fs::write(&cache, &b);
    Some(b)
}

// ───────────── стенды ─────────────

fn stands_dir(node: &Node) -> PathBuf {
    node.data_dir.join("stands")
}

fn valid_stand(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn stand_path(node: &Node, id: &str) -> Option<PathBuf> {
    valid_stand(id).then(|| stands_dir(node).join(format!("{}.mostboard", id))).filter(|p| p.is_file())
}

/// Сохранённые стенды: имя, кто, когда, сколько объектов, размер.
pub fn stands(node: &Node) -> Value {
    let mut out: Vec<Value> = std::fs::read_dir(stands_dir(node))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".mostboard")?.to_string();
            if !valid_stand(&id) {
                return None;
            }
            let size = e.metadata().ok()?.len();
            // заголовок — первые байты файла: name, time, author, count
            let head = read_head(&e.path());
            Some(json!({ "id": id, "size": size, "name": head["name"], "time": head["time"], "author": head["author"], "objects": head["count"] }))
        })
        .collect();
    out.sort_by(|a, b| b["time"].as_str().unwrap_or("").cmp(a["time"].as_str().unwrap_or("")));
    json!({ "stands": out })
}

fn read_head(p: &std::path::Path) -> Value {
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map(|v| json!({ "name": v["name"], "time": v["time"], "author": v["author"], "count": v["objs"].as_array().map(|a| a.len()).unwrap_or(0) })).unwrap_or(json!({}))
}

/// Сохранить доску канала стендом (с картинками внутри файла).
pub async fn stand_save(node: &Arc<Node>, r: &Arc<Room>, name: &str) -> Result<Value, (u16, String)> {
    use base64::Engine;
    let snap = state(node, r).await?;
    let objs = snap["objs"].as_array().cloned().unwrap_or_default();
    let mut images = Map::new();
    for o in &objs {
        if let Some(src) = o["src"].as_str() {
            if images.contains_key(src) {
                continue;
            }
            if let Some(b) = get_image(node, r, src).await {
                images.insert(src.to_string(), json!(base64::engine::general_purpose::STANDARD.encode(b)));
            }
        }
    }
    let name: String = name.trim().chars().take(80).collect();
    let name = if name.is_empty() { format!("Доска «{}»", r.cid) } else { name };
    let t = bridge_core::time::format(&bridge_core::time::now());
    let id = format!("{}-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"), &hex::encode(Sha256::digest(name.as_bytes()))[..6]);
    let v = json!({ "kind": "most-board", "v": 1, "name": name, "time": t, "author": node.node_name(), "room": r.key, "objs": objs, "bg": snap["bg"], "layers": snap["layers"], "images": images });
    let d = stands_dir(node);
    std::fs::create_dir_all(&d).map_err(|e| (500, e.to_string()))?;
    std::fs::write(d.join(format!("{}.mostboard", id)), v.to_string()).map_err(|e| (500, e.to_string()))?;
    node.info(&format!("Доска «{}» сохранена стендом «{}»", r.key, name));
    Ok(json!({ "ok": true, "id": id, "name": name }))
}

/// Загрузить стенд на доску канала: картинки — владельцу, объекты — одним изменением `load` (у всех сразу).
pub async fn stand_load(node: &Arc<Node>, r: &Arc<Room>, id: &str, cs: &str) -> Result<u64, (u16, String)> {
    use base64::Engine;
    let p = stand_path(node, id).ok_or((404, "нет такого стенда".to_string()))?;
    let v: Value = serde_json::from_slice(&std::fs::read(&p).map_err(|e| (500, e.to_string()))?).map_err(|_| (400, "файл стенда повреждён".to_string()))?;
    for (_, b64) in v["images"].as_object().cloned().unwrap_or_default() {
        if let Some(b) = b64.as_str().and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok()) {
            put_image(node, r, &b).await?;
        }
    }
    let objs = v["objs"].as_array().cloned().unwrap_or_default();
    let mut ops = Vec::new();
    {
        let enabled = state(node, r).await?["enabled"].as_bool().unwrap_or(false);
        if !enabled {
            ops.push(json!({ "t": "enable", "on": true }));
        }
    }
    ops.push(json!({ "t": "load", "objs": objs, "bg": v["bg"], "layers": v["layers"] }));
    op(node, r, cs, &ops).await
}

/// Принять файл стенда (.mostboard) — проверить и сохранить к себе.
pub fn stand_import(node: &Node, b: &[u8], fallback: &str) -> Result<Value, (u16, String)> {
    if b.len() > STAND_MAX {
        return Err((413, "файл стенда больше 200 МБ".into()));
    }
    let mut v: Value = serde_json::from_slice(b).map_err(|_| (400, "это не файл стенда".to_string()))?;
    if v["kind"] != "most-board" || !v["objs"].is_array() {
        return Err((400, "это не файл стенда".into()));
    }
    if v["name"].as_str().unwrap_or("").is_empty() {
        v["name"] = json!(fallback.trim_end_matches(".mostboard"));
    }
    let id = format!("{}-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"), &hex::encode(Sha256::digest(b))[..6]);
    let d = stands_dir(node);
    std::fs::create_dir_all(&d).map_err(|e| (500, e.to_string()))?;
    std::fs::write(d.join(format!("{}.mostboard", id)), v.to_string()).map_err(|e| (500, e.to_string()))?;
    Ok(json!({ "ok": true, "id": id, "name": v["name"] }))
}

pub fn stand_remove(node: &Node, id: &str) -> Result<(), (u16, String)> {
    let p = stand_path(node, id).ok_or((404, "нет такого стенда".to_string()))?;
    std::fs::remove_file(p).map_err(|e| (500, e.to_string()))
}

// ───────────── фоновая работа: рассылка и сохранение ─────────────

pub fn schedule(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut last_save = std::time::Instant::now();
        loop {
            let kick = node.boards.kick.notified();
            push_all(&node);
            if last_save.elapsed() > Duration::from_secs(2) {
                save_all(&node);
                last_save = std::time::Instant::now();
            }
            tokio::select! {
                _ = node.shutdown.cancelled() => { save_all(&node); break; }
                _ = kick => { tokio::time::sleep(Duration::from_millis(15)).await; }
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    });
}

fn push_all(node: &Arc<Node>) {
    let rooms: Vec<Arc<Room>> = node.boards.rooms.lock().values().filter(|r| r.is_mine(node)).cloned().collect();
    for r in rooms {
        let jobs: Vec<(String, Vec<Value>)> = {
            let mut st = r.st.lock();
            st.subs.iter_mut().filter(|(_, s)| !s.busy && !s.queue.is_empty()).map(|(n, s)| { s.busy = true; (n.clone(), std::mem::take(&mut s.queue)) }).collect()
        };
        for (peer, evs) in jobs {
            let (n, rr) = (node.clone(), r.clone());
            tokio::spawn(async move {
                let p = n.cfg.read().peer(&peer).cloned();
                let ok = match p {
                    Some(p) => {
                        let c = n.client_for(&p);
                        matches!(tokio::time::timeout(Duration::from_secs(5), c.post_json("/peer/board/push", &json!({ "room": rr.key, "events": evs }))).await, Ok(Ok((200..=299, _))))
                    }
                    None => false,
                };
                let mut st = rr.st.lock();
                if let Some(s) = st.subs.get_mut(&peer) {
                    s.busy = false;
                    if !ok {
                        // не дошло — сосед заметит пропуск и перечитает доску; не доставать его, пока не продлит подписку
                        s.queue.clear();
                        s.until = s.until.min(now() + 10);
                    }
                }
                drop(st);
                n.boards.kick.notify_one();
            });
        }
    }
}

fn save_all(node: &Node) {
    let rooms: Vec<Arc<Room>> = node.boards.rooms.lock().values().filter(|r| r.is_mine(node)).cloned().collect();
    for r in rooms {
        let v = {
            let mut st = r.st.lock();
            if !st.dirty {
                continue;
            }
            st.dirty = false;
            json!({ "enabled": st.enabled, "edit": st.edit, "seq": st.seq, "bg": st.bg, "layers": st.layers, "objs": st.objs.values().collect::<Vec<_>>() })
        };
        let d = dir(node, &r.cid);
        if std::fs::create_dir_all(&d).is_ok() {
            let tmp = d.join("board.json.tmp");
            if std::fs::write(&tmp, v.to_string()).is_ok() {
                let _ = std::fs::rename(&tmp, d.join("board.json"));
            }
        }
    }
}
