//! Файловый слой пространства: сканирование папки в манифест и решение, что забирать у соседа по правилам приёма.
//! Узел никогда не пишет в чужую папку — только забирает к себе. Сеть — снаружи (узел), здесь — правила и диск.

use crate::config::ShareConfig;
use crate::ignore::IgnoreRules;
use crate::path;
use crate::proto::{FileEntry, FileManifest};
use crate::scanner::walk;
use crate::time::{self, Time};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScanEntry {
    pub path: String,
    pub sha: String,
    pub size: i64,
    pub mtime_ticks: i64,
    pub rev: i64,
}

/// Состояние синхронизации файлов пространства (dataDir/state/<id>.json, поле files).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileSyncState {
    pub scan: BTreeMap<String, ScanEntry>,
    /// ключ пути → sha, на которой узел последний раз сошёлся с мостом
    pub base: BTreeMap<String, String>,
    /// сосед → ключ пути → sha у соседа при последнем просмотре
    pub seen: BTreeMap<String, BTreeMap<String, String>>,
}

/// Что делать с файлом соседа.
#[derive(Debug, PartialEq)]
pub enum Decision {
    /// Пропустить (правила, игнор, служебное).
    Skip,
    /// Ничего не качать; запомнить, что видели эту версию.
    Seen,
    /// Уже совпадает: запомнить и базу, и «видели».
    Same,
    /// Ничего (у соседа ничего нового).
    Nothing,
    /// Свой файл сейчас пишется — решить позже.
    Busy,
    /// Скачать; local_changed — своя правка, будет конфликт.
    Download { local_changed: bool },
}

pub struct FolderShare {
    pub root: PathBuf,
    history_dir: PathBuf,
    pub tmp_dir: PathBuf,
    keep_days: i64,
    pub state: FileSyncState,
    /// Файлы моложе — не трогаем (их, возможно, ещё пишут). Секунды.
    pub settle_secs: f64,
    manifest: FileManifest,
    /// Есть файлы моложе settle — нужен ещё один проход.
    pub unsettled: bool,
}

impl FolderShare {
    pub fn new(root: PathBuf, history_dir: PathBuf, tmp_dir: PathBuf, state: FileSyncState, keep_days: i64) -> Self {
        let _ = std::fs::create_dir_all(&root);
        FolderShare { root, history_dir, tmp_dir, keep_days, state, settle_secs: 2.0, manifest: FileManifest::default(), unsettled: false }
    }

    pub fn manifest(&self) -> &FileManifest {
        &self.manifest
    }

    /// Пересканировать папку. true — что-то изменилось.
    pub fn rescan(&mut self, node: &str, share: &str, ig: &IgnoreRules) -> bool {
        let now = time::now();
        let now_ticks = time::to_ticks(&now);
        let settle_ticks = (self.settle_secs * 10_000_000.0) as i64;
        let mut seen: HashSet<String> = HashSet::new();
        let mut files = Vec::new();
        let mut changed = false;
        self.unsettled = false;
        for w in walk(&self.root, &[crate::SERVICE_DIR]) {
            let Some(p) = path::norm(&w.rel) else { continue };
            if path::is_service(&p) || path::is_junk(&p) || ig.is_ignored(&p) {
                continue;
            }
            if now_ticks - w.mtime_ticks < settle_ticks {
                self.unsettled = true;
                continue; // ещё пишут — в следующий круг
            }
            let k = path::key(&p);
            let need = match self.state.scan.get(&k) {
                Some(e) => e.size != w.size || e.mtime_ticks != w.mtime_ticks || e.path != p,
                None => true,
            };
            if need {
                let sha = match path::sha256_file_settled(&w.full) {
                    Ok(s) => s,
                    Err(_) => {
                        self.unsettled = true;
                        // занят — если был в снимке, остаётся прежним
                        if self.state.scan.contains_key(&k) {
                            seen.insert(k);
                        }
                        continue;
                    }
                };
                match self.state.scan.get_mut(&k) {
                    None => {
                        self.state.scan.insert(k.clone(), ScanEntry { path: p.clone(), sha, size: w.size, mtime_ticks: w.mtime_ticks, rev: 1 });
                        changed = true;
                    }
                    Some(e) => {
                        if e.sha != sha {
                            e.rev += 1;
                            changed = true;
                        }
                        e.path = p.clone();
                        e.sha = sha;
                        e.size = w.size;
                        e.mtime_ticks = w.mtime_ticks;
                    }
                }
            }
            seen.insert(k);
        }
        // Пропавшие: удалены — или «ещё пишутся» (тогда остаются в снимке)
        let gone: Vec<String> = self.state.scan.keys().filter(|k| !seen.contains(*k)).cloned().collect();
        for k in gone {
            let p = self.state.scan[&k].path.clone();
            let exists = path::to_local(&self.root, &p).map(|f| f.is_file()).unwrap_or(false);
            if !exists || ig.is_ignored(&p) {
                self.state.scan.remove(&k);
                changed = true;
            }
        }
        for e in self.state.scan.values() {
            files.push(FileEntry { path: e.path.clone(), sha256: e.sha.clone(), size: e.size, modified: time::from_ticks(e.mtime_ticks), rev: e.rev });
        }
        files.sort_by(|a, b| crate::cmp_ordinal(&a.path, &b.path));
        self.manifest = FileManifest { node: node.into(), share: share.into(), time: now, files };
        changed
    }

    pub fn find(&self, p: &str) -> Option<&FileEntry> {
        let k = path::key(p);
        self.manifest.files.iter().find(|f| path::key(&f.path) == k)
    }

    fn local_sha(&self, k: &str) -> Option<&str> {
        self.state.scan.get(k).map(|e| e.sha.as_str())
    }

    fn scan_time(&self, k: &str) -> Time {
        self.state.scan.get(k).map(|e| time::from_ticks(e.mtime_ticks)).unwrap_or_else(time::min)
    }

    /// Решить, что делать с файлом соседа. Правила — ТЗ, раздел 5 («Забор от соседа»).
    pub fn decide(&mut self, r: &FileEntry, peer: &str, cfg: &ShareConfig, ig: &IgnoreRules) -> (Decision, Option<String>) {
        let Some(p) = path::norm(&r.path) else { return (Decision::Skip, None) };
        if path::is_service(&p) || path::is_junk(&p) || ig.is_ignored(&p) || !cfg.accepts_file(&p, peer) {
            return (Decision::Skip, None);
        }
        let k = path::key(&p);
        let l = self.local_sha(&k).map(|s| s.to_string());
        let b = self.state.base.get(&k).cloned();
        let s = self.state.seen.get(peer).and_then(|m| m.get(&k)).cloned();
        let rs = r.sha256.clone();
        let full = match path::to_local(&self.root, &p) {
            Ok(f) => f,
            Err(_) => return (Decision::Skip, None),
        };
        if l.as_deref() == Some(rs.as_str()) {
            return (Decision::Same, Some(p));
        }
        if s.as_deref() == Some(rs.as_str()) {
            return (Decision::Nothing, Some(p));
        }
        if b.as_deref() == Some(rs.as_str()) {
            return (Decision::Seen, Some(p));
        }
        if l.is_none() && full.exists() {
            return (Decision::Busy, Some(p)); // есть, но ещё пишется
        }
        let local_changed = l.is_some() && l != b;
        if local_changed && s.is_none() && b.is_none() {
            // Первая встреча: у обоих файл, версии разные. Свою, если новее, не трогаем
            if self.scan_time(&k) >= r.modified {
                return (Decision::Seen, Some(p));
            }
        } else if !local_changed && l.is_some() && s.is_none() && b.is_some() && r.modified < self.scan_time(&k) {
            // Первая встреча с соседом, у которого старая версия того, что пришло от другого, — не откатываемся
            return (Decision::Seen, Some(p));
        }
        (Decision::Download { local_changed }, Some(p))
    }

    pub fn mark(&mut self, peer: &str, p: &str, sha: &str, base_too: bool) {
        let k = path::key(p);
        if base_too {
            self.state.base.insert(k.clone(), sha.into());
        }
        self.state.seen.entry(peer.to_string()).or_default().insert(k, sha.into());
    }

    /// Поставить скачанный файл на место. Возвращает имя копии при конфликте.
    pub fn install(&mut self, node: &str, peer: &str, p: &str, tmp: &Path, r: &FileEntry, local_changed: bool) -> std::io::Result<Option<String>> {
        let k = path::key(p);
        let full = path::to_local(&self.root, p)?;
        let mut conflict = None;
        if local_changed && full.exists() {
            let copy = conflict_name(&full, Some(node));
            std::fs::rename(&full, &copy)?;
            conflict = Some(format!("{} → {}", p, copy.file_name().and_then(|n| n.to_str()).unwrap_or("")));
        } else if full.exists() {
            self.backup(p, &full);
        }
        if let Some(d) = full.parent() {
            std::fs::create_dir_all(d)?;
        }
        move_replace(tmp, &full)?;
        // время файла — как у источника: так «первая встреча» с третьим узлом сравнивает честно
        set_mtime(&full, &r.modified);
        let meta = std::fs::metadata(&full)?;
        let mtime = meta.modified().map(time::from_system).unwrap_or_else(|_| time::min());
        let rev = self.state.scan.get(&k).map(|e| e.rev).unwrap_or(0) + 1;
        self.state.scan.insert(k.clone(), ScanEntry { path: p.into(), sha: r.sha256.clone(), size: meta.len() as i64, mtime_ticks: time::to_ticks(&mtime), rev });
        self.state.base.insert(k.clone(), r.sha256.clone());
        self.state.seen.entry(peer.to_string()).or_default().insert(k, r.sha256.clone());
        Ok(conflict)
    }

    fn backup(&self, p: &str, full: &Path) {
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
        if let Ok(mut dest) = path::to_local(&self.history_dir, p) {
            let name = format!("{}.{}", dest.file_name().and_then(|n| n.to_str()).unwrap_or("f"), stamp);
            dest.set_file_name(name);
            if let Some(d) = dest.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::copy(full, &dest);
        }
    }

    /// Удалить из истории версии старше срока.
    pub fn clean_history(&self) {
        let limit = std::time::SystemTime::now() - std::time::Duration::from_secs((self.keep_days.max(0) as u64) * 86400);
        for w in walk(&self.history_dir, &[]) {
            if let Ok(m) = std::fs::metadata(&w.full).and_then(|m| m.modified()) {
                if m < limit {
                    let _ = std::fs::remove_file(&w.full);
                }
            }
        }
    }
}

/// «имя.конфликт-<узел>-<дата-время>.расш» (узел — необязательно).
pub fn conflict_name(full: &Path, node: Option<&str>) -> PathBuf {
    let dir = full.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = full.file_stem().and_then(|s| s.to_str()).unwrap_or("файл");
    let ext = full.extension().and_then(|s| s.to_str()).map(|e| format!(".{}", e)).unwrap_or_default();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let base = match node {
        Some(n) => format!("{}.конфликт-{}-{}", stem, n, stamp),
        None => format!("{}.конфликт-{}", stem, stamp),
    };
    let mut c = dir.join(format!("{}{}", base, ext));
    let mut i = 2;
    while c.exists() {
        c = dir.join(format!("{}-{}{}", base, i, ext));
        i += 1;
    }
    c
}

/// Переместить с заменой (между дисками — копированием).
pub fn move_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            let _ = std::fs::remove_file(from);
            Ok(())
        }
    }
}

pub fn set_mtime(full: &Path, t: &Time) {
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(full) {
        let st: std::time::SystemTime = (*t).into();
        let _ = f.set_modified(st);
    }
}
