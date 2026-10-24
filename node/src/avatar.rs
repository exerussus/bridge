//! Аватарки: своя картинка хранится у моста (bridge-data/avatar.img), её отпечаток уходит соседям в привете,
//! соседи забирают картинку один раз и держат у себя (bridge-data/avatars/<сосед>), пока отпечаток не сменится.

use crate::node::Node;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

/// До 2 МБ — чтобы влезла гифка (обычные картинки панель ужимает до 256×256 JPEG).
pub const MAX: usize = 2 * 1024 * 1024;

fn own_file(node: &Node) -> PathBuf {
    node.data_dir.join("avatar.img")
}

fn cache_file(node: &Node, peer: &str) -> PathBuf {
    node.data_dir.join("avatars").join(crate::mobile::folder_name(peer))
}

pub fn hash_of(b: &[u8]) -> String {
    hex::encode(&Sha256::digest(b)[..6])
}

/// Отпечаток своей аватарки (пусто — нет).
pub fn own_hash(node: &Node) -> String {
    std::fs::read(own_file(node)).map(|b| hash_of(&b)).unwrap_or_default()
}

pub fn own(node: &Node) -> Option<Vec<u8>> {
    std::fs::read(own_file(node)).ok()
}

/// Картинка: JPEG, PNG, WebP или GIF (по первым байтам).
pub fn mime_of(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else {
        None
    }
}

pub fn set(node: &Arc<Node>, b: &[u8]) -> Result<String, String> {
    if b.len() > MAX {
        return Err("картинка больше 2 МБ".into());
    }
    if mime_of(b).is_none() {
        return Err("нужна картинка: JPEG, PNG, WebP или GIF".into());
    }
    std::fs::write(own_file(node), b).map_err(|e| e.to_string())?;
    let h = hash_of(b);
    node.info("Аватарка обновлена — соседи увидят её при следующем обмене");
    node.notify_all("avatar");
    Ok(h)
}

pub fn remove(node: &Arc<Node>) {
    let _ = std::fs::remove_file(own_file(node));
    node.notify_all("avatar");
}

/// Аватарка соседа: из кэша, если отпечаток совпадает, иначе забрать у него.
pub async fn of_peer(node: &Arc<Node>, peer: &str) -> Option<Vec<u8>> {
    let want = node.peer_state(peer).avatar;
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
    let (code, b) = tokio::time::timeout(std::time::Duration::from_secs(10), client.get_bytes("/peer/avatar")).await.ok()?.ok()?;
    if code != 200 || b.len() > MAX || mime_of(&b).is_none() {
        return None;
    }
    let _ = std::fs::create_dir_all(f.parent()?);
    let _ = std::fs::write(&f, &b);
    Some(b)
}
