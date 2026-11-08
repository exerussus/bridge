//! Связь с соседями через свой сервер (3.20) — последний запасной путь, когда напрямую, через Radmin и по своей
//! сети (10.77) сосед не отвечает. Только исходящие соединения к серверу (TCP 443): не нужны ни права
//! администратора, ни адаптер, ни правила брандмауэра. Протокол и шифрование — `bridge_core::tunnel`.
//!
//! Принимающая сторона: канал управления (`run`) получает «к вам звонят», мост приходит за соединением и отдаёт его
//! своему HTTP моста (`router`) — как обычное входящее соединение соседа, только помеченное `Tunneled`
//! (адрес не проверяется — сосед доказывает себя подписью, как всегда).
//! Звонящая сторона: для соседа открывается порт на 127.0.0.1 (`url_for`); каждое соединение на него мост
//! переправляет соседу через сервер. Так весь остальной код ходит к соседу по обычному адресу `http://127.0.0.1:порт`.

use crate::node::Node;
use axum::Router;
use bridge_core::config::{PeerConfig, RelayConfig};
use bridge_core::signing;
use bridge_core::tunnel::{self as tn, CtlEvent, LinkInfo, Opener, Sealer, StreamCrypto, CHUNK};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// Пометка соединения, пришедшего через сервер: имя звонившего участника сервера.
#[derive(Clone, Debug)]
pub struct Tunneled(pub String);

/// Адрес, которым помечены такие соединения (ни петля, ни адрес соседа).
pub const TUNNEL_ADDR: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0);

#[derive(Default)]
pub struct Tunnel {
    /// Канал управления с сервером открыт — к этому мосту можно звонить.
    pub up: AtomicBool,
    /// Для какого «сервер|участник» открыт канал.
    pub ident: Mutex<String>,
    /// Порты на 127.0.0.1 для звонков: «сервер|участник соседа» → порт.
    ports: Mutex<HashMap<String, u16>>,
    app: OnceLock<Router>,
    me: OnceLock<std::sync::Weak<Node>>,
}

fn usable(rc: &RelayConfig) -> bool {
    rc.configured() && rc.link && rc.mode != "off"
}

fn ws_url(server: &str, pq: &str) -> Result<String, String> {
    let (host, port) = bridge_core::relay::split_addr(server)?;
    let h = if host.contains(':') { format!("[{}]", host) } else { host };
    Ok(format!("ws://{}:{}{}", h, port, pq))
}

/// WebSocket к серверу с подписью ключом участника.
async fn connect(node: &Node, rc: &RelayConfig, pq: &str) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, String> {
    let mut req = ws_url(&rc.server, pq)?.into_client_request().map_err(|e| e.to_string())?;
    let auth = format!("{} {}", bridge_core::AUTH_SCHEME, signing::header(&node.node_name(), &rc.secret, "GET", pq, &signing::empty_body_sha(), false));
    req.headers_mut().insert("authorization", auth.parse().map_err(|_| "подпись".to_string())?);
    let cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig { max_message_size: Some(1 << 20), max_frame_size: Some(1 << 20), ..Default::default() };
    let (ws, _) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async_with_config(req, Some(cfg), true))
        .await
        .map_err(|_| "сервер не ответил за 10 с".to_string())?
        .map_err(|e| match e {
            tokio_tungstenite::tungstenite::Error::Http(r) => format!("сервер ответил {}{}", r.status().as_u16(), r.body().as_ref().and_then(|b| String::from_utf8(b.clone()).ok()).map(|t| format!(": {}", t)).unwrap_or_default()),
            e => e.to_string(),
        })?;
    Ok(ws)
}

impl Tunnel {
    /// Что сообщать соседям в привете: «до меня можно через этот сервер».
    pub fn info(&self, node: &Node) -> Option<LinkInfo> {
        if !self.up.load(Ordering::Relaxed) {
            return None;
        }
        let rc = node.cfg.read().relay.clone();
        usable(&rc).then(|| LinkInfo { server: rc.server.clone(), member: rc.member.clone() })
    }

    /// Адрес соседа через сервер (`http://127.0.0.1:порт`), если до него так можно: оба на одном сервере.
    pub fn url_for(&self, node: &Node, peer: &PeerConfig) -> Option<String> {
        if !self.up.load(Ordering::Relaxed) {
            return None;
        }
        let rc = node.cfg.read().relay.clone();
        if !usable(&rc) {
            return None;
        }
        // имя соседа у сервера неизвестно — пробуем его имя узла: приглашение обычно на него и выписано
        // (не тот участник — он не знает ключ пары и соединение не примет)
        let (member, host) = if peer.relay_member.is_empty() { (peer.name.clone(), rc.server.clone()) } else { (peer.relay_member.clone(), peer.relay_host.clone()) };
        if !tn::same_server(&rc.server, &host) || member == rc.member {
            return None;
        }
        let peer = &PeerConfig { relay_member: member, ..peer.clone() };
        let key = format!("{}|{}|{}", rc.server, peer.relay_member, peer.key_id());
        if let Some(p) = self.ports.lock().get(&key) {
            return Some(format!("http://127.0.0.1:{}", p));
        }
        let n = self.me.get()?.upgrade()?;
        let l = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
        l.set_nonblocking(true).ok()?;
        let port = l.local_addr().ok()?.port();
        let l = tokio::net::TcpListener::from_std(l).ok()?;
        self.ports.lock().insert(key, port);
        let (name, member) = (peer.name.clone(), peer.relay_member.clone());
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = n.shutdown.cancelled() => break,
                    r = l.accept() => {
                        let Ok((s, _)) = r else { continue };
                        let _ = s.set_nodelay(true);
                        let (n2, name2, m2) = (n.clone(), name.clone(), member.clone());
                        tokio::spawn(async move {
                            if let Err(e) = dial(&n2, &name2, &m2, s).await {
                                n2.info_once(&format!("tunnel-dial|{}|{}", name2, e), &format!("Связь с «{}» через сервер: {}", name2, e), 1800);
                            }
                        });
                    }
                }
            }
        });
        Some(format!("http://127.0.0.1:{}", port))
    }

    /// Это адрес «через сервер»?
    pub fn is_tunnel_url(&self, url: &str) -> bool {
        let port = url.trim_start_matches("http://127.0.0.1:").trim_end_matches('/').parse::<u16>().ok();
        url.starts_with("http://127.0.0.1:") && port.map(|p| self.ports.lock().values().any(|x| *x == p)).unwrap_or(false)
    }

    fn app(&self, node: &Arc<Node>) -> Router {
        self.app.get_or_init(|| crate::http_peer::router(node.clone())).clone()
    }
}

/// Звонок: соединение с 127.0.0.1 → через сервер → сосед.
async fn dial(node: &Arc<Node>, peer_name: &str, member: &str, sock: tokio::net::TcpStream) -> Result<(), String> {
    let (rc, peer) = {
        let c = node.cfg.read();
        (c.relay.clone(), c.peers.iter().find(|p| p.name == peer_name).cloned())
    };
    let peer = peer.ok_or("соседа больше нет")?;
    let pq = format!("{}?to={}", tn::LINK_DIAL_PATH, crate::client::esc(member));
    let mut ws = connect(node, &rc, &pq).await?;
    let a: [u8; 32] = rand::random();
    ws.send(Message::Binary(tn::hello_dial(&peer.key_id(), &a))).await.map_err(|e| e.to_string())?;
    let b = match tokio::time::timeout(Duration::from_secs(tn::ACCEPT_WAIT_SECS + 5), ws.next()).await {
        Ok(Some(Ok(Message::Binary(x)))) => tn::parse_hello_accept(&x).ok_or("сосед ответил непонятно")?,
        Ok(_) => return Err("сосед не принял соединение (не на связи с сервером или ключ пары не совпал)".into()),
        Err(_) => return Err("сосед не ответил через сервер".into()),
    };
    let (seal, open) = StreamCrypto::new(&peer.secret, &a, &b, true).split();
    pump(sock, ws, seal, open).await;
    Ok(())
}

/// Гнать байты между обычным соединением и зашифрованным каналом, пока одна из сторон не закроется.
async fn pump<S, W>(sock: S, ws: W, mut seal: Sealer, mut open: Opener)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
    W: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + futures_util::Sink<Message> + Send + 'static,
{
    let (mut sr, mut sw) = tokio::io::split(sock);
    let (mut wt, mut wr) = ws.split();
    let up = async move {
        let mut buf = vec![0u8; CHUNK];
        loop {
            match sr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if wt.send(Message::Binary(seal.seal(&buf[..n]))).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = wt.send(Message::Close(None)).await;
    };
    let down = async move {
        while let Some(m) = wr.next().await {
            match m {
                Ok(Message::Binary(x)) => {
                    let Some(p) = open.open(&x) else { break };
                    if sw.write_all(&p).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Text(_)) | Ok(Message::Frame(_)) => {}
                _ => break,
            }
        }
        let _ = sw.shutdown().await;
    };
    // закрылась одна сторона — дочитать вторую недолго и закончить
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
}

/// К этому мосту звонят: забрать соединение у сервера и отдать его своему HTTP моста.
async fn accept(node: Arc<Node>, id: String, from: String) -> Result<(), String> {
    let rc = node.cfg.read().relay.clone();
    let pq = format!("{}?id={}", tn::LINK_ACCEPT_PATH, crate::client::esc(&id));
    let mut ws = connect(&node, &rc, &pq).await?;
    let first = match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
        Ok(Some(Ok(Message::Binary(x)))) => x,
        _ => return Err("звонящий ничего не прислал".into()),
    };
    let (kid, a) = tn::parse_hello_dial(&first).ok_or("непонятное начало соединения")?;
    let peer = node.cfg.read().peer_by_key(&kid).cloned();
    let Some(peer) = peer.filter(|p| p.enabled) else {
        let _ = ws.close(None).await;
        return Err(format!("звонок от «{}» с ключом, которого здесь нет", from));
    };
    let b: [u8; 32] = rand::random();
    ws.send(Message::Binary(tn::hello_accept(&b))).await.map_err(|e| e.to_string())?;
    let (seal, open) = StreamCrypto::new(&peer.secret, &a, &b, false).split();
    // соединение — своему HTTP моста, как обычное входящее, но с пометкой «через сервер»
    let (mine, theirs) = tokio::io::duplex(256 * 1024);
    let app = node.tunnel.app(&node).layer(axum::Extension(Tunneled(from.clone()))).layer(axum::Extension(axum::extract::ConnectInfo(TUNNEL_ADDR)));
    tokio::spawn(async move {
        let svc = hyper_util::service::TowerToHyperService::new(app);
        let _ = hyper::server::conn::http1::Builder::new().serve_connection(hyper_util::rt::TokioIo::new(theirs), svc).with_upgrades().await;
    });
    pump(mine, ws, seal, open).await;
    Ok(())
}

/// Канал управления: держать, пока сервер настроен и связь через него разрешена.
pub fn schedule(node: &Arc<Node>) {
    let _ = node.tunnel.me.set(Arc::downgrade(node));
    let n = node.clone();
    tokio::spawn(async move {
        let mut backoff = 5u64;
        loop {
            let rc = n.cfg.read().relay.clone();
            // сервер на связи и умеет связь мостов (3.20+); старому серверу не стучимся
            let server_ok = { let st = n.relay.status.lock(); st.ok && st.links == Some(true) };
            if usable(&rc) && server_ok {
                let ident = format!("{}|{}|{}", rc.server, rc.member, signing::key_id(&rc.secret));
                match ctl(&n, &rc, &ident).await {
                    Ok(()) => backoff = 5,
                    Err(e) => {
                        n.info_once(&format!("tunnel-ctl|{}", e), &format!("Связь с соседями через сервер: {}", e), 600);
                        backoff = (backoff * 2).min(120);
                    }
                }
                n.tunnel.up.store(false, Ordering::Relaxed);
            } else {
                backoff = 5;
            }
            tokio::select! {
                _ = n.shutdown.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
            }
        }
    });
}

async fn ctl(node: &Arc<Node>, rc: &RelayConfig, ident: &str) -> Result<(), String> {
    let mut ws = connect(node, rc, tn::LINK_CTL_PATH).await?;
    node.tunnel.up.store(true, Ordering::Relaxed);
    *node.tunnel.ident.lock() = ident.to_string();
    node.info_once(&format!("tunnel-up|{}", ident), "Связь с соседями через свой сервер — готова (если иначе до соседа не достучаться)", 3600);
    let mut check = tokio::time::interval(Duration::from_secs(15));
    let mut last = std::time::Instant::now();
    loop {
        tokio::select! {
            _ = node.shutdown.cancelled() => return Ok(()),
            _ = check.tick() => {
                // настройки сменились — переподключиться; сервер молчит дольше минуты — тоже
                let rc2 = node.cfg.read().relay.clone();
                let now = format!("{}|{}|{}", rc2.server, rc2.member, signing::key_id(&rc2.secret));
                if now != ident || !usable(&rc2) {
                    return Ok(());
                }
                if last.elapsed() > Duration::from_secs(70) {
                    return Err("сервер перестал отвечать".into());
                }
            }
            m = ws.next() => match m {
                Some(Ok(Message::Text(t))) => {
                    last = std::time::Instant::now();
                    let Ok(ev) = serde_json::from_str::<CtlEvent>(&t) else { continue };
                    if ev.t == "open" {
                        let n = node.clone();
                        tokio::spawn(async move {
                            let from = ev.from.clone();
                            if let Err(e) = accept(n.clone(), ev.id, ev.from).await {
                                n.info_once(&format!("tunnel-acc|{}|{}", from, e), &format!("Входящая связь через сервер от «{}»: {}", from, e), 120);
                            }
                        });
                    }
                }
                Some(Ok(_)) => { last = std::time::Instant::now(); }
                _ => return Err("сервер закрыл канал".into()),
            }
        }
    }
}
