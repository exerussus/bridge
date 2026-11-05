//! Сервер: своя локальная сеть MostNet. Адреса 10.77.x.y участникам, список соседей, пересылка пакетов между ПК
//! (UDP `netPort`, запасной путь — WebSocket на HTTP-порту). Пакеты зашифрованы от ПК до ПК — сервер их не читает.

use crate::{auth_member, load, log, save, Fail, Srv};
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use bridge_core::mostnet::{self, JoinReq, JoinResp, NetPeer, PeersResp, MTU, T_DATA, T_PING};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// Куда слать участнику: последний UDP-адрес и/или WebSocket.
struct End {
    member: String,
    udp: Option<(SocketAddr, Instant)>,
    ws: Option<(mpsc::Sender<Vec<u8>>, Instant)>,
}

#[derive(Default)]
pub struct NetState {
    /// Ограничение скорости: на участника — (запас байт, когда пополняли).
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
    tokens: Mutex<HashMap<[u8; 16], (String, Ipv4Addr)>>,
    ends: Mutex<HashMap<Ipv4Addr, End>>,
    udp: Mutex<Option<Arc<UdpSocket>>>,
}

impl NetState {
    pub fn drop_member(&self, name: &str) {
        self.tokens.lock().retain(|_, v| v.0 != name);
        self.ends.lock().retain(|_, e| e.member != name);
    }

    /// Адреса, с которых участник был на связи за последнюю минуту.
    pub fn online(&self) -> HashMap<String, &'static str> {
        let fresh = |t: &Instant| t.elapsed() < Duration::from_secs(60);
        self.ends
            .lock()
            .values()
            .filter_map(|e| {
                let u = e.udp.as_ref().map(|x| fresh(&x.1)).unwrap_or(false);
                let w = e.ws.as_ref().map(|x| fresh(&x.1) && !x.0.is_closed()).unwrap_or(false);
                (u || w).then(|| (e.member.clone(), if u { "udp" } else { "tcp" }))
            })
            .collect()
    }
}

fn peers_for(srv: &Srv, me: &str) -> Vec<NetPeer> {
    srv.cfg
        .read()
        .members
        .iter()
        .filter(|m| m.enabled && m.name != me && !m.vip.is_empty() && !m.net_key.is_empty())
        .map(|m| NetPeer { name: m.name.clone(), vip: m.vip.clone(), key: m.net_key.clone() })
        .collect()
}

/// Служба most-net подключается к сети: ключ ПК → адрес, токен, соседи.
pub async fn join(Extension(srv): Extension<Arc<Srv>>, headers: HeaderMap, body: Bytes) -> Result<Json<JoinResp>, Fail> {
    let m = auth_member(&srv, &headers, "POST", mostnet::JOIN_PATH, &bridge_core::path::sha256_bytes(&body))?;
    let req: JoinReq = serde_json::from_slice(&body).map_err(|_| Fail(StatusCode::BAD_REQUEST, "неверный запрос".into()))?;
    if mostnet::parse_key(&req.key).is_none() {
        return Err(Fail(StatusCode::BAD_REQUEST, "неверный ключ сети".into()));
    }
    let (vip, port) = {
        let _g = srv.join_lock.lock().await;
        let mut c = load(&srv.dir).map_err(|e| Fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        if c.net_port == 0 {
            return Err(Fail(StatusCode::FORBIDDEN, "своя сеть на сервере выключена (netPort: 0 в relay.json)".into()));
        }
        let used: Vec<String> = c.members.iter().map(|x| x.vip.clone()).collect();
        let me = c.members.iter_mut().find(|x| x.name == m.name).ok_or_else(|| Fail(StatusCode::FORBIDDEN, "участника нет".into()))?;
        if me.vip.is_empty() {
            let free = (1..0xFFFE).filter_map(mostnet::vip_of).find(|v| !used.contains(&v.to_string())).ok_or_else(|| Fail(StatusCode::INSUFFICIENT_STORAGE, "адреса кончились".into()))?;
            me.vip = free.to_string();
        }
        let changed = me.net_key != req.key;
        me.net_key = req.key.clone();
        let vip = me.vip.clone();
        let port = c.net_port;
        if changed || used.iter().all(|u| *u != vip) {
            save(&srv.dir, &c).map_err(|e| Fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        }
        *srv.cfg.write() = c;
        (vip, port)
    };
    let ip: Ipv4Addr = vip.parse().map_err(|_| Fail(StatusCode::INTERNAL_SERVER_ERROR, "адрес".into()))?;
    let tok: [u8; 16] = rand::random();
    {
        let mut t = srv.net.tokens.lock();
        t.retain(|_, v| v.0 != m.name);
        t.insert(tok, (m.name.clone(), ip));
    }
    log(&format!("сеть: «{}» подключился как {}", m.name, vip));
    Ok(Json(JoinResp { name: m.name.clone(), vip, prefix: mostnet::NET_PREFIX, token: hex::encode(tok), udp: port, peers: peers_for(&srv, &m.name) }))
}

pub async fn peers(Extension(srv): Extension<Arc<Srv>>, headers: HeaderMap) -> Result<Json<PeersResp>, Fail> {
    let m = auth_member(&srv, &headers, "GET", mostnet::PEERS_PATH, &bridge_core::signing::empty_body_sha())?;
    Ok(Json(PeersResp { peers: peers_for(&srv, &m.name) }))
}

enum Via {
    Udp(SocketAddr),
    Ws(mpsc::Sender<Vec<u8>>),
}

fn send_end(srv: &Srv, e: &End, frame: Vec<u8>) {
    let now = Duration::from_secs(30);
    let udp_ok = e.udp.as_ref().filter(|x| x.1.elapsed() < now);
    let ws_ok = e.ws.as_ref().filter(|x| !x.0.is_closed());
    // свежий UDP — им (быстрее); иначе — WebSocket
    match (udp_ok, ws_ok) {
        (Some((a, tu)), Some((w, tw))) if tw > tu => {
            let _ = w.try_send(frame);
            let _ = a;
        }
        (Some((a, _)), _) => {
            if let Some(s) = srv.net.udp.lock().clone() {
                let _ = s.try_send_to(&frame, *a);
            }
        }
        (None, Some((w, _))) => {
            let _ = w.try_send(frame);
        }
        _ => {}
    }
}

/// Кадр от ПК (по UDP или WebSocket).
fn on_up(srv: &Srv, b: &[u8], via: Via) {
    let Some((t, tok, to, data)) = mostnet::parse_up(b) else { return };
    let Some((name, me)) = srv.net.tokens.lock().get(&tok).cloned() else { return };
    let mbit = {
        let c = srv.cfg.read();
        if !c.members.iter().any(|m| m.name == name && m.enabled) {
            drop(c);
            srv.net.drop_member(&name);
            return;
        }
        c.net_mbit
    };
    // ограничение скорости на участника (ведро: запас — четверть секунды, не меньше 64 КБ); лишнее — выбросить
    if mbit > 0 && t == T_DATA {
        let rate = mbit as f64 * 125_000.0;
        let cap = (rate / 4.0).max(65_536.0);
        let mut g = srv.net.buckets.lock();
        let b = g.entry(name.clone()).or_insert((cap, Instant::now()));
        b.0 = (b.0 + b.1.elapsed().as_secs_f64() * rate).min(cap);
        b.1 = Instant::now();
        if b.0 < data.len() as f64 {
            return;
        }
        b.0 -= data.len() as f64;
    }
    {
        let mut ends = srv.net.ends.lock();
        let e = ends.entry(me).or_insert_with(|| End { member: name.clone(), udp: None, ws: None });
        e.member = name;
        match &via {
            Via::Udp(a) => e.udp = Some((*a, Instant::now())),
            Via::Ws(w) => e.ws = Some((w.clone(), Instant::now())),
        }
    }
    match t {
        T_PING => {
            let f = mostnet::frame_down(T_PING, me, &data[..data.len().min(64)]);
            match via {
                Via::Udp(a) => {
                    if let Some(s) = srv.net.udp.lock().clone() {
                        let _ = s.try_send_to(&f, a);
                    }
                }
                Via::Ws(w) => {
                    let _ = w.try_send(f);
                }
            }
        }
        T_DATA if data.len() <= MTU + 64 && to != me => {
            let f = mostnet::frame_down(T_DATA, me, data);
            let ends = srv.net.ends.lock();
            if let Some(e) = ends.get(&to) {
                send_end(srv, e, f);
            }
        }
        _ => {}
    }
}

pub async fn serve_udp(srv: Arc<Srv>, s: Arc<UdpSocket>) {
    *srv.net.udp.lock() = Some(s.clone());
    let mut buf = vec![0u8; 4096];
    loop {
        let r = tokio::select! {
            _ = srv.turn.stop.cancelled() => break,
            r = s.recv_from(&mut buf) => r,
        };
        let Ok((n, from)) = r else { continue };
        on_up(&srv, &buf[..n], Via::Udp(from));
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
pub struct WsQ {
    token: String,
}

/// Запасной путь, когда UDP до сервера не проходит: те же кадры по WebSocket (порт 443).
pub async fn ws(Extension(srv): Extension<Arc<Srv>>, Query(q): Query<WsQ>, up: WebSocketUpgrade) -> Result<Response, Fail> {
    let tok: [u8; 16] = hex::decode(&q.token).ok().and_then(|v| v.try_into().ok()).ok_or_else(|| Fail(StatusCode::BAD_REQUEST, "нет токена".into()))?;
    if !srv.net.tokens.lock().contains_key(&tok) {
        return Err(Fail(StatusCode::FORBIDDEN, "токен сети устарел — служба подключится заново".into()));
    }
    Ok(up.max_message_size(64 << 10).on_upgrade(move |s| ws_loop(srv, s)))
}

async fn ws_loop(srv: Arc<Srv>, mut sock: WebSocket) {
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(1024);
    loop {
        tokio::select! {
            _ = srv.turn.stop.cancelled() => break,
            m = rx.recv() => {
                let Some(f) = m else { break };
                if sock.send(Message::Binary(f)).await.is_err() {
                    break;
                }
            }
            m = sock.recv() => {
                match m {
                    Some(Ok(Message::Binary(b))) => on_up(&srv, &b, Via::Ws(tx.clone())),
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) | Some(Ok(Message::Text(_))) => {}
                    _ => break,
                }
            }
        }
    }
}
