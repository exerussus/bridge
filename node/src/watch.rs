//! Слежение за папками по событиям файловой системы (Windows — ReadDirectoryChangesW, Linux — inotify, macOS — kqueue).
//! Вместо обхода папок каждые 2 секунды: узел смотрит в папку, только когда в ней что-то поменялось.

use crate::node::Node;
use notify::{RecursiveMode, Watcher as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

pub struct Watcher {
    _inner: notify::RecommendedWatcher,
    pub roots: Vec<PathBuf>,
}

#[derive(Clone, Copy)]
enum Kind {
    Share(usize),
    Outgoing,
    Own,
    Mounts,
}

fn starts_with_ic(p: &Path, root: &Path) -> bool {
    if p.starts_with(root) {
        return true;
    }
    #[cfg(windows)]
    {
        let (a, b) = (p.to_string_lossy().to_lowercase(), root.to_string_lossy().to_lowercase());
        return a.starts_with(&b);
    }
    #[allow(unreachable_code)]
    false
}

impl Watcher {
    pub fn start(node: &Arc<Node>) -> Option<Watcher> {
        let mut targets: Vec<(PathBuf, Kind)> = Vec::new();
        for (i, rt) in node.shares().iter().enumerate() {
            targets.push((rt.root.clone(), Kind::Share(i)));
        }
        let cfg = node.cfg.read().clone();
        if cfg.mailbox.enabled {
            targets.push((node.resolve(&cfg.mailbox.outgoing), Kind::Outgoing));
        }
        for f in &cfg.folders {
            targets.push((node.resolve(&f.path), Kind::Own));
        }
        if cfg.mounts.enabled {
            let r = node.resolve(&cfg.mounts.root);
            let _ = std::fs::create_dir_all(&r);
            targets.push((r, Kind::Mounts));
        }
        let data_dir = node.data_dir.clone();
        let weak: Weak<Node> = Arc::downgrade(node);
        let map = targets.clone();
        let handler = move |res: notify::Result<notify::Event>| {
            let Some(node) = weak.upgrade() else { return };
            // Переполнение очереди событий или ошибка — пересмотреть всё
            let ev = match res {
                Ok(ev) if !ev.need_rescan() && !ev.paths.is_empty() => ev,
                _ => {
                    for rt in node.shares() {
                        rt.files_dirty.store(true, Ordering::SeqCst);
                    }
                    node.mailbox.dirty.store(true, Ordering::SeqCst);
                    node.folders.invalidate_own();
                    node.folders.own_dirty.store(true, Ordering::SeqCst);
                    node.folders.mounts_dirty.store(true, Ordering::SeqCst);
                    node.folders.mounts_touched.store(true, Ordering::SeqCst);
                    node.wake.notify_one();
                    return;
                }
            };
            let mut woke = false;
            for p in &ev.paths {
                if starts_with_ic(p, &data_dir) {
                    continue;
                }
                for (root, kind) in &map {
                    if !starts_with_ic(p, root) {
                        continue;
                    }
                    // Файловый интерфейс пространства (_bridge) пишет сам узел — его события не считаем
                    if let Ok(rel) = p.strip_prefix(root) {
                        if rel.components().next().map(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case(bridge_core::SERVICE_DIR)).unwrap_or(false) {
                            if matches!(kind, Kind::Share(_)) && rel.to_string_lossy().contains("outbox") {
                                woke = true;
                            }
                            continue;
                        }
                    }
                    match kind {
                        Kind::Share(i) => {
                            if let Some(rt) = node.shares.read().get(*i) {
                                rt.files_dirty.store(true, Ordering::SeqCst);
                            }
                        }
                        Kind::Outgoing => node.mailbox.dirty.store(true, Ordering::SeqCst),
                        Kind::Own => {
                            node.folders.invalidate_own();
                            node.folders.own_dirty.store(true, Ordering::SeqCst);
                        }
                        Kind::Mounts => {
                            node.folders.mounts_dirty.store(true, Ordering::SeqCst);
                            node.folders.mounts_touched.store(true, Ordering::SeqCst);
                        }
                    }
                    woke = true;
                }
            }
            if woke {
                *node.last_fs_event.lock() = std::time::Instant::now();
                node.wake.notify_one();
            }
        };
        let mut w = match notify::recommended_watcher(handler) {
            Ok(w) => w,
            Err(e) => {
                node.info(&format!("Слежение за папками недоступно ({}): буду просматривать их раз в 5 секунд", e));
                return None;
            }
        };
        let mut roots = Vec::new();
        for (root, _) in &targets {
            let _ = std::fs::create_dir_all(root);
            match w.watch(root, RecursiveMode::Recursive) {
                Ok(()) => roots.push(root.clone()),
                Err(e) => node.info(&format!("Не слежу за {}: {} — просматриваю по таймеру", root.display(), e)),
            }
        }
        Some(Watcher { _inner: w, roots })
    }
}
