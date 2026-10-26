//! HTTP узла: пропуск запросов (соседи — по ключу и адресу, локальный API — только с этого ПК и с токеном) и запросы соседей.

use crate::link;
use crate::node::Node;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Path as AxPath, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bridge_core::config::{canon_ip, PeerConfig};
use bridge_core::proto::*;
use bridge_core::{eq_ic, path, signing, time, API_PREFIX, AUTH_SCHEME, LOCAL_TOKEN_HEADER};
use futures_util::StreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

pub type St = State<Arc<Node>>;

/// Кто постучался (после пропуска).
#[derive(Clone)]
pub struct PeerCtx {
    pub name: String,
    pub body: Bytes,
}

pub fn err(code: u16, text: impl Into<String>) -> Response {
    let mut r = Json(ErrorResponse { error: text.into() }).into_response();
    *r.status_mut() = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    r
}

pub fn ok_json<T: serde::Serialize>(v: &T) -> Response {
    let body = serde_json::to_vec(v).unwrap_or_default();
    let mut r = Response::new(Body::from(body));
    r.headers_mut().insert("Content-Type", HeaderValue::from_static("application/json; charset=utf-8"));
    r
}

pub fn json_code<T: serde::Serialize>(code: u16, v: &T) -> Response {
    let mut r = ok_json(v);
    *r.status_mut() = StatusCode::from_u16(code).unwrap_or(StatusCode::OK);
    r
}

pub fn is_loopback(ip: Option<IpAddr>) -> bool {
    ip.map(|i| canon_ip(i).is_loopback()).unwrap_or(false)
}

/// Хост запроса — 127.0.0.1, localhost или [::1] (иначе это может быть подмена DNS со сторонней страницы).
pub fn host_is_local(h: &HeaderMap) -> bool {
    let host = h.get("host").and_then(|v| v.to_str().ok()).unwrap_or("");
    let name = if host.starts_with('[') { host.split(']').next().unwrap_or("").trim_start_matches('[').to_string() } else { host.split(':').next().unwrap_or("").to_string() };
    matches!(name.as_str(), "127.0.0.1" | "localhost" | "::1")
}

fn remember_stranger(node: &Node, name: &str, ip: &str, why: &str, kid: &str) {
    let mut g = node.strangers.lock();
    if g.len() > 50 {
        g.clear();
    }
    g.insert(
        format!("{}|{}", name, ip),
        crate::node::StrangerInfo { name: name.into(), ip: ip.into(), why: why.into(), kid: kid.into(), time: time::now() },
    );
}

async fn gate(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    let ip = Some(canon_ip(addr.ip()));
    let path_str = req.uri().path().to_string();
    let p = API_PREFIX;
    // Телефон: своя страница и свой ключ (cookie), с любого адреса. Менять что-то — только со своей страницы (заголовок).
    if path_str == "/m" || path_str.starts_with("/m/") {
        if path_str == "/m/" && req.method() == axum::http::Method::GET {
            let who = req.headers().get("x-forwarded-for").and_then(|v| v.to_str().ok()).map(String::from).unwrap_or_else(|| ip.map(|i| i.to_string()).unwrap_or_default());
            node.info_once(&format!("m|{}", who), &format!("Телефон: открыта страница моста с {}", who), 3600);
        }
        if req.method() != axum::http::Method::GET && req.method() != axum::http::Method::HEAD && path_str != "/m/share" && !req.headers().contains_key(crate::mobile::CSRF_HEADER) {
            return err(403, "запрос не со страницы моста");
        }
        return next.run(req).await;
    }
    // Через прокси (Tailscale Serve и т. п.) — только страница телефона: это не «с этого ПК», даже с 127.0.0.1
    if crate::mobile::proxied(req.headers()) && path_str != format!("{}/health", p) {
        return err(403, "через прокси доступна только страница телефона (/m/)");
    }
    let loopback = is_loopback(ip);
    if path_str.starts_with(&format!("{}/peer/", p)) {
        return peer_gate(node, ip, req, next).await;
    }
    // Остальное — только с 127.0.0.1 или с адресов соседей
    if !loopback && !node.cfg.read().peers.iter().any(|x| x.enabled && x.allows_ip(ip)) {
        node.info_once(&format!("ip|{}", addr.ip()), &format!("Отказ: запрос с адреса {} — адреса нет в белом списке — {}", addr.ip(), path_str), 10);
        return err(403, format!("адрес {} не в белом списке узла «{}»", addr.ip(), node.node_name()));
    }
    if path_str.starts_with(&format!("{}/local", p)) {
        if !loopback || !host_is_local(req.headers()) {
            return StatusCode::FORBIDDEN.into_response();
        }
        let token = req.headers().get(LOCAL_TOKEN_HEADER).and_then(|v| v.to_str().ok()).map(String::from).or_else(|| {
            // для потоков событий (EventSource не умеет заголовки) — токен в запросе
            req.uri().query().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=").map(String::from)))
        });
        if token.as_deref() != Some(node.local_token.as_str()) {
            return err(401, format!("нужен локальный токен ({})", LOCAL_TOKEN_HEADER));
        }
        // пока этим ПК управляют по сети — мост заперт: ничего, что меняет права, папки, соседей и настройки
        if req.method() != axum::http::Method::GET && node.remote.active().is_some() {
            let rest = path_str.trim_start_matches(&format!("{}/local/", p));
            if !crate::remote::allowed_while_controlled(rest) {
                let who = node.remote.active().map(|g| g.peer).unwrap_or_default();
                node.info_once(&format!("lock|{}", rest), &format!("Защита: «{}» отклонено — вашим ПК управляет {}, мост заперт до конца управления", rest, who), 10);
                return err(423, format!("мост заперт, пока вашим ПК управляет {} — права, папки, соседи и настройки сейчас не меняются", who));
            }
        }
        return next.run(req).await;
    }
    if path_str == format!("{}/health", p) {
        return next.run(req).await;
    }
    // приложения: файлы интерфейсов (для изолированной рамки) и API служб по токену приложения — только с этого ПК
    if path_str.starts_with("/apps/") || path_str.starts_with("/most/") || path_str.starts_with(&format!("{}/app/", p)) {
        if !loopback || !host_is_local(req.headers()) {
            return StatusCode::FORBIDDEN.into_response();
        }
        // пока этим ПК управляют, службы приложений ничего не меняют
        if path_str.starts_with(&format!("{}/app/", p)) && req.method() != axum::http::Method::GET && node.remote.active().is_some() {
            return err(423, "мост заперт, пока этим ПК управляют по сети");
        }
        return next.run(req).await;
    }
    if path_str == "/" || path_str == "/ui" || path_str == "/favicon.svg" || path_str.starts_with("/assets/") || path_str.starts_with("/app/") {
        if !loopback || !host_is_local(req.headers()) {
            return StatusCode::FORBIDDEN.into_response();
        }
        return next.run(req).await;
    }
    StatusCode::NOT_FOUND.into_response()
}

/// Пропуск запроса соседа: подпись → сосед по отпечатку ключа (у узлов до 1.4 — по имени) → включён → подпись верна →
/// адрес в белом списке (или сосед сам сообщил его в подписанном hello) → дальше. Каждый отказ — с понятной причиной.
async fn peer_gate(node: Arc<Node>, ip: Option<IpAddr>, req: Request, next: Next) -> Response {
    let ip_text = ip.map(|i| i.to_string()).unwrap_or_default();
    let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let parsed = auth.strip_prefix(&format!("{} ", AUTH_SCHEME)).and_then(signing::parse);
    let Some(parsed) = parsed else {
        let listed = is_loopback(ip) || node.cfg.read().peers.iter().any(|p| p.allows_ip(ip));
        node.info_once(&format!("nosig|{}", ip_text), &format!("Отказ: запрос с {} без подписи моста — {}", ip_text, req.uri().path()), 10);
        return if listed { err(401, "нужна подпись соседа") } else { StatusCode::FORBIDDEN.into_response() };
    };
    let claimed = if bridge_core::config::is_name(&parsed.node) { parsed.node.clone() } else { "?".into() };
    let me = node.node_name();
    let peer: Option<PeerConfig> = {
        let c = node.cfg.read();
        if parsed.kid.is_empty() { c.peer(&parsed.node).cloned() } else { c.peer_by_key(&parsed.kid).cloned() }
    };
    let Some(peer) = peer else {
        let (short, reason) = if !parsed.kid.is_empty() {
            ("ключ не подходит".to_string(), format!("у «{}» нет соседа с таким ключом: ключ на двух ПК должен быть одинаковым. Скопируйте ключ ещё раз (или подключите заново по адресу и ключу)", me))
        } else {
            (
                "нет соседа с таким именем (сосед старой версии)".to_string(),
                format!("узел «{}» здесь не знают: у «{}» нет соседа с таким именем. Обновите мост на «{}» — тогда имя не важно, узнаётся по ключу", claimed, me, claimed),
            )
        };
        remember_stranger(&node, &claimed, &ip_text, &short, &parsed.kid);
        node.info_once(&format!("unknown|{}|{}", claimed, ip_text), &format!("Отказ: «{}» с {} — {}", claimed, ip_text, short), 10);
        return err(401, reason);
    };
    if !peer.enabled {
        node.with_peer_state(&peer.name, |s| s.inbound_error = "выключен здесь — его запросы не принимаются".into());
        node.info_once(&format!("off|{}", peer.name), &format!("Отказ: «{}» выключен — запросы с {} не принимаются", peer.name, ip_text), 10);
        return err(401, format!("сосед выключен у «{}» (кнопка «Включить» в его панели)", me));
    }
    let method = req.method().clone();
    let pq = req.uri().path_and_query().map(|x| x.as_str().to_string()).unwrap_or_default();
    let path_str = req.uri().path().to_string();
    let query: HashMap<String, String> = req.uri().query().map(parse_query).unwrap_or_default();
    let stream = method == axum::http::Method::PUT && path_str.starts_with(&format!("{}/peer/folders/", API_PREFIX)) && path_str.ends_with("/file");
    let ip_known = peer.allows_ip(ip) || (is_loopback(ip) && peer.ips.iter().any(|x| x == "127.0.0.1"));
    let (parts, body) = req.into_parts();
    let mut bytes = Bytes::new();
    let body_sha;
    let mut pass_body = Body::empty();
    if stream {
        let sha = query.get("sha").cloned().unwrap_or_default();
        if sha.len() != 64 {
            return err(400, "нужна sha");
        }
        body_sha = sha;
        pass_body = body;
    } else {
        if ip_known {
            match axum::body::to_bytes(body, 1024 * 1024).await {
                Ok(b) => bytes = b,
                Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
            }
        }
        body_sha = path::sha256_bytes(&bytes);
    }
    if let Err(why) = signing::verify(&parsed, &peer.secret, method.as_str(), &pq, &body_sha, &node.nonces) {
        node.with_peer_state(&peer.name, |s| s.inbound_error = why.clone());
        node.info_once(&format!("sig|{}|{}", peer.name, why), &format!("Отказ: «{}» с {} — {}", peer.name, ip_text, why), 10);
        return err(401, why);
    }
    // Ключ и подпись верны — это точно этот сосед. Теперь адрес.
    if !ip_known {
        let listen = if path_str == format!("{}/peer/hello", API_PREFIX) { query.get("listen").cloned().unwrap_or_default() } else { String::new() };
        if !link::learn_address(&node, &peer.name, ip, &listen) {
            let why = format!(
                "адрес {} не записан у «{}» для соседа «{}» (записаны: {})",
                ip_text,
                me,
                peer.name,
                if peer.ips.is_empty() { "нет".to_string() } else { peer.ips.join(", ") }
            );
            node.with_peer_state(&peer.name, |s| s.inbound_error = format!("пришёл с незнакомого адреса {}", ip_text));
            remember_stranger(&node, &peer.name, &ip_text, "адрес не в списке соседа", &parsed.kid);
            node.info_once(
                &format!("ip|{}|{}", peer.name, ip_text),
                &format!("Отказ: «{}» пришёл с адреса {}, которого нет в его белом списке (ключ верный — адрес можно добавить в панели)", peer.name, ip_text),
                10,
            );
            return err(403, format!("{}. Добавьте адрес в панели «{}» → «Соседи» → «Стучались» → «Добавить адрес»", why, me));
        }
    }
    if !parsed.kid.is_empty() {
        link::learn_name(&node, &peer.name, &parsed.node);
    }
    // После learn_name сосед мог переименоваться
    let name = node.cfg.read().peer_by_key(&peer.key_id()).map(|p| p.name.clone()).unwrap_or(peer.name.clone());
    node.with_peer_state(&name, |s| {
        s.last_inbound = Some(time::now());
        s.inbound_error.clear();
        s.inbound_legacy = parsed.kid.is_empty();
    });
    let mut req = Request::from_parts(parts, if stream { pass_body } else { Body::from(bytes.clone()) });
    req.extensions_mut().insert(PeerCtx { name, body: bytes });
    next.run(req).await
}

pub fn parse_query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (url_decode(k), url_decode(v))
        })
        .collect()
}

pub fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if b[i] == b'%' && i + 2 < b.len() {
            match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|x| u8::from_str_radix(x, 16).ok()) {
                Some(v) => {
                    out.push(v);
                    i += 3;
                }
                None => {
                    out.push(b'%');
                    i += 1;
                }
            }
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Поднять HTTP на адресах listen.
pub async fn start_http(node: &Arc<Node>) -> Result<(), String> {
    let app = router(node.clone());
    let listen = node.cfg.read().listen.clone();
    let mut urls = Vec::new();
    for l in listen {
        let Some((ip, port)) = bridge_core::config::parse_listen(&l) else { continue };
        let sock = SocketAddr::new(ip, port);
        let listener = tokio::net::TcpListener::bind(sock).await.map_err(|e| format!("Не удалось слушать {}: {} (порт занят — мост уже запущен?)", l, e))?;
        let real = listener.local_addr().map_err(|e| e.to_string())?;
        urls.push(format!("http://{}", real));
        node.bound.lock().insert(canon_ip(real.ip()));
        serve(node, app.clone(), listener);
    }
    *node.urls.write() = urls;
    bind_auto(node).await;
    Ok(())
}

fn serve(node: &Arc<Node>, app: Router, listener: tokio::net::TcpListener) {
    let cancel = node.shutdown.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).tcp_nodelay(true).with_graceful_shutdown(async move { cancel.cancelled().await }).await;
    });
}

/// IPv4-адреса сетей этого ПК (без 127.x и 169.254.x — «сети нет»), с именем подключения.
pub fn interface_ips() -> Vec<(IpAddr, String)> {
    let mut out: Vec<(IpAddr, String)> = Vec::new();
    for i in if_addrs::get_if_addrs().unwrap_or_default() {
        let ip = i.ip();
        if let IpAddr::V4(v4) = ip {
            if v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() || out.iter().any(|(x, _)| *x == ip) {
                continue;
            }
            out.push((ip, i.name.clone()));
        }
    }
    out
}

/// Слушать на всех сетях ПК тем же портом (listenAuto): сосед из Radmin должен достучаться, даже если в listen только локальная сеть.
pub async fn bind_auto(node: &Arc<Node>) {
    let (auto, listen) = {
        let c = node.cfg.read();
        (c.listen_auto, c.listen.clone())
    };
    if !auto {
        return;
    }
    let parsed: Vec<(IpAddr, u16)> = listen.iter().filter_map(|l| bridge_core::config::parse_listen(l)).collect();
    if parsed.iter().any(|(ip, _)| ip.is_unspecified()) {
        return; // и так слушаем всё
    }
    let Some(port) = parsed.iter().find(|(ip, _)| !ip.is_loopback()).or(parsed.first()).map(|(_, p)| *p).filter(|p| *p != 0) else { return };
    let mut app: Option<Router> = None;
    for (ip, name) in interface_ips() {
        if node.bound.lock().contains(&ip) || node.bind_failed.lock().contains(&ip) {
            continue;
        }
        match tokio::net::TcpListener::bind(SocketAddr::new(ip, port)).await {
            Ok(listener) => {
                node.bound.lock().insert(ip);
                node.urls.write().push(format!("http://{}:{}", ip, port));
                node.info(&format!("Слушаю и на {}:{} ({}) — соседи из этой сети смогут подключиться", ip, port, name));
                let a = app.get_or_insert_with(|| router(node.clone())).clone();
                serve(node, a, listener);
            }
            Err(e) => {
                node.bind_failed.lock().insert(ip);
                node.info(&format!("Не удалось слушать {}:{} ({}): {}", ip, port, name, e));
            }
        }
    }
}

pub fn router(node: Arc<Node>) -> Router {
    let p = API_PREFIX;
    Router::new()
        .route(&format!("{}/health", p), get(health))
        .route(&format!("{}/peer/hello", p), get(hello))
        .route(&format!("{}/peer/shares/:share/manifest", p), get(share_manifest))
        .route(&format!("{}/peer/shares/:share/file", p), get(share_file))
        .route(&format!("{}/peer/shares/:share/events", p), get(share_events))
        .route(&format!("{}/peer/notify", p), post(notify))
        .route(&format!("{}/peer/mailbox", p), get(mailbox_list))
        .route(&format!("{}/peer/mailbox/file", p), get(mailbox_file))
        .route(&format!("{}/peer/mailbox/ack", p), post(mailbox_ack))
        .route(&format!("{}/peer/folders", p), get(folders))
        .route(&format!("{}/peer/folders/:id/manifest", p), get(folder_manifest))
        .route(&format!("{}/peer/folders/:id/file", p), get(folder_file).put(folder_put))
        .route(&format!("{}/peer/folders/:id/delete", p), post(folder_delete))
        .route(&format!("{}/peer/signal", p), post(signal))
        .route(&format!("{}/peer/agent/ask", p), post(agent_ask))
        .route(&format!("{}/peer/agent/events", p), post(agent_events))
        .route(&format!("{}/peer/agent/stop", p), post(agent_stop))
        .route(&format!("{}/peer/agent/answer", p), post(agent_answer))
        .route(&format!("{}/peer/agent/thread", p), get(agent_thread))
        .route(&format!("{}/peer/assistant/msg", p), post(assistant_msg))
        .route(&format!("{}/peer/avatar", p), get(peer_avatar))
        .route(&format!("{}/peer/tile", p), get(|State(n): St| async move { ok_json(&crate::tile::own(&n)) }))
        .route(&format!("{}/peer/tile/img", p), get(peer_tile_img))
        .route(&format!("{}/peer/tile/stimg", p), get(peer_tile_stimg))
        .route(&format!("{}/peer/joinsound", p), get(peer_joinsound))
        .route(&format!("{}/peer/pingsound", p), get(peer_pingsound))
        .route(&format!("{}/peer/chat/msg", p), post(peer_chat))
        .route(&format!("{}/peer/update/offer", p), post(peer_update_offer))
        .route(&format!("{}/peer/board/state", p), get(peer_board_state))
        .route(&format!("{}/peer/chan/art", p), get(peer_chan_art))
        .route(&format!("{}/peer/chan/img", p), get(peer_chan_img))
        .route(&format!("{}/peer/board/op", p), post(peer_board_op))
        .route(&format!("{}/peer/board/sub", p), post(peer_board_sub))
        .route(&format!("{}/peer/board/push", p), post(peer_board_push))
        .route(&format!("{}/peer/board/image", p), get(peer_board_image_get).post(peer_board_image_put))
        .route(&format!("{}/peer/update/file", p), get(peer_update_file))
        .route(&format!("{}/peer/assistant/reply", p), post(assistant_reply))
        .merge(crate::http_local::routes())
        .merge(crate::http_apps::routes())
        .merge(crate::mobile::routes())
        .layer(axum::extract::DefaultBodyLimit::disable())
        .layer(middleware::from_fn_with_state(node.clone(), gate))
        .with_state(node)
}

async fn health(State(node): St) -> Response {
    ok_json(&serde_json::json!({ "ok": true, "node": node.node_name(), "version": bridge_core::APP_VERSION, "protocol": bridge_core::PROTOCOL }))
}

fn ctx(req: &Request) -> PeerCtx {
    req.extensions().get::<PeerCtx>().cloned().expect("peer ctx")
}

fn shared(node: &Node, share: &str, peer: &str) -> Option<Arc<crate::shares::ShareRt>> {
    let rt = node.share(share)?;
    let c = node.cfg.read();
    let s = c.share(&rt.id)?;
    c.share_has_peer(s, peer).then_some(rt.clone())
}

async fn hello(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, Query(q): Query<HashMap<String, String>>, req: Request) -> Response {
    let pc = ctx(&req);
    link::learn_announced(&node, &pc.name, q.get("listen").map(|s| s.as_str()).unwrap_or(""), Some(canon_ip(addr.ip())));
    let shares: Vec<String> = {
        let c = node.cfg.read();
        c.shares.iter().filter(|s| c.share_has_peer(s, &pc.name)).map(|s| s.id.clone()).collect()
    };
    let mut revs = std::collections::BTreeMap::new();
    for id in &shares {
        if let Some(rt) = node.share(id) {
            if rt.files_dirty.load(Ordering::SeqCst) {
                crate::shares::local_check(&node, &rt).await;
            }
            // пока папка не «устоялась» — без отпечатка (сосед заберёт список полностью)
            if !rt.files_dirty.load(Ordering::SeqCst) {
                revs.insert(id.clone(), files_rev(&rt.files.lock().manifest().files));
            }
        }
    }
    let ps = node.peer_state(&pc.name);
    let channels = crate::media::channels_for(&node, &pc.name);
    let agent = {
        let c = node.cfg.read();
        if c.agent_allows(&pc.name) {
            c.agent.mode.clone()
        } else {
            String::new()
        }
    };
    let turn = node.turn.running().then(|| {
        let (username, credential) = node.turn.creds(&pc.name, std::time::Duration::from_secs(24 * 3600));
        TurnInfo { port: node.turn.port.load(Ordering::SeqCst), username, credential }
    });
    let runners: Vec<RunnerInfo> = {
        let c = node.cfg.read();
        c.runners
            .iter()
            .filter(|(_, r)| r.enabled && c.list_allows(&r.allow_from, &pc.name))
            .map(|(name, r)| RunnerInfo { name: name.clone(), title: r.title.clone(), share: r.share.clone(), params: r.params.clone(), hint: r.hint.clone(), confirm: r.confirm })
            .collect()
    };
    ok_json(&HelloResponse {
        avatar: crate::avatar::own_hash(&node),
        sound: crate::sound::own_hash(&node),
        ping: crate::sound::ping_hash(&node),
        tile: crate::tile::own_hash(&node),
        assistant: crate::assistant::hello_info(&node, &pc.name),
        runners,
        turn,
        channels,
        agent,
        node: node.node_name(),
        shares,
        status: Some(node.status()),
        you: Some(PeerView { name: pc.name.clone(), online: ps.online, last_seen: ps.last_seen, error: ps.error, url: node.cfg.read().peer(&pc.name).map(|p| p.url.clone()).unwrap_or_default() }),
        revs,
        ..Default::default()
    })
}

async fn share_manifest(State(node): St, AxPath(share): AxPath<String>, req: Request) -> Response {
    let pc = ctx(&req);
    let Some(rt) = shared(&node, &share, &pc.name) else { return err(404, "нет такого пространства для этого соседа") };
    // Свежий манифест — только если в папке что-то менялось
    if rt.files_dirty.load(Ordering::SeqCst) {
        crate::shares::local_check(&node, &rt).await;
    }
    let m = rt.files.lock().manifest().clone();
    ok_json(&m)
}

async fn send_file(node: &Arc<Node>, peer: &str, what: &str, label: &str, full: std::path::PathBuf, sha: String) -> Response {
    let f = match tokio::fs::File::open(&full).await {
        Ok(f) => f,
        Err(_) => return err(409, "файл занят, повторите"),
    };
    let len = f.metadata().await.map(|m| m.len()).unwrap_or(0);
    let guard = node.transfers.start("serve", peer, what, label, len as i64);
    let lim = node.limits.clone();
    let stream = tokio_util::io::ReaderStream::with_capacity(f, 64 * 1024).then(move |c| {
        let n = c.as_ref().map(|b| b.len()).unwrap_or(0);
        guard.add(n);
        let lim = lim.clone();
        async move {
            lim.up.take(n).await;
            c
        }
    });
    let mut r = Response::new(Body::from_stream(stream));
    let h = r.headers_mut();
    h.insert("Content-Type", HeaderValue::from_static("application/octet-stream"));
    h.insert("Content-Length", HeaderValue::from(len));
    if let Ok(v) = HeaderValue::from_str(&sha) {
        h.insert("X-Sha256", v);
    }
    r
}

#[derive(Deserialize)]
struct PathQ {
    path: Option<String>,
}

async fn share_file(State(node): St, AxPath(share): AxPath<String>, Query(q): Query<PathQ>, req: Request) -> Response {
    let pc = ctx(&req);
    let Some(rt) = shared(&node, &share, &pc.name) else { return err(404, "нет такого пространства для этого соседа") };
    let p = match path::normalize(q.path.as_deref().unwrap_or("")) {
        Ok(p) => p,
        Err(w) => return err(400, format!("путь: {}", w)),
    };
    if node.cfg.read().global_ignore().is_ignored(&p) {
        return err(404, "нет файла");
    }
    let entry = rt.files.lock().find(&p).cloned();
    let Some(entry) = entry else { return err(404, "нет файла") };
    let Ok(full) = path::to_local(&rt.root, &entry.path) else { return err(400, "путь") };
    send_file(&node, &pc.name, "пространство", &entry.path, full, entry.sha256).await
}

#[derive(Deserialize)]
struct EventsQ {
    after: Option<String>,
    limit: Option<usize>,
}

async fn share_events(State(node): St, AxPath(share): AxPath<String>, Query(q): Query<EventsQ>, req: Request) -> Response {
    let pc = ctx(&req);
    let Some(rt) = shared(&node, &share, &pc.name) else { return err(404, "нет такого пространства для этого соседа") };
    let after = parse_vector(q.after.as_deref().unwrap_or(""));
    let mut rb = rt.records.lock();
    // Курсор соседа = что у него уже есть: основа квитанций «доставлено»
    let key = rb.peer_cursors.keys().find(|k| eq_ic(k, &pc.name)).cloned().unwrap_or(pc.name.clone());
    let cur = rb.peer_cursors.entry(key).or_default();
    let mut changed = false;
    for (a, s) in &after {
        let k = cur.keys().find(|k| eq_ic(k, a)).cloned().unwrap_or(a.clone());
        let v = cur.entry(k).or_insert(0);
        if *s > *v {
            *v = *s;
            changed = true;
        }
    }
    if changed {
        rt.cursors_changed.store(true, Ordering::SeqCst);
        rt.interface_dirty.store(true, Ordering::SeqCst);
    }
    let page = rb.store.since(&after, q.limit.unwrap_or(500).clamp(1, 2000));
    ok_json(&page)
}

async fn notify(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let r: NotifyRequest = serde_json::from_slice(&pc.body).unwrap_or_default();
    match r.kind.as_str() {
        "mailbox" => node.mailbox.wake.store(true, Ordering::SeqCst),
        "folders" => node.folders.wake.store(true, Ordering::SeqCst),
        _ => {
            if let Some(rt) = shared(&node, &r.share, &pc.name) {
                rt.wake.store(true, Ordering::SeqCst);
            }
        }
    }
    node.wake.notify_one();
    ok_json(&serde_json::json!({ "ok": true }))
}

async fn mailbox_list(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    if !node.cfg.read().mailbox.enabled {
        return err(404, "ручная отправка выключена");
    }
    let Some(peer) = node.cfg.read().peer(&pc.name).cloned() else { return err(404, "нет соседа") };
    let files = node.mailbox.pending(&node, &peer).into_iter().map(|(e, _)| e).collect();
    ok_json(&MailboxList { from: node.node_name(), files })
}

async fn mailbox_file(State(node): St, Query(q): Query<PathQ>, req: Request) -> Response {
    let pc = ctx(&req);
    let p = match path::normalize(q.path.as_deref().unwrap_or("")) {
        Ok(p) => p,
        Err(w) => return err(400, format!("путь: {}", w)),
    };
    let Some(peer) = node.cfg.read().peer(&pc.name).cloned() else { return err(404, "нет соседа") };
    let item = node.mailbox.pending(&node, &peer).into_iter().find(|(e, _)| eq_ic(&e.path, &p));
    let Some((e, full)) = item else { return err(404, "нет файла") };
    send_file(&node, &pc.name, "отправка", &e.path, full, e.sha256).await
}

async fn mailbox_ack(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let Ok(ack) = serde_json::from_slice::<MailboxAck>(&pc.body) else { return err(400, "нужны path и sha256") };
    if path::norm(&ack.path).is_none() {
        return err(400, "нужны path и sha256");
    }
    let Some(peer) = node.cfg.read().peer(&pc.name).cloned() else { return err(404, "нет соседа") };
    ok_json(&serde_json::json!({ "ok": node.mailbox.acknowledge(&node, &peer, &ack) }))
}

async fn folders(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    ok_json(&node.folders.visible_to(&node, &pc.name))
}

async fn folder_manifest(State(node): St, AxPath(id): AxPath<String>, req: Request) -> Response {
    let pc = ctx(&req);
    match node.folders.manifest(&node, &id, &pc.name) {
        Ok(Some(m)) => ok_json(&m),
        Ok(None) => err(404, "нет такой папки"),
        Err(e) => err(409, e),
    }
}

async fn folder_file(State(node): St, AxPath(id): AxPath<String>, Query(q): Query<PathQ>, req: Request) -> Response {
    let pc = ctx(&req);
    let p = match path::normalize(q.path.as_deref().unwrap_or("")) {
        Ok(p) => p,
        Err(w) => return err(400, format!("путь: {}", w)),
    };
    match node.folders.own_file(&node, &id, &pc.name, &p) {
        Some((full, sha)) => send_file(&node, &pc.name, "папка", &p, full, sha).await,
        None => err(404, "нет файла"),
    }
}

async fn folder_put(State(node): St, AxPath(id): AxPath<String>, Query(q): Query<HashMap<String, String>>, req: Request) -> Response {
    let pc = ctx(&req);
    let p = match path::normalize(q.get("path").map(|s| s.as_str()).unwrap_or("")) {
        Ok(p) => p,
        Err(w) => return err(400, format!("путь: {}", w)),
    };
    let sha = q.get("sha").cloned().unwrap_or_default().to_lowercase();
    let base = q.get("base").cloned().unwrap_or_default();
    // Тело — во временный файл с проверкой размера и sha
    let max = node.max_file_bytes();
    if let Some(sz) = q.get("size").and_then(|s| s.parse::<i64>().ok()) {
        if sz > max {
            return err(413, format!("файл больше предела {} МБ у «{}»", max / 1024 / 1024, node.node_name()));
        }
    }
    let tmp = node.tmp_file();
    let mut file = match tokio::fs::File::create(&tmp).await {
        Ok(f) => f,
        Err(e) => return err(500, e.to_string()),
    };
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    let mut total: i64 = 0;
    let guard = node.transfers.start("recv", &pc.name, "папка", &p, q.get("size").and_then(|s| s.parse().ok()).unwrap_or(0));
    let mut stream = req.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            let _ = tokio::fs::remove_file(&tmp).await;
            return err(400, "передача оборвалась");
        };
        guard.add(chunk.len());
        node.limits.down.take(chunk.len()).await;
        total += chunk.len() as i64;
        if total > max {
            drop(file);
            let _ = tokio::fs::remove_file(&tmp).await;
            return err(413, "файл больше предела");
        }
        sha2::Digest::update(&mut hasher, &chunk);
        if file.write_all(&chunk).await.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
            return err(500, "не удалось записать");
        }
    }
    let _ = file.flush().await;
    drop(file);
    if hex::encode(sha2::Digest::finalize(hasher)) != sha {
        let _ = tokio::fs::remove_file(&tmp).await;
        return err(422, "файл повреждён при передаче");
    }
    let (code, msg) = node.folders.own_write(&node, &id, &pc.name, &p, &base, &sha, &tmp).await;
    let _ = tokio::fs::remove_file(&tmp).await;
    if code == 200 {
        ok_json(&serde_json::json!({ "ok": true, "message": msg }))
    } else {
        err(code, msg)
    }
}

async fn folder_delete(State(node): St, AxPath(id): AxPath<String>, req: Request) -> Response {
    let pc = ctx(&req);
    let Ok(r) = serde_json::from_slice::<FolderDeleteRequest>(&pc.body) else { return err(400, "нужны path и base") };
    let Some(p) = path::norm(&r.path) else { return err(400, "нужны path и base") };
    let (code, msg) = node.folders.own_delete(&node, &id, &pc.name, &p, &r.base).await;
    if code == 200 {
        ok_json(&serde_json::json!({ "ok": true, "message": msg }))
    } else {
        err(code, msg)
    }
}

/// Сигналинг экрана и голоса: сообщение от соседа — панелям этого ПК.
async fn signal(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let pc = ctx(&req);
    let Ok(mut msg) = serde_json::from_slice::<serde_json::Value>(&pc.body) else { return err(400, "не JSON") };
    if !msg.is_object() {
        return err(400, "не объект");
    }
    msg["from"] = serde_json::Value::String(pc.name.clone());
    // Браузер прячет свои адреса за «xxxx.local» (mDNS), а через VPN такие имена не находятся.
    // Мост знает настоящий адрес соседа — подставляем его.
    let ip = canon_ip(addr.ip()).to_string();
    if let Some(c) = msg.get_mut("candidate").and_then(|c| c.get_mut("candidate")) {
        if let Some(text) = c.as_str() {
            *c = serde_json::Value::String(unmask_candidate(text, &ip));
        }
    }
    if let Some(sdp) = msg.get_mut("desc").and_then(|d| d.get_mut("sdp")) {
        if let Some(text) = sdp.as_str() {
            let fixed: Vec<String> = text.split("\r\n").map(|l| if l.starts_with("a=candidate:") { unmask_candidate(l, &ip) } else { l.to_string() }).collect();
            *sdp = serde_json::Value::String(fixed.join("\r\n"));
        }
    }
    if let Some(sdp) = msg.get_mut("sdp") {
        if let Some(text) = sdp.as_str() {
            let fixed: Vec<String> = text.split("\r\n").map(|l| if l.starts_with("a=candidate:") { unmask_candidate(l, &ip) } else { l.to_string() }).collect();
            *sdp = serde_json::Value::String(fixed.join("\r\n"));
        }
    }
    msg["ts"] = serde_json::json!(chrono::Utc::now().timestamp());
    // Переслать участнику канала, который не сосед отправителю (этот узел — владелец канала)
    if let Some(to) = msg.get("relayTo").and_then(|t| t.as_str()).map(String::from) {
        if !eq_ic(&to, &node.node_name()) {
            return match crate::media::relay(&node, &pc.name, &to, msg).await {
                Ok(()) => ok_json(&serde_json::json!({ "ok": true })),
                Err((code, why)) => err(code, why),
            };
        }
    }
    // Пересланное владельцем канала: настоящий отправитель — origin
    if let Some(origin) = msg.get("origin").and_then(|t| t.as_str()).map(String::from) {
        let room = msg.get("room").and_then(|t| t.as_str()).unwrap_or("");
        if room.split(':').next().map(|o| eq_ic(o, &pc.name)).unwrap_or(false) {
            msg["from"] = serde_json::Value::String(origin);
            msg["via"] = serde_json::Value::String(pc.name.clone());
        }
        if let Some(o) = msg.as_object_mut() {
            o.remove("origin");
        }
    }
    let kind = msg.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
    if kind == "invite" || kind == "ask" {
        let what = if kind == "ask" { "screen".to_string() } else { msg.get("media").and_then(|m| m.as_str()).unwrap_or("voice").to_string() };
        let title = node.cfg.read().peer(&pc.name).map(|p| if p.title.is_empty() { p.name.clone() } else { p.title.clone() }).unwrap_or(pc.name.clone());
        node.info(&format!("{} от «{}»", if what == "screen" { "Просьба показать экран" } else { "Звонок" }, title));
        node.emit("media-invite", serde_json::json!({ "from": pc.name, "title": title, "media": what, "room": msg.get("room") }));
    }
    // для ассистента (поток /local/events): кто зашёл в канал, вышел, что поменял
    let room_ev = if msg.get("media").and_then(|m| m.as_str()) == Some("room") && (kind == "state" || kind == "leave") {
        let room = msg.get("room").and_then(|r| r.as_str()).unwrap_or("").to_string();
        let from = msg.get("from").and_then(|r| r.as_str()).unwrap_or("").to_string();
        let key = format!("{}|{}", room, from);
        let prev = node.media.presence.lock().get(&key).filter(|(_, t)| chrono::Utc::now().timestamp() - *t < crate::media::PRESENCE_TTL).map(|(v, _)| v.clone());
        let st = msg.get("st").cloned();
        let changed = prev.is_none() || prev != st;
        Some((room, from, prev.is_some(), st, changed))
    } else {
        None
    };
    let listeners = node.media.has_listeners();
    node.media.deliver(msg);
    if let Some((room, from, was, st, changed)) = room_ev {
        match (kind.as_str(), was) {
            ("state", true) if !changed => {}
            ("state", false) => node.emit("room-join", serde_json::json!({ "room": room, "peer": from, "state": st })),
            ("state", true) => node.emit("room-state", serde_json::json!({ "room": room, "peer": from, "state": st })),
            ("leave", _) => node.emit("room-leave", serde_json::json!({ "room": room, "peer": from })),
            _ => {}
        }
    }
    ok_json(&serde_json::json!({ "ok": true, "listeners": listeners }))
}

/// «candidate:… 1 udp 2122… abcd-ef.local 54321 typ host» → адрес вместо имени .local.
pub fn unmask_candidate(line: &str, ip: &str) -> String {
    let mut parts: Vec<String> = line.split(' ').map(String::from).collect();
    if parts.len() > 5 && parts[4].ends_with(".local") {
        parts[4] = ip.to_string();
    }
    parts.join(" ")
}

async fn peer_chat(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let v = match body_json(&pc) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match crate::chat::on_peer(&node, &pc.name, &v).await {
        Ok(()) => ok_json(&serde_json::json!({ "ok": true })),
        Err((c, e)) => err(c, e),
    }
}

// ───────────────────────── Доска канала ─────────────────────────

fn board_of(node: &Node, key: &str) -> Result<Arc<crate::board::Room>, Response> {
    crate::board::room(node, key).map_err(|(c, e)| err(c, e))
}

/// Оформление канала для участника.
async fn peer_chan_art(State(node): St, Query(m): Query<HashMap<String, String>>, req: Request) -> Response {
    let pc = ctx(&req);
    match crate::chanart::for_peer(&node, &pc.name, m.get("id").map(|s| s.as_str()).unwrap_or("")) {
        Some(v) => ok_json(&v),
        None => err(404, "нет такого канала"),
    }
}

async fn peer_chan_img(State(node): St, Query(m): Query<HashMap<String, String>>, req: Request) -> Response {
    let pc = ctx(&req);
    let g = |k: &str| m.get(k).map(|s| s.as_str()).unwrap_or("").to_string();
    match crate::chanart::img_for_peer(&node, &pc.name, &g("id"), &g("what")) {
        Some(b) => {
            let mime = crate::avatar::mime_of(&b).unwrap_or("application/octet-stream");
            let mut r = Response::new(Body::from(b));
            r.headers_mut().insert("Content-Type", HeaderValue::from_static(mime));
            r
        }
        None => err(404, "картинки нет"),
    }
}

async fn peer_board_state(State(node): St, Query(m): Query<HashMap<String, String>>, req: Request) -> Response {
    let pc = ctx(&req);
    let r = match board_of(&node, m.get("room").map(|s| s.as_str()).unwrap_or("")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    match crate::board::peer_state(&node, &r, &pc.name) {
        Ok(v) => ok_json(&v),
        Err((c, e)) => err(c, e),
    }
}

async fn peer_board_op(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let v = match body_json(&pc) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let r = match board_of(&node, v["room"].as_str().unwrap_or("")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let _ = crate::board::peer_sub(&node, &r, &pc.name);
    match crate::board::apply(&node, &r, &pc.name, v["cs"].as_str().unwrap_or(""), v["ops"].as_array().map(|a| a.as_slice()).unwrap_or(&[])) {
        Ok(seq) => ok_json(&serde_json::json!({ "ok": true, "seq": seq })),
        Err((c, e)) => err(c, e),
    }
}

async fn peer_board_sub(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let v = match body_json(&pc) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let r = match board_of(&node, v["room"].as_str().unwrap_or("")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    match crate::board::peer_sub(&node, &r, &pc.name) {
        Ok(()) => ok_json(&serde_json::json!({ "ok": true })),
        Err((c, e)) => err(c, e),
    }
}

async fn peer_board_push(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let v = match body_json(&pc) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let r = match board_of(&node, v["room"].as_str().unwrap_or("")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    match crate::board::on_push(&node, &r, &pc.name, v["events"].as_array().map(|a| a.as_slice()).unwrap_or(&[])) {
        Ok(()) => ok_json(&serde_json::json!({ "ok": true })),
        Err((c, e)) => err(c, e),
    }
}

async fn peer_board_image_put(State(node): St, req: Request) -> Response {
    use base64::Engine;
    let pc = ctx(&req);
    let v = match body_json(&pc) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let r = match board_of(&node, v["room"].as_str().unwrap_or("")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let Ok(b) = base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap_or("")) else { return err(400, "картинка: не base64") };
    match crate::board::store_image(&node, &r, &pc.name, &b) {
        Ok(id) => ok_json(&serde_json::json!({ "ok": true, "id": id })),
        Err((c, e)) => err(c, e),
    }
}

async fn peer_board_image_get(State(node): St, Query(m): Query<HashMap<String, String>>, req: Request) -> Response {
    let pc = ctx(&req);
    let r = match board_of(&node, m.get("room").map(|s| s.as_str()).unwrap_or("")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    match crate::board::own_image(&node, &r, &pc.name, m.get("id").map(|s| s.as_str()).unwrap_or("")) {
        Some(b) => {
            let mut resp = Response::new(Body::from(b));
            resp.headers_mut().insert("Content-Type", HeaderValue::from_static("application/octet-stream"));
            resp
        }
        None => err(404, "нет картинки"),
    }
}

/// Сосед предлагает новую версию моста (только сохраняем — ставится после подтверждения здесь).
async fn peer_update_offer(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let v = match body_json(&pc) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match crate::update::on_offer(&node, &pc.name, &v) {
        Ok(()) => ok_json(&serde_json::json!({ "ok": true })),
        Err((c, e)) => err(c, e),
    }
}

/// Отдать свой файл программы (только Мост.exe / bridge.exe из своей папки) соседу, которому мы предложили версию.
async fn peer_update_file(State(node): St, Query(m): Query<HashMap<String, String>>) -> Response {
    let name = m.get("name").cloned().unwrap_or_default();
    if !crate::update::names().contains(&name.as_str()) {
        return err(404, "такого файла нет");
    }
    match tokio::fs::read(node.updates.dir().join(&name)).await {
        Ok(b) => {
            let mut r = Response::new(Body::from(b));
            r.headers_mut().insert("Content-Type", HeaderValue::from_static("application/octet-stream"));
            r
        }
        Err(_) => err(404, "файла нет"),
    }
}

async fn peer_pingsound(State(node): St) -> Response {
    match crate::sound::ping_own(&node) {
        Some(b) => {
            let mime = crate::sound::mime_of(&b).unwrap_or("application/octet-stream");
            let mut r = Response::new(Body::from(b));
            r.headers_mut().insert("Content-Type", HeaderValue::from_static(mime));
            r
        }
        None => err(404, "звука нет"),
    }
}

async fn peer_joinsound(State(node): St) -> Response {
    match crate::sound::own(&node) {
        Some(b) => {
            let mime = crate::sound::mime_of(&b).unwrap_or("application/octet-stream");
            let mut r = Response::new(Body::from(b));
            r.headers_mut().insert("Content-Type", HeaderValue::from_static(mime));
            r
        }
        None => err(404, "звука нет"),
    }
}

async fn peer_tile_stimg(State(node): St) -> Response {
    match crate::tile::own_st_img(&node) {
        Some(b) => {
            let mime = crate::avatar::mime_of(&b).unwrap_or("application/octet-stream");
            let mut r = Response::new(Body::from(b));
            r.headers_mut().insert("Content-Type", HeaderValue::from_static(mime));
            r
        }
        None => err(404, "значка статуса нет"),
    }
}

async fn peer_tile_img(State(node): St) -> Response {
    match crate::tile::own_img(&node) {
        Some(b) => {
            let mime = crate::avatar::mime_of(&b).unwrap_or("application/octet-stream");
            let mut r = Response::new(Body::from(b));
            r.headers_mut().insert("Content-Type", HeaderValue::from_static(mime));
            r
        }
        None => err(404, "картинки плитки нет"),
    }
}

async fn peer_avatar(State(node): St) -> Response {
    match crate::avatar::own(&node) {
        Some(b) => {
            let mime = crate::avatar::mime_of(&b).unwrap_or("application/octet-stream");
            let mut r = Response::new(Body::from(b));
            r.headers_mut().insert("Content-Type", HeaderValue::from_static(mime));
            r
        }
        None => err(404, "аватарки нет"),
    }
}

// ───────────────────────── Ассистент ─────────────────────────

/// Клиент → хозяин: сообщение ассистенту. Имя отправителя — по подписи соседа (не из тела).
async fn assistant_msg(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let v = match body_json(&pc) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !node.assistant.is_host() {
        return err(404, "ассистента на этом ПК нет");
    }
    {
        let c = node.cfg.read();
        if !c.list_allows(&c.assistant.allow_from, &pc.name) {
            return err(403, "этому ПК писать ассистенту не разрешено (assistant.allowFrom)");
        }
    }
    let mut item = v;
    if let Some(o) = item.as_object_mut() {
        o.remove("seq");
        o.remove("peer");
    }
    match node.assistant.push_inbound(&node, item, &pc.name) {
        Ok(_) => ok_json(&serde_json::json!({ "ok": true })),
        Err(e) => err(400, e),
    }
}

/// Хозяин → клиент: ответ ассистента. Только от соседа, у которого ассистент (по его привету).
async fn assistant_reply(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let v = match body_json(&pc) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if node.peer_state(&pc.name).assistant.is_none() {
        return err(403, "у этого соседа нет ассистента");
    }
    match node.assistant.deliver_local(&node, &v) {
        Ok(()) => ok_json(&serde_json::json!({ "ok": true })),
        Err(e) => err(400, e),
    }
}

// ───────────────────────── Агент ─────────────────────────

fn body_json(pc: &PeerCtx) -> Result<serde_json::Value, Response> {
    serde_json::from_slice(&pc.body).map_err(|_| err(400, "не JSON"))
}

/// Сосед поручает задачу агенту этого ПК.
async fn agent_ask(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let v = match body_json(&pc) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match crate::agent::on_ask(&node, &pc.name, &v) {
        Ok(r) => ok_json(&r),
        Err((c, w)) => err(c, w),
    }
}

/// Исполнитель шлёт ход работы по нашему поручению.
async fn agent_events(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let v = match body_json(&pc) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match crate::agent::on_events(&node, &pc.name, &v) {
        Ok(()) => ok_json(&serde_json::json!({ "ok": true })),
        Err((c, w)) => err(c, w),
    }
}

/// Оркестратор просит остановить работу по его поручению.
/// Заказчик ответил на вопрос агента.
async fn agent_answer(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let v = match body_json(&pc) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    match node.agent.get(&s("thread")) {
        Some(t) if t.role == "in" && eq_ic(&t.peer, &pc.name) => match crate::agent::answer(&node, &s("thread"), &s("req"), &s("answer"), &pc.name).await {
            Ok(()) => ok_json(&serde_json::json!({ "ok": true })),
            Err((c, w)) => err(c, w),
        },
        _ => err(404, "нет такого поручения от вас"),
    }
}

async fn agent_stop(State(node): St, req: Request) -> Response {
    let pc = ctx(&req);
    let v = match body_json(&pc) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let id = v.get("thread").and_then(|s| s.as_str()).unwrap_or("");
    match node.agent.get(id) {
        Some(t) if t.role == "in" && eq_ic(&t.peer, &pc.name) => {
            crate::agent::stop_local(&node, id, &format!("остановил «{}»", pc.name));
            ok_json(&serde_json::json!({ "ok": true }))
        }
        _ => err(404, "нет такого поручения от вас"),
    }
}

/// Нить целиком — для оркестратора (если ход не доходил).
async fn agent_thread(State(node): St, Query(q): Query<HashMap<String, String>>, req: Request) -> Response {
    let pc = ctx(&req);
    let id = q.get("id").cloned().unwrap_or_default();
    match node.agent.get(&id) {
        Some(t) if t.role == "in" && eq_ic(&t.peer, &pc.name) => ok_json(&t),
        _ => err(404, "нет такого поручения от вас"),
    }
}
