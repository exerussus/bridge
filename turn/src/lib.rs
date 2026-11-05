//! Ретранслятор WebRTC (TURN, RFC 5766; к клиенту — UDP или TCP, к собеседнику — UDP). Общий для моста (ретранслятор
//! на ПК, `node/src/turn.rs`) и своего сервера (`most-relay`). Пускает только по временным ключам (как TURN REST API):
//! имя «срок:кто», пароль — HMAC-SHA1 от имени секретом ретранслятора.
//! По TCP — когда UDP до ретранслятора не проходит: размещение по TCP живёт, пока живо соединение; до Allocate на
//! соединении принимаются только сообщения STUN.

use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use parking_lot::Mutex;
use sha1::Sha1;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const COOKIE: u32 = 0x2112_A442;
const REALM: &str = "bridge";
const SOFTWARE: &str = "bridge-turn";
const MAX_TCP: usize = 256;
/// TCP-соединение без единого кадра столько времени — закрыть (WebRTC обновляет разрешения раз в 5 мин).
const TCP_IDLE: Duration = Duration::from_secs(600);
/// Очередь кадров к TCP-клиенту: переполнилась — лишнее выбрасываем (звук переживает потери, а память — нет).
const TCP_QUEUE: usize = 512;

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
    /// «кто» из имени ключа — для учёта трафика и отзыва.
    who: String,
    up: AtomicU64,
    down: AtomicU64,
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

/// (клиент, наш адрес приёма, по TCP ли)
type Key = (SocketAddr, SocketAddr, bool);

/// Куда отвечать клиенту: в UDP-сокет приёма или в его TCP-соединение.
#[derive(Clone)]
enum Back {
    Udp(Arc<UdpSocket>, SocketAddr),
    Tcp(mpsc::Sender<Vec<u8>>),
}

impl Back {
    async fn send(&self, b: &[u8]) {
        match self {
            Back::Udp(s, to) => {
                let _ = s.send_to(b, *to).await;
            }
            Back::Tcp(tx) => {
                let _ = tx.try_send(b.to_vec());
            }
        }
    }
}

/// Проверка «пускать ли этого «кто»» (сервер — отозванные участники).
pub type AllowFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct Opts {
    /// Адрес ретрансляции, который сообщать клиентам, если слушаем 0.0.0.0 (сервер: внешний IPv4).
    pub relay_ip: Option<IpAddr>,
    /// Порты адресов ретрансляции (включительно); None — любой свободный.
    pub relay_ports: Option<(u16, u16)>,
    pub max_allocs: usize,
    /// Не больше размещений на одного «кто».
    pub max_per_who: usize,
    pub allow: Option<AllowFn>,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { relay_ip: None, relay_ports: None, max_allocs: 128, max_per_who: 128, allow: None }
    }
}

/// Трафик одного «кто»: к клиенту и от клиента, байты.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub up: u64,
    pub down: u64,
    pub allocs: usize,
}

#[derive(Default)]
pub struct Turn {
    secret: Mutex<Vec<u8>>,
    nonce: Mutex<(String, String, Option<Instant>)>,
    allocs: Mutex<HashMap<Key, Arc<Alloc>>>,
    tcp_conns: AtomicUsize,
    /// Трафик закрытых размещений по «кто».
    done: Mutex<HashMap<String, Usage>>,
    pub opts: Opts,
    pub stop: CancellationToken,
}

impl Turn {
    pub fn new(secret: Vec<u8>, opts: Opts, stop: CancellationToken) -> Arc<Turn> {
        Arc::new(Turn { secret: Mutex::new(secret), opts, stop, ..Default::default() })
    }

    pub fn set_secret(&self, k: Vec<u8>) {
        *self.secret.lock() = k;
    }

    pub fn has_secret(&self) -> bool {
        !self.secret.lock().is_empty()
    }

    /// Временный ключ (как TURN REST API): имя «срок:кто», пароль — HMAC от имени.
    pub fn creds(&self, who: &str, ttl: Duration) -> (String, String) {
        use base64::Engine;
        let exp = chrono::Utc::now().timestamp() + ttl.as_secs() as i64;
        let user = format!("{}:{}", exp, who.replace(':', "_"));
        let pass = base64::engine::general_purpose::STANDARD.encode(hmac_sha1(&self.secret.lock(), user.as_bytes()));
        (user, pass)
    }

    fn password_for(&self, user: &str) -> Option<String> {
        use base64::Engine;
        let (exp, who) = user.split_once(':')?;
        let exp: i64 = exp.parse().ok()?;
        if exp < chrono::Utc::now().timestamp() {
            return None;
        }
        if let Some(allow) = &self.opts.allow {
            if !allow(who) {
                return None;
            }
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

    pub fn allocations(&self) -> usize {
        self.allocs.lock().len()
    }

    pub fn tcp_connections(&self) -> usize {
        self.tcp_conns.load(Ordering::SeqCst)
    }

    /// Трафик по «кто»: закрытые размещения + открытые сейчас.
    pub fn usage(&self) -> HashMap<String, Usage> {
        let mut m = self.done.lock().clone();
        for a in self.allocs.lock().values() {
            let u = m.entry(a.who.clone()).or_default();
            u.up += a.up.load(Ordering::Relaxed);
            u.down += a.down.load(Ordering::Relaxed);
            u.allocs += 1;
        }
        m
    }

    fn retire(&self, a: &Alloc) {
        a.cancel.cancel();
        let mut g = self.done.lock();
        let u = g.entry(a.who.clone()).or_default();
        u.up += a.up.swap(0, Ordering::Relaxed);
        u.down += a.down.swap(0, Ordering::Relaxed);
    }

    /// Отключить «кто» сразу (сервер: отозван): его размещения закрываются.
    pub fn drop_who(&self, who: &str) {
        let gone: Vec<Arc<Alloc>> = {
            let mut g = self.allocs.lock();
            let keys: Vec<Key> = g.iter().filter(|(_, a)| a.who == who).map(|(k, _)| *k).collect();
            keys.iter().filter_map(|k| g.remove(k)).collect()
        };
        for a in gone {
            self.retire(&a);
        }
    }

    fn sweep(&self) {
        let now = Instant::now();
        let gone: Vec<Arc<Alloc>> = {
            let mut g = self.allocs.lock();
            let keys: Vec<Key> = g.iter().filter(|(_, a)| *a.expires.lock() <= now).map(|(k, _)| *k).collect();
            keys.iter().filter_map(|k| g.remove(k)).collect()
        };
        for a in gone {
            self.retire(&a);
        }
    }
}


/// Принимать по UDP-сокету до остановки.
pub async fn serve_udp(t: Arc<Turn>, sock: Arc<UdpSocket>) {
    let local = match sock.local_addr() {
        Ok(a) => a,
        Err(_) => return,
    };
    let mut buf = vec![0u8; 65536];
    let mut sweep = Instant::now();
    loop {
        let r = tokio::select! {
            _ = t.stop.cancelled() => break,
            r = sock.recv_from(&mut buf) => r,
        };
        let Ok((n, from)) = r else { continue };
        let back = Back::Udp(sock.clone(), from);
        on_frame(&t, &back, local, from, (from, local, false), &buf[..n]).await;
        if sweep.elapsed() > Duration::from_secs(20) {
            sweep = Instant::now();
            t.sweep();
        }
    }
}

/// Один кадр от клиента (датаграмма UDP или кадр, вырезанный из TCP-потока).
async fn on_frame(t: &Arc<Turn>, back: &Back, local: SocketAddr, from: SocketAddr, key: Key, pkt: &[u8]) {
    if pkt.len() >= 4 && (0x40..0x80).contains(&pkt[0]) {
        channel_data(t, key, pkt).await;
    } else if let Some(m) = Msg::parse(pkt) {
        let resp = handle(t, back, local, from, key, &m).await;
        if std::env::var_os("BRIDGE_TURN_DEBUG").is_some() {
            let r = resp.as_ref().and_then(|r| Msg::parse(r)).map(|r| format!("{}/{} {:?}", r.method, r.class, r.get(A_ERROR_CODE).map(|e| (e[2] as u16) * 100 + e[3] as u16))).unwrap_or_default();
            eprintln!("TURN{} {} {} m={} c={} attrs={:?} -> {}", if key.2 { "/tcp" } else { "" }, local, from, m.method, m.class, m.attrs.iter().map(|a| a.0).collect::<Vec<_>>(), r);
        }
        if let Some(resp) = resp {
            back.send(&resp).await;
        }
    }
}

/// Длина следующего кадра в TCP-потоке (RFC 5766 §11.5): STUN — 20 + длина из заголовка, ChannelData — 4 + длина,
/// выровненная до 4 байт. `None` — данных пока мало; `Some(0)` — это не TURN, соединение закрыть.
fn tcp_frame_len(b: &[u8]) -> Option<usize> {
    if b.len() < 4 {
        return None;
    }
    let len = u16::from_be_bytes([b[2], b[3]]) as usize;
    match b[0] {
        0x00..=0x3F if b.len() >= 8 && b[4..8] != COOKIE.to_be_bytes() => Some(0),
        0x00..=0x3F if b.len() < 8 => None,
        0x00..=0x3F => Some(20 + len),
        0x40..=0x7F => Some((4 + len + 3) & !3),
        _ => Some(0),
    }
}

/// Принимать TCP-соединения TURN до остановки.
pub async fn serve_tcp(t: Arc<Turn>, l: TcpListener) {
    let Ok(local) = l.local_addr() else { return };
    loop {
        let r = tokio::select! {
            _ = t.stop.cancelled() => break,
            r = l.accept() => r,
        };
        let Ok((s, from)) = r else { continue };
        let t = t.clone();
        tokio::spawn(async move { serve_tcp_conn(t, s, local, from).await });
    }
}

/// Одно TCP-соединение TURN (сервер зовёт его сам, когда первый байт соединения — STUN).
pub async fn serve_tcp_conn(t: Arc<Turn>, s: TcpStream, local: SocketAddr, from: SocketAddr) {
    if t.tcp_conns.fetch_add(1, Ordering::SeqCst) >= MAX_TCP {
        t.tcp_conns.fetch_sub(1, Ordering::SeqCst);
        return;
    }
    tcp_conn(&t, s, local, from).await;
    t.tcp_conns.fetch_sub(1, Ordering::SeqCst);
    let gone = t.allocs.lock().remove(&(from, local, true));
    if let Some(a) = gone {
        t.retire(&a);
    }
}

async fn tcp_conn(t: &Arc<Turn>, s: TcpStream, local: SocketAddr, from: SocketAddr) {
    let _ = s.set_nodelay(true);
    let (mut rd, mut wr) = s.into_split();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(TCP_QUEUE);
    let done = CancellationToken::new();
    let wdone = done.clone();
    let writer = tokio::spawn(async move {
        loop {
            let b = tokio::select! {
                _ = wdone.cancelled() => break,
                b = rx.recv() => b,
            };
            let Some(b) = b else { break };
            if wr.write_all(&b).await.is_err() {
                break;
            }
        }
        wdone.cancel();
    });
    let back = Back::Tcp(tx);
    let key = (from, local, true);
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut chunk = vec![0u8; 16384];
    'conn: loop {
        let r = tokio::select! {
            _ = t.stop.cancelled() => break,
            _ = done.cancelled() => break,
            _ = tokio::time::sleep(TCP_IDLE) => break,
            r = rd.read(&mut chunk) => r,
        };
        let n = match r {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        loop {
            // ChannelData бывает только после Allocate на этом же соединении — иначе это не TURN-клиент
            if buf.first().is_some_and(|b| *b >= 0x40) && !t.allocs.lock().contains_key(&key) {
                break 'conn;
            }
            match tcp_frame_len(&buf) {
                None => break,
                Some(0) => break 'conn,
                Some(len) if buf.len() < len => break,
                Some(len) => {
                    let frame: Vec<u8> = buf.drain(..len).collect();
                    on_frame(t, &back, local, from, key, &frame).await;
                }
            }
        }
    }
    done.cancel();
    let _ = writer.await;
}

async fn channel_data(t: &Turn, key: Key, pkt: &[u8]) {
    let ch = u16::from_be_bytes([pkt[0], pkt[1]]);
    let len = u16::from_be_bytes([pkt[2], pkt[3]]) as usize;
    if 4 + len > pkt.len() {
        return;
    }
    let Some(a) = t.allocs.lock().get(&key).cloned() else { return };
    let peer = a.chans.lock().get(&ch).filter(|(_, t)| *t > Instant::now()).map(|x| x.0);
    if let Some(peer) = peer {
        a.up.fetch_add(len as u64, Ordering::Relaxed);
        let _ = a.relay.send_to(&pkt[4..4 + len], peer).await;
    }
}

fn err_resp(m: &Msg, code: u16, reason: &str, t: &Turn) -> Vec<u8> {
    let mut b = Builder::new(m.method, ERROR, m.tid);
    b.error(code, reason);
    if code == 401 || code == 438 {
        b.attr(A_REALM, REALM.as_bytes());
        b.attr(A_NONCE, t.nonce().as_bytes());
    }
    b.attr(A_SOFTWARE, SOFTWARE.as_bytes());
    b.finish()
}

async fn handle(t: &Arc<Turn>, back: &Back, local: SocketAddr, from: SocketAddr, key: Key, m: &Msg<'_>) -> Option<Vec<u8>> {
    if m.class == REQUEST && m.method == BINDING {
        let mut b = Builder::new(BINDING, SUCCESS, m.tid);
        b.xor(A_XOR_MAPPED_ADDRESS, from).attr(A_SOFTWARE, SOFTWARE.as_bytes());
        return Some(b.finish());
    }
    if m.class == INDICATION {
        if m.method == SEND {
            let a = t.allocs.lock().get(&key).cloned()?;
            let peer = xor_addr(m.get(A_XOR_PEER_ADDRESS)?, &m.tid)?;
            let data = m.get(A_DATA)?;
            if a.permitted(peer.ip()) {
                a.up.fetch_add(data.len() as u64, Ordering::Relaxed);
                let _ = a.relay.send_to(data, peer).await;
            }
        }
        return None;
    }
    if m.class != REQUEST {
        return None;
    }
    // всё остальное — только с ключом
    let (Some(user), Some(_)) = (m.text(A_USERNAME), m.get(A_MESSAGE_INTEGRITY)) else { return Some(err_resp(m, 401, "Unauthorized", t)) };
    if !t.nonce_ok(&m.text(A_NONCE).unwrap_or_default()) {
        return Some(err_resp(m, 438, "Stale Nonce", t));
    }
    let Some(pass) = t.password_for(&user) else { return Some(err_resp(m, 401, "Unauthorized", t)) };
    let realm = m.text(A_REALM).unwrap_or_else(|| REALM.into());
    let ukey = Md5::digest(format!("{}:{}:{}", user, realm, pass).as_bytes()).to_vec();
    if !m.check_integrity(&ukey) {
        return Some(err_resp(m, 401, "Unauthorized", t));
    }
    let ok = |b: &mut Builder| {
        b.attr(A_SOFTWARE, SOFTWARE.as_bytes());
        b.integrity(&ukey);
    };
    match m.method {
        ALLOCATE => {
            let existing = t.allocs.lock().get(&key).cloned();
            if let Some(a) = existing {
                if a.user != user {
                    return Some(err_resp(m, 437, "Allocation Mismatch", t));
                }
                // повтор того же запроса — тот же ответ
                let mut b = Builder::new(ALLOCATE, SUCCESS, m.tid);
                b.xor(A_XOR_RELAYED_ADDRESS, a.relayed).attr(A_LIFETIME, &600u32.to_be_bytes()).xor(A_XOR_MAPPED_ADDRESS, from);
                ok(&mut b);
                return Some(b.finish());
            }
            if m.get(A_REQUESTED_TRANSPORT).map(|v| v.first() == Some(&17)) != Some(true) {
                return Some(err_resp(m, 442, "Unsupported Transport Protocol", t));
            }
            let who = user.split_once(':').map(|x| x.1.to_string()).unwrap_or_default();
            {
                let g = t.allocs.lock();
                if g.len() >= t.opts.max_allocs || g.values().filter(|a| a.who == who).count() >= t.opts.max_per_who {
                    return Some(err_resp(m, 486, "Allocation Quota Reached", t));
                }
            }
            let Some((relay, relayed)) = bind_relay(t, local.ip()).await else { return Some(err_resp(m, 508, "Insufficient Capacity", t)) };
            let life = lifetime(m);
            let a = Arc::new(Alloc {
                relay: Arc::new(relay),
                who,
                up: AtomicU64::new(0),
                down: AtomicU64::new(0),
                relayed,
                user: user.clone(),
                key: ukey.clone(),
                expires: Mutex::new(Instant::now() + life),
                perms: Mutex::new(HashMap::new()),
                chans: Mutex::new(HashMap::new()),
                cancel: CancellationToken::new(),
            });
            t.allocs.lock().insert(key, a.clone());
            tokio::spawn(relay_loop(a.clone(), back.clone()));
            let mut b = Builder::new(ALLOCATE, SUCCESS, m.tid);
            b.xor(A_XOR_RELAYED_ADDRESS, relayed).attr(A_LIFETIME, &(life.as_secs() as u32).to_be_bytes()).xor(A_XOR_MAPPED_ADDRESS, from);
            ok(&mut b);
            Some(b.finish())
        }
        REFRESH | CREATE_PERMISSION | CHANNEL_BIND => {
            let Some(a) = t.allocs.lock().get(&key).cloned() else { return Some(err_resp(m, 437, "Allocation Mismatch", t)) };
            if a.user != user {
                return Some(err_resp(m, 441, "Wrong Credentials", t));
            }
            let mut b = Builder::new(m.method, SUCCESS, m.tid);
            match m.method {
                REFRESH => {
                    let life = lifetime(m);
                    if life.is_zero() {
                        let gone = t.allocs.lock().remove(&key);
                        if let Some(a) = gone {
                            t.retire(&a);
                        }
                    } else {
                        *a.expires.lock() = Instant::now() + life;
                    }
                    b.attr(A_LIFETIME, &(life.as_secs() as u32).to_be_bytes());
                }
                CREATE_PERMISSION => {
                    let peers: Vec<SocketAddr> = m.all(A_XOR_PEER_ADDRESS).filter_map(|v| xor_addr(v, &m.tid)).collect();
                    if peers.is_empty() {
                        return Some(err_resp(m, 400, "Bad Request", t));
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
                    let (Some(ch), Some(peer)) = (ch, peer) else { return Some(err_resp(m, 400, "Bad Request", t)) };
                    if !(0x4000..=0x7FFF).contains(&ch) {
                        return Some(err_resp(m, 400, "Bad Request", t));
                    }
                    let until = Instant::now() + Duration::from_secs(600);
                    {
                        let mut g = a.chans.lock();
                        if g.iter().any(|(c, (p, _))| (*c == ch && *p != peer) || (*c != ch && *p == peer)) {
                            drop(g);
                            return Some(err_resp(m, 400, "Bad Request", t));
                        }
                        g.insert(ch, (peer, until));
                    }
                    a.perms.lock().insert(peer.ip(), Instant::now() + Duration::from_secs(300));
                }
            }
            ok(&mut b);
            Some(b.finish())
        }
        _ => Some(err_resp(m, 400, "Bad Request", t)),
    }
}

/// Сокет ретрансляции и адрес, который сообщить клиенту. Слушаем 0.0.0.0 — нужен `relay_ip` (внешний адрес сервера).
async fn bind_relay(t: &Turn, ip: IpAddr) -> Option<(UdpSocket, SocketAddr)> {
    let shown = match (ip.is_unspecified(), t.opts.relay_ip) {
        (_, Some(r)) => r,
        (false, None) => ip,
        (true, None) => return None,
    };
    let s = match t.opts.relay_ports {
        None => UdpSocket::bind(SocketAddr::new(ip, 0)).await.ok()?,
        Some((lo, hi)) => {
            let mut got = None;
            for _ in 0..64 {
                let p = lo + (rand::random::<u32>() % ((hi - lo) as u32 + 1)) as u16;
                if let Ok(s) = UdpSocket::bind(SocketAddr::new(ip, p)).await {
                    got = Some(s);
                    break;
                }
            }
            got?
        }
    };
    let port = s.local_addr().ok()?.port();
    Some((s, SocketAddr::new(shown, port)))
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
async fn relay_loop(a: Arc<Alloc>, back: Back) {
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
        a.down.fetch_add(n as u64, Ordering::Relaxed);
        if let Some(ch) = a.chan_of(peer) {
            let mut out = Vec::with_capacity(n + 8);
            out.extend_from_slice(&ch.to_be_bytes());
            out.extend_from_slice(&(n as u16).to_be_bytes());
            out.extend_from_slice(data);
            while out.len() % 4 != 0 {
                out.push(0);
            }
            back.send(&out).await;
        } else {
            let mut tid = [0u8; 12];
            tid.copy_from_slice(&rand::random::<[u8; 12]>());
            let mut b = Builder::new(DATA, INDICATION, tid);
            b.xor(A_XOR_PEER_ADDRESS, peer).attr(A_DATA, data);
            back.send(&b.buf).await;
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
    fn tcp_framing() {
        // STUN: 20 байт заголовка + длина атрибутов
        let mut b = Builder::new(BINDING, REQUEST, [1u8; 12]);
        b.attr(A_SOFTWARE, b"x");
        let raw = b.finish();
        assert_eq!(tcp_frame_len(&raw), Some(raw.len()));
        assert_eq!(tcp_frame_len(&raw[..3]), None);
        // ChannelData: 4 + длина, выровнено до 4
        assert_eq!(tcp_frame_len(&[0x40, 0x00, 0x00, 0x05, 1, 2, 3, 4, 5]), Some(12));
        assert_eq!(tcp_frame_len(&[0x7F, 0xFF, 0x00, 0x08]), Some(12));
        // не TURN: STUN без magic cookie, старший бит
        assert_eq!(tcp_frame_len(&[0x00, 0x01, 0x00, 0x00, 1, 2, 3, 4]), Some(0));
        assert_eq!(tcp_frame_len(&[0x80, 0, 0, 0]), Some(0));
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
