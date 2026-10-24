//! Оформление канала (3.2): иконка (картинка или гифка) и подложка — картинка с вписыванием, маской, прозрачностью,
//! размытием и оттенком (те же настройки, что у плитки участника). Хранит владелец канала:
//! bridge-data/chan-art/<канал>/{icon.img, back.img, style.json}. Отпечаток — `art` в описании канала (привет);
//! участники забирают оформление у владельца один раз и держат у себя (bridge-data/chan-art-cache/<владелец>/<канал>/).

use crate::node::Node;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

pub const ICON_MAX: usize = 2 * 1024 * 1024;
pub const BACK_MAX: usize = 4 * 1024 * 1024;

fn dir(node: &Node, cid: &str) -> PathBuf {
    node.data_dir.join("chan-art").join(crate::mobile::folder_name(cid))
}
fn cache_dir(node: &Node, owner: &str, cid: &str) -> PathBuf {
    node.data_dir.join("chan-art-cache").join(crate::mobile::folder_name(owner)).join(crate::mobile::folder_name(cid))
}
fn file(what: &str) -> Option<&'static str> {
    match what {
        "icon" => Some("icon.img"),
        "back" => Some("back.img"),
        _ => None,
    }
}
fn hash_file(p: &std::path::Path) -> String {
    std::fs::read(p).map(|b| crate::avatar::hash_of(&b)).unwrap_or_default()
}

/// Своё оформление канала: {icon, back, style} (icon, back — отпечатки картинок, пусто — нет).
pub fn own(node: &Node, cid: &str) -> Value {
    let d = dir(node, cid);
    let style = std::fs::read_to_string(d.join("style.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or_else(|| json!({}));
    json!({ "icon": hash_file(&d.join("icon.img")), "back": hash_file(&d.join("back.img")), "style": style })
}

/// Отпечаток оформления (пусто — обычное).
pub fn hash(node: &Node, cid: &str) -> String {
    let v = own(node, cid);
    if v["icon"] == "" && v["back"] == "" && v["style"].as_object().map(|m| m.is_empty()).unwrap_or(true) {
        return String::new();
    }
    crate::avatar::hash_of(v.to_string().as_bytes())
}

fn own_channel(node: &Node, cid: &str) -> Result<String, String> {
    bridge_core::config::get_ic(&node.cfg.read().channels, cid).map(|_| ()).ok_or_else(|| format!("нет своего канала «{}»", cid))?;
    let key = node.cfg.read().channels.keys().find(|k| bridge_core::eq_ic(k, cid)).cloned().unwrap_or_else(|| cid.to_string());
    Ok(key)
}

/// Картинка оформления: what = icon | back.
pub fn set_img(node: &Arc<Node>, cid: &str, what: &str, b: &[u8]) -> Result<String, String> {
    let cid = own_channel(node, cid)?;
    let f = file(what).ok_or("what: icon или back")?;
    let max = if what == "icon" { ICON_MAX } else { BACK_MAX };
    if b.len() > max {
        return Err(format!("картинка больше {} МБ", max / 1024 / 1024));
    }
    if crate::avatar::mime_of(b).is_none() {
        return Err("нужна картинка: JPEG, PNG, WebP или GIF".into());
    }
    let d = dir(node, &cid);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    std::fs::write(d.join(f), b).map_err(|e| e.to_string())?;
    node.notify_all("avatar");
    Ok(crate::avatar::hash_of(b))
}

pub fn remove_img(node: &Arc<Node>, cid: &str, what: &str) -> Result<(), String> {
    let cid = own_channel(node, cid)?;
    let f = file(what).ok_or("what: icon или back")?;
    let _ = std::fs::remove_file(dir(node, &cid).join(f));
    node.notify_all("avatar");
    Ok(())
}

pub fn set_style(node: &Arc<Node>, cid: &str, v: &Value) -> Result<Value, String> {
    let cid = own_channel(node, cid)?;
    let st = crate::tile::clean(v)?;
    let d = dir(node, &cid);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    std::fs::write(d.join("style.json"), st.to_string()).map_err(|e| e.to_string())?;
    node.notify_all("avatar");
    Ok(st)
}

/// Картинка своего канала (для соседей и своей панели).
pub fn own_img(node: &Node, cid: &str, what: &str) -> Option<Vec<u8>> {
    std::fs::read(dir(node, cid).join(file(what)?)).ok()
}

/// Оформление канала «владелец:канал»: своё — сразу, чужое — из кэша по отпечатку или у владельца.
pub async fn of(node: &Arc<Node>, owner: &str, cid: &str) -> Option<Value> {
    if bridge_core::eq_ic(owner, &node.node_name()) {
        return Some(own(node, cid));
    }
    let want = crate::media::all_channels(node).into_iter().find(|c| bridge_core::eq_ic(&c.owner, owner) && bridge_core::eq_ic(&c.id, cid))?.art;
    if want.is_empty() {
        return None;
    }
    let d = cache_dir(node, owner, cid);
    if let Some(v) = std::fs::read_to_string(d.join("art.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        if v["hash"] == want {
            return Some(v["art"].clone());
        }
    }
    let p = node.cfg.read().peer(owner).cloned()?;
    let client = node.client_for(&p);
    let (code, t) = tokio::time::timeout(std::time::Duration::from_secs(10), client.get_text(&format!("/peer/chan/art?id={}", crate::client::esc(cid)))).await.ok()?.ok()?;
    if code != 200 {
        return None;
    }
    let mut v: Value = serde_json::from_str(&t).ok()?;
    v["style"] = crate::tile::clean(&v["style"]).unwrap_or_else(|_| json!({}));
    let _ = std::fs::create_dir_all(&d);
    let _ = std::fs::write(d.join("art.json"), json!({ "hash": want, "art": v }).to_string());
    Some(v)
}

/// Картинка оформления чужого канала: из кэша по отпечатку или у владельца.
pub async fn img_of(node: &Arc<Node>, owner: &str, cid: &str, what: &str) -> Option<Vec<u8>> {
    let f = file(what)?;
    if bridge_core::eq_ic(owner, &node.node_name()) {
        return own_img(node, cid, what);
    }
    let art = of(node, owner, cid).await?;
    let want = art[what].as_str().unwrap_or("").to_string();
    if want.is_empty() {
        return None;
    }
    let p = cache_dir(node, owner, cid).join(f);
    if let Ok(b) = std::fs::read(&p) {
        if crate::avatar::hash_of(&b) == want {
            return Some(b);
        }
    }
    let peer = node.cfg.read().peer(owner).cloned()?;
    let client = node.client_for(&peer);
    let (code, b) = tokio::time::timeout(std::time::Duration::from_secs(20), client.get_bytes(&format!("/peer/chan/img?id={}&what={}", crate::client::esc(cid), what))).await.ok()?.ok()?;
    let max = if what == "icon" { ICON_MAX } else { BACK_MAX };
    if code != 200 || b.len() > max || crate::avatar::mime_of(&b).is_none() {
        return None;
    }
    let _ = std::fs::write(&p, &b);
    Some(b)
}

/// Сосед просит оформление: только если он участник канала.
pub fn for_peer(node: &Node, peer: &str, cid: &str) -> Option<Value> {
    let ok = {
        let c = node.cfg.read();
        bridge_core::config::get_ic(&c.channels, cid).map(|ch| c.channel_has(ch, peer)).unwrap_or(false)
    };
    let key = node.cfg.read().channels.keys().find(|k| bridge_core::eq_ic(k, cid)).cloned()?;
    ok.then(|| own(node, &key))
}
pub fn img_for_peer(node: &Node, peer: &str, cid: &str, what: &str) -> Option<Vec<u8>> {
    for_peer(node, peer, cid)?;
    let key = node.cfg.read().channels.keys().find(|k| bridge_core::eq_ic(k, cid)).cloned()?;
    own_img(node, &key, what)
}
