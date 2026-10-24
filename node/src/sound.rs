//! Звук входа в канал: свой короткий звук (bridge-data/joinsound.bin), его отпечаток уходит соседям в привете,
//! соседи забирают звук один раз и держат у себя (bridge-data/sounds/<сосед>). Когда вы входите в канал, панели
//! остальных участников проигрывают ваш звук.

use crate::node::Node;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

pub const MAX: usize = 1024 * 1024;

fn own_file(node: &Node) -> PathBuf {
    node.data_dir.join("joinsound.bin")
}

fn cache_file(node: &Node, peer: &str) -> PathBuf {
    node.data_dir.join("sounds").join(crate::mobile::folder_name(peer))
}

/// Звук: MP3, OGG/Opus, WAV, WebM, M4A (по первым байтам).
pub fn mime_of(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"ID3") || (b.len() > 2 && b[0] == 0xFF && (b[1] & 0xE0) == 0xE0) {
        Some("audio/mpeg")
    } else if b.starts_with(b"OggS") {
        Some("audio/ogg")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        Some("audio/wav")
    } else if b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        Some("audio/webm")
    } else if b.len() > 12 && &b[4..8] == b"ftyp" {
        Some("audio/mp4")
    } else if b.starts_with(b"fLaC") {
        Some("audio/flac")
    } else {
        None
    }
}

pub fn hash_of(b: &[u8]) -> String {
    hex::encode(&Sha256::digest(b)[..6])
}

pub fn own_hash(node: &Node) -> String {
    std::fs::read(own_file(node)).map(|b| hash_of(&b)).unwrap_or_default()
}

pub fn own(node: &Node) -> Option<Vec<u8>> {
    std::fs::read(own_file(node)).ok()
}

pub fn set(node: &Arc<Node>, b: &[u8]) -> Result<String, String> {
    if b.len() > MAX {
        return Err("звук больше 1 МБ — нужен короткий, на пару секунд".into());
    }
    if mime_of(b).is_none() {
        return Err("нужен звук: MP3, OGG, WAV, WebM или M4A".into());
    }
    std::fs::write(own_file(node), b).map_err(|e| e.to_string())?;
    node.info("Звук входа обновлён — соседи услышат его, когда вы войдёте в канал");
    node.notify_all("avatar");
    Ok(hash_of(b))
}

pub fn remove(node: &Arc<Node>) {
    let _ = std::fs::remove_file(own_file(node));
    node.notify_all("avatar");
}

/// Звук соседа: из кэша, если отпечаток совпадает, иначе забрать у него.
pub async fn of_peer(node: &Arc<Node>, peer: &str) -> Option<Vec<u8>> {
    let want = node.peer_state(peer).sound;
    if want.is_empty() {
        return None;
    }
    let f = cache_file(node, peer);
    if let Ok(b) = std::fs::read(&f) {
        if hash_of(&b) == want {
            return Some(b);
        }
    }
    let p = node.cfg.read().peer(peer).cloned()?;
    let client = node.client_for(&p);
    let (code, b) = tokio::time::timeout(std::time::Duration::from_secs(10), client.get_bytes("/peer/joinsound")).await.ok()?.ok()?;
    if code != 200 || b.len() > MAX || mime_of(&b).is_none() {
        return None;
    }
    let _ = std::fs::create_dir_all(f.parent()?);
    let _ = std::fs::write(&f, &b);
    Some(b)
}

// ───────────── свой звук пинга: его слышите только вы, когда пингуют вас ─────────────

pub const PING_MAX: usize = 300 * 1024;

fn ping_file(node: &Node) -> PathBuf {
    node.data_dir.join("pingsound.bin")
}

pub fn ping_own(node: &Node) -> Option<Vec<u8>> {
    std::fs::read(ping_file(node)).ok()
}

pub fn ping_set(node: &Node, b: &[u8]) -> Result<String, String> {
    if b.len() > PING_MAX {
        return Err("звук пинга больше 300 КБ — нужен короткий".into());
    }
    if mime_of(b).is_none() {
        return Err("нужен звук: MP3, OGG, WAV, WebM или M4A".into());
    }
    std::fs::write(ping_file(node), b).map_err(|e| e.to_string())?;
    Ok(hash_of(b))
}

pub fn ping_remove(node: &Node) {
    let _ = std::fs::remove_file(ping_file(node));
}

pub fn ping_hash(node: &Node) -> String {
    ping_own(node).map(|b| hash_of(&b)).unwrap_or_default()
}

/// Звук пинга соседа (его слышит тот, кто пингует): из кэша, если отпечаток совпадает, иначе — у соседа.
pub async fn ping_of_peer(node: &Arc<Node>, peer: &str) -> Option<Vec<u8>> {
    let want = node.peer_state(peer).ping;
    if want.is_empty() {
        return None;
    }
    let f = node.data_dir.join("pings").join(crate::mobile::folder_name(peer));
    if let Ok(b) = std::fs::read(&f) {
        if hash_of(&b) == want {
            return Some(b);
        }
    }
    let p = node.cfg.read().peer(peer).cloned()?;
    let client = node.client_for(&p);
    let (code, b) = tokio::time::timeout(std::time::Duration::from_secs(10), client.get_bytes("/peer/pingsound")).await.ok()?.ok()?;
    if code != 200 || b.len() > PING_MAX || mime_of(&b).is_none() {
        return None;
    }
    let _ = std::fs::create_dir_all(f.parent()?);
    let _ = std::fs::write(&f, &b);
    Some(b)
}
