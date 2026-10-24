//! Удалённое управление этим ПК и «заглушили» — то, что узел знает о своём пользователе в канале.
//!
//! Управление: только после явного разрешения в панели этого ПК (там нужно удерживать кнопку 2 секунды). Панель
//! регистрирует разрешение (`grant`) — кому, что можно (мышь или мышь и клавиатура) и какой монитор показывается,
//! и продлевает его раз в несколько секунд; без продления через 30 с разрешение пропадает само. События мыши и
//! клавиатуры приходят от панели этого же ПК (локальный ключ) и выполняются, только если разрешение есть и сосед тот же.
//! Отозвать можно из панели, из меню трея; закрыли панель — разрешение истекает.

use crate::node::Node;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const GRANT_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct Grant {
    pub peer: String,
    pub keyboard: bool,
    /// размер показываемого монитора (физические пиксели) — по нему узел находит монитор
    pub w: i32,
    pub h: i32,
    pub since: Instant,
    pub until: Instant,
}

#[derive(Default)]
pub struct Remote {
    grant: Mutex<Option<Grant>>,
    /// кто заглушил этот ПК в канале (пусто — не заглушён)
    muted: Mutex<String>,
    /// нажатые сейчас клавиши и кнопки (отпустить при отзыве)
    held: Mutex<(Vec<String>, Vec<u8>)>,
}

impl Remote {
    pub fn state(&self) -> Value {
        let g = self.active();
        json!({
            "muted": *self.muted.lock(),
            "control": g.as_ref().map(|g| json!({ "peer": g.peer, "keyboard": g.keyboard, "seconds": g.since.elapsed().as_secs() })),
            "supported": cfg!(windows),
        })
    }
    pub fn active(&self) -> Option<Grant> {
        let mut g = self.grant.lock();
        if g.as_ref().map(|x| x.until < Instant::now()).unwrap_or(false) {
            *g = None;
        }
        g.clone()
    }
    pub fn muted_by(&self) -> String {
        self.muted.lock().clone()
    }
}

/// Панель сообщает: этот ПК заглушили (`by`) или снова слышно (пусто).
pub fn set_muted(node: &Node, by: &str) {
    let by: String = by.chars().take(60).collect();
    let changed = { let mut m = node.remote.muted.lock(); let c = *m != by; *m = by.clone(); c };
    if changed {
        if !by.is_empty() {
            node.info(&format!("Вас заглушил «{}» — включить микрофон можете только вы", by));
        }
        node.emit("self-muted", json!({ "on": !by.is_empty(), "by": by }));
    }
}

/// Разрешить соседу управление (или продлить разрешение).
pub fn grant(node: &Node, peer: &str, keyboard: bool, w: i32, h: i32) -> Result<(), String> {
    if !cfg!(windows) {
        return Err("управление ПК пока есть только в Windows".into());
    }
    if node.cfg.read().peer(peer).is_none() {
        return Err("нет такого соседа".into());
    }
    let now = Instant::now();
    let fresh = {
        let mut g = node.remote.grant.lock();
        let same = g.as_ref().map(|x| bridge_core::eq_ic(&x.peer, peer) && x.keyboard == keyboard && x.until > now).unwrap_or(false);
        let since = if same { g.as_ref().unwrap().since } else { now };
        *g = Some(Grant { peer: peer.to_string(), keyboard, w, h, since, until: now + GRANT_TTL });
        !same
    };
    if fresh {
        node.info(&format!("«{}» управляет этим ПК: {}", peer, if keyboard { "мышь и клавиатура" } else { "мышь" }));
        node.emit("control", json!({ "on": true, "peer": peer, "keyboard": keyboard }));
    }
    Ok(())
}

/// Для проверок: разрешение без проверки ОС (управление само по себе работает только в Windows).
#[doc(hidden)]
pub fn grant_for_test(node: &Node, peer: &str) {
    let now = Instant::now();
    *node.remote.grant.lock() = Some(Grant { peer: peer.to_string(), keyboard: false, w: 0, h: 0, since: now, until: now + GRANT_TTL });
}

/// Отозвать управление (отпустить всё нажатое).
pub fn revoke(node: &Node, why: &str) {
    let was = node.remote.grant.lock().take();
    release_all(node);
    if let Some(g) = was {
        node.info(&format!("Управление этим ПК («{}») прекращено{}", g.peer, if why.is_empty() { String::new() } else { format!(": {}", why) }));
        node.emit("control", json!({ "on": false, "peer": g.peer }));
        node.media.deliver(json!({ "media": "ui", "type": "ctl-revoked", "peer": g.peer, "why": why }));
    }
}

/// Что можно делать через локальный API, пока этим ПК управляют по сети (остальное — «мост заперт»):
/// разговор, чат, доска, пометки на показе, само управление и его отзыв. Права, соседи, папки, пространства,
/// каналы, телефоны, команды, обновления и настройки агентов — нет: ими управляет только сам хозяин ПК.
pub fn allowed_while_controlled(rest: &str) -> bool {
    let rest = rest.split('?').next().unwrap_or("");
    const OK: &[&str] = &[
        "media/signal", "media/local", "chat/send", "chat/read", "board/op", "board/image", "board/stand/save", "overlay/ann",
        "control/grant", "control/revoke", "control/input", "self/muted", "ui", "ui/claim", "ui/result", "ui/settings",
        "assistant/send", "assistant/read", "assistant/presence", "assistant/reply", "sync", "update/decline", "stickers/keep",
        "agent/progress", "agent/report", "agent/take", "shots/save",
    ];
    OK.contains(&rest)
}

/// События от панели: [{t: m|d|u|w|kd|ku, x, y, b, dx, dy, c}]; x, y — доли (0..1) показываемого монитора.
/// Щелчки и прокрутка над окнами самого моста (панель, меню у часов, его диалоги) и клавиши, когда активно окно
/// моста, не выполняются: управляющий не может ничего поменять в мосте этого ПК.
pub fn input(node: &Node, peer: &str, events: &[Value]) -> Result<usize, (u16, String)> {
    let g = node.remote.active().ok_or((403, "управление не разрешено".to_string()))?;
    if !bridge_core::eq_ic(&g.peer, peer) {
        return Err((403, "управление разрешено другому соседу".into()));
    }
    let mut done = 0;
    for e in events.iter().take(400) {
        let t = e["t"].as_str().unwrap_or("");
        if (t == "kd" || t == "ku") && !g.keyboard {
            continue;
        }
        if matches!(t, "d" | "w" | "kd") && own_window(&g, e) {
            node.info_once("ctl-guard", &format!("Защита: «{}» щёлкнул или нажал клавишу в окне моста — не выполнено (мост своим окном не управляется)", g.peer), 15);
            continue;
        }
        {
            let mut h = node.remote.held.lock();
            match t {
                "kd" => { if let Some(c) = e["c"].as_str() { if !h.0.iter().any(|x| x == c) { h.0.push(c.to_string()); } } }
                "ku" => { if let Some(c) = e["c"].as_str() { h.0.retain(|x| x != c); } }
                "d" => { let b = e["b"].as_u64().unwrap_or(0) as u8; if !h.1.contains(&b) { h.1.push(b); } }
                "u" => { let b = e["b"].as_u64().unwrap_or(0) as u8; h.1.retain(|x| *x != b); }
                _ => {}
            }
        }
        if inject(&g, e) {
            done += 1;
        }
    }
    Ok(done)
}

fn release_all(node: &Node) {
    let (keys, buttons) = std::mem::take(&mut *node.remote.held.lock());
    let g = Grant { peer: String::new(), keyboard: true, w: 0, h: 0, since: Instant::now(), until: Instant::now() };
    for c in keys {
        inject(&g, &json!({ "t": "ku", "c": c }));
    }
    for b in buttons {
        inject(&g, &json!({ "t": "u", "b": b }));
    }
}

/// Сканкод клавиши по KeyboardEvent.code: (код, расширенная).
pub fn scan(code: &str) -> Option<(u16, bool)> {
    let s = |c: u16| Some((c, false));
    let e = |c: u16| Some((c, true));
    if let Some(l) = code.strip_prefix("Key") {
        let c = l.chars().next()?;
        const ROW: &[(char, u16)] = &[('Q', 0x10), ('W', 0x11), ('E', 0x12), ('R', 0x13), ('T', 0x14), ('Y', 0x15), ('U', 0x16), ('I', 0x17), ('O', 0x18), ('P', 0x19), ('A', 0x1E), ('S', 0x1F), ('D', 0x20), ('F', 0x21), ('G', 0x22), ('H', 0x23), ('J', 0x24), ('K', 0x25), ('L', 0x26), ('Z', 0x2C), ('X', 0x2D), ('C', 0x2E), ('V', 0x2F), ('B', 0x30), ('N', 0x31), ('M', 0x32)];
        return ROW.iter().find(|(k, _)| *k == c).map(|(_, v)| (*v, false));
    }
    if let Some(d) = code.strip_prefix("Digit") {
        let n: u16 = d.parse().ok()?;
        return s(if n == 0 { 0x0B } else { 0x01 + n });
    }
    if let Some(f) = code.strip_prefix('F') {
        if let Ok(n) = f.parse::<u16>() {
            return match n { 1..=10 => s(0x3A + n), 11 => s(0x57), 12 => s(0x58), _ => None };
        }
    }
    match code {
        "Escape" => s(0x01), "Minus" => s(0x0C), "Equal" => s(0x0D), "Backspace" => s(0x0E), "Tab" => s(0x0F),
        "BracketLeft" => s(0x1A), "BracketRight" => s(0x1B), "Enter" => s(0x1C), "ControlLeft" => s(0x1D),
        "Semicolon" => s(0x27), "Quote" => s(0x28), "Backquote" => s(0x29), "ShiftLeft" => s(0x2A), "Backslash" => s(0x2B),
        "Comma" => s(0x33), "Period" => s(0x34), "Slash" => s(0x35), "ShiftRight" => s(0x36), "NumpadMultiply" => s(0x37),
        "AltLeft" => s(0x38), "Space" => s(0x39), "CapsLock" => s(0x3A), "NumLock" => e(0x45), "ScrollLock" => s(0x46),
        "Numpad7" => s(0x47), "Numpad8" => s(0x48), "Numpad9" => s(0x49), "NumpadSubtract" => s(0x4A), "Numpad4" => s(0x4B),
        "Numpad5" => s(0x4C), "Numpad6" => s(0x4D), "NumpadAdd" => s(0x4E), "Numpad1" => s(0x4F), "Numpad2" => s(0x50),
        "Numpad3" => s(0x51), "Numpad0" => s(0x52), "NumpadDecimal" => s(0x53), "IntlBackslash" => s(0x56),
        "NumpadEnter" => e(0x1C), "ControlRight" => e(0x1D), "NumpadDivide" => e(0x35), "PrintScreen" => e(0x37), "AltRight" => e(0x38),
        "Home" => e(0x47), "ArrowUp" => e(0x48), "PageUp" => e(0x49), "ArrowLeft" => e(0x4B), "ArrowRight" => e(0x4D), "End" => e(0x4F),
        "ArrowDown" => e(0x50), "PageDown" => e(0x51), "Insert" => e(0x52), "Delete" => e(0x53), "MetaLeft" => e(0x5B), "MetaRight" => e(0x5C), "ContextMenu" => e(0x5D),
        _ => None,
    }
}

#[cfg(not(any(windows, feature = "check-win")))]
fn inject(_g: &Grant, _e: &Value) -> bool {
    false
}

#[cfg(not(any(windows, feature = "check-win")))]
fn own_window(_g: &Grant, _e: &Value) -> bool {
    false
}

#[cfg(any(windows, feature = "check-win"))]
fn own_window(g: &Grant, e: &Value) -> bool {
    win::own_window(g, e)
}

#[cfg(any(windows, feature = "check-win"))]
fn inject(g: &Grant, e: &Value) -> bool {
    win::inject(g, e)
}

#[cfg(any(windows, feature = "check-win"))]
mod win {
    use super::{scan, Grant};
    use serde_json::Value;
    use windows_sys::Win32::Foundation::{BOOL, LPARAM, POINT, RECT, TRUE};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    use windows_sys::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT, MONITORINFOF_PRIMARY, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
        SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };

    /// Окно принадлежит процессу моста (панель, меню у часов, его диалоги выбора файлов)?
    fn ours(h: windows_sys::Win32::Foundation::HWND) -> bool {
        if h.is_null() {
            return false;
        }
        unsafe {
            let root = GetAncestor(h, GA_ROOT);
            let mut pid = 0u32;
            GetWindowThreadProcessId(if root.is_null() { h } else { root }, &mut pid);
            pid == std::process::id()
        }
    }

    /// Щелчок или прокрутка попадёт в окно моста, или клавиша — в активное окно моста.
    pub fn own_window(g: &Grant, e: &Value) -> bool {
        let prev = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let r = match e["t"].as_str().unwrap_or("") {
            "kd" | "ku" => ours(unsafe { GetForegroundWindow() }),
            _ => {
                let at = match (e["x"].as_f64(), e["y"].as_f64(), monitor(g.w, g.h)) {
                    (Some(x), Some(y), Some(r)) => Some(POINT {
                        x: (r.left as f64 + x.clamp(0.0, 1.0) * ((r.right - r.left - 1) as f64)).round() as i32,
                        y: (r.top as f64 + y.clamp(0.0, 1.0) * ((r.bottom - r.top - 1) as f64)).round() as i32,
                    }),
                    _ => {
                        // прокрутка без координат — туда, где сейчас указатель
                        let mut p = POINT { x: 0, y: 0 };
                        (unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) } != 0).then_some(p)
                    }
                };
                at.map(|p| ours(unsafe { WindowFromPoint(p) })).unwrap_or(false)
            }
        };
        unsafe { SetThreadDpiAwarenessContext(prev) };
        r
    }

    unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = &mut *(data as *mut Vec<(RECT, bool)>);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(m, &mut mi) != 0 {
            list.push((mi.rcMonitor, mi.dwFlags & MONITORINFOF_PRIMARY != 0));
        }
        TRUE
    }

    /// Монитор, который показывают: совпадает по размеру; иначе — основной.
    fn monitor(w: i32, h: i32) -> Option<RECT> {
        let mut list: Vec<(RECT, bool)> = Vec::new();
        unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(collect), &mut list as *mut _ as LPARAM) };
        let size = |r: &RECT| (r.right - r.left, r.bottom - r.top);
        list.iter().find(|(r, _)| size(r) == (w, h)).or_else(|| list.iter().find(|(_, p)| *p)).or(list.first()).map(|(r, _)| *r)
    }

    fn send(inputs: &[INPUT]) -> bool {
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) == inputs.len() as u32 }
    }

    fn mouse(flags: u32, dx: i32, dy: i32, data: i32) -> INPUT {
        let mut i: INPUT = unsafe { std::mem::zeroed() };
        i.r#type = INPUT_MOUSE;
        i.Anonymous.mi = MOUSEINPUT { dx, dy, mouseData: data as _, dwFlags: flags, time: 0, dwExtraInfo: 0 };
        i
    }

    pub fn inject(g: &Grant, e: &Value) -> bool {
        // координаты — в физических пикселях, как бы ни было настроено масштабирование
        let prev = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let ok = run(g, e);
        unsafe { SetThreadDpiAwarenessContext(prev) };
        ok
    }

    fn abs(g: &Grant, e: &Value) -> Option<(i32, i32)> {
        let (x, y) = (e["x"].as_f64()?, e["y"].as_f64()?);
        let r = monitor(g.w, g.h)?;
        let px = r.left as f64 + x.clamp(0.0, 1.0) * ((r.right - r.left - 1) as f64);
        let py = r.top as f64 + y.clamp(0.0, 1.0) * ((r.bottom - r.top - 1) as f64);
        let (vx, vy, vw, vh) = unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
        if vw <= 1 || vh <= 1 {
            return None;
        }
        Some((((px - vx as f64) * 65535.0 / (vw - 1) as f64).round() as i32, ((py - vy as f64) * 65535.0 / (vh - 1) as f64).round() as i32))
    }

    fn run(g: &Grant, e: &Value) -> bool {
        let move_abs = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        match e["t"].as_str().unwrap_or("") {
            "m" => abs(g, e).map(|(x, y)| send(&[mouse(move_abs, x, y, 0)])).unwrap_or(false),
            t @ ("d" | "u") => {
                let down = t == "d";
                let flag = match e["b"].as_u64().unwrap_or(0) {
                    0 => if down { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP },
                    1 => if down { MOUSEEVENTF_MIDDLEDOWN } else { MOUSEEVENTF_MIDDLEUP },
                    2 => if down { MOUSEEVENTF_RIGHTDOWN } else { MOUSEEVENTF_RIGHTUP },
                    _ => return false,
                };
                let mut v = Vec::new();
                if let Some((x, y)) = abs(g, e) {
                    v.push(mouse(move_abs, x, y, 0));
                }
                v.push(mouse(flag, 0, 0, 0));
                send(&v)
            }
            "w" => {
                let dy = e["dy"].as_f64().unwrap_or(0.0);
                let dx = e["dx"].as_f64().unwrap_or(0.0);
                let mut v = Vec::new();
                if dy.abs() > 0.0 {
                    v.push(mouse(MOUSEEVENTF_WHEEL, 0, 0, (-dy * 1.2).round().clamp(-2400.0, 2400.0) as i32));
                }
                if dx.abs() > 0.0 {
                    v.push(mouse(MOUSEEVENTF_HWHEEL, 0, 0, (dx * 1.2).round().clamp(-2400.0, 2400.0) as i32));
                }
                !v.is_empty() && send(&v)
            }
            t @ ("kd" | "ku") => {
                let Some((code, ext)) = e["c"].as_str().and_then(scan) else { return false };
                let mut i: INPUT = unsafe { std::mem::zeroed() };
                i.r#type = INPUT_KEYBOARD;
                let mut flags = KEYEVENTF_SCANCODE;
                if ext {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                if t == "ku" {
                    flags |= KEYEVENTF_KEYUP;
                }
                i.Anonymous.ki = KEYBDINPUT { wVk: 0, wScan: code, dwFlags: flags, time: 0, dwExtraInfo: 0 };
                send(&[i])
            }
            _ => false,
        }
    }
}
