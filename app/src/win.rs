#![cfg_attr(not(windows), allow(dead_code))]
//! Значок у часов на чистом Win32 (windows-sys): точка состояния, всплывающие уведомления, меню, автозапуск
//! (HKCU\…\Run), один экземпляр. Сообщения окна значка разбирает тот же цикл, что у окна приложения (Tauri).
//! Пока ПК управляют по сети — клавиша Pause (настоящая, не присланная) сразу останавливает управление.

#![allow(clippy::missing_safety_doc)]

use crate::{open_panel, panel, start_node};
use bridge_node::Node;
use parking_lot::Mutex;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const WM_EVENTS: u32 = WM_APP + 2;
/// Мост обновился по сети — закрыться и запустить новые файлы.
const WM_RESTART: u32 = WM_APP + 3;
/// Второй запуск программы просит показать окно (оно может быть спрятано к часам).
const WM_SHOWPANEL: u32 = WM_APP + 4;
/// Нажата настоящая клавиша Pause, пока ПК управляют, — остановить управление.
const WM_PANIC: u32 = WM_APP + 5;
const TRAY_CLASS: &str = "ExerussusBridgeTray";
/// Путь к программе на момент запуска (после обновления сам файл уже переименован в *.old).
static EXE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
const TIMER_ID: usize = 1;
const NIN_KEYSELECT_: u32 = WM_USER + 1;

const CMD_OPEN: usize = 100;
const CMD_VOICE: usize = 101;
const CMD_SEND: usize = 103;
const CMD_SYNC: usize = 104;
const CMD_FOLDER: usize = 105;
const CMD_INCOMING: usize = 106;
const CMD_AUTOSTART: usize = 110;
const CMD_NOTIFY: usize = 111;
const CMD_AUTOCALL: usize = 112;
const CMD_EXIT: usize = 199;
const CMD_STOPCTL: usize = 113;
const OV_TIMER: usize = 2;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "Exerussus.Bridge";

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn fill(dst: &mut [u16], s: &str) {
    let v: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..v.len()].copy_from_slice(&v);
    dst[v.len()] = 0;
}

/// Настройки самого приложения (bridge-data/app.json).
#[derive(Clone)]
struct AppSettings {
    notify: bool,
    auto_open_calls: bool,
}

impl AppSettings {
    fn file(node: &Node) -> std::path::PathBuf {
        node.data_dir.join("app.json")
    }
    fn load(node: &Node) -> Self {
        let v: Value = std::fs::read_to_string(Self::file(node)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        AppSettings { notify: v["notify"].as_bool().unwrap_or(true), auto_open_calls: v["autoOpenCalls"].as_bool().unwrap_or(true) }
    }
    fn save(&self, node: &Node) {
        let _ = std::fs::write(Self::file(node), serde_json::json!({ "notify": self.notify, "autoOpenCalls": self.auto_open_calls }).to_string());
    }
}

struct App {
    node: Arc<Node>,
    rt: tokio::runtime::Runtime,
    hwnd: HWND,
    icons: Vec<HICON>,
    state: usize,
    tip: String,
    queue: Arc<Mutex<VecDeque<Value>>>,
    taskbar_created: u32,
    click_tab: String,
    settings: AppSettings,
    attention: usize,
    received: Vec<String>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

pub fn message_box(text: &str) {
    let (t, c) = (w(text), w("Мост"));
    unsafe {
        MessageBoxW(null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONWARNING);
    }
}

/// Найти уже открытое окно панели этого узла (заголовок «… Мост · <узел>») и вывести его вперёд.
pub fn focus_panel(node: &str) -> bool {
    struct Find {
        needle: Vec<u16>,
        found: HWND,
    }
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let f = &mut *(l as *mut Find);
        if IsWindowVisible(h) == 0 {
            return 1;
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        if n > 0 && buf[..n as usize].windows(f.needle.len()).any(|w| w == f.needle.as_slice()) {
            f.found = h;
            return 0;
        }
        1
    }
    let mut f = Find { needle: format!("Мост · {}", node).encode_utf16().collect(), found: null_mut() };
    unsafe {
        EnumWindows(Some(each), &mut f as *mut Find as LPARAM);
        if f.found.is_null() {
            return false;
        }
        if IsIconic(f.found) != 0 {
            ShowWindow(f.found, SW_RESTORE);
        }
        SetForegroundWindow(f.found);
    }
    true
}

pub fn run() {
    if let Ok(e) = std::env::current_exe() {
        let _ = EXE.set(e);
    }
    let after_update = std::env::args().any(|a| a == "--after-update");
    // Один экземпляр: второй запуск открывает панель первого
    let mutex_name = w("Local\\Exerussus.Bridge.App");
    unsafe {
        let mut h = CreateMutexW(null(), 0, mutex_name.as_ptr());
        // после обновления старый экземпляр ещё закрывается — подождать его (до 30 с)
        if after_update {
            let t0 = std::time::Instant::now();
            while !h.is_null() && GetLastError() == ERROR_ALREADY_EXISTS && t0.elapsed().as_secs() < 30 {
                CloseHandle(h);
                std::thread::sleep(std::time::Duration::from_millis(300));
                h = CreateMutexW(null(), 0, mutex_name.as_ptr());
            }
        }
        if !h.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
            // первый экземпляр жив — пусть покажет своё окно (даже спрятанное к часам)
            let class = w(TRAY_CLASS);
            let tray = FindWindowExW(HWND_MESSAGE, null_mut(), class.as_ptr(), null());
            if !tray.is_null() {
                PostMessageW(tray, WM_SHOWPANEL, 0, 0);
                return;
            }
            if let Ok((cfg, _, _)) = crate::load_or_create() {
                if focus_panel(&cfg.node) {
                    return;
                }
                let url = cfg.listen.iter().find(|l| l.contains("127.0.0.1")).cloned().unwrap_or_else(|| "http://127.0.0.1:5390".into());
                let _ = bridge_node::http_local::open_url(&format!("{}/", url.trim_end_matches('/')));
            }
            return;
        }
    }
    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("bridge").build() {
        Ok(r) => r,
        Err(e) => return message_box(&format!("Не удалось запустить: {}", e)),
    };
    // после обновления порты могут освобождаться не сразу — несколько попыток
    let mut tries = 0;
    let (node, created) = loop {
        match start_node(&rt) {
            Ok(x) => break x,
            Err(e) if after_update && tries < 20 => {
                tries += 1;
                let _ = e;
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(e) => return message_box(&format!("Мост не запустился:\n\n{}", e)),
        }
    };
    let silent = std::env::args().any(|a| a == "--tray");
    unsafe { ui_loop(node, rt, created, silent) };
}

unsafe fn ui_loop(node: Arc<Node>, rt: tokio::runtime::Runtime, created: bool, silent: bool) {
    let hinst = GetModuleHandleW(null());
    let class = w(TRAY_CLASS);
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: null_mut(),
        hCursor: null_mut(),
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class.as_ptr(),
    };
    RegisterClassW(&wc);
    let title = w("Мост");
    let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, null_mut(), hinst, null());
    if hwnd.is_null() {
        return message_box("Не удалось создать окно трея");
    }
    let tc = w("TaskbarCreated");
    let taskbar_created = RegisterWindowMessageW(tc.as_ptr());
    // Значки: серый (нет соседей), зелёный, жёлтый, красный; с отметкой «ждёт вас»
    let colors = [0xFF8F8CC0u32, 0xFF4BFFB0, 0xFFFFC24B, 0xFFFF4D6D];
    let mut icons = Vec::new();
    for badge in [false, true] {
        for c in colors {
            icons.push(make_icon(c, badge));
        }
    }
    icons.push(make_alert_icon(false)); // 8 — заглушили
    icons.push(make_alert_icon(true)); // 9 — управляют
    let queue = Arc::new(Mutex::new(VecDeque::new()));
    // События узла → очередь окна
    {
        let mut rx = node.events_tx.subscribe();
        let q = queue.clone();
        let hwnd_val = hwnd as isize;
        rt.spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(v) => {
                        q.lock().push_back(v);
                        PostMessageW(hwnd_val as HWND, WM_EVENTS, 0, 0);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }
    // обновление по сети: узел просит перезапуск — окно закроет мост и запустит новые файлы
    {
        let hv = hwnd as isize;
        *node.updates.restart_hook.lock() = Some(Arc::new(move || {
            PostMessageW(hv as HWND, WM_RESTART, 0, 0);
        }));
    }
    let settings = AppSettings::load(&node);
    let handle = rt.handle().clone();
    APP.with(|a| {
        *a.borrow_mut() = Some(App {
            node: node.clone(),
            rt,
            hwnd,
            icons,
            state: 0,
            tip: String::new(),
            queue,
            taskbar_created,
            click_tab: "home".into(),
            settings,
            attention: 0,
            received: Vec::new(),
        })
    });
    add_icon(hwnd);
    refresh();
    SetTimer(hwnd, TIMER_ID, 2000, None);
    let n2 = node.clone();
    let hv = hwnd as isize;
    let ready = move || {
        if created {
            open_panel(&n2, "peers");
            let _ = n2;
        } else if !silent && n2.cfg.read().ui.auto_open {
            open_panel(&n2, "home");
        }
        if created {
            unsafe { PostMessageW(hv as HWND, WM_APP + 6, 0, 0) };
        }
    };
    // окно приложения (Tauri): его цикл сообщений обслуживает и значок у часов
    panel::run(node.clone(), handle, ready);
    if APP.with(|a| a.borrow().is_some()) {
        // окно не запустилось (нет WebView2) — свой цикл сообщений, панель — в браузере
        if created {
            open_panel(&node, "peers");
        } else if !silent && node.cfg.read().ui.auto_open {
            open_panel(&node, "home");
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn add_icon(hwnd: HWND) {
    let mut nid = nid(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    APP.with(|a| {
        if let Some(app) = a.borrow().as_ref() {
            nid.hIcon = app.icons[app.state];
        }
    });
    fill(&mut nid.szTip, "Мост");
    Shell_NotifyIconW(NIM_ADD, &nid);
    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    Shell_NotifyIconW(NIM_SETVERSION, &nid);
}

unsafe fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut n: NOTIFYICONDATAW = std::mem::zeroed();
    n.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    n.hWnd = hwnd;
    n.uID = 1;
    n
}

/// Всплывающее уведомление; щелчок по нему откроет панель на `tab`.
pub fn balloon(title: &str, text: &str, tab: &str) {
    APP.with(|a| {
        let mut b = a.borrow_mut();
        let Some(app) = b.as_mut() else { return };
        if !app.settings.notify {
            return;
        }
        app.click_tab = tab.into();
        unsafe {
            let mut n = nid(app.hwnd);
            n.uFlags = NIF_INFO;
            fill(&mut n.szInfoTitle, title);
            fill(&mut n.szInfo, text);
            n.dwInfoFlags = NIIF_INFO;
            Shell_NotifyIconW(NIM_MODIFY, &n);
        }
    });
}

/// Обновить значок и подсказку по состоянию узла.
fn refresh() {
    let mut notes: Vec<(String, String, String)> = Vec::new();
    APP.with(|a| {
        let mut b = a.borrow_mut();
        let Some(app) = b.as_mut() else { return };
        let node = &app.node;
        let cfg = node.cfg.read().clone();
        let (mut total, mut online) = (0, 0);
        for p in cfg.peers.iter().filter(|p| p.enabled && p.has_url()) {
            total += 1;
            if node.peer_state(&p.name).online {
                online += 1;
            }
        }
        let waiting = node.runner.waiting_count();
        let attention = bridge_node::http_local::attention_count(node);
        if attention > app.attention {
            notes.push(("Для вас новые записи".into(), format!("Ждут вашего действия: {}", attention), "records".into()));
        }
        app.attention = attention;
        let color = if total == 0 { 0 } else if online == total { 1 } else if online == 0 { 3 } else { 2 };
        let unread = node.assistant.unread();
        let muted = node.remote.muted_by();
        let control = node.remote.active();
        let state = if control.is_some() { 9 } else if !muted.is_empty() { 8 } else { color + if waiting + attention + unread as usize > 0 { 4 } else { 0 } };
        let status = if total == 0 {
            "соседей нет".to_string()
        } else if online == total {
            "все на связи".to_string()
        } else if online == 0 {
            "нет связи с соседями".to_string()
        } else {
            format!("на связи {} из {}", online, total)
        };
        let mut tip = format!("Мост · {}\n{}", cfg.node, status);
        if let Some(g) = &control {
            tip = format!("ВАШИМ ПК УПРАВЛЯЕТ {}\n{} · меню: «Остановить управление»", g.peer, if g.keyboard { "мышь и клавиатура" } else { "мышь" });
        } else if !muted.is_empty() {
            tip = format!("ВАС ЗАГЛУШИЛ {}\nВключить микрофон — кнопка «Микрофон» в панели", muted);
        }
        if waiting > 0 {
            tip.push_str(&format!("\nждут подтверждения: {}", waiting));
        }
        if attention > 0 {
            tip.push_str(&format!("\nзаписей для вас: {}", attention));
        }
        if unread > 0 {
            tip.push_str(&format!("\nассистент: новых {}", unread));
        }
        // окно панели: красная (заглушили) или янтарная (управляют) иконка в панели задач
        panel::alert(state.max(7) - 7, match state { 8 => Some(rgba(&alert_px(false))), 9 => Some(rgba(&alert_px(true))), _ => None });
        set_panic_hook(control.is_some(), app.hwnd);
        if state != app.state || tip != app.tip {
            app.state = state;
            app.tip = tip.clone();
            unsafe {
                let mut n = nid(app.hwnd);
                n.uFlags = NIF_ICON | NIF_TIP;
                n.hIcon = app.icons[state];
                fill(&mut n.szTip, &tip);
                Shell_NotifyIconW(NIM_MODIFY, &n);
            }
        }
        if !app.received.is_empty() {
            let r = std::mem::take(&mut app.received);
            let text = if r.len() == 1 { r[0].clone() } else { format!("{} и ещё {}", r[0], r.len() - 1) };
            notes.push(("Получены файлы".into(), text, "send".into()));
        }
    });
    for (t, x, tab) in notes {
        balloon(&t, &x, &tab);
    }
}

/// События узла: звонок, показ экрана, команды, принятые файлы.
fn drain_events() {
    let (events, node, settings) = APP.with(|a| {
        let b = a.borrow();
        let app = b.as_ref().unwrap();
        let ev: Vec<Value> = app.queue.lock().drain(..).collect();
        (ev, app.node.clone(), app.settings.clone())
    });
    for e in events {
        let d = &e["data"];
        match e["kind"].as_str().unwrap_or("") {
            "media-invite" => {
                let screen = d["media"] == "screen";
                let who = d["title"].as_str().unwrap_or("сосед").to_string();
                let tab = "voice";
                // Панель не открыта — входящий звонок некому показать: открыть её
                if settings.auto_open_calls && !panel::shown() {
                    open_panel(&node, tab);
                }
                balloon(if screen { "Экран" } else { "Звонок" }, &format!("{} {}", who, if screen { "просит показать экран" } else { "звонит" }), tab);
            }
            "agent" => {
                let title = d["title"].as_str().unwrap_or("").to_string();
                let peer = d["peer"].as_str().unwrap_or("").to_string();
                match (d["role"].as_str().unwrap_or(""), d["status"].as_str().unwrap_or("")) {
                    ("in", "queued") if d["mode"] == "manual" => balloon("Поручение для Claude", &format!("от {}: {} — откройте сессию Claude и скажите «возьми задачу из моста»", peer, title), "agent"),
                    ("in", "queued") => balloon("Агент работает", &format!("поручение от {}: {}", peer, title), "agent"),
                    ("out", "idle") => balloon("Агент: готово", &format!("{}: {}", peer, title), "agent"),
                    ("out", "failed") => balloon("Агент: не получилось", &format!("{}: {}", peer, title), "agent"),
                    _ => {}
                }
            }
            "command-waiting" => {
                balloon("Ждёт подтверждения", &format!("{} просит «{}»", d["from"].as_str().unwrap_or(""), d["runner"].as_str().unwrap_or("")), "home");
            }
            "mailbox-received" => {
                APP.with(|a| {
                    if let Some(app) = a.borrow_mut().as_mut() {
                        app.received.push(format!("{} от {}", d["path"].as_str().unwrap_or(""), d["peer"].as_str().unwrap_or("")));
                    }
                });
            }
            "chat-msg" => {
                // личное сообщение, а панель закрыта — всплывашка (щелчок откроет «Чаты»)
                if d["dm"].as_bool() == Some(true) && !node.media.has_listeners() {
                    let from = d["from"].as_str().unwrap_or("").to_string();
                    let title = node.cfg.read().peer(&from).map(|p| if p.title.is_empty() { p.name.clone() } else { p.title.clone() }).unwrap_or(from);
                    let author = d["author"].as_str().filter(|a| !a.is_empty()).map(|a| format!(" · {}", a)).unwrap_or_default();
                    balloon(&format!("{}{}", title, author), d["text"].as_str().unwrap_or(""), "chat");
                }
            }
            "assistant-msg" => {
                let text = d["text"].as_str().unwrap_or("").to_string();
                if d["alarm"].as_bool() == Some(true) {
                    if !panel::shown() {
                        open_panel(&node, "assistant");
                    }
                    balloon("Будильник", &text, "assistant");
                } else if d["notice"].as_bool() == Some(true) || !node.media.has_listeners() {
                    balloon(&node.assistant.title(), &text, "assistant");
                }
            }
            "self-muted" => {
                if d["on"] == true {
                    balloon("Вас заглушили", &format!("{} заглушил ваш микрофон — у вас, видимо, шумно. Включить его можете только вы: кнопка «Микрофон» в панели", d["by"].as_str().unwrap_or("")), "voice");
                }
            }
            "control" => {
                if d["on"] == true {
                    balloon("Вашим ПК управляют", &format!("{}: {}. Остановить — в меню этого значка или красной полосой в панели", d["peer"].as_str().unwrap_or(""), if d["keyboard"] == true { "мышь и клавиатура" } else { "мышь" }), "voice");
                }
            }
            "overlay" => {
                let hwnd = APP.with(|a| a.borrow().as_ref().map(|x| x.hwnd as isize)).unwrap_or(0);
                unsafe { crate::overlay::push(d, hwnd as HWND, OV_TIMER) };
            }
            "update-offer" => balloon("Обновление моста", &format!("«{}» предлагает версию {} — откройте панель, чтобы обновить", d["from"].as_str().unwrap_or(""), d["version"].as_str().unwrap_or("")), "home"),
            "update-done" => balloon("Мост обновлён", &format!("Теперь версия {}", d["version"].as_str().unwrap_or("")), "home"),
            "peer-version" => balloon("Сосед обновился", &format!("«{}» теперь на версии {}", d["peer"].as_str().unwrap_or(""), d["version"].as_str().unwrap_or("")), "peers"),
            "device-paired" => balloon("Телефон подключён", &format!("«{}» теперь работает с этим ПК", d["name"].as_str().unwrap_or("")), "phone"),
            "peer-named" => balloon("Сосед подключён", &format!("«{}» на связи", d["name"].as_str().unwrap_or("")), "peers"),
            _ => {}
        }
    }
    refresh();
}

fn autostart_enabled() -> bool {
    unsafe {
        let mut key: HKEY = null_mut();
        let path = w(RUN_KEY);
        if RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) != ERROR_SUCCESS {
            return false;
        }
        let name = w(RUN_VALUE);
        let r = RegQueryValueExW(key, name.as_ptr(), null(), null_mut(), null_mut(), null_mut());
        RegCloseKey(key);
        r == ERROR_SUCCESS
    }
}

fn set_autostart(on: bool) {
    unsafe {
        let mut key: HKEY = null_mut();
        let path = w(RUN_KEY);
        if RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_SET_VALUE, &mut key) != ERROR_SUCCESS {
            return;
        }
        let name = w(RUN_VALUE);
        if on {
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let args: Vec<String> = std::env::args().collect();
            let cfg = args.iter().position(|a| a.eq_ignore_ascii_case("--config")).and_then(|i| args.get(i + 1)).map(|c| format!(" --config \"{}\"", c)).unwrap_or_default();
            let val = w(&format!("\"{}\" --tray{}", exe, cfg));
            RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, val.as_ptr() as *const u8, (val.len() * 2) as u32);
        } else {
            RegDeleteValueW(key, name.as_ptr());
        }
        RegCloseKey(key);
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let menu = CreatePopupMenu();
    let item = |id: usize, text: &str, flags: u32| {
        let t = w(text);
        AppendMenuW(menu, MF_STRING | flags, id, t.as_ptr());
    };
    let (settings, name) = APP.with(|a| {
        let b = a.borrow();
        let app = b.as_ref().unwrap();
        (app.settings.clone(), app.node.node_name())
    });
    item(0, &format!("Мост · {}", name), MF_GRAYED);
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    let ctl = APP.with(|a| a.borrow().as_ref().and_then(|x| x.node.remote.active()));
    if let Some(g) = ctl {
        item(CMD_STOPCTL, &format!("⛔ Остановить управление ПК ({})", g.peer), 0);
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
    }
    item(CMD_OPEN, "Открыть панель", 0);
    item(CMD_VOICE, "Каналы (голос и экран)", 0);
    item(CMD_SEND, "Отправить файлы", 0);
    item(CMD_SYNC, "Обменяться сейчас", 0);
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    item(CMD_FOLDER, "Папка моста", 0);
    item(CMD_INCOMING, "Принятые файлы", 0);
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    item(CMD_AUTOSTART, "Запускать вместе с Windows", if autostart_enabled() { MF_CHECKED } else { 0 });
    item(CMD_NOTIFY, "Уведомления", if settings.notify { MF_CHECKED } else { 0 });
    item(CMD_AUTOCALL, "Открывать панель при звонке", if settings.auto_open_calls { MF_CHECKED } else { 0 });
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    item(CMD_EXIT, "Выход", 0);
    let mut pt: POINT = std::mem::zeroed();
    GetCursorPos(&mut pt);
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd, null());
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);
    if cmd > 0 {
        command(hwnd, cmd as usize);
    }
}

unsafe fn command(hwnd: HWND, cmd: usize) {
    let node = APP.with(|a| a.borrow().as_ref().map(|x| x.node.clone()));
    let Some(node) = node else { return };
    match cmd {
        CMD_OPEN => open_panel(&node, "home"),
        CMD_VOICE => open_panel(&node, "voice"),
        CMD_SEND => open_panel(&node, "send"),
        CMD_SYNC => node.request_sync(),
        CMD_FOLDER => {
            let _ = bridge_node::http_local::open_in_shell(&node.base_dir);
        }
        CMD_INCOMING => {
            let d = node.mailbox.in_root(&node);
            let _ = std::fs::create_dir_all(&d);
            let _ = bridge_node::http_local::open_in_shell(&d);
        }
        CMD_AUTOSTART => set_autostart(!autostart_enabled()),
        CMD_NOTIFY | CMD_AUTOCALL => APP.with(|a| {
            if let Some(app) = a.borrow_mut().as_mut() {
                if cmd == CMD_NOTIFY {
                    app.settings.notify = !app.settings.notify;
                } else {
                    app.settings.auto_open_calls = !app.settings.auto_open_calls;
                }
                app.settings.save(&app.node);
            }
        }),
        CMD_STOPCTL => {
            bridge_node::remote::revoke(&node, "остановлено в трее");
            refresh();
        }
        CMD_EXIT => {
            let n = nid(hwnd);
            Shell_NotifyIconW(NIM_DELETE, &n);
            KillTimer(hwnd, TIMER_ID);
            set_panic_hook(false, hwnd);
            let app = APP.with(|a| a.borrow_mut().take());
            if let Some(app) = app {
                app.node.shutdown.cancel();
                let node = app.node.clone();
                app.rt.block_on(async move { node.stop().await });
                app.rt.shutdown_timeout(std::time::Duration::from_secs(3));
            }
            if panel::app().is_some() {
                panel::quit();
            } else {
                PostQuitMessage(0);
            }
        }
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let taskbar = APP.with(|a| a.borrow().as_ref().map(|x| x.taskbar_created).unwrap_or(0));
    if taskbar != 0 && msg == taskbar {
        add_icon(hwnd);
        return 0;
    }
    match msg {
        WM_TRAY => {
            let event = (lparam as u32) & 0xFFFF;
            match event {
                WM_LBUTTONUP | NIN_SELECT | NIN_KEYSELECT_ => {
                    let node = APP.with(|a| a.borrow().as_ref().map(|x| x.node.clone()));
                    if let Some(n) = node {
                        open_panel(&n, "home");
                    }
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                NIN_BALLOONUSERCLICK => {
                    let x = APP.with(|a| a.borrow().as_ref().map(|x| (x.node.clone(), x.click_tab.clone())));
                    if let Some((n, tab)) = x {
                        open_panel(&n, &tab);
                    }
                }
                _ => {}
            }
            0
        }
        WM_EVENTS => {
            drain_events();
            0
        }
        WM_SHOWPANEL => {
            let node = APP.with(|a| a.borrow().as_ref().map(|x| x.node.clone()));
            if let Some(n) = node {
                open_panel(&n, "home");
            }
            0
        }
        m if m == WM_APP + 6 => {
            balloon("Мост запущен", "Подключите соседа: «Соседи» → «Пригласить» или «Подключить». Мост живёт здесь, у часов.", "peers");
            0
        }
        WM_PANIC => {
            let node = APP.with(|a| a.borrow().as_ref().map(|x| x.node.clone()));
            if let Some(n) = node {
                if n.remote.active().is_some() {
                    bridge_node::remote::revoke(&n, "остановлено клавишей Pause на этом ПК");
                    balloon("Управление остановлено", "Вы нажали Pause — сосед больше не управляет этим ПК", "voice");
                    refresh();
                }
            }
            0
        }
        WM_RESTART => {
            command(hwnd, CMD_EXIT);
            restart_after_update();
            0
        }
        WM_TIMER => {
            if wparam == OV_TIMER {
                if !crate::overlay::tick() {
                    KillTimer(hwnd, OV_TIMER);
                }
                return 0;
            }
            if wparam == TIMER_ID {
                refresh();
                let stopped = APP.with(|a| a.borrow().as_ref().map(|x| x.node.shutdown.is_cancelled()).unwrap_or(false));
                if stopped {
                    command(hwnd, CMD_EXIT);
                }
            }
            0
        }
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wparam != 0 {
                command(hwnd, CMD_EXIT);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn make_icon(dot: u32, badge: bool) -> HICON {
    icon_from(&icon_px(dot, badge))
}

unsafe fn make_alert_icon(control: bool) -> HICON {
    icon_from(&alert_px(control))
}

/// Пиксели ARGB → RGBA (для иконки окна).
fn rgba(px: &[u32]) -> Vec<u8> {
    px.iter().flat_map(|c| [(c >> 16) as u8, (c >> 8) as u8, *c as u8, (c >> 24) as u8]).collect()
}

/// Значок 32×32: синий скруглённый квадрат, белая дуга моста, точка состояния; отметка «ждёт вас» — красный уголок.
fn icon_px(dot: u32, badge: bool) -> Vec<u32> {
    const S: i32 = 32;
    let mut px = vec![0u32; (S * S) as usize];
    let put = |px: &mut Vec<u32>, x: i32, y: i32, c: u32| {
        if (0..S).contains(&x) && (0..S).contains(&y) {
            px[(y * S + x) as usize] = c;
        }
    };
    // фон
    for y in 0..S {
        for x in 0..S {
            let r = 7.0f32;
            let cx = (x as f32 + 0.5).clamp(r, S as f32 - r);
            let cy = (y as f32 + 0.5).clamp(r, S as f32 - r);
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            if d <= r {
                put(&mut px, x, y, 0xFF16133E);
            }
        }
    }
    // дуга: y = 21 − 10·(1 − ((x−16)/10)²), толщина 3
    // неоновая жила моста: циан → фиолетовый → маджента
    let cable = |t: f32| -> u32 {
        let (a, b, k) = if t < 0.55 { ((0x3D, 0xF2, 0xFF), (0x8F, 0x7C, 0xFF), t / 0.55) } else { ((0x8F, 0x7C, 0xFF), (0xFF, 0x3D, 0xCB), (t - 0.55) / 0.45) };
        let m = |p: i32, q: i32| (p as f32 + (q - p) as f32 * k).round() as u32;
        0xFF00_0000 | (m(a.0, b.0) << 16) | (m(a.1, b.1) << 8) | m(a.2, b.2)
    };
    for x in 5..27 {
        let t = (x as f32 - 16.0) / 10.0;
        let yc = 21.0 - 9.0 * (1.0 - t * t).max(0.0);
        for dy in -1..=1 {
            put(&mut px, x, (yc.round() as i32) + dy, cable((x - 5) as f32 / 21.0));
        }
    }
    for (cx, cy, c) in [(7, 21, 0xFF3DF2FFu32), (25, 21, 0xFFFF3DCB)] {
        for y in -2..=2 {
            for x in -2..=2 {
                if x * x + y * y <= 5 {
                    put(&mut px, cx + x, cy + y, c);
                }
            }
        }
    }
    // точка состояния (справа снизу) с белой обводкой
    for y in -7..=7 {
        for x in -7..=7 {
            let d = x * x + y * y;
            if d <= 49 {
                put(&mut px, 24 + x, 24 + y, if d <= 30 { dot } else { 0xFF0A0920 });
            }
        }
    }
    if badge {
        for y in 0..9 {
            for x in (S - 9 + y)..S {
                put(&mut px, x, y, 0xFFFF3DCB);
            }
        }
    }
    px
}

/// Значок из пикселей 32×32 (ARGB).
unsafe fn icon_from(px: &[u32]) -> HICON {
    const S: i32 = 32;
    let hdc = GetDC(null_mut());
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = S;
    bmi.bmiHeader.biHeight = -S; // сверху вниз
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut c_void = null_mut();
    let color = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if !bits.is_null() {
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, px.len());
    }
    let mask_bits = vec![0u8; (S * S / 8) as usize];
    let mask = CreateBitmap(S, S, 1, 1, mask_bits.as_ptr() as *const c_void);
    let ii = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
    let icon = CreateIconIndirect(&ii);
    DeleteObject(color);
    DeleteObject(mask);
    ReleaseDC(null_mut(), hdc);
    icon
}

/// Тревожные значки: «вас заглушили» (ярко-красный, перечёркнутый микрофон) и «вашим ПК управляют» (янтарный, указатель).
fn alert_px(control: bool) -> Vec<u32> {
    const S: i32 = 32;
    let mut px = vec![0u32; (S * S) as usize];
    let bg = if control { 0xFFFFA400u32 } else { 0xFFE8102Eu32 };
    let fg = if control { 0xFF14110Au32 } else { 0xFFFFFFFFu32 };
    for y in 0..S {
        for x in 0..S {
            let r = 6.0f32;
            let cx = (x as f32 + 0.5).clamp(r, S as f32 - r);
            let cy = (y as f32 + 0.5).clamp(r, S as f32 - r);
            if ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt() <= r {
                px[(y * S + x) as usize] = bg;
            }
        }
    }
    let put = |px: &mut Vec<u32>, x: i32, y: i32, c: u32| {
        if (0..S).contains(&x) && (0..S).contains(&y) {
            px[(y * S + x) as usize] = c;
        }
    };
    if control {
        // стрелка указателя
        for y in 0..20 {
            for x in 0..=(y * 11 / 19) {
                let inside = !(y > 13 && x > (19 - y) * 2);
                if inside {
                    put(&mut px, 8 + x, 5 + y, fg);
                }
            }
        }
        for t in 0..8 {
            for w in -1..=1 {
                put(&mut px, 14 + t / 2 + w, 18 + t, fg);
            }
        }
    } else {
        // микрофон: капсула, дужка, ножка
        for y in 5..18 {
            for x in 12..20 {
                let (cx, cy) = (16.0f32, if y < 9 { 9.0 } else if y > 14 { 14.0 } else { y as f32 });
                if ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt() <= 4.2 {
                    put(&mut px, x, y, fg);
                }
            }
        }
        for a in 0..=40 {
            let t = std::f32::consts::PI * a as f32 / 40.0;
            let (x, y) = (16.0 + 7.5 * t.cos(), 14.0 + 7.5 * t.sin());
            for (dx, dy) in [(0, 0), (1, 0), (0, 1)] {
                put(&mut px, x as i32 + dx, y as i32 + dy, fg);
            }
        }
        for y in 22..27 {
            put(&mut px, 15, y, fg);
            put(&mut px, 16, y, fg);
        }
        // косая черта: тёмная окантовка и белая линия
        for i in 3..29 {
            for w in -3i32..=3 {
                put(&mut px, i + w.max(0), i - w.min(0), if w.abs() >= 2 { 0xFF5A0010 } else { fg });
            }
        }
    }
    px
}

#[allow(dead_code)]
fn unused(_: Ordering) {}

/// Запустить новую версию (те же параметры + --tray --after-update); старая уже остановила узел и выходит.
fn restart_after_update() {
    let exe = EXE.get().cloned().or_else(|| std::env::current_exe().ok()).unwrap_or_default();
    let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--after-update" && a != "--tray").collect();
    let _ = std::process::Command::new(&exe).args(&args).arg("--tray").arg("--after-update").spawn();
}

// ───────────── аварийная остановка управления: настоящая клавиша Pause ─────────────

static PANIC_HOOK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static PANIC_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Ловушка клавиатуры стоит только пока ПК управляют: она отличает настоящие нажатия от присланных (LLKHF_INJECTED).
fn set_panic_hook(on: bool, hwnd: HWND) {
    use std::sync::atomic::Ordering as O;
    let cur = PANIC_HOOK.load(O::SeqCst);
    unsafe {
        if on && cur == 0 {
            PANIC_HWND.store(hwnd as isize, O::SeqCst);
            let h = SetWindowsHookExW(WH_KEYBOARD_LL, Some(panic_proc), GetModuleHandleW(null()), 0);
            PANIC_HOOK.store(h as isize, O::SeqCst);
        } else if !on && cur != 0 {
            UnhookWindowsHookEx(cur as HHOOK);
            PANIC_HOOK.store(0, O::SeqCst);
        }
    }
}

unsafe extern "system" fn panic_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && (wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN) {
        let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
        if kb.vkCode == windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_PAUSE as u32 && kb.flags & LLKHF_INJECTED == 0 {
            PostMessageW(PANIC_HWND.load(std::sync::atomic::Ordering::SeqCst) as HWND, WM_PANIC, 0, 0);
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}
