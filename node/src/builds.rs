//! Сборки (3.3.1): папка программы (например, сборка Unity) уходит соседу одним zip и распаковывается у него.
//! Два режима:
//! - патч — только файлы, изменившиеся с прошлой отправки этой сборки этому соседу, распаковка с заменой;
//! - чистая — всё целиком; у получателя прежняя папка сборки уходит в корзину, на её место — присланное.
//! Транспорт — ручная отправка: архив кладётся в «отправка/<сосед>/_сборки/<имя>/<режим>-<время>.zip»,
//! получатель, приняв такой файл, распаковывает его в папку сборки (по умолчанию «сборки/<имя>», можно задать свою).
//!
//! (3.3.5) Заготовки сборок: папка, кому (можно нескольким), режим и игнор — шаблоны как в .gitignore
//! (`Library/`, `*_BurstDebugInformation_DoNotShip/`, `**/*.pdb`, `!нужное.pdb` — вернуть исключённое).

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
    /// (3.8) Запущенные присланные программы: «сосед/имя» → процесс.
    runs: Mutex<BTreeMap<String, std::process::Child>>,
}

/// Что запускать в сборке: путь внутри папки сборки (без «..» и абсолютных), пусто — ничего.
pub fn clean_exe(p: &str) -> Result<String, String> {
    let p = p.trim().replace('\\', "/");
    let p = p.trim_start_matches("./");
    if p.is_empty() {
        return Ok(String::new());
    }
    if p.chars().count() > 300 {
        return Err("путь программы — до 300 знаков".into());
    }
    path::norm(p).ok_or_else(|| "программа — путь внутри папки сборки (например, CORA.exe или bin/Game.exe)".to_string())
}

/// Похожее на запускаемое в папке сборки (для подсказки в заготовке).
fn exe_candidates(src: &Path, ig: &BuildIgnore) -> Vec<String> {
    let mut v: Vec<String> = bridge_core::scanner::walk(src, &[])
        .into_iter()
        .filter(|w| ig.which(&w.rel).is_none() && w.rel.split('/').count() <= 3)
        .map(|w| w.rel)
        .filter(|r| {
            let l = r.to_lowercase();
            (l.ends_with(".exe") || l.ends_with(".x86_64") || l.ends_with(".sh") || l.ends_with(".bat") || l.ends_with(".cmd")) && !l.contains("unitycrashhandler") && !l.contains("/monobleedingedge/")
        })
        .collect();
    v.sort_by_key(|r| (r.matches('/').count(), r.to_lowercase()));
    v.truncate(20);
    v
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
fn presets_file(node: &Node) -> PathBuf {
    dir(node).join("presets.json")
}

// ───────────── заготовки ─────────────

/// Игнор сборки: общий игнор моста + свои шаблоны; `!шаблон` возвращает исключённое.
pub struct BuildIgnore {
    global: Arc<bridge_core::ignore::IgnoreRules>,
    own: bridge_core::ignore::IgnoreRules,
    keep: bridge_core::ignore::IgnoreRules,
}
impl BuildIgnore {
    pub fn new(node: &Node, lines: &[String]) -> Self {
        let own = lines.iter().filter(|l| !l.trim_start().starts_with('!')).cloned();
        let keep = lines.iter().filter_map(|l| l.trim_start().strip_prefix('!').map(|x| x.to_string()));
        BuildIgnore { global: node.cfg.read().global_ignore(), own: bridge_core::ignore::IgnoreRules::new(own), keep: bridge_core::ignore::IgnoreRules::new(keep) }
    }
    /// Какое правило исключает файл (None — файл идёт в архив).
    pub fn which(&self, rel: &str) -> Option<String> {
        if let Some(r) = self.global.which(rel) {
            return Some(format!("общий игнор: {}", r));
        }
        let r = self.own.which(rel)?;
        if self.keep.is_ignored(rel) {
            return None;
        }
        Some(r.to_string())
    }
}

fn lines_of(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).map(|x| x.to_string()).collect(),
        Value::String(t) => t.lines().map(|x| x.to_string()).collect(),
        _ => vec![],
    }
}

pub fn presets(node: &Node) -> Vec<Value> {
    read(&presets_file(node)).as_array().cloned().unwrap_or_default()
}
fn preset(node: &Node, id: &str) -> Option<Value> {
    presets(node).into_iter().find(|p| p["id"] == id)
}

/// Сохранить заготовки целиком: [{id, name, path, peers: [..], mode, ignore: [строки]}] (до 50).
pub fn set_presets(node: &Node, list: &Value) -> Result<Vec<Value>, String> {
    let a = list.as_array().ok_or("list: массив заготовок")?;
    if a.len() > 50 {
        return Err("заготовок — не больше 50".into());
    }
    let cfg = node.cfg.read();
    let mut out: Vec<Value> = Vec::new();
    for p in a {
        let id = p["id"].as_str().unwrap_or("").trim().to_string();
        if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("id заготовки «{}»: латиница, цифры, - и _ (до 40)", id));
        }
        if out.iter().any(|x| x["id"] == id.as_str()) {
            return Err(format!("id «{}» повторяется", id));
        }
        let name = clean_name(p["name"].as_str().unwrap_or("")).map_err(|e| format!("«{}»: {}", p["name"].as_str().unwrap_or(""), e))?;
        let path = p["path"].as_str().unwrap_or("").trim().to_string();
        if path.chars().count() > 1000 {
            return Err(format!("«{}»: слишком длинный путь", name));
        }
        let mut peers: Vec<String> = Vec::new();
        for x in p["peers"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str()) {
            let real = cfg.peer(x).map(|pc| pc.name.clone()).unwrap_or_else(|| x.to_string());
            if !peers.iter().any(|y| eq_ic(y, &real)) {
                peers.push(real);
            }
        }
        let mode = if p["mode"] == "clean" { "clean" } else { "patch" };
        let exe = clean_exe(p["exe"].as_str().unwrap_or("")).map_err(|e| format!("«{}»: {}", name, e))?;
        let ignore: Vec<String> = lines_of(&p["ignore"]).into_iter().map(|l| l.trim_end().to_string()).collect();
        if ignore.len() > 500 {
            return Err(format!("«{}»: в игноре не больше 500 строк", name));
        }
        let pats: Vec<String> = ignore.iter().map(|l| l.trim_start().trim_start_matches('!').to_string()).collect();
        if let Some(e) = bridge_core::ignore::validate(&pats, &format!("игнор «{}»", name)).into_iter().next() {
            return Err(e);
        }
        out.push(json!({ "id": id, "name": name, "path": path, "peers": peers, "mode": mode, "ignore": ignore, "exe": exe }));
    }
    drop(cfg);
    write(&presets_file(node), &json!(out));
    node.emit("builds", json!({}));
    Ok(out)
}

/// Что войдёт в архив: файлы и объём; что отсеет игнор — по правилам. Тяжёлое — звать из spawn_blocking.
pub fn preview(node: &Node, src: &str, ignore: &[String]) -> Result<Value, String> {
    let src = PathBuf::from(src.trim());
    if !src.is_absolute() || !src.is_dir() {
        return Err(format!("нет папки «{}» (нужен полный путь)", src.display()));
    }
    let ig = BuildIgnore::new(node, ignore);
    let (mut files, mut bytes) = (0i64, 0i64);
    let mut by: BTreeMap<String, (i64, i64, String)> = BTreeMap::new();
    let mut biggest: Vec<(i64, String)> = Vec::new();
    for w in bridge_core::scanner::walk(&src, &[]) {
        match ig.which(&w.rel) {
            Some(r) => {
                let e = by.entry(r).or_insert((0, 0, w.rel.clone()));
                e.0 += 1;
                e.1 += w.size;
            }
            None => {
                files += 1;
                bytes += w.size;
                biggest.push((w.size, w.rel.clone()));
            }
        }
    }
    biggest.sort_by(|a, b| b.0.cmp(&a.0));
    let mut rules: Vec<Value> = by.into_iter().map(|(r, (n, b, ex))| json!({ "rule": r, "files": n, "bytes": b, "example": ex })).collect();
    rules.sort_by(|a, b| b["bytes"].as_i64().cmp(&a["bytes"].as_i64()));
    let (ign_f, ign_b) = rules.iter().fold((0i64, 0i64), |a, r| (a.0 + r["files"].as_i64().unwrap_or(0), a.1 + r["bytes"].as_i64().unwrap_or(0)));
    Ok(json!({ "files": files, "bytes": bytes, "ignoredFiles": ign_f, "ignoredBytes": ign_b, "rules": rules, "biggest": biggest.into_iter().take(8).map(|(s, p)| json!({ "path": p, "bytes": s })).collect::<Vec<_>>(), "exes": exe_candidates(&src, &ig) }))
}

/// Отправить по заготовке всем её получателям.
pub fn send_preset(node: &Arc<Node>, id: &str, mode: Option<&str>, note: &str) -> Result<Value, (u16, String)> {
    let p = preset(node, id).ok_or((404, "нет такой заготовки".to_string()))?;
    let peers: Vec<String> = p["peers"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(String::from)).collect();
    if peers.is_empty() {
        return Err((400, "у заготовки не выбрано, кому отправлять".into()));
    }
    let mode = mode.filter(|m| !m.is_empty()).unwrap_or(p["mode"].as_str().unwrap_or("patch")).to_string();
    let ignore = lines_of(&p["ignore"]);
    let (mut ok, mut errs) = (Vec::new(), Vec::new());
    for peer in &peers {
        match send_full(node, peer, p["path"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or(""), &mode, &ignore, id, p["exe"].as_str().unwrap_or(""), note) {
            Ok(_) => ok.push(peer.clone()),
            Err((_, e)) => errs.push(format!("{}: {}", peer, e)),
        }
    }
    if ok.is_empty() {
        return Err((400, errs.join("; ")));
    }
    Ok(json!({ "ok": true, "started": ok, "errors": errs }))
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
    let all_in: Vec<Value> = h.as_array().cloned().unwrap_or_default().into_iter().filter(|e| e["dir"] == "in").collect();
    let mut received: Vec<Value> = recv
        .values()
        .map(|e| {
            let (p, n) = (e["peer"].as_str().unwrap_or(""), e["name"].as_str().unwrap_or(""));
            let mut e = e.clone();
            let target = target_dir(node, p, n);
            e["target"] = json!(target.display().to_string());
            // история версий этой сборки (новые — первыми) и что запускать (из последней удачной)
            let hist: Vec<Value> = all_in.iter().filter(|x| x["peer"] == p && x["name"] == n).rev().take(30).cloned().collect();
            let last_ok = hist.iter().find(|x| x["error"].is_null());
            let exe = last_ok.and_then(|x| x["exe"].as_str()).unwrap_or("").to_string();
            e["exe"] = json!(exe);
            e["exeExists"] = json!(!exe.is_empty() && path::to_local(&target, &exe).map(|f| f.is_file()).unwrap_or(false));
            e["lastOk"] = last_ok.cloned().unwrap_or(Value::Null);
            e["history"] = json!(hist);
            e["running"] = json!(running(node, p, n));
            e
        })
        .collect();
    received.sort_by(|a, b| b["at"].as_str().unwrap_or("").cmp(a["at"].as_str().unwrap_or("")));
    json!({
        "history": h.as_array().cloned().unwrap_or_default().into_iter().rev().take(50).collect::<Vec<_>>(),
        "received": received,
        "jobs": node.builds.jobs.lock().values().cloned().collect::<Vec<_>>(),
        "presets": presets(node),
        "defaultRoot": node.base_dir.join("сборки").display().to_string(),
    })
}

// ───────────── запуск присланного ─────────────

fn run_key(peer: &str, name: &str) -> String {
    format!("{}/{}", peer.to_lowercase(), name)
}

pub fn running(node: &Node, peer: &str, name: &str) -> bool {
    let mut g = node.builds.runs.lock();
    let k = run_key(peer, name);
    match g.get_mut(&k).map(|c| c.try_wait()) {
        Some(Ok(None)) => true,
        Some(_) => {
            g.remove(&k);
            false
        }
        None => false,
    }
}

/// Запустить программу присланной сборки (то, что указал отправитель). Только с этого ПК, по нажатию человека.
pub fn run(node: &Node, peer: &str, name: &str) -> Result<Value, String> {
    if let Some(g) = node.remote.active() {
        return Err(format!("пока этим ПК управляет {}, программы не запускаются", g.peer));
    }
    let name = clean_name(name)?;
    if running(node, peer, &name) {
        return Ok(json!({ "ok": true, "already": true }));
    }
    let st = status(node);
    let r = st["received"].as_array().and_then(|a| a.iter().find(|x| eq_ic(x["peer"].as_str().unwrap_or(""), peer) && x["name"] == name.as_str()).cloned()).ok_or("такой сборки здесь нет")?;
    let exe = r["exe"].as_str().unwrap_or("");
    if exe.is_empty() {
        return Err("отправитель не указал, что запускать".into());
    }
    let target = target_dir(node, peer, &name);
    let full = path::to_local(&target, exe).map_err(|_| "программа вне папки сборки".to_string())?;
    if !full.is_file() {
        return Err(format!("нет файла «{}» в папке сборки", exe));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&full, std::fs::Permissions::from_mode(0o755));
    }
    let child = std::process::Command::new(&full)
        .current_dir(full.parent().unwrap_or(&target))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("не запустилось «{}»: {}", exe, e))?;
    let pid = child.id();
    node.builds.runs.lock().insert(run_key(peer, &name), child);
    node.info(&format!("[сборки] запущено «{}» из сборки «{}» от {} (pid {})", exe, name, peer, pid));
    node.emit("builds", json!({}));
    Ok(json!({ "ok": true, "pid": pid }))
}

pub fn stop(node: &Node, peer: &str, name: &str) -> Result<Value, String> {
    let c = node.builds.runs.lock().remove(&run_key(peer, name));
    match c {
        Some(mut c) => {
            let _ = c.kill();
            let _ = c.wait();
            node.info(&format!("[сборки] остановлено: сборка «{}» от {}", name, peer));
            node.emit("builds", json!({}));
            Ok(json!({ "ok": true }))
        }
        None => Ok(json!({ "ok": true, "already": true })),
    }
}

// ───────────── отправка ─────────────

/// Упаковать папку и поставить в отправку. Долгое — идёт в фоне; ход виден в `jobs`.
pub fn send(node: &Arc<Node>, peer: &str, src: &str, name: &str, mode: &str) -> Result<Value, (u16, String)> {
    send_ex(node, peer, src, name, mode, &[], "")
}

pub fn send_ex(node: &Arc<Node>, peer: &str, src: &str, name: &str, mode: &str, ignore: &[String], preset: &str) -> Result<Value, (u16, String)> {
    send_full(node, peer, src, name, mode, ignore, preset, "", "")
}

/// Отправка с программой для запуска (`exe` — путь внутри сборки) и заметкой «что нового» (до 2000 знаков).
#[allow(clippy::too_many_arguments)]
pub fn send_full(node: &Arc<Node>, peer: &str, src: &str, name: &str, mode: &str, ignore: &[String], preset: &str, exe: &str, note: &str) -> Result<Value, (u16, String)> {
    let exe = clean_exe(exe).map_err(|e| (400, e))?;
    let note: String = note.trim().chars().take(2000).collect();
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
    node.builds.jobs.lock().insert(key.clone(), json!({ "peer": pc.name, "name": name, "mode": mode, "preset": preset, "stage": "считаю файлы", "done": 0, "total": 0 }));
    let n2 = node.clone();
    let (peer, mode2, ignore, preset) = (pc.name.clone(), mode.to_string(), ignore.to_vec(), preset.to_string());
    tokio::task::spawn_blocking(move || {
        let r = pack(&n2, &peer, &src, &name, &mode2, &key, &ignore, &exe, &note).map(|mut e| {
            e["preset"] = json!(preset);
            e
        });
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
                remember(&n2, json!({ "dir": "out", "peer": peer, "name": name, "mode": mode2, "preset": preset, "at": time::format(&time::now()), "error": e }));
            }
        }
    });
    Ok(json!({ "ok": true, "started": true }))
}

#[allow(clippy::too_many_arguments)]
fn pack(node: &Arc<Node>, peer: &str, src: &Path, name: &str, mode: &str, key: &str, ignore: &[String], exe: &str, note: &str) -> Result<Value, String> {
    let ig = BuildIgnore::new(node, ignore);
    let all = bridge_core::scanner::walk(src, &[]);
    let n_all = all.len();
    let files: Vec<_> = all.into_iter().filter(|w| ig.which(&w.rel).is_none()).collect();
    let skipped = n_all - files.len();
    if files.is_empty() {
        return Err(if skipped > 0 { "всё в папке отсеяно игнором".into() } else { "папка пуста".into() });
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
    let prev_v = read(&sf);
    let prev: BTreeMap<String, String> = serde_json::from_value(prev_v["files"].clone()).unwrap_or_default();
    // номер версии этой сборки для этого соседа (растёт с каждой отправкой)
    let n = prev_v["n"].as_u64().unwrap_or(0) + 1;
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
        let meta = json!({ "name": name, "mode": mode, "from": node.node_name(), "at": time::format(&time::now()), "files": pick.len(), "all": now.len(), "first": first, "exe": exe, "note": note, "n": n });
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
    write(&sf, &json!({ "files": now, "at": time::format(&time::now()), "n": n }));
    Ok(json!({ "dir": "out", "peer": peer, "name": name, "mode": if first && mode == "patch" { "clean" } else { mode }, "at": time::format(&time::now()), "files": pick.len(), "all": now.len(), "bytes": bytes, "zip": zsize, "src": src.display().to_string(), "first": first, "skipped": skipped, "exe": exe, "note": note, "n": n }))
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
    let exe = clean_exe(meta["exe"].as_str().unwrap_or("")).unwrap_or_default();
    let note: String = meta["note"].as_str().unwrap_or("").chars().take(2000).collect();
    Ok(json!({ "dir": "in", "peer": peer, "name": name, "mode": mode, "at": time::format(&time::now()), "files": files, "bytes": bytes, "target": target.display().to_string(), "old": old, "sentAt": meta["at"], "exe": exe, "note": note, "n": meta["n"] }))
}
