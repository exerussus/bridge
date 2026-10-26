//! Журнал активности (3.5): что хозяин этого ПК делает в мосте — минуты в канале и разговора, сообщения в чатах,
//! показ экрана, правки доски, пинги, отправки. Только факты, без содержимого (ни текста, ни звука, ни файлов).
//! Пишет только мост; приложения с правом `activity` читают (игре — начислять награды, в том числе за время,
//! пока она была закрыта). Хранится 30 дней: bridge-data/activity/<дата>.jsonl.

use crate::node::Node;
use bridge_core::time;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

const KEEP_DAYS: i64 = 30;
pub const KINDS: &[&str] = &["voice.minute", "chat.message", "screen.minute", "board.edit", "ping", "files.sent", "channel.join"];

#[derive(Default)]
pub struct Activity {
    lock: Mutex<()>,
    /// Последний номер события (растёт всегда; по нему приложения забирают новое).
    seq: Mutex<Option<u64>>,
    /// Правки доски — не чаще раза в минуту.
    board_at: Mutex<Option<std::time::Instant>>,
}

fn dir(node: &Node) -> PathBuf {
    node.data_dir.join("activity")
}

fn day_file(node: &Node, day: &str) -> PathBuf {
    dir(node).join(format!("{}.jsonl", day))
}

fn last_seq(node: &Node) -> u64 {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir(node)).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false)).collect()).unwrap_or_default();
    files.sort();
    for f in files.iter().rev() {
        if let Some(l) = std::fs::read_to_string(f).ok().and_then(|t| t.lines().filter(|l| !l.trim().is_empty()).last().map(String::from)) {
            if let Some(s) = serde_json::from_str::<Value>(&l).ok().and_then(|v| v["seq"].as_u64()) {
                return s;
            }
        }
    }
    0
}

/// Записать событие. `data` — поля события (без содержимого).
pub fn record(node: &Node, kind: &str, data: Value) -> Option<Value> {
    if !KINDS.contains(&kind) {
        return None;
    }
    if kind == "board.edit" {
        let mut g = node.activity.board_at.lock();
        if g.map(|t| t.elapsed().as_secs() < 60).unwrap_or(false) {
            return None;
        }
        *g = Some(std::time::Instant::now());
    }
    let _g = node.activity.lock.lock();
    let mut seq = node.activity.seq.lock();
    let n = seq.unwrap_or_else(|| last_seq(node)) + 1;
    *seq = Some(n);
    let now = chrono::Local::now();
    let mut e = json!({ "seq": n, "at": time::format(&time::now()), "kind": kind });
    if let Some(o) = data.as_object() {
        for (k, v) in o {
            if k != "seq" && k != "at" && k != "kind" {
                e[k] = v.clone();
            }
        }
    }
    let _ = std::fs::create_dir_all(dir(node));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(day_file(node, &now.format("%Y-%m-%d").to_string())) {
        let _ = writeln!(f, "{}", e);
    }
    drop(seq);
    drop(_g);
    crate::net::activity_event(node, &e);
    node.media.deliver(json!({ "media": "apps", "type": "activity", "e": e }));
    Some(e)
}

/// События после `cursor` (номер), до `limit` штук, старые — первыми.
pub fn since(node: &Node, cursor: u64, limit: usize) -> Value {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir(node)).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false)).collect()).unwrap_or_default();
    files.sort();
    let mut out: Vec<Value> = Vec::new();
    let mut more = false;
    for f in files {
        let Ok(t) = std::fs::read_to_string(&f) else { continue };
        for l in t.lines() {
            let Ok(v) = serde_json::from_str::<Value>(l) else { continue };
            if v["seq"].as_u64().unwrap_or(0) <= cursor {
                continue;
            }
            if out.len() >= limit {
                more = true;
                break;
            }
            out.push(v);
        }
        if more {
            break;
        }
    }
    let next = out.last().and_then(|v| v["seq"].as_u64()).unwrap_or(cursor);
    json!({ "events": out, "cursor": next, "more": more })
}

/// Итоги за сегодня или неделю: минуты в канале и разговора, сообщения и прочее.
pub fn stats(node: &Node, period: &str) -> Value {
    let days = if period == "week" { 7 } else { 1 };
    let today = chrono::Local::now().date_naive();
    let mut minutes = 0u64;
    let mut talk = 0u64;
    let mut counts = serde_json::Map::new();
    for d in 0..days {
        let day = today - chrono::Duration::days(d);
        let Ok(t) = std::fs::read_to_string(day_file(node, &day.format("%Y-%m-%d").to_string())) else { continue };
        for l in t.lines() {
            let Ok(v) = serde_json::from_str::<Value>(l) else { continue };
            let k = v["kind"].as_str().unwrap_or("").to_string();
            if k == "voice.minute" {
                minutes += 1;
                talk += v["talkSec"].as_u64().unwrap_or(0);
            }
            let c = counts.entry(k).or_insert(json!(0));
            *c = json!(c.as_u64().unwrap_or(0) + 1);
        }
    }
    json!({ "period": if days == 7 { "week" } else { "today" }, "voiceMinutes": minutes, "talkMinutes": talk / 60, "talkSeconds": talk, "counts": counts })
}

/// Убрать журналы старше 30 дней.
pub fn cleanup(node: &Node) {
    let cut = chrono::Local::now().date_naive() - chrono::Duration::days(KEEP_DAYS);
    if let Ok(rd) = std::fs::read_dir(dir(node)) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(d) = name.strip_suffix(".jsonl").and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()) {
                if d < cut {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}
