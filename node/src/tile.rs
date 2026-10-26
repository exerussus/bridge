//! Плитка участника: как вас видят в канале. Стиль (картинка-фон, её вписывание, маска, прозрачность, размытие,
//! оттенок, рамка, форма и размер аватарки, цвет подсветки и имени) и картинка-фон (можно гифку) хранятся у моста:
//! bridge-data/tile.json и bridge-data/tile.img. Отпечаток уходит соседям в привете, соседи забирают плитку один раз
//! и держат у себя (bridge-data/tiles/<сосед>.json и .img), пока отпечаток не сменится — как с аватаркой.

use crate::node::Node;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Картинка плитки: до 4 МБ (гифка — как есть).
pub const IMG_MAX: usize = 4 * 1024 * 1024;

fn style_file(node: &Node) -> PathBuf {
    node.data_dir.join("tile.json")
}
fn img_file(node: &Node) -> PathBuf {
    node.data_dir.join("tile.img")
}
/// (3.3.1) Значок статуса: маленькая картинка (до 256 КБ, можно гифку).
fn st_file(node: &Node) -> PathBuf {
    node.data_dir.join("tile-status.img")
}
pub const ST_MAX: usize = 256 * 1024;
/// Текст статуса: до 255 знаков, без управляющих символов.
pub fn clean_status_text(t: &str) -> String {
    t.chars().filter(|c| !c.is_control() || *c == ' ').map(|c| if c.is_control() { ' ' } else { c }).take(255).collect::<String>().trim().to_string()
}
fn cache(node: &Node, peer: &str, ext: &str) -> PathBuf {
    node.data_dir.join("tiles").join(format!("{}.{}", crate::mobile::folder_name(peer), ext))
}

fn color(v: &Value) -> Option<String> {
    let t = v.as_str()?.trim();
    if t.is_empty() {
        return Some(String::new());
    }
    let h = t.trim_start_matches('#');
    (h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("#{}", h.to_uppercase()))
}

/// Проверить стиль: только известные ключи, значения в пределах. Неизвестное отбрасывается.
pub fn clean(v: &Value) -> Result<Value, String> {
    let o = v.as_object().ok_or("стиль плитки — объект")?;
    let mut out = Map::new();
    let pick = |k: &str, allowed: &[&str], out: &mut Map<String, Value>| -> Result<(), String> {
        if let Some(x) = o.get(k) {
            let s = x.as_str().unwrap_or("");
            if !allowed.contains(&s) {
                return Err(format!("{}: {}", k, allowed.join(", ")));
            }
            out.insert(k.into(), json!(s));
        }
        Ok(())
    };
    pick("fit", &["cover", "contain", "repeat"], &mut out)?;
    pick("mask", &["none", "bottom", "top", "sides", "vignette", "circle", "diagonal"], &mut out)?;
    pick("avShape", &["circle", "round", "square", "hex", "none"], &mut out)?;
    for (k, lo, hi) in [("x", 0.0, 100.0), ("y", 0.0, 100.0), ("zoom", 100.0, 400.0), ("soft", 0.0, 100.0), ("opacity", 5.0, 100.0), ("blur", 0.0, 20.0), ("tintAmt", 0.0, 100.0), ("avSize", 50.0, 160.0)] {
        if let Some(x) = o.get(k) {
            let f = x.as_f64().ok_or(format!("{} — число", k))?;
            out.insert(k.into(), json!(f.clamp(lo, hi).round()));
        }
    }
    if o.get("stSilent").and_then(|x| x.as_bool()).unwrap_or(false) {
        out.insert("stSilent".into(), json!(true));
    }
    if let Some(x) = o.get("stText") {
        let t = clean_status_text(x.as_str().ok_or("stText — строка")?);
        if !t.is_empty() {
            out.insert("stText".into(), json!(t));
        }
    }
    for k in ["tint", "avRing", "glow", "name", "border", "base", "stColor"] {
        if let Some(x) = o.get(k) {
            let c = color(x).ok_or(format!("{}: цвет #rrggbb", k))?;
            if !c.is_empty() {
                out.insert(k.into(), json!(c));
            }
        }
    }
    Ok(Value::Object(out))
}

pub fn own_style(node: &Node) -> Value {
    std::fs::read_to_string(style_file(node)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}))
}
pub fn own_img(node: &Node) -> Option<Vec<u8>> {
    std::fs::read(img_file(node)).ok()
}
fn img_hash(node: &Node) -> String {
    own_img(node).map(|b| crate::avatar::hash_of(&b)).unwrap_or_default()
}
pub fn own_st_img(node: &Node) -> Option<Vec<u8>> {
    std::fs::read(st_file(node)).ok()
}
fn st_hash(node: &Node) -> String {
    own_st_img(node).map(|b| crate::avatar::hash_of(&b)).unwrap_or_default()
}

/// Своя плитка: {style, img, stImg} (img, stImg — отпечатки картинки-фона и значка статуса, пусто — нет).
pub fn own(node: &Node) -> Value {
    json!({ "style": own_style(node), "img": img_hash(node), "stImg": st_hash(node) })
}

/// Отпечаток плитки (пусто — плитка обычная).
pub fn own_hash(node: &Node) -> String {
    let st = own_style(node);
    let img = img_hash(node);
    let sti = st_hash(node);
    if img.is_empty() && sti.is_empty() && st.as_object().map(|m| m.is_empty()).unwrap_or(true) {
        return String::new();
    }
    crate::avatar::hash_of(format!("{}|{}|{}", st, img, sti).as_bytes())
}

pub fn set_style(node: &Arc<Node>, v: &Value) -> Result<Value, String> {
    let mut st = clean(v)?;
    // статус меняется отдельно: стиль без него статус не стирает
    let old = own_style(node);
    for k in ["stText", "stColor", "stSilent"] {
        if v.get(k).is_none() {
            if let Some(x) = old.get(k) {
                st[k] = x.clone();
            }
        }
    }
    std::fs::write(style_file(node), st.to_string()).map_err(|e| e.to_string())?;
    node.notify_all("avatar");
    Ok(st)
}

pub fn set_img(node: &Arc<Node>, b: &[u8]) -> Result<String, String> {
    if b.len() > IMG_MAX {
        return Err("картинка плитки больше 4 МБ".into());
    }
    if crate::avatar::mime_of(b).is_none() {
        return Err("нужна картинка: JPEG, PNG, WebP или GIF".into());
    }
    std::fs::write(img_file(node), b).map_err(|e| e.to_string())?;
    node.notify_all("avatar");
    Ok(crate::avatar::hash_of(b))
}

/// Статус: {text, color} — текст до 255 знаков и цвет значка (пусто — убрать).
pub fn set_status(node: &Arc<Node>, v: &Value) -> Result<Value, String> {
    let mut st = own_style(node);
    if !st.is_object() {
        st = json!({});
    }
    let m = st.as_object_mut().unwrap();
    if let Some(t) = v.get("text") {
        let t = clean_status_text(t.as_str().ok_or("text — строка")?);
        if t.is_empty() {
            m.remove("stText");
        } else {
            m.insert("stText".into(), json!(t));
        }
    }
    if let Some(b) = v.get("silent") {
        if b.as_bool().unwrap_or(false) {
            m.insert("stSilent".into(), json!(true));
        } else {
            m.remove("stSilent");
        }
    }
    if let Some(c) = v.get("color") {
        match color(c).ok_or("color: цвет #rrggbb")? {
            c if c.is_empty() => {
                m.remove("stColor");
            }
            c => {
                m.insert("stColor".into(), json!(c));
            }
        }
    }
    std::fs::write(style_file(node), st.to_string()).map_err(|e| e.to_string())?;
    node.notify_all("avatar");
    Ok(json!({ "text": st["stText"], "color": st["stColor"], "silent": st["stSilent"], "img": st_hash(node) }))
}

pub fn set_st_img(node: &Arc<Node>, b: &[u8]) -> Result<String, String> {
    if b.len() > ST_MAX {
        return Err("значок статуса больше 256 КБ".into());
    }
    if crate::avatar::mime_of(b).is_none() {
        return Err("нужна картинка: JPEG, PNG, WebP или GIF".into());
    }
    std::fs::write(st_file(node), b).map_err(|e| e.to_string())?;
    node.notify_all("avatar");
    Ok(crate::avatar::hash_of(b))
}

// ───────────── заготовки статуса (3.3.2) ─────────────
// Статусы заготавливаются в настройках ({id, text, color} + картинка-значок), а выбираются правой кнопкой на своей плитке.
// Заготовки живут только на этом ПК; соседям уходит выбранный статус — как раньше (текст, цвет, значок в плитке).

fn presets_file(node: &Node) -> PathBuf {
    node.data_dir.join("status-presets.json")
}
fn preset_img_file(node: &Node, id: &str) -> Option<PathBuf> {
    (!id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then(|| node.data_dir.join("status-img").join(format!("{}.img", id)))
}

/// Заготовки: [{id, text, color, img (отпечаток картинки, пусто — нет)}] и какая выбрана сейчас (`active`, пусто — без статуса).
pub fn presets(node: &Node) -> Value {
    let v: Value = std::fs::read_to_string(presets_file(node)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({ "list": [], "active": "" }));
    let list: Vec<Value> = v["list"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut p| {
            let id = p["id"].as_str().unwrap_or("").to_string();
            p["img"] = json!(preset_img_file(node, &id).and_then(|f| std::fs::read(f).ok()).map(|b| crate::avatar::hash_of(&b)).unwrap_or_default());
            p
        })
        .collect();
    // выбранная — только если статус плитки всё ещё её (могли поменять через API)
    let st = own_style(node);
    let active = v["active"].as_str().unwrap_or("").to_string();
    let still = list.iter().any(|p| p["id"] == active.as_str() && st.get("stText").and_then(|x| x.as_str()).unwrap_or("") == p["text"].as_str().unwrap_or(""));
    json!({ "list": list, "active": if still { active } else { String::new() } })
}

/// Сохранить заготовки (до 20): текст до 255 знаков, цвет #rrggbb. Картинки — отдельно (set_preset_img).
pub fn set_presets(node: &Arc<Node>, v: &Value) -> Result<Value, String> {
    let arr = v["list"].as_array().ok_or("list — массив заготовок")?;
    if arr.len() > 20 {
        return Err("заготовок — до 20".into());
    }
    let mut out = Vec::new();
    for p in arr {
        let id = p["id"].as_str().unwrap_or("").to_string();
        if preset_img_file(node, &id).is_none() {
            return Err("id заготовки: латиница, цифры и -".into());
        }
        let text = clean_status_text(p["text"].as_str().unwrap_or(""));
        let c = color(&p["color"]).ok_or("color: цвет #rrggbb")?;
        out.push(json!({ "id": id, "text": text, "color": c, "silent": p["silent"].as_bool().unwrap_or(false) }));
    }
    // убранные — без их картинок
    let keep: Vec<String> = out.iter().map(|p| p["id"].as_str().unwrap_or("").to_string()).collect();
    if let Ok(rd) = std::fs::read_dir(node.data_dir.join("status-img")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().trim_end_matches(".img").to_string();
            if !keep.contains(&n) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let old: Value = std::fs::read_to_string(presets_file(node)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let active = old["active"].as_str().unwrap_or("").to_string();
    let active = if keep.contains(&active) { active } else { String::new() };
    std::fs::write(presets_file(node), json!({ "list": out, "active": active }).to_string()).map_err(|e| e.to_string())?;
    // выбранную поправили — обновить и статус плитки
    if !active.is_empty() {
        apply_preset(node, &active)?;
    }
    Ok(presets(node))
}

pub fn set_preset_img(node: &Arc<Node>, id: &str, b: &[u8]) -> Result<String, String> {
    let f = preset_img_file(node, id).ok_or("id заготовки")?;
    if b.len() > ST_MAX {
        return Err("значок статуса больше 256 КБ".into());
    }
    if crate::avatar::mime_of(b).is_none() {
        return Err("нужна картинка: JPEG, PNG, WebP или GIF".into());
    }
    let _ = std::fs::create_dir_all(f.parent().unwrap());
    std::fs::write(&f, b).map_err(|e| e.to_string())?;
    if presets(node)["active"] == id {
        apply_preset(node, id)?;
    }
    Ok(crate::avatar::hash_of(b))
}

pub fn remove_preset_img(node: &Arc<Node>, id: &str) -> Result<(), String> {
    let f = preset_img_file(node, id).ok_or("id заготовки")?;
    let _ = std::fs::remove_file(f);
    if presets(node)["active"] == id {
        apply_preset(node, id)?;
    }
    Ok(())
}

pub fn preset_img(node: &Node, id: &str) -> Option<Vec<u8>> {
    std::fs::read(preset_img_file(node, id)?).ok()
}

/// Выбрать заготовку (пусто — без статуса): текст, цвет и значок уходят в плитку — соседи увидят.
pub fn apply_preset(node: &Arc<Node>, id: &str) -> Result<Value, String> {
    let f = presets_file(node);
    let mut v: Value = std::fs::read_to_string(&f).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({ "list": [], "active": "" }));
    if id.is_empty() {
        set_status(node, &json!({ "text": "", "color": "", "silent": false }))?;
        let _ = std::fs::remove_file(st_file(node));
        v["active"] = json!("");
    } else {
        let p = v["list"].as_array().and_then(|a| a.iter().find(|p| p["id"] == id).cloned()).ok_or("нет такой заготовки")?;
        set_status(node, &json!({ "text": p["text"], "color": p["color"], "silent": p["silent"].as_bool().unwrap_or(false) }))?;
        match preset_img(node, id) {
            Some(b) => {
                std::fs::write(st_file(node), b).map_err(|e| e.to_string())?;
            }
            None => {
                let _ = std::fs::remove_file(st_file(node));
            }
        }
        v["active"] = json!(id);
    }
    std::fs::write(&f, v.to_string()).map_err(|e| e.to_string())?;
    node.notify_all("avatar");
    Ok(presets(node))
}

/// Полная тишина на этом ПК (статус с флажком «тишина»): приложение не издаёт ни звука.
pub fn silent(node: &Node) -> bool {
    own_style(node).get("stSilent").and_then(|x| x.as_bool()).unwrap_or(false)
}

/// У соседа полная тишина (по его плитке в кэше).
pub fn peer_silent(node: &Node, peer: &str) -> bool {
    std::fs::read_to_string(cache(node, peer, "json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).map(|v| v["hash"] == node.peer_state(peer).tile.as_str() && v["style"]["stSilent"] == true).unwrap_or(false)
}

pub fn remove_st_img(node: &Arc<Node>) {
    let _ = std::fs::remove_file(st_file(node));
    node.notify_all("avatar");
}

pub fn remove_img(node: &Arc<Node>) {
    let _ = std::fs::remove_file(img_file(node));
    node.notify_all("avatar");
}

/// Сбросить плитку целиком (стиль и картинку).
pub fn reset(node: &Arc<Node>) {
    let _ = std::fs::remove_file(style_file(node));
    let _ = std::fs::remove_file(img_file(node));
    let _ = std::fs::remove_file(st_file(node));
    node.notify_all("avatar");
}

/// Плитка соседа {style, img}: из кэша, если отпечаток тот же, иначе забрать у него.
pub async fn of_peer(node: &Arc<Node>, peer: &str) -> Option<Value> {
    let want = node.peer_state(peer).tile;
    if want.is_empty() {
        return None;
    }
    let f = cache(node, peer, "json");
    if let Some(v) = std::fs::read_to_string(&f).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        if v["hash"] == want {
            return Some(json!({ "style": v["style"], "img": v["img"], "stImg": v["stImg"] }));
        }
    }
    let p = node.cfg.read().peer(peer).cloned()?;
    let client = node.client_for(&p);
    let (code, t) = tokio::time::timeout(std::time::Duration::from_secs(10), client.get_text("/peer/tile")).await.ok()?.ok()?;
    if code != 200 {
        return None;
    }
    let v: Value = serde_json::from_str(&t).ok()?;
    let style = clean(&v["style"]).unwrap_or_else(|_| json!({}));
    let img = v["img"].as_str().unwrap_or("").to_string();
    let sti = v["stImg"].as_str().unwrap_or("").to_string();
    let _ = std::fs::create_dir_all(f.parent()?);
    let _ = std::fs::write(&f, json!({ "hash": want, "style": style, "img": img, "stImg": sti }).to_string());
    Some(json!({ "style": style, "img": img, "stImg": sti }))
}

/// Картинка плитки соседа: из кэша, если отпечаток совпадает, иначе забрать.
pub async fn img_of_peer(node: &Arc<Node>, peer: &str) -> Option<Vec<u8>> {
    let t = of_peer(node, peer).await?;
    let want = t["img"].as_str().unwrap_or("").to_string();
    if want.is_empty() {
        return None;
    }
    let f = cache(node, peer, "img");
    if let Ok(b) = std::fs::read(&f) {
        if crate::avatar::hash_of(&b) == want {
            return Some(b);
        }
    }
    let p = node.cfg.read().peer(peer).cloned()?;
    let client = node.client_for(&p);
    let (code, b) = tokio::time::timeout(std::time::Duration::from_secs(20), client.get_bytes("/peer/tile/img")).await.ok()?.ok()?;
    if code != 200 || b.len() > IMG_MAX || crate::avatar::mime_of(&b).is_none() {
        return None;
    }
    let _ = std::fs::write(&f, &b);
    Some(b)
}

/// Свой статус для панели (и для состояния канала): {text, color, silent, img — отпечаток значка}; null — без статуса.
pub fn own_status(node: &Node) -> Value {
    let st = own_style(node);
    let img = st_hash(node);
    if st.get("stText").is_none() && st.get("stColor").is_none() && st.get("stSilent").is_none() && img.is_empty() {
        return Value::Null;
    }
    json!({ "text": st["stText"].as_str().unwrap_or(""), "color": st["stColor"].as_str().unwrap_or(""), "silent": st["stSilent"].as_bool().unwrap_or(false), "img": img })
}

/// Значок статуса соседа по отпечатку `want` (из состояния канала): кэш, иначе — сразу у соседа, не дожидаясь обмена.
pub async fn st_img_fresh(node: &Arc<Node>, peer: &str, want: &str) -> Option<Vec<u8>> {
    if want.is_empty() {
        return st_img_of_peer(node, peer).await;
    }
    let f = cache(node, peer, "st");
    if let Ok(b) = std::fs::read(&f) {
        if crate::avatar::hash_of(&b) == want {
            return Some(b);
        }
    }
    let p = node.cfg.read().peer(peer).cloned()?;
    let client = node.client_for(&p);
    let (code, b) = tokio::time::timeout(std::time::Duration::from_secs(15), client.get_bytes("/peer/tile/stimg")).await.ok()?.ok()?;
    if code != 200 || b.len() > ST_MAX || crate::avatar::mime_of(&b).is_none() {
        return None;
    }
    let _ = std::fs::create_dir_all(f.parent()?);
    let _ = std::fs::write(&f, &b);
    Some(b)
}

/// Значок статуса соседа: из кэша, если отпечаток совпадает, иначе забрать.
pub async fn st_img_of_peer(node: &Arc<Node>, peer: &str) -> Option<Vec<u8>> {
    let t = of_peer(node, peer).await?;
    let want = t["stImg"].as_str().unwrap_or("").to_string();
    if want.is_empty() {
        return None;
    }
    let f = cache(node, peer, "st");
    if let Ok(b) = std::fs::read(&f) {
        if crate::avatar::hash_of(&b) == want {
            return Some(b);
        }
    }
    let p = node.cfg.read().peer(peer).cloned()?;
    let client = node.client_for(&p);
    let (code, b) = tokio::time::timeout(std::time::Duration::from_secs(15), client.get_bytes("/peer/tile/stimg")).await.ok()?.ok()?;
    if code != 200 || b.len() > ST_MAX || crate::avatar::mime_of(&b).is_none() {
        return None;
    }
    let _ = std::fs::write(&f, &b);
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clean_style() {
        let v = clean(&json!({ "fit": "cover", "opacity": 300, "tint": "#abcdef", "evil": "<script>", "mask": "bottom" })).unwrap();
        assert_eq!(v["opacity"], 100.0);
        assert_eq!(v["tint"], "#ABCDEF");
        assert!(v.get("evil").is_none());
        assert!(clean(&json!({ "fit": "nope" })).is_err());
        assert!(clean(&json!({ "glow": "red" })).is_err());
    }
}
