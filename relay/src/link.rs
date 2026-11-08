//! Сервер: связь мостов через сервер (3.20). Мосты держат канал управления (`LINK_CTL_PATH`); звонок (`LINK_DIAL_PATH`)
//! сервер передаёт вызванному мосту, тот приходит на `LINK_ACCEPT_PATH`, и два WebSocket склеиваются. Содержимое
//! зашифровано ключом пары соседей — сервер его не читает (см. `bridge_core::tunnel`).

use crate::{auth_member, log, Fail, Srv};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, Query};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use bridge_core::signing;
use bridge_core::tunnel::{CtlEvent, ACCEPT_WAIT_SECS};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// Не больше стольких открытых каналов у одного участника.
const MAX_PER_MEMBER: usize = 64;

struct Pending {
    to: String,
    tx: oneshot::Sender<WebSocket>,
    at: Instant,
}

#[derive(Default)]
pub struct LinkState {
    /// Канал управления каждого участника: (номер подключения, куда слать события).
    ctl: Mutex<HashMap<String, (u64, mpsc::Sender<String>)>>,
    gen: AtomicU64,
    pending: Mutex<HashMap<String, Pending>>,
    /// Открытые каналы по участникам (звонящий и вызванный).
    active: Mutex<HashMap<String, usize>>,
    /// Трафик через связь мостов по участникам, байт.
    pub bytes: Mutex<HashMap<String, u64>>,
}

impl LinkState {
    /// Участник отозван — его канал управления закрыть.
    pub fn drop_member(&self, name: &str) {
        self.ctl.lock().remove(name);
    }

    /// Кто сейчас держит канал управления.
    pub fn online(&self) -> Vec<String> {
        self.ctl.lock().keys().cloned().collect()
    }
}

fn pq(uri: &Uri) -> String {
    uri.path_and_query().map(|x| x.as_str().to_string()).unwrap_or_else(|| uri.path().to_string())
}

fn auth(srv: &Srv, headers: &HeaderMap, uri: &Uri) -> Result<crate::Member, Fail> {
    auth_member(srv, headers, "GET", &pq(uri), &signing::empty_body_sha())
}

/// Канал управления: сюда сервер сообщает «к вам звонят».
pub async fn ctl(Extension(srv): Extension<Arc<Srv>>, headers: HeaderMap, uri: Uri, up: WebSocketUpgrade) -> Result<Response, Fail> {
    let m = auth(&srv, &headers, &uri)?;
    Ok(up.max_message_size(64 << 10).on_upgrade(move |s| ctl_loop(srv, m.name, s)))
}

async fn ctl_loop(srv: Arc<Srv>, name: String, mut sock: WebSocket) {
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let g = srv.link.gen.fetch_add(1, Ordering::SeqCst);
    let fresh = srv.link.ctl.lock().insert(name.clone(), (g, tx)).is_none();
    if fresh {
        log(&format!("связь мостов: «{}» на связи", name));
    }
    let mut ping = tokio::time::interval(Duration::from_secs(25));
    loop {
        tokio::select! {
            _ = srv.turn.stop.cancelled() => break,
            _ = ping.tick() => {
                let ev = serde_json::to_string(&CtlEvent { t: "ping".into(), ..Default::default() }).unwrap();
                if sock.send(Message::Text(ev)).await.is_err() { break; }
            }
            m = rx.recv() => {
                let Some(ev) = m else { break };
                if sock.send(Message::Text(ev)).await.is_err() { break; }
            }
            m = sock.recv() => {
                match m {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
    let mut c = srv.link.ctl.lock();
    if c.get(&name).map(|x| x.0) == Some(g) {
        c.remove(&name);
        drop(c);
        log(&format!("связь мостов: «{}» отключился", name));
    }
}

#[derive(serde::Deserialize)]
pub struct DialQ {
    to: String,
}

#[derive(serde::Deserialize)]
pub struct AcceptQ {
    id: String,
}

fn busy(srv: &Srv, name: &str) -> bool {
    srv.link.active.lock().get(name).copied().unwrap_or(0) >= MAX_PER_MEMBER
}

/// Звонок соседу: ждём, пока он придёт за соединением, и склеиваем.
pub async fn dial(Extension(srv): Extension<Arc<Srv>>, headers: HeaderMap, uri: Uri, Query(q): Query<DialQ>, up: WebSocketUpgrade) -> Result<Response, Fail> {
    let m = auth(&srv, &headers, &uri)?;
    let target_ok = srv.cfg.read().members.iter().any(|x| x.name == q.to && x.enabled);
    if !target_ok {
        return Err(Fail(StatusCode::NOT_FOUND, format!("участника «{}» на сервере нет", q.to)));
    }
    let Some(ctl) = srv.link.ctl.lock().get(&q.to).map(|x| x.1.clone()) else {
        return Err(Fail(StatusCode::NOT_FOUND, format!("«{}» сейчас не на связи с сервером", q.to)));
    };
    if busy(&srv, &m.name) || busy(&srv, &q.to) {
        return Err(Fail(StatusCode::TOO_MANY_REQUESTS, "слишком много открытых соединений".into()));
    }
    let id = hex::encode(rand::random::<[u8; 16]>());
    let (tx, rx) = oneshot::channel::<WebSocket>();
    {
        let mut p = srv.link.pending.lock();
        p.retain(|_, v| v.at.elapsed() < Duration::from_secs(ACCEPT_WAIT_SECS + 5));
        p.insert(id.clone(), Pending { to: q.to.clone(), tx, at: Instant::now() });
    }
    let ev = serde_json::to_string(&CtlEvent { t: "open".into(), id: id.clone(), from: m.name.clone() }).unwrap();
    if ctl.try_send(ev).is_err() {
        srv.link.pending.lock().remove(&id);
        return Err(Fail(StatusCode::SERVICE_UNAVAILABLE, format!("«{}» не принимает соединения", q.to)));
    }
    let (from, to) = (m.name, q.to);
    Ok(up.max_message_size(1 << 20).on_upgrade(move |a| async move {
        let other = tokio::time::timeout(Duration::from_secs(ACCEPT_WAIT_SECS), rx).await;
        srv.link.pending.lock().remove(&id);
        if let Ok(Ok(b)) = other {
            splice(srv, a, b, from, to).await;
        }
    }))
}

/// Вызванный мост пришёл за соединением.
pub async fn accept(Extension(srv): Extension<Arc<Srv>>, headers: HeaderMap, uri: Uri, Query(q): Query<AcceptQ>, up: WebSocketUpgrade) -> Result<Response, Fail> {
    let m = auth(&srv, &headers, &uri)?;
    let p = srv.link.pending.lock().remove(&q.id);
    let Some(p) = p else { return Err(Fail(StatusCode::GONE, "звонок уже не ждёт".into())) };
    if p.to != m.name {
        return Err(Fail(StatusCode::FORBIDDEN, "звонок не вам".into()));
    }
    let tx = p.tx;
    Ok(up.max_message_size(1 << 20).on_upgrade(move |b| async move {
        let _ = tx.send(b);
    }))
}

fn count(srv: &Srv, who: &[&String], delta: isize) {
    let mut a = srv.link.active.lock();
    for w in who {
        let e = a.entry((*w).clone()).or_insert(0);
        *e = (*e as isize + delta).max(0) as usize;
    }
}

async fn splice(srv: Arc<Srv>, a: WebSocket, b: WebSocket, from: String, to: String) {
    count(&srv, &[&from, &to], 1);
    let (mut aw, mut ar) = a.split();
    let (mut bw, mut br) = b.split();
    let mut sent = 0u64;
    let mut got = 0u64;
    let stop = srv.turn.stop.clone();
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            m = ar.next() => match m {
                Some(Ok(Message::Binary(x))) => { sent += x.len() as u64; if bw.send(Message::Binary(x)).await.is_err() { break; } }
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) | Some(Ok(Message::Text(_))) => {}
                _ => break,
            },
            m = br.next() => match m {
                Some(Ok(Message::Binary(x))) => { got += x.len() as u64; if aw.send(Message::Binary(x)).await.is_err() { break; } }
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) | Some(Ok(Message::Text(_))) => {}
                _ => break,
            },
        }
    }
    let _ = aw.send(Message::Close(None)).await;
    let _ = bw.send(Message::Close(None)).await;
    count(&srv, &[&from, &to], -1);
    let mut by = srv.link.bytes.lock();
    *by.entry(from).or_insert(0) += sent + got;
    *by.entry(to).or_insert(0) += sent + got;
}
