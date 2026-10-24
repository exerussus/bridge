//! Стикеры и гифки: у каждого свой набор (bridge-data/stickers/mine). Отправленный стикер уходит в чат обычным
//! вложением с пометкой `sticker`, а получатель сохраняет его отдельно — в «Присланные» (bridge-data/stickers/got),
//! где он живёт дольше суток чата; оттуда его можно забрать себе («В мои»).

use crate::node::Node;
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub const MAX: usize = 2 * 1024 * 1024;
/// Сколько присланных держать (старые уходят).
pub const GOT_MAX: usize = 300;

static INDEX_LOCK: Mutex<()> = Mutex::new(());

fn dir(node: &Node, set: &str) -> PathBuf {
    node.data_dir.join("stickers").join(if set == "got" { "got" } else { "mine" })
}

fn ext_of(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "jpg",
    }
}

/// id стикера: отпечаток содержимого + расширение («3fa9c2d1e0b7.gif»).
pub fn id_of(b: &[u8]) -> Option<String> {
    let mime = crate::avatar::mime_of(b)?;
    Some(format!("{}.{}", hex::encode(&Sha256::digest(b)[..6]), ext_of(mime)))
}

fn valid_id(id: &str) -> bool {
    let mut it = id.splitn(2, '.');
    let (h, e) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
    h.len() == 12 && h.chars().all(|c| c.is_ascii_hexdigit()) && ["png", "gif", "webp", "jpg"].contains(&e)
}

fn index_file(node: &Node) -> PathBuf {
    dir(node, "got").join("index.json")
}

fn read_index(node: &Node) -> Map<String, Value> {
    std::fs::read_to_string(index_file(node)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

fn files(node: &Node, set: &str) -> Vec<(String, std::time::SystemTime, u64)> {
    let mut v: Vec<_> = std::fs::read_dir(dir(node, set))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            if !valid_id(&n) {
                return None;
            }
            let m = e.metadata().ok()?;
            Some((n, m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len()))
        })
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

pub fn list(node: &Node) -> Value {
    let idx = read_index(node);
    let mine: Vec<Value> = files(node, "mine").into_iter().map(|(id, _, size)| json!({ "id": id, "size": size })).collect();
    let got: Vec<Value> = files(node, "got")
        .into_iter()
        .map(|(id, _, size)| {
            let m = idx.get(&id).cloned().unwrap_or(json!({}));
            json!({ "id": id, "size": size, "from": m["from"], "time": m["time"] })
        })
        .collect();
    json!({ "mine": mine, "got": got, "max": MAX })
}

/// Добавить в свои (картинку или гифку как есть).
pub fn add(node: &Node, b: &[u8]) -> Result<String, String> {
    if b.len() > MAX {
        return Err("стикер больше 2 МБ".into());
    }
    let id = id_of(b).ok_or("нужна картинка: GIF, PNG, WebP или JPEG")?;
    let d = dir(node, "mine");
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    std::fs::write(d.join(&id), b).map_err(|e| e.to_string())?;
    Ok(id)
}

pub fn remove(node: &Node, set: &str, id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err("нет такого стикера".into());
    }
    let _ = std::fs::remove_file(dir(node, set).join(id));
    if set == "got" {
        let _g = INDEX_LOCK.lock();
        let mut idx = read_index(node);
        idx.remove(id);
        let _ = std::fs::write(index_file(node), Value::Object(idx).to_string());
    }
    Ok(())
}

/// Присланный → в свои.
pub fn keep(node: &Node, id: &str) -> Result<(), String> {
    let p = path(node, "got", id).ok_or("нет такого стикера")?;
    let b = std::fs::read(p).map_err(|e| e.to_string())?;
    add(node, &b).map(|_| ())
}

pub fn path(node: &Node, set: &str, id: &str) -> Option<PathBuf> {
    if !valid_id(id) {
        return None;
    }
    let sets: &[&str] = match set {
        "mine" => &["mine"],
        "got" => &["got"],
        _ => &["mine", "got"],
    };
    sets.iter().map(|s| dir(node, s).join(id)).find(|p| p.is_file())
}

/// Вложение для чата из своего (или присланного) стикера.
pub fn attachment(node: &Node, id: &str) -> Result<Value, String> {
    use base64::Engine;
    let p = path(node, "", id).ok_or("нет такого стикера")?;
    let b = std::fs::read(p).map_err(|e| e.to_string())?;
    let mime = crate::avatar::mime_of(&b).unwrap_or("image/png");
    Ok(json!({ "name": id, "mime": mime, "sticker": true, "data": base64::engine::general_purpose::STANDARD.encode(&b) }))
}

/// Пришёл стикер в чате: сохранить отдельно в «Присланные» (если его нет в своих).
pub fn received(node: &Node, from: &str, b: &[u8]) {
    if b.len() > MAX {
        return;
    }
    let Some(id) = id_of(b) else { return };
    if dir(node, "mine").join(&id).is_file() {
        return;
    }
    let d = dir(node, "got");
    if std::fs::create_dir_all(&d).is_err() {
        return;
    }
    let _g = INDEX_LOCK.lock();
    let mut idx = read_index(node);
    if !d.join(&id).is_file() {
        if std::fs::write(d.join(&id), b).is_err() {
            return;
        }
        idx.insert(id.clone(), json!({ "from": from, "time": bridge_core::time::format(&bridge_core::time::now()) }));
    } else {
        // уже есть — поднять наверх
        let _ = std::fs::File::options().append(true).open(d.join(&id)).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    }
    let all = files(node, "got");
    for (old, _, _) in all.iter().skip(GOT_MAX) {
        let _ = std::fs::remove_file(d.join(old));
        idx.remove(old);
    }
    let _ = std::fs::write(index_file(node), Value::Object(idx).to_string());
}
