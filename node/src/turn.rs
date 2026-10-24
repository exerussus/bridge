//! Ретранслятор WebRTC (TURN, RFC 5766, только UDP). Нужен, когда участники разговора не видят друг друга напрямую:
//! телефон в Wi-Fi или Tailscale, а сосед — только в Radmin. Тогда звук идёт через этот ПК: оба браузера подключаются
//! к ретранслятору моста, мост пересылает пакеты между ними. Пускает только по временным ключам: их выдаёт сам мост —
//! своим панелям, своим телефонам и соседям (в подписанном привете).

use crate::node::Node;
use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use parking_lot::Mutex;
use sha1::Sha1;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

const COOKIE: u32 = 0x2112_A442;
const REALM: &str = "bridge";
const SOFTWARE: &str = "bridge-turn";
const MAX_ALLOCS: usize = 128;

// методы и классы STUN
const BINDING: u16 = 0x001;
const ALLOCATE: u16 = 0x003;
const REFRESH: u16 = 0x004;
const SEND: u16 = 0x006;
const DATA: u16 = 0x007;
const CREATE_PERMISSION: u16 = 0x008;
const CHANNEL_BIND: u16 = 0x009;
const REQUEST: u16 = 0;
const INDICATION: u16 = 1;
const SUCCESS: u16 = 2;
const ERROR: u16 = 3;

// атрибуты
const A_USERNAME: u16 = 0x0006;
const A_MESSAGE_INTEGRITY: u16 = 0x0008;
const A_ERROR_CODE: u16 = 0x0009;
const A_CHANNEL_NUMBER: u16 = 0x000C;
const A_LIFETIME: u16 = 0x000D;
const A_XOR_PEER_ADDRESS: u16 = 0x0012;
const A_DATA: u16 = 0x0013;
const A_REALM: u16 = 0x0014;
const A_NONCE: u16 = 0x0015;
const A_XOR_RELAYED_ADDRESS: u16 = 0x0016;
const A_REQUESTED_TRANSPORT: u16 = 0x0019;
const A_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const A_SOFTWARE: u16 = 0x8022;
const A_FINGERPRINT: u16 = 0x8028;

fn msg_type(method: u16, class: u16) -> u16 {
    (method & 0x000F) | ((method & 0x0070) << 1) | ((method & 0x0F80) << 2) | ((class & 1) << 4) | ((class & 2) << 7)
}

fn split_type(t: u16) -> (u16, u16) {
    let method = (t & 0x000F) | ((t >> 1) & 0x0070) | ((t >> 2) & 0x0F80);
    let class = ((t >> 4) & 1) | ((t >> 7) & 2);
    (method, class)
}

fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut m = <Hmac<Sha1> as Mac>::new_from_slice(key).expect("hmac");
    m.update(data);
    m.finalize().into_bytes().into()
}

struct Msg<'a> {
    method: u16,
    class: u16,
    tid: [u8; 12],
    raw: &'a [u8],
    /// (тип, смещение начала атрибута, значение)
    attrs: Vec<(u16, usize, &'a [u8])>,
}

impl<'a> Msg<'a> {
    fn parse(b: &'a [u8]) -> Option<Msg<'a>> {
        if b.len() < 20 || b[0] & 0xC0 != 0 {
            return None;
        }
        if u32::from_be_bytes([b[4], b[5], b[6], b[7]]) != COOKIE {
            return None;
        }
        let len = u16::from_be_bytes([b[2], b[3]]) as usize;
        if 20 + len > b.len() || len % 4 != 0 {
            return None;
        }
        let b = &b[..20 + len];
        let (method, class) = split_type(u16::from_be_bytes([b[0], b[1]]));
        let mut tid = [0u8; 12];
        tid.copy_from_slice(&b[8..20]);
        let mut attrs = Vec::new();
        let mut i = 20;
        while i + 4 <= b.len() {
            let t = u16::from_be_bytes([b[i], b[i + 1]]);
            let l = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
            if i + 4 + l > b.len() {
                return None;
            }
            attrs.push((t, i, &b[i + 4..i + 4 + l]));
            i += 4 + ((l + 3) & !3);
        }
        Some(Msg { method, class, tid, raw: b, attrs })
    }

    fn get(&self, t: u16) -> Option<&'a [u8]> {
        self.attrs.iter().find(|a| a.0 == t).map(|a| a.2)
    }

    fn all(&self, t: u16) -> impl Iterator<Item = &'a [u8]> + '_ {
        self.attrs.iter().filter(move |a| a.0 == t).map(|a| a.2)
    }

    fn text(&self, t: u16) -> Option<String> {
        self.get(t).map(|v| String::from_utf8_lossy(v).into_owned())
    }

    /// Проверка MESSAGE-INTEGRITY ключом `key`.
    fn check_integrity(&self, key: &[u8]) -> bool {
        let Some(&(_, off, val)) = self.attrs.iter().find(|a| a.0 == A_MESSAGE_INTEGRITY) else { return false };
        if val.len() != 20 {
            return false;
        }
        let mut copy = self.raw[..off].to_vec();
        let l = (off - 20 + 24) as u16;
        copy[2..4].copy_from_slice(&l.to_be_bytes());
        hmac_sha1(key, &copy) == val
    }
}

fn xor_addr(v: &[u8], tid: &[u8; 12]) -> Option<SocketAddr> {
    if v.len() < 8 {
        return None;
    }
    let port = u16::from_be_bytes([v[2], v[3]]) ^ (COOKIE >> 16) as u16;
    match v[1] {
        1 => {
            let c = COOKIE.to_be_bytes();
            let ip = std::net::Ipv4Addr::new(v[4] ^ c[0], v[5] ^ c[1], v[6] ^ c[2], v[7] ^ c[3]);
            Some(SocketAddr::new(IpAddr::V4(ip), port))
        }
        2 if v.len() >= 20 => {
            let mut x = [0u8; 16];
            x[..4].copy_from_slice(&COOKIE.to_be_bytes());
            x[4..].copy_from_slice(tid);
            let mut a = [0u8; 16];
            for i in 0..16 {
                a[i] = v[4 + i] ^ x[i];
            }
            Some(SocketAddr::new(IpAddr::V6(a.into()), port))
        }
        _ => None,
    }
}

struct Builder {
    buf: Vec<u8>,
    tid: [u8; 12],
}

impl Builder {
    fn new(method: u16, class: u16, tid: [u8; 12]) -> Self {
        let mut buf = Vec::with_capacity(128);
        buf.extend_from_slice(&msg_type(method, class).to_be_bytes());
        buf.extend_from_slice(&[0, 0]);
        buf.extend_from_slice(&COOKIE.to_be_bytes());
        buf.extend_from_slice(&tid);
        Builder { buf, tid }
    }

    fn set_len(&mut self, extra: usize) {
        let l = (self.buf.len() - 20 + extra) as u16;
        self.buf[2..4].copy_from_slice(&l.to_be_bytes());
    }

    fn attr(&mut self, t: u16, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(&t.to_be_bytes());
        self.buf.extend_from_slice(&(v.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(v);
        while self.buf.len() % 4 != 0 {
            self.buf.push(0);
        }
        self.set_len(0);
        self
    }

    fn xor(&mut self, t: u16, a: SocketAddr) -> &mut Self {
        let mut v = vec![0u8];
        let port = a.port() ^ (COOKIE >> 16) as u16;
        match a.ip() {
            IpAddr::V4(ip) => {
                v.push(1);
                v.extend_from_slice(&port.to_be_bytes());
                let c = COOKIE.to_be_bytes();
                let o = ip.octets();
                v.extend_from_slice(&[o[0] ^ c[0], o[1] ^ c[1], o[2] ^ c[2], o[3] ^ c[3]]);
            }
            IpAddr::V6(ip) => {
                v.push(2);
                v.extend_from_slice(&port.to_be_bytes());
                let mut x = [0u8; 16];
                x[..4].copy_from_slice(&COOKIE.to_be_bytes());
                x[4..].copy_from_slice(&self.tid);
                let o = ip.octets();
                for i in 0..16 {
                    v.push(o[i] ^ x[i]);
                }
            }
        }
        self.attr(t, &v)
    }

    fn error(&mut self, code: u16, reason: &str) -> &mut Self {
        let mut v = vec![0, 0, (code / 100) as u8, (code % 100) as u8];
        v.extend_from_slice(reason.as_bytes());
        self.attr(A_ERROR_CODE, &v)
    }

    fn integrity(&mut self, key: &[u8]) -> &mut Self {
        self.set_len(24);
        let mac = hmac_sha1(key, &self.buf);
        self.buf.extend_from_slice(&A_MESSAGE_INTEGRITY.to_be_bytes());
        self.buf.extend_from_slice(&20u16.to_be_bytes());
        self.buf.extend_from_slice(&mac);
        self.set_len(0);
        self
    }

    fn finish(mut self) -> Vec<u8> {
        self.set_len(8);
        let crc = crc32fast::hash(&self.buf) ^ 0x5354_554E;
        self.buf.extend_from_slice(&A_FINGERPRINT.to_be_bytes());
        self.buf.extend_from_slice(&4u16.to_be_bytes());
        self.buf.extend_from_slice(&crc.to_be_bytes());
        self.set_len(0);
        self.buf
    }
}

struct Alloc {
    relay: Arc<UdpSocket>,
    relayed: SocketAddr,
    user: String,
    key: Vec<u8>,
    expires: Mutex<Instant>,
    perms: Mutex<HashMap<IpAddr, Instant>>,
    chans: Mutex<HashMap<u16, (SocketAddr, Instant)>>,
    cancel: CancellationToken,
}

impl Alloc {
    fn permitted(&self, ip: IpAddr) -> bool {
        self.perms.lock().get(&ip).map(|t| *t > Instant::now()).unwrap_or(false)
    }
    fn chan_of(&self, peer: SocketAddr) -> Option<u16> {
        let now = Instant::now();
        self.chans.lock().iter().find(|(_, (a, t))| *a == peer && *t > now).map(|(c, _)| *c)
    }
}

type Key = (SocketAddr, SocketAddr); // (клиент, наш адрес приёма)

#[derive(Default)]
pub struct Turn {
    secret: Mutex<Vec<u8>>,
    pub port: AtomicU16,
    bound: Mutex<HashSet<IpAddr>>,
    failed: Mutex<HashSet<IpAddr>>,
    nonce: Mutex<(String, String, Option<Instant>)>,
    allocs: Mutex<HashMap<Key, Arc<Alloc>>>,
}

impl Turn {
    pub fn load(&self, node: &Node) {
        let f = node.data_dir.join("turn.key");
        let key = match std::fs::read(&f) {
            Ok(k) if k.len() >= 32 => k,
            _ => {
                let k: [u8; 32] = rand::random();
                let _ = std::fs::write(&f, k);
                k.to_vec()
            }
        };
        *self.secret.lock() = key;
    }

    /// Временный ключ ретранслятора (как TURN REST API): имя «срок:кто», пароль — HMAC от имени.
    pub fn creds(&self, who: &str, ttl: Duration) -> (String, String) {
        use base64::Engine;
        let exp = chrono::Utc::now().timestamp() + ttl.as_secs() as i64;
        let user = format!("{}:{}", exp, who.replace(':', "_"));
        let pass = base64::engine::general_purpose::STANDARD.encode(hmac_sha1(&self.secret.lock(), user.as_bytes()));
        (user, pass)
    }

    fn password_for(&self, user: &str) -> Option<String> {
        use base64::Engine;
        let exp: i64 = user.split(':').next()?.parse().ok()?;
        if exp < chrono::Utc::now().timestamp() {
            return None;
        }
        Some(base64::engine::general_purpose::STANDARD.encode(hmac_sha1(&self.secret.lock(), user.as_bytes())))
    }

    fn nonce(&self) -> String {
        let mut g = self.nonce.lock();
        if g.2.map(|t| t.elapsed() > Duration::from_secs(1800)).unwrap_or(true) {
            g.1 = std::mem::take(&mut g.0);
            g.0 = hex::encode(rand::random::<[u8; 12]>());
            g.2 = Some(Instant::now());
        }
        g.0.clone()
    }

    fn nonce_ok(&self, n: &str) -> bool {
        let _ = self.nonce();
        let g = self.nonce.lock();
        !n.is_empty() && (n == g.0 || n == g.1)
    }

    pub fn running(&self) -> bool {
        self.port.load(Ordering::SeqCst) != 0
    }

    pub fn allocations(&self) -> usize {
        self.allocs.lock().len()
    }
}

/// Слушать на 127.0.0.1 и на каждой сети ПК (у каждой — свой адрес ретрансляции: телефону в Wi-Fi — адрес Wi-Fi,
/// соседу в Radmin — адрес Radmin). Повторяется вместе с проверкой сетей.
pub async fn bind_all(node: &Arc<Node>) {
    let port = node.cfg.read().turn_port;
    if port == 0 {
        return;
    }
    if node.turn.secret.lock().is_empty() {
        node.turn.load(node);
    }
    let mut ips: Vec<IpAddr> = vec![IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)];
    ips.extend(crate::http_peer::interface_ips().into_iter().map(|x| x.0));
    for ip in ips {
        if node.turn.bound.lock().contains(&ip) || node.turn.failed.lock().contains(&ip) {
            continue;
        }
        match UdpSocket::bind(SocketAddr::new(ip, port)).await {
            Ok(s) => {
                let real = s.local_addr().map(|a| a.port()).unwrap_or(port);
                node.turn.port.store(real, Ordering::SeqCst);
                node.turn.bound.lock().insert(ip);
                let s = Arc::new(s);
                let n = node.clone();
                tokio::spawn(async move { serve(n, s).await });
            }
            Err(e) => {
                node.turn.failed.lock().insert(ip);
                if ip.is_loopback() {
                    node.info(&format!("Ретранслятор голоса: порт UDP {} занят ({}) — разговоры через этот ПК не пойдут", port, e));
                }
            }
        }
    }
}

async fn serve(node: Arc<Node>, sock: Arc<UdpSocket>) {
    let local = match sock.local_addr() {
        Ok(a) => a,
        Err(_) => return,
    };
    let mut buf = vec![0u8; 65536];
    let mut sweep = Instant::now();
    loop {
        let r = tokio::select! {
            _ = node.shutdown.cancelled() => break,
            r = sock.recv_from(&mut buf) => r,
        };
        let Ok((n, from)) = r else { continue };
        let pkt = &buf[..n];
        if n >= 4 && (0x40..0x80).contains(&pkt[0]) {
            channel_data(&node, local, from, pkt).await;
        } else if let Some(m) = Msg::parse(pkt) {
            let resp = handle(&node, &sock, local, from, &m).await;
            if std::env::var_os("BRIDGE_TURN_DEBUG").is_some() {
                let r = resp.as_ref().and_then(|r| Msg::parse(r)).map(|r| format!("{}/{} {:?}", r.method, r.class, r.get(A_ERROR_CODE).map(|e| (e[2] as u16) * 100 + e[3] as u16))).unwrap_or_default();
                eprintln!("TURN {} {} m={} c={} attrs={:?} -> {}", local, from, m.method, m.class, m.attrs.iter().map(|a| a.0).collect::<Vec<_>>(), r);
            }
            if let Some(resp) = resp {
                let _ = sock.send_to(&resp, from).await;
            }
        }
        if sweep.elapsed() > Duration::from_secs(20) {
            sweep = Instant::now();
            let now = Instant::now();
            node.turn.allocs.lock().retain(|_, a| {
                let alive = *a.expires.lock() > now;
                if !alive {
                    a.cancel.cancel();
                }
                alive
            });
        }
    }
}

async fn channel_data(node: &Node, local: SocketAddr, from: SocketAddr, pkt: &[u8]) {
    let ch = u16::from_be_bytes([pkt[0], pkt[1]]);
    let len = u16::from_be_bytes([pkt[2], pkt[3]]) as usize;
    if 4 + len > pkt.len() {
        return;
    }
    let Some(a) = node.turn.allocs.lock().get(&(from, local)).cloned() else { return };
    let peer = a.chans.lock().get(&ch).filter(|(_, t)| *t > Instant::now()).map(|x| x.0);
    if let Some(peer) = peer {
        let _ = a.relay.send_to(&pkt[4..4 + len], peer).await;
    }
}

fn err_resp(m: &Msg, code: u16, reason: &str, node: &Node) -> Vec<u8> {
    let mut b = Builder::new(m.method, ERROR, m.tid);
    b.error(code, reason);
    if code == 401 || code == 438 {
        b.attr(A_REALM, REALM.as_bytes());
        b.attr(A_NONCE, node.turn.nonce().as_bytes());
    }
    b.attr(A_SOFTWARE, SOFTWARE.as_bytes());
    b.finish()
}

async fn handle(node: &Arc<Node>, sock: &Arc<UdpSocket>, local: SocketAddr, from: SocketAddr, m: &Msg<'_>) -> Option<Vec<u8>> {
    if m.class == REQUEST && m.method == BINDING {
        let mut b = Builder::new(BINDING, SUCCESS, m.tid);
        b.xor(A_XOR_MAPPED_ADDRESS, from).attr(A_SOFTWARE, SOFTWARE.as_bytes());
        return Some(b.finish());
    }
    let key = (from, local);
    if m.class == INDICATION {
        if m.method == SEND {
            let a = node.turn.allocs.lock().get(&key).cloned()?;
            let peer = xor_addr(m.get(A_XOR_PEER_ADDRESS)?, &m.tid)?;
            let data = m.get(A_DATA)?;
            if a.permitted(peer.ip()) {
                let _ = a.relay.send_to(data, peer).await;
            }
        }
        return None;
    }
    if m.class != REQUEST {
        return None;
    }
    // всё остальное — только с ключом
    let (Some(user), Some(_)) = (m.text(A_USERNAME), m.get(A_MESSAGE_INTEGRITY)) else { return Some(err_resp(m, 401, "Unauthorized", node)) };
    if !node.turn.nonce_ok(&m.text(A_NONCE).unwrap_or_default()) {
        return Some(err_resp(m, 438, "Stale Nonce", node));
    }
    let Some(pass) = node.turn.password_for(&user) else { return Some(err_resp(m, 401, "Unauthorized", node)) };
    let realm = m.text(A_REALM).unwrap_or_else(|| REALM.into());
    let ukey = Md5::digest(format!("{}:{}:{}", user, realm, pass).as_bytes()).to_vec();
    if !m.check_integrity(&ukey) {
        return Some(err_resp(m, 401, "Unauthorized", node));
    }
    let ok = |b: &mut Builder| {
        b.attr(A_SOFTWARE, SOFTWARE.as_bytes());
        b.integrity(&ukey);
    };
    match m.method {
        ALLOCATE => {
            let existing = node.turn.allocs.lock().get(&key).cloned();
            if let Some(a) = existing {
                if a.user != user {
                    return Some(err_resp(m, 437, "Allocation Mismatch", node));
                }
                // повтор того же запроса — тот же ответ
                let mut b = Builder::new(ALLOCATE, SUCCESS, m.tid);
                b.xor(A_XOR_RELAYED_ADDRESS, a.relayed).attr(A_LIFETIME, &600u32.to_be_bytes()).xor(A_XOR_MAPPED_ADDRESS, from);
                ok(&mut b);
                return Some(b.finish());
            }
            if m.get(A_REQUESTED_TRANSPORT).map(|v| v.first() == Some(&17)) != Some(true) {
                return Some(err_resp(m, 442, "Unsupported Transport Protocol", node));
            }
            if node.turn.allocs.lock().len() >= MAX_ALLOCS {
                return Some(err_resp(m, 486, "Allocation Quota Reached", node));
            }
            let Ok(relay) = UdpSocket::bind(SocketAddr::new(local.ip(), 0)).await else { return Some(err_resp(m, 508, "Insufficient Capacity", node)) };
            let relayed = relay.local_addr().ok()?;
            let life = lifetime(m);
            let a = Arc::new(Alloc {
                relay: Arc::new(relay),
                relayed,
                user: user.clone(),
                key: ukey.clone(),
                expires: Mutex::new(Instant::now() + life),
                perms: Mutex::new(HashMap::new()),
                chans: Mutex::new(HashMap::new()),
                cancel: CancellationToken::new(),
            });
            node.turn.allocs.lock().insert(key, a.clone());
            tokio::spawn(relay_loop(a.clone(), sock.clone(), from));
            let mut b = Builder::new(ALLOCATE, SUCCESS, m.tid);
            b.xor(A_XOR_RELAYED_ADDRESS, relayed).attr(A_LIFETIME, &(life.as_secs() as u32).to_be_bytes()).xor(A_XOR_MAPPED_ADDRESS, from);
            ok(&mut b);
            Some(b.finish())
        }
        REFRESH | CREATE_PERMISSION | CHANNEL_BIND => {
            let Some(a) = node.turn.allocs.lock().get(&key).cloned() else { return Some(err_resp(m, 437, "Allocation Mismatch", node)) };
            if a.user != user {
                return Some(err_resp(m, 441, "Wrong Credentials", node));
            }
            let mut b = Builder::new(m.method, SUCCESS, m.tid);
            match m.method {
                REFRESH => {
                    let life = lifetime(m);
                    if life.is_zero() {
                        a.cancel.cancel();
                        node.turn.allocs.lock().remove(&key);
                    } else {
                        *a.expires.lock() = Instant::now() + life;
                    }
                    b.attr(A_LIFETIME, &(life.as_secs() as u32).to_be_bytes());
                }
                CREATE_PERMISSION => {
                    let peers: Vec<SocketAddr> = m.all(A_XOR_PEER_ADDRESS).filter_map(|v| xor_addr(v, &m.tid)).collect();
                    if peers.is_empty() {
                        return Some(err_resp(m, 400, "Bad Request", node));
                    }
                    let until = Instant::now() + Duration::from_secs(300);
                    let mut g = a.perms.lock();
                    for p in peers {
                        g.insert(p.ip(), until);
                    }
                }
                _ => {
                    let ch = m.get(A_CHANNEL_NUMBER).filter(|v| v.len() >= 2).map(|v| u16::from_be_bytes([v[0], v[1]]));
                    let peer = m.get(A_XOR_PEER_ADDRESS).and_then(|v| xor_addr(v, &m.tid));
                    let (Some(ch), Some(peer)) = (ch, peer) else { return Some(err_resp(m, 400, "Bad Request", node)) };
                    if !(0x4000..=0x7FFF).contains(&ch) {
                        return Some(err_resp(m, 400, "Bad Request", node));
                    }
                    let until = Instant::now() + Duration::from_secs(600);
                    {
                        let mut g = a.chans.lock();
                        if g.iter().any(|(c, (p, _))| (*c == ch && *p != peer) || (*c != ch && *p == peer)) {
                            drop(g);
                            return Some(err_resp(m, 400, "Bad Request", node));
                        }
                        g.insert(ch, (peer, until));
                    }
                    a.perms.lock().insert(peer.ip(), Instant::now() + Duration::from_secs(300));
                }
            }
            ok(&mut b);
            Some(b.finish())
        }
        _ => Some(err_resp(m, 400, "Bad Request", node)),
    }
}

fn lifetime(m: &Msg) -> Duration {
    let req = m.get(A_LIFETIME).filter(|v| v.len() >= 4).map(|v| u32::from_be_bytes([v[0], v[1], v[2], v[3]]) as u64);
    match req {
        Some(0) => Duration::ZERO,
        Some(s) => Duration::from_secs(s.clamp(60, 3600)),
        None => Duration::from_secs(600),
    }
}

/// Пакеты от собеседника на адрес ретрансляции → клиенту (ChannelData, если канал привязан, иначе Data-сообщение).
async fn relay_loop(a: Arc<Alloc>, sock: Arc<UdpSocket>, client: SocketAddr) {
    let mut buf = vec![0u8; 65536];
    loop {
        let r = tokio::select! {
            _ = a.cancel.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                if *a.expires.lock() < Instant::now() { break; }
                continue;
            }
            r = a.relay.recv_from(&mut buf) => r,
        };
        let Ok((n, peer)) = r else { continue };
        if !a.permitted(peer.ip()) {
            continue;
        }
        let data = &buf[..n];
        if let Some(ch) = a.chan_of(peer) {
            let mut out = Vec::with_capacity(n + 8);
            out.extend_from_slice(&ch.to_be_bytes());
            out.extend_from_slice(&(n as u16).to_be_bytes());
            out.extend_from_slice(data);
            while out.len() % 4 != 0 {
                out.push(0);
            }
            let _ = sock.send_to(&out, client).await;
        } else {
            let mut tid = [0u8; 12];
            tid.copy_from_slice(&rand::random::<[u8; 12]>());
            let mut b = Builder::new(DATA, INDICATION, tid);
            b.xor(A_XOR_PEER_ADDRESS, peer).attr(A_DATA, data);
            let _ = sock.send_to(&b.buf, client).await;
        }
    }
    let _ = a.key.len();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_roundtrip() {
        for (m, c) in [(ALLOCATE, REQUEST), (ALLOCATE, SUCCESS), (ALLOCATE, ERROR), (SEND, INDICATION), (DATA, INDICATION), (CHANNEL_BIND, SUCCESS)] {
            assert_eq!(split_type(msg_type(m, c)), (m, c));
        }
        assert_eq!(msg_type(BINDING, REQUEST), 0x0001);
        assert_eq!(msg_type(BINDING, SUCCESS), 0x0101);
        assert_eq!(msg_type(ALLOCATE, ERROR), 0x0113);
        assert_eq!(msg_type(DATA, INDICATION), 0x0017);
    }

    #[test]
    fn integrity_and_xor() {
        let tid = [7u8; 12];
        let mut b = Builder::new(ALLOCATE, REQUEST, tid);
        let a: SocketAddr = "192.168.1.172:51000".parse().unwrap();
        b.xor(A_XOR_PEER_ADDRESS, a).attr(A_USERNAME, b"123:x").integrity(b"key");
        let raw = b.finish();
        let m = Msg::parse(&raw).unwrap();
        assert!(m.check_integrity(b"key"));
        assert!(!m.check_integrity(b"other"));
        assert_eq!(xor_addr(m.get(A_XOR_PEER_ADDRESS).unwrap(), &tid), Some(a));
        assert_eq!(m.text(A_USERNAME).as_deref(), Some("123:x"));
    }
}
