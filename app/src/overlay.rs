#![cfg_attr(not(windows), allow(dead_code))]
//! Пометки зрителей поверх экрана показывающего: прозрачное окно на весь монитор, сквозь которое проходят щелчки,
//! поверх всех окон и исключённое из захвата экрана (WDA_EXCLUDEFROMCAPTURE) — зрители видят пометки у себя в
//! панели, а в сам показ они не попадают. Пинг — расходящиеся кольца, рисование — линия, через несколько секунд гаснет.

#![allow(clippy::missing_safety_doc)]

use serde_json::Value;
use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::time::Instant;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;
const PING_MS: u128 = 2600;
const LINE_HOLD_MS: u128 = 4000;
const LINE_FADE_MS: u128 = 2000;

enum Kind {
    Ping(f32, f32),
    Line { id: String, pts: Vec<(f32, f32)>, done: Option<Instant> },
}

struct Item {
    by: String,
    col: u32,
    t: Instant,
    kind: Kind,
}

struct Ov {
    hwnd: HWND,
    rect: RECT,
    items: Vec<Item>,
    dc: HDC,
    bmp: HBITMAP,
    bits: *mut u32,
    size: (i32, i32),
}

thread_local! {
    static OV: RefCell<Option<Ov>> = const { RefCell::new(None) };
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn color(s: &str) -> u32 {
    let h = s.trim_start_matches('#');
    u32::from_str_radix(h, 16).ok().filter(|_| h.len() == 6).unwrap_or(0x38E1F0)
}

unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let list = &mut *(data as *mut Vec<(RECT, bool)>);
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(m, &mut mi) != 0 {
        list.push((mi.rcMonitor, mi.dwFlags & MONITORINFOF_PRIMARY != 0));
    }
    1
}

unsafe fn monitor(wd: i32, ht: i32) -> Option<RECT> {
    let mut list: Vec<(RECT, bool)> = Vec::new();
    EnumDisplayMonitors(null_mut(), null(), Some(collect), &mut list as *mut _ as LPARAM);
    let size = |r: &RECT| (r.right - r.left, r.bottom - r.top);
    list.iter().find(|(r, _)| size(r) == (wd, ht)).or_else(|| list.iter().find(|(_, p)| *p)).or(list.first()).map(|(r, _)| *r)
}

unsafe fn ensure(wd: i32, ht: i32) -> bool {
    let rect = match monitor(wd, ht) {
        Some(r) => r,
        None => return false,
    };
    OV.with(|o| {
        let mut o = o.borrow_mut();
        if let Some(ov) = o.as_mut() {
            if ov.rect.left != rect.left || ov.rect.top != rect.top || ov.rect.right != rect.right || ov.rect.bottom != rect.bottom {
                ov.rect = rect;
                SetWindowPos(ov.hwnd, HWND_TOPMOST, rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top, SWP_NOACTIVATE);
            }
            return true;
        }
        let hinst = GetModuleHandleW(null());
        let class = w("ExerussusBridgeOverlay");
        let wc = WNDCLASSW { style: 0, lpfnWndProc: Some(DefWindowProcW), cbClsExtra: 0, cbWndExtra: 0, hInstance: hinst, hIcon: null_mut(), hCursor: null_mut(), hbrBackground: null_mut(), lpszMenuName: null(), lpszClassName: class.as_ptr() };
        RegisterClassW(&wc);
        let title = w("Мост · пометки");
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            null_mut(),
            null_mut(),
            hinst,
            null(),
        );
        if hwnd.is_null() {
            return false;
        }
        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        *o = Some(Ov { hwnd, rect, items: Vec::new(), dc: null_mut(), bmp: null_mut(), bits: null_mut(), size: (0, 0) });
        true
    })
}

/// Пометка от панели: {by, col, a: {k: ping|line|clear, x, y, id, pts, done}, w, h}.
pub unsafe fn push(d: &Value, main: HWND, timer: usize) {
    let prev = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let (wd, ht) = (d["w"].as_i64().unwrap_or(0) as i32, d["h"].as_i64().unwrap_or(0) as i32);
    if ensure(wd, ht) {
        let by = d["by"].as_str().unwrap_or("").to_string();
        let col = color(d["col"].as_str().unwrap_or(""));
        let a = &d["a"];
        OV.with(|o| {
            let mut o = o.borrow_mut();
            let Some(ov) = o.as_mut() else { return };
            match a["k"].as_str().unwrap_or("") {
                "ping" => ov.items.push(Item { by, col, t: Instant::now(), kind: Kind::Ping(a["x"].as_f64().unwrap_or(0.5) as f32, a["y"].as_f64().unwrap_or(0.5) as f32) }),
                "line" => {
                    let id = a["id"].as_str().unwrap_or("").to_string();
                    let raw: Vec<f32> = a["pts"].as_array().map(|v| v.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect()).unwrap_or_default();
                    let pts: Vec<(f32, f32)> = raw.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).take(4000).collect();
                    let done = if a["done"] == true { Some(Instant::now()) } else { None };
                    if let Some(it) = ov.items.iter_mut().find(|it| matches!(&it.kind, Kind::Line { id: i, .. } if *i == id)) {
                        it.kind = Kind::Line { id, pts, done };
                        it.t = Instant::now();
                    } else {
                        ov.items.push(Item { by, col, t: Instant::now(), kind: Kind::Line { id, pts, done } });
                    }
                }
                "clear" => ov.items.retain(|it| it.by != by),
                _ => {}
            }
            if ov.items.len() > 300 {
                let n = ov.items.len() - 300;
                ov.items.drain(..n);
            }
            ShowWindow(ov.hwnd, SW_SHOWNOACTIVATE);
        });
        SetTimer(main, timer, 33, None);
        tick();
    }
    SetThreadDpiAwarenessContext(prev);
}

/// Кадр. false — пометок не осталось (окно спрятано, таймер можно остановить).
pub fn tick() -> bool {
    OV.with(|o| {
        let mut o = o.borrow_mut();
        let Some(ov) = o.as_mut() else { return false };
        let now = Instant::now();
        ov.items.retain(|it| match &it.kind {
            Kind::Ping(..) => now.duration_since(it.t).as_millis() < PING_MS,
            Kind::Line { done, .. } => match done {
                Some(d) => now.duration_since(*d).as_millis() < LINE_HOLD_MS + LINE_FADE_MS,
                None => now.duration_since(it.t).as_millis() < 20_000,
            },
        });
        unsafe {
            if ov.items.is_empty() {
                ShowWindow(ov.hwnd, SW_HIDE);
                return false;
            }
            render(ov, now);
        }
        true
    })
}

unsafe fn render(ov: &mut Ov, now: Instant) {
    let (wd, ht) = (ov.rect.right - ov.rect.left, ov.rect.bottom - ov.rect.top);
    if wd <= 0 || ht <= 0 {
        return;
    }
    let screen = GetDC(null_mut());
    if ov.size != (wd, ht) || ov.bits.is_null() {
        if !ov.bmp.is_null() {
            DeleteObject(ov.bmp);
        }
        if ov.dc.is_null() {
            ov.dc = CreateCompatibleDC(screen);
        }
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = wd;
        bmi.bmiHeader.biHeight = -ht;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = null_mut();
        ov.bmp = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        ov.bits = bits as *mut u32;
        ov.size = (wd, ht);
        SelectObject(ov.dc, ov.bmp);
    }
    if ov.bits.is_null() {
        ReleaseDC(null_mut(), screen);
        return;
    }
    let buf = std::slice::from_raw_parts_mut(ov.bits, (wd * ht) as usize);
    buf.fill(0);
    let mut cv = Canvas { buf, w: wd, h: ht };
    let scale = (ht as f32 / 1080.0).max(0.75);
    for it in &ov.items {
        match &it.kind {
            Kind::Ping(x, y) => {
                let age = now.duration_since(it.t).as_millis() as f32 / PING_MS as f32;
                let (px, py) = (x * wd as f32, y * ht as f32);
                for i in 0..2 {
                    let a = (age - i as f32 * 0.18).max(0.0);
                    cv.ring(px, py, (8.0 + a * 60.0) * scale, 3.0 * scale, it.col, (1.0 - a).max(0.0) * 0.9);
                }
                cv.disc(px, py, 6.0 * scale, it.col, (1.0 - age).max(0.0));
            }
            Kind::Line { pts, done, .. } => {
                let fade = match done {
                    Some(d) => 1.0 - ((now.duration_since(*d).as_millis() as f32 - LINE_HOLD_MS as f32).max(0.0) / LINE_FADE_MS as f32),
                    None => 1.0,
                }
                .clamp(0.0, 1.0);
                let r = 3.0 * scale;
                for s in pts.windows(2) {
                    let (ax, ay, bx, by) = (s[0].0 * wd as f32, s[0].1 * ht as f32, s[1].0 * wd as f32, s[1].1 * ht as f32);
                    let n = ((bx - ax).hypot(by - ay) / (r * 0.5)).ceil().max(1.0) as i32;
                    for k in 0..=n {
                        let t = k as f32 / n as f32;
                        cv.disc_max(ax + (bx - ax) * t, ay + (by - ay) * t, r, it.col, fade * 0.95);
                    }
                }
            }
        }
    }
    let pos = POINT { x: ov.rect.left, y: ov.rect.top };
    let size = SIZE { cx: wd, cy: ht };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
    UpdateLayeredWindow(ov.hwnd, screen, &pos, &size, ov.dc, &src, 0, &blend, ULW_ALPHA);
    ReleaseDC(null_mut(), screen);
}

/// Простейшая растеризация в буфер BGRA с предумноженной прозрачностью.
struct Canvas<'a> {
    buf: &'a mut [u32],
    w: i32,
    h: i32,
}

impl Canvas<'_> {
    fn blend(&mut self, x: i32, y: i32, rgb: u32, a: f32, max: bool) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h || a <= 0.0 {
            return;
        }
        let i = (y * self.w + x) as usize;
        let d = self.buf[i];
        let da = (d >> 24) as f32 / 255.0;
        if max && da >= a {
            return;
        }
        let (r, g, b) = (((rgb >> 16) & 255) as f32, ((rgb >> 8) & 255) as f32, (rgb & 255) as f32);
        let (dr, dg, db) = (((d >> 16) & 255) as f32, ((d >> 8) & 255) as f32, (d & 255) as f32);
        let k = 1.0 - a;
        let oa = (a + da * k).min(1.0);
        let px = |s: f32, dd: f32| ((s * a + dd * k).min(255.0)) as u32;
        self.buf[i] = (((oa * 255.0) as u32) << 24) | (px(r, dr) << 16) | (px(g, dg) << 8) | px(b, db);
    }
    fn disc_inner(&mut self, cx: f32, cy: f32, r: f32, rgb: u32, a: f32, max: bool) {
        let (x0, x1, y0, y1) = ((cx - r - 1.0) as i32, (cx + r + 1.0) as i32, (cy - r - 1.0) as i32, (cy + r + 1.0) as i32);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let cov = (r + 0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, rgb, cov * a, max);
                }
            }
        }
    }
    fn disc(&mut self, cx: f32, cy: f32, r: f32, rgb: u32, a: f32) {
        self.disc_inner(cx, cy, r, rgb, a, false)
    }
    /// для линий: соседние кружки не накладываются друг на друга (ровная прозрачность)
    fn disc_max(&mut self, cx: f32, cy: f32, r: f32, rgb: u32, a: f32) {
        self.disc_inner(cx, cy, r, rgb, a, true)
    }
    fn ring(&mut self, cx: f32, cy: f32, r: f32, th: f32, rgb: u32, a: f32) {
        let o = r + th;
        let (x0, x1, y0, y1) = ((cx - o - 1.0) as i32, (cx + o + 1.0) as i32, (cy - o - 1.0) as i32, (cy + o + 1.0) as i32);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let cov = (th / 2.0 + 0.5 - (d - r).abs()).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(x, y, rgb, cov * a, true);
                }
            }
        }
    }
}
