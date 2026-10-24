//! Записи пространства: журнал событий + текущие состояния + превращение действий в события.
//! Свои действия проверяются заранее (правила типа, схема, ожидаемая версия); чужие события — правилами приёма.

use crate::config::ShareConfig;
use crate::eq_ic;
use crate::events::EventStore;
use crate::proto::{ops, record_types, *};
use crate::reducer;
use crate::schemas::{is_schema_id, SchemaSet};
use crate::time;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

pub struct RecordBook {
    pub node: String,
    pub share_id: String,
    pub root: PathBuf,
    pub store: EventStore,
    states: HashMap<String, RecordState>,
    /// сосед → его курсор (что он у нас уже забрал): основа квитанций доставки
    pub peer_cursors: BTreeMap<String, BTreeMap<String, i64>>,
    changed: HashSet<String>,
}

pub fn is_record_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 96 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

impl RecordBook {
    pub fn open(node: &str, share_id: &str, root: PathBuf, events_dir: PathBuf) -> Self {
        let store = EventStore::open(events_dir, share_id);
        let mut b = RecordBook { node: node.into(), share_id: share_id.into(), root, store, states: HashMap::new(), peer_cursors: BTreeMap::new(), changed: HashSet::new() };
        for id in b.store.record_ids() {
            b.rebuild(&id);
        }
        b
    }

    fn rebuild(&mut self, id: &str) {
        if let Some(s) = reducer::fold(self.store.events_of(id), None) {
            self.states.insert(id.to_string(), s);
        }
        self.changed.insert(id.to_string());
    }

    pub fn take_changed(&mut self) -> Vec<String> {
        self.changed.drain().collect()
    }

    pub fn get(&self, id: &str) -> Option<&RecordState> {
        self.states.get(id)
    }

    /// Записи без удалённых.
    pub fn all(&self) -> Vec<&RecordState> {
        self.states.values().filter(|s| !s.is_deleted()).collect()
    }

    /// Только удалённые (корзина записей).
    pub fn deleted(&self) -> Vec<&RecordState> {
        self.states.values().filter(|s| s.is_deleted()).collect()
    }

    pub fn count(&self) -> usize {
        self.states.len()
    }

    /// Данные для проверки схемы: поля без служебного статуса строгих типов.
    fn data_of(s: &RecordState) -> Value {
        let mut d = s.fields.clone();
        if reducer::initial_status(&s.type_).is_some() {
            d.shift_remove("status");
        }
        Value::Object(d)
    }

    /// Состояние для выдачи наружу: копия + доставка соседям + ошибки схемы.
    pub fn view(&self, id: &str, at_version: Option<i64>, share_peers: &[String], schemas: &SchemaSet) -> Option<RecordState> {
        let mut s = match at_version {
            Some(v) => reducer::fold(self.store.events_of(id), Some(v))?,
            None => self.get(id)?.clone(),
        };
        let events = self.store.events_of(id);
        let mut delivered = BTreeMap::new();
        for peer in share_peers {
            let cur = self.peer_cursors.iter().find(|(k, _)| eq_ic(k, peer)).map(|(_, v)| v);
            let ok = events.iter().all(|e| {
                eq_ic(&e.author, peer) || cur.and_then(|c| c.iter().find(|(a, _)| eq_ic(a, &e.author)).map(|(_, s)| *s >= e.seq)).unwrap_or(false)
            });
            delivered.insert(peer.clone(), ok);
        }
        s.delivered = Some(delivered);
        if let Some(sc) = s.schema.clone() {
            let errs = schemas.validate(&sc, &Self::data_of(&s));
            if !errs.is_empty() {
                s.schema_errors = Some(errs);
            }
        }
        s.sync_derived();
        Some(s)
    }

    /// История записи: события в порядке применения.
    pub fn history(&self, id: &str) -> Vec<BridgeEvent> {
        let mut l = self.store.events_of(id);
        l.sort_by(reducer::compare);
        l
    }

    pub fn new_id(&self) -> String {
        let day = chrono::Utc::now().format("%Y%m%d").to_string();
        let mut n = self.states.len() + 1;
        loop {
            let id = format!("{}-{}-{}", self.node, day, n);
            if !self.states.contains_key(&id) && !self.store.has_record(&id) {
                return id;
            }
            n += 1;
        }
    }

    /// Выполнить действие от имени этого узла.
    pub fn act(&mut self, mut a: BridgeAction, share_peers: &[String], schemas: &SchemaSet) -> ActionResult {
        let e = match self.build(&mut a) {
            Ok(e) => e,
            Err(err) => return ActionResult::err(err),
        };
        // Пробное применение к копии: правила типа и схема — до записи в журнал
        let mut current = self.get(&e.record).cloned().unwrap_or_else(|| RecordState { id: e.record.clone(), share: self.share_id.clone(), ..Default::default() });
        let mut trial = e.clone();
        trial.clock = self.store.clock() + 1;
        trial.author = self.node.clone();
        trial.eid = format!("{}:?", self.node);
        if let Err(why) = reducer::apply(&mut current, &trial) {
            return ActionResult { error: why, record: Some(e.record), ..Default::default() };
        }
        if let Some(sc) = current.schema.clone() {
            if a.action == "create" || a.action == "set" {
                let errs = schemas.validate(&sc, &Self::data_of(&current));
                if !errs.is_empty() {
                    return ActionResult { error: format!("не по схеме {}: {}", sc, errs.join("; ")), record: Some(e.record), ..Default::default() };
                }
            }
        }
        let record = e.record.clone();
        let stored = match self.store.append_own(e) {
            Ok(s) => s,
            Err(err) => return ActionResult::err(format!("не удалось записать журнал: {}", err)),
        };
        self.rebuild(&record);
        let version = self.get(&record).map(|s| s.version).unwrap_or(0);
        ActionResult { ok: true, error: String::new(), record: Some(record.clone()), eid: Some(stored.eid), version, state: self.view(&record, None, share_peers, schemas) }
    }

    fn build(&self, a: &mut BridgeAction) -> Result<BridgeEvent, String> {
        let action = a.action.trim().to_lowercase();
        a.action = action.clone();
        let actor = if a.actor.trim().is_empty() { self.node.clone() } else { a.actor.trim().to_string() };
        let note = a.note.clone();
        if action == "create" {
            let type_ = a.type_.clone().unwrap_or_default();
            if type_.trim().is_empty() {
                return Err("create: нужен type".into());
            }
            let id = match a.record.as_deref().map(str::trim) {
                Some(r) if !r.is_empty() => r.to_string(),
                _ => self.new_id(),
            };
            if !is_record_id(&id) {
                return Err("id записи — латиница, цифры, «-», «_», «.» (до 96)".into());
            }
            if self.get(&id).is_some() || self.store.has_record(&id) {
                return Err(format!("запись {} уже есть", id));
            }
            if let Some(sc) = &a.schema {
                if !is_schema_id(sc) {
                    return Err("schema — «имя@версия»".into());
                }
            }
            if type_ == record_types::DTO && a.schema.is_none() {
                return Err("dto: нужна schema («имя@версия»)".into());
            }
            let mut p = JsonObject::new();
            p.insert("fields".into(), Value::Object(a.fields.clone().unwrap_or_default()));
            return Ok(self.new_event(&id, type_.trim(), a.schema.clone(), ops::CREATE, p, &actor, &note, 0, None));
        }
        let rid = a.record.as_deref().map(str::trim).unwrap_or("");
        if rid.is_empty() {
            return Err(format!("{}: нужен record (id записи)", action));
        }
        let s = self.get(rid).ok_or_else(|| format!("нет записи {}", rid))?.clone();
        if let Some(ev) = a.expect_version {
            if ev != s.version {
                return Err(format!("версия записи {}, ожидалась {} — запись изменилась, перечитайте", s.version, ev));
            }
        }
        let mut p = JsonObject::new();
        let mut prev: Option<BTreeMap<String, String>> = None;
        let mut schema: Option<String> = None;
        let op: &str;
        let mut note = note;
        match action.as_str() {
            "set" => {
                let f = a.fields.clone().filter(|f| !f.is_empty()).ok_or("set: нужны fields")?;
                op = ops::SET;
                prev = Some(prev_of(&s, f.keys().cloned()));
                p.insert("fields".into(), Value::Object(f));
                if let Some(sc) = &a.schema {
                    if !is_schema_id(sc) {
                        return Err("schema — «имя@версия»".into());
                    }
                    schema = Some(sc.clone());
                }
            }
            "transition" | "approve" | "reject" => {
                op = ops::TRANSITION;
                let to = match action.as_str() {
                    "approve" => "accepted".to_string(),
                    "reject" => "rejected".to_string(),
                    _ => a.to.clone().unwrap_or_default(),
                };
                if to.trim().is_empty() {
                    return Err("transition: нужен to".into());
                }
                p.insert("to".into(), Value::String(to));
                let mut keys = vec!["status".to_string()];
                if let Some(f) = a.fields.clone().filter(|f| !f.is_empty()) {
                    keys.extend(f.keys().cloned());
                    p.insert("fields".into(), Value::Object(f));
                }
                prev = Some(prev_of(&s, keys.into_iter()));
                if action == "reject" && note.trim().is_empty() {
                    if let Some(t) = a.text.clone().filter(|t| !t.trim().is_empty()) {
                        note = t;
                    }
                }
            }
            // Удаление — обычная правка поля deleted: доходит до всех узлов (и до старых версий), вернуть — restore
            "delete" | "restore" => {
                op = ops::SET;
                let mut f = JsonObject::new();
                f.insert("deleted".into(), Value::Bool(action == "delete"));
                prev = Some(prev_of(&s, std::iter::once("deleted".to_string())));
                p.insert("fields".into(), Value::Object(f));
            }
            "comment" => {
                op = ops::COMMENT;
                p.insert("text".into(), Value::String(a.text.clone().unwrap_or_default()));
                if let Some(r) = a.reply_to.clone().filter(|r| !r.trim().is_empty()) {
                    p.insert("replyTo".into(), Value::String(r));
                }
            }
            "ack" => {
                op = ops::ACK;
                p.insert("level".into(), Value::String(a.level.clone().unwrap_or_else(|| "seen".into())));
                p.insert("text".into(), Value::String(a.text.clone().unwrap_or_default()));
                if let Some(t) = a.target.clone().filter(|t| !t.trim().is_empty()) {
                    p.insert("target".into(), Value::String(t));
                }
            }
            "link" | "unlink" => {
                op = if action == "link" { ops::LINK } else { ops::UNLINK };
                p.insert("target".into(), Value::String(a.target.clone().unwrap_or_default()));
                p.insert("rel".into(), Value::String(a.rel.clone().filter(|r| !r.trim().is_empty()).unwrap_or_else(|| "relates".into())));
            }
            "revert" => {
                op = ops::REVERT;
                p.insert("target".into(), Value::String(a.target.clone().unwrap_or_default()));
            }
            "attach" => {
                op = ops::ATTACH;
                let path = crate::path::normalize(a.path.as_deref().unwrap_or("")).map_err(|w| format!("attach: путь — {}", w))?;
                let full = crate::path::to_local(&self.root, &path).map_err(|e| e.to_string())?;
                if !full.is_file() {
                    return Err(format!("attach: нет файла {} в папке пространства", path));
                }
                let sha = crate::path::sha256_file(&full).map_err(|e| e.to_string())?;
                let size = std::fs::metadata(&full).map(|m| m.len() as i64).unwrap_or(0);
                p.insert("path".into(), Value::String(path));
                p.insert("sha256".into(), Value::String(sha));
                p.insert("size".into(), Value::from(size));
                p.insert("title".into(), Value::String(a.title.clone().unwrap_or_default()));
            }
            _ => {
                return Err(format!(
                    "неизвестное действие «{}» (create, set, transition, comment, ack, link, unlink, attach, revert, approve, reject, delete, restore)",
                    a.action
                ))
            }
        }
        let prev = prev.filter(|p| !p.is_empty());
        Ok(self.new_event(&s.id, &s.type_, schema, op, p, &actor, &note, s.version, prev))
    }

    #[allow(clippy::too_many_arguments)]
    fn new_event(&self, id: &str, type_: &str, schema: Option<String>, op: &str, payload: JsonObject, actor: &str, note: &str, base: i64, prev: Option<BTreeMap<String, String>>) -> BridgeEvent {
        BridgeEvent {
            share: self.share_id.clone(),
            record: id.into(),
            type_: type_.into(),
            schema,
            op: op.into(),
            payload: Some(payload),
            author: self.node.clone(),
            actor: actor.into(),
            note: note.into(),
            time: time::now(),
            base,
            prev,
            ..Default::default()
        }
    }

    /// Принять события от соседа. Отклоняются: не по правилам приёма, от имени этого узла, с неверным eid или пространством.
    pub fn ingest(&mut self, from_peer: &str, events: Vec<BridgeEvent>, participants: &[String], cfg: &ShareConfig) -> (usize, usize) {
        let (mut ok, mut bad) = (0, 0);
        let mut touched: HashSet<String> = HashSet::new();
        for e in events {
            if self.store.knows(&e.eid) {
                continue;
            }
            let why: Option<String> = if !eq_ic(&e.share, &self.share_id) {
                Some("чужое пространство".into())
            } else if e.eid != make_eid(&e.author, e.seq) || e.seq <= 0 {
                Some("неверный eid".into())
            } else if eq_ic(&e.author, &self.node) {
                Some("событие от имени этого узла, которого он не создавал".into())
            } else if !participants.iter().any(|p| eq_ic(p, &e.author)) {
                Some(format!("автор {} — не участник пространства на этом узле", e.author))
            } else if !is_record_id(&e.record) {
                Some("неверный id записи".into())
            } else if !cfg.records.accepts(&e.author, &e.type_, &e.op) {
                Some(format!("правила приёма: {} / {} / {}", e.author, e.type_, e.op))
            } else {
                None
            };
            if let Some(w) = why {
                self.store.reject(e, &w, from_peer);
                bad += 1;
                continue;
            }
            let rec = e.record.clone();
            if self.store.ingest(e) {
                ok += 1;
                touched.insert(rec);
            }
        }
        for id in touched {
            self.rebuild(&id);
        }
        (ok, bad)
    }
}

fn prev_of(s: &RecordState, fields: impl Iterator<Item = String>) -> BTreeMap<String, String> {
    let mut d = BTreeMap::new();
    for f in fields {
        if let Some(eid) = s.setters.get(&f) {
            d.insert(f, eid.clone());
        }
    }
    d
}
