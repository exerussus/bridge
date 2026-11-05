//! Работа сети: подключение к серверу, адаптер, шифрование для каждого соседа, пересылка, путь UDP или TCP 443.

use crate::tun::{Sink, Tun};
use bridge_core::mostnet::{self, JoinReq, JoinResp, NetKey, PeerCrypto, PeersResp, MTU, T_DATA, T_PING};
use bridge_core::signing;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Cfg {
    /// «адрес:порт» HTTP сервера (как у моста).
    pub server: String,
    pub member: String,
    pub secret: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerView {
    pub name: String,
    pub vip: String,
    /// Сколько секунд назад от него пришёл пакет (None — ещё не было).
    pub last_rx: Option<u64>,
    pub rx: u64,
    pub tx: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub version: String,
    /// off | connecting | on | error
    pub state: String,
    pub error: String,
    pub name: String,
    pub vip: String,
    pub server: String,
    /// udp | tcp
    pub path: String,
    pub rtt: Option<u64>,
    pub peers: Vec<PeerView>,
    pub configured: bool,
    pub enabled: bool,
}

struct Peer {
    name: String,
    key: String,
    crypto: PeerCrypto,
    rx: u64,
    tx: u64,
    last_rx: Option<Instant>,
}

pub struct Engine {
    pub dir: PathBuf,
    pub cfg: Mutex<Cfg>,
    pub st: Mutex<Status>,
    pub wake: Notify,
    key: NetKey,
    logs: Mutex<VecDeque<String>>,
    tun: Mutex<Option<Arc<Tun>>>,
    sink: Sink,
}

impl Engine {
    pub fn load(dir: PathBuf) -> Result<Arc<Engine>, String> {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        let kf = dir.join("net.key");
        let key = match std::fs::read(&kf) {
            Ok(b) if b.len() == 32 => NetKey::from_bytes(b.try_into().unwrap()),
            _ => {
                let k = NetKey::generate();
                std::fs::write(&kf, k.to_bytes()).map_err(|e| format!("{}: {}", kf.display(), e))?;
                k
            }
        };
        let cfg: Cfg = std::fs::read_to_string(dir.join("config.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Ok(Arc::new(Engine {
            dir,
            cfg: Mutex::new(cfg),
            st: Mutex::new(Status { version: env!("CARGO_PKG_VERSION").into(), state: "off".into(), ..Default::default() }),
            wake: Notify::new(),
            key,
            logs: Mutex::new(VecDeque::new()),
            tun: Mutex::new(None),
            sink: Arc::new(Mutex::new(None)),
        }))
    }

    pub fn log(&self, msg: &str) {
        let line = format!("{} {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
        println!("{}", line);
        let mut g = self.logs.lock();
        g.push_back(line.clone());
        while g.len() > 300 {
            g.pop_front();
        }
        drop(g);
        let f = self.dir.join("log.txt");
        if std::fs::metadata(&f).map(|m| m.len() > 1 << 20).unwrap_or(false) {
            let _ = std::fs::rename(&f, self.dir.join("log.old.txt"));
        }
        use std::io::Write;
        if let Ok(mut h) = std::fs::OpenOptions::new().create(true).append(true).open(&f) {
            let _ = writeln!(h, "{}", line);
        }
    }

    pub fn logs(&self) -> Vec<String> {
        self.logs.lock().iter().cloned().collect()
    }

    /// Новые настройки от моста: сохранить и, если что-то поменялось, переподключиться.
    pub fn set_cfg(&self, c: Cfg) -> Result<(), String> {
        let changed = *self.cfg.lock() != c;
        if !changed {
            return Ok(());
        }
        let tmp = self.dir.join("config.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&c).unwrap()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.dir.join("config.json")).map_err(|e| e.to_string())?;
        *self.cfg.lock() = c;
        self.wake.notify_one();
        Ok(())
    }

    pub fn status(&self) -> Status {
        let c = self.cfg.lock().clone();
        let mut s = self.st.lock().clone();
        s.configured = !c.server.is_empty() && !c.secret.is_empty();
        s.enabled = c.enabled;
        s.server = c.server;
        s
    }

    fn set_state(&self, state: &str, err: &str) {
        let mut s = self.st.lock();
        s.state = state.into();
        s.error = err.into();
        if state != "on" {
            s.path.clear();
            s.rtt = None;
        }
    }

    fn tun_for(self: &Arc<Self>, vip: Ipv4Addr) -> Result<Arc<Tun>, String> {
        let mut g = self.tun.lock();
        if let Some(t) = g.as_ref() {
            t.set_addr(vip, mostnet::NET_PREFIX)?;
            return Ok(t.clone());
        }
        let t = Tun::open(vip, mostnet::NET_PREFIX)?;
        t.start_reader(self.sink.clone());
        *g = Some(t.clone());
        Ok(t)
    }
}

/// Главный цикл: включено и настроено — держать сеанс, при сбое — пауза и снова.
pub async fn run(e: Arc<Engine>) {
    let mut backoff = 3u64;
    loop {
        let c = e.cfg.lock().clone();
        if !c.enabled || c.server.is_empty() || c.secret.is_empty() {
            e.set_state("off", "");
            e.st.lock().peers.clear();
            // выключено — адаптер убрать, чтобы в системе не висела сеть с адресом
            // (на Linux устройство TUN держит поток чтения — его не убрать, только опустить; это стенд проверки)
            let t = if cfg!(windows) { e.tun.lock().take() } else { e.tun.lock().clone() };
            if let Some(t) = t {
                t.close();
                if cfg!(windows) {
                    e.log("сеть выключена — адаптер MostNet убран");
                }
            }
            *e.sink.lock() = None;
            e.wake.notified().await;
            continue;
        }
        let stop = tokio_util::sync::CancellationToken::new();
        let res = tokio::select! {
            r = session(e.clone(), c.clone(), stop.clone()) => r,
            _ = e.wake.notified() => Ok(()),
        };
        stop.cancel();
        *e.sink.lock() = None;
        match res {
            Ok(()) => backoff = 3,
            Err(err) => {
                e.log(&format!("сеть: {}", err));
                e.set_state("error", &err);
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                    _ = e.wake.notified() => {}
                }
                backoff = (backoff * 2).min(60);
            }
        }
    }
}

fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())
}

fn auth(c: &Cfg, method: &str, path: &str, body: &[u8]) -> String {
    let sha = if body.is_empty() { signing::empty_body_sha() } else { bridge_core::path::sha256_bytes(body) };
    format!("{} {}", bridge_core::AUTH_SCHEME, signing::header(&c.member, &c.secret, method, path, &sha, false))
}

async fn answer<T: serde::de::DeserializeOwned>(r: reqwest::Response) -> Result<T, String> {
    let code = r.status();
    let text = r.text().await.map_err(|e| e.to_string())?;
    if !code.is_success() {
        let why = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or(text);
        return Err(format!("сервер отказал ({}): {}", code.as_u16(), why));
    }
    serde_json::from_str(&text).map_err(|_| "ответ не похож на most-relay 3.17+ — обновите сервер".to_string())
}

async fn fetch_peers(c: &Cfg) -> Result<PeersResp, String> {
    let (host, port) = bridge_core::relay::split_addr(&c.server)?;
    let url = format!("{}{}", bridge_core::relay::base_url(&host, port), mostnet::PEERS_PATH);
    let r = http()?.get(url).header("Authorization", auth(c, "GET", mostnet::PEERS_PATH, b"")).send().await.map_err(|e| e.to_string())?;
    answer(r).await
}

struct Link {
    udp: Arc<UdpSocket>,
    server_udp: SocketAddr,
    ws: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    use_ws: AtomicBool,
    token: [u8; 16],
}

impl Link {
    fn send(&self, frame: Vec<u8>) {
        if self.use_ws.load(Ordering::SeqCst) {
            if let Some(w) = self.ws.lock().as_ref() {
                let _ = w.try_send(frame);
            }
        } else {
            let _ = self.udp.try_send_to(&frame, self.server_udp);
        }
    }

    fn ping(&self, via_udp: bool) {
        let ts = (chrono::Utc::now().timestamp_millis() as u64).to_be_bytes();
        let f = mostnet::frame_up(T_PING, &self.token, Ipv4Addr::UNSPECIFIED, &ts);
        if via_udp {
            let _ = self.udp.try_send_to(&f, self.server_udp);
        } else if let Some(w) = self.ws.lock().as_ref() {
            let _ = w.try_send(f);
        }
    }
}

/// Кадр от сервера и каким путём пришёл.
type Down = (Vec<u8>, bool);

async fn connect_ws(host: &str, port: u16, token: &[u8; 16], down: mpsc::Sender<Down>, stop: tokio_util::sync::CancellationToken) -> Result<mpsc::Sender<Vec<u8>>, String> {
    let h = if host.contains(':') { format!("[{}]", host) } else { host.to_string() };
    let url = format!("ws://{}:{}{}?token={}", h, port, mostnet::WS_PATH, hex::encode(token));
    let (ws, _) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(url)).await.map_err(|_| "TCP 443 до сервера не отвечает".to_string())?.map_err(|e| format!("TCP 443: {}", e))?;
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(4096);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                m = rx.recv() => {
                    let Some(f) = m else { break };
                    if sink.send(Message::Binary(f)).await.is_err() { break; }
                }
                m = stream.next() => {
                    match m {
                        Some(Ok(Message::Binary(b))) => { let _ = down.try_send((b, true)); }
                        Some(Ok(_)) => {}
                        _ => break,
                    }
                }
            }
        }
    });
    Ok(tx)
}

fn sync_peers(e: &Engine, peers: &mut HashMap<Ipv4Addr, Peer>, list: Vec<mostnet::NetPeer>) {
    let mut keep = Vec::new();
    for p in list {
        let (Ok(ip), Some(k)) = (p.vip.parse::<Ipv4Addr>(), mostnet::parse_key(&p.key)) else { continue };
        keep.push(ip);
        let fresh = peers.get(&ip).map(|x| x.key != p.key).unwrap_or(true);
        if fresh {
            if !peers.contains_key(&ip) {
                e.log(&format!("сеть: сосед «{}» — {}", p.name, ip));
            }
            peers.insert(ip, Peer { name: p.name.clone(), key: p.key.clone(), crypto: PeerCrypto::new(&e.key, k), rx: 0, tx: 0, last_rx: None });
        } else if let Some(x) = peers.get_mut(&ip) {
            x.name = p.name;
        }
    }
    peers.retain(|ip, p| {
        let ok = keep.contains(ip);
        if !ok {
            e.log(&format!("сеть: «{}» больше не в сети", p.name));
        }
        ok
    });
}

fn publish_peers(e: &Engine, peers: &HashMap<Ipv4Addr, Peer>) {
    let mut v: Vec<PeerView> = peers
        .iter()
        .map(|(ip, p)| PeerView { name: p.name.clone(), vip: ip.to_string(), last_rx: p.last_rx.map(|t| t.elapsed().as_secs()), rx: p.rx, tx: p.tx })
        .collect();
    v.sort_by(|a, b| a.vip.cmp(&b.vip));
    e.st.lock().peers = v;
}

async fn session(e: Arc<Engine>, c: Cfg, stop: tokio_util::sync::CancellationToken) -> Result<(), String> {
    e.set_state("connecting", "");
    let (host, http_port) = bridge_core::relay::split_addr(&c.server)?;
    // 1. адрес, токен, соседи
    let body = serde_json::to_vec(&JoinReq { key: e.key.public_b64() }).unwrap();
    let url = format!("{}{}", bridge_core::relay::base_url(&host, http_port), mostnet::JOIN_PATH);
    let r = http()?
        .post(url)
        .header("Authorization", auth(&c, "POST", mostnet::JOIN_PATH, &body))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("сервер {} не отвечает: {}", c.server, e))?;
    let jr: JoinResp = answer(r).await?;
    let vip: Ipv4Addr = jr.vip.parse().map_err(|_| "сервер прислал неверный адрес".to_string())?;
    let token: [u8; 16] = hex::decode(&jr.token).ok().and_then(|v| v.try_into().ok()).ok_or("сервер прислал неверный токен")?;
    // 2. адаптер
    let tun = e.tun_for(vip)?;
    {
        let mut s = e.st.lock();
        s.vip = jr.vip.clone();
        s.name = jr.name.clone();
    }
    let mut peers: HashMap<Ipv4Addr, Peer> = HashMap::new();
    sync_peers(&e, &mut peers, jr.peers);
    // 3. путь до сервера: UDP, если отвечает за 3 с; иначе TCP 443 (WebSocket)
    let udp = Arc::new(UdpSocket::bind("0.0.0.0:0").await.map_err(|e| e.to_string())?);
    let server_udp = tokio::net::lookup_host((host.as_str(), jr.udp)).await.map_err(|e| e.to_string())?.next().ok_or("адрес сервера")?;
    let link = Arc::new(Link { udp: udp.clone(), server_udp, ws: Mutex::new(None), use_ws: AtomicBool::new(false), token });
    let (down_tx, mut down_rx) = mpsc::channel::<Down>(8192);
    {
        let (u, d, s) = (udp.clone(), down_tx.clone(), stop.clone());
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                tokio::select! {
                    _ = s.cancelled() => break,
                    r = u.recv_from(&mut buf) => {
                        if let Ok((n, _)) = r { let _ = d.try_send((buf[..n].to_vec(), false)); }
                    }
                }
            }
        });
    }
    let last_pong_udp = Arc::new(AtomicU64::new(0));
    let mut got_udp = false;
    for _ in 0..3 {
        link.ping(true);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while let Ok(Some((f, false))) = tokio::time::timeout_at(deadline, down_rx.recv()).await {
            if matches!(mostnet::parse_down(&f), Some((T_PING, _, _))) {
                got_udp = true;
                break;
            }
        }
        if got_udp {
            break;
        }
    }
    let mut last_pong = Instant::now();
    if !got_udp {
        e.log("сеть: UDP до сервера не проходит — работаю через TCP 443");
        let w = connect_ws(&host, http_port, &token, down_tx.clone(), stop.clone()).await?;
        *link.ws.lock() = Some(w);
        link.use_ws.store(true, Ordering::SeqCst);
        link.ping(false);
    }
    let path = if got_udp { "udp" } else { "tcp" };
    {
        let mut s = e.st.lock();
        s.state = "on".into();
        s.error.clear();
        s.path = path.into();
    }
    e.log(&format!("сеть: подключено как {} ({}), соседей: {}, путь: {}", vip, jr.name, peers.len(), if got_udp { "UDP" } else { "TCP 443" }));
    publish_peers(&e, &peers);
    // 4. пакеты адаптера → сюда
    let (pk_tx, mut pk_rx) = mpsc::channel::<Vec<u8>>(8192);
    *e.sink.lock() = Some(pk_tx);
    let (peers_tx, mut peers_rx) = mpsc::channel::<Result<PeersResp, String>>(2);
    let mut tick = tokio::time::interval(Duration::from_secs(10));
    let mut n_tick = 0u64;
    // список соседей — раз в 30 с, а при незнакомом адресе — сразу (не чаще раза в 3 с)
    let mut last_fetch = Instant::now();
    let fetch_now = |last: &mut Instant| {
        if last.elapsed() > Duration::from_secs(3) {
            *last = Instant::now();
            let (c2, tx) = (c.clone(), peers_tx.clone());
            tokio::spawn(async move { let _ = tx.send(fetch_peers(&c2).await).await; });
        }
    };
    loop {
        tokio::select! {
            _ = stop.cancelled() => return Ok(()),
            p = pk_rx.recv() => {
                let Some(p) = p else { return Ok(()) };
                let Some((_, dst)) = mostnet::ipv4_addrs(&p) else { continue };
                if p.len() > MTU || dst == vip {
                    continue;
                }
                if mostnet::is_broadcast(dst) {
                    for (ip, peer) in peers.iter_mut() {
                        let sealed = peer.crypto.seal(&p);
                        peer.tx += p.len() as u64;
                        link.send(mostnet::frame_up(T_DATA, &token, *ip, &sealed));
                    }
                } else if let Some(peer) = peers.get_mut(&dst) {
                    let sealed = peer.crypto.seal(&p);
                    peer.tx += p.len() as u64;
                    link.send(mostnet::frame_up(T_DATA, &token, dst, &sealed));
                } else if mostnet::in_net(dst) {
                    fetch_now(&mut last_fetch);
                }
            }
            f = down_rx.recv() => {
                let Some((f, via_ws)) = f else { return Err("канал до сервера закрыт".into()) };
                let Some((t, from, data)) = mostnet::parse_down(&f) else { continue };
                match t {
                    T_PING => {
                        let sent = data.get(..8).map(|b| u64::from_be_bytes(b.try_into().unwrap())).unwrap_or(0);
                        let rtt = (chrono::Utc::now().timestamp_millis() as u64).saturating_sub(sent);
                        if !via_ws {
                            last_pong_udp.store(chrono::Utc::now().timestamp() as u64, Ordering::SeqCst);
                            if link.use_ws.swap(false, Ordering::SeqCst) {
                                e.log("сеть: UDP до сервера снова проходит — перешёл на UDP");
                                e.st.lock().path = "udp".into();
                            }
                        }
                        if via_ws == link.use_ws.load(Ordering::SeqCst) {
                            last_pong = Instant::now();
                            e.st.lock().rtt = Some(rtt);
                        }
                    }
                    T_DATA => {
                        let Some(peer) = peers.get_mut(&from) else {
                            fetch_now(&mut last_fetch);
                            continue;
                        };
                        let Some(p) = peer.crypto.open(data) else { continue };
                        // внутри — пакет именно от этого соседа
                        if mostnet::ipv4_addrs(&p).map(|(s, _)| s) != Some(from) {
                            continue;
                        }
                        peer.rx += p.len() as u64;
                        peer.last_rx = Some(Instant::now());
                        tun.write(&p);
                    }
                    _ => {}
                }
            }
            r = peers_rx.recv() => {
                match r {
                    Some(Ok(pr)) => { sync_peers(&e, &mut peers, pr.peers); publish_peers(&e, &peers); }
                    Some(Err(err)) => {
                        if err.contains("(401)") || err.contains("(403)") {
                            return Err(err);
                        }
                    }
                    None => {}
                }
            }
            _ = tick.tick() => {
                n_tick += 1;
                let on_ws = link.use_ws.load(Ordering::SeqCst);
                link.ping(!on_ws);
                if on_ws && n_tick % 3 == 0 {
                    link.ping(true); // вдруг UDP заработал
                }
                publish_peers(&e, &peers);
                if n_tick % 3 == 0 {
                    last_fetch = Instant::now() - Duration::from_secs(10);
                    fetch_now(&mut last_fetch);
                }
                if last_pong.elapsed() > Duration::from_secs(35) {
                    if !on_ws {
                        e.log("сеть: UDP до сервера пропал — перехожу на TCP 443");
                        let w = connect_ws(&host, http_port, &token, down_tx.clone(), stop.clone()).await?;
                        *link.ws.lock() = Some(w);
                        link.use_ws.store(true, Ordering::SeqCst);
                        e.st.lock().path = "tcp".into();
                        link.ping(false);
                        last_pong = Instant::now();
                    } else {
                        return Err("сервер не отвечает".into());
                    }
                }
            }
        }
    }
}
