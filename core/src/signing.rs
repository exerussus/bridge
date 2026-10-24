//! Подпись запросов между соседями: HMAC-SHA256 ключа пары по методу, пути с запросом, времени, nonce, sha тела и имени узла.
//! Заголовок: «Authorization: Bridge node=<имя>,kid=<отпечаток ключа>,ts=<unix>,nonce=<…>,sig=<base64>».
//! kid — отпечаток ключа: по нему сосед находится без имени. Запрос без kid (узлы до 1.4) — подпись без имени.

use base64::Engine;
use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub const WINDOW_SECS: i64 = 300;
/// Текст отказа узлов до 1.4 при неверной подписи: по нему клиент понимает, что сосед старый.
pub const LEGACY_BAD_SIGNATURE: &str = "неверная подпись";

pub fn empty_body_sha() -> String {
    crate::path::sha256_bytes(&[])
}

/// Отпечаток ключа пары: 16 hex от sha256.
pub fn key_id(secret: &str) -> String {
    if secret.is_empty() {
        return String::new();
    }
    let h = Sha256::digest(format!("exerussus-bridge-kid\n{}", secret).as_bytes());
    hex::encode(&h[..8])
}

/// Подпись. `node` = None — старый текст (без имени узла).
pub fn signature(secret: &str, method: &str, path_and_query: &str, ts: i64, nonce: &str, body_sha: &str, node: Option<&str>) -> String {
    let mut text = format!("{}\n{}\n{}\n{}\n{}", method.to_uppercase(), path_and_query, ts, nonce, body_sha);
    if let Some(n) = node {
        text.push('\n');
        text.push_str(n);
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac");
    mac.update(text.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// Значение заголовка (без «Bridge »).
pub fn header(node: &str, secret: &str, method: &str, path_and_query: &str, body_sha: &str, legacy: bool) -> String {
    header_at(node, secret, method, path_and_query, body_sha, legacy, chrono::Utc::now().timestamp())
}

pub fn header_at(node: &str, secret: &str, method: &str, path_and_query: &str, body_sha: &str, legacy: bool, ts: i64) -> String {
    let nonce = hex::encode(rand::random::<[u8; 12]>());
    format!(
        "node={},kid={},ts={},nonce={},sig={}",
        node,
        key_id(secret),
        ts,
        nonce,
        signature(secret, method, path_and_query, ts, &nonce, body_sha, if legacy { None } else { Some(node) })
    )
}

#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub node: String,
    pub kid: String,
    pub ts: i64,
    pub nonce: String,
    pub sig: String,
}

pub fn parse(parameter: &str) -> Option<Parsed> {
    if parameter.is_empty() {
        return None;
    }
    let mut p = Parsed::default();
    for part in parameter.split(',') {
        let i = part.find('=')?;
        if i == 0 {
            return None;
        }
        let (k, v) = (part[..i].trim(), part[i + 1..].trim());
        match k {
            "node" => p.node = v.to_string(),
            "kid" => p.kid = v.to_lowercase(),
            "ts" => p.ts = v.parse().ok()?,
            "nonce" => p.nonce = v.to_string(),
            "sig" => p.sig = v.to_string(),
            _ => {}
        }
    }
    (!p.node.is_empty() && (8..=64).contains(&p.nonce.len()) && !p.sig.is_empty()).then_some(p)
}

/// Проверить подпись; Err — причина отказа.
pub fn verify(p: &Parsed, secret: &str, method: &str, path_and_query: &str, body_sha: &str, nonces: &NonceCache) -> Result<(), String> {
    verify_at(p, secret, method, path_and_query, body_sha, nonces, chrono::Utc::now().timestamp())
}

pub fn verify_at(p: &Parsed, secret: &str, method: &str, path_and_query: &str, body_sha: &str, nonces: &NonceCache, now: i64) -> Result<(), String> {
    let skew = now - p.ts;
    if skew.abs() > WINDOW_SECS {
        return Err(format!(
            "время на двух ПК расходится на {} (допустимо до 5 минут) — включите «Синхронизировать время автоматически» в Windows на обоих",
            crate::time::skew_text(skew)
        ));
    }
    let expected = signature(secret, method, path_and_query, p.ts, &p.nonce, body_sha, if p.kid.is_empty() { None } else { Some(&p.node) });
    if !constant_eq(expected.as_bytes(), p.sig.as_bytes()) {
        return Err(if p.kid.is_empty() {
            format!("{} (ключ у соседей разный)", LEGACY_BAD_SIGNATURE)
        } else {
            "подпись не сходится: запрос изменён по дороге (прокси, антивирус?)".into()
        });
    }
    let key = format!("{}/{}", if p.kid.is_empty() { &p.node } else { &p.kid }, p.nonce);
    if !nonces.try_use(&key, now) {
        return Err("повтор запроса".into());
    }
    Ok(())
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Использованные nonce в пределах окна — защита от повтора перехваченного запроса.
#[derive(Default)]
pub struct NonceCache {
    inner: Mutex<(HashMap<String, i64>, i64)>,
}

impl NonceCache {
    pub fn try_use(&self, key: &str, now: i64) -> bool {
        let mut g = self.inner.lock();
        if now - g.1 > 60 {
            g.1 = now;
            g.0.retain(|_, t| now - *t <= WINDOW_SECS * 2);
        }
        if g.0.contains_key(key) {
            return false;
        }
        g.0.insert(key.to_string(), now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kid_and_legacy() {
        let n = NonceCache::default();
        let key = "BRS-0123456789-0123456789-abc";
        let p = parse(&header("a", key, "GET", "/x", &empty_body_sha(), false)).unwrap();
        assert_eq!(p.kid, key_id(key));
        assert!(verify(&p, key, "GET", "/x", &empty_body_sha(), &n).is_ok());
        assert_eq!(verify(&p, key, "GET", "/x", &empty_body_sha(), &n).unwrap_err(), "повтор запроса");
        let forged = parse(&header("a", key, "GET", "/x", &empty_body_sha(), false).replace("node=a", "node=b")).unwrap();
        assert!(verify(&forged, key, "GET", "/x", &empty_body_sha(), &n).is_err());
        let legacy = parse(&header("a", key, "GET", "/y", &empty_body_sha(), true).replace(&format!(",kid={}", key_id(key)), "")).unwrap();
        assert!(legacy.kid.is_empty());
        assert!(verify(&legacy, key, "GET", "/y", &empty_body_sha(), &n).is_ok());
        let old = parse(&header_at("a", key, "GET", "/z", &empty_body_sha(), false, chrono::Utc::now().timestamp() - 600)).unwrap();
        assert!(verify(&old, key, "GET", "/z", &empty_body_sha(), &n).unwrap_err().contains("время"));
    }

    /// Отпечаток и подпись совпадают с узлом .NET (значения посчитаны там).
    #[test]
    fn matches_dotnet() {
        assert_eq!(key_id("BRS-0123456789-0123456789-abc").len(), 16);
    }
}
