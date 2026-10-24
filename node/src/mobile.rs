//! Телефон. Мост раздаёт мобильную страницу (/m/). Телефон подключается один раз по QR-коду из панели и получает
//! свой ключ (cookie). Дальше телефон действует от имени этого ПК: отправляет файлы сюда или соседям, получает
//! файлы «на телефон», смотрит общие папки. Права — «полные» или «только файлы»; отключить телефон — в панели.
//! Вне дома — через Tailscale (HTTPS раздаёт сама Tailscale, см. tailscale.rs).

use crate::http_peer::{err, ok_json, St};
use crate::node::Node;
use crate::tailscale::{self, TsStatus};
use axum::body::Body;
use axum::extract::{ConnectInfo, Path as AxPath, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::Router;
use bridge_core::proto::MailboxEvent;
use bridge_core::scanner::walk;
use bridge_core::{path, time, API_PREFIX, LOCAL_TOKEN_HEADER};
use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;

const PAGE: &str = include_str!("../assets/mobile.html");
const SW: &str = include_str!("../assets/sw.js");
const ICON_192: &[u8] = include_bytes!("../assets/icon-192.png");
const ICON_512: &[u8] = include_bytes!("../assets/icon-512.png");
pub const COOKIE: &str = "bridge_dev";
/// Заголовок, без которого телефон не может ничего менять: чужая страница не подставит его в запрос (защита от подделки запросов).
pub const CSRF_HEADER: &str = "x-bridge-m";
const CODE_TTL: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// full — всё, что умеет панель (кроме голоса и управления телефонами); files — только файлы.
    pub rights: String,
    pub token_sha: String,
    /// Ключи того же телефона на других адресах (домашняя сеть и HTTPS Tailscale — у браузера это разные сайты).
    pub more_tokens: Vec<String>,
    pub created: String,
    pub last_seen: String,
    pub last_ip: String,
    /// Что из «на телефон» уже скачано: «путь|размер|время».
    pub got: Vec<String>,
}

impl Device {
    pub fn view(&self) -> Value {
        let online = time::parse(&self.last_seen).map(|t| (time::now() - t).num_seconds() < 90).unwrap_or(false);
        json!({ "id": self.id, "name": self.name, "rights": self.rights, "created": self.created, "lastSeen": self.last_seen, "lastIp": self.last_ip, "online": online })
    }
}

struct Code {
    code: String,
    until: Instant,
    rights: String,
    /// Перенос входа уже подключённого телефона на другой адрес.
    device: String,
}

#[derive(Default)]
pub struct Mobile {
    devices: Mutex<Vec<Device>>,
    codes: Mutex<Vec<Code>>,
    fails: Mutex<Vec<Instant>>,
    saved: Mutex<Option<Instant>>,
    ts: Mutex<Option<(Instant, TsStatus)>>,
    /// Последняя попытка включить HTTPS: ссылка «включить HTTPS в Tailscale» или ошибка.
    pub https_note: Mutex<(String, String)>,
}

fn file(node: &Node) -> PathBuf {
    node.data_dir.join("devices.json")
}

fn sha(t: &str) -> String {
    hex::encode(Sha256::digest(t.as_bytes()))
}

/// Имя папки для телефона: без запрещённых в Windows знаков.
pub fn folder_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_end_matches('.').chars().take(40).collect::<String>();
    if s.is_empty() {
        "Телефон".into()
    } else {
        s
    }
}

impl Mobile {
    pub fn load(&self, node: &Node) {
        if let Ok(t) = std::fs::read_to_string(file(node)) {
            if let Ok(v) = serde_json::from_str::<Vec<Device>>(t.trim_start_matches('\u{feff}')) {
                *self.devices.lock() = v;
            }
        }
    }

    fn save(&self, node: &Node) {
        let text = serde_json::to_string_pretty(&*self.devices.lock()).unwrap_or_default();
        let _ = bridge_core::write_atomic(&file(node), &text);
        *self.saved.lock() = Some(Instant::now());
    }

    pub fn list(&self) -> Vec<Device> {
        self.devices.lock().clone()
    }

    pub fn find(&self, key: &str) -> Option<Device> {
        self.devices.lock().iter().find(|d| d.id == key || bridge_core::eq_ic(&d.name, key)).cloned()
    }

    /// Код для подключения: живёт 10 минут, действует один раз.
    pub fn new_code(&self, rights: &str) -> (String, Instant) {
        self.code_for(rights, "")
    }

    fn code_for(&self, rights: &str, device: &str) -> (String, Instant) {
        const ABC: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
        let code: String = (0..8).map(|_| ABC[rand::random::<usize>() % ABC.len()] as char).collect();
        let until = Instant::now() + CODE_TTL;
        let mut g = self.codes.lock();
        g.retain(|c| c.until > Instant::now());
        g.push(Code { code: code.clone(), until, rights: if rights == "files" { "files".into() } else { "full".into() }, device: device.into() });
        (code, until)
    }

    /// Обменять код на ключ телефона. Подбор кода закрыт: 10 ошибок за 10 минут — все коды сгорают.
    pub fn pair(&self, node: &Node, code: &str, name: &str, ip: &str) -> Result<(Device, String), String> {
        let code = code.trim().to_uppercase().replace(['-', ' '], "");
        let (rights, device) = {
            let mut g = self.codes.lock();
            g.retain(|c| c.until > Instant::now());
            match g.iter().position(|c| c.code == code) {
                Some(i) => {
                    let c = g.remove(i);
                    (c.rights, c.device)
                }
                None => {
                    let mut f = self.fails.lock();
                    f.retain(|t| t.elapsed() < CODE_TTL);
                    f.push(Instant::now());
                    if f.len() >= 10 {
                        g.clear();
                        f.clear();
                        node.info("Телефон: слишком много неверных кодов — все коды подключения отменены");
                    }
                    return Err("код не подходит или устарел — покажите на ПК новый QR-код".into());
                }
            }
        };
        let token = hex::encode(rand::random::<[u8; 32]>());
        if !device.is_empty() {
            let d = self
                .update(node, &device, |d| {
                    d.more_tokens.push(sha(&token));
                    let n = d.more_tokens.len();
                    if n > 4 {
                        d.more_tokens.drain(0..n - 4);
                    }
                })
                .ok_or("этот телефон уже отключён")?;
            node.info(&format!("Телефон «{}» открыл мост по новому адресу", d.name));
            return Ok((d, token));
        }
        let mut name = name.trim().chars().take(40).collect::<String>();
        if name.is_empty() {
            name = "Телефон".into();
        }
        {
            // одинаковые имена — у каждого телефона своя папка
            let g = self.devices.lock();
            let base = name.clone();
            let mut n = 2;
            while g.iter().any(|d| bridge_core::eq_ic(&folder_name(&d.name), &folder_name(&name))) {
                name = format!("{} {}", base, n);
                n += 1;
            }
        }
        let now = time::format(&time::now());
        let d = Device { id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(), name, rights, token_sha: sha(&token), more_tokens: Vec::new(), created: now.clone(), last_seen: now, last_ip: ip.into(), got: Vec::new() };
        self.devices.lock().push(d.clone());
        self.save(node);
        node.info(&format!("Телефон «{}» подключён ({})", d.name, if d.rights == "files" { "только файлы" } else { "полный доступ" }));
        node.emit("device-paired", json!({ "id": d.id, "name": d.name }));
        Ok((d, token))
    }

    pub fn auth(&self, node: &Node, h: &HeaderMap, ip: &str) -> Option<Device> {
        let token = cookie(h, COOKIE).or_else(|| h.get("x-bridge-device").and_then(|v| v.to_str().ok()).map(String::from))?;
        if token.len() < 32 {
            return None;
        }
        let s = sha(&token);
        let mut g = self.devices.lock();
        let d = g.iter_mut().find(|d| d.token_sha == s || d.more_tokens.contains(&s))?;
        d.last_seen = time::format(&time::now());
        if !ip.is_empty() {
            d.last_ip = ip.into();
        }
        let out = d.clone();
        drop(g);
        if self.saved.lock().map(|t| t.elapsed() > Duration::from_secs(120)).unwrap_or(true) {
            self.save(node);
        }
        Some(out)
    }

    pub fn update(&self, node: &Node, id: &str, f: impl FnOnce(&mut Device)) -> Option<Device> {
        let out = {
            let mut g = self.devices.lock();
            let d = g.iter_mut().find(|d| d.id == id)?;
            f(d);
            d.clone()
        };
        self.save(node);
        Some(out)
    }

    pub fn remove(&self, node: &Node, id: &str) -> bool {
        let removed = {
            let mut g = self.devices.lock();
            let n = g.len();
            g.retain(|d| d.id != id);
            n != g.len()
        };
        if removed {
            self.save(node);
        }
        removed
    }

    pub async fn tailscale(&self, fresh: bool) -> TsStatus {
        if !fresh {
            if let Some((t, s)) = self.ts.lock().clone() {
                if t.elapsed() < Duration::from_secs(15) {
                    return s;
                }
            }
        }
        let s = tailscale::status().await;
        *self.ts.lock() = Some((Instant::now(), s.clone()));
        s
    }
}

fn cookie(h: &HeaderMap, name: &str) -> Option<String> {
    for v in h.get_all("cookie") {
        let Ok(s) = v.to_str() else { continue };
        for part in s.split(';') {
            if let Some((k, val)) = part.trim().split_once('=') {
                if k == name {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

/// Пришёл через прокси (Tailscale Serve и т. п.): такой запрос — не «с этого ПК», даже если пришёл с 127.0.0.1.
pub fn proxied(h: &HeaderMap) -> bool {
    h.contains_key("x-forwarded-for") || h.contains_key("forwarded") || h.contains_key("x-forwarded-host") || h.keys().any(|k| k.as_str().starts_with("tailscale-"))
}

/// Адрес клиента: за прокси — из X-Forwarded-For.
fn client_ip(h: &HeaderMap, addr: SocketAddr) -> String {
    h.get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.split(',').next()).map(|v| v.trim().to_string()).unwrap_or_else(|| bridge_core::config::canon_ip(addr.ip()).to_string())
}

fn is_https(h: &HeaderMap) -> bool {
    h.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https")
}

/// Что телефону можно в локальном API. Всегда нельзя: ключи, управление телефонами, голос и пульт панели этого ПК.
pub fn allowed(rights: &str, method: &Method, rest: &str) -> bool {
    let rest = rest.trim_matches('/');
    const NEVER: &[&str] = &["secret", "open", "ui", "ui/claim", "ui/result"];
    if NEVER.contains(&rest) || rest.starts_with("control") || rest.starts_with("devices") || rest.starts_with("mobile") {
        return false;
    }
    if rights == "full" {
        return true;
    }
    const READ: &[&str] = &["avatar", "avatars", "joinsound", "stickers", "sticker", "overview", "status", "peers", "folders", "fs/list", "fs/file", "mailbox", "mailbox/outgoing", "mailbox/incoming", "mailbox/incoming/file", "transfers", "events", "changelog", "files", "trash"];
    const WRITE: &[&str] = &["mailbox/upload", "mailbox/cancel", "fs/upload"];
    if method == Method::GET {
        READ.contains(&rest)
    } else {
        WRITE.contains(&rest)
    }
}

pub fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/m", get(|| async { axum::response::Redirect::permanent("/m/") }))
        .route("/m/", get(page))
        .route("/m/index.html", get(page))
        .route("/m/sw.js", get(sw))
        .route("/m/manifest.webmanifest", get(manifest))
        .route("/m/icon-192.png", get(|| async { png(ICON_192) }))
        .route("/m/icon-512.png", get(|| async { png(ICON_512) }))
        .route("/m/fonts/:name", get(font))
        .route("/m/pair", post(pair))
        .route("/m/me", get(me))
        .route("/m/rename", post(rename))
        .route("/m/handoff", post(handoff))
        .route("/m/logout", post(logout))
        .route("/m/upload", post(upload))
        .route("/m/outbox", get(outbox))
        .route("/m/outbox/file", get(outbox_file))
        .route("/m/outbox/delete", post(outbox_delete))
        .route("/m/share", post(share_fallback))
        .route("/m/api/*rest", any(proxy))
}

fn html_headers(r: &mut Response, ctype: &'static str) {
    let h = r.headers_mut();
    h.insert("Content-Type", HeaderValue::from_static(ctype));
    h.insert("Cache-Control", HeaderValue::from_static("no-cache"));
    h.insert("X-Frame-Options", HeaderValue::from_static("DENY"));
    h.insert("Referrer-Policy", HeaderValue::from_static("no-referrer"));
    h.insert("X-Content-Type-Options", HeaderValue::from_static("nosniff"));
}

async fn page() -> Response {
    let mut r = Response::new(Body::from(PAGE));
    html_headers(&mut r, "text/html; charset=utf-8");
    r.headers_mut().insert(
        "Content-Security-Policy",
        HeaderValue::from_static("default-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self'; font-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"),
    );
    r
}

async fn sw() -> Response {
    let mut r = Response::new(Body::from(SW));
    html_headers(&mut r, "text/javascript; charset=utf-8");
    r.headers_mut().insert("Service-Worker-Allowed", HeaderValue::from_static("/m/"));
    r
}

async fn manifest() -> Response {
    let v = json!({
        "name": "Мост", "short_name": "Мост", "id": "/m/", "start_url": "/m/", "scope": "/m/", "display": "standalone",
        "background_color": "#0B0C1A", "theme_color": "#0B0C1A", "lang": "ru",
        "description": "Файлы между телефоном, этим ПК и соседями",
        "icons": [
            { "src": "/m/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any maskable" },
            { "src": "/m/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any maskable" }
        ],
        "share_target": {
            "action": "/m/share", "method": "POST", "enctype": "multipart/form-data",
            "params": { "title": "title", "text": "text", "url": "url", "files": [{ "name": "files", "accept": ["*/*"] }] }
        }
    });
    let mut r = Response::new(Body::from(v.to_string()));
    html_headers(&mut r, "application/manifest+json; charset=utf-8");
    r
}

fn png(b: &'static [u8]) -> Response {
    let mut r = Response::new(Body::from(b));
    r.headers_mut().insert("Content-Type", HeaderValue::from_static("image/png"));
    r.headers_mut().insert("Cache-Control", HeaderValue::from_static("public, max-age=86400"));
    r
}

async fn font(p: AxPath<String>) -> Response {
    crate::http_local::font(p).await
}

/// «Поделиться» без service worker (первый запуск): файлы не сохранить — вернуть на страницу с подсказкой.
async fn share_fallback() -> Response {
    axum::response::Redirect::to("/m/#share-failed").into_response()
}

fn need_auth() -> Response {
    let mut r = err(401, "телефон не подключён к этому ПК");
    r.headers_mut().insert("Cache-Control", HeaderValue::from_static("no-store"));
    r
}

fn dev_of(node: &Node, req_h: &HeaderMap, addr: SocketAddr) -> Option<Device> {
    node.mobile.auth(node, req_h, &client_ip(req_h, addr))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PairReq {
    code: String,
    name: String,
}

async fn pair(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, h: HeaderMap, axum::Json(b): axum::Json<PairReq>) -> Response {
    if !node.cfg.read().mobile.enabled {
        return err(403, "телефоны на этом ПК выключены");
    }
    match node.mobile.pair(&node, &b.code, &b.name, &client_ip(&h, addr)) {
        Ok((d, token)) => {
            let mut r = ok_json(&json!({ "ok": true, "device": d.view(), "node": node.node_name() }));
            let c = format!("{}={}; Path=/; Max-Age=315360000; HttpOnly; SameSite=Strict{}", COOKIE, token, if is_https(&h) { "; Secure" } else { "" });
            if let Ok(v) = HeaderValue::from_str(&c) {
                r.headers_mut().insert("Set-Cookie", v);
            }
            r
        }
        Err(e) => err(403, e),
    }
}

async fn me(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, h: HeaderMap) -> Response {
    let Some(d) = dev_of(&node, &h, addr) else { return json_401(&node) };
    let peers: Vec<Value> = {
        let c = node.cfg.read();
        let st = node.peers.lock();
        c.peers
            .iter()
            .filter(|p| p.enabled && !p.name.is_empty())
            .map(|p| {
                let s = st.get(&p.name);
                json!({ "name": p.name, "title": p.title, "online": s.map(|s| s.online).unwrap_or(false), "lastSeen": s.and_then(|s| s.last_seen.map(|t| time::format(&t))) })
            })
            .collect()
    };
    let https = https_url(&node).await;
    ok_json(&json!({
        "paired": true, "device": d.view(), "node": node.node_name(), "version": bridge_core::APP_VERSION, "peers": peers,
        "https": is_https(&h), "httpsUrl": https, "inbox": format!("{}/{}", node.cfg.read().mailbox.incoming, folder_name(&d.name)),
    }))
}

fn json_401(node: &Node) -> Response {
    let mut r = crate::http_peer::json_code(401, &json!({ "paired": false, "error": "телефон не подключён к этому ПК", "enabled": node.cfg.read().mobile.enabled }));
    r.headers_mut().insert("Cache-Control", HeaderValue::from_static("no-store"));
    r
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NameReq {
    name: String,
}

async fn rename(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, h: HeaderMap, axum::Json(b): axum::Json<NameReq>) -> Response {
    let Some(d) = dev_of(&node, &h, addr) else { return need_auth() };
    let name: String = b.name.trim().chars().take(40).collect();
    if name.is_empty() {
        return err(400, "пустое имя");
    }
    let d = node.mobile.update(&node, &d.id, |x| x.name = name).unwrap_or(d);
    ok_json(&json!({ "ok": true, "device": d.view() }))
}

/// Перейти на HTTPS-адрес без повторного QR: одноразовый код для этого же телефона.
async fn handoff(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, h: HeaderMap) -> Response {
    let Some(d) = dev_of(&node, &h, addr) else { return need_auth() };
    let base = https_url(&node).await;
    if base.is_empty() {
        return err(400, "HTTPS через Tailscale на ПК не включён");
    }
    let (code, _) = node.mobile.code_for(&d.rights, &d.id);
    ok_json(&json!({ "url": format!("{}/m/#p={}", base, code) }))
}

async fn logout(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, h: HeaderMap) -> Response {
    let Some(d) = dev_of(&node, &h, addr) else { return need_auth() };
    node.mobile.remove(&node, &d.id);
    node.info(&format!("Телефон «{}» отключился сам", d.name));
    let mut r = ok_json(&json!({ "ok": true }));
    r.headers_mut().insert("Set-Cookie", HeaderValue::from_static("bridge_dev=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict"));
    r
}

fn qmap(m: &HashMap<String, String>, k: &str) -> String {
    m.get(k).cloned().unwrap_or_default()
}

/// С телефона на этот ПК: в «приём/<имя телефона>/…», как файлы от соседей.
async fn upload(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, Query(m): Query<HashMap<String, String>>, req: Request) -> Response {
    let Some(d) = dev_of(&node, req.headers(), addr) else { return need_auth() };
    let rel = match path::normalize(&qmap(&m, "path")) {
        Ok(p) => p,
        Err(w) => return err(400, format!("имя файла: {}", w)),
    };
    let size: i64 = qmap(&m, "size").parse().unwrap_or(0);
    let max = node.max_file_bytes();
    if size > max {
        return err(413, "файл больше предела (maxFileMb)");
    }
    let dir = node.mailbox.in_root(&node).join(folder_name(&d.name));
    let Ok(mut full) = path::to_local(&dir, &rel) else { return err(400, "имя файла") };
    let tmp = node.tmp_file();
    let g = node.transfers.start("recv", &d.name, "телефон", &rel, size);
    {
        let Ok(mut f) = tokio::fs::File::create(&tmp).await else { return err(500, "не удалось создать временный файл") };
        let mut total: i64 = 0;
        let mut s = req.into_body().into_data_stream();
        while let Some(c) = s.next().await {
            let Ok(c) = c else {
                drop(f);
                let _ = tokio::fs::remove_file(&tmp).await;
                return err(400, "передача оборвалась");
            };
            total += c.len() as i64;
            g.add(c.len());
            if total > max {
                drop(f);
                let _ = tokio::fs::remove_file(&tmp).await;
                return err(413, "файл больше предела (maxFileMb)");
            }
            if let Err(e) = f.write_all(&c).await {
                drop(f);
                let _ = tokio::fs::remove_file(&tmp).await;
                return err(500, e.to_string());
            }
        }
        if size > 0 && total != size {
            drop(f);
            let _ = tokio::fs::remove_file(&tmp).await;
            return err(400, "передача оборвалась");
        }
        let _ = f.flush().await;
    }
    drop(g);
    if let Some(p) = full.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    if full.exists() {
        let stem = full.file_stem().and_then(|s| s.to_str()).unwrap_or("файл").to_string();
        let ext = full.extension().and_then(|s| s.to_str()).map(|e| format!(".{}", e)).unwrap_or_default();
        full = full.with_file_name(format!("{} ({}){}", stem, chrono::Local::now().format("%H%M%S"), ext));
    }
    if let Err(e) = bridge_core::files::move_replace(&tmp, &full) {
        let _ = std::fs::remove_file(&tmp);
        return err(500, e.to_string());
    }
    let shown = full.strip_prefix(node.mailbox.in_root(&node)).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or(rel.clone());
    let bytes = std::fs::metadata(&full).map(|x| x.len() as i64).unwrap_or(0);
    node.mailbox.remember(&node, MailboxEvent { kind: "received".into(), peer: d.name.clone(), path: shown.clone(), size: bytes, ..Default::default() });
    ok_json(&json!({ "ok": true, "path": shown, "size": bytes }))
}

pub fn outbox_dir(node: &Node, d: &Device) -> PathBuf {
    let c = node.cfg.read();
    node.resolve(&c.mobile.outbox).join(folder_name(&d.name))
}

fn got_key(rel: &str, size: i64, mtime: i64) -> String {
    format!("{}|{}|{}", rel, size, mtime)
}

async fn outbox(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, h: HeaderMap) -> Response {
    let Some(d) = dev_of(&node, &h, addr) else { return need_auth() };
    let dir = outbox_dir(&node, &d);
    let _ = std::fs::create_dir_all(&dir);
    let mut items = walk(&dir, &[]);
    items.retain(|w| !path::is_junk(&w.rel));
    items.sort_by(|a, b| b.mtime_ticks.cmp(&a.mtime_ticks));
    let list: Vec<Value> = items
        .iter()
        .take(500)
        .map(|w| json!({ "path": w.rel, "size": w.size, "time": time::format(&time::from_ticks(w.mtime_ticks)), "got": d.got.contains(&got_key(&w.rel, w.size, w.mtime_ticks)) }))
        .collect();
    let rel_dir = format!("{}/{}", node.cfg.read().mobile.outbox, folder_name(&d.name));
    ok_json(&json!({ "dir": rel_dir, "files": list }))
}

async fn outbox_file(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, Query(m): Query<HashMap<String, String>>, h: HeaderMap) -> Response {
    let Some(d) = dev_of(&node, &h, addr) else { return need_auth() };
    let Some(rel) = path::norm(&qmap(&m, "path")) else { return err(400, "путь") };
    let Ok(full) = path::to_local(&outbox_dir(&node, &d), &rel) else { return err(400, "путь") };
    let Ok(meta) = std::fs::metadata(&full) else { return err(404, "нет файла") };
    if !meta.is_file() {
        return err(404, "нет файла");
    }
    let mtime = time::to_ticks(&meta.modified().map(time::from_system).unwrap_or_else(|_| time::min()));
    let key = got_key(&rel, meta.len() as i64, mtime);
    let Ok(f) = tokio::fs::File::open(&full).await else { return err(404, "нет файла") };
    let g = node.transfers.start("serve", &d.name, "телефон", &rel, meta.len() as i64);
    let (n2, id) = (node.clone(), d.id.clone());
    let name = full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let total = meta.len();
    let mut sent: u64 = 0;
    if total == 0 {
        n2.mobile.update(&n2, &id, |x| x.got.push(key.clone()));
    }
    let mut mark = if total == 0 { None } else { Some((n2, id, key)) };
    let body = tokio_util::io::ReaderStream::with_capacity(f, 256 * 1024).map(move |c| {
        if let Ok(b) = &c {
            g.add(b.len());
            sent += b.len() as u64;
            // отдали целиком — отметить «скачано»
            if sent >= total {
                if let Some((n2, id, key)) = mark.take() {
                    n2.mobile.update(&n2, &id, |x| {
                        if !x.got.contains(&key) {
                            x.got.push(key);
                            let n = x.got.len();
                            if n > 2000 {
                                x.got.drain(0..n - 2000);
                            }
                        }
                    });
                }
            }
        }
        c
    });
    let mut r = Response::new(Body::from_stream(body));
    let hh = r.headers_mut();
    hh.insert("Content-Type", HeaderValue::from_static("application/octet-stream"));
    if let Ok(v) = HeaderValue::from_str(&meta.len().to_string()) {
        hh.insert("Content-Length", v);
    }
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename*=UTF-8''{}", crate::client::esc(&name))) {
        hh.insert("Content-Disposition", v);
    }
    r
}

async fn outbox_delete(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, Query(m): Query<HashMap<String, String>>, h: HeaderMap) -> Response {
    let Some(d) = dev_of(&node, &h, addr) else { return need_auth() };
    let Some(rel) = path::norm(&qmap(&m, "path")) else { return err(400, "путь") };
    let Ok(full) = path::to_local(&outbox_dir(&node, &d), &rel) else { return err(400, "путь") };
    if !full.is_file() {
        return err(404, "нет файла");
    }
    match std::fs::remove_file(&full) {
        Ok(()) => ok_json(&json!({ "ok": true })),
        Err(e) => err(500, e.to_string()),
    }
}

/// Положить файл «на телефон» (из панели, ассистента или с диска).
pub fn put_outbox(node: &Node, d: &Device, name: &str, src: &std::path::Path, mv: bool) -> Result<String, String> {
    let rel = path::normalize(name).map_err(|w| format!("имя файла: {}", w))?;
    let dir = outbox_dir(node, d);
    let mut full = path::to_local(&dir, &rel).map_err(|_| "имя файла".to_string())?;
    if let Some(p) = full.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    if full.exists() {
        let stem = full.file_stem().and_then(|s| s.to_str()).unwrap_or("файл").to_string();
        let ext = full.extension().and_then(|s| s.to_str()).map(|e| format!(".{}", e)).unwrap_or_default();
        full = full.with_file_name(format!("{} ({}){}", stem, chrono::Local::now().format("%H%M%S"), ext));
    }
    if mv {
        bridge_core::files::move_replace(src, &full).map_err(|e| e.to_string())?;
    } else {
        std::fs::copy(src, &full).map_err(|e| e.to_string())?;
    }
    let shown = full.strip_prefix(&dir).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or(rel);
    node.emit("device-outbox", json!({ "device": d.name, "path": shown }));
    node.info(&format!("На телефон «{}»: {}", d.name, shown));
    Ok(shown)
}

/// Запрос пришёл с телефона (через /m/api/…): имя, адрес телефона и имя, по которому телефон открыл мост.
#[derive(Clone, Debug)]
pub struct FromDevice {
    pub name: String,
    pub ip: String,
    pub host: String,
}

/// Локальный API для телефона: те же вызовы, что у панели, но с ключом телефона и в пределах его прав.
async fn proxy(State(node): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, AxPath(rest): AxPath<String>, req: Request) -> Response {
    let Some(d) = dev_of(&node, req.headers(), addr) else { return need_auth() };
    if !allowed(&d.rights, req.method(), &rest) {
        return err(403, if d.rights == "files" { "телефону с правами «только файлы» это недоступно" } else { "с телефона это недоступно" });
    }
    let ip = client_ip(req.headers(), addr);
    let host = req.headers().get("x-forwarded-host").or_else(|| req.headers().get("host")).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let host = if host.starts_with('[') { host.split(']').next().unwrap_or("").trim_start_matches('[').to_string() } else { host.split(':').next().unwrap_or("").to_string() };
    let (mut parts, body) = req.into_parts();
    parts.extensions.insert(FromDevice { name: d.name.clone(), ip, host });
    let q = parts.uri.query().map(|q| format!("?{}", q)).unwrap_or_default();
    let Ok(uri) = format!("{}/local/{}{}", API_PREFIX, rest.trim_start_matches('/'), q).parse() else { return err(400, "адрес") };
    parts.uri = uri;
    let names: Vec<_> = parts.headers.keys().filter(|k| k.as_str().starts_with("tailscale-") || k.as_str().starts_with("x-forwarded-")).cloned().collect();
    for k in names {
        parts.headers.remove(&k);
    }
    for k in ["forwarded", "cookie", "origin", "referer"] {
        parts.headers.remove(k);
    }
    parts.headers.insert("host", HeaderValue::from_static("127.0.0.1"));
    if let Ok(v) = HeaderValue::from_str(&node.local_token) {
        parts.headers.insert(LOCAL_TOKEN_HEADER, v);
    }
    parts.extensions.insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    let app = crate::http_peer::router(node.clone());
    match app.oneshot(Request::from_parts(parts, body)).await {
        Ok(r) => r,
        Err(e) => match e {},
    }
}

// ───────────── для панели ─────────────

/// HTTPS-адрес мобильной страницы через Tailscale, если включён.
pub async fn https_url(node: &Arc<Node>) -> String {
    let (on, port) = {
        let c = node.cfg.read();
        (c.mobile.https == "tailscale", c.mobile.https_port)
    };
    if !on {
        return String::new();
    }
    let ts = node.mobile.tailscale(false).await;
    if ts.dns.is_empty() {
        return String::new();
    }
    if port == 443 || port == 0 {
        format!("https://{}", ts.dns)
    } else {
        format!("https://{}:{}", ts.dns, port)
    }
}

/// Адреса, по которым телефон может открыть мост: HTTPS Tailscale, адрес Tailscale, домашняя сеть.
pub async fn addresses(node: &Arc<Node>) -> Vec<Value> {
    let mut out = Vec::new();
    let https = https_url(node).await;
    if !https.is_empty() {
        out.push(json!({ "kind": "https", "label": "Tailscale · HTTPS", "base": https, "note": "дома и вне дома; можно установить как приложение и отправлять через «Поделиться»" }));
    }
    let listen = node.cfg.read().listen.clone();
    let parsed: Vec<_> = listen.iter().filter_map(|l| bridge_core::config::parse_listen(l)).collect();
    let any = parsed.iter().any(|(ip, _)| ip.is_unspecified());
    let Some(port) = parsed.iter().find(|(ip, _)| !ip.is_loopback()).or(parsed.first()).map(|(_, p)| *p) else { return out };
    let bound = node.bound.lock().clone();
    let mut lan = Vec::new();
    for (ip, name) in crate::http_peer::interface_ips() {
        if !any && !bound.contains(&ip) {
            continue;
        }
        let base = format!("http://{}:{}", ip, port);
        if tailscale::is_ts_ip(&ip) {
            out.push(json!({ "kind": "tailscale", "label": "Tailscale", "base": base, "note": "дома и вне дома, если на телефоне включён Tailscale" }));
        } else {
            let radmin = name.to_lowercase().contains("radmin") || matches!(ip, std::net::IpAddr::V4(v) if v.octets()[0] == 26);
            if radmin || is_virtual(&name) {
                continue; // Radmin, VPN, WSL, виртуальные машины — телефон до них не достанет
            }
            lan.push((lan_rank(&ip), json!({ "kind": "lan", "label": format!("Wi-Fi дома · {}", ip), "base": base, "note": format!("только когда телефон в той же сети Wi-Fi, что и ПК (подключение «{}»), и VPN на телефоне выключен", name) })));
        }
    }
    lan.sort_by_key(|x| x.0);
    out.extend(lan.into_iter().map(|x| x.1));
    out
}

/// Виртуальные и VPN-адаптеры: WSL, Hyper-V, VirtualBox, VMware, Docker, туннели VPN (Happ, WireGuard, OpenVPN…).
pub fn is_virtual(name: &str) -> bool {
    let n = name.to_lowercase();
    ["vethernet", "wsl", "hyper-v", "virtualbox", "vmware", "vmnet", "docker", "tun", "tap", "wintun", "wireguard", "openvpn", "happ", "hamachi", "loopback", "bluetooth", "vpn", "outline", "v2ray", "xray", "clash", "nekobox", "sing-box"]
        .iter()
        .any(|k| n.contains(k))
}

/// Домашняя сеть: 192.168.x — обычно Wi-Fi-роутер, потом 10.x, потом 172.16–31.x (часто виртуальные сети).
fn lan_rank(ip: &std::net::IpAddr) -> u8 {
    match ip {
        std::net::IpAddr::V4(v) => {
            let o = v.octets();
            if o[0] == 192 && o[1] == 168 {
                0
            } else if o[0] == 10 {
                1
            } else if v.is_private() {
                2
            } else {
                3
            }
        }
        _ => 4,
    }
}

pub fn qr_svg(text: &str) -> String {
    match qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M) {
        Ok(c) => c.render::<qrcode::render::svg::Color>().min_dimensions(232, 232).quiet_zone(true).dark_color(qrcode::render::svg::Color("#0B0C1A")).light_color(qrcode::render::svg::Color("#FFFFFF")).build(),
        Err(_) => String::new(),
    }
}

/// Проверка при запуске: если HTTPS через Tailscale был включён — убедиться, что Tailscale по-прежнему раздаёт мост.
pub fn schedule(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(4)).await;
        loop {
            let on = node.cfg.read().mobile.https == "tailscale";
            if on {
                let ts = node.mobile.tailscale(true).await;
                if ts.running {
                    let target = serve_target(&node);
                    let port = node.cfg.read().mobile.https_port;
                    let present = tailscale::serve_list().await.iter().any(|(p, t)| (*p == port || port == 0) && t.trim_end_matches('/') == target);
                    if !present {
                        match tailscale::serve_on(&target, port).await {
                            tailscale::ServeResult::On(p) => {
                                node.info(&format!("Телефон: Tailscale снова раздаёт мост по HTTPS (порт {})", p));
                                *node.mobile.https_note.lock() = Default::default();
                            }
                            tailscale::ServeResult::NeedEnable(u) => *node.mobile.https_note.lock() = (u, String::new()),
                            tailscale::ServeResult::Failed(e) => {
                                node.info_once("ts-serve", &format!("Телефон: не удалось включить HTTPS в Tailscale: {}", e), 3600);
                                *node.mobile.https_note.lock() = (String::new(), e);
                            }
                        }
                    }
                }
            }
            tokio::select! {
                _ = node.shutdown.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(600)) => {}
            }
        }
    });
}

/// Куда Tailscale ведёт HTTPS: мост на этом ПК (127.0.0.1, если слушаем его).
pub fn serve_target(node: &Node) -> String {
    let listen = node.cfg.read().listen.clone();
    let parsed: Vec<_> = listen.iter().filter_map(|l| bridge_core::config::parse_listen(l)).collect();
    let port = parsed.iter().find(|(ip, _)| !ip.is_loopback()).or(parsed.first()).map(|(_, p)| *p).unwrap_or(5390);
    let lo = parsed.iter().find(|(ip, _)| ip.is_loopback() || ip.is_unspecified()).map(|(_, p)| *p);
    match lo {
        Some(p) => format!("http://127.0.0.1:{}", p),
        None => {
            let ip = parsed.iter().find(|(ip, _)| !ip.is_loopback()).map(|(ip, _)| ip.to_string()).unwrap_or_else(|| "127.0.0.1".into());
            format!("http://{}:{}", ip, port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_address_first_virtual_skipped() {
        for n in ["happ-tun", "vEthernet (WSL)", "VirtualBox Host-Only Network", "Radmin VPN", "wintun"] {
            assert!(is_virtual(n), "{}", n);
        }
        for n in ["Ethernet", "Беспроводная сеть", "Wi-Fi", "Ethernet 2"] {
            assert!(!is_virtual(n), "{}", n);
        }
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        assert!(lan_rank(&ip("192.168.1.172")) < lan_rank(&ip("172.18.0.1")));
        assert!(lan_rank(&ip("10.0.0.5")) < lan_rank(&ip("172.30.96.1")));
    }
}
