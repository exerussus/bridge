//! Связь мостов через свой сервер (3.20): когда напрямую, через Radmin и по своей сети (10.77) сосед не отвечает.
//!
//! Оба моста держат исходящее соединение с сервером (WebSocket `LINK_CTL_PATH`, подпись ключом участника) — входящие
//! на ПК не нужны: ни прав администратора, ни брандмауэра, ни адаптера. Чтобы связаться с соседом, мост открывает
//! `LINK_DIAL_PATH?to=<участник>`; сервер сообщает соседу по его каналу `{"t":"open","id","from"}`, тот приходит на
//! `LINK_ACCEPT_PATH?id=…`, и сервер склеивает два WebSocket. По склеенному каналу идёт обычный HTTP моста.
//!
//! Шифрование — от моста до моста, ключом пары соседей (тем же, что подписывает запросы): сервер видит только объём.
//! Рукопожатие: звонящий шлёт `[1][длина kid][kid][nonce_a 32]`, отвечающий — `[1][nonce_b 32]`; ключи направлений —
//! HKDF-SHA256(секрет пары, соль nonce_a‖nonce_b). Дальше каждое сообщение WebSocket — ChaCha20-Poly1305 со счётчиком
//! в nonce (свой в каждую сторону). Подменить, переставить или повторить сообщение нельзя — канал рвётся.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub const LINK_CTL_PATH: &str = "/relay/v1/link/ctl";
pub const LINK_DIAL_PATH: &str = "/relay/v1/link/dial";
pub const LINK_ACCEPT_PATH: &str = "/relay/v1/link/accept";
/// Сколько сервер ждёт, пока вызванный мост придёт за соединением.
pub const ACCEPT_WAIT_SECS: u64 = 15;
/// Наибольшее сообщение канала (открытый текст).
pub const CHUNK: usize = 32 * 1024;

/// Событие канала управления (сервер → мост).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CtlEvent {
    /// "open" — к вам звонят; "ping" — проверка канала.
    pub t: String,
    pub id: String,
    /// Участник сервера, который звонит.
    pub from: String,
}

/// Как до этого моста достучаться через сервер (в привете соседям).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LinkInfo {
    /// Адрес сервера, как он записан в настройках (`хост:порт`).
    pub server: String,
    /// Имя этого моста у сервера.
    pub member: String,
}

/// Хост сервера без порта — чтобы сравнить «тот же ли сервер» у двух мостов.
pub fn same_server(a: &str, b: &str) -> bool {
    let h = |s: &str| crate::relay::split_addr(s).map(|(h, p)| (h.to_lowercase(), p)).ok();
    match (h(a), h(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

pub fn hello_dial(kid: &str, nonce_a: &[u8; 32]) -> Vec<u8> {
    let mut v = vec![1u8, kid.len().min(255) as u8];
    v.extend_from_slice(&kid.as_bytes()[..kid.len().min(255)]);
    v.extend_from_slice(nonce_a);
    v
}

pub fn parse_hello_dial(b: &[u8]) -> Option<(String, [u8; 32])> {
    if b.len() < 2 || b[0] != 1 {
        return None;
    }
    let n = b[1] as usize;
    if b.len() != 2 + n + 32 {
        return None;
    }
    let kid = std::str::from_utf8(&b[2..2 + n]).ok()?.to_string();
    Some((kid, b[2 + n..].try_into().ok()?))
}

pub fn hello_accept(nonce_b: &[u8; 32]) -> Vec<u8> {
    let mut v = vec![1u8];
    v.extend_from_slice(nonce_b);
    v
}

pub fn parse_hello_accept(b: &[u8]) -> Option<[u8; 32]> {
    if b.len() != 33 || b[0] != 1 {
        return None;
    }
    b[1..].try_into().ok()
}

/// Шифры одного канала: в свою сторону и в обратную.
pub struct StreamCrypto {
    out: ChaCha20Poly1305,
    inp: ChaCha20Poly1305,
    out_n: u64,
    inp_n: u64,
}

fn key(secret: &str, a: &[u8; 32], b: &[u8; 32], info: &[u8]) -> ChaCha20Poly1305 {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(a);
    salt[32..].copy_from_slice(b);
    let hk = Hkdf::<Sha256>::new(Some(&salt), secret.as_bytes());
    let mut k = [0u8; 32];
    hk.expand(info, &mut k).expect("hkdf");
    ChaCha20Poly1305::new(Key::from_slice(&k))
}

fn nonce(n: u64) -> Nonce {
    let mut b = [0u8; 12];
    b[4..].copy_from_slice(&n.to_be_bytes());
    *Nonce::from_slice(&b)
}

impl StreamCrypto {
    /// `dialer` — эта сторона звонила.
    pub fn new(secret: &str, nonce_a: &[u8; 32], nonce_b: &[u8; 32], dialer: bool) -> StreamCrypto {
        let d2a = key(secret, nonce_a, nonce_b, b"most-link/1 dial->accept");
        let a2d = key(secret, nonce_a, nonce_b, b"most-link/1 accept->dial");
        let (out, inp) = if dialer { (d2a, a2d) } else { (a2d, d2a) };
        StreamCrypto { out, inp, out_n: 0, inp_n: 0 }
    }

    pub fn seal(&mut self, data: &[u8]) -> Vec<u8> {
        let n = self.out_n;
        self.out_n += 1;
        self.out.encrypt(&nonce(n), data).expect("seal")
    }

    /// Сообщения идут строго по порядку (WebSocket поверх TCP): ждём ровно следующий номер.
    pub fn open(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        let p = self.inp.decrypt(&nonce(self.inp_n), data).ok()?;
        self.inp_n += 1;
        Some(p)
    }

    /// Разделить на две половины — для двух потоков (чтение и запись).
    pub fn split(self) -> (Sealer, Opener) {
        (Sealer { c: self.out, n: self.out_n }, Opener { c: self.inp, n: self.inp_n })
    }
}

pub struct Sealer {
    c: ChaCha20Poly1305,
    n: u64,
}

impl Sealer {
    pub fn seal(&mut self, data: &[u8]) -> Vec<u8> {
        let n = self.n;
        self.n += 1;
        self.c.encrypt(&nonce(n), data).expect("seal")
    }
}

pub struct Opener {
    c: ChaCha20Poly1305,
    n: u64,
}

impl Opener {
    pub fn open(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        let p = self.c.decrypt(&nonce(self.n), data).ok()?;
        self.n += 1;
        Some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_and_stream() {
        let (a, b): ([u8; 32], [u8; 32]) = (rand::random(), rand::random());
        let h = hello_dial("kid-123", &a);
        assert_eq!(parse_hello_dial(&h), Some(("kid-123".to_string(), a)));
        assert_eq!(parse_hello_accept(&hello_accept(&b)), Some(b));
        let mut d = StreamCrypto::new("BRS-secret", &a, &b, true);
        let mut x = StreamCrypto::new("BRS-secret", &a, &b, false);
        let m1 = d.seal(b"GET /hello");
        let m2 = d.seal(b"second");
        assert_eq!(x.open(&m1).as_deref(), Some(&b"GET /hello"[..]));
        // повтор и перестановка не проходят
        assert!(x.open(&m1).is_none());
        assert_eq!(x.open(&m2).as_deref(), Some(&b"second"[..]));
        assert_eq!(d.open(&x.seal(b"200 OK")).as_deref(), Some(&b"200 OK"[..]));
        // чужой ключ пары не расшифрует
        let mut evil = StreamCrypto::new("BRS-other", &a, &b, false);
        assert!(evil.open(&d.seal(b"x")).is_none());
        // своё же сообщение в обратную сторону не принимается (разные ключи направлений)
        let mut d2 = StreamCrypto::new("BRS-secret", &a, &b, true);
        let own = d2.seal(b"loop");
        let mut d3 = StreamCrypto::new("BRS-secret", &a, &b, true);
        assert!(d3.open(&own).is_none());
        assert!(same_server("82.114.229.113:443", "82.114.229.113") && !same_server("1.2.3.4:443", "82.114.229.113:443"));
    }
}
