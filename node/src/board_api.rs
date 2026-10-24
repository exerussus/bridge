//! Доска для ассистента: понятные вызовы поверх «сырых» операций доски (board/op).
//!
//! Ассистент кладёт на доску текст, заметки, фигуры, линии, стрелки, картинки (файл с этого ПК или base64) и PDF,
//! двигает и правит их, ищет по тексту, виду, слою и области, управляет слоями, фоном и включением доски.
//! Всё уходит владельцу канала обычными изменениями доски — участники видят сразу, отмена в панели работает как обычно.
//! Координаты — единицы доски (как «x · y» внизу доски), ось y — вниз.

use crate::board::{self, Room};
use crate::node::Node;
use serde_json::{json, Map, Value};
use std::sync::Arc;

type Res = Result<Value, (u16, String)>;

const CS: &str = "api";
const DEF_TEXT: &str = "#E8EAFB";
const DARK_TEXT: &str = "#11131F";
const NOTE_BG: &str = "#FFE58A";

fn bad(t: impl Into<String>) -> (u16, String) {
    (400, t.into())
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn num(v: &Value, k: &str) -> Option<f64> {
    match v.get(k) {
        Some(Value::Number(n)) => n.as_f64().filter(|f| f.is_finite()),
        Some(Value::String(t)) => t.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

fn r1(f: f64) -> Value {
    json!((f * 10.0).round() / 10.0)
}

fn now_ms() -> f64 {
    chrono::Utc::now().timestamp_millis() as f64
}

fn new_id(i: usize) -> String {
    format!("api-{}-{}", &uuid::Uuid::new_v4().simple().to_string()[..10], i)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// ───────────── цвета, шрифты ─────────────

/// Цвет: #rgb, #rrggbb или имя (white, black, cyan, red, orange, yellow, green, purple, blue, gray и по-русски).
pub fn color(v: &str) -> Result<String, String> {
    let t = v.trim().to_lowercase();
    let named = match t.as_str() {
        "white" | "белый" => "#E8EAFB",
        "black" | "чёрный" | "черный" => "#11131F",
        "cyan" | "голубой" | "бирюзовый" => "#38E1F0",
        "red" | "красный" => "#FF4F64",
        "orange" | "amber" | "оранжевый" => "#FFB547",
        "yellow" | "жёлтый" | "желтый" => "#FFE58A",
        "green" | "зелёный" | "зеленый" => "#5DE08E",
        "purple" | "violet" | "фиолетовый" => "#9B7BFF",
        "blue" | "синий" => "#4F7BFF",
        "gray" | "grey" | "серый" => "#8A8FB0",
        "pink" | "розовый" => "#FF8FA0",
        _ => "",
    };
    if !named.is_empty() {
        return Ok(named.into());
    }
    let h = t.trim_start_matches('#');
    if h.chars().all(|c| c.is_ascii_hexdigit()) {
        if h.len() == 6 {
            return Ok(format!("#{}", h.to_uppercase()));
        }
        if h.len() == 3 {
            let e: String = h.chars().flat_map(|c| [c, c]).collect();
            return Ok(format!("#{}", e.to_uppercase()));
        }
    }
    Err(format!("цвет «{}»: нужен #rrggbb, #rgb или имя (white, black, cyan, red, orange, yellow, green, purple, blue, gray, pink)", v))
}

/// Шрифт: '' (Мост — Manrope), simple (Golos Text), tech (Tektur), hand (Caveat). Принимаются и названия.
pub fn font(v: &str) -> Result<&'static str, String> {
    let t = v.trim().to_lowercase();
    Ok(match t.as_str() {
        "" | "default" | "мост" | "manrope" | "обычный" => "",
        "simple" | "простой" | "golos" | "golos text" => "simple",
        "tech" | "техно" | "scifi" | "sci-fi" | "tektur" => "tech",
        "hand" | "от руки" | "рукописный" | "caveat" | "handwritten" => "hand",
        _ => return Err(format!("шрифт «{}»: default, simple, tech или hand", v)),
    })
}

fn font_name(ff: &str) -> &'static str {
    match ff {
        "simple" => "simple (Golos Text)",
        "tech" => "tech (Tektur)",
        "hand" => "hand (Caveat)",
        _ => "default (Manrope)",
    }
}

// ───────────── рамка объекта (примерно для текста) ─────────────

fn char_w(o: &Value) -> f64 {
    let k = match s(o, "ff") {
        "hand" => 0.46,
        "tech" => 0.6,
        "simple" => 0.55,
        _ => 0.56,
    };
    if o["fb"].as_bool().unwrap_or(false) {
        k * 1.07
    } else {
        k
    }
}

/// Рамка [x0, y0, x1, y1]. Для текста и заметок — оценка (точный размер знает только панель).
pub fn bbox(o: &Value) -> [f64; 4] {
    let x = num(o, "x").unwrap_or(0.0);
    let y = num(o, "y").unwrap_or(0.0);
    match s(o, "t") {
        "path" => {
            let p: Vec<f64> = o["pts"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect()).unwrap_or_default();
            if p.len() < 2 {
                return [x, y, x, y];
            }
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for c in p.chunks(2) {
                if c.len() == 2 {
                    x0 = x0.min(c[0]);
                    x1 = x1.max(c[0]);
                    y0 = y0.min(c[1]);
                    y1 = y1.max(c[1]);
                }
            }
            let w = num(o, "w").unwrap_or(3.0) / 2.0;
            [x + x0 - w, y + y0 - w, x + x1 + w, y + y1 + w]
        }
        "arrow" => {
            let (x2, y2) = (num(o, "x2").unwrap_or(x), num(o, "y2").unwrap_or(y));
            [x.min(x2), y.min(y2), x.max(x2), y.max(y2)]
        }
        "link" => {
            let (ax, ay, bx, by) = (num(&o["a"], "x").unwrap_or(0.0), num(&o["a"], "y").unwrap_or(0.0), num(&o["b"], "x").unwrap_or(0.0), num(&o["b"], "y").unwrap_or(0.0));
            [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
        }
        "text" => {
            let fs = num(o, "fs").unwrap_or(20.0);
            let text = s(o, "text");
            let lines: Vec<&str> = text.split('\n').collect();
            let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1).max(1) as f64 * fs * char_w(o);
            [x, y, x + w, y + lines.len().max(1) as f64 * fs * 1.3]
        }
        "note" => {
            let fs = num(o, "fs").unwrap_or(16.0);
            let w = num(o, "w").unwrap_or(220.0);
            let per = ((w - 28.0) / (fs * char_w(o))).max(1.0);
            let lines: f64 = s(o, "text").split('\n').map(|l| (l.chars().count() as f64 / per).ceil().max(1.0)).sum();
            let h = (28.0 + lines * fs * 1.35).max(num(o, "h").unwrap_or(0.0));
            [x, y, x + w, y + h]
        }
        _ => {
            let (w, h) = (num(o, "w").unwrap_or(0.0), num(o, "h").unwrap_or(0.0));
            [x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)]
        }
    }
}

fn union(a: Option<[f64; 4]>, b: [f64; 4]) -> [f64; 4] {
    match a {
        None => b,
        Some(a) => [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])],
    }
}

// ───────────── картинки: размер по заголовку файла ─────────────

/// Ширина и высота PNG, JPEG, GIF, WebP — по заголовку, без декодирования.
pub fn img_size(b: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| -> Option<u32> { Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32) };
    let le16 = |i: usize| -> Option<u32> { Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32) };
    let le24 = |i: usize| -> Option<u32> { Some(u32::from_le_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, 0])) };
    if b.starts_with(b"\x89PNG") && b.len() >= 24 {
        return Some((u32::from_be_bytes([b[16], b[17], b[18], b[19]]), u32::from_be_bytes([b[20], b[21], b[22], b[23]])));
    }
    if b.starts_with(b"GIF8") {
        return Some((le16(6)?, le16(8)?));
    }
    if b.len() > 30 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return match &b[12..16] {
            b"VP8 " => Some((le16(26)? & 0x3FFF, le16(28)? & 0x3FFF)),
            b"VP8L" => {
                let v = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
                Some(((v & 0x3FFF) + 1, ((v >> 14) & 0x3FFF) + 1))
            }
            b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
            _ => None,
        };
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = b[i + 1];
            if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) || m == 0xFF {
                i += if m == 0xFF { 1 } else { 2 };
                continue;
            }
            let len = be16(i + 2)? as usize;
            if (0xC0..=0xCF).contains(&m) && m != 0xC4 && m != 0xC8 && m != 0xCC {
                return Some((be16(i + 7)?, be16(i + 5)?));
            }
            i += 2 + len;
        }
    }
    None
}

/// Байты из {path} (файл на этом ПК) или {data} (base64, можно data:…;base64,…).
fn bytes_of(v: &Value, max: usize) -> Result<Option<(Vec<u8>, String)>, (u16, String)> {
    if !s(v, "path").is_empty() {
        let p = std::path::PathBuf::from(s(v, "path"));
        let meta = std::fs::metadata(&p).map_err(|_| (404, format!("нет файла {}", p.display())))?;
        if !meta.is_file() {
            return Err((404, format!("{} — не файл", p.display())));
        }
        if meta.len() as usize > max {
            return Err((413, format!("файл больше {} МБ", max / 1024 / 1024)));
        }
        let name = p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Ok(Some((std::fs::read(&p).map_err(|e| (500, e.to_string()))?, name)));
    }
    if !s(v, "data").is_empty() {
        use base64::Engine;
        let d = s(v, "data");
        let d = d.split_once("base64,").map(|x| x.1).unwrap_or(d);
        let b = base64::engine::general_purpose::STANDARD.decode(d.trim()).map_err(|_| bad("data: не base64"))?;
        if b.len() > max {
            return Err((413, format!("больше {} МБ", max / 1024 / 1024)));
        }
        return Ok(Some((b, s(v, "name").to_string())));
    }
    if !s(v, "url").is_empty() {
        return Err(bad("по ссылке мост не скачивает: сохраните файл и передайте path (путь на этом ПК) или data (base64)"));
    }
    Ok(None)
}

// ───────────── доска: снимок, слои ─────────────

async fn open(node: &Arc<Node>, room: &str) -> Result<(Arc<Room>, Value), (u16, String)> {
    let key = resolve_room(node, room)?;
    let r = board::room(node, &key)?;
    let st = board::state(node, &r).await?;
    Ok((r, st))
}

/// Комната: «владелец:канал», id канала или его название (если без владельца и совпадений несколько — ошибка).
pub fn resolve_room(node: &Node, room: &str) -> Result<String, (u16, String)> {
    let room = room.trim();
    if room.is_empty() {
        return Err(bad("нужен room: «владелец:канал», id или название канала (список — GET board/rooms)"));
    }
    if room.contains(':') {
        return Ok(room.to_string());
    }
    let all = crate::media::all_channels(node);
    let hit: Vec<_> = all.iter().filter(|c| bridge_core::eq_ic(&c.id, room) || c.title.to_lowercase() == room.to_lowercase()).collect();
    match hit.len() {
        1 => Ok(format!("{}:{}", hit[0].owner, hit[0].id)),
        0 => Err((404, format!("нет канала «{}» (есть: {})", room, all.iter().map(|c| c.title.clone()).collect::<Vec<_>>().join(", ")))),
        _ => Err(bad(format!("каналов «{}» несколько — укажите «владелец:канал»: {}", room, hit.iter().map(|c| format!("{}:{}", c.owner, c.id)).collect::<Vec<_>>().join(", ")))),
    }
}

fn layers_of(st: &Value) -> Vec<(String, String)> {
    let mut l: Vec<(String, String)> = st["layers"].as_array().map(|a| a.iter().filter_map(|x| Some((x["id"].as_str()?.to_string(), x["name"].as_str().unwrap_or("").to_string()))).collect()).unwrap_or_default();
    if l.is_empty() {
        l.push(("base".into(), "Слой 1".into()));
    }
    l
}

/// Слой по id или названию; пусто — верхний слой (то, что кладёт ассистент, видно поверх).
fn layer_id(st: &Value, want: &str) -> Result<String, (u16, String)> {
    let l = layers_of(st);
    if want.trim().is_empty() {
        return Ok(l.last().map(|x| x.0.clone()).unwrap_or_else(|| "base".into()));
    }
    l.iter()
        .find(|(id, name)| id == want || name.to_lowercase() == want.trim().to_lowercase())
        .map(|x| x.0.clone())
        .ok_or_else(|| (404, format!("нет слоя «{}» (есть: {})", want, l.iter().map(|x| x.1.clone()).collect::<Vec<_>>().join(", "))))
}

/// Объекты доски; у связей концы, привязанные к фигурам, получают текущие координаты (центр фигуры).
fn objs_of(st: &Value) -> Vec<Value> {
    let mut list = st["objs"].as_array().cloned().unwrap_or_default();
    let centers: std::collections::HashMap<String, (f64, f64)> = list
        .iter()
        .filter(|o| s(o, "t") != "link")
        .map(|o| {
            let b = bbox(o);
            (s(o, "id").to_string(), ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0))
        })
        .collect();
    for o in list.iter_mut().filter(|o| o["t"] == "link") {
        for k in ["a", "b"] {
            if let Some(&(x, y)) = o[k]["id"].as_str().and_then(|id| centers.get(id)) {
                o[k]["x"] = r1(x);
                o[k]["y"] = r1(y);
            }
        }
    }
    list
}

/// Фигуры (3.3): вид → внутреннее имя.
const SHAPE_KINDS: &[&str] = &["rect", "round", "ellipse", "diamond", "triangle", "hexagon", "parallelogram", "trapezoid", "terminal", "document", "database", "predefined", "manual", "delay", "offpage", "cloud", "callout", "star", "arrowR", "cross", "frame", "lane"];
fn shape_kind(v: &str) -> Result<&'static str, String> {
    let k = match v.trim().to_lowercase().as_str() {
        "" | "rectangle" | "box" | "process" | "прямоугольник" => "rect",
        "rounded" | "скруглённый" => "round",
        "circle" | "oval" | "эллипс" | "круг" => "ellipse",
        "decision" | "rhombus" | "ромб" => "diamond",
        "start" | "end" | "stadium" | "pill" => "terminal",
        "db" | "cylinder" | "storage" => "database",
        "io" | "input" | "output" => "parallelogram",
        "subprocess" => "predefined",
        "arrow" | "arrowr" => "arrowR",
        "plus" => "cross",
        "group" | "container" | "рамка" => "frame",
        "swimlane" | "дорожка" => "lane",
        "note" | "bubble" => "callout",
        o => return SHAPE_KINDS.iter().find(|k| k.to_lowercase() == o).copied().ok_or_else(|| format!("фигура «{}»: {}", v, SHAPE_KINDS.join(", "))),
    };
    Ok(k)
}
fn shape_size(k: &str) -> (f64, f64) {
    match k {
        "ellipse" => (140.0, 90.0),
        "diamond" => (150.0, 100.0),
        "triangle" => (130.0, 110.0),
        "hexagon" => (160.0, 90.0),
        "terminal" => (160.0, 64.0),
        "database" => (120.0, 110.0),
        "cloud" | "callout" => (180.0, 110.0),
        "star" => (120.0, 115.0),
        "cross" => (100.0, 100.0),
        "arrowR" => (170.0, 90.0),
        "frame" => (520.0, 340.0),
        "lane" => (760.0, 200.0),
        "document" => (160.0, 96.0),
        _ => (160.0, 80.0),
    }
}
fn head_kind(v: &str) -> Result<&'static str, String> {
    Ok(match v.trim().to_lowercase().as_str() {
        "" | "none" | "нет" => "none",
        "arrow" | "стрелка" => "arrow",
        "triangle" | "tri" => "tri",
        "diamond" | "ромб" => "diamond",
        "circle" | "dot" | "круг" => "circle",
        "one" => "one",
        "many" | "crow" => "many",
        _ => return Err(format!("конец «{}»: none, arrow, triangle, diamond, circle, one, many", v)),
    })
}
/// Конец связи: id объекта (строкой) или точка [x, y].
fn link_end(v: &Value, side: &str, st: &Value) -> Result<Value, (u16, String)> {
    if let Some(id) = v.as_str() {
        let id = id.trim();
        let o = objs_of(st).into_iter().find(|o| s(o, "id") == id).ok_or_else(|| (404, format!("связь: нет объекта {}", id)))?;
        if matches!(s(&o, "t"), "link" | "path" | "arrow") {
            return Err(bad("связь привязывается к фигуре, тексту, заметке, картинке — не к линиям"));
        }
        let b = bbox(&o);
        let mut e = json!({ "id": id, "x": r1((b[0] + b[2]) / 2.0), "y": r1((b[1] + b[3]) / 2.0) });
        match side {
            "" | "auto" => {}
            "t" | "top" => e["s"] = json!("t"),
            "r" | "right" => e["s"] = json!("r"),
            "b" | "bottom" => e["s"] = json!("b"),
            "l" | "left" => e["s"] = json!("l"),
            _ => return Err(bad("сторона: top, right, bottom, left или auto")),
        }
        return Ok(e);
    }
    match (v.get(0).and_then(|x| x.as_f64()), v.get(1).and_then(|x| x.as_f64())) {
        (Some(x), Some(y)) => Ok(json!({ "x": r1(x), "y": r1(y) })),
        _ => Err(bad("связь: from и to — id объекта или точка [x, y]")),
    }
}

fn light_bg(st: &Value) -> bool {
    let c = st["bg"]["c"].as_str().unwrap_or("");
    let l = st["bg"]["l"].as_f64().unwrap_or(100.0) / 100.0;
    let h = c.trim_start_matches('#');
    if h.len() != 6 {
        return l > 1.6;
    }
    let ch = |i: usize| {
        let v = u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap_or(0) as f64;
        if l <= 1.0 {
            v * l
        } else {
            v + (255.0 - v) * (l - 1.0)
        }
    };
    ch(0) * 0.299 + ch(1) * 0.587 + ch(2) * 0.114 > 150.0
}

/// Короткий вид объекта для ассистента: id, вид, слой, рамка и главное.
fn brief(o: &Value, layers: &[(String, String)]) -> Value {
    let b = bbox(o);
    let lid = o["L"].as_str().unwrap_or("base");
    let mut m = Map::new();
    m.insert("id".into(), o["id"].clone());
    m.insert("type".into(), json!(match s(o, "t") { "img" => "image", t => t }));
    m.insert("layer".into(), json!(lid));
    if let Some(n) = layers.iter().find(|x| x.0 == lid) {
        m.insert("layerName".into(), json!(n.1));
    }
    m.insert("box".into(), json!([r1(b[0]), r1(b[1]), r1(b[2]), r1(b[3])]));
    for (k, out) in [("text", "text"), ("c", "color"), ("bg", "fill"), ("f", "fill"), ("src", "src"), ("pdf", "pdf"), ("page", "page"), ("by", "by"), ("fs", "size")] {
        if let Some(v) = o.get(k).filter(|v| !v.is_null()) {
            m.insert(out.into(), v.clone());
        }
    }
    if s(o, "t") == "shape" {
        m.insert("kind".into(), json!(if s(o, "k").is_empty() { "rect" } else { s(o, "k") }));
    }
    if s(o, "t") == "link" {
        for (k, out) in [("a", "from"), ("b", "to")] {
            let e = &o[k];
            m.insert(out.into(), match e["id"].as_str() { Some(id) => json!(id), None => json!([e["x"], e["y"]]) });
            if let Some(sd) = e["s"].as_str() {
                m.insert(format!("{}Side", out), json!(match sd { "t" => "top", "r" => "right", "b" => "bottom", _ => "left" }));
            }
        }
        m.insert("kind".into(), json!(if s(o, "k").is_empty() { "straight" } else { s(o, "k") }));
        m.insert("start".into(), json!(if s(o, "ha").is_empty() { "none" } else { s(o, "ha") }));
        m.insert("end".into(), json!(if o.get("hb").is_none() { "arrow" } else { s(o, "hb") }));
    }
    if matches!(s(o, "t"), "text" | "note" | "shape" | "link") {
        m.insert("font".into(), json!(font_name(s(o, "ff"))));
        for (k, out) in [("fb", "bold"), ("fi", "italic"), ("fu", "underline")] {
            if o[k].as_bool().unwrap_or(false) {
                m.insert(out.into(), json!(true));
            }
        }
        if !s(o, "fa").is_empty() {
            m.insert("align".into(), o["fa"].clone());
        }
    }
    Value::Object(m)
}

// ───────────── чтение ─────────────

/// GET board/rooms — доски, которые видны с этого ПК: все каналы (свои и соседей).
pub async fn rooms(node: &Arc<Node>) -> Res {
    let me = node.node_name();
    let list: Vec<Value> = crate::media::all_channels(node)
        .into_iter()
        .map(|c| json!({ "room": format!("{}:{}", c.owner, c.id), "title": c.title, "owner": c.owner, "mine": bridge_core::eq_ic(&c.owner, &me), "members": c.members }))
        .collect();
    Ok(json!({ "rooms": list }))
}

/// GET board/describe?room — обзор: включена ли, кто правит, фон, слои (сколько объектов в каждом), границы, виды.
pub async fn describe(node: &Arc<Node>, q: &Value) -> Res {
    let (r, st) = open(node, s(q, "room")).await?;
    let objs = objs_of(&st);
    let layers = layers_of(&st);
    let mut by_type: Map<String, Value> = Map::new();
    let mut bounds = None;
    for o in &objs {
        let t = match s(o, "t") { "img" => "image", t => t };
        let n = by_type.get(t).and_then(|v| v.as_u64()).unwrap_or(0);
        by_type.insert(t.into(), json!(n + 1));
        bounds = Some(union(bounds, bbox(o)));
    }
    let ls: Vec<Value> = layers
        .iter()
        .enumerate()
        .map(|(i, (id, name))| json!({ "id": id, "name": name, "order": i, "objects": objs.iter().filter(|o| o["L"].as_str().unwrap_or("base") == id).count() }))
        .collect();
    Ok(json!({
        "room": r.key, "owner": r.owner, "enabled": st["enabled"], "edit": st["edit"], "bg": st["bg"], "lightBg": light_bg(&st),
        "layers": ls, "objects": objs.len(), "byType": by_type,
        "bounds": bounds.map(|b| json!([r1(b[0]), r1(b[1]), r1(b[2]), r1(b[3])])),
        "fonts": ["default (Manrope)", "simple (Golos Text)", "tech (Tektur)", "hand (Caveat)"],
    }))
}

/// GET board/find?room&q&type&layer&by&x0&y0&x1&y1&limit — поиск объектов.
pub async fn find(node: &Arc<Node>, q: &Value) -> Res {
    let (r, st) = open(node, s(q, "room")).await?;
    let layers = layers_of(&st);
    let text = s(q, "q").to_lowercase();
    let types: Vec<String> = s(q, "type").split(',').map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).map(|t| if t == "image" { "img".into() } else { t }).collect();
    let layer = if s(q, "layer").is_empty() { None } else { Some(layer_id(&st, s(q, "layer"))?) };
    let by = s(q, "by").to_lowercase();
    let area = match (num(q, "x0"), num(q, "y0"), num(q, "x1"), num(q, "y1")) {
        (Some(a), Some(b), Some(c), Some(d)) => Some([a.min(c), b.min(d), a.max(c), b.max(d)]),
        _ => None,
    };
    let limit = num(q, "limit").unwrap_or(100.0).clamp(1.0, 2000.0) as usize;
    let mut hits: Vec<Value> = objs_of(&st)
        .into_iter()
        .filter(|o| {
            if !types.is_empty() && !types.iter().any(|t| t == s(o, "t")) {
                return false;
            }
            if let Some(l) = &layer {
                if o["L"].as_str().unwrap_or("base") != l {
                    return false;
                }
            }
            if !by.is_empty() && s(o, "by").to_lowercase() != by {
                return false;
            }
            if !text.is_empty() {
                let hay = format!("{} {} {}", s(o, "text"), s(o, "pdf"), s(o, "id")).to_lowercase();
                if !hay.contains(&text) {
                    return false;
                }
            }
            if let Some(a) = area {
                let b = bbox(o);
                if b[2] < a[0] || b[0] > a[2] || b[3] < a[1] || b[1] > a[3] {
                    return false;
                }
            }
            true
        })
        .collect();
    let total = hits.len();
    // сверху вниз, слева направо — как читают
    hits.sort_by(|a, b| {
        let (x, y) = (bbox(a), bbox(b));
        (x[1], x[0]).partial_cmp(&(y[1], y[0])).unwrap_or(std::cmp::Ordering::Equal)
    });
    let list: Vec<Value> = hits.iter().take(limit).map(|o| brief(o, &layers)).collect();
    Ok(json!({ "room": r.key, "total": total, "objs": list }))
}

/// GET board/get?room&ids=a,b — объекты целиком (как хранятся) и их рамки.
pub async fn get(node: &Arc<Node>, q: &Value) -> Res {
    let (r, st) = open(node, s(q, "room")).await?;
    let ids: Vec<&str> = s(q, "ids").split(',').chain(s(q, "id").split(',')).map(|x| x.trim()).filter(|x| !x.is_empty()).collect();
    if ids.is_empty() {
        return Err(bad("нужен ids"));
    }
    let objs = objs_of(&st);
    let found: Vec<Value> = ids
        .iter()
        .filter_map(|id| objs.iter().find(|o| s(o, "id") == *id))
        .map(|o| {
            let b = bbox(o);
            json!({ "obj": o, "box": [r1(b[0]), r1(b[1]), r1(b[2]), r1(b[3])] })
        })
        .collect();
    let missing: Vec<&str> = ids.iter().copied().filter(|id| !objs.iter().any(|o| s(o, "id") == *id)).collect();
    Ok(json!({ "room": r.key, "objs": found, "missing": missing }))
}

// ───────────── запись ─────────────

async fn ops(node: &Arc<Node>, r: &Arc<Room>, st: &Value, list: Vec<Value>) -> Result<u64, (u16, String)> {
    if list.is_empty() {
        return Ok(st["seq"].as_u64().unwrap_or(0));
    }
    let needs_on = list.iter().any(|o| !matches!(s(o, "t"), "enable" | "edit" | "cur"));
    if needs_on && !st["enabled"].as_bool().unwrap_or(false) {
        return Err((409, "доска выключена — включите: POST board/enable {room, on: true}".into()));
    }
    board::op(node, r, CS, &list).await
}

/// Применить понятные поля к объекту (создание и правка). `fresh` — новый объект (ставит значения по умолчанию).
fn apply_fields(o: &mut Map<String, Value>, v: &Value, fresh: bool, light: bool) -> Result<(), (u16, String)> {
    let t = o.get("t").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let set = |o: &mut Map<String, Value>, k: &str, val: Value| {
        o.insert(k.into(), val);
    };
    for k in ["x", "y", "x2", "y2"] {
        if let Some(f) = num(v, k) {
            set(o, k, r1(f));
        }
    }
    if let (Some(a), Some(b)) = (v["from"].get(0).and_then(|x| x.as_f64()), v["from"].get(1).and_then(|x| x.as_f64())) {
        set(o, "x", r1(a));
        set(o, "y", r1(b));
    }
    if let (Some(a), Some(b)) = (v["to"].get(0).and_then(|x| x.as_f64()), v["to"].get(1).and_then(|x| x.as_f64())) {
        set(o, "x2", r1(a));
        set(o, "y2", r1(b));
    }
    if matches!(t.as_str(), "rect" | "img" | "note" | "shape") {
        for k in ["w", "h"] {
            if let Some(f) = num(v, k) {
                if t != "note" && f.abs() < 1.0 {
                    return Err(bad(format!("{}: слишком мало", k)));
                }
                set(o, k, r1(f));
            }
        }
    }
    if let Some(c) = v.get("color").and_then(|x| x.as_str()) {
        let c = color(c).map_err(bad)?;
        if t == "note" {
            set(o, "bg", json!(c));
        } else {
            set(o, "c", json!(c));
        }
    }
    if let Some(c) = v.get("textColor").and_then(|x| x.as_str()) {
        set(o, if matches!(t.as_str(), "shape" | "link") { "tc" } else { "c" }, json!(color(c).map_err(bad)?));
    }
    // (3.3) фигуры и связи
    if matches!(t.as_str(), "shape" | "link") {
        if let Some(d) = v.get("dash").and_then(|x| x.as_str()) {
            match d {
                "" | "solid" => {
                    o.remove("sd");
                }
                "dash" | "dashed" => set(o, "sd", json!("dash")),
                "dot" | "dotted" => set(o, "sd", json!("dot")),
                _ => return Err(bad("dash: solid, dash или dot")),
            }
        }
    }
    if t == "shape" {
        if let Some(k) = v.get("kind").or_else(|| v.get("shape")).and_then(|x| x.as_str()) {
            set(o, "k", json!(shape_kind(k).map_err(bad)?));
        }
        if let Some(a) = v.get("valign").and_then(|x| x.as_str()) {
            match a {
                "middle" | "" => {
                    o.remove("va");
                }
                "top" | "bottom" => set(o, "va", json!(a)),
                _ => return Err(bad("valign: top, middle или bottom")),
            }
        }
    }
    if t == "link" {
        if let Some(k) = v.get("kind").and_then(|x| x.as_str()) {
            match k {
                "straight" | "elbow" | "curve" => set(o, "k", json!(k)),
                _ => return Err(bad("kind связи: straight, elbow или curve")),
            }
        }
        for (k, key) in [("start", "ha"), ("end", "hb")] {
            if let Some(h) = v.get(k).and_then(|x| x.as_str()) {
                set(o, key, json!(head_kind(h).map_err(bad)?));
            }
        }
        if let Some(lt) = num(v, "labelAt") {
            set(o, "lt", json!((lt.clamp(0.0, 1.0) * 1000.0).round() / 1000.0));
        }
        if let Some(l) = v.get("label") {
            let text = l.as_str().ok_or_else(|| bad("label — строка"))?;
            if text.is_empty() {
                o.remove("text");
            } else {
                set(o, "text", json!(text));
            }
        }
    }
    if let Some(f) = v.get("fill") {
        if f.is_null() || f.as_str() == Some("") || f.as_bool() == Some(false) {
            o.remove("f");
        } else if let Some(c) = f.as_str() {
            let c = color(c).map_err(bad)?;
            set(o, if t == "note" { "bg" } else { "f" }, json!(c));
        }
    }
    if let Some(w) = num(v, "width") {
        let w = w.clamp(0.5, 200.0);
        set(o, if t == "path" { "w" } else { "sw" }, r1(w));
    }
    if let Some(a) = num(v, "opacity") {
        set(o, "a", json!((a.clamp(0.05, 1.0) * 100.0).round() / 100.0));
    }
    if matches!(t.as_str(), "text" | "note" | "shape" | "link") {
        if let Some(x) = v.get("text") {
            let text = x.as_str().ok_or_else(|| bad("text — строка"))?;
            if text.chars().count() > 20_000 {
                return Err(bad("текст больше 20000 знаков"));
            }
            set(o, "text", json!(text));
        }
        if let Some(f) = num(v, "size").or_else(|| num(v, "fs")) {
            set(o, "fs", json!(f.clamp(4.0, 2000.0).round()));
        }
        if let Some(f) = v.get("font").and_then(|x| x.as_str()) {
            let f = font(f).map_err(bad)?;
            if f.is_empty() {
                o.remove("ff");
            } else {
                set(o, "ff", json!(f));
            }
        }
        for (k, key) in [("bold", "fb"), ("italic", "fi"), ("underline", "fu")] {
            if let Some(b) = v.get(k).and_then(|x| x.as_bool()) {
                if b {
                    set(o, key, json!(true));
                } else {
                    o.remove(key);
                }
            }
        }
        if let Some(a) = v.get("align").and_then(|x| x.as_str()) {
            match a {
                "left" | "" => {
                    o.remove("fa");
                }
                "center" | "right" => set(o, "fa", json!(a)),
                _ => return Err(bad("align: left, center или right")),
            }
        }
    }
    if t == "path" {
        if let Some(p) = v.get("points") {
            let flat: Vec<f64> = match p.as_array() {
                Some(a) if a.first().map(|x| x.is_array()).unwrap_or(false) => a.iter().flat_map(|pt| [pt.get(0).and_then(|x| x.as_f64()), pt.get(1).and_then(|x| x.as_f64())]).flatten().collect(),
                Some(a) => a.iter().filter_map(|x| x.as_f64()).collect(),
                None => return Err(bad("points: [[x, y], …]")),
            };
            if flat.len() < 2 || flat.len() % 2 != 0 || flat.len() > 40_000 {
                return Err(bad("points: от 1 до 20000 точек [[x, y], …]"));
            }
            let (ox, oy) = (flat[0], flat[1]);
            let rel: Vec<Value> = flat.chunks(2).flat_map(|c| [r1(c[0] - ox), r1(c[1] - oy)]).collect();
            set(o, "x", r1(ox));
            set(o, "y", r1(oy));
            set(o, "pts", json!(rel));
        }
    }
    if fresh {
        let dflt_text = if light { DARK_TEXT } else { DEF_TEXT };
        match t.as_str() {
            "text" => {
                o.entry("c").or_insert(json!(dflt_text));
                o.entry("fs").or_insert(json!(24));
                o.entry("text").or_insert(json!(""));
            }
            "note" => {
                o.entry("bg").or_insert(json!(NOTE_BG));
                o.entry("fs").or_insert(json!(18));
                o.entry("w").or_insert(json!(240));
                o.entry("text").or_insert(json!(""));
            }
            "rect" => {
                o.entry("c").or_insert(json!(dflt_text));
                o.entry("sw").or_insert(json!(3));
                o.entry("w").or_insert(json!(200));
                o.entry("h").or_insert(json!(120));
            }
            "arrow" => {
                o.entry("c").or_insert(json!(dflt_text));
                o.entry("sw").or_insert(json!(3));
            }
            "path" => {
                o.entry("c").or_insert(json!(dflt_text));
                o.entry("w").or_insert(json!(3));
            }
            "shape" => {
                let k = o.get("k").and_then(|x| x.as_str()).unwrap_or("rect").to_string();
                let (w, h) = shape_size(&k);
                o.entry("k").or_insert(json!("rect"));
                o.entry("c").or_insert(json!(dflt_text));
                if k != "frame" && k != "lane" {
                    o.entry("f").or_insert(json!(if light { "#FFFFFF" } else { "#1C2140" }));
                } else {
                    o.entry("fb").or_insert(json!(true));
                }
                o.entry("sw").or_insert(json!(2));
                o.entry("w").or_insert(json!(w));
                o.entry("h").or_insert(json!(h));
                o.entry("fs").or_insert(json!(if k == "frame" || k == "lane" { 16 } else { 18 }));
            }
            "link" => {
                o.entry("c").or_insert(json!(dflt_text));
                o.entry("sw").or_insert(json!(2));
                o.entry("k").or_insert(json!("straight"));
                o.entry("ha").or_insert(json!("none"));
                o.entry("hb").or_insert(json!("arrow"));
                o.entry("fs").or_insert(json!(15));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Место для объекта без координат: справа от всего, что на доске (столбиком сверху вниз), или по `at`.
struct Placer {
    x: f64,
    y: f64,
    row_h: f64,
    col_x: f64,
    gap: f64,
    layout: String,
    max_w: f64,
}

impl Placer {
    fn new(st: &Value, v: &Value) -> Placer {
        let gap = num(v, "gap").unwrap_or(40.0).clamp(0.0, 2000.0);
        let (x, y) = match (v["at"].get(0).and_then(|x| x.as_f64()), v["at"].get(1).and_then(|x| x.as_f64())) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                let mut b = None;
                for o in objs_of(st) {
                    b = Some(union(b, bbox(&o)));
                }
                match b {
                    Some(b) => (b[2] + 120.0, b[1]),
                    None => (0.0, 0.0),
                }
            }
        };
        let layout = match s(v, "layout") { "column" | "grid" => s(v, "layout").to_string(), _ => "row".to_string() };
        Placer { x, y, row_h: 0.0, col_x: x, gap, layout, max_w: num(v, "rowWidth").unwrap_or(2400.0).max(200.0) }
    }
    fn place(&mut self, w: f64, h: f64) -> (f64, f64) {
        match self.layout.as_str() {
            "column" => {
                let p = (self.x, self.y);
                self.y += h + self.gap;
                p
            }
            "grid" => {
                if self.x > self.col_x && self.x + w > self.col_x + self.max_w {
                    self.x = self.col_x;
                    self.y += self.row_h + self.gap;
                    self.row_h = 0.0;
                }
                let p = (self.x, self.y);
                self.x += w + self.gap;
                self.row_h = self.row_h.max(h);
                p
            }
            _ => {
                let p = (self.x, self.y);
                self.x += w + self.gap;
                p
            }
        }
    }
}

/// POST board/add {room, objs: [...], layer?, at?: [x, y], layout?: row|column|grid, gap?} — положить объекты.
/// Объект: {type: text|note|rect|arrow|line|path|image|pdf, …}. Без x, y объекты встают рядом с тем, что уже есть.
pub async fn add(node: &Arc<Node>, v: &Value) -> Res {
    let (r, st) = open(node, s(v, "room")).await?;
    let list: Vec<Value> = match (v.get("objs").and_then(|x| x.as_array()), v.get("obj")) {
        (Some(a), _) => a.clone(),
        (None, Some(o)) if o.is_object() => vec![o.clone()],
        _ if v.get("type").is_some() => vec![v.clone()],
        _ => return Err(bad("нужен objs: [{type, …}] (или один obj)")),
    };
    if list.is_empty() || list.len() > 500 {
        return Err(bad("objs: от 1 до 500 объектов за раз"));
    }
    if !st["enabled"].as_bool().unwrap_or(false) {
        return Err((409, "доска выключена — включите: POST board/enable {room, on: true}".into()));
    }
    let light = light_bg(&st);
    let mut placer = Placer::new(&st, v);
    let existing: std::collections::HashSet<String> = objs_of(&st).iter().map(|o| s(o, "id").to_string()).collect();
    let mut put = Vec::new();
    let mut made = Vec::new();
    let mut pdfs = Vec::new();
    let mut diagrams = Vec::new();
    let z0 = now_ms();
    for (i, item) in list.iter().enumerate() {
        let kind = s(item, "type").to_lowercase();
        let kind = if kind.is_empty() { s(item, "t").to_lowercase() } else { kind };
        let layer = layer_id(&st, if s(item, "layer").is_empty() { s(v, "layer") } else { s(item, "layer") })?;
        let id = if s(item, "id").is_empty() { new_id(i) } else { s(item, "id").to_string() };
        if !valid_id(&id) {
            return Err(bad(format!("id «{}»: латиница, цифры, - и _ (до 64)", id)));
        }
        if existing.contains(&id) {
            return Err((409, format!("объект {} уже есть — правка: POST board/edit", id)));
        }
        if kind == "pdf" {
            pdfs.push((item.clone(), layer));
            continue;
        }
        if matches!(kind.as_str(), "diagram" | "mermaid") {
            diagrams.push((item.clone(), layer));
            continue;
        }
        let mut item = item.clone();
        let t = match kind.as_str() {
            "text" | "note" | "rect" | "arrow" | "path" | "shape" | "link" => kind.clone(),
            "frame" | "lane" => {
                item["kind"] = json!(kind);
                "shape".into()
            }
            "connector" | "edge" => "link".into(),
            "line" => "path".into(),
            "image" | "img" | "picture" => "img".into(),
            "" => return Err(bad(format!("объект {}: нужен type (text, note, shape, link, frame, rect, arrow, line, path, image, pdf, diagram)", i + 1))),
            _ => return Err(bad(format!("type «{}»: text, note, shape, link, frame, rect, arrow, line, path, image, pdf, diagram", kind))),
        };
        let mut o = Map::new();
        o.insert("id".into(), json!(id));
        o.insert("t".into(), json!(t));
        o.insert("L".into(), json!(layer));
        o.insert("z".into(), json!(z0 + i as f64));
        // связь: концы — id объектов (в том числе только что добавленных) или точки
        if t == "link" {
            let mut st2 = st.clone();
            if let Some(a) = st2["objs"].as_array_mut() {
                a.extend(made.iter().cloned());
            }
            let from = item.get("from").cloned().ok_or_else(|| bad("link: нужны from и to (id объекта или [x, y])"))?;
            let to = item.get("to").cloned().ok_or_else(|| bad("link: нужны from и to (id объекта или [x, y])"))?;
            o.insert("a".into(), link_end(&from, s(&item, "fromSide"), &st2)?);
            o.insert("b".into(), link_end(&to, s(&item, "toSide"), &st2)?);
            item.as_object_mut().map(|m| { m.remove("from"); m.remove("to"); m.remove("x"); m.remove("y"); });
        }
        if t == "shape" && item.get("kind").is_none() && item.get("shape").is_none() {
            o.insert("k".into(), json!("rect"));
        }
        if kind == "line" && item.get("points").is_none() {
            let (x, y) = (num(&item, "x").unwrap_or(0.0), num(&item, "y").unwrap_or(0.0));
            let (x2, y2) = (num(&item, "x2").ok_or_else(|| bad("line: нужны x, y, x2, y2"))?, num(&item, "y2").ok_or_else(|| bad("line: нужны x, y, x2, y2"))?);
            item["points"] = json!([[x, y], [x2, y2]]);
            item.as_object_mut().map(|m| { m.remove("x"); m.remove("y"); m.remove("x2"); m.remove("y2"); });
        }
        if t == "img" {
            let (src, nw, nh) = if !s(&item, "src").is_empty() {
                (s(&item, "src").to_string(), num(&item, "w").unwrap_or(400.0), num(&item, "h").unwrap_or(300.0))
            } else {
                let (b, _) = bytes_of(&item, board::IMG_MAX)?.ok_or_else(|| bad("image: нужен path (файл на этом ПК), data (base64) или src (id картинки доски)"))?;
                let (w, h) = img_size(&b).ok_or_else(|| bad("image: нужна картинка PNG, JPEG, GIF или WebP"))?;
                let id = board::put_image(node, &r, &b).await?;
                (id, w as f64, h as f64)
            };
            o.insert("src".into(), json!(src));
            // размер: w и/или h (пропорции сохраняются), иначе по картинке, но не больше maxSize (800)
            let max = num(&item, "maxSize").unwrap_or(800.0).max(16.0);
            let (w, h) = match (num(&item, "w"), num(&item, "h")) {
                (Some(w), Some(h)) => (w, h),
                (Some(w), None) => (w, w * nh / nw.max(1.0)),
                (None, Some(h)) => (h * nw / nh.max(1.0), h),
                _ => {
                    let k = (max / nw.max(nh).max(1.0)).min(1.0);
                    (nw * k, nh * k)
                }
            };
            o.insert("w".into(), r1(w));
            o.insert("h".into(), r1(h));
            item.as_object_mut().map(|m| { m.remove("w"); m.remove("h"); });
        }
        apply_fields(&mut o, &item, true, light)?;
        if t == "arrow" && (o.get("x2").is_none() || o.get("y2").is_none()) {
            return Err(bad("arrow: нужны x, y, x2, y2 (или from: [x, y], to: [x, y])"));
        }
        if t == "path" && o.get("pts").is_none() {
            return Err(bad("path: нужен points: [[x, y], …]"));
        }
        // нет координат — поставить рядом с тем, что есть
        let has_xy = item.get("x").is_some() || item.get("points").is_some() || item.get("from").is_some() || t == "link";
        if !has_xy {
            let mut ov = Value::Object(o.clone());
            ov["x"] = json!(0);
            ov["y"] = json!(0);
            let b = bbox(&ov);
            let (px, py) = placer.place(b[2] - b[0], b[3] - b[1]);
            let (dx, dy) = (px - b[0], py - b[1]);
            o.insert("x".into(), r1(dx));
            o.insert("y".into(), r1(dy));
            if t == "arrow" {
                let (x2, y2) = (num(&ov, "x2").unwrap_or(0.0), num(&ov, "y2").unwrap_or(0.0));
                o.insert("x2".into(), r1(x2 + dx));
                o.insert("y2".into(), r1(y2 + dy));
            }
        }
        let ov = Value::Object(o);
        made.push(ov.clone());
        put.push(json!({ "t": "put", "o": ov }));
    }
    let seq = ops(node, &r, &st, put).await?;
    let layers = layers_of(&st);
    let mut out: Vec<Value> = made.iter().map(|o| brief(o, &layers)).collect();
    // Mermaid: разбирает и раскладывает панель (меряет текст), объекты кладёт на доску
    for (item, layer) in diagrams {
        let text = item.get("mermaid").or_else(|| item.get("text")).and_then(|x| x.as_str()).unwrap_or("").to_string();
        if text.trim().is_empty() {
            return Err(bad("diagram: нужен mermaid — текст flowchart (например «flowchart TD\n A[Начало] --> B{Решение}»)"));
        }
        if text.len() > 100_000 {
            return Err(bad("diagram: текст больше 100 КБ"));
        }
        let (x, y) = match (num(&item, "x"), num(&item, "y")) {
            (Some(x), Some(y)) => (x, y),
            _ => placer.place(800.0, 600.0),
        };
        let mut args = json!({ "room": r.key, "text": text, "x": x, "y": y, "layer": layer, "color": if light { DARK_TEXT } else { DEF_TEXT }, "fill": if light { "#FFFFFF" } else { "#1C2140" } });
        if let Some(c) = item.get("color").and_then(|x| x.as_str()) {
            args["color"] = json!(color(c).map_err(bad)?);
        }
        if let Some(f) = item.get("fill").and_then(|x| x.as_str()) {
            args["fill"] = json!(if f.is_empty() { String::new() } else { color(f).map_err(bad)? });
        }
        let res = crate::media::ui_command(node, "board_mermaid", args, std::time::Duration::from_secs(60)).await.map_err(|(c, e)| (c, format!("diagram: {}", e)))?;
        let mut all = objs_of(&st);
        all.extend(res["objs"].as_array().cloned().unwrap_or_default());
        let fake = json!({ "objs": all });
        for o in objs_of(&fake).into_iter().filter(|o| res["objs"].as_array().map(|a| a.iter().any(|x| x["id"] == o["id"])).unwrap_or(false)) {
            out.push(brief(&o, &layers));
        }
    }
    // PDF рисует панель (pdf.js): страницы → картинки на этой доске
    for (item, layer) in pdfs {
        let (b, name) = bytes_of(&item, 60 * 1024 * 1024)?.ok_or_else(|| bad("pdf: нужен path (файл на этом ПК) или data (base64)"))?;
        if !b.starts_with(b"%PDF") {
            return Err(bad("pdf: это не PDF"));
        }
        let tmp = format!("{}.pdf", uuid::Uuid::new_v4().simple());
        let dir = node.data_dir.join("tmp").join("board");
        std::fs::create_dir_all(&dir).map_err(|e| (500, e.to_string()))?;
        std::fs::write(dir.join(&tmp), &b).map_err(|e| (500, e.to_string()))?;
        let (x, y) = match (num(&item, "x"), num(&item, "y")) {
            (Some(x), Some(y)) => (x, y),
            _ => placer.place(800.0, 1100.0),
        };
        let args = json!({ "room": r.key, "tmp": tmp, "name": if s(&item, "name").is_empty() { name } else { s(&item, "name").to_string() }, "x": x, "y": y, "layer": layer, "w": num(&item, "w").unwrap_or(800.0), "pages": item["pages"].clone(), "maxPages": num(&item, "maxPages").unwrap_or(40.0) });
        let res = crate::media::ui_command(node, "board_pdf", args, std::time::Duration::from_secs(240)).await;
        let _ = std::fs::remove_file(dir.join(&tmp));
        let res = res.map_err(|(c, e)| (c, format!("PDF: {}", e)))?;
        for o in res["objs"].as_array().cloned().unwrap_or_default() {
            out.push(brief(&o, &layers));
        }
    }
    Ok(json!({ "room": r.key, "seq": seq, "objs": out }))
}

/// Временный PDF для панели (board_pdf): отдать и забыть.
pub fn tmp_pdf(node: &Node, id: &str) -> Option<Vec<u8>> {
    if !id.ends_with(".pdf") || id.len() != 36 || !id[..32].chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    std::fs::read(node.data_dir.join("tmp").join("board").join(id)).ok()
}

fn ids_of(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(a) = v["ids"].as_array() {
        out.extend(a.iter().filter_map(|x| x.as_str().map(String::from)));
    } else if let Some(t) = v["ids"].as_str() {
        out.extend(t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()));
    }
    if let Some(t) = v["id"].as_str() {
        out.push(t.to_string());
    }
    out
}

fn pick(st: &Value, ids: &[String]) -> Result<Vec<Value>, (u16, String)> {
    if ids.is_empty() {
        return Err(bad("нужен id или ids"));
    }
    let objs = objs_of(st);
    let mut out = Vec::new();
    for id in ids {
        out.push(objs.iter().find(|o| s(o, "id") == id).cloned().ok_or_else(|| (404, format!("нет объекта {}", id)))?);
    }
    Ok(out)
}

/// POST board/edit {room, id | ids, …поля} — поменять текст, цвет, размер, шрифт, стиль, рамку, слой.
pub async fn edit(node: &Arc<Node>, v: &Value) -> Res {
    let (r, st) = open(node, s(v, "room")).await?;
    let list = pick(&st, &ids_of(v))?;
    let light = light_bg(&st);
    let layer = if s(v, "layer").is_empty() { None } else { Some(layer_id(&st, s(v, "layer"))?) };
    let mut put = Vec::new();
    let mut made = Vec::new();
    for o in list {
        let mut m = o.as_object().cloned().unwrap_or_default();
        m.remove("by");
        if s(&o, "t") == "img" && (v.get("path").is_some() || v.get("data").is_some()) {
            if let Some((b, _)) = bytes_of(v, board::IMG_MAX)? {
                img_size(&b).ok_or_else(|| bad("нужна картинка PNG, JPEG, GIF или WebP"))?;
                m.insert("src".into(), json!(board::put_image(node, &r, &b).await?));
            }
        }
        if s(&o, "t") == "link" {
            for (k, key, side) in [("from", "a", "fromSide"), ("to", "b", "toSide")] {
                if let Some(e) = v.get(k) {
                    m.insert(key.into(), link_end(e, s(v, side), &st)?);
                } else if v.get(side).is_some() {
                    let mut e = m.get(key).cloned().unwrap_or(Value::Null);
                    if e["id"].is_string() {
                        e = link_end(&e["id"], s(v, side), &st)?;
                        m.insert(key.into(), e);
                    }
                }
            }
            let mut v2 = v.clone();
            v2.as_object_mut().map(|x| { x.remove("from"); x.remove("to"); x.remove("x"); x.remove("y"); });
            apply_fields(&mut m, &v2, false, light)?;
        } else {
            apply_fields(&mut m, v, false, light)?;
        }
        if let Some(l) = &layer {
            m.insert("L".into(), json!(l));
        }
        if v["toFront"].as_bool().unwrap_or(false) {
            m.insert("z".into(), json!(now_ms()));
        }
        let ov = Value::Object(m);
        made.push(ov.clone());
        put.push(json!({ "t": "put", "o": ov }));
    }
    let seq = ops(node, &r, &st, put).await?;
    let layers = layers_of(&st);
    Ok(json!({ "room": r.key, "seq": seq, "objs": made.iter().map(|o| brief(o, &layers)).collect::<Vec<_>>() }))
}

/// POST board/move {room, ids, dx, dy} — сдвинуть; {room, ids, x, y} — поставить левый верхний угол группы в точку.
pub async fn move_(node: &Arc<Node>, v: &Value) -> Res {
    let (r, st) = open(node, s(v, "room")).await?;
    let list = pick(&st, &ids_of(v))?;
    let (dx, dy) = match (num(v, "dx"), num(v, "dy"), num(v, "x"), num(v, "y")) {
        (_, _, Some(x), Some(y)) => {
            let mut b = None;
            for o in &list {
                b = Some(union(b, bbox(o)));
            }
            let b = b.unwrap_or([0.0; 4]);
            (x - b[0], y - b[1])
        }
        (dx, dy, _, _) if dx.is_some() || dy.is_some() => (dx.unwrap_or(0.0), dy.unwrap_or(0.0)),
        _ => return Err(bad("нужны dx, dy (сдвиг) или x, y (куда поставить)")),
    };
    let mut put = Vec::new();
    let mut made = Vec::new();
    for o in list {
        let mut o = o;
        o.as_object_mut().map(|m| m.remove("by"));
        if o["t"] == "link" {
            // привязанные концы едут с фигурами сами, свободные — сдвигаем
            for k in ["a", "b"] {
                if o[k]["id"].is_null() {
                    if let (Some(x), Some(y)) = (num(&o[k], "x"), num(&o[k], "y")) {
                        o[k]["x"] = r1(x + dx);
                        o[k]["y"] = r1(y + dy);
                    }
                }
            }
        } else {
            for (k, d) in [("x", dx), ("y", dy), ("x2", dx), ("y2", dy)] {
                if let Some(f) = num(&o, k) {
                    o[k] = r1(f + d);
                }
            }
        }
        made.push(o.clone());
        put.push(json!({ "t": "put", "o": o }));
    }
    let seq = ops(node, &r, &st, put).await?;
    let layers = layers_of(&st);
    Ok(json!({ "room": r.key, "seq": seq, "objs": made.iter().map(|o| brief(o, &layers)).collect::<Vec<_>>() }))
}

/// POST board/remove {room, ids}
pub async fn remove(node: &Arc<Node>, v: &Value) -> Res {
    let (r, st) = open(node, s(v, "room")).await?;
    let mut ids = ids_of(v);
    pick(&st, &ids)?;
    // связи удалённых фигур — тоже
    for o in objs_of(&st) {
        if o["t"] == "link" && !ids.iter().any(|i| i == s(&o, "id")) && ["a", "b"].iter().any(|k| o[*k]["id"].as_str().map(|x| ids.iter().any(|i| i == x)).unwrap_or(false)) {
            ids.push(s(&o, "id").to_string());
        }
    }
    let seq = ops(node, &r, &st, vec![json!({ "t": "del", "ids": ids })]).await?;
    Ok(json!({ "room": r.key, "seq": seq, "removed": ids }))
}

/// POST board/clear {room, layer?} — стереть всё или только один слой.
pub async fn clear(node: &Arc<Node>, v: &Value) -> Res {
    let (r, st) = open(node, s(v, "room")).await?;
    if s(v, "layer").is_empty() {
        let n = objs_of(&st).len();
        let seq = ops(node, &r, &st, vec![json!({ "t": "clear" })]).await?;
        return Ok(json!({ "room": r.key, "seq": seq, "removed": n }));
    }
    let l = layer_id(&st, s(v, "layer"))?;
    let ids: Vec<String> = objs_of(&st).iter().filter(|o| o["L"].as_str().unwrap_or("base") == l).map(|o| s(o, "id").to_string()).collect();
    let n = ids.len();
    let seq = ops(node, &r, &st, if ids.is_empty() { vec![] } else { vec![json!({ "t": "del", "ids": ids })] }).await?;
    Ok(json!({ "room": r.key, "seq": seq, "removed": n }))
}

/// POST board/layer {room, action: add|rename|remove|up|down|top|bottom, id?|name?, to?} — слои (порядок снизу вверх).
pub async fn layer(node: &Arc<Node>, v: &Value) -> Res {
    let (r, st) = open(node, s(v, "room")).await?;
    let mut l = layers_of(&st);
    let action = s(v, "action");
    let mut extra = vec![];
    let made;
    match action {
        "add" => {
            if l.len() >= 64 {
                return Err(bad("слоёв не больше 64"));
            }
            let id = if valid_id(s(v, "id")) { s(v, "id").to_string() } else { format!("api{}", &uuid::Uuid::new_v4().simple().to_string()[..10]) };
            if l.iter().any(|x| x.0 == id) {
                return Err((409, format!("слой {} уже есть", id)));
            }
            let name: String = if s(v, "name").is_empty() { format!("Слой {}", l.len() + 1) } else { s(v, "name").chars().take(40).collect() };
            l.push((id.clone(), name));
            made = id;
        }
        "rename" | "remove" | "up" | "down" | "top" | "bottom" => {
            let want = if s(v, "id").is_empty() { s(v, "layer") } else { s(v, "id") };
            let want = if want.is_empty() { s(v, "name") } else { want };
            let id = layer_id(&st, want)?;
            let i = l.iter().position(|x| x.0 == id).unwrap_or(0);
            match action {
                "rename" => {
                    let to = if s(v, "to").is_empty() { s(v, "newName") } else { s(v, "to") };
                    if to.trim().is_empty() {
                        return Err(bad("rename: нужен to (новое название)"));
                    }
                    l[i].1 = to.chars().take(40).collect();
                }
                "remove" => {
                    if l.len() < 2 {
                        return Err(bad("последний слой не удалить"));
                    }
                    let ids: Vec<String> = objs_of(&st).iter().filter(|o| o["L"].as_str().unwrap_or("base") == id).map(|o| s(o, "id").to_string()).collect();
                    if !ids.is_empty() {
                        extra.push(json!({ "t": "del", "ids": ids }));
                    }
                    l.remove(i);
                }
                "up" if i + 1 < l.len() => l.swap(i, i + 1),
                "down" if i > 0 => l.swap(i, i - 1),
                "top" => {
                    let x = l.remove(i);
                    l.push(x);
                }
                "bottom" => {
                    let x = l.remove(i);
                    l.insert(0, x);
                }
                _ => {}
            }
            made = id;
        }
        _ => return Err(bad("action: add, rename, remove, up, down, top, bottom")),
    }
    let list: Vec<Value> = l.iter().map(|(id, name)| json!({ "id": id, "name": name })).collect();
    let mut all = extra;
    all.push(json!({ "t": "layers", "list": list }));
    let seq = ops(node, &r, &st, all).await?;
    Ok(json!({ "room": r.key, "seq": seq, "layer": made, "layers": list }))
}

/// POST board/bg {room, color?, light?} — фон: цвет (пусто — по умолчанию) и яркость 20–200 %.
pub async fn bg(node: &Arc<Node>, v: &Value) -> Res {
    let (r, st) = open(node, s(v, "room")).await?;
    let c = match v.get("color").and_then(|x| x.as_str()) {
        Some("") | Some("default") => String::new(),
        Some(c) => color(c).map_err(bad)?,
        None => st["bg"]["c"].as_str().unwrap_or("").to_string(),
    };
    let l = num(v, "light").unwrap_or(st["bg"]["l"].as_f64().unwrap_or(100.0)).clamp(20.0, 200.0).round();
    let seq = ops(node, &r, &st, vec![json!({ "t": "bg", "c": c, "l": l })]).await?;
    Ok(json!({ "room": r.key, "seq": seq, "bg": { "c": c, "l": l } }))
}

/// POST board/enable {room, on}
pub async fn enable(node: &Arc<Node>, v: &Value) -> Res {
    let (r, st) = open(node, s(v, "room")).await?;
    let on = v["on"].as_bool().unwrap_or(true);
    let seq = ops(node, &r, &st, vec![json!({ "t": "enable", "on": on })]).await?;
    Ok(json!({ "room": r.key, "seq": seq, "enabled": on }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_colors() {
        // PNG 3×2
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&3u32.to_be_bytes());
        png.extend_from_slice(&2u32.to_be_bytes());
        assert_eq!(img_size(&png), Some((3, 2)));
        let gif = b"GIF89a\x05\x00\x07\x00".to_vec();
        assert_eq!(img_size(&gif), Some((5, 7)));
        assert_eq!(color("red").unwrap(), "#FF4F64");
        assert_eq!(color("#abc").unwrap(), "#AABBCC");
        assert!(color("nope").is_err());
        assert_eq!(font("Техно").unwrap(), "tech");
        let b = bbox(&json!({ "t": "arrow", "x": 10, "y": 50, "x2": 0, "y2": 0 }));
        assert_eq!(b, [0.0, 0.0, 10.0, 50.0]);
    }
}
