//! Общие папки. Хозяин — узел, у которого папка лежит: его копия главная. Соседи с правом записи добавляют, меняют и удаляют
//! файлы напрямую (их узел шлёт изменения хозяину), с правом чтения — получают копию. Удалённое и перезаписанное — в корзину.

use crate::client::{PeerClient, PeerError};
use crate::link;
use crate::mailbox::remove_empty_dirs;
use crate::node::Node;
use bridge_core::config::{cached_ignore, SharedFolderConfig, READ, WRITE};
use bridge_core::ignore::IgnoreRules;
use bridge_core::proto::{FileEntry, FolderManifest, SharedFolderInfo};
use bridge_core::scanner::{walk, TreeScanner};
use bridge_core::time::{self, Time};
use bridge_core::{eq_ic, path, to_json};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const MY_EDITS_DIR: &str = "_мои-правки";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Mount {
    pub peer: String,
    pub id: String,
    pub title: String,
    pub access: String,
    pub root: String,
    /// путь → sha, на которой мы и хозяин последний раз сошлись
    pub base: BTreeMap<String, String>,
    pub hold: bool,
    pub hold_reason: String,
    pub allow_deletes_once: bool,
    #[serde(with = "time::opt")]
    pub last_sync: Option<Time>,
    pub error: String,
    pub files: i64,
    pub bytes: i64,
    pub recent: Vec<String>,
    pub revoked: bool,
    pub owner_ignore: Vec<String>,
    pub key: String,
    /// Когда применялись удаления (unix, с): защита от массового удаления видит и удаление «по частям».
    pub recent_deletes: Vec<i64>,
    /// Отпечаток папки у хозяина при последнем обмене (не изменился и здесь ничего не трогали — обмен не нужен).
    pub owner_rev: String,
    /// Предел размера файла у хозяина, байт (0 — без предела или старая версия).
    pub owner_max: i64,
}

impl Default for Mount {
    fn default() -> Self {
        Mount {
            peer: String::new(),
            id: String::new(),
            title: String::new(),
            access: String::new(),
            root: String::new(),
            base: BTreeMap::new(),
            hold: false,
            hold_reason: String::new(),
            allow_deletes_once: false,
            last_sync: None,
            error: String::new(),
            files: 0,
            bytes: 0,
            recent: Vec::new(),
            revoked: false,
            owner_ignore: Vec::new(),
            key: String::new(),
            recent_deletes: Vec::new(),
            owner_rev: String::new(),
            owner_max: 0,
        }
    }
}

fn base_get<'a>(m: &'a BTreeMap<String, String>, k: &str) -> Option<&'a String> {
    m.get(k).or_else(|| m.iter().find(|(x, _)| eq_ic(x, k)).map(|(_, v)| v))
}

fn base_remove(m: &mut BTreeMap<String, String>, k: &str) {
    if m.remove(k).is_none() {
        if let Some(x) = m.keys().find(|x| eq_ic(x, k)).cloned() {
            m.remove(&x);
        }
    }
}

pub struct Folders {
    own_scanners: Mutex<HashMap<String, TreeScanner>>,
    /// Кэш файлов своих папок (сбрасывается при изменениях).
    own_cache: Mutex<HashMap<String, Vec<FileEntry>>>,
    own_gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    mounts: Mutex<BTreeMap<String, Mount>>,
    mount_scanners: Mutex<HashMap<String, TreeScanner>>,
    round_gate: tokio::sync::Mutex<()>,
    own_signature: Mutex<String>,
    mount_signature: Mutex<String>,
    last_sweep: Mutex<Option<std::time::Instant>>,
    state_file: PathBuf,
    trash_root: PathBuf,
    pub wake: AtomicBool,
    pub own_dirty: AtomicBool,
    pub mounts_dirty: AtomicBool,
    /// В подключённых папках что-то меняли с прошлого обмена (тогда обмен нужен, даже если у хозяина без изменений).
    pub mounts_touched: AtomicBool,
    /// Большие файлы идут в фоне и не держат остальные: что сейчас передаётся, что уже скачано и ждёт установки, что не прошло.
    transfers: Mutex<HashMap<String, Transfer>>,
    prefetched: Mutex<HashMap<String, (PathBuf, String)>>,
    failed: Mutex<HashMap<String, Failed>>,
    notes: Mutex<Vec<(String, String)>>,
}

/// Файлы больше этого — в фоне (маленькие при этом идут сразу).
pub const BIG_FILE: i64 = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct Transfer {
    pub path: String,
    pub up: bool,
    pub size: i64,
    pub started: std::time::Instant,
}

struct Failed {
    sha: String,
    reason: String,
    /// None — не повторять, пока файл не изменится (например, больше предела)
    until: Option<std::time::Instant>,
}

fn tkey(mount: &str, p: &str) -> String {
    format!("{}\n{}", mount.to_lowercase(), p.to_lowercase())
}

pub fn human(n: i64) -> String {
    let f = n as f64;
    if f >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} ГБ", f / 1024.0 / 1024.0 / 1024.0)
    } else if f >= 1024.0 * 1024.0 {
        format!("{:.1} МБ", f / 1024.0 / 1024.0)
    } else if f >= 1024.0 {
        format!("{:.0} КБ", f / 1024.0)
    } else {
        format!("{} Б", n)
    }
}

fn safe(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn mount_scope(m: &Mount) -> String {
    format!("общие_{}_{}", m.peer, m.id)
}

impl Folders {
    pub fn new(data_dir: &Path) -> Self {
        Folders {
            own_scanners: Mutex::new(HashMap::new()),
            own_cache: Mutex::new(HashMap::new()),
            own_gates: Mutex::new(HashMap::new()),
            mounts: Mutex::new(BTreeMap::new()),
            mount_scanners: Mutex::new(HashMap::new()),
            round_gate: tokio::sync::Mutex::new(()),
            own_signature: Mutex::new(String::new()),
            mount_signature: Mutex::new(String::new()),
            last_sweep: Mutex::new(None),
            state_file: data_dir.join("state").join("mounts.json"),
            trash_root: data_dir.join("trash"),
            wake: AtomicBool::new(false),
            own_dirty: AtomicBool::new(true),
            mounts_dirty: AtomicBool::new(true),
            mounts_touched: AtomicBool::new(true),
            transfers: Mutex::new(HashMap::new()),
            prefetched: Mutex::new(HashMap::new()),
            failed: Mutex::new(HashMap::new()),
            notes: Mutex::new(Vec::new()),
        }
    }

    /// Не повторять передачу, которая не прошла (этот же файл, ещё не истёк срок).
    fn blocked(&self, k: &str, sha: &str) -> bool {
        let mut g = self.failed.lock();
        match g.get(k) {
            Some(f) if f.sha.eq_ignore_ascii_case(sha) && f.until.map(|u| std::time::Instant::now() < u).unwrap_or(true) => true,
            Some(_) => {
                g.remove(k);
                false
            }
            None => false,
        }
    }

    fn fail(&self, k: &str, sha: &str, reason: &str, retry_secs: Option<u64>) {
        self.failed.lock().insert(
            k.to_string(),
            Failed { sha: sha.to_string(), reason: reason.to_string(), until: retry_secs.map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s)) },
        );
    }

    /// Для панели: что передаётся и что не прошло в этой папке.
    pub fn transfer_view(&self, mount_key: &str) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
        let pre = format!("{}\n", mount_key.to_lowercase());
        let t = self
            .transfers
            .lock()
            .iter()
            .filter(|(k, _)| k.starts_with(&pre))
            .map(|(_, t)| serde_json::json!({ "path": t.path, "up": t.up, "size": t.size, "secs": t.started.elapsed().as_secs() }))
            .collect();
        let f = self
            .failed
            .lock()
            .iter()
            .filter(|(k, _)| k.starts_with(&pre))
            .map(|(k, f)| serde_json::json!({ "path": k[pre.len()..].to_string(), "reason": f.reason }))
            .collect();
        (t, f)
    }

    /// Передать большой файл в фоне. up — отправить хозяину; иначе — скачать во временный файл (установит следующий обмен).
    #[allow(clippy::too_many_arguments)]
    fn spawn_transfer(&self, node: &Arc<Node>, client: &Arc<PeerClient>, m: &mut Mount, p: &str, full: &Path, sha: &str, base: &str, size: i64, up: bool) {
        let tk = tkey(&m.key, p);
        if self.transfers.lock().contains_key(&tk) {
            return;
        }
        self.transfers.lock().insert(tk.clone(), Transfer { path: p.to_string(), up, size, started: std::time::Instant::now() });
        Self::note(m, &format!("{} в фоне: {} ({})", if up { "отправляю" } else { "скачиваю" }, p, human(size)));
        let (node, client) = (node.clone(), client.clone());
        let (id, mkey, p, full, sha, base) = (m.id.clone(), m.key.clone(), p.to_string(), full.to_path_buf(), sha.to_string(), base.to_string());
        tokio::spawn(async move {
            let f = &node.folders;
            let res: Result<(), (bool, String)> = if up {
                match client.folder_upload(&id, &p, &full, &sha, &base).await {
                    Ok(c) if c == 200 || c == 409 => Ok(()),
                    Ok(403) => Err((false, "запись запрещена хозяином".into())),
                    Ok(c) => Err((false, format!("хозяин ответил кодом {}", c))),
                    Err(PeerError::Http(413, _)) => Err((true, format!("{} больше предела у хозяина (maxFileMb там)", human(size)))),
                    Err(e) => Err((false, e.to_string())),
                }
            } else {
                let tmp = node.tmp_file();
                match client.folder_download(&id, &p, &tmp, node.max_file_bytes()).await {
                    Ok(got) if got.eq_ignore_ascii_case(&sha) => {
                        f.prefetched.lock().insert(tk.clone(), (tmp, sha.clone()));
                        Ok(())
                    }
                    Ok(_) => {
                        let _ = std::fs::remove_file(&tmp);
                        Ok(()) // у хозяина файл успел измениться — следующий обмен возьмёт новый
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp);
                        Err((matches!(e, PeerError::Http(413, _)), e.to_string()))
                    }
                }
            };
            f.transfers.lock().remove(&tk);
            match res {
                Ok(()) => {
                    if up {
                        f.notes.lock().push((mkey.clone(), format!("отправлен хозяину: {}", p)));
                    }
                }
                Err((permanent, why)) => {
                    f.fail(&tk, &sha, &why, if permanent { None } else { Some(120) });
                    f.notes.lock().push((mkey.clone(), format!("не {}: {} — {}", if up { "отправлен" } else { "скачан" }, p, why)));
                    node.info(&format!("[общая {}] {} не {}: {}", mkey, p, if up { "отправлен" } else { "скачан" }, why));
                }
            }
            f.mounts_touched.store(true, Ordering::SeqCst);
            f.wake.store(true, Ordering::SeqCst);
            node.wake.notify_one();
        });
    }

    pub fn load(&self, node: &Node) {
        let Ok(text) = std::fs::read_to_string(&self.state_file) else { return };
        match serde_json::from_str::<Vec<Mount>>(text.trim_start_matches('\u{feff}')) {
            Ok(list) => {
                let mut g = self.mounts.lock();
                for mut m in list {
                    m.key = format!("{}/{}", m.peer, m.id);
                    g.insert(m.key.to_lowercase(), m);
                }
            }
            Err(e) => node.info(&format!("Общие папки: состояние повреждено, начну заново: {}", e)),
        }
    }

    pub fn save(&self, node: &Node) {
        let list: Vec<Mount> = self.mounts.lock().values().cloned().collect();
        if let Err(e) = bridge_core::write_atomic(&self.state_file, &to_json(&list)) {
            node.info(&format!("Общие папки: не удалось сохранить состояние: {}", e));
        }
    }

    pub fn mounts(&self) -> Vec<Mount> {
        self.mounts.lock().values().cloned().collect()
    }

    pub fn find_mount(&self, peer: &str, id: &str) -> Option<Mount> {
        self.mounts.lock().get(&format!("{}/{}", peer, id).to_lowercase()).cloned()
    }

    pub fn mount_root(&self, node: &Node) -> PathBuf {
        let c = node.cfg.read();
        node.resolve(&c.mounts.root)
    }

    pub fn trash_dir(&self) -> &Path {
        &self.trash_root
    }

    // ───────────────────────── Хозяин ─────────────────────────

    pub fn own_root(&self, node: &Node, f: &SharedFolderConfig) -> PathBuf {
        node.resolve(&f.path)
    }

    pub fn own_ignore(&self, node: &Node, f: &SharedFolderConfig) -> Arc<IgnoreRules> {
        let c = node.cfg.read();
        cached_ignore(c.ignore.iter().chain(f.ignore.iter()))
    }

    pub fn mount_ignore(&self, node: &Node, m: &Mount) -> Arc<IgnoreRules> {
        let c = node.cfg.read();
        let own: Vec<String> = c.mounts.ignore.iter().find(|(k, _)| eq_ic(k, &format!("{}/{}", m.peer, m.id))).map(|(_, v)| v.clone()).unwrap_or_default();
        cached_ignore(m.owner_ignore.iter().chain(c.ignore.iter()).chain(own.iter()))
    }

    /// Файлы своей папки без игнора (из кэша, пока папка не менялась).
    pub fn own_files(&self, node: &Node, f: &SharedFolderConfig) -> Vec<FileEntry> {
        let key = f.id.to_lowercase();
        if let Some(c) = self.own_cache.lock().get(&key) {
            return c.clone();
        }
        let root = self.own_root(node, f);
        let ig = self.own_ignore(node, f);
        let mut sc = self.own_scanners.lock();
        let s = sc.entry(key.clone()).or_insert_with(|| TreeScanner::new(root.clone()));
        if s.root() != root {
            *s = TreeScanner::new(root.clone());
        }
        let mut busy = 0;
        let files: Vec<FileEntry> = s.scan(&mut busy).into_iter().filter(|x| !ig.is_ignored(&x.path)).collect();
        if busy == 0 {
            self.own_cache.lock().insert(key, files.clone());
        }
        files
    }

    /// Сбросить кэш своих папок (что-то изменилось на диске).
    pub fn invalidate_own(&self) {
        self.own_cache.lock().clear();
    }

    fn gate_for(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.own_gates.lock().entry(id.to_lowercase()).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))).clone()
    }

    pub fn visible_to(&self, node: &Node, peer: &str) -> Vec<SharedFolderInfo> {
        let cfg = node.cfg.read().clone();
        cfg.folders
            .iter()
            .filter_map(|f| {
                let acc = cfg.access_of(f, peer)?;
                if !self.own_root(node, f).is_dir() {
                    return None;
                }
                let files = self.own_files(node, f);
                Some(SharedFolderInfo {
                    id: f.id.clone(),
                    title: if f.title.is_empty() { f.id.clone() } else { f.title.clone() },
                    access: acc.into(),
                    files: files.len() as i64,
                    bytes: files.iter().map(|x| x.size).sum(),
                    ignore: self.own_ignore(node, f).patterns(),
                    rev: bridge_core::proto::files_rev(&files),
                    max_bytes: if cfg.max_file_mb > 0 { cfg.max_file_mb * 1024 * 1024 } else { 0 },
                })
            })
            .collect()
    }

    /// Манифест для соседа. Ok(None) — нет доступа; Err — папки нет на диске (пустой манифест опасен: сосед всё удалит).
    pub fn manifest(&self, node: &Node, id: &str, peer: &str) -> Result<Option<FolderManifest>, String> {
        let cfg = node.cfg.read().clone();
        let Some(f) = cfg.folder(id) else { return Ok(None) };
        let Some(acc) = cfg.access_of(f, peer) else { return Ok(None) };
        if !self.own_root(node, f).is_dir() {
            return Err(format!("папка «{}» недоступна на диске хозяина", f.id));
        }
        Ok(Some(FolderManifest { folder: f.id.clone(), access: acc.into(), files: self.own_files(node, f) }))
    }

    pub fn own_file(&self, node: &Node, id: &str, peer: &str, p: &str) -> Option<(PathBuf, String)> {
        let cfg = node.cfg.read().clone();
        let f = cfg.folder(id)?;
        cfg.access_of(f, peer)?;
        let entry = self.own_files(node, f).into_iter().find(|x| eq_ic(&x.path, p))?;
        Some((path::to_local(&self.own_root(node, f), &entry.path).ok()?, entry.sha256))
    }

    /// Записать файл от соседа (право write) из временного файла `tmp` (sha уже проверена). (код, сообщение)
    pub async fn own_write(&self, node: &Arc<Node>, id: &str, peer: &str, p: &str, base: &str, sha: &str, tmp: &Path) -> (u16, String) {
        let cfg = node.cfg.read().clone();
        let Some(f) = cfg.folder(id).cloned() else { return (404, "нет такой папки".into()) };
        match cfg.access_of(&f, peer) {
            None => return (404, "нет такой папки".into()),
            Some(READ) => return (403, "только чтение".into()),
            _ => {}
        }
        if let Some(rule) = self.own_ignore(node, &f).which(p) {
            return (403, format!("в игноре («{}»)", rule));
        }
        let root = self.own_root(node, &f);
        if !root.is_dir() {
            return (409, "папка недоступна на диске хозяина".into());
        }
        let Ok(full) = path::to_local(&root, p) else { return (400, "путь".into()) };
        let gate = self.gate_for(&f.id);
        let _g = gate.lock().await;
        let current = if full.is_file() {
            match path::sha256_file_settled(&full) {
                Ok(s) => s,
                Err(_) => return (409, "файл занят у хозяина, повторите".into()),
            }
        } else {
            String::new()
        };
        if !current.eq_ignore_ascii_case(base) {
            return if current == sha { (200, "уже такой".into()) } else { (409, "файл изменился у хозяина".into()) };
        }
        if let Some(d) = full.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if full.is_file() {
            self.to_trash(node, &f.id, p, &full);
        }
        if let Err(e) = bridge_core::files::move_replace(tmp, &full) {
            return (500, e.to_string());
        }
        if let Some(s) = self.own_scanners.lock().get_mut(&f.id.to_lowercase()) {
            s.remember(p, &full, sha);
        }
        self.invalidate_own();
        drop(_g);
        node.info(&format!("[общая «{}»] {} записал {}", f.id, peer, p));
        node.emit("folder-write", serde_json::json!({ "folder": f.id, "peer": peer, "path": p }));
        self.notify_folder_changed(node, Some(peer));
        (200, "ок".into())
    }

    pub async fn own_delete(&self, node: &Arc<Node>, id: &str, peer: &str, p: &str, base: &str) -> (u16, String) {
        let cfg = node.cfg.read().clone();
        let Some(f) = cfg.folder(id).cloned() else { return (404, "нет такой папки".into()) };
        match cfg.access_of(&f, peer) {
            None => return (404, "нет такой папки".into()),
            Some(READ) => return (403, "только чтение".into()),
            _ => {}
        }
        if let Some(rule) = self.own_ignore(node, &f).which(p) {
            return (403, format!("в игноре («{}»)", rule));
        }
        let root = self.own_root(node, &f);
        let Ok(full) = path::to_local(&root, p) else { return (400, "путь".into()) };
        let gate = self.gate_for(&f.id);
        let _g = gate.lock().await;
        if !full.is_file() {
            return (200, "уже удалён".into());
        }
        let Ok(current) = path::sha256_file_settled(&full) else { return (409, "файл занят у хозяина".into()) };
        if !current.eq_ignore_ascii_case(base) {
            return (409, "файл изменился у хозяина".into());
        }
        self.to_trash(node, &f.id, p, &full);
        remove_empty_dirs(&root, full.parent().unwrap_or(&root));
        self.invalidate_own();
        drop(_g);
        node.info(&format!("[общая «{}»] {} удалил {}", f.id, peer, p));
        self.notify_folder_changed(node, Some(peer));
        (200, "ок".into())
    }

    /// Прежняя версия — в корзину узла (хранится mounts.trashMinutes).
    fn to_trash(&self, node: &Node, scope: &str, p: &str, full: &Path) {
        if node.cfg.read().mounts.trash_minutes <= 0 {
            let _ = std::fs::remove_file(full);
            return;
        }
        let batch = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f").to_string();
        let Ok(dest) = path::to_local(&self.trash_root.join(safe(scope)).join(batch), p) else { return };
        if let Some(d) = dest.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if bridge_core::files::move_replace(full, &dest).is_err() {
            let _ = std::fs::remove_file(full);
        }
    }

    fn notify_folder_changed(&self, node: &Arc<Node>, except: Option<&str>) {
        let cfg = node.cfg.read().clone();
        let names: Vec<String> = cfg
            .peers
            .iter()
            .filter(|p| p.enabled && except.map(|e| !eq_ic(e, &p.name)).unwrap_or(true))
            .filter(|p| cfg.folders.iter().any(|f| cfg.access_of(f, &p.name).is_some()))
            .map(|p| p.name.clone())
            .collect();
        node.notify_peers(names, "", "folders");
    }

    /// Изменились ли свои общие папки (тогда — оповестить соседей).
    pub fn check_own_changes(&self, node: &Arc<Node>) {
        let cfg = node.cfg.read().clone();
        if cfg.folders.is_empty() {
            return;
        }
        self.invalidate_own();
        let sig: String = cfg
            .folders
            .iter()
            .map(|f| {
                let files = self.own_files(node, f);
                let mut h: u64 = 0;
                for x in &files {
                    h ^= fxhash(&x.sha256) ^ fxhash(&x.path);
                }
                format!("{}:{}:{}:{}", f.id, self.own_ignore(node, f).patterns().join(","), files.len(), h)
            })
            .collect::<Vec<_>>()
            .join(";");
        let mut g = self.own_signature.lock();
        if *g == sig {
            return;
        }
        let first = g.is_empty();
        *g = sig;
        drop(g);
        if !first {
            self.notify_folder_changed(node, None);
        }
    }

    /// Поменялось ли что-то в подключённых папках у нас.
    pub fn local_mounts_changed(&self) -> bool {
        let mounts = self.mounts();
        if mounts.is_empty() {
            return false;
        }
        let (mut n, mut size, mut ticks) = (0i64, 0i64, 0i64);
        for m in &mounts {
            for w in walk(Path::new(&m.root), &[]) {
                n += 1;
                size += w.size;
                ticks ^= w.mtime_ticks ^ fxhash(&w.rel) as i64;
            }
        }
        let sig = format!("{}|{}|{}", n, size, ticks);
        let mut g = self.mount_signature.lock();
        if *g == sig {
            return false;
        }
        let first = g.is_empty();
        *g = sig;
        !first
    }

    /// Убрать из корзины всё старше срока (раз в минуту).
    pub fn sweep_trash(&self, node: &Node) {
        {
            let mut g = self.last_sweep.lock();
            if g.map(|t| t.elapsed().as_secs() < 60).unwrap_or(false) {
                return;
            }
            *g = Some(std::time::Instant::now());
        }
        let minutes = node.cfg.read().mounts.trash_minutes.max(0) as u64;
        let limit = std::time::SystemTime::now() - std::time::Duration::from_secs(minutes * 60);
        let Ok(scopes) = std::fs::read_dir(&self.trash_root) else { return };
        for s in scopes.flatten() {
            let Ok(batches) = std::fs::read_dir(s.path()) else { continue };
            for b in batches.flatten() {
                let t = b.metadata().and_then(|m| m.created().or_else(|_| m.modified())).unwrap_or(std::time::SystemTime::now());
                if t < limit {
                    let _ = std::fs::remove_dir_all(b.path());
                }
            }
        }
    }

    pub fn trash_list(&self) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        let Ok(scopes) = std::fs::read_dir(&self.trash_root) else { return out };
        for s in scopes.flatten() {
            let Ok(batches) = std::fs::read_dir(s.path()) else { continue };
            for b in batches.flatten() {
                let t = b.metadata().and_then(|m| m.created().or_else(|_| m.modified())).map(time::from_system).unwrap_or_else(|_| time::now());
                for w in walk(&b.path(), &[]) {
                    out.push(serde_json::json!({
                        "scope": s.file_name().to_string_lossy(), "batch": b.file_name().to_string_lossy(),
                        "path": w.rel, "size": w.size, "time": time::format(&t),
                    }));
                }
            }
        }
        out
    }

    /// Вернуть файл из корзины на место. Если на месте уже есть файл — рядом, с пометкой «из корзины».
    pub fn trash_restore(&self, node: &Node, scope: &str, batch: &str, p: &str) -> Option<String> {
        let mut norm = path::norm(p)?;
        if safe(scope) != scope || safe(batch) != batch {
            return None;
        }
        let src = path::to_local(&self.trash_root.join(scope).join(batch), &norm).ok()?;
        if !src.is_file() {
            return None;
        }
        let cfg = node.cfg.read().clone();
        let root = if let Some(own) = cfg.folders.iter().find(|f| safe(&f.id) == scope) {
            self.own_root(node, own)
        } else {
            let m = self.mounts().into_iter().find(|m| safe(&mount_scope(m)) == scope)?;
            if m.access != WRITE {
                return None;
            }
            self.mounts_touched.store(true, Ordering::SeqCst);
            self.wake.store(true, Ordering::SeqCst);
            PathBuf::from(&m.root)
        };
        let mut dest = path::to_local(&root, &norm).ok()?;
        if dest.exists() {
            let (stem, ext) = match norm.rfind('.') {
                Some(i) if i > norm.rfind('/').map(|x| x + 1).unwrap_or(0) => (norm[..i].to_string(), norm[i..].to_string()),
                _ => (norm.clone(), String::new()),
            };
            norm = format!("{} (из корзины){}", stem, ext);
            dest = path::to_local(&root, &norm).ok()?;
        }
        if let Some(d) = dest.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        bridge_core::files::move_replace(&src, &dest).ok()?;
        remove_empty_dirs(&self.trash_root.join(scope), src.parent()?);
        self.invalidate_own();
        Some(norm)
    }

    /// Удалить файл из своей общей папки (из панели) — в корзину.
    pub fn own_local_delete(&self, node: &Node, f: &SharedFolderConfig, p: &str) {
        let root = self.own_root(node, f);
        let Ok(full) = path::to_local(&root, p) else { return };
        if !full.is_file() {
            return;
        }
        self.to_trash(node, &f.id, p, &full);
        remove_empty_dirs(&root, full.parent().unwrap_or(&root));
        self.invalidate_own();
    }

    /// Удалить файл из подключённой папки соседа (из панели) — в корзину; удаление уйдёт хозяину.
    pub fn mount_local_delete(&self, node: &Node, m: &Mount, p: &str) {
        let root = PathBuf::from(&m.root);
        let Ok(full) = path::to_local(&root, p) else { return };
        if !full.is_file() {
            return;
        }
        self.to_trash(node, &mount_scope(m), p, &full);
        remove_empty_dirs(&root, full.parent().unwrap_or(&root));
        self.mounts_touched.store(true, Ordering::SeqCst);
        self.wake.store(true, Ordering::SeqCst);
    }

    pub fn allow_deletes(&self, peer: &str, id: &str) {
        if let Some(m) = self.mounts.lock().get_mut(&format!("{}/{}", peer, id).to_lowercase()) {
            m.allow_deletes_once = true;
            self.wake.store(true, Ordering::SeqCst);
        }
    }

    /// Сосед удалён или переименован: забыть его папки (файлы на диске остаются).
    pub fn forget_peer(&self, node: &Node, peer: &str) {
        let prefix = format!("{}/", peer.to_lowercase());
        self.mounts.lock().retain(|k, _| !k.starts_with(&prefix));
        self.mount_scanners.lock().retain(|k, _| !k.starts_with(&prefix));
        self.save(node);
    }

    // ───────────────────────── Подключённые папки соседей ─────────────────────────

    /// Обменяться со всеми общими папками соседей.
    pub async fn round(&self, node: &Arc<Node>) {
        let Ok(_g) = self.round_gate.try_lock() else { return };
        self.wake.store(false, Ordering::SeqCst);
        let cfg = node.cfg.read().clone();
        if !cfg.mounts.enabled {
            return;
        }
        let mroot = node.resolve(&cfg.mounts.root);
        let touched = self.mounts_touched.swap(false, Ordering::SeqCst);
        // состояние пишется на диск, только если что-то поменялось (у принимающего — тысячи записей)
        let mut changed = false;
        for peer in cfg.peers.iter().filter(|p| p.enabled && p.has_url()) {
            let ps = node.peer_state(&peer.name);
            let client = node.client_for(peer);
            let folders = if !ps.online && ps.last_seen.is_some() {
                Err(None)
            } else {
                client.folders().await.map_err(Some)
            };
            let folders = match folders {
                Ok(f) => f,
                Err(e) => {
                    let why = match e {
                        Some(e) => {
                            if e.status() == Some(404) {
                                continue; // старый сосед без общих папок
                            }
                            link::explain(&e, peer).0
                        }
                        None => ps.error.clone(),
                    };
                    for m in self.mounts.lock().values_mut().filter(|m| eq_ic(&m.peer, &peer.name)) {
                        let e = format!("нет связи с хозяином: {}", why);
                        if m.error != e {
                            m.error = e;
                            changed = true;
                        }
                    }
                    continue;
                }
            };
            for info in &folders {
                let key = format!("{}/{}", peer.name, info.id).to_lowercase();
                let mut m = {
                    let mut g = self.mounts.lock();
                    let m = g.entry(key.clone()).or_insert_with(|| Mount { peer: peer.name.clone(), id: info.id.clone(), ..Default::default() });
                    let root = mroot.join(&peer.name).join(&info.id).to_string_lossy().into_owned();
                    if m.peer != peer.name || m.title != info.title || m.access != info.access || m.root != root || m.revoked || m.owner_ignore != info.ignore {
                        changed = true;
                    }
                    m.peer = peer.name.clone();
                    m.title = info.title.clone();
                    m.access = info.access.clone();
                    m.root = mroot.join(&peer.name).join(&info.id).to_string_lossy().into_owned();
                    m.revoked = false;
                    m.owner_ignore = info.ignore.clone();
                    m.owner_max = info.max_bytes;
                    m.key = format!("{}/{}", peer.name, info.id);
                    m.clone()
                };
                // У хозяина ничего не менялось, здесь тоже, ничего не задержано — обмен не нужен
                if !touched && !info.rev.is_empty() && info.rev == m.owner_rev && !m.hold && !m.allow_deletes_once && m.error.is_empty() {
                    continue;
                }
                changed = true;
                match self.sync_mount(node, &client, &mut m).await {
                    Ok(()) => {
                        if m.error.starts_with("нет связи") {
                            m.error.clear();
                        }
                        // отпечаток запоминается, только если всё сошлось (иначе — повторить в следующий раз)
                        m.owner_rev = if m.error.is_empty() && !m.hold { info.rev.clone() } else { String::new() };
                    }
                    Err(e) => {
                        node.info(&format!("[общая {}] {}", m.key, e));
                        m.error = e;
                    }
                }
                self.mounts.lock().insert(key, m);
            }
            // доступ отозвали — папка остаётся у нас как есть, но больше не обновляется
            for m in self.mounts.lock().values_mut().filter(|m| eq_ic(&m.peer, &peer.name) && !folders.iter().any(|f| eq_ic(&f.id, &m.id)) && !m.revoked) {
                changed = true;
                m.revoked = true;
                m.error = "хозяин закрыл доступ — копия больше не обновляется".into();
            }
        }
        if changed {
            self.save(node);
        }
    }

    fn note(m: &mut Mount, text: &str) {
        m.recent.push(format!("{}  {}", chrono::Local::now().format("%d.%m %H:%M:%S"), text));
        let n = m.recent.len();
        if n > 30 {
            m.recent.drain(0..n - 30);
        }
    }

/// (3.3.1) План синхронизации папки соседа — что сейчас разошлось и что сделает обмен, без самого обмена.
    pub async fn plan(&self, node: &Arc<Node>, peer: &str, id: &str) -> Result<serde_json::Value, (u16, String)> {
        let m = self.find_mount(peer, id).ok_or((404, "нет такой папки соседа".to_string()))?;
        let pc = node.cfg.read().peer(&m.peer).cloned().ok_or((404, "нет соседа".to_string()))?;
        let client = node.client_for(&pc);
        let manifest = client.folder_manifest(&m.id).await.map_err(|e| (502, link::explain(&e, &pc).0))?;
        let ig = self.mount_ignore(node, &m);
        let root = PathBuf::from(&m.root);
        let remote: HashMap<String, FileEntry> = manifest.files.into_iter().filter(|f| path::norm(&f.path).is_some() && !ig.is_ignored(&f.path)).map(|f| (f.path.to_lowercase(), f)).collect();
        let local: HashMap<String, FileEntry> = {
            let mut busy = 0;
            let mut sc = self.mount_scanners.lock();
            let s = sc.entry(m.key.to_lowercase()).or_insert_with(|| {
                let mut t = TreeScanner::new(root.clone());
                t.skip_top.push(MY_EDITS_DIR.into());
                t
            });
            s.scan(&mut busy).into_iter().filter(|f| !ig.is_ignored(&f.path)).map(|f| (f.path.to_lowercase(), f)).collect()
        };
        let write = m.access == WRITE;
        let mut items = Vec::new();
        let (mut down, mut up) = (0i64, 0i64);
        let mut seen = HashSet::new();
        for p in remote.values().map(|f| f.path.clone()).chain(local.values().map(|f| f.path.clone())).chain(m.base.keys().cloned()) {
            let k = p.to_lowercase();
            if !seen.insert(k.clone()) {
                continue;
            }
            let (lf, rf) = (local.get(&k), remote.get(&k));
            let (l, r) = (lf.map(|x| x.sha256.clone()), rf.map(|x| x.sha256.clone()));
            if l == r {
                continue;
            }
            let b = base_get(&m.base, &p).cloned();
            let (lc, rc) = (l != b, r != b);
            let (act, size) = if !lc && rc {
                match rf {
                    None => ("delete_here", lf.map(|x| x.size).unwrap_or(0)),
                    Some(f) => (if lf.is_some() { "update" } else { "download" }, f.size),
                }
            } else if lc && !rc {
                match (write, lf) {
                    (true, None) => ("delete_owner", rf.map(|x| x.size).unwrap_or(0)),
                    (true, Some(f)) => ("upload", f.size),
                    (false, _) => ("restore", rf.map(|x| x.size).unwrap_or(0)),
                }
            } else {
                ("conflict", rf.map(|x| x.size).unwrap_or(0))
            };
            match act {
                "download" | "update" | "restore" | "conflict" => down += size,
                "upload" => up += size,
                _ => {}
            }
            items.push(serde_json::json!({ "path": p, "act": act, "size": size, "local": lf.map(|x| serde_json::json!({ "size": x.size, "time": time::format(&x.modified) })), "remote": rf.map(|x| serde_json::json!({ "size": x.size, "time": time::format(&x.modified) })) }));
        }
        items.sort_by(|a, b| a["act"].as_str().cmp(&b["act"].as_str()).then(a["path"].as_str().cmp(&b["path"].as_str())));
        let (sd, su) = (node.transfers.speed(&m.peer, true), node.transfers.speed(&m.peer, false));
        let cfg = node.cfg.read().clone();
        let lim = |mbit: f64| if mbit > 0.0 { (mbit * 125_000.0) as i64 } else { 0 };
        let sd = if sd > 0 { sd } else { lim(cfg.max_down_mbit as f64) };
        let su = if su > 0 { su } else { lim(cfg.max_up_mbit as f64) };
        let eta = |bytes: i64, sp: i64| if bytes == 0 { 0 } else if sp > 0 { bytes / sp + 1 } else { -1 };
        let (tr, failed) = self.transfer_view(&m.key);
        Ok(serde_json::json!({
            "peer": m.peer, "id": m.id, "access": m.access, "hold": m.hold, "holdReason": m.hold_reason, "lastSync": m.last_sync.map(|t| time::format(&t)),
            "items": items.iter().take(2000).collect::<Vec<_>>(), "total": items.len(), "down": down, "up": up,
            "speedDown": sd, "speedUp": su, "etaDown": eta(down, sd), "etaUp": eta(up, su),
            "localFiles": local.len(), "remoteFiles": remote.len(), "transfers": tr, "failed": failed,
        }))
    }

    async fn sync_mount(&self, node: &Arc<Node>, client: &Arc<PeerClient>, m: &mut Mount) -> Result<(), String> {
        let root = PathBuf::from(&m.root);
        let notes: Vec<String> = {
            let mut g = self.notes.lock();
            let (mine, rest): (Vec<_>, Vec<_>) = g.drain(..).partition(|(k, _)| eq_ic(k, &m.key));
            *g = rest;
            mine.into_iter().map(|x| x.1).collect()
        };
        for n in notes {
            Self::note(m, &n);
        }
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let manifest = client.folder_manifest(&m.id).await.map_err(|e| e.to_string())?;
        // Игнор: таких путей для обмена нет — не скачиваем, не отправляем, не удаляем
        let ig = self.mount_ignore(node, m);
        let remote: HashMap<String, FileEntry> =
            manifest.files.into_iter().filter(|f| path::norm(&f.path).is_some() && !ig.is_ignored(&f.path)).map(|f| (f.path.to_lowercase(), f)).collect();
        let mut busy = 0;
        let local: HashMap<String, FileEntry> = {
            let mut sc = self.mount_scanners.lock();
            let s = sc.entry(m.key.to_lowercase()).or_insert_with(|| {
                let mut t = TreeScanner::new(root.clone());
                t.skip_top.push(MY_EDITS_DIR.into());
                t
            });
            s.scan(&mut busy).into_iter().filter(|f| !ig.is_ignored(&f.path)).map(|f| (f.path.to_lowercase(), f)).collect()
        };
        let ignored_base: Vec<String> = m.base.keys().filter(|p| ig.is_ignored(p)).cloned().collect();
        for p in ignored_base {
            m.base.remove(&p);
        }
        let mut paths: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for p in remote.values().map(|f| f.path.clone()).chain(local.values().map(|f| f.path.clone())).chain(m.base.keys().cloned()) {
            if seen.insert(p.to_lowercase()) {
                paths.push(p);
            }
        }
        let write = m.access == WRITE;
        // Защита от массового удаления
        let remote_gone = m.base.iter().filter(|(p, b)| !remote.contains_key(&p.to_lowercase()) && local.get(&p.to_lowercase()).map(|l| &l.sha256 == *b).unwrap_or(false)).count();
        let local_gone = m
            .base
            .iter()
            .filter(|(p, b)| {
                !local.contains_key(&p.to_lowercase())
                    && remote.get(&p.to_lowercase()).map(|r| &r.sha256 == *b).unwrap_or(false)
                    && !path::to_local(&root, p).map(|f| f.exists()).unwrap_or(false)
            })
            .count();
        // Удаления за последние 10 минут считаются вместе с нынешними: удаление тысячи файлов «по частям» тоже задержится
        let now_s = chrono::Utc::now().timestamp();
        m.recent_deletes.retain(|t| now_s - *t < 600);
        let recent = m.recent_deletes.len();
        let base_ref = m.base.len() + recent;
        let massive = base_ref >= 10 && remote_gone + local_gone > 0 && (remote_gone + local_gone + recent) * 2 > base_ref;
        if massive && !m.allow_deletes_once {
            m.hold = true;
            let mut parts = Vec::new();
            if remote_gone > 0 {
                parts.push(format!("у хозяина {}", remote_gone));
            }
            if local_gone > 0 {
                parts.push(format!("здесь {}", local_gone));
            }
            m.hold_reason = format!(
                "разом удаляется {} из {} файлов ({}){} — подтвердите в панели",
                remote_gone + local_gone,
                m.base.len(),
                parts.join(", "),
                if recent > 0 { format!("; до этого за 10 минут удалено {}", recent) } else { String::new() }
            );
        } else if !massive {
            m.hold = false;
            m.hold_reason.clear();
        }
        let allow_deletes = !m.hold || m.allow_deletes_once;
        let scope = mount_scope(m);
        for p in paths {
            let k = p.to_lowercase();
            let l = local.get(&k).map(|x| x.sha256.clone());
            let r = remote.get(&k).map(|x| x.sha256.clone());
            let b = base_get(&m.base, &p).cloned();
            let lsize = local.get(&k).map(|x| x.size).unwrap_or(0);
            let rsize = remote.get(&k).map(|x| x.size).unwrap_or(0);
            let Ok(full) = path::to_local(&root, &p) else { continue };
            if l.is_none() && full.exists() {
                continue; // файл сейчас пишут
            }
            if l == r {
                match &l {
                    Some(s) => {
                        base_remove(&mut m.base, &p);
                        m.base.insert(p.clone(), s.clone());
                    }
                    None => base_remove(&mut m.base, &p),
                }
                continue;
            }
            let (local_changed, remote_changed) = (l != b, r != b);
            if !local_changed && remote_changed {
                match &r {
                    None => {
                        if !allow_deletes {
                            continue;
                        }
                        self.to_trash(node, &scope, &p, &full);
                        remove_empty_dirs(&root, full.parent().unwrap_or(&root));
                        base_remove(&mut m.base, &p);
                        m.recent_deletes.push(chrono::Utc::now().timestamp());
                        Self::note(m, &format!("удалён хозяином: {}", p));
                    }
                    Some(rs) => {
                        if self.download(node, client, m, &p, rs, rsize, &full, &scope).await? {
                            Self::note(m, &format!("обновлён: {}", p));
                        }
                    }
                }
            } else if local_changed && !remote_changed {
                if write {
                    match &l {
                        None => {
                            if !allow_deletes {
                                continue;
                            }
                            let code = match &r {
                                None => 200,
                                Some(rs) => client.folder_delete(&m.id, &p, rs).await.map_err(|e| e.to_string())?,
                            };
                            if code == 200 {
                                base_remove(&mut m.base, &p);
                                m.recent_deletes.push(chrono::Utc::now().timestamp());
                                Self::note(m, &format!("удалён у хозяина: {}", p));
                            }
                        }
                        Some(ls) => {
                            let tk = tkey(&m.key, &p);
                            if self.transfers.lock().contains_key(&tk) || self.blocked(&tk, ls) {
                                continue;
                            }
                            let own_max = node.max_file_bytes();
                            let max = if m.owner_max > 0 { own_max.min(m.owner_max) } else { own_max };
                            if lsize > max {
                                let why = if max == m.owner_max && m.owner_max > 0 {
                                    format!("{} больше предела у хозяина «{}» ({} МБ, maxFileMb там)", human(lsize), m.peer, max / 1024 / 1024)
                                } else {
                                    format!("{} больше предела {} МБ (maxFileMb)", human(lsize), max / 1024 / 1024)
                                };
                                self.fail(&tk, ls, &why, None);
                                Self::note(m, &format!("не отправлен: {} — {}", p, why));
                                node.info(&format!("[общая {}] {} не отправлен: {}", m.key, p, why));
                                continue;
                            }
                            let base = r.clone().unwrap_or_default();
                            if lsize > BIG_FILE {
                                self.spawn_transfer(node, client, m, &p, &full, ls, &base, lsize, true);
                                continue;
                            }
                            match client.folder_upload(&m.id, &p, &full, ls, &base).await {
                                Ok(200) => {
                                    base_remove(&mut m.base, &p);
                                    m.base.insert(p.clone(), ls.clone());
                                    Self::note(m, &format!("отправлен хозяину: {}", p));
                                }
                                Ok(403) => m.error = "запись запрещена хозяином".into(),
                                Ok(409) => {} // у хозяина файл изменился — следующий обмен разберёт конфликт
                                Ok(c) => Self::note(m, &format!("не отправлен: {} — хозяин ответил кодом {}", p, c)),
                                Err(PeerError::Http(code, why)) => {
                                    self.fail(&tk, ls, &why, if code == 413 { None } else { Some(120) });
                                    Self::note(m, &format!("не отправлен: {} — {}", p, why));
                                    node.info(&format!("[общая {}] {} не отправлен: {}", m.key, p, why));
                                }
                                Err(e) => return Err(e.to_string()),
                            }
                        }
                    }
                } else {
                    // Только чтение: свою правку не теряем — в «_мои-правки», копию хозяина восстанавливаем
                    if l.is_some() {
                        if let Ok(mut dest) = path::to_local(&root.join(MY_EDITS_DIR), &p) {
                            if let Some(d) = dest.parent() {
                                let _ = std::fs::create_dir_all(d);
                            }
                            if dest.exists() {
                                dest = bridge_core::files::conflict_name(&dest, None);
                            }
                            let _ = std::fs::rename(&full, &dest);
                        }
                    }
                    match &r {
                        Some(rs) => {
                            self.download(node, client, m, &p, rs, rsize, &full, &scope).await?;
                        }
                        None => base_remove(&mut m.base, &p),
                    }
                    Self::note(m, &format!("только чтение — правка сохранена в «{}»: {}", MY_EDITS_DIR, p));
                }
            } else {
                // Правили обе стороны: копия хозяина главная, наша — рядом как «конфликт»
                if l.is_some() {
                    let copy = bridge_core::files::conflict_name(&full, None);
                    let _ = std::fs::rename(&full, &copy);
                    Self::note(m, &format!("конфликт: ваша версия — {}", copy.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                }
                match &r {
                    Some(rs) => {
                        self.download(node, client, m, &p, rs, rsize, &full, &scope).await?;
                    }
                    None => base_remove(&mut m.base, &p),
                }
            }
        }
        if m.allow_deletes_once {
            m.recent_deletes.clear();
            m.allow_deletes_once = false;
            m.hold = false;
            m.hold_reason.clear();
        }
        let mut b2 = 0;
        let now: Vec<FileEntry> = {
            let mut sc = self.mount_scanners.lock();
            sc.get_mut(&m.key.to_lowercase()).map(|s| s.scan(&mut b2)).unwrap_or_default().into_iter().filter(|f| !ig.is_ignored(&f.path)).collect()
        };
        m.files = now.len() as i64;
        m.bytes = now.iter().map(|x| x.size).sum();
        m.last_sync = Some(time::now());
        m.error = if busy > 0 { format!("{} файл(ов) сейчас заняты другой программой — уйдут позже", busy) } else { String::new() };
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    /// Скачать и поставить файл. false — не сейчас (идёт в фоне, больше предела, не прошёл): база не меняется.
    async fn download(&self, node: &Arc<Node>, client: &Arc<PeerClient>, m: &mut Mount, p: &str, sha: &str, size: i64, full: &Path, scope: &str) -> Result<bool, String> {
        let tk = tkey(&m.key, p);
        let pre = self.prefetched.lock().remove(&tk);
        let tmp = match pre {
            Some((t, s)) if s.eq_ignore_ascii_case(sha) => t,
            other => {
                if let Some((t, _)) = other {
                    let _ = std::fs::remove_file(&t);
                }
                if self.transfers.lock().contains_key(&tk) || self.blocked(&tk, sha) {
                    return Ok(false);
                }
                let max = node.max_file_bytes();
                if size > max {
                    let why = format!("{} больше предела {} МБ (maxFileMb)", human(size), max / 1024 / 1024);
                    self.fail(&tk, sha, &why, None);
                    Self::note(m, &format!("не скачан: {} — {}", p, why));
                    return Ok(false);
                }
                if size > BIG_FILE {
                    let base = base_get(&m.base, p).cloned().unwrap_or_default();
                    self.spawn_transfer(node, client, m, p, full, sha, &base, size, false);
                    return Ok(false);
                }
                let tmp = node.tmp_file();
                match client.folder_download(&m.id, p, &tmp, node.max_file_bytes()).await {
                    Ok(got) if got.eq_ignore_ascii_case(sha) => tmp,
                    Ok(_) => {
                        let _ = std::fs::remove_file(&tmp);
                        return Ok(false);
                    }
                    Err(PeerError::Http(code, why)) => {
                        let _ = std::fs::remove_file(&tmp);
                        self.fail(&tk, sha, &why, if code == 413 { None } else { Some(120) });
                        Self::note(m, &format!("не скачан: {} — {}", p, why));
                        return Ok(false);
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp);
                        return Err(e.to_string());
                    }
                }
            }
        };
        if let Some(d) = full.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if full.exists() {
            self.to_trash(node, scope, p, full);
        }
        bridge_core::files::move_replace(&tmp, full).map_err(|e| e.to_string())?;
        if let Some(s) = self.mount_scanners.lock().get_mut(&m.key.to_lowercase()) {
            s.remember(p, full, sha);
        }
        base_remove(&mut m.base, p);
        m.base.insert(p.to_string(), sha.to_string());
        node.emit("file-in", serde_json::json!({ "mount": m.key, "path": p }));
        Ok(true)
    }
}

fn fxhash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
