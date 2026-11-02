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

/// Файл не менялся, пока его читали (размер и время те же).
fn unchanged(w: &Walked) -> bool {
    match std::fs::metadata(&w.full) {
        Ok(m) => m.len() as i64 == w.size && m.modified().map(|t| time::to_ticks(&time::from_system(t)) == w.mtime_ticks).unwrap_or(false),
        Err(_) => false,
    }
}

/// Сканер с кэшем sha.
pub struct TreeScanner {
    root: PathBuf,
    cache: HashMap<String, Cached>,
    pub skip_top: Vec<String>,
    /// Сколько файл должен пролежать без изменений, чтобы попасть в снимок (тики по 100 нс). Пустой — в 5 раз дольше:
    /// многие программы сначала создают пустой файл, а пишут в него позже (3.10).
    pub settle_ticks: i64,
    /// Ключи (path::key) файлов, которые при последнем обходе были заняты.
    pub busy_keys: HashSet<String>,
}

impl TreeScanner {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        TreeScanner { root: root.into(), cache: HashMap::new(), skip_top: Vec::new(), settle_ticks: 20_000_000, busy_keys: HashSet::new() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn with_settle(mut self, secs: f64) -> Self {
        self.settle_ticks = (secs.max(0.0) * 10_000_000.0) as i64;
        self
    }

    /// Файлы папки (по пути, по порядку). `busy` — сколько пропущено, потому что их пишут.
    /// Пока файл пишут (свежий, занят или изменился во время чтения), в снимок идёт его прежняя версия, если она известна:
    /// иначе сосед решил бы, что файл удалён.
    pub fn scan(&mut self, busy: &mut usize) -> Vec<FileEntry> {
        *busy = 0;
        self.busy_keys.clear();
        let mut list = Vec::new();
        if !self.root.is_dir() {
            return list;
        }
        let skip: Vec<&str> = self.skip_top.iter().map(|s| s.as_str()).collect();
        let mut seen: HashSet<String> = HashSet::new();
        let now = time::to_ticks(&time::now());
        for w in walk(&self.root, &skip) {
            let Some(p) = path::norm(&w.rel) else { continue };
            if path::is_junk(&p) {
                continue;
            }
            let k = path::key(&p);
            seen.insert(k.clone());
            if let Some(c) = self.cache.get(&k).filter(|c| c.size == w.size && c.ticks == w.mtime_ticks) {
                list.push(FileEntry { path: p, sha256: c.sha.clone(), size: w.size, modified: time::from_ticks(w.mtime_ticks), rev: 0 });
                continue;
            }
            let age = now - w.mtime_ticks;
            let need = if w.size == 0 { self.settle_ticks * 5 } else { self.settle_ticks };
            // время из будущего (часы сбиты, файл с другого ПК) — не ждём
            let got = if (0..need).contains(&age) { None } else { path::sha256_file_settled(&w.full).ok().filter(|_| unchanged(&w)) };
            match got {
                Some(sha) => {
                    self.cache.insert(k, Cached { size: w.size, ticks: w.mtime_ticks, sha: sha.clone() });
                    list.push(FileEntry { path: p, sha256: sha, size: w.size, modified: time::from_ticks(w.mtime_ticks), rev: 0 });
                }
                None => {
                    *busy += 1;
                    self.busy_keys.insert(k.clone());
                    if let Some(c) = self.cache.get(&k) {
                        list.push(FileEntry { path: p, sha256: c.sha.clone(), size: c.size, modified: time::from_ticks(c.ticks), rev: 0 });
                    }
                }
            }
        }
        self.cache.retain(|k, _| seen.contains(k));
        list.sort_by(|a, b| crate::cmp_ordinal(&a.path, &b.path));
        list
    }

    /// Забыть кэш: следующий обход пересчитает sha всех файлов (жёсткая сверка, 3.11).
    pub fn forget(&mut self) {
        self.cache.clear();
    }

    /// Запомнить известную sha (после своей же записи файла — без повторного чтения).
    pub fn remember(&mut self, rel: &str, full: &Path, sha: &str) {
        if let Ok(m) = std::fs::metadata(full) {
            let mtime = m.modified().map(time::from_system).unwrap_or_else(|_| time::min());
            self.cache.insert(path::key(rel), Cached { size: m.len() as i64, ticks: time::to_ticks(&mtime), sha: sha.into() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_age(p: &Path, secs: i64) {
        crate::files::set_mtime(p, &(time::now() - chrono::Duration::seconds(secs)));
    }

    #[test]
    fn fresh_and_empty_files_wait() {
        let dir = std::env::temp_dir().join(format!("most-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = TreeScanner::new(&dir).with_settle(2.0);
        let (a, e) = (dir.join("a.txt"), dir.join("empty.txt"));
        std::fs::write(&a, b"hello").unwrap();
        std::fs::write(&e, b"").unwrap();
        let mut busy = 0;
        // свежие: не в снимке
        assert!(s.scan(&mut busy).is_empty());
        assert_eq!(busy, 2);
        // через 3 с: обычный файл готов, пустой ещё ждёт (нужно 10 с)
        set_age(&a, 3);
        set_age(&e, 3);
        let l = s.scan(&mut busy);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].path, "a.txt");
        assert_eq!(busy, 1);
        set_age(&e, 11);
        assert_eq!(s.scan(&mut busy).len(), 2);
        assert_eq!(busy, 0);
        // файл пишут заново: в снимке прежняя версия, ключ — в занятых
        std::fs::write(&a, b"").unwrap();
        let l = s.scan(&mut busy);
        let x = l.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!(x.size, 5);
        assert!(s.busy_keys.contains("a.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
