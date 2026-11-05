//! Канал администратора (мастер-ключ): WebSocket `/relay/v1/admin`, всё внутри зашифровано.
//!
//! Рукопожатие (текстом, JSON):
//!   клиент → `{v:1, key, ts, nonce, sig}`, sig = HMAC-SHA256(мастер, "most-relay admin c\n{key}\n{ts}\n{nonce}");
//!   сервер → `{key, sig}`, sig = HMAC-SHA256(мастер, "most-relay admin s\n{ключ клиента}\n{key}").
//! key — одноразовые X25519. Подпись привязывает их к мастер-ключу: без него не подменить ключи и не прочитать канал.
//! Ключи шифрования: HKDF-SHA256(соль = мастер, общий секрет) → 64 байта: клиент→сервер, сервер→клиент.
//! Дальше — двоичные кадры ChaCha20-Poly1305, nonce = номер кадра (у каждой стороны свой счётчик, строго по порядку).
//! Внутри — JSON: запрос `{id, op, args}`, ответ `{re, ok, data | error}`, событие `{ev, …}`.

use base64::Engine;
use chacha20poly1305::aead::Aead;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub const ADMIN_PATH: &str = "/relay/v1/admin";
pub const WINDOW_SECS: i64 = 300;

/// Мастер-ключ: «MRM-» и 64 hex (256 бит).
pub fn new_master() -> String {
    format!("MRM-{}", hex::encode(rand::random::<[u8; 32]>()).to_uppercase())
}

pub fn is_master(s: &str) -> bool {
    let s = s.trim();
    s.len() == 68 && s[..4].eq_ignore_ascii_case("MRM-") && s[4..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Сравнение без утечки по времени.
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_uppercase(), b.trim().to_uppercase());
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn mac(master: &str, text: &str) -> String {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(master.trim().to_uppercase().as_bytes()).expect("hmac");
    m.update(text.as_bytes());
    b64().encode(m.finalize().into_bytes())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientHello {
    pub v: u32,
    pub key: String,
    pub ts: i64,
    pub nonce: String,
    pub sig: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerHello {
    pub key: String,
    pub sig: String,
    /// Отказ (рукопожатие не прошло) — текст для человека.
    pub error: String,
}

/// Одна сторона шифрованного канала: свой ключ и счётчик кадров на отправку, ключ и счётчик собеседника на приём.
pub struct Channel {
    out: ChaCha20Poly1305,
    out_n: u64,
    inp: ChaCha20Poly1305,
    in_n: u64,
}

fn nonce(n: u64) -> chacha20poly1305::Nonce {
    let mut b = [0u8; 12];
    b[4..].copy_from_slice(&n.to_be_bytes());
    b.into()
}

impl Channel {
    fn derive(master: &str, shared: &[u8; 32], client: bool) -> Channel {
        let hk = hkdf::Hkdf::<Sha256>::new(Some(master.trim().to_uppercase().as_bytes()), shared);
        let mut k = [0u8; 64];
        hk.expand(b"most-relay admin v1", &mut k).expect("hkdf");
        let c2s = ChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&k[..32]));
        let s2c = ChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&k[32..]));
        if client {
            Channel { out: c2s, out_n: 0, inp: s2c, in_n: 0 }
        } else {
            Channel { out: s2c, out_n: 0, inp: c2s, in_n: 0 }
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let ct = self.out.encrypt(&nonce(self.out_n), plain).expect("seal");
        self.out_n += 1;
        ct
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn open(&mut self, ct: &[u8]) -> Result<Vec<u8>, String> {
        let p = self.inp.decrypt(&nonce(self.in_n), ct).map_err(|_| "кадр не расшифровался — канал подменён или сбился".to_string())?;
        self.in_n += 1;
        Ok(p)
    }

    /// Разделить на отправку и приём (их держат разные задачи).
    pub fn split(self) -> (Sealer, Opener) {
        (Sealer { k: self.out, n: self.out_n }, Opener { k: self.inp, n: self.in_n })
    }
}

pub struct Sealer {
    k: ChaCha20Poly1305,
    n: u64,
}

impl Sealer {
    pub fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let ct = self.k.encrypt(&nonce(self.n), plain).expect("seal");
        self.n += 1;
        ct
    }
}

pub struct Opener {
    k: ChaCha20Poly1305,
    n: u64,
}

impl Opener {
    pub fn open(&mut self, ct: &[u8]) -> Result<Vec<u8>, String> {
        let p = self.k.decrypt(&nonce(self.n), ct).map_err(|_| "кадр не расшифровался — канал подменён или сбился".to_string())?;
        self.n += 1;
        Ok(p)
    }
}

/// Клиент: начать рукопожатие.
pub struct ClientStart {
    secret: x25519_dalek::EphemeralSecret,
    pub hello: ClientHello,
}

impl ClientStart {
    pub fn new(master: &str) -> ClientStart {
        let secret = x25519_dalek::EphemeralSecret::random_from_rng(rand::rngs::OsRng);
        let key = b64().encode(x25519_dalek::PublicKey::from(&secret).as_bytes());
        let ts = chrono::Utc::now().timestamp();
        let nonce = hex::encode(rand::random::<[u8; 16]>());
        let sig = mac(master, &format!("most-relay admin c\n{}\n{}\n{}", key, ts, nonce));
        ClientStart { secret, hello: ClientHello { v: 1, key, ts, nonce, sig } }
    }

    /// Проверить ответ сервера и получить канал.
    pub fn finish(self, master: &str, s: &ServerHello) -> Result<Channel, String> {
        if !s.error.is_empty() {
            return Err(s.error.clone());
        }
        let want = mac(master, &format!("most-relay admin s\n{}\n{}", self.hello.key, s.key));
        if !same(&want, &s.sig) {
            return Err("сервер не подтвердил мастер-ключ — это не тот сервер или ответ подменили".into());
        }
        let pk: [u8; 32] = b64().decode(&s.key).ok().and_then(|v| v.try_into().ok()).ok_or("сервер прислал неверный ключ")?;
        let shared = self.secret.diffie_hellman(&x25519_dalek::PublicKey::from(pk));
        Ok(Channel::derive(master, shared.as_bytes(), true))
    }
}

/// Сервер: проверить привет клиента (время, подпись; повтор nonce проверяет вызывающий) и ответить.
pub fn server_accept(master: &str, h: &ClientHello, now: i64) -> Result<(ServerHello, Channel), String> {
    if h.v != 1 {
        return Err("другая версия канала — обновите приложение «Сервер» или most-relay".into());
    }
    if (now - h.ts).abs() > WINDOW_SECS {
        return Err("часы на ПК и сервере расходятся больше чем на 5 минут — включите синхронизацию времени".into());
    }
    let want = mac(master, &format!("most-relay admin c\n{}\n{}\n{}", h.key, h.ts, h.nonce));
    if !same(&want, &h.sig) {
        return Err("мастер-ключ не подходит".into());
    }
    let pk: [u8; 32] = b64().decode(&h.key).ok().and_then(|v| v.try_into().ok()).ok_or("неверный ключ клиента")?;
    let mine = x25519_dalek::EphemeralSecret::random_from_rng(rand::rngs::OsRng);
    let key = b64().encode(x25519_dalek::PublicKey::from(&mine).as_bytes());
    let shared = mine.diffie_hellman(&x25519_dalek::PublicKey::from(pk));
    let sig = mac(master, &format!("most-relay admin s\n{}\n{}", h.key, key));
    Ok((ServerHello { key, sig, error: String::new() }, Channel::derive(master, shared.as_bytes(), false)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_and_frames() {
        let m = new_master();
        assert!(is_master(&m) && is_master(&m.to_lowercase()));
        let c = ClientStart::new(&m);
        let (sh, mut srv) = server_accept(&m, &c.hello, chrono::Utc::now().timestamp()).unwrap();
        let mut cli = c.finish(&m, &sh).unwrap();
        let x = cli.seal(b"hello");
        assert_eq!(srv.open(&x).unwrap(), b"hello");
        let y = srv.seal(b"world");
        assert_eq!(cli.open(&y).unwrap(), b"world");
        // повтор кадра не проходит
        assert!(srv.open(&x).is_err());
        // чужой мастер-ключ
        let c2 = ClientStart::new(&new_master());
        assert!(server_accept(&m, &c2.hello, chrono::Utc::now().timestamp()).is_err());
        // подмена ответа сервера
        let c3 = ClientStart::new(&m);
        let (mut sh3, _) = server_accept(&m, &c3.hello, chrono::Utc::now().timestamp()).unwrap();
        sh3.key = ClientStart::new(&m).hello.key;
        assert!(c3.finish(&m, &sh3).is_err());
    }
}
