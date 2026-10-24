//! Сборки (3.3.1): папка программы (например, сборка Unity) уходит соседу одним zip и распаковывается у него.
//! Два режима:
//! - патч — только файлы, изменившиеся с прошлой отправки этой сборки этому соседу, распаковка с заменой;
//! - чистая — всё целиком; у получателя прежняя папка сборки уходит в корзину, на её место — присланное.
//! Транспорт — ручная отправка: архив кладётся в «отправка/<сосед>/_сборки/<имя>/<режим>-<время>.zip»,
//! получатель, приняв такой файл, распаковывает его в папку сборки (по умолчанию «сборки/<имя>», можно задать свою).

use crate::node::Node;
use bridge_core::time;
use bridge_core::{eq_ic, path};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Папка в отправке/приёме, по которой узнаются сборки.
pub const DIR: &str = "_сборки";
const META: &str = "_most-build.json";

#[derive(Default)]
pub struct Builds {
    /// Что сейчас упаковывается: ключ «сосед/имя» → ход.
    pub jobs: Mutex<BTreeMap<String, Value>>,
    lock: Mutex<()>,
}

fn dir(node: &Node) -> PathBuf {
    node.data_dir.join("builds")
}
fn read(p: &Path) -> Value {
    std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}
fn write(p: &Path, v: &Value) {
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = bridge_core::write_atomic(p, &v.to_string());
}

/// Имя сборки: буквы, цифры, пробел, . _ - (до 60).
pub fn clean_name(n: &str) -> Result<String, String> {
    let n = n.trim();
    if n.is_empty() || n.chars().count() > 60 || n.starts_with('.') || !n.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-')) {
        return Err("имя сборки: буквы, цифры, пробел, точка, _ и - (до 60)".into());
    }
    Ok(n.to_string())
}

fn sent_file(node: &Node, peer: &str, name: &str) -> PathBuf {
    dir(node).join("sent").join(format!("{}--{}.json", crate::mobile::folder_name(peer), crate::mobile::folder_name(name)))
}
fn history_file(node: &Node) -> PathBuf {
    dir(node).join("history.json")
}
fn targets_file(node: &Node) -> PathBuf {
    dir(node).join("targets.json")
}

fn remember(node: &Node, e: Value) {
    let f = history_file(node);
    let mut h: Vec<Value> = read(&f).as_array().cloned().unwrap_or_default();
    h.push(e);
    if h.len() > 200 {
        h.drain(0..h.len() - 200);
    }
    write(&f, &json!(h));
    node.emit("builds", json!({}));
}

/// Куда распаковывать сборку `name` от `peer`.
pub fn target_dir(node: &Node, peer: &str, name: &str) -> PathBuf {
    let t = read(&targets_file(node));
    if let Some(p) = t[format!("{}/{}", peer.to_lowercase(), name)].as_str().filter(|p| !p.trim().is_empty()) {
        return node.resolve(p);
    }
    node.base_dir.join("сборки").join(crate::mobile::folder_name(name))
}

pub fn set_target(node: &Node, peer: &str, name: &str, p: &str) -> Result<(), String> {
    let name = clean_name(name)?;
    let f = targets_file(node);
    let mut t = read(&f);
    if !t.is_object() {
        t = json!({});
    }
    let key = format!("{}/{}", peer.to_lowercase(), name);
    if p.trim().is_empty() {
        t.as_object_mut().unwrap().remove(&key);
    } else {
        let pb = PathBuf::from(p.trim());
        if !pb.is_absolute() {
            return Err("нужен полный путь, например D:\\Сборки\\CORA".into());
        }
        t[key] = json!(p.trim());
    }
    write(&f, &t);
    Ok(())
}

/// Состояние для панели.
pub fn status(node: &Node) -> Value {
    let h = read(&history_file(node));
    let mut recv: BTreeMap<String, Value> = BTreeMap::new();
    for e in h.as_array().cloned().unwrap_or_default().iter().filter(|e| e["dir"] == "in") {
        let k = format!("{}/{}", e["peer"].as_str().unwrap_or(""), e["name"].as_str().unwrap_or(""));
        recv.insert(k, e.clone());
    }
    let received: Vec<Value> = recv
        .values()
        .map(|e| {
            let (p, n) = (e["peer"].as_str().unwrap_or(""), e["name"].as_str().unwrap_or(""));
            let mut e = e.clone();
            e["target"] = json!(target_dir(node, p, n).display().to_string());
            e
        })
        .collect();
    json!({
        "history": h.as_array().cloned().unwrap_or_default().into_iter().rev().take(50).collect::<Vec<_>>(),
        "received": received,
        "jobs": node.builds.jobs.lock().values().cloned().collect::<Vec<_>>(),
        "defaultRoot": node.base_dir.join("сборки").display().to_string(),
    })
}

// ───────────── отправка ─────────────

/// Упаковать папку и поставить в отправку. Долгое — идёт в фоне; ход виден в `jobs`.
pub fn send(node: &Arc<Node>, peer: &str, src: &str, name: &str, mode: &str) -> Result<Value, (u16, String)> {
    let pc = node.cfg.read().peer(peer).cloned().ok_or((404, format!("нет соседа «{}»", peer)))?;
    let name = clean_name(name).map_err(|e| (400, e))?;
    if mode != "patch" && mode != "clean" {
        return Err((400, "mode: patch (только изменённое) или clean (целиком)".into()));
    }
    let src = PathBuf::from(src.trim());
    if !src.is_absolute() || !src.is_dir() {
        return Err((400, format!("нет папки «{}» (нужен полный путь)", src.display())));
    }
    let key = format!("{}/{}", pc.name, name);
    if node.builds.jobs.lock().contains_key(&key) {
        return Err((409, "эта сборка этому соседу уже упаковывается".into()));
    }
    node.builds.jobs.lock().insert(key.clone(), json!({ "peer": pc.name, "name": name, "mode": mode, "stage": "считаю файлы", "done": 0, "total": 0 }));
    let n2 = node.clone();
    let (peer, mode2) = (pc.name.clone(), mode.to_string());
    tokio::task::spawn_blocking(move || {
        let r = pack(&n2, &peer, &src, &name, &mode2, &key);
        n2.builds.jobs.lock().remove(&key);
        match r {
            Ok(e) => {
                n2.info(&format!("[сборки] «{}» → {}: {} ({} файлов, {})", name, peer, if mode2 == "clean" { "чистая" } else { "патч" }, e["files"], crate::folders::human(e["bytes"].as_i64().unwrap_or(0))));
                remember(&n2, e);
                n2.mailbox.dirty.store(true, std::sync::atomic::Ordering::SeqCst);
                n2.notify_peers(vec![peer.clone()], "", "mailbox");
            }
            Err(e) => {
                n2.info(&format!("[сборки] «{}» → {}: не упаковалась — {}", name, peer, e));
                remember(&n2, json!({ "dir": "out", "peer": peer, "name": name, "mode": mode2, "at": time::format(&time::now()), "error": e }));
            }
        }
    });
    Ok(json!({ "ok": true, "started": true }))
}

fn pack(node: &Arc<Node>, peer: &str, src: &Path, name: &str, mode: &str, key: &str) -> Result<Value, String> {
    let ig = node.cfg.read().global_ignore();
    let files: Vec<_> = bridge_core::scanner::walk(src, &[]).into_iter().filter(|w| ig.which(&w.rel).is_none()).collect();
    if files.is_empty() {
        return Err("папка пуста".into());
    }
    let total = files.len();
    let set_job = |stage: &str, done: usize| {
        if let Some(j) = node.builds.jobs.lock().get_mut(key) {
            j["stage"] = json!(stage);
            j["done"] = json!(done);
            j["total"] = json!(total);
        }
    };
    // отпечатки — чтобы патч взял только изменённое
    let mut now: BTreeMap<String, String> = BTreeMap::new();
    for (i, w) in files.iter().enumerate() {
        let sha = path::sha256_file(&w.full).map_err(|e| format!("{}: {}", w.rel, e))?;
        now.insert(w.rel.clone(), sha);
        if i % 50 == 0 {
            set_job("сверяю файлы", i);
        }
    }
    let sf = sent_file(node, peer, name);
    let prev: BTreeMap<String, String> = serde_json::from_value(read(&sf)["files"].clone()).unwrap_or_default();
    let first = prev.is_empty();
    let pick: Vec<_> = files.iter().filter(|w| mode == "clean" || first || prev.get(&w.rel) != now.get(&w.rel)).collect();
    if pick.is_empty() {
        return Err("с прошлой отправки ничего не изменилось — патч пустой".into());
    }
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let rel = format!("{}/{}/{}-{}.zip", DIR, name, mode, ts);
    let out = path::to_local(&node.mailbox.out_root(node).join(peer), &rel).map_err(|_| "путь".to_string())?;
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = node.tmp_file();
    let mut bytes: i64 = 0;
    {
        let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut z = zip::ZipWriter::new(std::io::BufWriter::new(f));
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(3)).large_file(true);
        let meta = json!({ "name": name, "mode": mode, "from": node.node_name(), "at": time::format(&time::now()), "files": pick.len(), "all": now.len(), "first": first });
        z.start_file(META, opt).map_err(|e| e.to_string())?;
        z.write_all(meta.to_string().as_bytes()).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 1 << 20];
        for (i, w) in pick.iter().enumerate() {
            z.start_file(w.rel.as_str(), opt).map_err(|e| e.to_string())?;
            let mut f = std::fs::File::open(&w.full).map_err(|e| format!("{}: {}", w.rel, e))?;
            loop {
                let n = f.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                z.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            }
            bytes += w.size;
            if i % 20 == 0 {
                set_job("упаковываю", i);
            }
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    let zsize = std::fs::metadata(&tmp).map(|m| m.len() as i64).unwrap_or(0);
    let max = node.max_file_bytes();
    if zsize > max {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("архив {} больше предела файла (maxFileMb)", crate::folders::human(zsize)));
    }
    bridge_core::files::move_replace(&tmp, &out).map_err(|e| e.to_string())?;
    bridge_core::files::set_mtime(&out, &(time::now() - chrono::Duration::seconds(5)));
    // отпечатки — «что у соседа» после этой отправки
    write(&sf, &json!({ "files": now, "at": time::format(&time::now()) }));
    Ok(json!({ "dir": "out", "peer": peer, "name": name, "mode": if first && mode == "patch" { "clean" } else { mode }, "at": time::format(&time::now()), "files": pick.len(), "all": now.len(), "bytes": bytes, "zip": zsize, "src": src.display().to_string(), "first": first }))
}

// ───────────── приём ─────────────

/// Принятый файл ручной отправки: если это сборка — распаковать. `rel` — путь в «приём/<сосед>/».
pub fn on_received(node: &Arc<Node>, peer: &str, file: &Path, rel_in_peer: &str) {
    let parts: Vec<&str> = rel_in_peer.split('/').collect();
    if parts.len() != 3 || !eq_ic(parts[0], DIR) || !parts[2].to_lowercase().ends_with(".zip") {
        return;
    }
    let (n2, peer, file, name) = (node.clone(), peer.to_string(), file.to_path_buf(), parts[1].to_string());
    tokio::task::spawn_blocking(move || {
        let _g = n2.builds.lock.lock();
        let r = install(&n2, &peer, &file, &name);
        match r {
            Ok(e) => {
                n2.info(&format!("[сборки] «{}» от {}: распакована ({}, {} файлов) → {}", name, peer, if e["mode"] == "clean" { "чистая" } else { "патч" }, e["files"], e["target"].as_str().unwrap_or("")));
                remember(&n2, e);
                // архив своё дело сделал
                let _ = std::fs::remove_file(&file);
                if let Some(d) = file.parent() {
                    crate::mailbox::remove_empty_dirs(&n2.mailbox.in_root(&n2), d);
                }
            }
            Err(e) => {
                n2.info(&format!("[сборки] «{}» от {}: не распаковалась — {} (архив оставлен: {})", name, peer, e, file.display()));
                remember(&n2, json!({ "dir": "in", "peer": peer, "name": name, "at": time::format(&time::now()), "error": e, "zip": file.display().to_string() }));
            }
        }
    });
}

fn install(node: &Arc<Node>, peer: &str, zipf: &Path, name: &str) -> Result<Value, String> {
    let name = clean_name(name)?;
    let f = std::fs::File::open(zipf).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f)).map_err(|e| format!("не zip: {}", e))?;
    let meta: Value = match z.by_name(META) {
        Ok(mut e) => {
            let mut s = String::new();
            let _ = e.read_to_string(&mut s);
            serde_json::from_str(&s).unwrap_or(Value::Null)
        }
        Err(_) => Value::Null,
    };
    let mode = if meta["mode"] == "clean" || meta["first"] == true { "clean" } else { "patch" };
    let target = target_dir(node, peer, &name);
    // чистая: прежняя папка — в корзину (вернуть можно оттуда)
    let mut old = String::new();
    if mode == "clean" && target.is_dir() && std::fs::read_dir(&target).map(|mut d| d.next().is_some()).unwrap_or(false) {
        let to = node.folders.trash_dir().join("сборки").join(format!("{}--{}", crate::mobile::folder_name(&name), chrono::Local::now().format("%Y%m%d-%H%M%S")));
        if let Some(d) = to.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&target, &to).map_err(|e| format!("прежнюю папку сборки не убрать в корзину ({}): {}", target.display(), e))?;
        old = to.display().to_string();
    }
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let ig = node.cfg.read().global_ignore();
    let (mut files, mut bytes) = (0i64, 0i64);
    let mut buf = vec![0u8; 1 << 20];
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| e.to_string())?;
        if e.is_dir() || e.name() == META {
            continue;
        }
        // защита: только внутрь папки сборки, без «..» и абсолютных путей
        let Some(rel) = e.enclosed_name() else { continue };
        let rel_s = rel.to_string_lossy().replace('\\', "/");
        let Some(norm) = path::norm(&rel_s) else { continue };
        if ig.which(&norm).is_some() {
            continue;
        }
        let Ok(dest) = path::to_local(&target, &norm) else { continue };
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = dest.with_extension("most-part");
        {
            let mut out = std::fs::File::create(&tmp).map_err(|e| format!("{}: {}", norm, e))?;
            loop {
                let n = e.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                bytes += n as i64;
            }
        }
        bridge_core::files::move_replace(&tmp, &dest).map_err(|e| format!("{}: {} (файл занят? закройте программу)", norm, e))?;
        files += 1;
    }
    Ok(json!({ "dir": "in", "peer": peer, "name": name, "mode": mode, "at": time::format(&time::now()), "files": files, "bytes": bytes, "target": target.display().to_string(), "old": old, "sentAt": meta["at"] }))
}
