//! «Мост» — приложение: узел в том же процессе, своё окно панели (Tauri 2, WebView2) и значок у часов
//! (состояние, меню, уведомления, автозапуск). Панель общается с мостом внутри программы, без сети и ключей.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(any(windows, feature = "check-win"))]
mod overlay;
#[cfg_attr(not(windows), allow(dead_code))]
mod panel;
#[cfg(any(windows, feature = "check-win"))]
mod win;

use bridge_core::config::NodeConfig;
use bridge_node::{Node, StartOptions};
use std::path::PathBuf;
use std::sync::Arc;

/// Папка программы: рядом с exe.
pub fn base_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| PathBuf::from("."))
}

/// Настройки: bridge.json рядом с программой. Нет — создать (имя ПК, его адреса) и открыть панель на «Соседях».
pub fn load_or_create() -> Result<(NodeConfig, PathBuf, bool), String> {
    // --config <путь> — как у 1.4 (так может быть записан автозапуск); иначе bridge.json рядом с программой
    let args: Vec<String> = std::env::args().collect();
    let file = args.iter().position(|a| a.eq_ignore_ascii_case("--config")).and_then(|i| args.get(i + 1)).map(PathBuf::from).unwrap_or_else(|| base_dir().join("bridge.json"));
    if file.exists() {
        return NodeConfig::load(&file).map(|c| (c, file, false));
    }
    let raw = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    let mut name: String = raw.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    name = name.trim_matches('-').to_string();
    if !bridge_core::config::is_name(&name) {
        name = "my-pc".into();
    }
    let mut cfg = NodeConfig { node: name, ..Default::default() };
    for ip in bridge_node::link::local_ips() {
        cfg.listen.push(format!("http://{}:5390", ip));
    }
    std::fs::write(&file, bridge_core::to_json(&cfg)).map_err(|e| format!("Не удалось записать {}: {}. Положите программу в папку, куда можно писать (например C:\\Bridge).", file.display(), e))?;
    Ok((cfg, file, true))
}

pub fn start_node(rt: &tokio::runtime::Runtime) -> Result<(Arc<Node>, bool), String> {
    let (cfg, file, created) = load_or_create()?;
    let dir = file.parent().map(|p| p.to_path_buf()).unwrap_or_else(base_dir);
    let node = rt.block_on(Node::start(cfg, StartOptions { base_dir: dir, config_file: Some(file), console: false, ..Default::default() }))?;
    // ассистент через API (/local/ui) может попросить открыть панель — тем же окном
    let weak = Arc::downgrade(&node);
    *node.media.panel_opener.lock() = Some(Arc::new(move |tab: &str| {
        if let Some(n) = weak.upgrade() {
            open_panel(&n, tab);
        }
    }));
    Ok((node, created))
}

/// Открыть панель: окно приложения; не вышло (нет WebView2) — Edge или Chrome как приложение, иначе браузер.
pub fn open_panel(node: &Node, tab: &str) {
    if panel::open(tab) {
        return;
    }
    open_browser(node, tab);
}

/// Запасной путь (как до 3.0): панель как отдельное окно Edge/Chrome со своим профилем.
pub fn open_browser(node: &Node, tab: &str) {
    if node.media.has_listeners() {
        node.media.deliver(serde_json::json!({ "media": "ui", "type": "tab", "tab": tab }));
        return;
    }
    let url = format!("{}#{}", node.dashboard_url(), tab);
    #[cfg(windows)]
    {
        let env = |k: &str| std::env::var(k).unwrap_or_default();
        let candidates = [
            format!("{}\\Microsoft\\Edge\\Application\\msedge.exe", env("ProgramFiles(x86)")),
            format!("{}\\Microsoft\\Edge\\Application\\msedge.exe", env("ProgramFiles")),
            format!("{}\\Google\\Chrome\\Application\\chrome.exe", env("ProgramFiles")),
            format!("{}\\Google\\Chrome\\Application\\chrome.exe", env("ProgramFiles(x86)")),
            format!("{}\\Google\\Chrome\\Application\\chrome.exe", env("LOCALAPPDATA")),
        ];
        let profile = node.data_dir.join("panel-profile");
        for exe in candidates.iter().filter(|p| std::path::Path::new(p).is_file()) {
            let r = std::process::Command::new(exe)
                .arg(format!("--app={}", url))
                .arg(format!("--user-data-dir={}", profile.display()))
                .args(["--autoplay-policy=no-user-gesture-required", "--disable-popup-blocking", "--no-first-run", "--no-default-browser-check"])
                .spawn();
            if r.is_ok() {
                return;
            }
        }
    }
    let _ = bridge_node::http_local::open_url(&url);
}

/// Уведомление из значка у часов (щелчок откроет панель на `tab`).
pub fn notify(title: &str, text: &str, tab: &str) {
    #[cfg(windows)]
    {
        let (t, x, tab) = (title.to_string(), text.to_string(), tab.to_string());
        if let Some(a) = panel::app() {
            let _ = a.run_on_main_thread(move || win::balloon(&t, &x, &tab));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (title, text, tab);
    }
}

/// Сообщение об ошибке запуска.
pub fn fatal(text: &str) {
    #[cfg(windows)]
    win::message_box(text);
    #[cfg(not(windows))]
    eprintln!("{}", text);
}

#[cfg(windows)]
fn main() {
    win::run();
}

#[cfg(not(windows))]
fn main() {
    // Без окна приложения (Linux, macOS): узел в фоне, панель — в браузере
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
    match start_node(&rt) {
        Ok((node, created)) => {
            println!("Мост запущен: {}", node.dashboard_url());
            if created || node.cfg.read().ui.auto_open {
                open_browser(&node, if created { "peers" } else { "home" });
            }
            rt.block_on(async {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = node.shutdown.cancelled() => {},
                }
                node.stop().await;
            });
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}
