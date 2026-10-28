//! HTTP приложений (3.4): локальный API панели (`/local/apps…`, с ключом), файлы веб-интерфейсов (`/apps/<id>/ui/…`,
//! только с 127.0.0.1, без ключа — их открывает изолированная рамка) и API для служб и своих окон (`/app/…`, по
//! токену приложения `MOST_TOKEN`).

use crate::apps;
use crate::http_peer::{err, ok_json, St};
use axum::body::Body;
use axum::extract::{Path as AxPath, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use bridge_core::API_PREFIX;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

type Q = Query<HashMap<String, String>>;
fn q<'a>(m: &'a HashMap<String, String>, k: &str) -> &'a str {
    m.get(k).map(|s| s.as_str()).unwrap_or("")
}
fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}
fn res(r: Result<Value, String>) -> Response {
    match r {
        Ok(v) => ok_json(&v),
        Err(e) => err(400, e),
    }
}

pub fn routes() -> Router<Arc<crate::node::Node>> {
    let l = |x: &str| format!("{}/local/{}", API_PREFIX, x);
    let a = |x: &str| format!("{}/app/{}", API_PREFIX, x);
    Router::new()
        .route(&l("apps"), get(|State(n): St| async move { ok_json(&apps::status(&n)) }))
        .route(&l("apps/allow"), post(|State(n): St, Json(v): Json<Value>| async move {
            let perms = v["perms"].as_array().map(|x| x.iter().filter_map(|p| p.as_str().map(String::from)).collect());
            res(apps::allow(&n, &s(&v, "id"), perms).map(|_| json!({ "ok": true })))
        }))
        .route(&l("apps/revoke"), post(|State(n): St, Json(v): Json<Value>| async move { res(apps::revoke(&n, &s(&v, "id")).map(|_| json!({ "ok": true }))) }))
        .route(&l("apps/remove"), post(|State(n): St, Json(v): Json<Value>| async move { res(apps::remove(&n, &s(&v, "id")).map(|to| json!({ "ok": true, "trash": to }))) }))
        // передача приложений соседям (3.6)
        .route(&l("apps/share"), post(|State(n): St, Json(v): Json<Value>| async move {
            let peers: Vec<String> = v["peers"].as_array().map(|x| x.iter().filter_map(|p| p.as_str().map(String::from)).collect()).unwrap_or_default();
            res(crate::app_share::share(&n, &s(&v, "id"), &peers).await)
        }))
        .route(&l("apps/offer/accept"), post(|State(n): St, Json(v): Json<Value>| async move { res(crate::app_share::accept(&n, &s(&v, "offer")).map(|_| json!({ "ok": true }))) }))
        .route(&l("apps/offer/decline"), post(|State(n): St, Json(v): Json<Value>| async move {
            crate::app_share::decline(&n, &s(&v, "offer"));
            ok_json(&json!({ "ok": true }))
        }))
        .route(&l("apps/take"), post(|State(n): St, Json(v): Json<Value>| async move { res(crate::app_share::take(&n, &s(&v, "peer"), &s(&v, "id")).await.map(|o| json!({ "ok": true, "offer": o }))) }))
        .route(&l("apps/start"), post(|State(n): St, Json(v): Json<Value>| async move { res(apps::start(&n, &s(&v, "id"), &s(&v, "kind"))) }))
        .route(&l("apps/stop"), post(|State(n): St, Json(v): Json<Value>| async move {
            apps::stop(&n, &s(&v, "id"), &s(&v, "kind"), "остановлено из панели");
            ok_json(&json!({ "ok": true }))
        }))
        .route(&l("apps/touch"), post(|State(n): St, Json(v): Json<Value>| async move {
            apps::touch(&n, &s(&v, "id"));
            ok_json(&json!({ "ok": true }))
        }))
        .route(&l("apps/log"), get(|State(n): St, Query(m): Q| async move { ok_json(&json!({ "lines": apps::log(&n, q(&m, "id")) })) }))
        .route(&l("apps/action"), post(|State(n): St, Json(v): Json<Value>| async move { res(apps::action(&n, &s(&v, "id"), &s(&v, "action"), &v["params"]).await) }))
        .route(&l("apps/store"), get(|State(n): St, Query(m): Q| async move { local_store_get(&n, q(&m, "id"), q(&m, "key")) }).post(|State(n): St, Json(v): Json<Value>| async move {
            if apps::get(&n, &s(&v, "id")).is_none() {
                return err(404, "нет такого приложения");
            }
            res(apps::store_set(&n, &s(&v, "id"), &s(&v, "key"), v["value"].clone()).map(|_| json!({ "ok": true })))
        }))
        .route(&l("apps/import"), post(import))
        .route(&l("apps/svc/:id/*rest"), any(svc_proxy))
        .route(&l("apps/svc/:id/"), any(svc_proxy_root))
        // файлы интерфейсов — для изолированной рамки (без ключа, только с этого ПК)
        .route("/apps/:id/ui/*path", get(ui_file))
        .route("/apps/:id/icon", get(icon))
        .route("/most/sdk.js", get(sdk))
        .route("/apps/sdk.js", get(sdk))
        .route("/assets/apphost.html", get(apphost))
        // службы и свои окна приложений — по токену приложения
        .route(&a("me"), get(app_me))
        .route(&a("store"), get(app_store_get).post(app_store_set))
        .route(&a("notify"), post(app_notify))
        .route(&a("toast"), post(app_toast))
        // сеансы (3.5): сеть приложений, связь мостов, редактор, билеты, активность
        .route(&format!("{}/peer/net", API_PREFIX), get(peer_net))
        .route(&a("net"), get(app_net))
        .route(&a("dev"), post(dev_request).get(dev_state))
        .route(&a("ticket"), get(|State(n): St, Query(m): Q| async move {
            match crate::net::check_ticket(&n, q(&m, "t")) {
                Some(v) => ok_json(&json!({ "ok": true, "who": v })),
                None => err(404, "билет не найден: участник вышел или сеанс закончился"),
            }
        }))
        .route(&a("activity"), get(app_activity))
        // ключ пользователя (3.7): приложению — свой ключ для издателя и подпись строки
        .route(&a("identity"), get(app_identity))
        .route(&a("identity/sign"), post(app_identity_sign))
        .route(&l("identity"), get(|State(n): St| async move { ok_json(&crate::identity::status(&n)) }))
        .route(&l("identity/rotate"), post(|State(n): St| async move { res(crate::identity::rotate(&n)) }))
        .route(&l("identity/export"), post(|State(n): St, Json(v): Json<Value>| async move { res(crate::identity::export(&n, &s(&v, "password"))) }))
        .route(&l("identity/import"), post(|State(n): St, Json(v): Json<Value>| async move { res(crate::identity::import(&n, &v["file"], &s(&v, "password"))) }))
        .route(&l("apps/identity"), get(|State(n): St, Query(m): Q| async move { res(crate::identity::get(&n, q(&m, "id"))) }))
        .route(&l("apps/identity/sign"), post(|State(n): St, Json(v): Json<Value>| async move { res(crate::identity::sign(&n, &s(&v, "id"), &s(&v, "purpose"), v["payload"].as_str().unwrap_or("")).await) }))
        .route(&l("apps/identity/answer"), post(|State(n): St, Json(v): Json<Value>| async move { res(crate::identity::answer(&n, &s(&v, "ask"), v["ok"] == true, v["remember"] == true).map(|_| json!({ "ok": true }))) }))
        .route(&l("apps/net"), get(|State(n): St| async move { ok_json(&crate::net::status(&n)) }))
        .route(&l("apps/net/channel"), post(|State(n): St, Json(v): Json<Value>| async move {
            let members: Vec<String> = v["members"].as_array().map(|x| x.iter().filter_map(|m| m.as_str().map(String::from)).collect()).unwrap_or_default();
            crate::net::set_channel(&n, &s(&v, "room"), &s(&v, "title"), members);
            ok_json(&json!({ "ok": true }))
        }))
        .route(&l("apps/net/ticket"), post(|State(n): St, Json(v): Json<Value>| async move {
            let id = s(&v, "id");
            let Some(app) = apps::get(&n, &id) else { return err(404, "нет такого приложения") };
            if !apps::consent_ok(&n, &app) {
                return err(403, "приложение не разрешено на этом ПК");
            }
            let t = crate::net::panel_ticket(&n, &id);
            ok_json(&json!({ "url": format!("{}{}/app/net?panel={}", apps::base_url(&n).replacen("http://", "ws://", 1), API_PREFIX, t) }))
        }))
        .route(&l("apps/dev/answer"), post(|State(n): St, Json(v): Json<Value>| async move {
            res(crate::net::dev_answer(&n, &s(&v, "req"), v["ok"] == true).map(|_| json!({ "ok": true })))
        }))
        .route(&l("apps/activity"), get(|State(n): St, Query(m): Q| async move {
            if !q(&m, "stats").is_empty() {
                return ok_json(&crate::activity::stats(&n, q(&m, "stats")));
            }
            ok_json(&crate::activity::since(&n, q(&m, "since").parse().unwrap_or(0), q(&m, "limit").parse().unwrap_or(500).min(2000)))
        }))
        .route(&l("apps/activity/report"), post(|State(n): St, Json(v): Json<Value>| async move {
            let kind = s(&v, "kind");
            let mut d = v.clone();
            if let Some(o) = d.as_object_mut() {
                o.remove("kind");
            }
            match crate::activity::record(&n, &kind, d) {
                Some(e) => ok_json(&json!({ "ok": true, "e": e })),
                None => ok_json(&json!({ "ok": false })),
            }
        }))
}

async fn peer_net(State(node): St, axum::Extension(ctx): axum::Extension<crate::http_peer::PeerCtx>, ws: axum::extract::WebSocketUpgrade) -> Response {
    let peer = ctx.name.clone();
    ws.max_message_size(4 << 20).on_upgrade(move |sock| crate::net::serve_peer(node, peer, sock))
}

/// Сеть приложения: своё окно и служба — по токену (?token= или X-Most-App), веб-интерфейс — пропуском панели (?panel=).
async fn app_net(State(node): St, h: HeaderMap, Query(m): Q, ws: axum::extract::WebSocketUpgrade) -> Response {
    let (app, label) = if !q(&m, "panel").is_empty() {
        match crate::net::take_panel_ticket(&node, q(&m, "panel")).and_then(|id| apps::get(&node, &id)) {
            Some(a) => (a, node.node_name()),
            None => return err(401, "пропуск панели устарел"),
        }
    } else {
        let t = h.get("x-most-app").and_then(|v| v.to_str().ok()).map(String::from).unwrap_or_else(|| q(&m, "token").to_string());
        match apps::by_token(&node, &t) {
            Some(a) => (a, node.node_name()),
            None => return err(401, "нужен токен приложения (MOST_TOKEN) — или подключите редактор: POST /bridge/v1/app/dev"),
        }
    };
    if !apps::consent_ok(&node, &app) {
        return err(403, "приложение не разрешено на этом ПК");
    }
    let perms = apps::granted(&node, &app);
    let label = if q(&m, "name").is_empty() { label } else { format!("{} · {}", label, q(&m, "name").chars().take(30).collect::<String>()) };
    ws.max_message_size(2 << 20).on_upgrade(move |sock| crate::net::serve_app(node, sock, app.id, perms, label))
}

async fn dev_request(State(node): St, Json(v): Json<Value>) -> Response {
    if node.remote.active().is_some() {
        return err(423, "пока этим ПК управляют по сети, подключения не принимаются");
    }
    match crate::net::dev_request(&node, &s(&v, "app"), &s(&v, "who")) {
        Ok(req) => ok_json(&json!({ "req": req, "hint": "подтвердите подключение в панели моста; затем GET dev?req= вернёт токен" })),
        Err(e) => err(400, e),
    }
}

async fn dev_state(State(node): St, Query(m): Q) -> Response {
    ok_json(&crate::net::dev_state(&node, q(&m, "req")))
}

async fn app_identity(State(node): St, h: HeaderMap) -> Response {
    let Some(a) = app_of(&node, &h) else { return err(401, "нужен токен приложения") };
    match crate::identity::get(&node, &a.id) {
        Ok(v) => ok_json(&v),
        Err(e) => err(403, e),
    }
}

async fn app_identity_sign(State(node): St, h: HeaderMap, Json(v): Json<Value>) -> Response {
    let Some(a) = app_of(&node, &h) else { return err(401, "нужен токен приложения") };
    match crate::identity::sign(&node, &a.id, &s(&v, "purpose"), v["payload"].as_str().unwrap_or("")).await {
        Ok(v) => ok_json(&v),
        Err(e) => err(403, e),
    }
}

async fn app_activity(State(node): St, h: HeaderMap, Query(m): Q) -> Response {
    let Some(a) = app_of(&node, &h) else { return err(401, "нужен токен приложения") };
    if !apps::granted(&node, &a).iter().any(|p| p == "activity") {
        return err(403, "у приложения нет права activity");
    }
    if !q(&m, "stats").is_empty() {
        return ok_json(&crate::activity::stats(&node, q(&m, "stats")));
    }
    ok_json(&crate::activity::since(&node, q(&m, "since").parse().unwrap_or(0), q(&m, "limit").parse().unwrap_or(500).min(2000)))
}

fn local_store_get(n: &crate::node::Node, id: &str, key: &str) -> Response {
    if apps::get(n, id).is_none() {
        return err(404, "нет такого приложения");
    }
    ok_json(&json!({ "value": apps::store_get(n, id, key) }))
}

async fn import(State(node): St, Query(m): Q, req: Request) -> Response {
    let name = q(&m, "name").to_string();
    let tmp = match crate::http_local::body_to_tmp(&node, req).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    match apps::import(&node, &name, &tmp) {
        Ok(p) => ok_json(&json!({ "ok": true, "path": p })),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            err(400, e)
        }
    }
}

// ───────────── прокси к службе ─────────────

async fn svc_proxy_root(st: State<Arc<crate::node::Node>>, AxPath(id): AxPath<String>, req: Request) -> Response {
    svc(st.0, id, String::new(), req).await
}
async fn svc_proxy(State(node): St, AxPath((id, rest)): AxPath<(String, String)>, req: Request) -> Response {
    svc(node, id, rest, req).await
}

async fn svc(node: Arc<crate::node::Node>, id: String, rest: String, req: Request) -> Response {
    let Some(a) = apps::get(&node, &id) else { return err(404, "нет такого приложения") };
    if !a.manifest["service"].is_object() || a.manifest["service"]["http"] == false {
        return err(400, "у приложения нет службы с HTTP");
    }
    let port = match apps::service_port(&node, &id).await {
        Ok(p) if p > 0 => p,
        Ok(_) => return err(500, "служба без порта"),
        Err(e) => return err(409, e),
    };
    let qs = req.uri().query().map(|x| format!("?{}", x)).unwrap_or_default();
    let url = format!("http://127.0.0.1:{}/{}{}", port, rest.trim_start_matches('/'), qs);
    let method = reqwest::Method::from_bytes(req.method().as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let ctype = req.headers().get("content-type").and_then(|v| v.to_str().ok()).map(String::from);
    let body = match axum::body::to_bytes(req.into_body(), 64 << 20).await {
        Ok(b) => b,
        Err(_) => return err(413, "запрос к службе больше 64 МБ"),
    };
    let client = reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(120)).build().unwrap_or_default();
    let mut rb = client.request(method, &url).body(body.to_vec());
    if let Some(c) = ctype {
        rb = rb.header("content-type", c);
    }
    match rb.send().await {
        Ok(r) => {
            let code = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let ct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).map(String::from);
            let bytes = r.bytes().await.unwrap_or_default();
            let mut resp = Response::new(Body::from(bytes));
            *resp.status_mut() = code;
            if let Some(ct) = ct.and_then(|c| HeaderValue::from_str(&c).ok()) {
                resp.headers_mut().insert("content-type", ct);
            }
            resp
        }
        Err(e) => err(502, format!("служба не ответила: {}", e)),
    }
}

// ───────────── файлы интерфейса ─────────────

fn mime(p: &str) -> &'static str {
    match p.rsplit('.').next().unwrap_or("").to_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Политика для документов приложения: песочница (даже если страницу открыть напрямую), свои файлы, данные и blob.
fn app_csp(node: &crate::node::Node, a: &apps::App) -> String {
    let base = apps::base_url(node);
    let lock = if a.perms.iter().any(|p| p == "input.capture") { " allow-pointer-lock" } else { "" };
    // адреса из app.json `network` (показаны хозяину при разрешении; входят в отпечаток)
    let net: String = a.network.iter().map(|u| format!(" {}", u)).collect();
    format!(
        "sandbox allow-scripts allow-forms{lock}; default-src 'self' {base} data: blob:; script-src 'self' {base} 'unsafe-inline' 'unsafe-eval' 'wasm-unsafe-eval' blob:; style-src 'self' {base} 'unsafe-inline'; img-src 'self' {base} data: blob:; media-src 'self' {base} data: blob: mediastream:; font-src 'self' {base} data:; connect-src 'self' {base} data: blob:{net}; worker-src 'self' {base} blob:; frame-src 'none'; object-src 'none'; frame-ancestors http://127.0.0.1:* http://localhost:* bridge: http://bridge.localhost https://bridge.localhost"
    )
}

fn file_response(path: &std::path::Path, ct: &str) -> Response {
    match std::fs::read(path) {
        Ok(b) => {
            let mut r = Response::new(Body::from(b));
            let h = r.headers_mut();
            h.insert("Content-Type", HeaderValue::from_str(ct).unwrap_or(HeaderValue::from_static("application/octet-stream")));
            h.insert("Cache-Control", HeaderValue::from_static("no-cache"));
            // модули и шрифты из изолированной рамки (её адрес — «null») — только с CORS
            h.insert("Access-Control-Allow-Origin", HeaderValue::from_static("*"));
            h.insert("X-Content-Type-Options", HeaderValue::from_static("nosniff"));
            r
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn ui_file(State(node): St, AxPath((id, path)): AxPath<(String, String)>) -> Response {
    let Some(a) = apps::get(&node, &id) else { return StatusCode::NOT_FOUND.into_response() };
    if !apps::consent_ok(&node, &a) || !a.unsupported.is_empty() {
        return err(403, "приложение не разрешено на этом ПК");
    }
    let Some(p) = apps::ui_file(&node, &id, &path) else { return StatusCode::NOT_FOUND.into_response() };
    let ct = mime(&path);
    let mut r = file_response(&p, ct);
    if ct.starts_with("text/html") {
        if let Ok(v) = HeaderValue::from_str(&app_csp(&node, &a)) {
            r.headers_mut().insert("Content-Security-Policy", v);
        }
    }
    r
}

async fn icon(State(node): St, AxPath(id): AxPath<String>) -> Response {
    match apps::icon_file(&node, &id) {
        Some(p) => {
            let ct = mime(&p.to_string_lossy());
            let mut r = file_response(&p, if ct.starts_with("image/") { ct } else { "application/octet-stream" });
            r.headers_mut().insert("Content-Security-Policy", HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'"));
            r
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn sdk() -> Response {
    let mut r = Response::new(Body::from(include_str!("../assets/most-app.js")));
    let h = r.headers_mut();
    h.insert("Content-Type", HeaderValue::from_static("text/javascript; charset=utf-8"));
    h.insert("Cache-Control", HeaderValue::from_static("no-cache"));
    h.insert("Access-Control-Allow-Origin", HeaderValue::from_static("*"));
    r
}

/// Отдельное окно приложения: рамка с интерфейсом, вызовы — через панель, открывшую окно.
async fn apphost() -> Response {
    let mut r = Response::new(Body::from(include_str!("../assets/apphost.html")));
    let h = r.headers_mut();
    h.insert("Content-Type", HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert("Cache-Control", HeaderValue::from_static("no-store"));
    h.insert("Content-Security-Policy", HeaderValue::from_static("default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: http://127.0.0.1:*; frame-src http://127.0.0.1:* http://localhost:*"));
    r
}

// ───────────── API служб и своих окон (токен приложения) ─────────────

fn app_of(node: &crate::node::Node, h: &HeaderMap) -> Option<apps::App> {
    let t = h.get("x-most-app").and_then(|v| v.to_str().ok()).map(String::from).or_else(|| h.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(String::from))?;
    apps::by_token(node, &t)
}

async fn app_me(State(node): St, h: HeaderMap) -> Response {
    let Some(a) = app_of(&node, &h) else { return err(401, "нужен токен приложения (MOST_TOKEN в заголовке X-Most-App)") };
    ok_json(&json!({ "id": a.id, "name": a.name, "version": a.version, "granted": apps::granted(&node, &a), "node": node.node_name(), "data": apps::data_dir(&node, &a.id).display().to_string() }))
}

async fn app_store_get(State(node): St, h: HeaderMap, Query(m): Q) -> Response {
    let Some(a) = app_of(&node, &h) else { return err(401, "нужен токен приложения") };
    ok_json(&json!({ "value": apps::store_get(&node, &a.id, q(&m, "key")) }))
}

async fn app_store_set(State(node): St, h: HeaderMap, Json(v): Json<Value>) -> Response {
    let Some(a) = app_of(&node, &h) else { return err(401, "нужен токен приложения") };
    res(apps::store_set(&node, &a.id, &s(&v, "key"), v["value"].clone()).map(|_| json!({ "ok": true })))
}

async fn app_notify(State(node): St, h: HeaderMap, Json(v): Json<Value>) -> Response {
    let Some(a) = app_of(&node, &h) else { return err(401, "нужен токен приложения") };
    if !apps::granted(&node, &a).iter().any(|p| p == "notify") {
        return err(403, "у приложения нет права notify");
    }
    let text: String = s(&v, "text").chars().take(300).collect();
    node.media.deliver(json!({ "media": "apps", "type": "notify", "id": a.id, "name": a.name, "text": text }));
    ok_json(&json!({ "ok": true }))
}

async fn app_toast(State(node): St, h: HeaderMap, Json(v): Json<Value>) -> Response {
    let Some(a) = app_of(&node, &h) else { return err(401, "нужен токен приложения") };
    let text: String = s(&v, "text").chars().take(300).collect();
    node.media.deliver(json!({ "media": "apps", "type": "toast", "id": a.id, "name": a.name, "text": text }));
    ok_json(&json!({ "ok": true }))
}
