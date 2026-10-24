//! Свёртка событий записи в состояние. Чистая функция: одинаковый набор событий даёт одинаковое состояние на любом узле.
//! Порядок — (clock, author, seq). Событие, нарушающее правила типа, не применяется и попадает в invalid.

use crate::proto::{ops, record_types as rt, *};
use crate::{cmp_ordinal, eq_ic};
use serde_json::Value;
use std::cmp::Ordering;

pub fn compare(a: &BridgeEvent, b: &BridgeEvent) -> Ordering {
    a.clock.cmp(&b.clock).then_with(|| cmp_ordinal(&a.author, &b.author)).then_with(|| a.seq.cmp(&b.seq))
}

/// Состояние записи (до версии `up_to` включительно, если задана).
pub fn fold(mut events: Vec<BridgeEvent>, up_to: Option<i64>) -> Option<RecordState> {
    events.sort_by(compare);
    let mut s: Option<RecordState> = None;
    for e in &events {
        if let (Some(v), Some(st)) = (up_to, &s) {
            if st.version >= v {
                break;
            }
        }
        let st = s.get_or_insert_with(|| RecordState { id: e.record.clone(), share: e.share.clone(), ..Default::default() });
        let _ = apply(st, e);
    }
    if let Some(st) = s.as_mut() {
        st.sync_derived();
    }
    s
}

fn str_of(p: &JsonObject, k: &str) -> String {
    match p.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn text_of(p: &JsonObject, k: &str) -> Option<String> {
    match p.get(k) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(v) => Some(v.to_string()),
    }
}

fn rel_of(p: &JsonObject) -> String {
    match p.get("rel") {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        _ => "relates".into(),
    }
}

/// Применить одно событие (с проверкой правил). Err — причина отказа.
pub fn apply(s: &mut RecordState, e: &BridgeEvent) -> Result<(), String> {
    let why = check(s, e);
    s.version += 1;
    s.last_eid = e.eid.clone();
    if let Err(w) = why {
        s.invalid.push(InvalidEvent { eid: e.eid.clone(), reason: w.clone() });
        return Err(w);
    }
    s.updated = e.time;
    let empty = JsonObject::new();
    let p = e.payload.as_ref().unwrap_or(&empty);
    match e.op.as_str() {
        ops::CREATE => {
            s.type_ = e.type_.clone();
            s.schema = e.schema.clone();
            s.creator = e.author.clone();
            s.creator_actor = e.actor.clone();
            s.created = e.time;
            if let Some(Value::Object(f)) = p.get("fields") {
                for (k, v) in f {
                    set_field(s, k, Some(v.clone()), e, false);
                }
            }
            if let Some(initial) = initial_status(&e.type_) {
                if s.fields.get("status").map(|v| v.is_null()).unwrap_or(true) {
                    set_field(s, "status", Some(Value::String(initial.into())), e, false);
                }
            }
        }
        ops::SET => {
            if e.schema.is_some() {
                s.schema = e.schema.clone();
            }
            if let Some(Value::Object(f)) = p.get("fields") {
                for (k, v) in f {
                    set_field(s, k, Some(v.clone()), e, true);
                }
            }
        }
        ops::TRANSITION => {
            let to = text_of(p, "to").unwrap_or_default();
            set_field(s, "status", Some(Value::String(to)), e, false);
            if let Some(Value::Object(f)) = p.get("fields") {
                for (k, v) in f {
                    set_field(s, k, Some(v.clone()), e, true);
                }
            }
        }
        ops::COMMENT => s.comments.push(CommentInfo {
            eid: e.eid.clone(),
            author: e.author.clone(),
            actor: e.actor.clone(),
            text: str_of(p, "text"),
            reply_to: match p.get("replyTo") {
                Some(Value::String(r)) => Some(r.clone()),
                _ => None,
            },
            time: e.time,
            reverted: false,
        }),
        ops::ATTACH => s.attachments.push(AttachmentInfo {
            eid: e.eid.clone(),
            path: str_of(p, "path"),
            sha256: str_of(p, "sha256"),
            size: p.get("size").and_then(|v| v.as_i64()).unwrap_or(0),
            title: str_of(p, "title"),
            present: None,
            reverted: false,
        }),
        ops::LINK => {
            let (t, r) = (str_of(p, "target"), rel_of(p));
            s.links.retain(|l| !(l.target == t && l.rel == r));
            s.links.push(LinkInfo { target: t, rel: r, eid: e.eid.clone() });
        }
        ops::UNLINK => {
            let (t, r) = (str_of(p, "target"), rel_of(p));
            s.links.retain(|l| !(l.target == t && l.rel == r));
        }
        ops::ACK => s.acks.push(AckInfo {
            eid: e.eid.clone(),
            author: e.author.clone(),
            actor: e.actor.clone(),
            level: str_of(p, "level"),
            target: match p.get("target") {
                Some(Value::String(t)) => Some(t.clone()),
                _ => None,
            },
            version: e.base,
            text: str_of(p, "text"),
            time: e.time,
        }),
        ops::REVERT => {
            let t = str_of(p, "target");
            for c in s.comments.iter_mut().filter(|c| c.eid == t) {
                c.reverted = true;
            }
            for a in s.attachments.iter_mut().filter(|a| a.eid == t) {
                a.reverted = true;
            }
            s.links.retain(|l| l.eid != t);
            s.acks.retain(|a| a.eid != t);
        }
        _ => {}
    }
    s.sync_derived();
    Ok(())
}

/// Установить поле. Встречная правка — текущее значение поставил другой автор, а событие о нём не знало (prev).
fn set_field(s: &mut RecordState, field: &str, value: Option<Value>, e: &BridgeEvent, check_conflict: bool) {
    let value = value.filter(|v| !v.is_null());
    if check_conflict {
        let current = s.setters.get(field).cloned();
        let prev = e.prev.as_ref().and_then(|p| p.get(field)).cloned();
        let own = current.as_ref().map(|c| c.to_lowercase().starts_with(&format!("{}:", e.author.to_lowercase()))).unwrap_or(false);
        if let Some(cur) = current.filter(|c| Some(c) != prev.as_ref() && !own) {
            let existing = s.fields.get(field).cloned();
            let values = s.conflicts.entry(field.to_string()).or_insert_with(|| {
                vec![ConflictValue { eid: cur.clone(), author: author_of(&cur), actor: String::new(), value: existing }]
            });
            values.push(ConflictValue { eid: e.eid.clone(), author: e.author.clone(), actor: e.actor.clone(), value: value.clone() });
        } else {
            s.conflicts.remove(field);
        }
    }
    match value {
        None => {
            s.fields.shift_remove(field);
        }
        Some(v) => {
            s.fields.insert(field.to_string(), v);
        }
    }
    s.setters.insert(field.to_string(), e.eid.clone());
}

fn author_of(eid: &str) -> String {
    match eid.rfind(':') {
        Some(i) if i > 0 => eid[..i].to_string(),
        _ => eid.to_string(),
    }
}

// ───────────────────────── Правила типов ─────────────────────────

struct Move(&'static str, &'static str, &'static str);

fn moves(type_: &str) -> Option<&'static [Move]> {
    const TASK: &[Move] = &[
        Move("new", "accepted", "assignee"),
        Move("new", "rejected", "assignee"),
        Move("accepted", "in_progress", "assignee"),
        Move("new", "in_progress", "assignee"),
        Move("in_progress", "review", "assignee"),
        Move("review", "done", "creator"),
        Move("review", "in_progress", "creator"),
        Move("new", "cancelled", "creator"),
        Move("accepted", "cancelled", "creator"),
        Move("in_progress", "cancelled", "creator"),
        Move("review", "cancelled", "creator"),
        Move("done", "new", "creator"),
        Move("rejected", "new", "creator"),
        Move("cancelled", "new", "creator"),
    ];
    const COMMAND: &[Move] = &[
        Move("pending", "accepted", "assignee"),
        Move("pending", "rejected", "assignee"),
        Move("accepted", "running", "assignee"),
        Move("running", "done", "assignee"),
        Move("running", "failed", "assignee"),
        Move("accepted", "failed", "assignee"),
        Move("pending", "cancelled", "creator"),
        Move("accepted", "cancelled", "creator"),
    ];
    const DECISION: &[Move] = &[
        Move("proposed", "accepted", "other"),
        Move("proposed", "rejected", "other"),
        Move("accepted", "proposed", "creator"),
        Move("rejected", "proposed", "creator"),
    ];
    match type_ {
        rt::TASK => Some(TASK),
        rt::COMMAND => Some(COMMAND),
        rt::DECISION => Some(DECISION),
        _ => None,
    }
}

pub fn statuses(type_: &str) -> &'static [&'static str] {
    match type_ {
        rt::TASK => &["new", "accepted", "in_progress", "review", "done", "rejected", "cancelled"],
        rt::COMMAND => &["pending", "accepted", "running", "done", "failed", "rejected", "cancelled"],
        rt::DECISION => &["proposed", "accepted", "rejected"],
        _ => &[],
    }
}

pub fn initial_status(type_: &str) -> Option<&'static str> {
    match type_ {
        rt::TASK => Some("new"),
        rt::COMMAND => Some("pending"),
        rt::DECISION => Some("proposed"),
        _ => None,
    }
}

fn strict(type_: &str) -> bool {
    moves(type_).is_some()
}

fn blank(p: &JsonObject, k: &str) -> bool {
    match p.get(k) {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        _ => false,
    }
}

fn is_type_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// (создатель?, исполнитель?)
pub fn roles(s: &RecordState, node: &str) -> (bool, bool) {
    (eq_ic(node, &s.creator), eq_ic(node, s.assignee().unwrap_or("")))
}

pub fn check(s: &RecordState, e: &BridgeEvent) -> Result<(), String> {
    let empty = JsonObject::new();
    let p = e.payload.as_ref().unwrap_or(&empty);
    if !ops::ALL.contains(&e.op.as_str()) {
        return Err(format!("неизвестная операция «{}»", e.op));
    }
    if e.op == ops::CREATE {
        if s.is_created() {
            return Err("запись уже создана".into());
        }
        if !crate::config::is_name(&e.type_) && !is_type_name(&e.type_) {
            return Err("тип записи — латиница, цифры, «-», «_», «.»".into());
        }
        let f = match p.get("fields") {
            Some(Value::Object(f)) => f.clone(),
            _ => JsonObject::new(),
        };
        if e.type_ == rt::TASK {
            if blank(&f, "title") {
                return Err("у задачи нужен title".into());
            }
            if blank(&f, "assignee") {
                return Err("у задачи нужен assignee (узел-исполнитель)".into());
            }
        }
        if e.type_ == rt::COMMAND {
            if blank(&f, "runner") {
                return Err("у команды нужен runner".into());
            }
            if blank(&f, "assignee") {
                return Err("у команды нужен assignee (узел-исполнитель)".into());
            }
        }
        if strict(&e.type_) && f.get("status").map(|v| !v.is_null()).unwrap_or(false) {
            return Err("статус новой записи ставится сам".into());
        }
        return Ok(());
    }
    if !s.is_created() {
        return Err("запись ещё не создана".into());
    }
    if e.type_ != s.type_ {
        return Err("тип события не совпадает с типом записи".into());
    }
    match e.op.as_str() {
        ops::SET => {
            let f = match p.get("fields") {
                Some(Value::Object(f)) if !f.is_empty() => f,
                _ => return Err("set: нужны fields".into()),
            };
            if strict(&s.type_) && f.contains_key("status") {
                return Err("статус меняется только переходом (transition)".into());
            }
            if strict(&s.type_) && (f.contains_key("assignee") || f.contains_key("runner") || f.contains_key("args")) && !eq_ic(&e.author, &s.creator) {
                return Err("исполнителя и параметры меняет только создатель".into());
            }
            if s.type_ == rt::COMMAND && s.status() != Some("pending") && (f.contains_key("runner") || f.contains_key("args")) {
                return Err("параметры команды после принятия не меняются".into());
            }
            Ok(())
        }
        ops::TRANSITION => {
            let to = text_of(p, "to").unwrap_or_default();
            if to.trim().is_empty() {
                return Err("transition: нужен to".into());
            }
            if let Some(Value::Object(tf)) = p.get("fields") {
                if tf.contains_key("status") {
                    return Err("transition: status в fields не нужен".into());
                }
            }
            let Some(mv) = moves(&s.type_) else { return Ok(()) };
            let from = s.status().unwrap_or("").to_string();
            if !statuses(&s.type_).contains(&to.as_str()) {
                return Err(format!("нет статуса «{}» у типа {}", to, s.type_));
            }
            let (creator, assignee) = roles(s, &e.author);
            let Some(m) = mv.iter().find(|m| m.0 == from && m.1 == to) else {
                return Err(format!("переход {} → {} не предусмотрен", from, to));
            };
            let ok = match m.2 {
                "creator" => creator,
                "assignee" => assignee,
                "other" => !creator,
                _ => true,
            };
            if !ok {
                let who = match m.2 {
                    "creator" => "создатель записи",
                    "assignee" => "исполнитель",
                    "other" => "другая сторона (не автор)",
                    _ => "любой",
                };
                return Err(format!("переход {} → {} делает {}", from, to, who));
            }
            if s.type_ == rt::TASK && from == "review" && to == "in_progress" && e.note.trim().is_empty() {
                return Err("возврат на доработку — с пояснением (note)".into());
            }
            Ok(())
        }
        ops::COMMENT => {
            if blank(p, "text") {
                Err("comment: пустой текст".into())
            } else {
                Ok(())
            }
        }
        ops::ATTACH => {
            if crate::path::norm(&str_of(p, "path")).is_none() || str_of(p, "sha256").is_empty() {
                Err("attach: нужен путь и sha256".into())
            } else {
                Ok(())
            }
        }
        ops::LINK | ops::UNLINK => {
            if blank(p, "target") {
                Err("link: нужен target".into())
            } else {
                Ok(())
            }
        }
        ops::ACK => match str_of(p, "level").as_str() {
            "accepted" | "rejected" | "seen" => Ok(()),
            _ => Err("ack: level — accepted, rejected или seen".into()),
        },
        ops::REVERT => {
            let t = str_of(p, "target");
            if !t.to_lowercase().starts_with(&format!("{}:", e.author.to_lowercase())) {
                return Err("отменить можно только своё событие".into());
            }
            let known = s.comments.iter().any(|c| c.eid == t) || s.attachments.iter().any(|a| a.eid == t) || s.links.iter().any(|l| l.eid == t) || s.acks.iter().any(|a| a.eid == t);
            if known {
                Ok(())
            } else {
                Err("revert: отменяются комментарии, вложения, связи и подтверждения этой записи".into())
            }
        }
        _ => Ok(()),
    }
}
