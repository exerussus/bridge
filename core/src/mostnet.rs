//! Своя локальная сеть MostNet (3.17): общее для сервера (`most-relay`), службы на ПК (`most-net`) и моста.
//!
//! Каждый ПК — участник своего сервера с постоянным адресом 10.77.x.y. Служба `most-net` получает адрес и список
//! соседей по сети (`POST /relay/v1/net/join`, `GET /relay/v1/net/peers` — подпись ключом участника, как у моста),
//! ловит IP-пакеты своего адаптера, шифрует их для соседа и отдаёт серверу; сервер пересылает по адресу.
//!
//! Кадры до сервера (UDP `NET_PORT` или WebSocket `/relay/v1/net/ws` на том же порту, что HTTP):
//!   ПК → сервер: `[1][тип][токен 16][кому 4][данные]`; тип 1 — пакет соседу, 2 — «я здесь» (данные — 8 байт метки).
//!   сервер → ПК: `[1][тип][от кого 4][данные]`; тип 1 — пакет от соседа, 2 — ответ на «я здесь» (та же метка).
//!
//! Данные пакета соседу зашифрованы от ПК до ПК: `[запуск 16][номер 8][ChaCha20-Poly1305]`. Ключ — HKDF от X25519
//! постоянных ключей пары, номера запуска отправителя и направления; номер — счётчик, повторы отсекает окно (2048).
//! Сервер видит только «от какого адреса к какому» и размер.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::Ipv4Addr;

pub const JOIN_PATH: &str = "/relay/v1/net/join";
pub const PEERS_PATH: &str = "/relay/v1/net/peers";
pub const WS_PATH: &str = "/relay/v1/net/ws";
pub const NET_PORT: u16 = 3479;
/// Сеть 10.77.0.0/16.
pub const NET_BASE: [u8; 2] = [10, 77];
pub const NET_PREFIX: u8 = 16;
pub const FRAME_V: u8 = 1;
pub const T_DATA: u8 = 1;
pub const T_PING: u8 = 2;
/// Самый большой IP-пакет, который пропускаем (MTU адаптера).
pub const MTU: usize = 1280;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct NetPeer {
    pub name: String,
    pub vip: String,
    /// Открытый ключ X25519 (base64).
    pub key: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JoinReq {
    pub key: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JoinResp {
    pub name: String,
    pub vip: String,
    pub prefix: u8,
    /// Токен сеанса (hex) — в каждом кадре до сервера.
    pub token: String,
    pub udp: u16,
    pub peers: Vec<NetPeer>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PeersResp {
    pub peers: Vec<NetPeer>,
}

pub fn in_net(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == NET_BASE[0] && o[1] == NET_BASE[1]
}

/// Широковещательный или групповой адрес: такой пакет уходит всем соседям.
pub fn is_broadcast(ip: Ipv4Addr) -> bool {
    ip.is_broadcast() || ip.is_multicast() || (in_net(ip) && ip.octets()[2] == 255 && ip.octets()[3] == 255)
}

/// Адрес по номеру: 1 → 10.77.0.2 (0 и .1 не выдаём, .255.255 — широковещательный).
pub fn vip_of(n: u32) -> Option<Ipv4Addr> {
    let v = n + 1;
    if v >= 0xFFFF {
        return None;
    }
    Some(Ipv4Addr::new(NET_BASE[0], NET_BASE[1], (v >> 8) as u8, (v & 0xFF) as u8))
}

/// Куда и откуда идёт IPv4-пакет.
pub fn ipv4_addrs(p: &[u8]) -> Option<(Ipv4Addr, Ipv4Addr)> {
    if p.len() < 20 || p[0] >> 4 != 4 {
        return None;
    }
    Some((Ipv4Addr::new(p[12], p[13], p[14], p[15]), Ipv4Addr::new(p[16], p[17], p[18], p[19])))
}

// ---------- кадры ----------

pub fn frame_up(t: u8, token: &[u8; 16], to: Ipv4Addr, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(22 + data.len());
    v.push(FRAME_V);
    v.push(t);
    v.extend_from_slice(token);
    v.extend_from_slice(&to.octets());
    v.extend_from_slice(data);
    v
}

/// (тип, токен, кому, данные)
pub fn parse_up(b: &[u8]) -> Option<(u8, [u8; 16], Ipv4Addr, &[u8])> {
    if b.len() < 22 || b[0] != FRAME_V {
        return None;
    }
    let mut tok = [0u8; 16];
    tok.copy_from_slice(&b[2..18]);
    Some((b[1], tok, Ipv4Addr::new(b[18], b[19], b[20], b[21]), &b[22..]))
}

pub fn frame_down(t: u8, from: Ipv4Addr, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(6 + data.len());
    v.push(FRAME_V);
    v.push(t);
    v.extend_from_slice(&from.octets());
    v.extend_from_slice(data);
    v
}

/// (тип, от кого, данные)
pub fn parse_down(b: &[u8]) -> Option<(u8, Ipv4Addr, &[u8])> {
    if b.len() < 6 || b[0] != FRAME_V {
        return None;
    }
    Some((b[1], Ipv4Addr::new(b[2], b[3], b[4], b[5]), &b[6..]))
}

// ---------- шифрование от ПК до ПК ----------

use base64::Engine;
use chacha20poly1305::aead::Aead;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Постоянный ключ ПК в сети.
pub struct NetKey {
    secret: x25519_dalek::StaticSecret,
    pub public: [u8; 32],
}

impl NetKey {
    pub fn generate() -> NetKey {
        Self::from_bytes(rand::random())
    }

    pub fn from_bytes(b: [u8; 32]) -> NetKey {
        let secret = x25519_dalek::StaticSecret::from(b);
        let public = *x25519_dalek::PublicKey::from(&secret).as_bytes();
        NetKey { secret, public }
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.secret.to_bytes()
    }

    pub fn public_b64(&self) -> String {
        b64().encode(self.public)
    }
}

pub fn parse_key(s: &str) -> Option<[u8; 32]> {
    b64().decode(s.trim()).ok()?.try_into().ok()
}

/// Окно приёма: номера, уже принятые из последних 2048.
#[derive(Default)]
struct Window {
    top: u64,
    bits: Vec<u64>,
    any: bool,
}

const WIN: u64 = 2048;

impl Window {
    fn check_and_set(&mut self, n: u64) -> bool {
        if self.bits.is_empty() {
            self.bits = vec![0; (WIN / 64) as usize];
        }
        if !self.any || n > self.top {
            let shift = if self.any { n - self.top } else { WIN };
            if shift >= WIN {
                self.bits.iter_mut().for_each(|b| *b = 0);
            } else {
                for i in self.top + 1..=n {
                    let k = (i % WIN) as usize;
                    self.bits[k / 64] &= !(1 << (k % 64));
                }
            }
            self.top = n;
            self.any = true;
        } else if self.top - n >= WIN {
            return false;
        }
        let k = (n % WIN) as usize;
        let was = self.bits[k / 64] & (1 << (k % 64)) != 0;
        self.bits[k / 64] |= 1 << (k % 64);
        !was
    }
}

/// Шифрование с одним соседом: свой ключ на отправку (номер запуска — наш), ключи приёма — по его номерам запуска.
pub struct PeerCrypto {
    shared: [u8; 32],
    mine: [u8; 32],
    theirs: [u8; 32],
    boot: [u8; 16],
    out: ChaCha20Poly1305,
    ctr: u64,
    inp: HashMap<[u8; 16], (ChaCha20Poly1305, Window)>,
}

fn derive(shared: &[u8; 32], from: &[u8; 32], to: &[u8; 32], boot: &[u8; 16]) -> ChaCha20Poly1305 {
    let mut info = Vec::with_capacity(16 + 80);
    info.extend_from_slice(b"mostnet v1");
    info.extend_from_slice(from);
    info.extend_from_slice(to);
    info.extend_from_slice(boot);
    let hk = hkdf::Hkdf::<sha2::Sha256>::new(None, shared);
    let mut k = [0u8; 32];
    hk.expand(&info, &mut k).expect("hkdf");
    ChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&k))
}

fn nonce(n: u64) -> chacha20poly1305::Nonce {
    let mut b = [0u8; 12];
    b[4..].copy_from_slice(&n.to_be_bytes());
    b.into()
}

impl PeerCrypto {
    /// Номер запуска — свой у каждой пары ключей шифрования: счётчик начинается с нуля только с новым ключом.
    /// (3.19.2: раньше номер был один на процесс службы — после переподключения к серверу счётчик
    /// начинался с нуля на том же ключе: сосед отбрасывал пакеты как повторы, а nonce повторялся.)
    pub fn new(me: &NetKey, peer: [u8; 32]) -> PeerCrypto {
        let boot: [u8; 16] = rand::random();
        let shared = *me.secret.diffie_hellman(&x25519_dalek::PublicKey::from(peer)).as_bytes();
        let out = derive(&shared, &me.public, &peer, &boot);
        PeerCrypto { shared, mine: me.public, theirs: peer, boot, out, ctr: 0, inp: HashMap::new() }
    }

    pub fn seal(&mut self, packet: &[u8]) -> Vec<u8> {
        let n = self.ctr;
        self.ctr += 1;
        let mut v = Vec::with_capacity(24 + packet.len() + 16);
        v.extend_from_slice(&self.boot);
        v.extend_from_slice(&n.to_be_bytes());
        v.extend(self.out.encrypt(&nonce(n), packet).expect("seal"));
        v
    }

    pub fn open(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        if data.len() < 24 + 16 {
            return None;
        }
        let mut boot = [0u8; 16];
        boot.copy_from_slice(&data[..16]);
        let n = u64::from_be_bytes(data[16..24].try_into().ok()?);
        if !self.inp.contains_key(&boot) {
            if self.inp.len() >= 4 {
                self.inp.clear();
            }
            let c = derive(&self.shared, &self.theirs, &self.mine, &boot);
            self.inp.insert(boot, (c, Window::default()));
        }
        let (c, w) = self.inp.get_mut(&boot)?;
        let p = c.decrypt(&nonce(n), &data[24..]).ok()?;
        // номер отмечаем только после проверки подлинности — чужой мусор окно не сдвигает
        w.check_and_set(n).then_some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames() {
        let tok = [7u8; 16];
        let to = Ipv4Addr::new(10, 77, 0, 3);
        let f = frame_up(T_DATA, &tok, to, b"abc");
        assert_eq!(parse_up(&f), Some((T_DATA, tok, to, &b"abc"[..])));
        let d = frame_down(T_PING, to, b"x");
        assert_eq!(parse_down(&d), Some((T_PING, to, &b"x"[..])));
        assert_eq!(vip_of(1), Some(Ipv4Addr::new(10, 77, 0, 2)));
        assert_eq!(vip_of(255), Some(Ipv4Addr::new(10, 77, 1, 0)));
        assert!(is_broadcast(Ipv4Addr::new(10, 77, 255, 255)) && is_broadcast(Ipv4Addr::BROADCAST) && is_broadcast(Ipv4Addr::new(224, 0, 0, 251)));
        assert!(!is_broadcast(Ipv4Addr::new(10, 77, 0, 255)));
    }

    #[test]
    fn crypto_pair() {
        let (a, b) = (NetKey::generate(), NetKey::generate());
        let mut ab = PeerCrypto::new(&a, b.public);
        let mut ba = PeerCrypto::new(&b, a.public);
        let x = ab.seal(b"ping");
        assert_eq!(ba.open(&x).as_deref(), Some(&b"ping"[..]));
        assert!(ba.open(&x).is_none(), "повтор");
        let y = ba.seal(b"pong");
        assert_eq!(ab.open(&y).as_deref(), Some(&b"pong"[..]));
        // не по порядку — принимается, второй раз — нет
        let p1 = ab.seal(b"1");
        let p2 = ab.seal(b"2");
        assert!(ba.open(&p2).is_some() && ba.open(&p1).is_some() && ba.open(&p1).is_none());
        // перезапуск отправителя: новый номер запуска — новый ключ, счётчик с нуля
        let mut ab2 = PeerCrypto::new(&a, b.public);
        assert_eq!(ba.open(&ab2.seal(b"again")).as_deref(), Some(&b"again"[..]));
        // переподключение без перезапуска процесса (новая пара шифров) — тоже принимается, и много раз подряд
        for i in 0..6u8 {
            let mut again = PeerCrypto::new(&a, b.public);
            for _ in 0..3 {
                assert_eq!(ba.open(&again.seal(&[i])).as_deref(), Some(&[i][..]), "переподключение {}", i);
            }
        }
        // чужой ключ не расшифрует
        let c = NetKey::generate();
        let mut cb = PeerCrypto::new(&c, b.public);
        assert!(ba.open(&cb.seal(b"evil")).is_none());
        // подмена байта
        let mut z = ab.seal(b"data");
        let l = z.len();
        z[l - 1] ^= 1;
        assert!(ba.open(&z).is_none());
    }

    #[test]
    fn window_old() {
        let mut w = Window::default();
        assert!(w.check_and_set(5000));
        assert!(!w.check_and_set(5000 - WIN));
        assert!(w.check_and_set(4000) && !w.check_and_set(4000));
        assert!(w.check_and_set(9000) && w.check_and_set(8000) && !w.check_and_set(6000));
    }
}
