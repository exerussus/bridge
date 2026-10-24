//! Ручная отправка. «отправка/<сосед>/…» → сосед забирает в «приём/<мы>/…» → подтверждает → у нас файл удаляется.
//! Папка соседа — по имени или по одному из его IP.

use crate::node::Node;
use bridge_core::config::{NodeConfig, PeerConfig};
use bridge_core::proto::{FileEntry, MailboxAck, MailboxEvent};
use bridge_core::scanner::walk;
use bridge_core::{path, time};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct Mailbox {
    sha_cache: Mutex<HashMap<PathBuf, (i64, i64, String)>>,
    history: Mutex<Vec<MailboxEvent>>,
    gate: tokio::sync::Mutex<()>,
    signature: Mutex<String>,
    blocked: Mutex<HashSet<String>>,
    pub wake: AtomicBool,
    pub dirty: AtomicBool,
}

impl Default for Mailbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Mailbox {
    pub fn new() -> Self {
        Mailbox {
            sha_cache: Mutex::new(HashMap::new()),
            history: Mutex::new(Vec::new()),
            gate: tokio::sync::Mutex::new(()),
            signature: Mutex::new(String::new()),
            blocked: Mutex::new(HashSet::new()),
            wake: AtomicBool::new(false),
            dirty: AtomicBool::new(true),
        }
    }

    pub fn outgoing_root(&self, node: &Node, cfg: &NodeConfig) -> PathBuf {
        node.resolve(&cfg.mailbox.outgoing)
    }

    pub fn out_root(&self, node: &Node) -> PathBuf {
        let c = node.cfg.read();
        node.resolve(&c.mailbox.outgoing)
    }

    pub fn in_root(&self, node: &Node) -> PathBuf {
        let c = node.cfg.read();
        node.resolve(&c.mailbox.incoming)
    }

    pub fn history(&self) -> Vec<MailboxEvent> {
        self.history.lock().clone()
    }

    pub fn remember(&self, node: &Node, e: MailboxEvent) {
        let line = format!(
            "[отправка] {}{}{}",
            match e.kind.as_str() {
                "sent" => format!("доставлено {}: ", e.peer),
                "received" => format!("получено от {}: ", e.peer),
                _ => format!("{}: ", e.peer),
            },
            e.path,
            if e.message.is_empty() { String::new() } else { format!(" — {}", e.message) }
        );
        node.info(&line);
        node.emit(&format!("mailbox-{}", e.kind), serde_json::to_value(&e).unwrap_or_default());
        let mut h = self.history.lock();
        h.push(e);
        let n = h.len();
        if n > 200 {
            h.drain(0..n - 200);
        }
    }

    /// Создать папки соседей и памятку.
    pub fn prepare(&self, node: &Node) {
        let cfg = node.cfg.read().clone();
        if !cfg.mailbox.enabled {
            return;
        }
        let out = node.resolve(&cfg.mailbox.outgoing);
        let _ = std::fs::create_dir_all(&out);
        let _ = std::fs::create_dir_all(node.resolve(&cfg.mailbox.incoming));
        for p in cfg.peers.iter().filter(|p| p.enabled && !p.pending) {
            let _ = std::fs::create_dir_all(out.join(&p.name));
        }
        let readme = out.join("_КАК-ОТПРАВИТЬ.txt");
        if !readme.exists() {
            let text = format!(
                "Положите файл или папку в папку соседа (по имени или IP) — узел отправит сам.\r\nКогда сосед подтвердит приём, файл здесь удалится. У соседа он появится в «{}\\{}\\».\r\nСоседи: {}\r\n",
                cfg.mailbox.incoming,
                cfg.node,
                cfg.peers.iter().map(|p| format!("{} ({})", p.name, p.ips.join(", "))).collect::<Vec<_>>().join(", ")
            );
            let _ = std::fs::write(readme, text);
        }
    }

    fn dirs_for(&self, node: &Node, peer: &PeerConfig) -> Vec<PathBuf> {
        let root = self.out_root(node);
        std::iter::once(peer.name.clone()).chain(peer.ips.iter().cloned()).map(|n| root.join(n)).filter(|d| d.is_dir()).collect()
    }

    /// Что ждёт отправки этому соседу.
    pub fn pending(&self, node: &Node, peer: &PeerConfig) -> Vec<(FileEntry, PathBuf)> {
        let mut list = Vec::new();
        let mut seen = HashSet::new();
        let ig = node.cfg.read().global_ignore();
        let settle = (node.settle_secs * 10_000_000.0) as i64;
        let now = time::to_ticks(&time::now());
        for dir in self.dirs_for(node, peer) {
            for w in walk(&dir, &[]) {
                let Some(p) = path::norm(&w.rel) else { continue };
                if path::is_junk(&p) || ig.is_ignored(&p) || !seen.insert(p.to_lowercase()) {
                    continue;
                }
                if now - w.mtime_ticks < settle {
                    continue;
                }
                let cached = self.sha_cache.lock().get(&w.full).filter(|c| c.0 == w.size && c.1 == w.mtime_ticks).map(|c| c.2.clone());
                let sha = match cached {
                    Some(s) => s,
                    None => match path::sha256_file_settled(&w.full) {
                        Ok(s) => {
                            self.sha_cache.lock().insert(w.full.clone(), (w.size, w.mtime_ticks, s.clone()));
                            s
                        }
                        Err(_) => continue,
                    },
                };
                list.push((FileEntry { path: p, sha256: sha, size: w.size, modified: time::from_ticks(w.mtime_ticks), rev: 0 }, w.full));
            }
        }
        list
    }

    /// Сосед подтвердил приём — удалить файл, если он не менялся, и пустые папки над ним.
    pub fn acknowledge(&self, node: &Node, peer: &PeerConfig, ack: &MailboxAck) -> bool {
        let items = self.pending(node, peer);
        let Some((entry, full)) = items.into_iter().find(|(e, _)| bridge_core::eq_ic(&e.path, &ack.path)) else { return false };
        if !entry.sha256.eq_ignore_ascii_case(&ack.sha256) {
            return false;
        }
        if let Err(e) = std::fs::remove_file(&full) {
            self.remember(node, MailboxEvent { kind: "error".into(), peer: peer.name.clone(), path: ack.path.clone(), message: format!("доставлено, но не удалось удалить: {}", e), ..Default::default() });
            return true;
        }
        self.sha_cache.lock().remove(&full);
        if let Some(root) = self.dirs_for(node, peer).into_iter().find(|d| full.starts_with(d)) {
            remove_empty_dirs(&root, full.parent().unwrap_or(&root));
        }
        self.remember(node, MailboxEvent { kind: "sent".into(), peer: peer.name.clone(), path: ack.path.clone(), size: entry.size, ..Default::default() });
        true
    }

    /// Изменились ли исходящие.
    pub fn outgoing_changed(&self, node: &Node) -> bool {
        let root = self.out_root(node);
        if !node.cfg.read().mailbox.enabled || !root.is_dir() {
            return false;
        }
        let (mut n, mut size, mut ticks) = (0i64, 0i64, 0i64);
        for w in walk(&root, &[]) {
            n += 1;
            size += w.size;
            ticks ^= w.mtime_ticks;
        }
        let sig = format!("{}|{}|{}", n, size, ticks);
        let mut g = self.signature.lock();
        if *g == sig {
            return false;
        }
        *g = sig;
        n > 0
    }

    /// Забрать у соседей всё, что они нам отправили, и подтвердить.
    pub async fn round(&self, node: &Arc<Node>) {
        let Ok(_g) = self.gate.try_lock() else { return };
        self.wake.store(false, Ordering::SeqCst);
        let cfg = node.cfg.read().clone();
        if !cfg.mailbox.enabled {
            return;
        }
        let in_root = node.resolve(&cfg.mailbox.incoming);
        let ig = cfg.global_ignore();
        for peer in cfg.peers.iter().filter(|p| p.enabled && p.has_url() && cfg.mailbox.accepts(&p.name)) {
            if !node.peer_state(&peer.name).online && node.peer_state(&peer.name).last_seen.is_some() {
                continue; // сосед недоступен — не стучимся лишний раз
            }
            let client = node.client_for(peer);
            let list = match client.mailbox_list().await {
                Ok(l) => l,
                Err(e) => {
                    if !matches!(e.status(), Some(404) | None) {
                        self.remember(node, MailboxEvent { kind: "error".into(), peer: peer.name.clone(), message: e.to_string(), ..Default::default() });
                    }
                    continue;
                }
            };
            for f in list.files {
                let Some(p) = path::norm(&f.path) else { continue };
                if let Some(rule) = ig.which(&p) {
                    if self.blocked.lock().insert(format!("{}/{}/{}", peer.name, f.path, f.sha256)) {
                        self.remember(node, MailboxEvent { kind: "error".into(), peer: peer.name.clone(), path: f.path.clone(), message: format!("не принят: в игноре («{}»)", rule), ..Default::default() });
                    }
                    continue;
                }
                let Ok(mut dest) = path::to_local(&in_root.join(&peer.name), &p) else { continue };
                let ack = MailboxAck { path: f.path.clone(), sha256: f.sha256.clone() };
                if dest.is_file() && path::sha256_file(&dest).map(|s| s.eq_ignore_ascii_case(&f.sha256)).unwrap_or(false) {
                    let _ = client.mailbox_ack(&ack).await; // уже есть — просто подтвердить
                    continue;
                }
                let tmp = node.tmp_file();
                match client.mailbox_download(&f.path, &tmp, node.max_file_bytes()).await {
                    Ok(sha) if sha.eq_ignore_ascii_case(&f.sha256) => {
                        if let Some(d) = dest.parent() {
                            let _ = std::fs::create_dir_all(d);
                        }
                        if dest.exists() {
                            dest = free_name(&dest); // одноимённый другой файл не затираем
                        }
                        if let Err(e) = bridge_core::files::move_replace(&tmp, &dest) {
                            self.remember(node, MailboxEvent { kind: "error".into(), peer: peer.name.clone(), path: f.path.clone(), message: e.to_string(), ..Default::default() });
                            continue;
                        }
                        let _ = client.mailbox_ack(&ack).await;
                        let rel = path::relative(&in_root, &dest).unwrap_or(f.path.clone());
                        self.remember(node, MailboxEvent { kind: "received".into(), peer: peer.name.clone(), path: rel, size: f.size, ..Default::default() });
                        // сборка (3.3.1) — распаковать
                        if let Some(r) = path::relative(&in_root.join(&peer.name), &dest) {
                            crate::builds::on_received(node, &peer.name, &dest, &r);
                        }
                    }
                    Ok(_) => {
                        let _ = std::fs::remove_file(&tmp);
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp);
                        if e.status() != Some(404) {
                            self.remember(node, MailboxEvent { kind: "error".into(), peer: peer.name.clone(), path: f.path.clone(), message: e.to_string(), ..Default::default() });
                        }
                    }
                }
            }
        }
    }

    /// Сводка: сколько ждёт отправки каждому соседу, последние события.
    pub fn status(&self, node: &Node) -> serde_json::Value {
        let cfg = node.cfg.read().clone();
        let ig = cfg.global_ignore();
        let mut pending = serde_json::Map::new();
        for p in cfg.peers.iter().filter(|p| p.enabled) {
            let (mut files, mut bytes) = (0i64, 0i64);
            for d in self.dirs_for(node, p) {
                for w in walk(&d, &[]) {
                    if path::is_junk(&w.rel) || ig.is_ignored(&w.rel) {
                        continue;
                    }
                    files += 1;
                    bytes += w.size;
                }
            }
            pending.insert(p.name.clone(), serde_json::json!({ "files": files, "bytes": bytes }));
        }
        let mut recent = self.history();
        recent.reverse();
        recent.truncate(30);
        serde_json::json!({
            "enabled": cfg.mailbox.enabled,
            "outgoing": node.resolve(&cfg.mailbox.outgoing),
            "incoming": node.resolve(&cfg.mailbox.incoming),
            "pending": pending,
            "recent": recent,
        })
    }
}

pub fn free_name(p: &Path) -> PathBuf {
    let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("файл").to_string();
    let ext = p.extension().and_then(|s| s.to_str()).map(|e| format!(".{}", e)).unwrap_or_default();
    let mut i = 2;
    loop {
        let c = dir.join(format!("{} ({}){}", stem, i, ext));
        if !c.exists() {
            return c;
        }
        i += 1;
    }
}

/// Удалить пустые папки от `dir` вверх до `root` (не включая).
pub fn remove_empty_dirs(root: &Path, dir: &Path) {
    let mut d = dir.to_path_buf();
    while d.starts_with(root) && d != root {
        if std::fs::read_dir(&d).map(|mut x| x.next().is_none()).unwrap_or(false) {
            if std::fs::remove_dir(&d).is_err() {
                break;
            }
        } else {
            break;
        }
        match d.parent() {
            Some(p) => d = p.to_path_buf(),
            None => break,
        }
    }
}
