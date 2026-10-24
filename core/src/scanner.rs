//! Снимок папки: путь → sha, размер, время. sha пересчитывается только у изменившихся файлов.
//! Файлы, которые сейчас кто-то пишет, в снимок не попадают (возьмутся в следующий раз).

use crate::path;
use crate::proto::FileEntry;
use crate::time;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Файл папки при обходе.
pub struct Walked {
    pub full: PathBuf,
    pub rel: String,
    pub size: i64,
    pub mtime_ticks: i64,
}

/// Обойти папку: файлы с размером и временем. Ссылки на папки не обходятся; `skip_top` — папки первого уровня, которые пропустить.
pub fn walk(root: &Path, skip_top: &[&str]) -> Vec<Walked> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let full = entry.path();
            if ft.is_dir() {
                if dir == root {
                    if let Some(n) = full.file_name().and_then(|n| n.to_str()) {
                        if skip_top.iter().any(|s| s.eq_ignore_ascii_case(n) || s.to_lowercase() == n.to_lowercase()) {
                            continue;
                        }
                    }
                }
                match entry.metadata() {
                    Ok(m) if !path::is_link(&m) => stack.push(full),
                    _ => {}
                }
                continue;
            }
            if ft.is_symlink() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let Some(rel) = path::relative(root, &full) else { continue };
            let mtime = meta.modified().map(time::from_system).unwrap_or_else(|_| time::min());
            out.push(Walked { full, rel, size: meta.len() as i64, mtime_ticks: time::to_ticks(&mtime) });
        }
    }
    out
}

#[derive(Clone)]
struct Cached {
    size: i64,
    ticks: i64,
    sha: String,
}

/// Сканер с кэшем sha.
pub struct TreeScanner {
    root: PathBuf,
    cache: HashMap<String, Cached>,
    pub skip_top: Vec<String>,
}

impl TreeScanner {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        TreeScanner { root: root.into(), cache: HashMap::new(), skip_top: Vec::new() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Файлы папки (по пути, по порядку). `busy` — сколько пропущено, потому что их пишут.
    pub fn scan(&mut self, busy: &mut usize) -> Vec<FileEntry> {
        *busy = 0;
        let mut list = Vec::new();
        if !self.root.is_dir() {
            return list;
        }
        let skip: Vec<&str> = self.skip_top.iter().map(|s| s.as_str()).collect();
        let mut seen: HashSet<String> = HashSet::new();
        for w in walk(&self.root, &skip) {
            let Some(p) = path::norm(&w.rel) else { continue };
            if path::is_junk(&p) {
                continue;
            }
            let k = path::key(&p);
            seen.insert(k.clone());
            let sha = match self.cache.get(&k) {
                Some(c) if c.size == w.size && c.ticks == w.mtime_ticks => c.sha.clone(),
                _ => match path::sha256_file_settled(&w.full) {
                    Ok(sha) => {
                        self.cache.insert(k, Cached { size: w.size, ticks: w.mtime_ticks, sha: sha.clone() });
                        sha
                    }
                    Err(_) => {
                        *busy += 1;
                        continue;
                    }
                },
            };
            list.push(FileEntry { path: p, sha256: sha, size: w.size, modified: time::from_ticks(w.mtime_ticks), rev: 0 });
        }
        self.cache.retain(|k, _| seen.contains(k));
        list.sort_by(|a, b| crate::cmp_ordinal(&a.path, &b.path));
        list
    }

    /// Запомнить известную sha (после своей же записи файла — без повторного чтения).
    pub fn remember(&mut self, rel: &str, full: &Path, sha: &str) {
        if let Ok(m) = std::fs::metadata(full) {
            let mtime = m.modified().map(time::from_system).unwrap_or_else(|_| time::min());
            self.cache.insert(path::key(rel), Cached { size: m.len() as i64, ticks: time::to_ticks(&mtime), sha: sha.into() });
        }
    }
}
