//! Локальный API (только с этого ПК, с токеном) и панель. Всё, что делает панель, — через этот API: им же пользуются
//! командная строка, скрипты и ассистенты. Поток событий — /local/events (SSE).

use crate::http_peer::{err, json_code, ok_json, St};
use crate::link;
use crate::node::Node;
use crate::shares::{self, ShareRt};
use axum::body::Body;
use axum::extract::{Path as AxPath, Query, Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bridge_core::config::{self, get_ic, is_name, GroupConfig, NodeConfig, PeerConfig, SharedFolderConfig, READ, WRITE};
use bridge_core::ignore::{self, IgnoreRules};
use bridge_core::proto::*;
use bridge_core::scanner::walk;
use bridge_core::{eq_ic, path, time, API_PREFIX};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

include!(concat!(env!("OUT_DIR"), "/ui_files.rs"));
/// Панель не собрана (нет ui/dist при сборке моста) — подсказка вместо неё.
const NO_UI: &str = "<!doctype html><meta charset=utf-8><title>Мост</title><body style='font:16px system-ui;background:#0E0F22;color:#E8EAFB;padding:40px'><h2>Панель не собрана</h2><p>Соберите интерфейс: в папке <code>ui</code> — <code>npm ci</code> и <code>npm run build</code>, затем пересоберите мост (<code>build.cmd</code> делает всё сам).</p>";
/// Запрос пришёл из окна приложения «Мост» (протокол bridge://), а не из браузера.
#[derive(Clone, Copy)]
pub struct FromApp;
const FAVICON: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'><defs><linearGradient id='g' x1='0' x2='1'><stop offset='0' stop-color='#3DF2FF'/><stop offset='1' stop-color='#FF3DCB'/></linearGradient></defs><rect width='32' height='32' rx='8' fill='#0A0920'/><path d='M5 22c5-9 17-9 22 0' stroke='url(#g)' stroke-width='3' fill='none' stroke-linecap='round'/><path d='M9 22v-4M16 22v-7M23 22v-4' stroke='url(#g)' stroke-width='1.6' stroke-linecap='round' opacity='.8'/><circle cx='5' cy='22' r='2.4' fill='#3DF2FF'/><circle cx='27' cy='22' r='2.4' fill='#FF3DCB'/></svg>";
/// Шрифты панели встроены в программу (панель работает без интернета). Exo 2 и Manrope — SIL OFL 1.1.
const FONTS: [(&str, &[u8]); 4] = [
    ("exo2-cyrillic.woff2", include_bytes!("../assets/fonts/exo-2-cyrillic-wght-normal.woff2")),
    ("exo2-latin.woff2", include_bytes!("../assets/fonts/exo-2-latin-wght-normal.woff2")),
    ("manrope-cyrillic.woff2", include_bytes!("../assets/fonts/manrope-cyrillic-wght-normal.woff2")),
    ("manrope-latin.woff2", include_bytes!("../assets/fonts/manrope-latin-wght-normal.woff2")),
];

pub(crate) async fn font(AxPath(name): AxPath<String>) -> Response {
    let Some((_, data)) = FONTS.iter().find(|(n, _)| *n == name) else { return StatusCode::NOT_FOUND.into_response() };
    let mut r = Response::new(Body::from(*data));
    r.headers_mut().insert("Content-Type", HeaderValue::from_static("font/woff2"));
    r.headers_mut().insert("Cache-Control", HeaderValue::from_static("public, max-age=31536000, immutable"));
    r
}

type Q = Query<HashMap<String, String>>;

fn q<'a>(m: &'a HashMap<String, String>, k: &str) -> &'a str {
    m.get(k).map(|s| s.as_str()).unwrap_or("")
}

pub fn routes() -> Router<Arc<Node>> {
    let l = |s: &str| format!("{}/local/{}", API_PREFIX, s);
    Router::new()
        .route("/", get(page))
        .route("/ui", get(page))
        .route("/favicon.svg", get(favicon))
        .route("/assets/fonts/:name", get(font))
        .route("/assets/popout.html", get(popout))
        .route("/app/*path", get(ui_asset))
        .route(&l("overview"), get(overview))
        .route(&l("status"), get(status))
        .route(&l("peers"), get(peers_state).post(peer_upsert))
        .route(&l("peers/remove"), post(peer_remove))
        .route(&l("peers/connect"), post(peer_connect))
        .route(&l("peers/invite"), post(peer_invite))
        .route(&l("peers/check"), get(peer_check))
        .route(&l("peers/rename"), post(peer_rename))
        .route(&l("groups"), get(groups_get).post(groups_set))
        .route(&l("groups/remove"), post(groups_remove))
        .route(&l("access"), get(access_matrix))
        .route(&l("secret"), post(secret))
        .route(&l("log"), get(log))
        .route(&l("events"), get(events_stream))
        .route(&l("sync"), post(sync))
        .route(&l("records"), get(records))
        .route(&l("records/:id"), get(record))
        .route(&l("records/:id/history"), get(record_history))
        .route(&l("actions"), post(actions))
        .route(&l("rejected"), get(rejected))
        .route(&l("wait"), get(wait))
        .route(&l("files"), get(files))
        .route(&l("mailbox"), get(mailbox_status))
        .route(&l("mailbox/outgoing"), get(mailbox_outgoing))
        .route(&l("mailbox/upload"), post(mailbox_upload))
        .route(&l("upload/part"), post(upload_part))
        .route(&l("builds"), get(|State(n): St| async move { ok_json(&crate::builds::status(&n)) }))
        .route(&l("builds/send"), post(builds_send))
        .route(&l("builds/target"), post(builds_target))
        .route(&l("builds/presets"), get(|State(n): St| async move { ok_json(&json!({ "list": crate::builds::presets(&n) })) }).post(builds_presets))
        .route(&l("builds/preview"), post(builds_preview))
        .route(&l("builds/run"), post(|State(n): St, Json(v): Json<Value>| async move { match crate::builds::run(&n, v["peer"].as_str().unwrap_or(""), v["name"].as_str().unwrap_or("")) { Ok(v) => ok_json(&v), Err(e) => err(400, e) } }))
        .route(&l("builds/stop"), post(|State(n): St, Json(v): Json<Value>| async move { match crate::builds::stop(&n, v["peer"].as_str().unwrap_or(""), v["name"].as_str().unwrap_or("")) { Ok(v) => ok_json(&v), Err(e) => err(400, e) } }))
        .route(&l("mailbox/cancel"), post(mailbox_cancel))
        .route(&l("mailbox/incoming"), get(mailbox_incoming))
        .route(&l("mailbox/incoming/file"), get(mailbox_incoming_file))
        .route(&l("open"), post(open_dir))
        .route(&l("ignore"), get(ignore_get).post(ignore_set))
        .route(&l("ignore/add"), post(ignore_add))
        .route(&l("ignore/remove"), post(ignore_remove))
        .route(&l("ignore/test"), get(ignore_test))
        .route(&l("folders"), get(folders_get).post(folders_set))
        .route(&l("folders/remove"), post(folders_remove))
        .route(&l("folders/settings"), post(folders_settings))
        .route(&l("mounts/allow-deletes"), post(allow_deletes))
        .route(&l("mounts/plan"), get(|State(n): St, Query(m): Q| async move { match n.folders.plan(&n, q(&m, "peer"), q(&m, "id")).await { Ok(v) => ok_json(&v), Err((c, e)) => err(c, e) } }))
        .route(&l("trash"), get(trash))
        .route(&l("trash/restore"), post(trash_restore))
        .route(&l("fs/list"), get(fs_list))
        .route(&l("fs/file"), get(fs_file))
        .route(&l("fs/upload"), post(fs_upload))
        .route(&l("fs/delete"), post(fs_delete))
        .route(&l("media/signal"), post(media_signal))
        .route(&l("media/stream"), get(media_stream))
        .route(&l("media/invites"), get(media_invites))
        .route(&l("media/presence"), get(media_presence))
        .route(&l("media/ice"), get(media_ice))
        .route(&l("media/local"), post(media_local))
        .route(&l("transfers"), get(transfers_get))
        .route(&l("changelog"), get(changelog))
        .route(&l("ui"), post(ui_cmd))
        .route(&l("ui/claim"), post(ui_claim))
        .route(&l("ui/result"), post(ui_result))
        .route(&l("mailbox/send"), post(mailbox_send))
        .route(&l("agent"), get(agent_get))
        .route(&l("agent/settings"), post(agent_settings))
        .route(&l("agent/ask"), post(agent_ask))
        .route(&l("agent/thread"), get(agent_thread))
        .route(&l("agent/wait"), get(agent_wait))
        .route(&l("agent/stop"), post(agent_stop))
        .route(&l("agent/delete"), post(agent_delete))
        .route(&l("agent/inbox"), get(agent_inbox))
        .route(&l("agent/wait-task"), get(agent_wait_task))
        .route(&l("agent/take"), post(agent_take))
        .route(&l("agent/progress"), post(agent_progress))
        .route(&l("agent/report"), post(agent_report))
        .route(&l("agent/request"), post(agent_request))
        .route(&l("agent/request/wait"), get(agent_request_wait))
        .route(&l("agent/answer"), post(agent_answer))
        .route(&l("agent/exchange"), post(agent_exchange))
        .route(&l("channels"), get(channels_get).post(channels_set))
        .route(&l("shares"), post(shares_set))
        .route(&l("shares/remove"), post(shares_remove))
        .route(&l("shares/offers"), get(shares_offers))
        .route(&l("channels/remove"), post(channels_remove))
        .route(&l("devices"), get(devices_get).post(devices_set))
        .route(&l("devices/pair"), post(devices_pair))
        .route(&l("devices/remove"), post(devices_remove))
        .route(&l("devices/send"), post(devices_send))
        .route(&l("devices/upload"), post(devices_upload))
        .route(&l("mobile"), get(mobile_get).post(mobile_set))
        .route(&l("mobile/https"), post(mobile_https))
        .route(&l("chat"), get(chat_list))
        .route(&l("chat/messages"), get(chat_messages))
        .route(&l("chat/send"), post(chat_send))
        .route(&l("chat/read"), post(chat_read))
        .route(&l("chat/file"), get(chat_file))
        .route(&l("avatar"), get(avatar_get).post(avatar_set))
        .route(&l("avatar/remove"), post(avatar_remove))
        .route(&l("avatars"), get(avatars))
        .route(&l("tile"), get(tile_get).post(tile_set))
        .route(&l("chan/art"), get(chan_art_get))
        .route(&l("chan/img"), get(chan_img_get).post(chan_img_set))
        .route(&l("chan/img/remove"), post(chan_img_remove))
        .route(&l("chan/style"), post(chan_style_set))
        .route(&l("tile/image"), get(tile_img_get).post(tile_img_set))
        .route(&l("tile/image/remove"), post(|State(n): St| async move { crate::tile::remove_img(&n); ok_json(&json!({ "ok": true })) }))
        .route(&l("tile/status"), post(tile_status_set))
        .route(&l("tile/status/presets"), get(|State(n): St| async move { ok_json(&crate::tile::presets(&n)) }).post(tile_presets_set))
        .route(&l("tile/status/apply"), post(tile_preset_apply))
        .route(&l("tile/status/preset/image"), get(tile_preset_img_get).post(tile_preset_img_set))
        .route(&l("tile/status/preset/image/remove"), post(tile_preset_img_remove))
        .route(&l("tile/status/image"), get(tile_st_img_get).post(tile_st_img_set))
        .route(&l("tile/status/image/remove"), post(|State(n): St| async move { crate::tile::remove_st_img(&n); ok_json(&json!({ "ok": true })) }))
        .route(&l("tile/reset"), post(|State(n): St| async move { crate::tile::reset(&n); ok_json(&json!({ "ok": true })) }))
        .route(&l("joinsound"), get(joinsound_get).post(joinsound_set))
        .route(&l("pingsound"), get(pingsound_get).post(pingsound_set))
        .route(&l("pingsound/remove"), post(pingsound_remove))
        .route(&l("self/muted"), post(self_muted))
        .route(&l("control/state"), get(control_state))
        .route(&l("control/grant"), post(control_grant))
        .route(&l("control/revoke"), post(control_revoke))
        .route(&l("control/input"), post(control_input))
        .route(&l("overlay/ann"), post(overlay_ann))
        .route(&l("board/state"), get(board_state))
        .route(&l("board/poll"), get(board_poll))
        .route(&l("board/op"), post(board_op))
        .route(&l("board/image"), get(board_image_get).post(board_image_put))
        .route(&l("board/stands"), get(board_stands))
        .route(&l("board/stand/save"), post(board_stand_save))
        .route(&l("board/stand/load"), post(board_stand_load))
        .route(&l("board/stand/file"), get(board_stand_file))
        .route(&l("board/stand/import"), post(board_stand_import))
        .route(&l("board/stand/remove"), post(board_stand_remove))
        .route(&l("board/rooms"), get(|State(n): St| async move { bres(crate::board_api::rooms(&n).await) }))
        .route(&l("board/describe"), get(|State(n): St, Query(m): Q| async move { bres(crate::board_api::describe(&n, &qv(&m)).await) }))
        .route(&l("board/find"), get(|State(n): St, Query(m): Q| async move { bres(crate::board_api::find(&n, &qv(&m)).await) }))
        .route(&l("board/get"), get(|State(n): St, Query(m): Q| async move { bres(crate::board_api::get(&n, &qv(&m)).await) }))
        .route(&l("board/add"), post(|State(n): St, Json(v): Json<Value>| async move { bres(crate::board_api::add(&n, &v).await) }))
        .route(&l("board/edit"), post(|State(n): St, Json(v): Json<Value>| async move { bres(crate::board_api::edit(&n, &v).await) }))
        .route(&l("board/move"), post(|State(n): St, Json(v): Json<Value>| async move { bres(crate::board_api::move_(&n, &v).await) }))
        .route(&l("board/remove"), post(|State(n): St, Json(v): Json<Value>| async move { bres(crate::board_api::remove(&n, &v).await) }))
        .route(&l("board/clear"), post(|State(n): St, Json(v): Json<Value>| async move { bres(crate::board_api::clear(&n, &v).await) }))
        .route(&l("board/layer"), post(|State(n): St, Json(v): Json<Value>| async move { bres(crate::board_api::layer(&n, &v).await) }))
        .route(&l("board/bg"), post(|State(n): St, Json(v): Json<Value>| async move { bres(crate::board_api::bg(&n, &v).await) }))
        .route(&l("board/enable"), post(|State(n): St, Json(v): Json<Value>| async move { bres(crate::board_api::enable(&n, &v).await) }))
        .route(&l("board/tmp"), get(board_tmp))
        .route(&l("shots/save"), post(shots_save))
        .route(&l("joinsound/remove"), post(joinsound_remove))
        .route(&l("stickers"), get(stickers_list).post(stickers_add))
        .route(&l("stickers/remove"), post(stickers_remove))
        .route(&l("stickers/keep"), post(stickers_keep))
        .route(&l("sticker"), get(sticker_get))
        .route(&l("update/state"), get(update_state))
        .route(&l("update/offer"), post(update_offer))
        .route(&l("update/accept"), post(update_accept))
        .route(&l("update/decline"), post(update_decline))
        .route(&l("assistant/info"), get(assistant_info))
        .route(&l("assistant/wait"), get(assistant_wait))
        .route(&l("assistant/reply"), post(assistant_reply))
        .route(&l("assistant/presence"), post(assistant_presence))
        .route(&l("assistant/chat"), get(assistant_chat))
        .route(&l("assistant/send"), post(assistant_send))
        .route(&l("assistant/read"), post(assistant_read))
        .route(&l("ui/settings"), get(ui_settings_get).post(ui_settings_set))
        .route(&l("api"), get(api_index))
}

async fn page(State(node): St, app: Option<axum::Extension<FromApp>>) -> Response {
    let index = UI_FILES.iter().find(|(n, _)| *n == "index.html").map(|(_, b)| String::from_utf8_lossy(b).into_owned()).unwrap_or_else(|| NO_UI.to_string());
    // в окне приложения ключ не нужен (запросы идут внутри программы) — и в страницу его не кладём
    let token = if app.is_some() { "" } else { node.local_token.as_str() };
    let html = index.replace("%%TOKEN%%", token).replace("%%NODE%%", &html_escape(&node.node_name()));
    let mut r = Response::new(Body::from(html));
    let h = r.headers_mut();
    h.insert("Content-Type", HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert("Cache-Control", HeaderValue::from_static("no-store"));
    h.insert("X-Frame-Options", HeaderValue::from_static("DENY"));
    h.insert("Content-Security-Policy", HeaderValue::from_static(PANEL_CSP));
    r
}

/// Политика панели: только свои скрипты; ipc.localhost — связь окна приложения с программой; worker и wasm — просмотр PDF.
pub const PANEL_CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob: mediastream:; connect-src 'self' ipc: http://ipc.localhost ws://127.0.0.1:*; worker-src 'self' blob:; font-src 'self' data:; frame-src 'self' http://127.0.0.1:* http://localhost:*; frame-ancestors 'none'";

/// Файлы сборки панели (/app/…): имена с хэшем — кэшировать можно навсегда.
async fn ui_asset(AxPath(path): AxPath<String>) -> Response {
    let key = format!("app/{}", path.trim_start_matches('/'));
    let Some((_, data)) = UI_FILES.iter().find(|(n, _)| *n == key) else { return StatusCode::NOT_FOUND.into_response() };
    let mime = match key.rsplit('.').next().unwrap_or("") {
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        "json" => "application/json",
        _ => "application/octet-stream",
    };
    let mut r = Response::new(Body::from(*data));
    r.headers_mut().insert("Content-Type", HeaderValue::from_static(mime));
    r.headers_mut().insert("Cache-Control", HeaderValue::from_static("public, max-age=31536000, immutable"));
    r
}

/// Настройки панели этого ПК (звук, вид, доска): bridge-data/ui.json. До 3.0 они жили в браузере.
async fn ui_settings_get(State(node): St) -> Response {
    let v: Value = std::fs::read_to_string(node.data_dir.join("ui.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    ok_json(&json!({ "settings": v }))
}

async fn ui_settings_set(State(node): St, Json(body): Json<Value>) -> Response {
    let s = body.get("settings").cloned().unwrap_or(Value::Null);
    if !s.is_object() {
        return err(400, "settings — объект");
    }
    let text = s.to_string();
    if text.len() > 256 * 1024 {
        return err(400, "настройки больше 256 КБ");
    }
    let tmp = node.data_dir.join("ui.json.tmp");
    if let Err(e) = std::fs::write(&tmp, &text).and_then(|_| std::fs::rename(&tmp, node.data_dir.join("ui.json"))) {
        return err(500, format!("не удалось сохранить настройки: {}", e));
    }
    ok_json(&json!({ "ok": true }))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Отдельное окно показа экрана (видео берёт у открывшей его панели).
async fn popout() -> Response {
    let mut r = Response::new(Body::from(include_str!("../assets/popout.html")));
    let h = r.headers_mut();
    h.insert("Content-Type", HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert("Cache-Control", HeaderValue::from_static("no-store"));
    h.insert("Content-Security-Policy", HeaderValue::from_static("default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; media-src 'self' blob: mediastream:; img-src 'self' data:"));
    r
}

async fn favicon() -> Response {
    let mut r = Response::new(Body::from(FAVICON));
    r.headers_mut().insert("Content-Type", HeaderValue::from_static("image/svg+xml"));
    r
}

// ───────────────────────── Обзор и статус ─────────────────────────

pub fn overview_value(node: &Arc<Node>) -> Value {
    let cfg = node.cfg.read().clone();
    let peers: Vec<Value> = cfg
        .peers
        .iter()
        .map(|p| {
            let ps = node.peer_state(&p.name);
            let issues: Vec<String> = node
                .shares()
                .iter()
                .filter(|rt| cfg.share(&rt.id).map(|s| cfg.share_has_peer(s, &p.name)).unwrap_or(false))
                .filter_map(|rt| rt.reports.lock().get(&p.name).filter(|r| !r.starts_with("ок")).map(|r| format!("{}: {}", rt.id, r)))
                .collect();
            json!({
                "name": p.name, "title": p.title, "url": p.url, "ips": p.ips, "enabled": p.enabled,
                "online": ps.online, "lastSeen": ps.last_seen.map(|t| time::format(&t)), "latency": ps.latency_ms, "filesIn": ps.files_in,
                "error": ps.error, "hint": ps.hint, "warning": ps.warning, "announcedName": ps.announced_name,
                "pending": p.pending, "invite": p.pending && p.url.is_empty(), "keyId": p.key_id(), "version": ps.version, "legacy": ps.legacy,
                "remote": ps.remote, "lastInbound": ps.last_inbound.map(|t| time::format(&t)), "inboundError": ps.inbound_error,
                "status": ps.status,
                "shares": cfg.shares.iter().filter(|s| cfg.share_has_peer(s, &p.name)).map(|s| s.id.clone()).collect::<Vec<_>>(),
                "groups": cfg.groups.iter().filter(|(_, g)| g.members.iter().any(|m| eq_ic(m, &p.name))).map(|(k, _)| k.clone()).collect::<Vec<_>>(),
                "issues": issues, "agent": ps.agent, "runners": ps.runners, "assistant": ps.assistant,
            })
        })
        .collect();
    let shares: Vec<Value> = node
        .shares()
        .iter()
        .map(|rt| {
            let sc = rt.config(node).unwrap_or_default();
            let (records, open, rejected) = {
                let rb = rt.records.lock();
                let all = rb.all();
                let open = all.iter().filter(|s| matches!(s.status(), Some("new" | "accepted" | "in_progress" | "review" | "pending" | "proposed"))).count();
                (all.len(), open, rb.store.rejected().len())
            };
            let (files, bytes) = {
                let f = rt.files.lock();
                (f.manifest().files.len(), f.manifest().files.iter().map(|x| x.size).sum::<i64>())
            };
            let sch = rt.schemas.lock();
            json!({
                "id": rt.id, "title": sc.title, "folder": rt.root, "folderCfg": sc.folder, "peers": sc.peers, "accept": sc.accept, "recordsAccept": sc.records.accept,
                "lastRound": rt.last_round.lock().map(|t| time::format(&t)),
                "files": files, "bytes": bytes, "records": records, "open": open, "inbox": inbox_count(node, rt),
                "reports": rt.reports.lock().clone(), "conflicts": rt.conflicts.lock().clone(), "rejected": rejected,
                "schemaErrors": sch.errors, "schemas": sch.set.ids(),
            })
        })
        .collect();
    let day_ago = time::now() - chrono::Duration::hours(24);
    let strangers: Vec<Value> = node
        .strangers
        .lock()
        .values()
        .filter(|s| s.time > day_ago)
        .filter(|s| {
            let k = if s.kid.is_empty() { cfg.peer(&s.name) } else { cfg.peer_by_key(&s.kid) };
            !k.map(|k| k.enabled && k.allows_ip(s.ip.parse().ok())).unwrap_or(false)
        })
        .map(|s| serde_json::to_value(s).unwrap_or_default())
        .collect();
    json!({
        "node": node.status(),
        "urls": *node.urls.read(),
        "dashboard": node.dashboard_url(),
        "configFile": node.config_file,
        "baseDir": node.base_dir,
        "localIps": link::local_ips(),
        "listen": cfg.listen,
        "announce": node.announce_urls(),
        "peers": peers,
        "groups": cfg.groups,
        "shares": shares,
        "waiting": node.runner.waiting_list(),
        "strangers": strangers,
        "transfers": node.transfers.list(),
        "runners": cfg.runners.iter().map(|(k, r)| (k.clone(), json!({ "enabled": r.enabled, "title": r.title, "allowFrom": r.allow_from, "confirm": r.confirm, "params": r.params }))).collect::<serde_json::Map<_, _>>(),
        "mailbox": node.mailbox.status(node),
        "media": { "invites": node.media.pending_invites() },
    })
}

pub fn inbox_count(node: &Node, rt: &ShareRt) -> usize {
    let me = node.node_name();
    let rb = rt.records.lock();
    rb.all()
        .iter()
        .filter(|s| {
            let (creator, assignee) = bridge_core::reducer::roles(s, &me);
            let st = s.status().unwrap_or("");
            (s.type_ == record_types::TASK && assignee && matches!(st, "new" | "accepted" | "in_progress"))
                || (s.type_ == record_types::TASK && creator && st == "review")
                || (s.type_ == record_types::DECISION && !creator && st == "proposed")
                || (s.type_ == record_types::COMMAND && node.runner.is_waiting(&rt.id, &s.id))
                || !s.conflicts.is_empty()
        })
        .count()
}

pub fn attention_count(node: &Node) -> usize {
    node.shares().iter().map(|rt| inbox_count(node, rt)).sum()
}

async fn overview(State(node): St) -> Response {
    ok_json(&overview_value(&node))
}

pub fn local_status(node: &Arc<Node>) -> Value {
    let peers: Vec<_> = node.peers.lock().values().cloned().collect();
    json!({
        "node": node.status(),
        "urls": *node.urls.read(),
        "peers": peers,
        "shares": node.shares().iter().map(|rt| {
            let sc = rt.config(node).unwrap_or_default();
            let rb = rt.records.lock();
            json!({
                "id": rt.id, "folder": rt.root, "peers": sc.peers, "lastRound": rt.last_round.lock().map(|t| time::format(&t)),
                "files": rt.files.lock().manifest().files.len(), "records": rb.count(), "vector": vector_text(&rb.store.vector()),
                "reports": rt.reports.lock().clone(), "conflicts": rt.conflicts.lock().clone(), "rejected": rb.store.rejected().len(),
                "schemaErrors": rt.schemas.lock().errors.clone(),
            })
        }).collect::<Vec<_>>(),
        "waiting": node.runner.waiting_list(),
        "mailbox": node.mailbox.status(node),
    })
}

async fn status(State(node): St) -> Response {
    ok_json(&local_status(&node))
}

async fn peers_state(State(node): St) -> Response {
    let peers: Vec<_> = node.peers.lock().values().cloned().collect();
    ok_json(&peers)
}

async fn secret() -> Response {
    ok_json(&json!({ "secret": NodeConfig::new_secret() }))
}

async fn log(State(node): St, Query(m): Q) -> Response {
    let lines: usize = m.get("lines").and_then(|x| x.parse().ok()).unwrap_or(300).clamp(10, 2000);
    let text = std::fs::read_to_string(node.data_dir.join("log.txt")).unwrap_or_default();
    let all: Vec<&str> = text.lines().map(|l| l.trim_start_matches('\u{feff}')).filter(|l| !l.is_empty()).collect();
    let from = all.len().saturating_sub(lines);
    ok_json(&json!({ "lines": all[from..] }))
}

/// Поток событий для ассистентов и панели: сосед на связи / пропал, пришёл файл, запись, звонок…
async fn events_stream(State(node): St) -> Response {
    let rx = node.events_tx.subscribe();
    // пометки на показе — только для трея (их много и они мгновенные)
    let stream = tokio_stream_from(rx).filter(|v| futures_util::future::ready(v["kind"] != "overlay")).map(|v| Ok::<_, std::convert::Infallible>(Event::default().event("bridge").data(v.to_string())));
    Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(20))).into_response()
}

fn tokio_stream_from<T: Clone + Send + 'static>(mut rx: tokio::sync::broadcast::Receiver<T>) -> impl futures_util::Stream<Item = T> {
    async_stream(move |tx| async move {
        loop {
            match rx.recv().await {
                Ok(v) => {
                    if tx.send(v).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    })
}

/// Простой генератор потока через канал.
fn async_stream<T, F, Fut>(f: F) -> impl futures_util::Stream<Item = T>
where
    T: Send + 'static,
    F: FnOnce(tokio::sync::mpsc::Sender<T>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel::<T>(64);
    tokio::spawn(f(tx));
    futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) })
}

async fn sync(State(node): St) -> Response {
    node.folders.invalidate_own();
    for rt in node.shares() {
        rt.files_dirty.store(true, Ordering::SeqCst);
    }
    node.sync_now().await;
    ok_json(&local_status(&node))
}

// ───────────────────────── Соседи ─────────────────────────

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PeerUpsert {
    pub name: String,
    pub title: Option<String>,
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub extra_ips: Option<Vec<String>>,
    pub secret: Option<String>,
    pub enabled: Option<bool>,
    pub shares: Option<Vec<String>>,
    pub groups: Option<Vec<String>>,
}

fn apply_upsert(cfg: &mut NodeConfig, name: &str, r: &PeerUpsert) {
    if cfg.peer(name).is_none() {
        cfg.peers.push(PeerConfig { name: name.into(), ..Default::default() });
    }
    let p = cfg.peer_mut(name).unwrap();
    if let Some(t) = &r.title {
        p.title = t.trim().into();
    }
    if let Some(e) = r.enabled {
        p.enabled = e;
    }
    if let Some(s) = r.secret.as_ref().filter(|s| !s.trim().is_empty()) {
        p.secret = s.trim().into();
    }
    if let Some(ip) = r.ip.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        let port = r.port.unwrap_or_else(|| config::parse_url(&p.url).map(|(_, port)| port).unwrap_or(5390));
        p.url = format!("http://{}:{}", ip, port);
        let mut ips = vec![ip.clone()];
        for x in r.extra_ips.clone().unwrap_or_else(|| p.ips.clone()) {
            let x = x.trim().to_string();
            if !x.is_empty() && !ips.contains(&x) {
                ips.push(x);
            }
        }
        p.ips = ips;
    } else if let Some(extra) = &r.extra_ips {
        if let Some(first) = p.ips.first().cloned() {
            let mut ips = vec![first.clone()];
            for x in extra.iter().map(|x| x.trim().to_string()) {
                if !x.is_empty() && x != first && !ips.contains(&x) {
                    ips.push(x);
                }
            }
            p.ips = ips;
        }
    }
    let pname = p.name.clone();
    if let Some(want) = &r.shares {
        for s in cfg.shares.iter_mut() {
            let w = want.iter().any(|x| eq_ic(x, &s.id));
            let has = s.peers.iter().any(|x| eq_ic(x, &pname));
            if w && !has {
                s.peers.push(pname.clone());
            }
            if !w && has {
                s.peers.retain(|x| !eq_ic(x, &pname));
            }
        }
    }
    if let Some(want) = &r.groups {
        for (g, gr) in cfg.groups.iter_mut() {
            let w = want.iter().any(|x| eq_ic(x, g));
            let has = gr.members.iter().any(|x| eq_ic(x, &pname));
            if w && !has {
                gr.members.push(pname.clone());
            }
            if !w && has {
                gr.members.retain(|x| !eq_ic(x, &pname));
            }
        }
    }
}

pub fn upsert_peer(node: &Arc<Node>, r: PeerUpsert) -> Result<Option<String>, String> {
    let name = r.name.trim().to_string();
    if !is_name(&name) {
        return Err("имя узла соседа — латиница, цифры, «-», «_»".into());
    }
    if let Some(ip) = r.ip.as_ref().filter(|s| !s.trim().is_empty()) {
        if ip.trim().parse::<std::net::IpAddr>().is_err() {
            return Err("адрес — IPv4, например 192.168.1.172".into());
        }
    }
    let mut r = r;
    let mut generated = None;
    let created;
    {
        let _g = node.config_lock.lock();
        let mut cfg = node.cfg.write();
        created = cfg.peer(&name).is_none();
        if created && r.secret.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
            let s = NodeConfig::new_secret();
            r.secret = Some(s.clone());
            generated = Some(s);
        }
        if created && r.ip.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
            return Err("у нового соседа нужен адрес".into());
        }
        // Сначала — на копии: списки меняются целиком, цикл обмена не ломается
        let mut clone = cfg.clone();
        apply_upsert(&mut clone, &name, &r);
        let errs = clone.validate();
        if !errs.is_empty() {
            return Err(errs.join("; "));
        }
        *cfg = clone;
        node.peers.lock().entry(name.clone()).or_insert_with(|| crate::node::PeerState { name: name.clone(), ..Default::default() });
    }
    node.mailbox.prepare(node);
    for rt in node.shares() {
        rt.wake.store(true, Ordering::SeqCst);
    }
    node.folders.wake.store(true, Ordering::SeqCst);
    node.wake.notify_one();
    let e = node.save_config();
    node.info(&format!("Панель: сосед «{}» {}{}", name, if created { "добавлен" } else { "изменён" }, e.as_ref().map(|e| format!(" (не сохранено: {})", e)).unwrap_or_default()));
    match e {
        None => Ok(generated),
        Some(e) => Err(format!("изменено, но не сохранено в файл: {}", e)),
    }
}

async fn peer_upsert(State(node): St, body: axum::body::Bytes) -> Response {
    let r: PeerUpsert = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return err(400, format!("не JSON: {}", e)),
    };
    match upsert_peer(&node, r) {
        Ok(secret) => ok_json(&json!({ "ok": true, "secret": secret, "saved": node.config_file.is_some() })),
        Err(e) => err(422, e),
    }
}

/// Удалить соседа: из списка, пространств, правил приёма, групп, доступа к папкам, исполнителей и игнора.
pub fn remove_peer(node: &Arc<Node>, name: &str) -> Result<(), String> {
    let real;
    {
        let _g = node.config_lock.lock();
        let mut cfg = node.cfg.write();
        let Some(p) = cfg.peer(name).cloned() else { return Err("нет такого соседа".into()) };
        real = p.name.clone();
        let eqp = |x: &String| eq_ic(x, &real);
        cfg.peers.retain(|x| !eq_ic(&x.name, &real));
        for s in cfg.shares.iter_mut() {
            s.peers.retain(|x| !eqp(x));
            s.members.retain(|x| !eqp(x));
            s.accept = s
                .accept
                .iter()
                .cloned()
                .filter_map(|mut a| {
                    let had = a.from.iter().any(eqp);
                    a.from.retain(|x| !eqp(x));
                    (!had || !a.from.is_empty()).then_some(a)
                })
                .collect();
        }
        for f in cfg.folders.iter_mut() {
            f.access.retain(|k, _| !eqp(k));
        }
        for g in cfg.groups.values_mut() {
            g.members.retain(|x| !eqp(x));
        }
        for c in cfg.channels.values_mut() {
            c.members.retain(|x| !eqp(x));
        }
        cfg.mailbox.accept_from.retain(|x| !eqp(x));
        for r in cfg.runners.values_mut() {
            r.allow_from.retain(|x| !eqp(x));
        }
        let prefix = format!("{}/", real.to_lowercase());
        cfg.mounts.ignore.retain(|k, _| !k.to_lowercase().starts_with(&prefix));
        node.peers.lock().remove(&real);
        node.forget_client(&real);
    }
    node.folders.forget_peer(node, &real);
    let e = node.save_config();
    node.info(&format!("Панель/API: сосед «{}» удалён{}. Его файлы и история записей остались.", real, e.as_ref().map(|e| format!(" (не сохранено: {})", e)).unwrap_or_default()));
    match e {
        None => Ok(()),
        Some(e) => Err(format!("удалён, но не сохранено в файл: {}", e)),
    }
}

async fn peer_remove(State(node): St, Query(m): Q) -> Response {
    match remove_peer(&node, q(&m, "name")) {
        Ok(()) => ok_json(&json!({ "ok": true })),
        Err(e) => err(if e == "нет такого соседа" { 404 } else { 500 }, e),
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ConnectRequest {
    address: String,
    secret: Option<String>,
    title: Option<String>,
}

async fn peer_connect(State(node): St, body: axum::body::Bytes) -> Response {
    let r: ConnectRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return err(400, format!("не JSON: {}", e)),
    };
    let res = link::connect_peer(&node, &r.address, r.secret.as_deref().unwrap_or(""), r.title.as_deref().unwrap_or("")).await;
    json_code(if res.ok { 200 } else { 422 }, &res)
}

async fn peer_invite(State(node): St, Query(m): Q) -> Response {
    let (r, warning) = link::invite(&node, q(&m, "title"));
    ok_json(&json!({ "ok": true, "name": r.name, "secret": r.secret, "addresses": r.addresses, "line": r.line, "warning": warning }))
}

async fn peer_check(State(node): St, Query(m): Q) -> Response {
    let name = q(&m, "name");
    if name.is_empty() {
        return ok_json(&link::check_all(&node).await);
    }
    match link::check_peer(&node, name).await {
        Some(c) => ok_json(&c),
        None => err(404, "нет такого соседа"),
    }
}

async fn peer_rename(State(node): St, Query(m): Q) -> Response {
    match link::rename_peer(&node, q(&m, "name"), q(&m, "to")) {
        Ok(()) => ok_json(&json!({ "ok": true })),
        Err(e) => err(if e == "нет такого соседа" { 404 } else { 422 }, e),
    }
}

// ───────────────────────── Группы и права (2.0) ─────────────────────────

async fn groups_get(State(node): St) -> Response {
    ok_json(&node.cfg.read().groups)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct GroupSet {
    id: String,
    title: String,
    members: Vec<String>,
}

async fn groups_set(State(node): St, body: axum::body::Bytes) -> Response {
    let r: GroupSet = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return err(400, format!("не JSON: {}", e)),
    };
    let id = r.id.trim().to_string();
    {
        let _g = node.config_lock.lock();
        let mut cfg = node.cfg.write();
        let mut clone = cfg.clone();
        let key = clone.groups.keys().find(|k| eq_ic(k, &id)).cloned().unwrap_or(id.clone());
        clone.groups.insert(key, GroupConfig { title: r.title.trim().into(), members: r.members.clone() });
        let errs = clone.validate();
        if !errs.is_empty() {
            return err(422, errs.join("; "));
        }
        *cfg = clone;
    }
    let e = node.save_config();
    node.notify_all("folders");
    node.folders.wake.store(true, Ordering::SeqCst);
    node.info(&format!("Панель: группа «{}» — {}", id, if r.members.is_empty() { "пусто".into() } else { r.members.join(", ") }));
    ok_json(&json!({ "ok": true, "saved": e.is_none(), "error": e }))
}

async fn groups_remove(State(node): St, Query(m): Q) -> Response {
    let id = q(&m, "id").to_string();
    let tag = format!("@{}", id);
    {
        let _g = node.config_lock.lock();
        let mut cfg = node.cfg.write();
        cfg.groups.retain(|k, _| !eq_ic(k, &id));
        for f in cfg.folders.iter_mut() {
            f.access.retain(|k, _| !eq_ic(k, &tag));
        }
        for s in cfg.shares.iter_mut() {
            s.peers.retain(|p| !eq_ic(p, &tag));
        }
        for c in cfg.channels.values_mut() {
            c.members.retain(|p| !eq_ic(p, &tag));
        }
    }
    let e = node.save_config();
    node.notify_all("folders");
    ok_json(&json!({ "ok": true, "saved": e.is_none() }))
}

/// Кто что видит: соседи × (папки, пространства) — права с учётом групп.
async fn access_matrix(State(node): St) -> Response {
    let cfg = node.cfg.read().clone();
    let rows: Vec<Value> = cfg
        .peers
        .iter()
        .map(|p| {
            let folders: serde_json::Map<String, Value> = cfg
                .folders
                .iter()
                .map(|f| {
                    let direct = f.access.iter().find(|(k, _)| eq_ic(k, &p.name)).map(|(_, v)| v.clone());
                    let via: Vec<String> = f
                        .access
                        .iter()
                        .filter(|(k, _)| k.strip_prefix('@').map(|g| cfg.in_group(g, &p.name)).unwrap_or(false))
                        .map(|(k, v)| format!("{}:{}", k, v))
                        .collect();
                    (f.id.clone(), json!({ "access": cfg.access_of(f, &p.name), "direct": direct, "via": via, "all": f.access.get("*") }))
                })
                .collect();
            let shares: serde_json::Map<String, Value> = cfg.shares.iter().map(|s| (s.id.clone(), json!(cfg.share_has_peer(s, &p.name)))).collect();
            json!({ "peer": p.name, "title": p.title, "groups": cfg.groups.iter().filter(|(_, g)| g.members.iter().any(|m| eq_ic(m, &p.name))).map(|(k, _)| k.clone()).collect::<Vec<_>>(), "folders": folders, "shares": shares })
        })
        .collect();
    ok_json(&json!({ "rows": rows, "folders": cfg.folders.iter().map(|f| json!({"id": f.id, "title": f.title})).collect::<Vec<_>>(), "shares": cfg.shares.iter().map(|s| json!({"id": s.id, "title": s.title})).collect::<Vec<_>>(), "groups": cfg.groups }))
}

// ───────────────────────── Записи ─────────────────────────

fn filter_records<'a>(all: Vec<&'a RecordState>, m: &HashMap<String, String>) -> Vec<&'a RecordState> {
    all.into_iter()
        .filter(|s| {
            let t = q(m, "type");
            let st = q(m, "status");
            let a = q(m, "assignee");
            let text = q(m, "q");
            (t.is_empty() || eq_ic(&s.type_, t))
                && (st.is_empty() || eq_ic(s.status().unwrap_or(""), st))
                && (a.is_empty() || eq_ic(s.assignee().unwrap_or(""), a))
                && (text.is_empty() || format!("{} {}", s.id, serde_json::to_string(&s.fields).unwrap_or_default()).to_lowercase().contains(&text.to_lowercase()))
        })
        .collect()
}

async fn records(State(node): St, Query(m): Q) -> Response {
    let mut list = Vec::new();
    for rt in node.shares() {
        if !q(&m, "share").is_empty() && !eq_ic(q(&m, "share"), &rt.id) {
            continue;
        }
        let rb = rt.records.lock();
        let base = if q(&m, "deleted") == "1" { rb.deleted() } else { rb.all() };
        for s in filter_records(base, &m) {
            list.push(shares::summary(&rt.id, s));
        }
    }
    ok_json(&list)
}

async fn record(State(node): St, AxPath(id): AxPath<String>, Query(m): Q) -> Response {
    let rt = match shares::resolve_share(&node, Some(q(&m, "share")).filter(|s| !s.is_empty()), Some(&id)) {
        Ok(rt) => rt,
        Err(e) => return err(404, e),
    };
    let peers = rt.peers(&node);
    let schemas = rt.schemas();
    let at = m.get("at").and_then(|v| v.parse().ok());
    let view = rt.records.lock().view(&id, at, &peers, &schemas);
    match view {
        Some(s) => ok_json(&s),
        None => err(404, format!("нет записи {}", id)),
    }
}

async fn record_history(State(node): St, AxPath(id): AxPath<String>, Query(m): Q) -> Response {
    let rt = match shares::resolve_share(&node, Some(q(&m, "share")).filter(|s| !s.is_empty()), Some(&id)) {
        Ok(rt) => rt,
        Err(e) => return err(404, e),
    };
    let rb = rt.records.lock();
    if rb.get(&id).is_none() {
        return err(404, format!("нет записи {}", id));
    }
    ok_json(&shares::history_view(&rb, &id))
}

async fn actions(State(node): St, body: axum::body::Bytes) -> Response {
    let a: BridgeAction = match serde_json::from_slice(&body) {
        Ok(a) => a,
        Err(e) => return err(400, format!("не JSON действия: {}", e)),
    };
    let res = shares::act(&node, a);
    json_code(if res.ok { 200 } else { 422 }, &res)
}

async fn rejected(State(node): St, Query(m): Q) -> Response {
    match shares::resolve_share(&node, Some(q(&m, "share")).filter(|s| !s.is_empty()), None) {
        Ok(rt) => ok_json(&rt.records.lock().store.rejected().to_vec()),
        Err(e) => err(400, e),
    }
}

/// Долгий опрос: ответ, как только в пространстве появятся события после курсора (или по таймауту).
async fn wait(State(node): St, Query(m): Q) -> Response {
    let rt = match shares::resolve_share(&node, Some(q(&m, "share")).filter(|s| !s.is_empty()), None) {
        Ok(rt) => rt,
        Err(e) => return err(400, e),
    };
    let after = parse_vector(q(&m, "after"));
    let timeout: u64 = m.get("timeout").and_then(|v| v.parse().ok()).unwrap_or(30).clamp(1, 300);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(timeout);
    while std::time::Instant::now() < until {
        if rt.records.lock().store.has_since(&after) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let rb = rt.records.lock();
    let page = rb.store.since(&after, 500);
    ok_json(&json!({ "vector": vector_text(&rb.store.vector()), "events": page.events, "more": page.more }))
}

async fn files(State(node): St, Query(m): Q) -> Response {
    match shares::resolve_share(&node, Some(q(&m, "share")).filter(|s| !s.is_empty()), None) {
        Ok(rt) => {
            let mf = rt.files.lock().manifest().clone();
            ok_json(&mf)
        }
        Err(e) => err(400, e),
    }
}

// ───────────────────────── Ручная отправка ─────────────────────────

async fn mailbox_status(State(node): St) -> Response {
    ok_json(&node.mailbox.status(&node))
}

async fn mailbox_outgoing(State(node): St) -> Response {
    let cfg = node.cfg.read().clone();
    let map: serde_json::Map<String, Value> =
        cfg.peers.iter().filter(|p| p.enabled).map(|p| (p.name.clone(), json!(node.mailbox.pending(&node, p).into_iter().map(|(e, _)| e).collect::<Vec<_>>()))).collect();
    ok_json(&map)
}

/// Тело запроса — во временный файл (с пределом размера).
/// (3.3.1) Большие файлы панель шлёт частями (`POST upload/part?id&offset`), а сам вызов — с `?upload=<id>&size=N`
/// и пустым телом: тогда берём собранный файл. Иначе — тело запроса целиком.
fn part_path(node: &Node, id: &str) -> Option<PathBuf> {
    if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    Some(node.data_dir.join("tmp").join("uploads").join(format!("{}.part", id)))
}

async fn upload_part(State(node): St, Query(m): Q, req: Request) -> Response {
    let Some(p) = part_path(&node, q(&m, "id")) else { return err(400, "id части") };
    let off: u64 = q(&m, "offset").parse().unwrap_or(u64::MAX);
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let have = std::fs::metadata(&p).map(|x| x.len()).unwrap_or(0);
    if off != have {
        return err(409, format!("часть не по порядку: есть {} байт, пришла с {}", have, off));
    }
    let max = node.max_file_bytes();
    let mut f = match tokio::fs::OpenOptions::new().create(true).append(true).open(&p).await {
        Ok(f) => f,
        Err(e) => return err(500, e.to_string()),
    };
    let mut total = have as i64;
    let mut st = req.into_body().into_data_stream();
    while let Some(c) = st.next().await {
        let Ok(c) = c else { return err(400, "передача оборвалась") };
        total += c.len() as i64;
        if total > max {
            drop(f);
            let _ = tokio::fs::remove_file(&p).await;
            return err(413, "файл больше предела (maxFileMb)");
        }
        if let Err(e) = f.write_all(&c).await {
            return err(500, e.to_string());
        }
    }
    let _ = f.flush().await;
    ok_json(&json!({ "ok": true, "size": total }))
}

pub(crate) async fn body_to_tmp(node: &Node, req: Request) -> Result<PathBuf, Response> {
    let qs: HashMap<String, String> = req.uri().query().unwrap_or("").split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect(); // upload и size — без %-кодов
    if let Some(id) = qs.get("upload").filter(|x| !x.is_empty()) {
        let p = part_path(node, id).ok_or_else(|| err(400, "upload"))?;
        let have = std::fs::metadata(&p).map(|x| x.len()).map_err(|_| err(404, "частей файла нет — отправьте заново"))?;
        let want: u64 = qs.get("size").and_then(|x| x.parse().ok()).unwrap_or(have);
        if have != want {
            let _ = std::fs::remove_file(&p);
            return Err(err(400, format!("файл пришёл не целиком: {} из {} байт", have, want)));
        }
        let tmp = node.tmp_file();
        bridge_core::files::move_replace(&p, &tmp).map_err(|e| err(500, e.to_string()))?;
        return Ok(tmp);
    }
    let tmp = node.tmp_file();
    let max = node.max_file_bytes();
    let mut f = tokio::fs::File::create(&tmp).await.map_err(|e| err(500, e.to_string()))?;
    let mut total: i64 = 0;
    let mut s = req.into_body().into_data_stream();
    while let Some(c) = s.next().await {
        let c = c.map_err(|_| err(400, "передача оборвалась"))?;
        total += c.len() as i64;
        if total > max {
            drop(f);
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(err(413, "файл больше предела (maxFileMb)"));
        }
        f.write_all(&c).await.map_err(|e| err(500, e.to_string()))?;
    }
    f.flush().await.map_err(|e| err(500, e.to_string()))?;
    Ok(tmp)
}

/// POST builds/presets {list} — заготовки сборок целиком.
async fn builds_presets(State(node): St, Json(v): Json<Value>) -> Response {
    match crate::builds::set_presets(&node, &v["list"]) {
        Ok(l) => ok_json(&json!({ "ok": true, "list": l })),
        Err(e) => err(400, e),
    }
}

/// POST builds/preview {path, ignore} или {preset} — что войдёт в архив и что отсеет игнор.
async fn builds_preview(State(node): St, Json(v): Json<Value>) -> Response {
    let (path, ignore) = match v["preset"].as_str().filter(|x| !x.is_empty()) {
        Some(id) => match crate::builds::presets(&node).into_iter().find(|p| p["id"] == id) {
            Some(p) => (p["path"].as_str().unwrap_or("").to_string(), p["ignore"].clone()),
            None => return err(404, "нет такой заготовки"),
        },
        None => (v["path"].as_str().unwrap_or("").to_string(), v["ignore"].clone()),
    };
    let ignore: Vec<String> = match &ignore {
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
        Value::String(t) => t.lines().map(String::from).collect(),
        _ => vec![],
    };
    let n2 = node.clone();
    match tokio::task::spawn_blocking(move || crate::builds::preview(&n2, &path, &ignore)).await {
        Ok(Ok(r)) => ok_json(&r),
        Ok(Err(e)) => err(400, e),
        Err(e) => err(500, e.to_string()),
    }
}

/// POST builds/send {preset, mode?} — по заготовке всем её получателям; или {peer, path, name, mode, ignore?} — разово.
async fn builds_send(State(node): St, Json(v): Json<Value>) -> Response {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    if !s("preset").is_empty() {
        return match crate::builds::send_preset(&node, &s("preset"), Some(&s("mode")), &s("note")) {
            Ok(r) => ok_json(&r),
            Err((c, e)) => err(c, e),
        };
    }
    let ignore: Vec<String> = v["ignore"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    match crate::builds::send_ex(&node, &s("peer"), &s("path"), &s("name"), &s("mode"), &ignore, "") {
        Ok(r) => ok_json(&r),
        Err((c, e)) => err(c, e),
    }
}

/// POST builds/target {peer, name, path} — куда распаковывать сборку от соседа (пусто — по умолчанию).
async fn builds_target(State(node): St, Json(v): Json<Value>) -> Response {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    match crate::builds::set_target(&node, &s("peer"), &s("name"), &s("path")) {
        Ok(()) => ok_json(&json!({ "ok": true, "target": crate::builds::target_dir(&node, &s("peer"), &s("name")) })),
        Err(e) => err(400, e),
    }
}

async fn mailbox_upload(State(node): St, Query(m): Q, req: Request) -> Response {
    let Some(peer) = node.cfg.read().peer(q(&m, "peer")).cloned() else { return err(404, "нет такого соседа") };
    let p = match path::normalize(q(&m, "path")) {
        Ok(p) => p,
        Err(w) => return err(400, format!("путь: {}", w)),
    };
    if let Some(rule) = node.cfg.read().global_ignore().which(&p).map(String::from) {
        return err(403, format!("в игноре («{}») — не отправляется", rule));
    }
    let dir = node.mailbox.out_root(&node).join(&peer.name);
    let Ok(mut full) = path::to_local(&dir, &p) else { return err(400, "путь") };
    // сначала во временный файл — чтобы узел не отправил недописанное
    let tmp = match body_to_tmp(&node, req).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let Some(d) = full.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if full.exists() {
        let stem = full.file_stem().and_then(|s| s.to_str()).unwrap_or("файл").to_string();
        let ext = full.extension().and_then(|s| s.to_str()).map(|e| format!(".{}", e)).unwrap_or_default();
        full = full.with_file_name(format!("{} ({}){}", stem, chrono::Local::now().format("%H%M%S"), ext));
    }
    if let Err(e) = bridge_core::files::move_replace(&tmp, &full) {
        return err(500, e.to_string());
    }
    crate::activity::record(&node, "files.sent", json!({ "bytes": std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0) }));
    // готов к отправке сразу
    bridge_core::files::set_mtime(&full, &(time::now() - chrono::Duration::seconds(5)));
    node.mailbox.dirty.store(true, Ordering::SeqCst);
    node.notify_peers(vec![peer.name.clone()], "", "mailbox");
    let rel = path::relative(&dir, &full).unwrap_or(p);
    node.info(&format!("[отправка] из панели в очередь для {}: {}", peer.name, rel));
    ok_json(&json!({ "ok": true, "path": rel }))
}

async fn mailbox_cancel(State(node): St, Query(m): Q) -> Response {
    let Some(peer) = node.cfg.read().peer(q(&m, "peer")).cloned() else { return err(400, "нужны peer и path") };
    let Some(p) = path::norm(q(&m, "path")) else { return err(400, "нужны peer и path") };
    let Some((_, full)) = node.mailbox.pending(&node, &peer).into_iter().find(|(e, _)| eq_ic(&e.path, &p)) else { return err(404, "уже отправлен или нет такого") };
    // не удаляем — убираем в «_отменено», чтобы ничего не потерять
    let Ok(dest) = path::to_local(&node.mailbox.out_root(&node).join("_отменено").join(&peer.name), &p) else { return err(400, "путь") };
    if let Some(d) = dest.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = bridge_core::files::move_replace(&full, &dest);
    ok_json(&json!({ "ok": true }))
}

async fn mailbox_incoming(State(node): St) -> Response {
    let root = node.mailbox.in_root(&node);
    let mut list: Vec<(i64, Value)> = walk(&root, &[])
        .into_iter()
        .map(|w| (w.mtime_ticks, json!({ "path": w.rel, "size": w.size, "time": time::format(&time::from_ticks(w.mtime_ticks)) })))
        .collect();
    list.sort_by(|a, b| b.0.cmp(&a.0));
    list.truncate(500);
    ok_json(&list.into_iter().map(|x| x.1).collect::<Vec<_>>())
}

pub(crate) async fn download_file(full: PathBuf) -> Response {
    let Ok(f) = tokio::fs::File::open(&full).await else { return err(404, "нет файла") };
    let name = full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut r = Response::new(Body::from_stream(tokio_util::io::ReaderStream::with_capacity(f, 256 * 1024)));
    let h = r.headers_mut();
    h.insert("Content-Type", HeaderValue::from_static("application/octet-stream"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename*=UTF-8''{}", crate::client::esc(&name))) {
        h.insert("Content-Disposition", v);
    }
    r
}

async fn mailbox_incoming_file(State(node): St, Query(m): Q) -> Response {
    let Some(p) = path::norm(q(&m, "path")) else { return err(400, "путь") };
    let Ok(full) = path::to_local(&node.mailbox.in_root(&node), &p) else { return err(400, "путь") };
    if !full.is_file() {
        return err(404, "нет файла");
    }
    download_file(full).await
}

async fn open_dir(State(node): St, Query(m): Q) -> Response {
    let cfg = node.cfg.read().clone();
    let peer_dir = |root: PathBuf| match cfg.peer(q(&m, "peer")) {
        Some(p) => root.join(&p.name),
        None => root,
    };
    let dir: Option<PathBuf> = match q(&m, "target") {
        "outgoing" => Some(peer_dir(node.mailbox.out_root(&node))),
        "incoming" => Some(peer_dir(node.mailbox.in_root(&node))),
        "share" => node.share(q(&m, "share")).map(|rt| rt.root.clone()),
        "data" => Some(node.data_dir.clone()),
        "trash" => Some(node.folders.trash_dir().to_path_buf()),
        "folder" => cfg.folder(q(&m, "id")).map(|f| node.folders.own_root(&node, f)),
        "mount" => node.folders.find_mount(q(&m, "peer"), q(&m, "id")).map(|m| PathBuf::from(m.root)),
        "mounts" => Some(node.folders.mount_root(&node)),
        "node" => Some(node.base_dir.clone()),
        "phone-outbox" => Some(node.resolve(&cfg.mobile.outbox)),
        "apps" => Some(crate::apps::folder(&node)),
        "build" => crate::builds::clean_name(q(&m, "name")).ok().map(|n| crate::builds::target_dir(&node, q(&m, "peer"), &n)),
        _ => None,
    };
    let Some(dir) = dir else { return err(400, "неизвестная папка") };
    let _ = std::fs::create_dir_all(&dir);
    match open_in_shell(&dir) {
        Ok(()) => ok_json(&json!({ "ok": true, "dir": dir })),
        Err(e) => err(500, format!("не удалось открыть: {}", e)),
    }
}

pub fn open_in_shell(target: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe").arg(target).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(target).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(target).spawn().map(|_| ())
    }
}

pub fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ())
    }
}

// ───────────────────────── Игнор ─────────────────────────

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct IgnoreRequest {
    scope: String,
    id: String,
    peer: String,
    patterns: Option<Vec<String>>,
    pattern: Option<String>,
}

fn ignore_list(cfg: &NodeConfig, node: &Node, r: &IgnoreRequest) -> Option<Vec<String>> {
    match r.scope.as_str() {
        "" | "node" => Some(cfg.ignore.clone()),
        "folder" => cfg.folder(&r.id).map(|f| f.ignore.clone()),
        "mount" => {
            let m = node.folders.find_mount(&r.peer, &r.id)?;
            Some(cfg.mounts.ignore.iter().find(|(k, _)| eq_ic(k, &m.key)).map(|(_, v)| v.clone()).unwrap_or_default())
        }
        _ => None,
    }
}

fn change_ignore(node: &Arc<Node>, r: &IgnoreRequest, mode: &str) -> Result<Option<String>, String> {
    {
        let _g = node.config_lock.lock();
        let mut cfg = node.cfg.write();
        let Some(list) = ignore_list(&cfg, node, r) else {
            return Err(format!("нет такой области: scope={} (node | folder + id | mount + peer + id)", r.scope));
        };
        let mut next = list.clone();
        let p = r.pattern.as_deref().and_then(ignore::clean);
        match mode {
            "set" => {
                next = Vec::new();
                for x in r.patterns.clone().unwrap_or_default().iter().filter_map(|x| ignore::clean(x)) {
                    if !next.iter().any(|n| eq_ic(n, &x)) {
                        next.push(x);
                    }
                }
            }
            "add" => {
                let p = p.ok_or("пустой шаблон")?;
                if !next.iter().any(|n| eq_ic(n, &p)) {
                    next.push(p);
                }
            }
            _ => {
                let p = p.ok_or("пустой шаблон")?;
                next.retain(|x| ignore::clean(x).map(|c| !eq_ic(&c, &p)).unwrap_or(true));
            }
        }
        let errs = ignore::validate(&next, "ignore");
        if !errs.is_empty() {
            return Err(errs.join("; "));
        }
        match r.scope.as_str() {
            "" | "node" => cfg.ignore = next,
            "folder" => {
                let id = r.id.clone();
                if let Some(f) = cfg.folders.iter_mut().find(|f| eq_ic(&f.id, &id)) {
                    f.ignore = next;
                }
            }
            _ => {
                let key = node.folders.find_mount(&r.peer, &r.id).map(|m| m.key).unwrap_or_default();
                cfg.mounts.ignore.retain(|k, _| !eq_ic(k, &key));
                if !next.is_empty() {
                    cfg.mounts.ignore.insert(key, next);
                }
            }
        }
    }
    let e = node.save_config();
    node.folders.invalidate_own();
    node.notify_all("folders");
    node.folders.wake.store(true, Ordering::SeqCst);
    node.mailbox.wake.store(true, Ordering::SeqCst);
    for rt in node.shares() {
        rt.files_dirty.store(true, Ordering::SeqCst);
    }
    node.wake.notify_one();
    node.info(&format!("Панель/API: игнор {} {} — {}{}", if r.scope.is_empty() { "node" } else { &r.scope }, r.id, mode, r.pattern.as_ref().map(|p| format!(" «{}»", p)).unwrap_or_default()));
    Ok(e.map(|e| format!("применено, но bridge.json не записан: {}", e)))
}

async fn ignore_get(State(node): St) -> Response {
    let cfg = node.cfg.read().clone();
    let mounts: serde_json::Map<String, Value> = node
        .folders
        .mounts()
        .iter()
        .map(|m| (m.key.clone(), json!({ "own": cfg.mounts.ignore.iter().find(|(k, _)| eq_ic(k, &m.key)).map(|(_, v)| v.clone()).unwrap_or_default(), "owner": m.owner_ignore })))
        .collect();
    ok_json(&json!({
        "node": cfg.ignore,
        "folders": cfg.folders.iter().map(|f| (f.id.clone(), json!(f.ignore))).collect::<serde_json::Map<_, _>>(),
        "mounts": mounts,
        "syntax": [
            "имя или *.bak — файл или папка с таким именем на любой глубине",
            "Папка/ — только папка с таким именем (и всё внутри)",
            "док/черновики или /корень.txt — путь от корня папки (и всё внутри)",
            "* — любые символы в имени, ** — любая глубина, ? — один символ, # — комментарий",
        ],
    }))
}

async fn ignore_mode(node: Arc<Node>, body: axum::body::Bytes, mode: &str) -> Response {
    let r: IgnoreRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return err(400, format!("не JSON: {}", e)),
    };
    match change_ignore(&node, &r, mode) {
        Ok(warning) => {
            let cfg = node.cfg.read().clone();
            ok_json(&json!({ "ok": true, "warning": warning, "ignore": ignore_list(&cfg, &node, &r) }))
        }
        Err(e) => err(422, e),
    }
}

async fn ignore_set(State(node): St, body: axum::body::Bytes) -> Response {
    ignore_mode(node, body, "set").await
}
async fn ignore_add(State(node): St, body: axum::body::Bytes) -> Response {
    ignore_mode(node, body, "add").await
}
async fn ignore_remove(State(node): St, body: axum::body::Bytes) -> Response {
    ignore_mode(node, body, "remove").await
}

fn fs_ignore(node: &Node, m: &HashMap<String, String>) -> Arc<IgnoreRules> {
    let cfg = node.cfg.read().clone();
    match (q(m, "kind"), q(m, "scope")) {
        ("own", _) | (_, "folder") => cfg.folder(q(m, "id")).map(|f| node.folders.own_ignore(node, f)).unwrap_or_default(),
        ("mount", _) | (_, "mount") => node.folders.find_mount(q(m, "peer"), q(m, "id")).map(|mm| node.folders.mount_ignore(node, &mm)).unwrap_or_default(),
        _ => cfg.global_ignore(),
    }
}

async fn ignore_test(State(node): St, Query(m): Q) -> Response {
    let Some(p) = path::norm(q(&m, "path")) else { return err(400, "путь") };
    let rules = fs_ignore(&node, &m);
    let rule = rules.which(&p).map(String::from);
    ok_json(&json!({ "path": p, "ignored": rule.is_some(), "rule": rule }))
}

// ───────────────────────── Общие папки ─────────────────────────

async fn folders_get(State(node): St) -> Response {
    let cfg = node.cfg.read().clone();
    let own: Vec<Value> = cfg
        .folders
        .iter()
        .map(|f| {
            let root = node.folders.own_root(&node, f);
            let files = node.folders.own_files(&node, f);
            json!({
                "id": f.id, "title": f.title, "path": f.path, "root": root, "access": f.access,
                "files": files.len(), "bytes": files.iter().map(|x| x.size).sum::<i64>(), "exists": root.is_dir(), "ignore": f.ignore,
            })
        })
        .collect();
    let mounts: Vec<Value> = node
        .folders
        .mounts()
        .iter()
        .map(|m| {
            let mut recent = m.recent.clone();
            recent.reverse();
            recent.truncate(10);
            let (transfers, failed) = node.folders.transfer_view(&m.key);
            json!({
                "transfers": transfers, "failed": failed,
                "peer": m.peer, "id": m.id, "title": m.title, "access": m.access, "root": m.root, "lastSync": m.last_sync.map(|t| time::format(&t)), "error": m.error,
                "ignore": cfg.mounts.ignore.iter().find(|(k, _)| eq_ic(k, &m.key)).map(|(_, v)| v.clone()).unwrap_or_default(), "ownerIgnore": m.owner_ignore,
                "revoked": m.revoked, "hold": m.hold, "holdReason": m.hold_reason, "files": m.files, "bytes": m.bytes, "recent": recent,
            })
        })
        .collect();
    ok_json(&json!({
        "own": own, "mounts": mounts, "trashMinutes": cfg.mounts.trash_minutes, "nodeIgnore": cfg.ignore,
        "mountsRoot": node.folders.mount_root(&node), "mountsEnabled": cfg.mounts.enabled, "groups": cfg.groups,
        "maxFileMb": cfg.max_file_mb, "upMbit": cfg.max_up_mbit, "downMbit": cfg.max_down_mbit,
    }))
}

async fn folders_set(State(node): St, body: axum::body::Bytes) -> Response {
    let mut req: SharedFolderConfig = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return err(400, format!("не JSON: {}", e)),
    };
    {
        let _g = node.config_lock.lock();
        let mut cfg = node.cfg.write();
        req.id = req.id.trim().into();
        if req.path.trim().is_empty() {
            req.path = format!("общие-мои/{}", req.id);
        }
        req.access.retain(|_, v| v == READ || v == WRITE);
        let mut clone = cfg.clone();
        clone.folders.retain(|f| !eq_ic(&f.id, &req.id));
        clone.folders.push(req.clone());
        let errs = clone.validate();
        if !errs.is_empty() {
            return err(422, errs.join("; "));
        }
        if let Some(old) = cfg.folder(&req.id) {
            if req.ignore.is_empty() {
                req.ignore = old.ignore.clone(); // игнор меняется через /local/ignore — не терять его при правке доступа
            }
        }
        cfg.folders.retain(|f| !eq_ic(&f.id, &req.id));
        cfg.folders.push(req.clone());
        let _ = std::fs::create_dir_all(node.resolve(&req.path));
    }
    let e = node.save_config();
    node.folders.invalidate_own();
    node.rewatch();
    node.notify_all("folders");
    node.info(&format!(
        "Панель: общая папка «{}» — доступ: {}",
        req.id,
        if req.access.is_empty() { "ни у кого".to_string() } else { req.access.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join(", ") }
    ));
    ok_json(&json!({ "ok": true, "saved": e.is_none(), "error": e }))
}

async fn folders_remove(State(node): St, Query(m): Q) -> Response {
    {
        let _g = node.config_lock.lock();
        node.cfg.write().folders.retain(|f| !eq_ic(&f.id, q(&m, "id")));
    }
    let e = node.save_config();
    node.notify_all("folders");
    ok_json(&json!({ "ok": true, "saved": e.is_none(), "note": "файлы остались на месте — закрыт только доступ" }))
}

/// Настройки передачи: корзина (trashMinutes), предел размера файла (maxFileMb, 0 — без предела),
/// скорость отдачи и приёма (upMbit, downMbit, Мбит/с; 0 или пусто — без ограничения). Любые из них.
async fn folders_settings(State(node): St, Query(m): Q) -> Response {
    let num = |k: &str| m.get(k).map(|v| v.trim().replace(',', ".")).map(|v| if v.is_empty() { Some(0.0) } else { v.parse::<f64>().ok().filter(|x| *x >= 0.0) });
    let trash = match num("trashMinutes") {
        Some(Some(v)) if v <= 525_600.0 => Some(v as i64),
        Some(_) => return err(400, "корзина: минуты от 0 до 525600"),
        None => None,
    };
    let max_mb = match num("maxFileMb") {
        Some(Some(v)) => Some(v as i64),
        Some(None) => return err(400, "предел размера: число мегабайт, 0 — без предела"),
        None => None,
    };
    let up = match num("upMbit") {
        Some(Some(v)) => Some(v),
        Some(None) => return err(400, "скорость отдачи: число Мбит/с, пусто — без ограничения"),
        None => None,
    };
    let down = match num("downMbit") {
        Some(Some(v)) => Some(v),
        Some(None) => return err(400, "скорость приёма: число Мбит/с, пусто — без ограничения"),
        None => None,
    };
    {
        let _g = node.config_lock.lock();
        let mut c = node.cfg.write();
        if let Some(v) = trash {
            c.mounts.trash_minutes = v;
        }
        if let Some(v) = max_mb {
            c.max_file_mb = v;
        }
        if let Some(v) = up {
            c.max_up_mbit = v;
        }
        if let Some(v) = down {
            c.max_down_mbit = v;
        }
    }
    node.apply_limits();
    let e = node.save_config();
    ok_json(&json!({ "ok": true, "saved": e.is_none() }))
}

async fn allow_deletes(State(node): St, Query(m): Q) -> Response {
    node.folders.allow_deletes(q(&m, "peer"), q(&m, "id"));
    node.wake.notify_one();
    ok_json(&json!({ "ok": true }))
}

async fn trash(State(node): St) -> Response {
    ok_json(&json!({ "minutes": node.cfg.read().mounts.trash_minutes, "items": node.folders.trash_list() }))
}

async fn trash_restore(State(node): St, Query(m): Q) -> Response {
    match node.folders.trash_restore(&node, q(&m, "scope"), q(&m, "batch"), q(&m, "path")) {
        Some(p) => {
            node.wake.notify_one();
            ok_json(&json!({ "ok": true, "path": p }))
        }
        None => err(404, "не удалось вернуть (нет файла или папка только для чтения)"),
    }
}

/// Корень файлового обозревателя: своя общая папка или подключённая папка соседа.
fn fs_root(node: &Node, m: &HashMap<String, String>) -> Result<(PathBuf, bool), String> {
    match q(m, "kind") {
        "own" => node.cfg.read().folder(q(m, "id")).map(|f| (node.folders.own_root(node, f), true)).ok_or_else(|| "нет такой папки".into()),
        "mount" => {
            let mm = node.folders.find_mount(q(m, "peer"), q(m, "id")).ok_or("нет такой папки соседа")?;
            Ok((PathBuf::from(&mm.root), mm.access == WRITE && !mm.revoked))
        }
        _ => Err("kind — own или mount".into()),
    }
}

async fn fs_list(State(node): St, Query(m): Q) -> Response {
    let (root, writable) = match fs_root(&node, &m) {
        Ok(x) => x,
        Err(e) => return err(404, e),
    };
    let ig = fs_ignore(&node, &m);
    let show_hidden = q(&m, "hidden") == "1";
    let mut hidden = 0;
    let mut items = walk(&root, &[]);
    items.retain(|w| !path::is_junk(&w.rel));
    items.sort_by(|a, b| a.rel.to_lowercase().cmp(&b.rel.to_lowercase()));
    let mut list = Vec::new();
    for w in items {
        let rule = ig.which(&w.rel).map(String::from);
        if rule.is_some() {
            hidden += 1;
            if !show_hidden {
                continue;
            }
        }
        if list.len() < 3000 {
            list.push(json!({ "path": w.rel, "size": w.size, "time": time::format(&time::from_ticks(w.mtime_ticks)), "ignored": rule }));
        }
    }
    ok_json(&json!({ "root": root, "writable": writable, "hidden": hidden, "ignore": ig.patterns(), "files": list }))
}

async fn fs_file(State(node): St, Query(m): Q) -> Response {
    let (root, _) = match fs_root(&node, &m) {
        Ok(x) => x,
        Err(e) => return err(404, e),
    };
    let Some(p) = path::norm(q(&m, "path")) else { return err(404, "путь") };
    let Ok(full) = path::to_local(&root, &p) else { return err(404, "путь") };
    if !full.is_file() {
        return err(404, "нет файла");
    }
    download_file(full).await
}

async fn fs_upload(State(node): St, Query(m): Q, req: Request) -> Response {
    let (root, writable) = match fs_root(&node, &m) {
        Ok(x) => x,
        Err(e) => return err(404, e),
    };
    if !writable {
        return err(403, "папка только для чтения");
    }
    let p = match path::normalize(q(&m, "path")) {
        Ok(p) if p.split('/').next() != Some(crate::folders::MY_EDITS_DIR) => p,
        Ok(_) => return err(400, "путь"),
        Err(w) => return err(400, format!("путь: {}", w)),
    };
    if let Some(rule) = fs_ignore(&node, &m).which(&p).map(String::from) {
        return err(403, format!("в игноре («{}») — сюда не кладётся", rule));
    }
    let tmp = match body_to_tmp(&node, req).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let Ok(full) = path::to_local(&root, &p) else { return err(400, "путь") };
    if q(&m, "kind") == "own" && full.exists() {
        if let Some(f) = node.cfg.read().folder(q(&m, "id")).cloned() {
            node.folders.own_local_delete(&node, &f, &p); // прежняя версия — в корзину
        }
    }
    if let Some(d) = full.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Err(e) = bridge_core::files::move_replace(&tmp, &full) {
        return err(500, e.to_string());
    }
    node.folders.invalidate_own();
    node.folders.mounts_touched.store(true, Ordering::SeqCst);
    node.folders.wake.store(true, Ordering::SeqCst);
    node.wake.notify_one();
    ok_json(&json!({ "ok": true, "path": p }))
}

async fn fs_delete(State(node): St, Query(m): Q) -> Response {
    let (_, writable) = match fs_root(&node, &m) {
        Ok(x) => x,
        Err(e) => return err(404, e),
    };
    let Some(p) = path::norm(q(&m, "path")) else { return err(404, "путь") };
    if !writable {
        return err(403, "папка только для чтения");
    }
    if q(&m, "kind") == "own" {
        if let Some(f) = node.cfg.read().folder(q(&m, "id")).cloned() {
            node.folders.own_local_delete(&node, &f, &p);
        }
    } else if let Some(mm) = node.folders.find_mount(q(&m, "peer"), q(&m, "id")) {
        node.folders.mount_local_delete(&node, &mm, &p);
    }
    node.wake.notify_one();
    ok_json(&json!({ "ok": true }))
}

// ───────────────────────── Экран и голос ─────────────────────────

/// Сигнал панели этого ПК — соседу (to). Ответ: дошло ли, открыта ли у соседа панель.
async fn media_signal(State(node): St, dev: Option<axum::Extension<crate::mobile::FromDevice>>, Json(mut msg): Json<Value>) -> Response {
    let to = msg.get("to").and_then(|t| t.as_str()).unwrap_or("").to_string();
    // Телефон прячет свои адреса за «xxxx.local» — подставить его настоящий адрес (сосед подставил бы адрес ПК)
    if let Some(d) = &dev {
        unmask_all(&mut msg, &d.ip);
    }
    let room = msg.get("room").and_then(|t| t.as_str()).unwrap_or("").to_string();
    // своё состояние в канале — запомнить (видно в API и после перезагрузки панели)
    if msg.get("media").and_then(|m| m.as_str()) == Some("room") {
        let kind = msg.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
        node.media.note_presence(&room, &node.node_name(), &kind, msg.get("st"));
    }
    if let Some(o) = msg.as_object_mut() {
        o.remove("to");
    }
    let direct = node.cfg.read().peer(&to).cloned().filter(|p| p.enabled && p.has_url());
    let peer = match direct {
        Some(p) => p,
        None => {
            // Не сосед — через владельца канала (он сосед обоим)
            let owner = room.split_once(':').map(|x| x.0.to_string()).unwrap_or_default();
            let via = node.cfg.read().peer(&owner).cloned().filter(|p| p.enabled && p.has_url() && !eq_ic(&owner, &to));
            match via {
                Some(p) => {
                    msg["relayTo"] = Value::String(to.clone());
                    p
                }
                None => return err(404, format!("«{}» не сосед этого ПК", to)),
            }
        }
    };
    // Сосед без связи — сразу отказ, не ждать таймаута (панель шлёт состояние каждые 5 с)
    let ps = node.peer_state(&peer.name);
    if !ps.online && ps.last_seen.is_some() {
        return err(503, format!("нет связи с «{}»: {}", peer.name, ps.error));
    }
    let client = node.client_for(&peer);
    match tokio::time::timeout(std::time::Duration::from_secs(8), client.signal(&msg)).await {
        Ok(Ok(())) => ok_json(&json!({ "ok": true })),
        Ok(Err(e)) => err(e.status().unwrap_or(502), link::explain(&e, &peer).0),
        Err(_) => err(504, link::explain_timeout(&peer).0),
    }
}

fn unmask_all(msg: &mut Value, ip: &str) {
    if let Some(c) = msg.get_mut("candidate").and_then(|c| c.get_mut("candidate")) {
        if let Some(text) = c.as_str() {
            *c = Value::String(crate::http_peer::unmask_candidate(text, ip));
        }
    }
    if let Some(sdp) = msg.get_mut("desc").and_then(|d| d.get_mut("sdp")) {
        if let Some(text) = sdp.as_str() {
            let fixed: Vec<String> = text.split("\r\n").map(|l| if l.starts_with("a=candidate:") { crate::http_peer::unmask_candidate(l, ip) } else { l.to_string() }).collect();
            *sdp = Value::String(fixed.join("\r\n"));
        }
    }
}

/// Ретрансляторы для WebRTC: соседей (из их приветов) и свой — для телефона (по тому имени, по которому он открыл мост).
async fn media_ice(State(node): St, dev: Option<axum::Extension<crate::mobile::FromDevice>>) -> Response {
    let mut list: Vec<Value> = Vec::new();
    let port = node.turn.port.load(Ordering::SeqCst);
    if let Some(d) = &dev {
        if node.turn.running() && !d.host.is_empty() {
            let (u, c) = node.turn.creds(&format!("phone-{}", d.name), std::time::Duration::from_secs(12 * 3600));
            list.push(json!({ "urls": [format!("turn:{}:{}?transport=udp", d.host, port)], "username": u, "credential": c }));
        }
    }
    // телефону ретрансляторы соседей не нужны (Radmin и т. п. ему недоступны): только свой ПК
    let peers: Vec<(String, String)> = if dev.is_some() { Vec::new() } else { node.cfg.read().peers.iter().filter(|p| p.enabled && p.has_url()).map(|p| (p.name.clone(), p.url.clone())).collect() };
    for (name, url) in peers {
        let st = node.peer_state(&name);
        let (Some(t), true) = (st.turn, st.online) else { continue };
        let Some((ip, _)) = bridge_core::config::parse_url(&url) else { continue };
        let host = match ip {
            std::net::IpAddr::V6(v) => format!("[{}]", v),
            v => v.to_string(),
        };
        list.push(json!({ "urls": [format!("turn:{}:{}?transport=udp", host, t.port)], "username": t.username, "credential": t.credential }));
    }
    ok_json(&json!({ "iceServers": list }))
}

/// Сообщение панелям этого же ПК (и телефонам): «зашёл в канал с другого устройства» и т. п.
async fn media_local(State(node): St, Json(mut msg): Json<Value>) -> Response {
    if !msg.is_object() {
        return err(400, "не объект");
    }
    msg["media"] = json!("local");
    let _ = node.media.tx.send(msg);
    ok_json(&json!({ "ok": true }))
}

/// Поток сигналов для панели (SSE): приглашения, предложения, ответы, ICE.
async fn media_stream(State(node): St, dev: Option<axum::Extension<crate::mobile::FromDevice>>) -> Response {
    let rx = node.media.tx.subscribe();
    // телефон — не панель: пульт ассистента и входящие звонки по-прежнему открывают панель на ПК
    let counted = dev.is_none();
    if counted {
        node.media.listeners.fetch_add(1, Ordering::SeqCst);
    }
    let guard = ListenerGuard(node.clone(), counted);
    let stream = tokio_stream_from(rx).map(move |v| {
        let _keep = &guard;
        Ok::<_, std::convert::Infallible>(Event::default().event("signal").data(v.to_string()))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15))).into_response()
}

struct ListenerGuard(Arc<Node>, bool);

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        if self.1 {
            self.0.media.listeners.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

async fn media_invites(State(node): St) -> Response {
    ok_json(&node.media.pending_invites())
}

/// Перечень локального API (для ассистентов: что можно вызвать).
async fn api_index() -> Response {
    ok_json(&json!({
        "prefix": format!("{}/local/", API_PREFIX),
        "auth": "заголовок X-Bridge-Local: <bridge-data/local.token> (или ?token= для потоков событий); только с 127.0.0.1",
        "events": "GET events — поток SSE: peer-online, peer-offline, peer-named, file-in, mailbox-received, mailbox-sent, record, records-in, command-waiting, media-invite, folder-write, room-join, room-leave, room-state, agent, device-paired, device-outbox, chat-msg, assistant-in, assistant-msg",
        "ui": "POST ui {action, …} — пульт панели: state, join {channel, quiet?}, leave, mic {on}, deafen {on}, volume {peer, value}, call {peers}, answer {room?}, decline {room?}, share {quality?}, unshare {id?}, watch {peer|stream, on?}, popout {peer|stream}, ask_screen {peer}, tab {tab}, board_png {room, ids?|area?, scale?} → {path}, board_view {room, ids?|area?|x,y}, screen_shot {peer|stream} → {path}, screen_point {peer, x, y}, screen_draw {peer, points}, screen_clear {peer}",
        "docs": "полный справочник — API.md рядом с программой",
        "calls": [
            "GET overview — всё для панели", "GET status", "GET peers", "POST peers/connect {address, secret?, title?}", "POST peers/invite?title",
            "GET peers/check[?name]", "POST peers/rename?name&to", "POST peers/remove?name", "POST peers {name, title?, ip?, port?, extraIps?, secret?, enabled?, shares?, groups?}",
            "GET groups", "POST groups {id, title, members}", "POST groups/remove?id", "GET access — кто что видит",
            "POST mailbox/upload?peer&path (тело — файл)", "POST upload/part?id&offset (большие файлы частями; затем вызов с ?upload=id&size=N)", "GET builds", "GET mounts/plan?peer&id", "POST builds/send {peer, path, name, mode: patch|clean}", "POST builds/target {peer, name, path}", "GET mailbox/outgoing", "POST mailbox/cancel?peer&path", "GET mailbox/incoming", "GET mailbox/incoming/file?path",
            "GET folders", "POST folders {id, title, path, access:{сосед|@группа|*: read|write}}", "POST folders/remove?id", "GET fs/list?kind&id[&peer]", "POST fs/upload?kind&id[&peer]&path", "POST fs/delete", "GET fs/file",
            "GET ignore", "POST ignore {scope, id, peer, patterns}", "POST ignore/add|remove {scope, id, peer, pattern}", "GET ignore/test?path[&scope&id&peer]",
            "GET records[?share&type&status&assignee&q]", "GET records/{id}", "GET records/{id}/history", "POST actions {action, …}", "GET wait?share&after&timeout",
            "GET channels", "POST channels {id, title, members:[сосед|@группа|*]}", "POST channels/remove?id", "GET media/presence — кто в каком канале и что делает",
            "GET transfers — идущие передачи (процент, скорость, осталось)", "POST folders/settings?upMbit&downMbit&maxFileMb&trashMinutes",
            "POST mailbox/send {peer, path, dest?}", "GET changelog", "GET agent", "POST agent/ask {peer, prompt, thread?}", "GET agent/wait?id&after&timeout", "POST agent/stop?id", "POST agent/request {thread, question, buttons?, input?}", "GET agent/request/wait?thread&id&timeout", "POST agent/answer {thread, req, answer}",
            "POST sync", "GET log?lines", "POST media/signal {to, type, room, …}", "GET media/stream (SSE)", "POST open?target",
            "GET chat — беседы", "GET chat/messages?conv&after&timeout", "POST chat/send {conv | to | channel, text, author?}", "POST chat/read?conv",
            "GET devices — телефоны", "POST devices/pair {rights?} — код и QR для подключения", "POST devices {id, name?, rights?}", "POST devices/remove?id",
            "POST devices/send {device?, path, name?} — файл с диска на телефон", "POST devices/upload?device&path (тело — файл)",
            "GET mobile[?fresh=1] — телефоны, адреса, Tailscale", "POST mobile/https {on} — HTTPS через Tailscale", "POST mobile {enabled?, outbox?}",
            "GET board/rooms", "GET board/describe?room", "GET board/find?room&q&type&layer&by&x0&y0&x1&y1&limit", "GET board/get?room&ids",
            "POST board/add {room, objs: [{type: text|note|rect|arrow|line|path|image|pdf, …}], layer?, at?, layout?}", "POST board/edit {room, id|ids, …}", "POST board/move {room, ids, dx, dy | x, y}",
            "POST board/remove {room, ids}", "POST board/clear {room, layer?}", "POST board/layer {room, action, layer?, name?, to?}", "POST board/bg {room, color?, light?}", "POST board/enable {room, on}",
            "GET tile[?peer]", "POST tile {style}", "POST tile/image (тело — картинка)", "POST tile/image/remove", "POST tile/reset", "POST tile/status {text?, color?}", "GET|POST tile/status/presets {list: [{id, text, color}]}", "POST tile/status/apply {id}", "GET|POST tile/status/preset/image?id", "POST tile/status/preset/image/remove?id", "GET tile/status/image[?peer]", "POST tile/status/image", "POST tile/status/image/remove",
        ],
    }))
}

#[allow(dead_code)]
fn status_code(c: u16) -> StatusCode {
    StatusCode::from_u16(c).unwrap_or(StatusCode::OK)
}

#[allow(dead_code)]
fn group_ref(cfg: &NodeConfig, g: &str) -> bool {
    get_ic(&cfg.groups, g).is_some()
}

async fn media_presence(State(node): St) -> Response {
    ok_json(&node.media.presence_list())
}

/// Каналы: свои (с настройками) и все видимые (свои + соседей).
async fn channels_get(State(node): St) -> Response {
    let cfg = node.cfg.read().clone();
    let own: Vec<Value> = cfg.channels.iter().map(|(id, c)| json!({ "id": id, "title": c.title, "members": c.members, "resolved": cfg.channel_members(c) })).collect();
    ok_json(&json!({ "me": cfg.node, "own": own, "all": crate::media::all_channels(&node), "presence": node.media.presence_list() }))
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct ChannelReq {
    id: String,
    title: String,
    members: Vec<String>,
}

async fn channels_set(State(node): St, body: axum::body::Bytes) -> Response {
    let r: ChannelReq = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return err(400, format!("не JSON: {}", e)),
    };
    let id = r.id.trim().to_string();
    {
        let _g = node.config_lock.lock();
        let mut clone = node.cfg.read().clone();
        let key = clone.channels.keys().find(|k| eq_ic(k, &id)).cloned().unwrap_or(id.clone());
        clone.channels.insert(key, bridge_core::config::ChannelConfig { title: r.title.trim().into(), members: r.members.clone() });
        let errors = clone.validate();
        if !errors.is_empty() {
            return err(422, errors.join("; "));
        }
        *node.cfg.write() = clone;
    }
    let e = node.save_config();
    node.info(&format!("Панель: канал «{}» — участники: {}", id, if r.members.is_empty() { "никого".into() } else { r.members.join(", ") }));
    node.request_sync();
    ok_json(&json!({ "ok": true, "saved": e.is_none(), "error": e }))
}

// ───────────── Пространства: создать, изменить, убрать ─────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct ShareReq {
    id: String,
    title: String,
    folder: String,
    peers: Vec<String>,
    accept: Vec<bridge_core::config::FileAcceptRule>,
    #[serde(rename = "recordsAccept")]
    records_accept: Vec<bridge_core::config::RecordAcceptRule>,
}

/// POST shares {id, title, folder, peers, accept: [{pattern, from}], recordsAccept: [{from, types, ops}]} — создать или изменить.
/// Папка по умолчанию — «Пространства/<id>» рядом с мостом. Сменили папку — пространство открывается заново (файлы не переносятся).
async fn shares_set(State(node): St, Json(r): Json<ShareReq>) -> Response {
    let id = r.id.trim().to_string();
    if !bridge_core::config::is_name(&id) {
        return err(400, "id пространства — латиница, цифры, «-», «_»");
    }
    let folder = if r.folder.trim().is_empty() { format!("Пространства/{}", id) } else { r.folder.trim().to_string() };
    let (fresh, reopen) = {
        let _g = node.config_lock.lock();
        let mut clone = node.cfg.read().clone();
        let old = clone.shares.iter().position(|s| eq_ic(&s.id, &id));
        let mut sc = old.map(|i| clone.shares[i].clone()).unwrap_or_default();
        let reopen = old.is_some() && node.resolve(&sc.folder) != node.resolve(&folder);
        sc.id = old.map(|i| clone.shares[i].id.clone()).unwrap_or(id.clone());
        sc.title = r.title.trim().to_string();
        sc.folder = folder.clone();
        sc.peers = r.peers.iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
        sc.accept = r.accept.into_iter().filter(|a| !a.pattern.trim().is_empty()).collect();
        sc.records.accept = r.records_accept;
        match old {
            Some(i) => clone.shares[i] = sc,
            None => clone.shares.push(sc),
        }
        let errors = clone.validate();
        if !errors.is_empty() {
            return err(422, errors.join("; "));
        }
        *node.cfg.write() = clone;
        (old.is_none(), reopen)
    };
    let e = node.save_config();
    let sc = node.cfg.read().shares.iter().find(|s| eq_ic(&s.id, &id)).cloned().unwrap();
    let _ = std::fs::create_dir_all(node.resolve(&sc.folder));
    if fresh || reopen {
        if reopen {
            node.shares.write().retain(|s| !eq_ic(&s.id, &id));
        }
        node.open_share(&sc);
        node.rewatch();
    }
    node.info(&format!("Панель: пространство «{}» {}", id, if fresh { "создано" } else { "изменено" }));
    node.request_sync();
    ok_json(&json!({ "ok": true, "created": fresh, "saved": e.is_none(), "error": e }))
}

/// POST shares/remove?id — убрать пространство с этого ПК (папка и файлы остаются на диске).
async fn shares_remove(State(node): St, Query(m): Q) -> Response {
    let id = q(&m, "id").to_string();
    let had = {
        let _g = node.config_lock.lock();
        let mut c = node.cfg.write();
        let n = c.shares.len();
        c.shares.retain(|s| !eq_ic(&s.id, &id));
        for r in c.runners.values_mut() {
            if eq_ic(&r.share, &id) {
                r.enabled = false;
            }
        }
        c.shares.len() != n
    };
    if !had {
        return err(404, "нет такого пространства");
    }
    node.shares.write().retain(|s| !eq_ic(&s.id, &id));
    node.rewatch();
    let e = node.save_config();
    node.info(&format!("Панель: пространство «{}» убрано (папка осталась на диске)", id));
    node.request_sync();
    ok_json(&json!({ "ok": true, "saved": e.is_none(), "error": e }))
}

/// GET shares/offers — пространства, которые соседи открыли этому ПК, а здесь их ещё нет.
async fn shares_offers(State(node): St) -> Response {
    let own: Vec<String> = node.cfg.read().shares.iter().map(|s| s.id.to_lowercase()).collect();
    let mut out = Vec::new();
    for p in node.peers.lock().values() {
        for s in &p.shares {
            if !own.contains(&s.to_lowercase()) {
                out.push(json!({ "peer": p.name, "id": s }));
            }
        }
    }
    ok_json(&json!({ "offers": out }))
}

async fn channels_remove(State(node): St, Query(m): Q) -> Response {
    let id = q(&m, "id").to_string();
    {
        let _g = node.config_lock.lock();
        node.cfg.write().channels.retain(|k, _| !eq_ic(k, &id));
    }
    let e = node.save_config();
    node.request_sync();
    ok_json(&json!({ "ok": true, "saved": e.is_none(), "error": e }))
}

/// Идущие передачи файлов: [{kind: up|down|serve|recv, peer, what, path, total, done, secs, speed, eta}].
async fn transfers_get(State(node): St) -> Response {
    ok_json(&node.transfers.list())
}

// ───────────────────────── Агент ─────────────────────────

fn agent_err((code, why): (u16, String)) -> Response {
    err(code, why)
}

/// Всё для вкладки «Агенты»: настройки, найден ли Claude, нити, у кого из соседей агент.
async fn agent_get(State(node): St) -> Response {
    let cfg = node.cfg.read().clone();
    let peers: Vec<Value> = cfg
        .peers
        .iter()
        .filter(|p| p.enabled && !p.pending)
        .map(|p| {
            let s = node.peer_state(&p.name);
            json!({ "name": p.name, "title": p.title, "online": s.online, "agent": s.agent, "mayAsk": cfg.agent_allows(&p.name) })
        })
        .collect();
    let exchange = if cfg.agent.exchange.is_empty() {
        Value::Null
    } else {
        cfg.folder(&cfg.agent.exchange).map(|f| json!({ "id": f.id, "path": node.folders.own_root(&node, f), "access": f.access })).unwrap_or(Value::Null)
    };
    ok_json(&json!({
        "settings": cfg.agent, "claude": crate::agent::find_claude(&node).map(|p| p.display().to_string()), "workDir": crate::agent::work_dir(&node),
        "exchange": exchange, "peers": peers, "threads": node.agent.list(), "mcp": mcp_hint(&node),
        "local": { "name": node.node_name(), "mode": cfg.agent.mode },
    }))
}

async fn agent_settings(State(node): St, body: axum::body::Bytes) -> Response {
    let v: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return err(400, format!("не JSON: {}", e)),
    };
    {
        let _g = node.config_lock.lock();
        let mut clone = node.cfg.read().clone();
        let mut a = serde_json::to_value(&clone.agent).unwrap_or(Value::Null);
        if let (Some(dst), Some(src)) = (a.as_object_mut(), v.as_object()) {
            for (k, x) in src {
                dst.insert(k.clone(), x.clone());
            }
        }
        clone.agent = match serde_json::from_value(a) {
            Ok(x) => x,
            Err(e) => return err(400, format!("настройки агента: {}", e)),
        };
        let errors = clone.validate();
        if !errors.is_empty() {
            return err(422, errors.join("; "));
        }
        *node.cfg.write() = clone;
    }
    let e = node.save_config();
    let a = node.cfg.read().agent.clone();
    node.info(&format!("Панель: агент — {}{}", match a.mode.as_str() { "auto" => "сам запускает Claude", "manual" => "поручения ждут сессию Claude", _ => "выключен" }, if a.allow_from.is_empty() { String::new() } else { format!(", поручать могут: {}", a.allow_from.join(", ")) }));
    crate::agent::schedule(&node);
    node.request_sync();
    ok_json(&json!({ "ok": true, "saved": e.is_none(), "error": e }))
}

async fn agent_ask(State(node): St, Json(v): Json<Value>) -> Response {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
    let peer = s("peer").unwrap_or_default();
    match crate::agent::ask_full(&node, &peer, &s("prompt").unwrap_or_default(), s("thread").as_deref(), s("title").as_deref(), &v["record"]).await {
        Ok(t) => ok_json(&thread_view(&node, &t, -1)),
        Err(e) => agent_err(e),
    }
}

async fn agent_thread(State(node): St, Query(m): Q) -> Response {
    let after = m.get("after").and_then(|x| x.parse().ok()).unwrap_or(-1);
    match node.agent.get(q(&m, "id")) {
        Some(t) => ok_json(&thread_view(&node, &t, after)),
        None => err(404, "нет такой нити"),
    }
}

/// Нить для панели и инструментов: с путём обменной папки на этом ПК.
fn thread_view(node: &Node, t: &crate::agent::Thread, after: i64) -> Value {
    let mut v = t.since(after);
    if let Some(p) = crate::agent::exchange_path(node, t) {
        v["exchangePath"] = json!(p.display().to_string());
    }
    v
}

/// Долгое ожидание: пока нить в работе и нового итога нет (до timeout секунд, по умолчанию 300).
async fn agent_wait(State(node): St, Query(m): Q) -> Response {
    let after = m.get("after").and_then(|x| x.parse().ok()).unwrap_or(-1);
    let secs = m.get("timeout").and_then(|x| x.parse::<u64>().ok()).unwrap_or(300).clamp(1, 3600);
    match crate::agent::wait(&node, q(&m, "id"), after, std::time::Duration::from_secs(secs)).await {
        Some(t) => ok_json(&thread_view(&node, &t, after)),
        None => err(404, "нет такой нити"),
    }
}

async fn agent_stop(State(node): St, Query(m): Q) -> Response {
    match crate::agent::stop(&node, q(&m, "id")).await {
        Ok(()) => ok_json(&json!({ "ok": true })),
        Err(e) => agent_err(e),
    }
}

/// Убрать нить из списка (только у себя).
async fn agent_delete(State(node): St, Query(m): Q) -> Response {
    let id = q(&m, "id").to_string();
    if node.agent.get(&id).map(|t| t.busy() && t.role == "in").unwrap_or(false) {
        crate::agent::stop_local(&node, &id, "удалено");
    }
    node.agent.threads.lock().remove(&id);
    let _ = std::fs::remove_file(node.data_dir.join("agent").join(format!("{}.json", id.replace(['/', '\\', ':'], "_"))));
    ok_json(&json!({ "ok": true }))
}

async fn agent_inbox(State(node): St) -> Response {
    ok_json(&crate::agent::inbox(&node))
}

/// Ручной режим: ждать, пока придёт поручение (до timeout секунд).
async fn agent_wait_task(State(node): St, Query(m): Q) -> Response {
    let secs = m.get("timeout").and_then(|x| x.parse::<u64>().ok()).unwrap_or(300).clamp(1, 3600);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        let list = crate::agent::inbox(&node);
        if !list.is_empty() {
            return ok_json(&list);
        }
        let n = node.agent.changed.notified();
        tokio::select! {
            _ = n => {}
            _ = tokio::time::sleep_until(deadline) => return ok_json(&Vec::<Value>::new()),
        }
    }
}

async fn agent_take(State(node): St, Query(m): Q) -> Response {
    match crate::agent::take(&node, q(&m, "id")) {
        Ok(v) => ok_json(&v),
        Err(e) => agent_err(e),
    }
}

async fn agent_progress(State(node): St, Json(v): Json<Value>) -> Response {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    match crate::agent::progress_full(&node, &s("thread"), &s("text"), &s("phase"), v.get("waiting").and_then(|x| x.as_bool())) {
        Ok(()) => ok_json(&json!({ "ok": true })),
        Err(e) => agent_err(e),
    }
}

/// POST agent/request {thread, question, buttons?, input?} — агент спрашивает людей. → вопрос {id, …}.
async fn agent_request(State(node): St, Json(v): Json<Value>) -> Response {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let buttons: Vec<String> = v["buttons"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    match crate::agent::request(&node, &s("thread"), &s("question"), &buttons, v["input"].as_bool().unwrap_or(false)) {
        Ok(r) => ok_json(&json!(r)),
        Err(e) => agent_err(e),
    }
}

/// GET agent/request/wait?thread&id&timeout — ждать ответа (до timeout секунд, по умолчанию 300).
async fn agent_request_wait(State(node): St, Query(m): Q) -> Response {
    let secs = m.get("timeout").and_then(|x| x.parse::<u64>().ok()).unwrap_or(300).clamp(1, 3600);
    match crate::agent::wait_answer(&node, q(&m, "thread"), q(&m, "id"), std::time::Duration::from_secs(secs)).await {
        Some(r) => ok_json(&json!({ "answered": true, "req": r })),
        None => ok_json(&json!({ "answered": false, "thread": node.agent.get(q(&m, "thread")).map(|t| t.status) })),
    }
}

/// POST agent/answer {thread, req, answer} — ответить агенту (из панели; у заказчика уйдёт исполнителю).
async fn agent_answer(State(node): St, Json(v): Json<Value>) -> Response {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    match crate::agent::answer(&node, &s("thread"), &s("req"), &s("answer"), &node.node_name()).await {
        Ok(()) => ok_json(&json!({ "ok": true })),
        Err(e) => agent_err(e),
    }
}

async fn agent_report(State(node): St, Json(v): Json<Value>) -> Response {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(true);
    match crate::agent::report(&node, &s("thread"), &s("result"), ok) {
        Ok(()) => ok_json(&json!({ "ok": true })),
        Err(e) => agent_err(e),
    }
}

/// Обменная папка для агентов: своя общая папка «agent-obmen» (создать при нужде) + запись соседу.
async fn agent_exchange(State(node): St, Query(m): Q) -> Response {
    let peer = q(&m, "peer").to_string();
    if node.cfg.read().peer(&peer).is_none() {
        return err(404, "нет такого соседа");
    }
    {
        let _g = node.config_lock.lock();
        let mut c = node.cfg.write();
        let id = if c.agent.exchange.is_empty() { "agent-obmen".to_string() } else { c.agent.exchange.clone() };
        if c.folder(&id).is_none() {
            c.folders.push(SharedFolderConfig { id: id.clone(), title: "Обмен с агентами".into(), path: "общие-мои/агент-обмен".into(), ..Default::default() });
        }
        if let Some(f) = c.folders.iter_mut().find(|f| eq_ic(&f.id, &id)) {
            f.access.insert(peer.clone(), "write".into());
        }
        c.agent.exchange = id;
    }
    let e = node.save_config();
    let c = node.cfg.read().clone();
    if let Some(f) = c.folder(&c.agent.exchange) {
        let _ = std::fs::create_dir_all(node.folders.own_root(&node, f));
    }
    node.rewatch();
    node.request_sync();
    node.info(&format!("Агент: обменная папка «{}» — запись для «{}»", c.agent.exchange, peer));
    ok_json(&json!({ "ok": true, "saved": e.is_none(), "id": c.agent.exchange }))
}

/// Как подключить инструменты моста к Claude на этом ПК: путь к bridge.exe (рядом с программой) и к настройкам.
fn mcp_hint(node: &Node) -> Value {
    let dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())).unwrap_or_default();
    let exe = dir.join(if cfg!(windows) { "bridge.exe" } else { "bridge" });
    let cfgf = node.config_file.clone().unwrap_or_else(|| node.base_dir.join("bridge.json"));
    let (e, c) = (exe.display().to_string(), cfgf.display().to_string());
    json!({
        "exe": e, "config": c, "exeFound": exe.is_file(),
        "json": json!({ "mcpServers": { "bridge": { "command": e, "args": ["mcp", "--config", c] } } }),
        "cli": format!("claude mcp add --scope user bridge -- \"{}\" mcp --config \"{}\"", e, c),
    })
}

/// Журнал изменений (встроен в программу): панель показывает его по щелчку на версии.
async fn changelog() -> Response {
    ok_json(&json!({ "version": bridge_core::APP_VERSION, "text": include_str!("../../CHANGELOG.md") }))
}

// ───────────────────────── Пульт панели (для ассистента) ─────────────────────────

/// Действие в панели: {action, ...}. Действия: tab, state, join, leave, mic, deafen, voice, share, unshare, watch, popout, call, ask_screen.
async fn ui_cmd(State(node): St, Json(v): Json<Value>) -> Response {
    let action = v.get("action").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if action.is_empty() {
        return err(400, "нужен action");
    }
    // показ экрана ждёт выбора человека — дольше
    let secs = v.get("timeout").and_then(|x| x.as_u64()).unwrap_or(if action == "share" { 120 } else { 30 }).clamp(1, 600);
    // доска: room можно назвать как угодно (id или название канала) — панель ждёт «владелец:канал»
    let mut v = v;
    if action.starts_with("board_") {
        if let Some(r) = v.get("room").and_then(|x| x.as_str()).filter(|r| !r.is_empty()) {
            match crate::board_api::resolve_room(&node, r) {
                Ok(k) => v["room"] = json!(k),
                Err((c, e)) => return err(c, e),
            }
        }
    }
    match crate::media::ui_command(&node, &action, v.clone(), std::time::Duration::from_secs(secs)).await {
        Ok(data) => ok_json(&json!({ "ok": true, "data": data })),
        Err((c, w)) => err(c, w),
    }
}

/// Панель берётся выполнить команду: первая — да, остальные — нет.
async fn ui_claim(State(node): St, Json(v): Json<Value>) -> Response {
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let pid = v.get("panel").and_then(|x| x.as_str()).unwrap_or("").to_string();
    // кто лучше: окно в канале (1000) > в фокусе (100) > открытое позже (секунды с открытия — меньше лучше)
    let score = v.get("score").and_then(|x| x.as_i64()).unwrap_or(0);
    if !node.media.ui_waiters.lock().contains_key(&id) {
        return ok_json(&json!({ "mine": false }));
    }
    node.media.ui_claims.lock().entry(id.clone()).or_default().push((pid.clone(), score));
    // собрать отклики всех окон
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let best = node.media.ui_claims.lock().get(&id).and_then(|l| l.iter().max_by_key(|(_, s)| *s).map(|(p, _)| p.clone()));
    ok_json(&json!({ "mine": best.as_deref() == Some(pid.as_str()) }))
}

async fn ui_result(State(node): St, Json(v): Json<Value>) -> Response {
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if let Some(tx) = node.media.ui_waiters.lock().remove(&id) {
        let _ = tx.send(v);
    }
    ok_json(&json!({ "ok": true }))
}

/// Отправить соседу файл с диска этого ПК (для ассистента — без загрузки тела): {peer, path, dest?}.
async fn mailbox_send(State(node): St, Json(v): Json<Value>) -> Response {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let (peer, path) = (s("peer"), s("path"));
    let src = std::path::PathBuf::from(&path);
    if !src.is_file() {
        return err(404, format!("нет файла {}", path));
    }
    if node.cfg.read().peer(&peer).is_none() {
        return err(404, format!("нет соседа «{}»", peer));
    }
    let dest = if s("dest").is_empty() { src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { s("dest") };
    let Some(rel) = bridge_core::path::norm(&dest) else { return err(400, "имя у соседа: без «..» и абсолютных путей") };
    let cfg = node.cfg.read().clone();
    let root = node.mailbox.outgoing_root(&node, &cfg).join(&peer);
    let Ok(full) = bridge_core::path::to_local(&root, &rel) else { return err(400, "путь") };
    if let Some(d) = full.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    match std::fs::copy(&src, &full) {
        Ok(n) => {
            node.mailbox.dirty.store(true, std::sync::atomic::Ordering::SeqCst);
            node.notify_peers(vec![peer.clone()], "", "mailbox");
            ok_json(&json!({ "ok": true, "path": rel, "size": n }))
        }
        Err(e) => err(500, e.to_string()),
    }
}

// ───────────── Телефоны ─────────────

async fn devices_get(State(node): St) -> Response {
    ok_json(&node.mobile.list().iter().map(|d| d.view()).collect::<Vec<_>>())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DeviceSet {
    id: String,
    name: Option<String>,
    rights: Option<String>,
}

async fn devices_set(State(node): St, Json(b): Json<DeviceSet>) -> Response {
    if let Some(r) = &b.rights {
        if r != "full" && r != "files" {
            return err(400, "права: full или files");
        }
    }
    let name = b.name.map(|n| n.trim().chars().take(40).collect::<String>()).filter(|n| !n.is_empty());
    match node.mobile.update(&node, &b.id, |d| {
        if let Some(n) = name {
            d.name = n;
        }
        if let Some(r) = b.rights {
            d.rights = r;
        }
    }) {
        Some(d) => ok_json(&json!({ "ok": true, "device": d.view() })),
        None => err(404, "нет такого телефона"),
    }
}

async fn devices_remove(State(node): St, Query(m): Q) -> Response {
    let id = node.mobile.find(q(&m, "id")).map(|d| d.id).unwrap_or_default();
    if node.mobile.remove(&node, &id) {
        node.info("Телефон отключён — его ключ больше не действует");
        ok_json(&json!({ "ok": true }))
    } else {
        err(404, "нет такого телефона")
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PairBody {
    rights: String,
}

/// Код и QR для подключения телефона: по одному на каждый адрес, по которому телефон может открыть мост.
async fn devices_pair(State(node): St, body: Option<Json<PairBody>>) -> Response {
    if !node.cfg.read().mobile.enabled {
        return err(403, "телефоны выключены (mobile.enabled)");
    }
    let rights = body.map(|b| b.0.rights).unwrap_or_default();
    let (code, until) = node.mobile.new_code(&rights);
    let secs = until.saturating_duration_since(std::time::Instant::now()).as_secs();
    let urls: Vec<Value> = crate::mobile::addresses(&node)
        .await
        .into_iter()
        .map(|mut a| {
            let url = format!("{}/m/#p={}", a["base"].as_str().unwrap_or(""), code);
            a["qr"] = json!(crate::mobile::qr_svg(&url));
            a["url"] = json!(url);
            a
        })
        .collect();
    ok_json(&json!({ "code": code, "expiresIn": secs, "urls": urls }))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DevSend {
    device: String,
    path: String,
    name: String,
}

/// Положить файл с диска «на телефон» (копия). Для ассистента: POST devices/send {device, path}.
async fn devices_send(State(node): St, Json(b): Json<DevSend>) -> Response {
    let Some(d) = pick_device(&node, &b.device) else { return err(404, "нет такого телефона") };
    let src = PathBuf::from(&b.path);
    if !src.is_file() {
        return err(404, format!("нет файла {}", b.path));
    }
    let name = if b.name.is_empty() { src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { b.name };
    match crate::mobile::put_outbox(&node, &d, &name, &src, false) {
        Ok(p) => ok_json(&json!({ "ok": true, "device": d.name, "path": p })),
        Err(e) => err(400, e),
    }
}

/// Файл байтами «на телефон» (из панели): POST devices/upload?device&path, тело — файл.
async fn devices_upload(State(node): St, Query(m): Q, req: Request) -> Response {
    let Some(d) = pick_device(&node, q(&m, "device")) else { return err(404, "нет такого телефона") };
    let tmp = match body_to_tmp(&node, req).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    match crate::mobile::put_outbox(&node, &d, q(&m, "path"), &tmp, true) {
        Ok(p) => ok_json(&json!({ "ok": true, "device": d.name, "path": p })),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            err(400, e)
        }
    }
}

/// Телефон по id или имени; если не указан и телефон один — он.
fn pick_device(node: &Node, key: &str) -> Option<crate::mobile::Device> {
    if key.is_empty() {
        let l = node.mobile.list();
        return if l.len() == 1 { l.into_iter().next() } else { None };
    }
    node.mobile.find(key)
}

async fn mobile_get(State(node): St, Query(m): Q) -> Response {
    let ts = node.mobile.tailscale(q(&m, "fresh") == "1").await;
    let cfg = node.cfg.read().mobile.clone();
    let (need, error) = node.mobile.https_note.lock().clone();
    let addresses = crate::mobile::addresses(&node).await;
    let outbox = node.resolve(&cfg.outbox);
    ok_json(&json!({
        "enabled": cfg.enabled, "https": cfg.https, "httpsPort": cfg.https_port, "httpsUrl": crate::mobile::https_url(&node).await,
        "needEnable": need, "httpsError": error, "tailscale": ts, "addresses": addresses,
        "outbox": outbox, "devices": node.mobile.list().iter().map(|d| d.view()).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MobileSet {
    enabled: Option<bool>,
    outbox: Option<String>,
}

async fn mobile_set(State(node): St, Json(b): Json<MobileSet>) -> Response {
    let _g = node.config_lock.lock();
    {
        let mut c = node.cfg.write();
        if let Some(e) = b.enabled {
            c.mobile.enabled = e;
        }
        if let Some(o) = b.outbox.filter(|o| !o.trim().is_empty()) {
            c.mobile.outbox = o.trim().to_string();
        }
    }
    match node.save_config() {
        None => ok_json(&json!({ "ok": true })),
        Some(e) => err(500, e),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct HttpsSet {
    on: bool,
}

/// HTTPS через Tailscale: включить — Tailscale раздаёт мост по https://<пк>.<сеть>.ts.net; выключить — перестаёт.
async fn mobile_https(State(node): St, Json(b): Json<HttpsSet>) -> Response {
    let save = |https: &str, port: u16| {
        let _g = node.config_lock.lock();
        {
            let mut c = node.cfg.write();
            c.mobile.https = https.into();
            c.mobile.https_port = port;
        }
        node.save_config()
    };
    if !b.on {
        let port = node.cfg.read().mobile.https_port;
        let off = if port != 0 { crate::tailscale::serve_off(port).await } else { Ok(()) };
        *node.mobile.https_note.lock() = Default::default();
        if let Some(e) = save("off", 0) {
            return err(500, e);
        }
        node.info("Телефон: HTTPS через Tailscale выключен");
        return ok_json(&json!({ "ok": true, "warning": off.err() }));
    }
    let ts = node.mobile.tailscale(true).await;
    if !ts.installed {
        return err(400, "Tailscale не установлен на этом ПК. Установите его (tailscale.com/download), войдите — и на телефоне тоже, под той же учётной записью.");
    }
    if !ts.running {
        return err(400, format!("Tailscale не подключён (состояние: {}). Откройте Tailscale и войдите.", if ts.state.is_empty() { "неизвестно" } else { &ts.state }));
    }
    let target = crate::mobile::serve_target(&node);
    let want = node.cfg.read().mobile.https_port;
    match crate::tailscale::serve_on(&target, want).await {
        crate::tailscale::ServeResult::On(port) => {
            *node.mobile.https_note.lock() = Default::default();
            if let Some(e) = save("tailscale", port) {
                return err(500, e);
            }
            node.info(&format!("Телефон: HTTPS через Tailscale включён — https://{}{}", ts.dns, if port == 443 { String::new() } else { format!(":{}", port) }));
            ok_json(&json!({ "ok": true, "url": crate::mobile::https_url(&node).await }))
        }
        crate::tailscale::ServeResult::NeedEnable(url) => {
            *node.mobile.https_note.lock() = (url.clone(), String::new());
            ok_json(&json!({ "ok": false, "needEnable": url }))
        }
        crate::tailscale::ServeResult::Failed(e) => {
            *node.mobile.https_note.lock() = (String::new(), e.clone());
            err(400, format!("Tailscale не включил HTTPS: {}", e))
        }
    }
}

// ───────────── Ассистент ─────────────

async fn assistant_info(State(node): St) -> Response {
    ok_json(&node.assistant.info())
}

async fn assistant_wait(State(node): St, Query(m): Q) -> Response {
    let after: u64 = q(&m, "after").parse().unwrap_or(0);
    let timeout: u64 = q(&m, "timeout").parse().unwrap_or(25).min(60);
    ok_json(&node.assistant.wait(after, std::time::Duration::from_secs(timeout)).await)
}

async fn assistant_reply(State(node): St, Json(b): Json<Value>) -> Response {
    match crate::assistant::reply(&node, b).await {
        Ok(v) => ok_json(&v),
        Err((c, e)) => err(c, e),
    }
}

async fn assistant_presence(State(node): St, Json(b): Json<Value>) -> Response {
    node.assistant.presence(&node, &b);
    ok_json(&json!({ "ok": true, "online": node.assistant.online() }))
}

/// Чат для панели: где ассистент, на связи ли, история (новее after), «печатает…». Долгий опрос — timeout.
async fn assistant_chat(State(node): St, Query(m): Q) -> Response {
    let after: u64 = q(&m, "after").parse().unwrap_or(0);
    let timeout: u64 = q(&m, "timeout").parse().unwrap_or(0).min(30);
    let typing0 = node.assistant.typing();
    let online0 = crate::assistant::host_of(&node).map(|h| h.2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout);
    loop {
        let notified = node.assistant.hist_notify.notified();
        let items = node.assistant.history_after(after);
        let host = crate::assistant::host_of(&node);
        let changed = !items.is_empty() || node.assistant.typing() != typing0 || host.as_ref().map(|h| h.2) != online0;
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if changed || left.is_zero() {
            return ok_json(&json!({
                "host": host.as_ref().map(|h| h.0.clone()), "title": host.as_ref().map(|h| h.1.clone()).unwrap_or_else(|| "Ассистент".into()),
                "online": host.as_ref().map(|h| h.2).unwrap_or(false), "isHost": node.assistant.is_host(),
                "items": items, "typing": node.assistant.typing(), "unread": node.assistant.unread(), "pending": node.assistant.pending(),
            }));
        }
        let _ = tokio::time::timeout(left.min(std::time::Duration::from_secs(5)), notified).await;
    }
}

async fn assistant_send(State(node): St, Json(b): Json<Value>) -> Response {
    match node.assistant.send(&node, &b) {
        Ok(v) => ok_json(&json!({ "ok": true, "item": v })),
        Err(e) => err(400, e),
    }
}

async fn assistant_read(State(node): St) -> Response {
    node.assistant.mark_read();
    ok_json(&json!({ "ok": true }))
}

// ───────────── Аватарки ─────────────

fn image(b: Vec<u8>) -> Response {
    let mime = crate::avatar::mime_of(&b).unwrap_or("application/octet-stream");
    let mut r = Response::new(Body::from(b));
    r.headers_mut().insert("Content-Type", HeaderValue::from_static(mime));
    r.headers_mut().insert("Cache-Control", HeaderValue::from_static("private, max-age=86400"));
    r
}

/// GET avatar[?peer] — своя картинка или соседа (картинка, 404 — нет).
async fn avatar_get(State(node): St, Query(m): Q) -> Response {
    let peer = q(&m, "peer");
    let b = if peer.is_empty() || eq_ic(peer, &node.node_name()) { crate::avatar::own(&node) } else { crate::avatar::of_peer(&node, peer).await };
    match b {
        Some(b) => image(b),
        None => err(404, "аватарки нет"),
    }
}

/// POST avatar — тело: картинка (JPEG/PNG/WebP/GIF, до 2 МБ; гифка — анимированная).
async fn avatar_set(State(node): St, body: axum::body::Bytes) -> Response {
    match crate::avatar::set(&node, &body) {
        Ok(h) => ok_json(&json!({ "ok": true, "hash": h })),
        Err(e) => err(400, e),
    }
}

async fn avatar_remove(State(node): St) -> Response {
    crate::avatar::remove(&node);
    ok_json(&json!({ "ok": true }))
}

/// Отпечатки аватарок: своя и соседей (меняется отпечаток — панель перерисует картинку).
async fn avatars(State(node): St) -> Response {
    let peers: serde_json::Map<String, Value> = node.peers.lock().values().filter(|p| !p.avatar.is_empty()).map(|p| (p.name.clone(), json!(p.avatar))).collect();
    let sounds: serde_json::Map<String, Value> = node.peers.lock().values().filter(|p| !p.sound.is_empty()).map(|p| (p.name.clone(), json!(p.sound))).collect();
    ok_json(&json!({ "me": crate::avatar::own_hash(&node), "node": node.node_name(), "peers": peers, "sound": crate::sound::own_hash(&node), "sounds": sounds, "tile": crate::tile::own_hash(&node), "tiles": node.peers.lock().values().filter(|p| !p.tile.is_empty()).map(|p| (p.name.clone(), json!(p.tile))).collect::<serde_json::Map<String, Value>>(), "silent": crate::tile::silent(&node), "status": crate::tile::own_status(&node), "ping": crate::sound::ping_hash(&node), "pings": node.peers.lock().values().filter(|p| !p.ping.is_empty()).map(|p| (p.name.clone(), json!(p.ping))).collect::<serde_json::Map<String, Value>>() }))
}

// ───────────── Плитка участника ─────────────

/// GET tile[?peer] — плитка: {style, img} (img — отпечаток картинки-фона, пусто — нет).
async fn tile_get(State(node): St, Query(m): Q) -> Response {
    let peer = q(&m, "peer");
    if peer.is_empty() || eq_ic(peer, &node.node_name()) {
        return ok_json(&crate::tile::own(&node));
    }
    ok_json(&crate::tile::of_peer(&node, peer).await.unwrap_or_else(|| json!({ "style": {}, "img": "" })))
}

/// POST tile {style} — свой стиль плитки (неизвестные ключи отбрасываются, значения — в пределах).
async fn tile_set(State(node): St, Json(v): Json<Value>) -> Response {
    let st = if v.get("style").is_some() { &v["style"] } else { &v };
    match crate::tile::set_style(&node, st) {
        Ok(s) => ok_json(&json!({ "ok": true, "style": s, "hash": crate::tile::own_hash(&node) })),
        Err(e) => err(400, e),
    }
}

/// POST tile/status {text?, color?} — свой статус: текст до 255 знаков, цвет значка (пусто — убрать).
async fn tile_status_set(State(node): St, Json(v): Json<Value>) -> Response {
    match crate::tile::set_status(&node, &v) {
        Ok(s) => ok_json(&json!({ "ok": true, "status": s, "hash": crate::tile::own_hash(&node) })),
        Err(e) => err(400, e),
    }
}

/// POST tile/status/presets {list: [{id, text, color}]} — заготовки статуса (до 20).
async fn tile_presets_set(State(node): St, Json(v): Json<Value>) -> Response {
    match crate::tile::set_presets(&node, &v) {
        Ok(r) => ok_json(&json!({ "ok": true, "presets": r, "hash": crate::tile::own_hash(&node) })),
        Err(e) => err(400, e),
    }
}

/// POST tile/status/apply {id} — выбрать заготовку (пусто — без статуса).
async fn tile_preset_apply(State(node): St, Json(v): Json<Value>) -> Response {
    match crate::tile::apply_preset(&node, v["id"].as_str().unwrap_or("")) {
        Ok(r) => ok_json(&json!({ "ok": true, "presets": r, "hash": crate::tile::own_hash(&node) })),
        Err(e) => err(400, e),
    }
}

async fn tile_preset_img_get(State(node): St, Query(m): Q) -> Response {
    match crate::tile::preset_img(&node, q(&m, "id")) {
        Some(b) => image(b),
        None => err(404, "значка нет"),
    }
}

async fn tile_preset_img_set(State(node): St, Query(m): Q, body: axum::body::Bytes) -> Response {
    match crate::tile::set_preset_img(&node, q(&m, "id"), &body) {
        Ok(h) => ok_json(&json!({ "ok": true, "img": h, "hash": crate::tile::own_hash(&node) })),
        Err(e) => err(400, e),
    }
}

async fn tile_preset_img_remove(State(node): St, Query(m): Q) -> Response {
    match crate::tile::remove_preset_img(&node, q(&m, "id")) {
        Ok(()) => ok_json(&json!({ "ok": true, "hash": crate::tile::own_hash(&node) })),
        Err(e) => err(400, e),
    }
}

/// GET tile/status/image[?peer] — значок статуса.
async fn tile_st_img_get(State(node): St, Query(m): Q) -> Response {
    let peer = q(&m, "peer");
    // v — отпечаток значка из состояния канала: может быть новее того, что мост уже знает о соседе
    let b = if peer.is_empty() || eq_ic(peer, &node.node_name()) { crate::tile::own_st_img(&node) } else { crate::tile::st_img_fresh(&node, peer, q(&m, "v")).await };
    match b {
        Some(b) => {
            let mime = crate::avatar::mime_of(&b).unwrap_or("application/octet-stream");
            let mut r = Response::new(Body::from(b));
            r.headers_mut().insert("Content-Type", HeaderValue::from_static(mime));
            r.headers_mut().insert("Cache-Control", HeaderValue::from_static("max-age=31536000, immutable"));
            r
        }
        None => err(404, "значка статуса нет"),
    }
}

/// POST tile/status/image — тело: картинка до 256 КБ (можно гифку).
async fn tile_st_img_set(State(node): St, body: axum::body::Bytes) -> Response {
    match crate::tile::set_st_img(&node, &body) {
        Ok(h) => ok_json(&json!({ "ok": true, "img": h, "hash": crate::tile::own_hash(&node) })),
        Err(e) => err(400, e),
    }
}

/// GET tile/image[?peer] — картинка-фон плитки.
async fn tile_img_get(State(node): St, Query(m): Q) -> Response {
    let peer = q(&m, "peer");
    let b = if peer.is_empty() || eq_ic(peer, &node.node_name()) { crate::tile::own_img(&node) } else { crate::tile::img_of_peer(&node, peer).await };
    match b {
        Some(b) => image(b),
        None => err(404, "картинки плитки нет"),
    }
}

/// POST tile/image — тело: картинка (JPEG/PNG/WebP/GIF, до 4 МБ; гифка — анимированная).
async fn tile_img_set(State(node): St, body: axum::body::Bytes) -> Response {
    match crate::tile::set_img(&node, &body) {
        Ok(h) => ok_json(&json!({ "ok": true, "img": h })),
        Err(e) => err(400, e),
    }
}

// ───────────── Оформление канала ─────────────

/// room — «владелец:канал».
fn split_room(r: &str) -> (String, String) {
    let (o, c) = r.split_once(':').unwrap_or(("", r));
    (o.to_string(), c.to_string())
}

/// GET chan/art?room — {icon, back, style} (отпечатки картинок; пусто — нет).
async fn chan_art_get(State(node): St, Query(m): Q) -> Response {
    let (o, c) = split_room(q(&m, "room"));
    let o = if o.is_empty() { node.node_name() } else { o };
    ok_json(&crate::chanart::of(&node, &o, &c).await.unwrap_or_else(|| json!({ "icon": "", "back": "", "style": {} })))
}

/// GET chan/img?room&what=icon|back — картинка оформления.
async fn chan_img_get(State(node): St, Query(m): Q) -> Response {
    let (o, c) = split_room(q(&m, "room"));
    let o = if o.is_empty() { node.node_name() } else { o };
    match crate::chanart::img_of(&node, &o, &c, q(&m, "what")).await {
        Some(b) => image(b),
        None => err(404, "картинки нет"),
    }
}

/// POST chan/img?id&what=icon|back (тело — картинка) — только свой канал.
async fn chan_img_set(State(node): St, Query(m): Q, body: axum::body::Bytes) -> Response {
    match crate::chanart::set_img(&node, q(&m, "id"), q(&m, "what"), &body) {
        Ok(h) => ok_json(&json!({ "ok": true, "hash": h })),
        Err(e) => err(400, e),
    }
}

async fn chan_img_remove(State(node): St, Json(v): Json<Value>) -> Response {
    match crate::chanart::remove_img(&node, v["id"].as_str().unwrap_or(""), v["what"].as_str().unwrap_or("")) {
        Ok(()) => ok_json(&json!({ "ok": true })),
        Err(e) => err(400, e),
    }
}

/// POST chan/style {id, style} — подложка: те же ключи, что у плитки (fit, x, y, zoom, mask, soft, opacity, blur, tint, tintAmt).
async fn chan_style_set(State(node): St, Json(v): Json<Value>) -> Response {
    match crate::chanart::set_style(&node, v["id"].as_str().unwrap_or(""), &v["style"]) {
        Ok(s) => ok_json(&json!({ "ok": true, "style": s })),
        Err(e) => err(400, e),
    }
}

// ───────────── Звук входа ─────────────

/// GET joinsound[?peer] — свой звук входа или соседа.
async fn joinsound_get(State(node): St, Query(m): Q) -> Response {
    let peer = q(&m, "peer");
    let b = if peer.is_empty() || eq_ic(peer, &node.node_name()) { crate::sound::own(&node) } else { crate::sound::of_peer(&node, peer).await };
    match b {
        Some(b) => {
            let mime = crate::sound::mime_of(&b).unwrap_or("application/octet-stream");
            let mut r = Response::new(Body::from(b));
            r.headers_mut().insert("Content-Type", HeaderValue::from_static(mime));
            r.headers_mut().insert("Cache-Control", HeaderValue::from_static("private, max-age=86400"));
            r
        }
        None => err(404, "звука нет"),
    }
}

/// POST joinsound — тело: звук (MP3/OGG/WAV/WebM/M4A, до 1 МБ).
async fn joinsound_set(State(node): St, body: axum::body::Bytes) -> Response {
    match crate::sound::set(&node, &body) {
        Ok(h) => ok_json(&json!({ "ok": true, "hash": h })),
        Err(e) => err(400, e),
    }
}

async fn joinsound_remove(State(node): St) -> Response {
    crate::sound::remove(&node);
    ok_json(&json!({ "ok": true }))
}

// ───────────── Свой звук пинга (слышите только вы) ─────────────

async fn pingsound_get(State(node): St, Query(m): Q) -> Response {
    let peer = q(&m, "peer");
    let b = if peer.is_empty() || eq_ic(peer, &node.node_name()) { crate::sound::ping_own(&node) } else { crate::sound::ping_of_peer(&node, peer).await };
    match b {
        Some(b) => {
            let mime = crate::sound::mime_of(&b).unwrap_or("application/octet-stream");
            let mut r = Response::new(Body::from(b));
            r.headers_mut().insert("Content-Type", HeaderValue::from_static(mime));
            r
        }
        None => err(404, "своего звука пинга нет"),
    }
}

async fn pingsound_set(State(node): St, body: axum::body::Bytes) -> Response {
    match crate::sound::ping_set(&node, &body) {
        Ok(h) => ok_json(&json!({ "ok": true, "hash": h })),
        Err(e) => err(400, e),
    }
}

async fn pingsound_remove(State(node): St) -> Response {
    crate::sound::ping_remove(&node);
    ok_json(&json!({ "ok": true }))
}

// ───────────── «Заглушили» и управление этим ПК ─────────────

/// POST self/muted {by} — панель: этот ПК заглушили (by — кто), пусто — снова слышно. Трей показывает это значком.
async fn self_muted(State(node): St, Json(v): Json<Value>) -> Response {
    crate::remote::set_muted(&node, v["by"].as_str().unwrap_or(""));
    ok_json(&json!({ "ok": true }))
}

/// POST overlay/ann {by, col, name, a, w, h} — пометка зрителя на показе: трей рисует её поверх экрана (в показ не попадает).
async fn overlay_ann(State(node): St, Json(v): Json<Value>) -> Response {
    node.emit("overlay", v);
    ok_json(&json!({ "ok": true }))
}

async fn control_state(State(node): St) -> Response {
    ok_json(&node.remote.state())
}

/// POST control/grant {peer, keyboard, w, h} — разрешить (и продлевать раз в несколько секунд) управление соседу.
async fn control_grant(State(node): St, Json(v): Json<Value>) -> Response {
    match crate::remote::grant(&node, v["peer"].as_str().unwrap_or(""), v["keyboard"].as_bool().unwrap_or(false), v["w"].as_i64().unwrap_or(0) as i32, v["h"].as_i64().unwrap_or(0) as i32) {
        Ok(()) => ok_json(&json!({ "ok": true })),
        Err(e) => err(400, e),
    }
}

async fn control_revoke(State(node): St, body: axum::body::Bytes) -> Response {
    let why = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v["why"].as_str().map(String::from)).unwrap_or_default();
    crate::remote::revoke(&node, &why);
    ok_json(&json!({ "ok": true }))
}

/// POST control/input {peer, events} — события мыши и клавиатуры от соседа (через панель этого ПК).
async fn control_input(State(node): St, Json(v): Json<Value>) -> Response {
    match crate::remote::input(&node, v["peer"].as_str().unwrap_or(""), v["events"].as_array().map(|a| a.as_slice()).unwrap_or(&[])) {
        Ok(n) => ok_json(&json!({ "ok": true, "done": n })),
        Err((c, e)) => err(c, e),
    }
}

// ───────────── Доска канала ─────────────

/// Ответ вызова доски для ассистента.
fn bres(r: Result<Value, (u16, String)>) -> Response {
    match r {
        Ok(v) => ok_json(&v),
        Err((c, e)) => err(c, e),
    }
}

/// Параметры запроса как JSON (для общих обработчиков GET и POST).
fn qv(m: &HashMap<String, String>) -> Value {
    Value::Object(m.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect())
}

/// GET board/tmp?id — PDF, который ассистент положил на доску (панель рисует его страницы).
async fn board_tmp(State(node): St, Query(m): Q) -> Response {
    match crate::board_api::tmp_pdf(&node, q(&m, "id")) {
        Some(b) => {
            let mut r = Response::new(Body::from(b));
            r.headers_mut().insert("Content-Type", HeaderValue::from_static("application/pdf"));
            r
        }
        None => err(404, "нет такого файла"),
    }
}

/// POST shots/save?kind&name (тело — PNG) — картинка от панели для ассистента (снимок показа, доска) →
/// {path}: файл в bridge-data/shots/. Хранятся последние 50.
async fn shots_save(State(node): St, Query(m): Q, body: axum::body::Bytes) -> Response {
    if !body.starts_with(b"\x89PNG") && !body.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return err(400, "нужна картинка PNG или JPEG");
    }
    if body.len() > 40 * 1024 * 1024 {
        return err(413, "картинка больше 40 МБ");
    }
    let dir = node.data_dir.join("shots");
    let _ = std::fs::create_dir_all(&dir);
    let kind: String = q(&m, "kind").chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect();
    let ext = if body.starts_with(b"\x89PNG") { "png" } else { "jpg" };
    let name = format!("{}-{}.{}", if kind.is_empty() { "shot".to_string() } else { kind }, chrono::Local::now().format("%Y%m%d-%H%M%S-%3f"), ext);
    let p = dir.join(&name);
    if let Err(e) = std::fs::write(&p, &body) {
        return err(500, e.to_string());
    }
    // не копить: оставить последние 50
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut all: Vec<_> = rd.flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
        all.sort();
        let n = all.len();
        for (_, f) in all.into_iter().take(n.saturating_sub(50)) {
            let _ = std::fs::remove_file(f);
        }
    }
    ok_json(&json!({ "ok": true, "path": p.display().to_string(), "name": name, "size": body.len() }))
}

fn board_room(node: &Node, key: &str) -> Result<Arc<crate::board::Room>, Response> {
    crate::board::room(node, key).map_err(|(c, e)| err(c, e))
}

/// GET board/state?room — снимок доски: {room, owner, enabled, edit, seq, objs}.
async fn board_state(State(node): St, Query(m): Q) -> Response {
    let r = match board_room(&node, q(&m, "room")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    match crate::board::state(&node, &r).await {
        Ok(v) => ok_json(&v),
        Err((c, e)) => err(c, e),
    }
}

/// GET board/poll?room&after&timeout — изменения после номера after (долгий опрос).
async fn board_poll(State(node): St, Query(m): Q) -> Response {
    let r = match board_room(&node, q(&m, "room")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let after = q(&m, "after").parse().unwrap_or(0);
    let timeout = q(&m, "timeout").parse().unwrap_or(25);
    ok_json(&crate::board::poll(&node, &r, after, timeout).await)
}

/// POST board/op {room, cs, ops: [...]}
async fn board_op(State(node): St, Json(v): Json<Value>) -> Response {
    let r = match board_room(&node, v["room"].as_str().unwrap_or("")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let ops = v["ops"].as_array().cloned().unwrap_or_default();
    match crate::board::op(&node, &r, v["cs"].as_str().unwrap_or(""), &ops).await {
        Ok(seq) => {
            if ops.iter().any(|o| o["t"] == "put" || o["t"] == "del") {
                crate::activity::record(&node, "board.edit", json!({}));
            }
            ok_json(&json!({ "ok": true, "seq": seq }))
        }
        Err((c, e)) => err(c, e),
    }
}

/// POST board/image?room — тело: картинка → {id}.
async fn board_image_put(State(node): St, Query(m): Q, body: axum::body::Bytes) -> Response {
    let r = match board_room(&node, q(&m, "room")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    match crate::board::put_image(&node, &r, &body).await {
        Ok(id) => ok_json(&json!({ "ok": true, "id": id })),
        Err((c, e)) => err(c, e),
    }
}

async fn board_image_get(State(node): St, Query(m): Q) -> Response {
    let r = match board_room(&node, q(&m, "room")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    match crate::board::get_image(&node, &r, q(&m, "id")).await {
        Some(b) => {
            let mut resp = image(b);
            resp.headers_mut().insert("Cache-Control", HeaderValue::from_static("private, max-age=31536000, immutable"));
            resp
        }
        None => err(404, "нет картинки"),
    }
}

async fn board_stands(State(node): St) -> Response {
    ok_json(&crate::board::stands(&node))
}

/// POST board/stand/save {room, name}
async fn board_stand_save(State(node): St, Json(v): Json<Value>) -> Response {
    let r = match board_room(&node, v["room"].as_str().unwrap_or("")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    match crate::board::stand_save(&node, &r, v["name"].as_str().unwrap_or("")).await {
        Ok(v) => ok_json(&v),
        Err((c, e)) => err(c, e),
    }
}

/// POST board/stand/load {room, id, cs} — доска канала сменится у всех.
async fn board_stand_load(State(node): St, Json(v): Json<Value>) -> Response {
    let r = match board_room(&node, v["room"].as_str().unwrap_or("")) {
        Ok(r) => r,
        Err(e) => return e,
    };
    match crate::board::stand_load(&node, &r, v["id"].as_str().unwrap_or(""), v["cs"].as_str().unwrap_or("")).await {
        Ok(seq) => ok_json(&json!({ "ok": true, "seq": seq })),
        Err((c, e)) => err(c, e),
    }
}

/// GET board/stand/file?id — файл стенда (.mostboard), чтобы переслать или сохранить.
async fn board_stand_file(State(node): St, Query(m): Q) -> Response {
    let Some(p) = crate::board::stand_path(&node, q(&m, "id")) else { return err(404, "нет такого стенда") };
    let Ok(b) = tokio::fs::read(&p).await else { return err(404, "нет такого стенда") };
    let name = serde_json::from_slice::<Value>(&b).ok().and_then(|v| v["name"].as_str().map(String::from)).unwrap_or_else(|| "доска".into());
    let mut resp = Response::new(Body::from(b));
    resp.headers_mut().insert("Content-Type", HeaderValue::from_static("application/octet-stream"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename*=UTF-8''{}.mostboard", crate::client::esc(&name))) {
        resp.headers_mut().insert("Content-Disposition", v);
    }
    resp
}

/// POST board/stand/import?name — тело: файл стенда.
async fn board_stand_import(State(node): St, Query(m): Q, body: axum::body::Bytes) -> Response {
    match crate::board::stand_import(&node, &body, q(&m, "name")) {
        Ok(v) => ok_json(&v),
        Err((c, e)) => err(c, e),
    }
}

async fn board_stand_remove(State(node): St, Json(v): Json<Value>) -> Response {
    match crate::board::stand_remove(&node, v["id"].as_str().unwrap_or("")) {
        Ok(()) => ok_json(&json!({ "ok": true })),
        Err((c, e)) => err(c, e),
    }
}

// ───────────── Стикеры ─────────────

async fn stickers_list(State(node): St) -> Response {
    ok_json(&crate::stickers::list(&node))
}

/// POST stickers — тело: картинка или гифка (до 2 МБ) → в свои.
async fn stickers_add(State(node): St, body: axum::body::Bytes) -> Response {
    match crate::stickers::add(&node, &body) {
        Ok(id) => ok_json(&json!({ "ok": true, "id": id })),
        Err(e) => err(400, e),
    }
}

/// POST stickers/remove {id, set: mine|got}
async fn stickers_remove(State(node): St, Json(v): Json<Value>) -> Response {
    match crate::stickers::remove(&node, v["set"].as_str().unwrap_or("mine"), v["id"].as_str().unwrap_or("")) {
        Ok(()) => ok_json(&json!({ "ok": true })),
        Err(e) => err(404, e),
    }
}

/// POST stickers/keep {id} — присланный → в свои.
async fn stickers_keep(State(node): St, Json(v): Json<Value>) -> Response {
    match crate::stickers::keep(&node, v["id"].as_str().unwrap_or("")) {
        Ok(()) => ok_json(&json!({ "ok": true })),
        Err(e) => err(404, e),
    }
}

/// GET sticker?id[&set] — картинка стикера.
async fn sticker_get(State(node): St, Query(m): Q) -> Response {
    let Some(p) = crate::stickers::path(&node, q(&m, "set"), q(&m, "id")) else { return err(404, "нет стикера") };
    match tokio::fs::read(&p).await {
        Ok(b) => {
            let mut r = image(b);
            r.headers_mut().insert("Cache-Control", HeaderValue::from_static("private, max-age=31536000, immutable"));
            r
        }
        Err(_) => err(404, "нет стикера"),
    }
}

// ───────────── Обновление моста ─────────────

async fn update_state(State(node): St) -> Response {
    ok_json(&node.updates.state())
}

/// POST update/offer {peers?: [имя]} — «Поделиться версией» (без списка — всем на связи).
async fn update_offer(State(node): St, body: axum::body::Bytes) -> Response {
    let v: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let only: Vec<String> = v["peers"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let r = crate::update::offer(&node, &only).await;
    if let Some(e) = r["error"].as_str() {
        return err(400, e);
    }
    ok_json(&r)
}

/// POST update/accept — скачать предложенную версию, проверить, заменить файлы и перезапуститься (идёт в фоне).
async fn update_accept(State(node): St) -> Response {
    if node.updates.state()["offer"].is_null() {
        return err(404, "предложения обновиться нет");
    }
    let n = node.clone();
    tokio::spawn(async move {
        let _ = crate::update::accept(&n).await;
    });
    ok_json(&json!({ "ok": true }))
}

async fn update_decline(State(node): St) -> Response {
    crate::update::decline(&node);
    ok_json(&json!({ "ok": true }))
}

// ───────────── Чаты ─────────────

async fn chat_list(State(node): St) -> Response {
    ok_json(&crate::chat::list(&node))
}

/// Сообщения беседы (или всех, если conv пуст) новее after; timeout — долгий опрос.
async fn chat_messages(State(node): St, Query(m): Q) -> Response {
    let conv = q(&m, "conv").to_string();
    let after: u64 = q(&m, "after").parse().unwrap_or(0);
    let timeout: u64 = q(&m, "timeout").parse().unwrap_or(0).min(30);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout);
    loop {
        let notified = node.chat.notify.notified();
        let items = node.chat.after(&conv, after);
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if !items.is_empty() || left.is_zero() {
            let n = items.iter().filter_map(|v| v["n"].as_u64()).max().unwrap_or(after);
            return ok_json(&json!({ "items": items, "n": n }));
        }
        let _ = tokio::time::timeout(left, notified).await;
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ChatSend {
    conv: String,
    to: String,
    channel: String,
    text: String,
    author: String,
    /// [{name, mime, data(base64)}] или [{path}] — файл с диска
    files: Vec<Value>,
    /// id своего (или присланного) стикера — уйдёт вложением с пометкой «стикер»
    sticker: String,
    /// ответ на сообщение: {id, from, author?, text, q?}
    reply: Value,
}

/// Написать: conv («dm:<сосед>», «ch:<владелец>:<канал>») или to (сосед) или channel (id или название канала).
async fn chat_send(State(node): St, Json(b): Json<ChatSend>) -> Response {
    let conv = if !b.conv.is_empty() {
        b.conv.clone()
    } else if !b.to.is_empty() {
        format!("dm:{}", b.to)
    } else if !b.channel.is_empty() {
        let found = crate::media::all_channels(&node).into_iter().find(|c| eq_ic(&c.id, &b.channel) || eq_ic(&c.title, &b.channel) || eq_ic(&format!("{}:{}", c.owner, c.id), &b.channel));
        match found {
            Some(c) => format!("ch:{}:{}", c.owner, c.id),
            None => return err(404, format!("нет канала «{}»", b.channel)),
        }
    } else {
        return err(400, "укажите conv, to или channel");
    };
    let mut files = b.files.clone();
    if !b.sticker.is_empty() {
        match crate::stickers::attachment(&node, &b.sticker) {
            Ok(a) => files.push(a),
            Err(e) => return err(404, e),
        }
    }
    match crate::chat::send_full(&node, &conv, &b.text, &b.author, &files, &b.reply).await {
        Ok(v) => {
            if b.author.is_empty() {
                crate::activity::record(&node, "chat.message", json!({ "where": if conv.starts_with("ch:") { "channel" } else { "dm" }, "len": b.text.chars().count(), "files": files.len() }));
            }
            ok_json(&v)
        }
        Err((c, e)) => err(c, e),
    }
}

async fn chat_read(State(node): St, Query(m): Q) -> Response {
    node.chat.mark_read(q(&m, "conv"));
    ok_json(&json!({ "ok": true }))
}

/// Вложение сообщения чата: GET chat/file?id&name (картинки показываются прямо в чате).
async fn chat_file(State(node): St, Query(m): Q) -> Response {
    let Some(p) = crate::chat::file_path(&node, q(&m, "id"), q(&m, "name")) else { return err(404, "нет файла (сообщения хранятся сутки)") };
    let Ok(b) = tokio::fs::read(&p).await else { return err(404, "нет файла") };
    let name = q(&m, "name").to_string();
    let img = crate::avatar::mime_of(&b);
    let mut r = Response::new(Body::from(b));
    let h = r.headers_mut();
    h.insert("Content-Type", HeaderValue::from_static(img.unwrap_or("application/octet-stream")));
    h.insert("Cache-Control", HeaderValue::from_static("private, max-age=86400"));
    h.insert("X-Content-Type-Options", HeaderValue::from_static("nosniff"));
    let disp = if img.is_some() && q(&m, "dl") != "1" { "inline" } else { "attachment" };
    if let Ok(v) = HeaderValue::from_str(&format!("{}; filename*=UTF-8''{}", disp, crate::client::esc(&name))) {
        h.insert("Content-Disposition", v);
    }
    r
}
