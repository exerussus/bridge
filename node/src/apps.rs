//! Приложения (3.4): свой набор на каждом ПК — веб-интерфейс в изолированной рамке, своё окно (сборка Unity, любой
//! exe), фоновая служба, кнопки. Папка «приложения» рядом с «отправкой»: в ней — папки с app.json или пакеты
//! `.most-app` (та же папка в zip). Мост следит за ней и подхватывает изменения без перезапуска.
//!
//! Ничего не запускается без согласия хозяина ПК: согласие привязано к отпечатку исполняемых файлов и списку прав
//! (сменились — мост спросит снова). Веб-интерфейс ходит в мост только через панель (postMessage, права проверяет
//! панель), служба и своё окно — по своему токену (`MOST_TOKEN`) с правами ровно этого приложения.
//! Пока этим ПК управляют по сети, приложения не запускаются.

use crate::node::Node;
use bridge_core::time;
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Папка с приложениями (в папке моста).
pub const FOLDER: &str = "приложения";
/// Пакет приложения — zip с app.json в корне.
pub const EXT: &str = "most-app";
const MANIFEST: &str = "app.json";
/// Права, которые может попросить приложение.
pub const PERMS: &[&str] = &["files.pick", "files.save", "files.read", "notify", "channel.post", "peers", "mail.send", "sessions", "activity", "input.capture"];
/// Служба с `run: "open"` гаснет, когда приложение не открыто дольше этого.
const IDLE_STOP: Duration = Duration::from_secs(90);
const LOG_LINES: usize = 400;
const STORE_MAX: usize = 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct App {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    /// Где лежат файлы (папка или распакованный пакет).
    pub root: PathBuf,
    /// Папка или пакет в «приложениях».
    pub source: PathBuf,
    pub packaged: bool,
    pub manifest: Value,
    /// Папка веб-интерфейса внутри root и файл входа в ней.
    pub ui_dir: String,
    pub ui_entry: String,
    pub icon: String,
    pub perms: Vec<String>,
    /// Отпечаток исполняемого: файлы вне ui/ + права и команды из app.json.
    pub hash: String,
    /// Меняется, когда меняются файлы интерфейса (панель перезагружает открытое).
    pub ui_stamp: String,
    pub unsupported: String,
}

#[derive(Clone)]
struct RunState {
    pid: u32,
    port: u16,
    since: time::Time,
    stop: CancellationToken,
}

#[derive(Default)]
struct Run {
    service: Option<RunState>,
    native: Option<RunState>,
    restarts: u32,
    last_exit: String,
    log: VecDeque<String>,
    touched: Option<Instant>,
}

#[derive(Default)]
pub struct Apps {
    list: Mutex<BTreeMap<String, App>>,
    /// Источники с ошибкой: путь → причина.
    errors: Mutex<BTreeMap<String, String>>,
    /// Отпечатки источников: чтобы не перечитывать то, что не менялось.
    seen: Mutex<BTreeMap<PathBuf, (String, Option<App>, String)>>,
    runs: Mutex<BTreeMap<String, Run>>,
    /// Токен службы/окна → id приложения.
    tokens: Mutex<BTreeMap<String, String>>,
    store_lock: Mutex<()>,
    /// Один проход по папке за раз (фон и «подхватить сейчас» не распаковывают пакет вдвоём).
    scan_lock: Mutex<()>,
}

pub fn folder(node: &Node) -> PathBuf {
    node.base_dir.join(FOLDER)
}
fn data_root(node: &Node) -> PathBuf {
    node.data_dir.join("apps")
}
pub fn data_dir(node: &Node, id: &str) -> PathBuf {
    node.data_dir.join("apps-data").join(id)
}
fn consent_file(node: &Node) -> PathBuf {
    data_root(node).join("consent.json")
}
fn read_json(p: &Path) -> Value {
    std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}
fn write_json(p: &Path, v: &Value) -> Result<(), String> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    bridge_core::write_atomic(p, &v.to_string()).map_err(|e| e.to_string())
}

/// Адрес моста для рамок приложений и служб: только 127.0.0.1 (сюда не ходит ничего, кроме этого ПК).
pub fn base_url(node: &Node) -> String {
    for u in node.urls.read().iter() {
        if let Some((ip, port)) = bridge_core::config::parse_listen(u) {
            if ip.is_loopback() || ip.is_unspecified() {
                return format!("http://127.0.0.1:{}", port);
            }
        }
    }
    node.urls.read().first().cloned().unwrap_or_default()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') && !id.starts_with('-')
}

fn platform() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// Относительный путь внутри приложения: без «..», без абсолютных.
fn rel_ok(p: &str) -> Option<String> {
    let p = p.trim().replace('\\', "/");
    bridge_core::path::norm(p.trim_start_matches("./"))
}

/// Прочитать и проверить app.json. root — папка приложения.
pub fn parse_manifest(root: &Path, text: &str) -> Result<App, String> {
    let m: Value = serde_json::from_str(text).map_err(|e| format!("app.json: {}", e))?;
    let s = |k: &str| m[k].as_str().unwrap_or("").trim().to_string();
    let id = s("id");
    if !valid_id(&id) {
        return Err(format!("app.json: id «{}» — латиница в нижнем регистре, цифры, - и _ (до 40)", id));
    }
    let name = s("name");
    if name.is_empty() || name.chars().count() > 60 {
        return Err("app.json: name — название до 60 знаков".into());
    }
    let version = if s("version").is_empty() { "0.0.0".into() } else { s("version") };
    if version.len() > 20 {
        return Err("app.json: version — до 20 знаков".into());
    }
    let mut perms = Vec::new();
    for p in m["permissions"].as_array().cloned().unwrap_or_default() {
        let p = p.as_str().unwrap_or("").to_string();
        if !PERMS.contains(&p.as_str()) {
            return Err(format!("app.json: неизвестное право «{}» (есть: {})", p, PERMS.join(", ")));
        }
        if !perms.contains(&p) {
            perms.push(p);
        }
    }
    let (mut ui_dir, mut ui_entry) = (String::new(), String::new());
    if let Some(u) = m["ui"].as_str() {
        let u = rel_ok(u).ok_or("app.json: ui — путь внутри приложения")?;
        if !root.join(&u).is_file() {
            return Err(format!("app.json: нет файла интерфейса «{}»", u));
        }
        match u.rsplit_once('/') {
            Some((d, f)) => {
                ui_dir = d.to_string();
                ui_entry = f.to_string();
            }
            None => ui_entry = u,
        }
    }
    for k in ["service", "native"] {
        if m[k].is_object() {
            let cmd = m[k]["cmd"].as_str().and_then(rel_ok).ok_or(format!("app.json: {}.cmd — программа внутри приложения", k))?;
            if !root.join(&cmd).is_file() {
                return Err(format!("app.json: нет программы «{}» ({})", cmd, k));
            }
        }
    }
    if let Some(a) = m["actions"].as_array() {
        for x in a {
            if x["id"].as_str().map(valid_id) != Some(true) || x["title"].as_str().unwrap_or("").is_empty() || x["run"].as_str().unwrap_or("").trim().is_empty() {
                return Err("app.json: actions — [{id, title, run}]".into());
            }
            let first = split_args(x["run"].as_str().unwrap_or("")).into_iter().next().unwrap_or_default();
            let first = rel_ok(&first).ok_or("app.json: actions.run — программа внутри приложения")?;
            if !root.join(&first).is_file() {
                return Err(format!("app.json: нет программы «{}» (кнопка «{}»)", first, x["title"].as_str().unwrap_or("")));
            }
        }
    }
    if m["ui"].is_null() && !m["native"].is_object() && !m["actions"].is_array() && !m["net"].is_object() {
        return Err("app.json: нужен ui, native, actions или net".into());
    }
    let icon = m["icon"].as_str().and_then(rel_ok).filter(|p| root.join(p).is_file()).unwrap_or_default();
    let unsupported = match m["platforms"].as_array() {
        Some(a) if !a.iter().any(|x| x.as_str() == Some(platform())) => format!("только для: {}", a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")),
        _ => String::new(),
    };
    Ok(App {
        id,
        name,
        version,
        description: s("description").chars().take(500).collect(),
        author: s("author").chars().take(80).collect(),
        root: root.to_path_buf(),
        manifest: m,
        ui_dir,
        ui_entry,
        icon,
        perms,
        unsupported,
        ..Default::default()
    })
}

/// Командная строка → аргументы (кавычки "…" держат пробелы).
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut q, mut any) = (false, false);
    for c in s.chars() {
        match c {
            '"' => {
                q = !q;
                any = true;
            }
            c if c.is_whitespace() && !q => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Отпечаток набора файлов без чтения содержимого: число, размер, время.
fn fingerprint(root: &Path, filter: impl Fn(&str) -> bool) -> String {
    let mut h = Sha256::new();
    let mut files = bridge_core::scanner::walk(root, &[]);
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    for w in files.iter().filter(|w| filter(&w.rel)) {
        h.update(w.rel.as_bytes());
        h.update(w.size.to_le_bytes());
        let mt = std::fs::metadata(&w.full).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis()).unwrap_or(0);
        h.update(mt.to_le_bytes());
    }
    hex::encode(&h.finalize()[..12])
}

fn in_ui(app: &App, rel: &str) -> bool {
    !app.ui_dir.is_empty() && (rel == app.ui_dir || rel.starts_with(&format!("{}/", app.ui_dir)))
}

/// Отпечаток для согласия: содержимое всех файлов вне интерфейса (кроме значка) + права и команды.
fn code_hash(app: &App) -> String {
    let mut h = Sha256::new();
    let m = &app.manifest;
    h.update(json!({ "p": app.perms, "s": m["service"], "n": m["native"], "a": m["actions"], "l": m["launch"], "net": m["net"] }).to_string().as_bytes());
    let mut files = bridge_core::scanner::walk(&app.root, &[]);
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    let mut buf = vec![0u8; 1 << 20];
    for w in files {
        if in_ui(app, &w.rel) || w.rel == app.icon || w.rel == MANIFEST {
            continue;
        }
        h.update(w.rel.as_bytes());
        if let Ok(mut f) = std::fs::File::open(&w.full) {
            while let Ok(n) = f.read(&mut buf) {
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
        }
    }
    hex::encode(&h.finalize()[..16])
}

/// Распаковать пакет в bridge-data/apps/pkg/<имя>-<отпечаток>/ (безопасно: только внутрь).
fn unpack(node: &Node, pkg: &Path, fp: &str) -> Result<PathBuf, String> {
    let stem = crate::mobile::folder_name(&pkg.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    let dest = data_root(node).join("pkg").join(format!("{}-{}", stem, &fp[..8.min(fp.len())]));
    if dest.join(MANIFEST).is_file() {
        return Ok(dest);
    }
    let f = std::fs::File::open(pkg).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f)).map_err(|e| format!("не zip: {}", e))?;
    let tmp = dest.with_extension("part");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let mut total: u64 = 0;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| e.to_string())?;
        if e.is_dir() {
            continue;
        }
        let Some(rel) = e.enclosed_name() else { continue };
        let Some(norm) = bridge_core::path::norm(&rel.to_string_lossy().replace('\\', "/")) else { continue };
        let Ok(out) = bridge_core::path::to_local(&tmp, &norm) else { continue };
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        total += std::io::copy(&mut e, &mut w).map_err(|e| e.to_string())?;
        if total > 4 << 30 {
            return Err("пакет больше 4 ГБ".into());
        }
    }
    // пакет мог быть упакован с папкой сверху: app.json — в единственной вложенной папке
    let mut root = tmp.clone();
    if !root.join(MANIFEST).is_file() {
        let subs: Vec<PathBuf> = std::fs::read_dir(&tmp).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
        if subs.len() == 1 && subs[0].join(MANIFEST).is_file() {
            root = subs[0].clone();
        } else {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err("в пакете нет app.json".into());
        }
    }
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&root, &dest).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(dest)
}

/// Один проход по папке «приложения». Возвращает true, если что-то поменялось (панели — «перечитай»).
pub fn scan(node: &Arc<Node>) -> bool {
    let _g = node.apps.scan_lock.lock();
    let dir = folder(node);
    let _ = std::fs::create_dir_all(&dir);
    let mut found: Vec<(PathBuf, bool)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.starts_with('_') {
                continue;
            }
            if p.is_dir() && p.join(MANIFEST).is_file() {
                found.push((p, false));
            } else if p.is_file() && name.to_lowercase().ends_with(&format!(".{}", EXT)) {
                found.push((p, true));
            }
        }
    }
    found.sort();
    let mut next: BTreeMap<String, App> = BTreeMap::new();
    let mut errors: BTreeMap<String, String> = BTreeMap::new();
    let mut reload: Vec<String> = Vec::new();
    let mut seen_next: BTreeMap<PathBuf, (String, Option<App>, String)> = BTreeMap::new();
    for (src, packaged) in found {
        // отпечаток источника: пакет — размер и время; папка — app.json + все файлы
        let fp = if packaged {
            let md = std::fs::metadata(&src).ok();
            format!("{}-{}", md.as_ref().map(|m| m.len()).unwrap_or(0), md.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis()).unwrap_or(0))
        } else {
            fingerprint(&src, |_| true)
        };
        let prev = node.apps.seen.lock().get(&src).cloned();
        let (app, err) = match prev {
            Some((pfp, app, err)) if pfp == fp => (app, err),
            _ => {
                let r = (|| -> Result<App, String> {
                    let root = if packaged { unpack(node, &src, &fp)? } else { src.clone() };
                    let text = std::fs::read_to_string(root.join(MANIFEST)).map_err(|e| format!("app.json: {}", e))?;
                    let mut app = parse_manifest(&root, &text)?;
                    app.source = src.clone();
                    app.packaged = packaged;
                    app.ui_stamp = fingerprint(&root, |rel| in_ui(&app, rel) || rel == app.icon);
                    app.hash = code_hash(&app);
                    Ok(app)
                })();
                match r {
                    Ok(a) => (Some(a), String::new()),
                    Err(e) => (None, e),
                }
            }
        };
        seen_next.insert(src.clone(), (fp, app.clone(), err.clone()));
        let label = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match app {
            Some(a) => {
                if let Some(other) = next.get(&a.id) {
                    errors.insert(label, format!("id «{}» уже занят: {}", a.id, other.source.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()));
                    continue;
                }
                next.insert(a.id.clone(), a);
            }
            None => {
                errors.insert(label, err);
            }
        }
    }
    let changed;
    {
        let mut cur = node.apps.list.lock();
        let mut stop: Vec<String> = Vec::new();
        for (id, old) in cur.iter() {
            match next.get(id) {
                None => stop.push(id.clone()),
                Some(n) => {
                    if n.hash != old.hash {
                        stop.push(id.clone());
                    }
                    if n.ui_stamp != old.ui_stamp {
                        reload.push(id.clone());
                    }
                }
            }
        }
        let sig = |m: &BTreeMap<String, App>| m.values().map(|a| format!("{}|{}|{}|{}|{}", a.id, a.version, a.hash, a.ui_stamp, a.root.display())).collect::<Vec<_>>().join(";");
        changed = sig(&cur) != sig(&next) || *node.apps.errors.lock() != errors;
        *cur = next;
        drop(cur);
        for id in stop {
            stop_all(node, &id, "приложение изменилось или убрано");
        }
    }
    *node.apps.errors.lock() = errors;
    *node.apps.seen.lock() = seen_next;
    if changed {
        node.media.deliver(json!({ "media": "apps", "type": "changed" }));
    }
    for id in reload {
        node.media.deliver(json!({ "media": "apps", "type": "reload", "id": id }));
    }
    // распакованные пакеты, которые больше не нужны
    if changed {
        let keep: Vec<PathBuf> = node.apps.list.lock().values().filter(|a| a.packaged).map(|a| a.root.clone()).collect();
        if let Ok(rd) = std::fs::read_dir(data_root(node).join("pkg")) {
            for e in rd.flatten() {
                if !keep.contains(&e.path()) {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
    }
    changed
}

/// Фон: следить за папкой (раз в 2 с) и гасить службы `run: "open"`, которые никто не держит.
pub fn schedule(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        loop {
            let n2 = node.clone();
            let _ = tokio::task::spawn_blocking(move || scan(&n2)).await;
            idle_stop(&node);
            start_always(&node);
            tokio::select! {
                _ = node.shutdown.cancelled() => {
                    let ids: Vec<String> = node.apps.runs.lock().keys().cloned().collect();
                    for id in ids { stop_all(&node, &id, "мост завершает работу"); }
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    });
}

fn idle_stop(node: &Arc<Node>) {
    let list = node.apps.list.lock().clone();
    let mut to_stop = Vec::new();
    for (id, r) in node.apps.runs.lock().iter() {
        let open_mode = list.get(id).map(|a| a.manifest["service"]["run"].as_str().unwrap_or("open") != "always").unwrap_or(true);
        if r.service.is_some() && open_mode && r.touched.map(|t| t.elapsed() > IDLE_STOP).unwrap_or(true) {
            to_stop.push(id.clone());
        }
    }
    for id in to_stop {
        stop(node, &id, "service", "приложение не открыто");
    }
}

fn start_always(node: &Arc<Node>) {
    if node.remote.active().is_some() {
        return;
    }
    let list = node.apps.list.lock().clone();
    for a in list.values() {
        if a.manifest["service"]["run"].as_str() == Some("always") && consent_ok(node, a) && a.unsupported.is_empty() {
            let running = node.apps.runs.lock().get(&a.id).map(|r| r.service.is_some()).unwrap_or(false);
            let backoff = node.apps.runs.lock().get(&a.id).map(|r| r.restarts > 20).unwrap_or(false);
            if !running && !backoff {
                let _ = start(node, &a.id, "service");
            }
        }
    }
}

// ───────────── согласие ─────────────

fn consent(node: &Node) -> Value {
    let v = read_json(&consent_file(node));
    if v.is_object() {
        v
    } else {
        json!({})
    }
}

pub fn consent_ok(node: &Node, a: &App) -> bool {
    consent(node)[&a.id]["hash"].as_str() == Some(a.hash.as_str())
}

/// Права, которые хозяин дал приложению (подмножество запрошенных).
pub fn granted(node: &Node, a: &App) -> Vec<String> {
    let c = consent(node);
    if c[&a.id]["hash"].as_str() != Some(a.hash.as_str()) {
        return vec![];
    }
    c[&a.id]["perms"].as_array().map(|x| x.iter().filter_map(|p| p.as_str().map(String::from)).filter(|p| a.perms.contains(p)).collect()).unwrap_or_default()
}

pub fn allow(node: &Node, id: &str, perms: Option<Vec<String>>) -> Result<(), String> {
    let a = get(node, id).ok_or("нет такого приложения")?;
    let perms: Vec<String> = perms.unwrap_or_else(|| a.perms.clone()).into_iter().filter(|p| a.perms.contains(p)).collect();
    let mut c = consent(node);
    c[id] = json!({ "hash": a.hash, "perms": perms, "at": time::format(&time::now()), "version": a.version });
    write_json(&consent_file(node), &c)?;
    node.info(&format!("[приложения] «{}» {}: разрешено (права: {})", a.name, a.version, if perms.is_empty() { "—".into() } else { perms.join(", ") }));
    node.media.deliver(json!({ "media": "apps", "type": "changed" }));
    Ok(())
}

pub fn revoke(node: &Arc<Node>, id: &str) -> Result<(), String> {
    let mut c = consent(node);
    if let Some(o) = c.as_object_mut() {
        o.remove(id);
    }
    write_json(&consent_file(node), &c)?;
    stop_all(node, id, "разрешение отозвано");
    node.media.deliver(json!({ "media": "apps", "type": "changed" }));
    Ok(())
}

pub fn get(node: &Node, id: &str) -> Option<App> {
    node.apps.list.lock().get(id).cloned()
}

// ───────────── для панели ─────────────

fn state_of(node: &Node, a: &App) -> &'static str {
    if !a.unsupported.is_empty() {
        return "unsupported";
    }
    let c = consent(node);
    match c[&a.id]["hash"].as_str() {
        None => "new",
        Some(h) if h != a.hash => "changed",
        _ => "ok",
    }
}

pub fn status(node: &Node) -> Value {
    let list = node.apps.list.lock().clone();
    let runs = node.apps.runs.lock();
    let apps: Vec<Value> = list
        .values()
        .map(|a| {
            let r = runs.get(&a.id);
            let rs = |s: &Option<RunState>| s.as_ref().map(|x| json!({ "pid": x.pid, "port": x.port, "since": time::format(&x.since) }));
            let m = &a.manifest;
            json!({
                "id": a.id, "name": a.name, "version": a.version, "description": a.description, "author": a.author,
                "packaged": a.packaged, "source": a.source.display().to_string(),
                "ui": !a.ui_entry.is_empty(), "entry": a.ui_entry, "native": m["native"].is_object(), "service": m["service"].is_object(),
                "actions": m["actions"].as_array().cloned().unwrap_or_default().iter().map(|x| json!({ "id": x["id"], "title": x["title"], "params": x["params"], "confirm": x["confirm"] })).collect::<Vec<_>>(),
                "icon": !a.icon.is_empty(), "perms": a.perms, "granted": granted(node, a), "state": state_of(node, a), "unsupported": a.unsupported,
                "window": m["window"], "net": m["net"], "opens": m["opens"], "launch": m["launch"],
                "dev": if a.packaged { Value::Null } else { m["dev"].clone() },
                "hash": a.hash, "uiStamp": a.ui_stamp,
                "runningService": r.and_then(|r| rs(&r.service)), "runningNative": r.and_then(|r| rs(&r.native)),
                "lastExit": r.map(|r| r.last_exit.clone()).unwrap_or_default(),
            })
        })
        .collect();
    let errors: Vec<Value> = node.apps.errors.lock().iter().map(|(k, v)| json!({ "source": k, "error": v })).collect();
    json!({ "list": apps, "errors": errors, "folder": folder(node).display().to_string(), "base": base_url(node), "platform": platform() })
}

/// Файл интерфейса: только из папки ui (или значок).
pub fn ui_file(node: &Node, id: &str, rel: &str) -> Option<PathBuf> {
    let a = get(node, id)?;
    if a.ui_entry.is_empty() {
        return None;
    }
    let rel = bridge_core::path::norm(rel.trim_start_matches('/'))?;
    let base = if a.ui_dir.is_empty() { a.root.clone() } else { a.root.join(&a.ui_dir) };
    // интерфейс без своей папки — отдаём только его файл, не всё приложение
    if a.ui_dir.is_empty() && rel != a.ui_entry {
        return None;
    }
    let p = bridge_core::path::to_local(&base, &rel).ok()?;
    p.is_file().then_some(p)
}

pub fn icon_file(node: &Node, id: &str) -> Option<PathBuf> {
    let a = get(node, id)?;
    (!a.icon.is_empty()).then(|| a.root.join(&a.icon)).filter(|p| p.is_file())
}

/// Убрать приложение: папку или пакет — в корзину моста (вернуть можно оттуда).
pub fn remove(node: &Arc<Node>, id: &str) -> Result<String, String> {
    let a = get(node, id).ok_or("нет такого приложения")?;
    stop_all(node, id, "приложение убрано");
    let name = a.source.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let to = node.folders.trash_dir().join("приложения").join(format!("{}--{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), name));
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&a.source, &to).map_err(|e| format!("не удалось убрать «{}»: {}", name, e))?;
    let mut c = consent(node);
    if let Some(o) = c.as_object_mut() {
        o.remove(id);
    }
    let _ = write_json(&consent_file(node), &c);
    node.info(&format!("[приложения] «{}» убрано в корзину моста: {}", a.name, to.display()));
    let n2 = node.clone();
    tokio::task::spawn_blocking(move || scan(&n2));
    Ok(to.display().to_string())
}

/// Положить пакет в папку приложений (перетащили .most-app в окно).
pub fn import(node: &Arc<Node>, name: &str, tmp: &Path) -> Result<String, String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if !base.to_lowercase().ends_with(&format!(".{}", EXT)) || base.len() > 120 || base.starts_with('.') {
        return Err(format!("нужен файл .{}", EXT));
    }
    let base = crate::mobile::folder_name(base);
    let dest = folder(node).join(&base);
    let _ = std::fs::create_dir_all(folder(node));
    bridge_core::files::move_replace(tmp, &dest).map_err(|e| e.to_string())?;
    let n2 = node.clone();
    tokio::task::spawn_blocking(move || scan(&n2));
    Ok(dest.display().to_string())
}

// ───────────── хранилище приложения ─────────────

pub fn store_get(node: &Node, id: &str, key: &str) -> Value {
    let v = read_json(&data_dir(node, id).join("store.json"));
    if key.is_empty() {
        if v.is_object() {
            v
        } else {
            json!({})
        }
    } else {
        v[key].clone()
    }
}

pub fn store_set(node: &Node, id: &str, key: &str, val: Value) -> Result<(), String> {
    if key.is_empty() || key.len() > 200 {
        return Err("ключ — 1–200 знаков".into());
    }
    let _g = node.apps.store_lock.lock();
    let f = data_dir(node, id).join("store.json");
    let mut v = read_json(&f);
    if !v.is_object() {
        v = json!({});
    }
    if val.is_null() {
        v.as_object_mut().unwrap().remove(key);
    } else {
        v[key] = val;
    }
    if v.to_string().len() > STORE_MAX {
        return Err("хранилище приложения больше 1 МБ".into());
    }
    write_json(&f, &v)
}

// ───────────── запуск служб и окон ─────────────

/// Токен приложения для подключённого редактора (живёт до перезапуска моста).
pub fn issue_token(node: &Node, id: &str) -> String {
    let t = hex::encode(rand::random::<[u8; 20]>());
    node.apps.tokens.lock().insert(t.clone(), id.to_string());
    t
}

/// Приложение по токену службы или окна.
pub fn by_token(node: &Node, token: &str) -> Option<App> {
    if token.is_empty() {
        return None;
    }
    let id = node.apps.tokens.lock().get(token).cloned()?;
    get(node, &id)
}

pub fn touch(node: &Arc<Node>, id: &str) {
    node.apps.runs.lock().entry(id.to_string()).or_default().touched = Some(Instant::now());
}

fn log_line(node: &Node, id: &str, line: String) {
    let mut runs = node.apps.runs.lock();
    let r = runs.entry(id.to_string()).or_default();
    r.log.push_back(format!("{} {}", chrono::Local::now().format("%H:%M:%S"), line));
    while r.log.len() > LOG_LINES {
        r.log.pop_front();
    }
}

pub fn log(node: &Node, id: &str) -> Vec<String> {
    node.apps.runs.lock().get(id).map(|r| r.log.iter().cloned().collect()).unwrap_or_default()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(0)
}

/// Запустить службу (`service`) или своё окно (`native`).
pub fn start(node: &Arc<Node>, id: &str, kind: &str) -> Result<Value, String> {
    let a = get(node, id).ok_or("нет такого приложения")?;
    if !a.unsupported.is_empty() {
        return Err(format!("«{}» не для этой системы ({})", a.name, a.unsupported));
    }
    if !consent_ok(node, &a) {
        return Err(format!("«{}» ещё не разрешено на этом ПК", a.name));
    }
    if let Some(g) = node.remote.active() {
        return Err(format!("пока этим ПК управляет {}, приложения не запускаются", g.peer));
    }
    if kind != "service" && kind != "native" {
        return Err("kind: service или native".into());
    }
    let spec = a.manifest[kind].clone();
    if !spec.is_object() {
        return Err(format!("у «{}» нет {}", a.name, if kind == "service" { "службы" } else { "своего окна" }));
    }
    {
        let runs = node.apps.runs.lock();
        if let Some(r) = runs.get(id) {
            let cur = if kind == "service" { &r.service } else { &r.native };
            if let Some(s) = cur {
                return Ok(json!({ "ok": true, "already": true, "pid": s.pid, "port": s.port }));
            }
        }
    }
    let cmd_rel = spec["cmd"].as_str().and_then(rel_ok).ok_or("cmd")?;
    let exe = a.root.join(&cmd_rel);
    let args: Vec<String> = spec["args"].as_array().map(|x| x.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let token = hex::encode(rand::random::<[u8; 20]>());
    let port = if kind == "service" { free_port() } else { 0 };
    let data = data_dir(node, id);
    let _ = std::fs::create_dir_all(&data);
    let mut c = tokio::process::Command::new(&exe);
    c.args(&args)
        .current_dir(exe.parent().unwrap_or(&a.root))
        .env("MOST_API", format!("{}{}/app", base_url(node), bridge_core::API_PREFIX))
        .env("MOST_TOKEN", &token)
        .env("MOST_APP", id)
        .env("MOST_DATA", &data)
        .env("MOST_ROOT", &a.root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if port > 0 {
        c.env("MOST_PORT", port.to_string());
    }
    #[cfg(windows)]
    if kind == "service" {
        c.creation_flags(0x0800_0000); // без окна консоли
    }
    let mut child = c.spawn().map_err(|e| format!("не запустилось «{}»: {}", cmd_rel, e))?;
    let pid = child.id().unwrap_or(0);
    let stop_tok = CancellationToken::new();
    node.apps.tokens.lock().insert(token.clone(), id.to_string());
    {
        let mut runs = node.apps.runs.lock();
        let r = runs.entry(id.to_string()).or_default();
        let st = RunState { pid, port, since: time::now(), stop: stop_tok.clone() };
        if kind == "service" {
            r.service = Some(st);
            r.touched = Some(Instant::now());
        } else {
            r.native = Some(st);
        }
    }
    let label = if kind == "service" { "служба" } else { "окно" };
    log_line(node, id, format!("[мост] {} запущено: {} (pid {}{})", label, cmd_rel, pid, if port > 0 { format!(", порт {}", port) } else { String::new() }));
    node.info(&format!("[приложения] «{}»: {} запущено (pid {})", a.name, label, pid));
    for (pipe, tag) in [(child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), ""), (child.stderr.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), "! ")] {
        if let Some(p) = pipe {
            let (n2, id2) = (node.clone(), id.to_string());
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(p).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    log_line(&n2, &id2, format!("{}{}", tag, l.chars().take(1000).collect::<String>()));
                }
            });
        }
    }
    let (n2, id2, kind2, name) = (node.clone(), id.to_string(), kind.to_string(), a.name.clone());
    tokio::spawn(async move {
        let stopped;
        let code = tokio::select! {
            r = child.wait() => { stopped = false; r.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string()) }
            _ = stop_tok.cancelled() => { stopped = true; let _ = child.kill().await; "остановлено".to_string() }
        };
        n2.apps.tokens.lock().remove(&token);
        {
            let mut runs = n2.apps.runs.lock();
            let r = runs.entry(id2.clone()).or_default();
            if kind2 == "service" {
                r.service = None;
                if !stopped {
                    r.restarts += 1;
                }
            } else {
                r.native = None;
            }
            r.last_exit = format!("{} {}", if kind2 == "service" { "служба" } else { "окно" }, code);
        }
        log_line(&n2, &id2, format!("[мост] {} завершено: {}", if kind2 == "service" { "служба" } else { "окно" }, code));
        if !stopped && kind2 == "service" {
            n2.info(&format!("[приложения] «{}»: служба завершилась ({})", name, code));
            // упала — `always` поднимется сама через 10 с (start_always), `open` — при следующем обращении
            tokio::time::sleep(Duration::from_secs(8)).await;
        }
        n2.media.deliver(json!({ "media": "apps", "type": "changed" }));
    });
    node.media.deliver(json!({ "media": "apps", "type": "changed" }));
    Ok(json!({ "ok": true, "pid": pid, "port": port }))
}

pub fn stop(node: &Arc<Node>, id: &str, kind: &str, why: &str) {
    let st = {
        let mut runs = node.apps.runs.lock();
        runs.get_mut(id).and_then(|r| if kind == "service" { r.service.take() } else { r.native.take() })
    };
    if let Some(s) = st {
        log_line(node, id, format!("[мост] остановка: {}", why));
        s.stop.cancel();
    }
}

pub fn stop_all(node: &Arc<Node>, id: &str, why: &str) {
    stop(node, id, "service", why);
    stop(node, id, "native", why);
}

/// Порт службы (запустить, если не запущена).
pub async fn service_port(node: &Arc<Node>, id: &str) -> Result<u16, String> {
    let running = node.apps.runs.lock().get(id).and_then(|r| r.service.as_ref().map(|s| s.port));
    touch(node, id);
    if let Some(p) = running {
        return Ok(p);
    }
    let r = start(node, id, "service")?;
    let port = r["port"].as_u64().unwrap_or(0) as u16;
    // дать службе подняться: ждём, пока порт начнёт отвечать (до 10 с)
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(port)
}

// ───────────── кнопки ─────────────

/// Выполнить кнопку приложения: программа из пакета с подстановкой параметров {имя} (каждый — отдельным аргументом,
/// без оболочки). До 2 минут; вывод — до 64 КБ.
pub async fn action(node: &Arc<Node>, id: &str, action: &str, params: &Value) -> Result<Value, String> {
    let a = get(node, id).ok_or("нет такого приложения")?;
    if !consent_ok(node, &a) {
        return Err(format!("«{}» ещё не разрешено на этом ПК", a.name));
    }
    if let Some(g) = node.remote.active() {
        return Err(format!("пока этим ПК управляет {}, приложения не запускаются", g.peer));
    }
    let spec = a.manifest["actions"].as_array().and_then(|x| x.iter().find(|x| x["id"] == action).cloned()).ok_or("нет такой кнопки")?;
    let mut argv: Vec<String> = Vec::new();
    for part in split_args(spec["run"].as_str().unwrap_or("")) {
        let mut s = part.clone();
        if let Some(o) = params.as_object() {
            for (k, v) in o {
                let val = match v {
                    Value::String(t) => t.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                s = s.replace(&format!("{{{}}}", k), &val);
            }
        }
        argv.push(s);
    }
    let exe_rel = rel_ok(&argv[0]).ok_or("программа")?;
    let exe = a.root.join(&exe_rel);
    let data = data_dir(node, id);
    let _ = std::fs::create_dir_all(&data);
    let mut c = tokio::process::Command::new(&exe);
    c.args(&argv[1..]).current_dir(&a.root).env("MOST_APP", id).env("MOST_DATA", &data).stdin(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000);
    log_line(node, id, format!("[мост] кнопка «{}»", spec["title"].as_str().unwrap_or(action)));
    let out = tokio::time::timeout(Duration::from_secs(120), c.output()).await.map_err(|_| "кнопка выполнялась дольше 2 минут — остановлена".to_string())?.map_err(|e| format!("не запустилось «{}»: {}", exe_rel, e))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        text.push_str(&err);
    }
    if text.len() > 64 * 1024 {
        let mut cut = 64 * 1024;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n…");
    }
    log_line(node, id, format!("[мост] кнопка «{}» — {}", spec["title"].as_str().unwrap_or(action), out.status));
    Ok(json!({ "ok": out.status.success(), "code": out.status.code(), "out": text }))
}

/// Упаковать папку приложения в .most-app (zip; app.json проверяется). Возвращает путь и число файлов.
pub fn pack(dir: &Path, out: Option<PathBuf>) -> Result<(PathBuf, usize), String> {
    use std::io::Write;
    let text = std::fs::read_to_string(dir.join(MANIFEST)).map_err(|e| format!("app.json: {}", e))?;
    let a = parse_manifest(dir, &text)?;
    let out = out.unwrap_or_else(|| dir.parent().unwrap_or(Path::new(".")).join(format!("{}-{}.{}", a.id, a.version, EXT)));
    let f = std::fs::File::create(&out).map_err(|e| format!("{}: {}", out.display(), e))?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(f));
    let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(6)).large_file(true);
    let mut n = 0;
    let mut files = bridge_core::scanner::walk(dir, &[]);
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    let mut buf = vec![0u8; 1 << 20];
    for w in files {
        if w.full == out || w.rel.split('/').any(|s| s == ".git" || s == "node_modules") {
            continue;
        }
        z.start_file(w.rel.as_str(), opt).map_err(|e| e.to_string())?;
        let mut src = std::fs::File::open(&w.full).map_err(|e| format!("{}: {}", w.rel, e))?;
        loop {
            let k = src.read(&mut buf).map_err(|e| e.to_string())?;
            if k == 0 {
                break;
            }
            z.write_all(&buf[..k]).map_err(|e| e.to_string())?;
        }
        n += 1;
    }
    z.finish().map_err(|e| e.to_string())?;
    Ok((out, n))
}

/// Заготовка приложения: app.json и веб-интерфейс с SDK.
pub fn template(dir: &Path, id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err("id — латиница в нижнем регистре, цифры, - и _ (до 40)".into());
    }
    if dir.join(MANIFEST).exists() {
        return Err(format!("в {} уже есть app.json", dir.display()));
    }
    std::fs::create_dir_all(dir.join("ui")).map_err(|e| e.to_string())?;
    let m = json!({ "id": id, "name": id, "version": "0.1.0", "description": "", "ui": "ui/index.html", "window": { "w": 480, "h": 640 }, "permissions": ["notify"], "bridge": "apps@1" });
    std::fs::write(dir.join(MANIFEST), serde_json::to_string_pretty(&m).unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("ui/index.html"), TEMPLATE_HTML).map_err(|e| e.to_string())?;
    Ok(())
}

const TEMPLATE_HTML: &str = r#"<!doctype html>
<html lang="ru">
<head><meta charset="utf-8"><title>Приложение</title>
<style>body{font:15px system-ui,sans-serif;margin:24px;background:#0B0C1A;color:#E8EAFB}button{font:inherit;padding:8px 14px;border-radius:10px;border:1px solid #38E1F0;background:#12132A;color:inherit;cursor:pointer}</style>
<script src="/most/sdk.js"></script>
</head>
<body>
<h1 id="t">…</h1>
<p>Нажато: <b id="n">0</b></p>
<button id="b">Нажать</button>
<script>
most.ready().then(async me => {
  document.getElementById('t').textContent = me.name + ' на ' + me.node;
  let n = (await most.store.get('count')) || 0;
  const show = () => document.getElementById('n').textContent = n;
  show();
  document.getElementById('b').onclick = async () => { n++; show(); await most.store.set('count', n); most.window.badge(n); };
});
</script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn args() {
        assert_eq!(split_args(r#"bin/tv.exe off "Living room" {x}"#), vec!["bin/tv.exe", "off", "Living room", "{x}"]);
        assert_eq!(split_args(r#"a "" b"#), vec!["a", "", "b"]);
    }
    #[test]
    fn manifest() {
        let d = std::env::temp_dir().join(format!("most-app-{}", rand::random::<u32>()));
        std::fs::create_dir_all(d.join("ui")).unwrap();
        std::fs::write(d.join("ui/index.html"), "<b>hi</b>").unwrap();
        let a = parse_manifest(&d, r#"{"id":"tv","name":"Пульт","ui":"ui/index.html","permissions":["notify"]}"#).unwrap();
        assert_eq!((a.ui_dir.as_str(), a.ui_entry.as_str()), ("ui", "index.html"));
        assert!(parse_manifest(&d, r#"{"id":"TV","name":"x","ui":"ui/index.html"}"#).is_err());
        assert!(parse_manifest(&d, r#"{"id":"tv","name":"x","ui":"../x.html"}"#).is_err());
        assert!(parse_manifest(&d, r#"{"id":"tv","name":"x","ui":"ui/index.html","permissions":["root"]}"#).is_err());
        assert!(parse_manifest(&d, r#"{"id":"tv","name":"x","native":{"cmd":"bin/no.exe"}}"#).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
