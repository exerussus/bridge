//! Окно панели — Tauri 2 (WebView2 — тот же движок, что у Edge).
//!
//! Страница и все запросы панели идут по протоколу `bridge://` прямо в мост внутри программы: никакой сети и
//! никакого ключа в странице (в браузере, как раньше, — 127.0.0.1 с локальным ключом). Сигналы для панели (звонки,
//! соединения в канале, команды ассистента) приходят событием окна `bridge-media` вместо потока SSE.
//!
//! Крестик прячет окно к часам: разговор, показы и доска продолжаются. Выйти — меню значка у часов.

use bridge_node::Node;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::webview::{NewWindowResponse, PermissionResponse};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tower::ServiceExt;

static APP: OnceLock<AppHandle> = OnceLock::new();
static NODE: OnceLock<Arc<Node>> = OnceLock::new();
static POPS: AtomicUsize = AtomicUsize::new(0);
/// Окно нельзя создать (нет WebView2) — открывать панель в браузере.
static BROKEN: AtomicBool = AtomicBool::new(false);
/// Иконка и заголовок окна сейчас «тревожные» (заглушили, управляют) — чтобы не дёргать окно каждые 2 с.
static ALERT: AtomicUsize = AtomicUsize::new(usize::MAX);

const MAIN: &str = "main";
/// Флаги движка окна: звук без нажатий (звонок, пинг), без засыпания в фоне (разговор и доска живут в свёрнутом окне).
const BROWSER_ARGS: &str = "--autoplay-policy=no-user-gesture-required --disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows";

pub fn app() -> Option<&'static AppHandle> {
    APP.get()
}

/// Запустить окно приложения (цикл сообщений Windows — его же получает и значок у часов). Возвращается при выходе.
pub fn run(node: Arc<Node>, rt: tokio::runtime::Handle, on_ready: impl FnOnce() + Send + 'static) {
    let _ = NODE.set(node.clone());
    let n2 = node.clone();
    let builder = tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("bridge", move |_ctx, req, responder| {
            let node = n2.clone();
            rt.spawn(async move {
                responder.respond(dispatch(node, req).await);
            });
        })
        .setup(move |app| {
            let _ = APP.set(app.handle().clone());
            // сигналы панели: из моста — событием в окно
            let mut rx = node.media.tx.subscribe();
            let h = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(v) => {
                            let _ = h.emit_to(MAIN, "bridge-media", v);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            });
            on_ready();
            Ok(())
        });
    let app = match builder.build(tauri::generate_context!()) {
        Ok(a) => a,
        Err(e) => {
            BROKEN.store(true, Ordering::SeqCst);
            crate::fatal(&format!("Окно приложения не запустилось: {}\n\nПанель откроется в браузере.", e));
            return;
        }
    };
    app.run(|_app, ev| {
        if let tauri::RunEvent::ExitRequested { api, code, .. } = ev {
            // окна закрыли (а не «Выход» в меню) — программа живёт у часов
            if code.is_none() {
                api.prevent_exit();
            }
        }
    });
}

/// Завершить программу (после того как узел остановлен).
pub fn quit() {
    if let Some(a) = APP.get() {
        a.exit(0);
    }
}

/// Показать окно панели на вкладке `tab` (создать, если ещё нет). false — окна нет и не будет (тогда — браузер).
pub fn open(tab: &str) -> bool {
    let Some(app) = APP.get().cloned() else { return false };
    if BROKEN.load(Ordering::SeqCst) {
        return false;
    }
    let tab = tab.to_string();
    let a2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = a2.get_webview_window(MAIN) {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
            if let Some(n) = NODE.get() {
                n.media.deliver(json!({ "media": "ui", "type": "tab", "tab": tab }));
            }
            return;
        }
        if let Err(e) = create(&a2, &tab) {
            BROKEN.store(true, Ordering::SeqCst);
            if let Some(n) = NODE.get() {
                n.info(&format!("Окно панели не создалось ({}) — открываю панель в браузере", e));
                crate::open_browser(n, &tab);
            }
        }
    });
    true
}

/// Окно есть и видно.
pub fn shown() -> bool {
    APP.get().and_then(|a| a.get_webview_window(MAIN)).map(|w| w.is_visible().unwrap_or(false)).unwrap_or(false)
}

fn window_file(node: &Node) -> std::path::PathBuf {
    node.data_dir.join("window.json")
}

fn create(app: &AppHandle, tab: &str) -> tauri::Result<WebviewWindow> {
    let node = NODE.get().cloned().expect("узел");
    let url = WebviewUrl::CustomProtocol(format!("bridge://localhost/#{}", tab).parse().expect("адрес"));
    let saved: Value = std::fs::read_to_string(window_file(&node)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let a2 = app.clone();
    let mut b = WebviewWindowBuilder::new(app, MAIN, url)
        .title(format!("Мост · {}", node.node_name()))
        .inner_size(saved["w"].as_f64().unwrap_or(1440.0).max(980.0), saved["h"].as_f64().unwrap_or(900.0).max(640.0))
        .min_inner_size(980.0, 640.0)
        .additional_browser_args(BROWSER_ARGS)
        .background_throttling(BackgroundThrottlingPolicy::Disabled)
        // перетаскивание файлов в панель (отправка, папки, доска, чат) — обычное, как в браузере
        .disable_drag_drop_handler()
        .devtools(cfg!(debug_assertions) || std::env::var("BRIDGE_DEVTOOLS").is_ok())
        // микрофон, камера и показ экрана — только этой панели (другой страницы в окне быть не может)
        .on_permission_request(|_, _| PermissionResponse::Allow)
        .on_navigation(|u| {
            let ok = is_ours(u);
            if !ok {
                let _ = bridge_node::http_local::open_url(u.as_str());
            }
            ok
        })
        .on_new_window(move |u, features| {
            // показ в отдельном окне — окно моста; ссылки наружу — в обычном браузере
            if is_ours(&u) {
                let label = format!("pop-{}", POPS.fetch_add(1, Ordering::SeqCst));
                match WebviewWindowBuilder::new(&a2, label, WebviewUrl::External("about:blank".parse().unwrap()))
                    .window_features(features)
                    .title("Показ — Мост")
                    .on_permission_request(|_, _| PermissionResponse::Allow)
                    .build()
                {
                    Ok(window) => return NewWindowResponse::Create { window },
                    Err(_) => return NewWindowResponse::Deny,
                }
            }
            let _ = bridge_node::http_local::open_url(u.as_str());
            NewWindowResponse::Deny
        });
    if let (Some(x), Some(y)) = (saved["x"].as_f64(), saved["y"].as_f64()) {
        b = b.position(x, y);
    } else {
        b = b.center();
    }
    if let Ok(img) = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png")) {
        b = b.icon(img.to_owned())?;
    }
    let w = b.build()?;
    if saved["max"].as_bool() == Some(true) {
        let _ = w.maximize();
    }
    node.media.listeners.fetch_add(1, Ordering::SeqCst);
    let w2 = w.clone();
    w.on_window_event(move |ev| match ev {
        WindowEvent::CloseRequested { api, .. } => {
            // крестик — к часам: разговор и показы продолжаются
            api.prevent_close();
            save_place(&w2);
            let _ = w2.hide();
        }
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => save_place_later(&w2),
        _ => {}
    });
    Ok(w)
}

fn is_ours(u: &tauri::Url) -> bool {
    matches!(u.scheme(), "bridge") || u.host_str() == Some("bridge.localhost") || u.as_str() == "about:blank"
}

static SAVE_PENDING: AtomicBool = AtomicBool::new(false);
fn save_place_later(w: &WebviewWindow) {
    if SAVE_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let w = w.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        SAVE_PENDING.store(false, Ordering::SeqCst);
        save_place(&w);
    });
}

fn save_place(w: &WebviewWindow) {
    let Some(node) = NODE.get() else { return };
    if w.is_minimized().unwrap_or(false) || !w.is_visible().unwrap_or(false) {
        return;
    }
    let max = w.is_maximized().unwrap_or(false);
    let k = w.scale_factor().unwrap_or(1.0);
    let mut v = std::fs::read_to_string(window_file(node)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(json!({}));
    v["max"] = json!(max);
    if !max {
        if let (Ok(p), Ok(s)) = (w.outer_position(), w.inner_size()) {
            v["x"] = json!((p.x as f64 / k).round());
            v["y"] = json!((p.y as f64 / k).round());
            v["w"] = json!((s.width as f64 / k).round());
            v["h"] = json!((s.height as f64 / k).round());
        }
    }
    let _ = std::fs::write(window_file(node), v.to_string());
}

/// Иконка и мигание окна, когда вас заглушили (красная) или ПК управляют (янтарная). rgba — 32×32.
pub fn alert(kind: usize, rgba: Option<Vec<u8>>) {
    if ALERT.swap(kind, Ordering::SeqCst) == kind {
        return;
    }
    let Some(app) = APP.get().cloned() else { return };
    let _ = app.clone().run_on_main_thread(move || {
        let Some(w) = app.get_webview_window(MAIN) else { return };
        let img = match &rgba {
            Some(px) => Some(tauri::image::Image::new_owned(px.clone(), 32, 32)),
            None => tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png")).ok().map(|i| i.to_owned()),
        };
        if let Some(i) = img {
            let _ = w.set_icon(i);
        }
        if rgba.is_some() {
            let _ = w.request_user_attention(Some(tauri::UserAttentionType::Critical));
        }
    });
}

/// Запрос панели → мост (тот же маршрутизатор, что у локального API; ключ подставляет программа).
async fn dispatch(node: Arc<Node>, req: tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    let path = req.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| "/".into());
    // команды самому окну
    if let Some(cmd) = path.strip_prefix("/__app/") {
        let body: Value = serde_json::from_slice(req.body()).unwrap_or(Value::Null);
        app_cmd(&node, cmd.split('?').next().unwrap_or(""), &body);
        return tauri::http::Response::builder().status(200).header("Content-Type", "application/json").body(b"{\"ok\":true}".to_vec()).unwrap();
    }
    let mut b = axum::http::Request::builder().method(req.method().clone()).uri(&path);
    for (k, v) in req.headers() {
        let name = k.as_str();
        if name == "host" || name == "origin" || name == "referer" || name == bridge_core::LOCAL_TOKEN_HEADER.to_ascii_lowercase() {
            continue;
        }
        b = b.header(k, v);
    }
    b = b.header("host", "127.0.0.1").header(bridge_core::LOCAL_TOKEN_HEADER, node.local_token.as_str());
    let mut r = match b.body(axum::body::Body::from(req.into_body())) {
        Ok(r) => r,
        Err(e) => return text(400, &e.to_string()),
    };
    r.extensions_mut().insert(axum::extract::ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    r.extensions_mut().insert(bridge_node::http_local::FromApp);
    let app = bridge_node::http_peer::router(node.clone());
    let resp = match app.oneshot(r).await {
        Ok(x) => x,
        Err(e) => match e {},
    };
    let (parts, body) = resp.into_parts();
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b.to_vec(),
        Err(e) => return text(500, &e.to_string()),
    };
    let mut out = tauri::http::Response::builder().status(parts.status);
    for (k, v) in parts.headers.iter() {
        out = out.header(k, v);
    }
    out.body(bytes).unwrap_or_else(|_| text(500, "ответ"))
}

fn text(code: u16, t: &str) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder().status(code).header("Content-Type", "text/plain; charset=utf-8").body(t.as_bytes().to_vec()).unwrap()
}

fn app_cmd(node: &Arc<Node>, cmd: &str, body: &Value) {
    match cmd {
        "title" => {
            let t = body["title"].as_str().unwrap_or("").chars().take(120).collect::<String>();
            if let Some(w) = APP.get().and_then(|a| a.get_webview_window(MAIN)) {
                let _ = w.set_title(&if t.is_empty() { format!("Мост · {}", node.node_name()) } else { t });
            }
        }
        "notify" => {
            let (t, x, tab) = (body["title"].as_str().unwrap_or("Мост").to_string(), body["body"].as_str().unwrap_or("").to_string(), body["tab"].as_str().unwrap_or("home").to_string());
            crate::notify(&t, &x, &tab);
        }
        _ => {}
    }
}
