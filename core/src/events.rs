//! Журнал событий пространства: файл JSONL на автора, только дописывание; дедупликация по eid.

use crate::proto::{make_eid, BridgeEvent, EventsPage};
use crate::time::{self, Time};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RejectedEvent {
    pub event: BridgeEvent,
    pub reason: String,
    pub from: String,
    #[serde(with = "time::ser")]
    pub time: Time,
}

impl Default for RejectedEvent {
    fn default() -> Self {
        RejectedEvent { event: BridgeEvent::default(), reason: String::new(), from: String::new(), time: time::min() }
    }
}

pub struct EventStore {
    dir: PathBuf,
    pub share: String,
    by_author: BTreeMap<String, Vec<BridgeEvent>>,
    by_record: HashMap<String, Vec<(String, i64)>>,
    eids: HashSet<String>,
    vector: BTreeMap<String, i64>,
    rejected: Vec<RejectedEvent>,
    clock: i64,
}

fn lc(s: &str) -> String {
    s.to_lowercase()
}

impl EventStore {
    pub fn open(dir: PathBuf, share: &str) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let mut s = EventStore {
            dir,
            share: share.into(),
            by_author: BTreeMap::new(),
            by_record: HashMap::new(),
            eids: HashSet::new(),
            vector: BTreeMap::new(),
            rejected: Vec::new(),
            clock: 0,
        };
        s.load();
        s
    }

    fn load(&mut self) {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false)).collect();
        files.sort();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            let text = text.trim_start_matches('\u{feff}');
            let rejected_file = f.file_name().map(|n| n == "_rejected.jsonl").unwrap_or(false);
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if rejected_file {
                    if let Ok(r) = serde_json::from_str::<RejectedEvent>(line) {
                        self.bump(&r.event.author, r.event.seq);
                        self.eids.insert(lc(&r.event.eid));
                        self.rejected.push(r);
                    }
                    continue;
                }
                let Ok(e) = serde_json::from_str::<BridgeEvent>(line) else { continue }; // оборванная строка
                if !self.eids.insert(lc(&e.eid)) {
                    continue;
                }
                self.index(e);
            }
        }
        for l in self.by_author.values_mut() {
            l.sort_by_key(|e| e.seq);
        }
    }

    fn author_key(&self, author: &str) -> String {
        // автор без учёта регистра: используем уже известное написание
        self.by_author.keys().find(|k| crate::eq_ic(k, author)).cloned().unwrap_or_else(|| author.to_string())
    }

    fn index(&mut self, e: BridgeEvent) {
        let ak = self.author_key(&e.author);
        self.bump(&e.author, e.seq);
        if e.clock > self.clock {
            self.clock = e.clock;
        }
        self.by_record.entry(e.record.clone()).or_default().push((ak.clone(), e.seq));
        self.by_author.entry(ak).or_default().push(e);
    }

    fn bump(&mut self, author: &str, seq: i64) {
        let key = self.vector.keys().find(|k| crate::eq_ic(k, author)).cloned().unwrap_or_else(|| author.to_string());
        let v = self.vector.entry(key).or_insert(0);
        if seq > *v {
            *v = seq;
        }
    }

    fn append_line(&self, file: &str, json: &str) -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join(file))?;
        f.write_all(json.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_data()
    }

    pub fn vector(&self) -> BTreeMap<String, i64> {
        self.vector.clone()
    }

    pub fn clock(&self) -> i64 {
        self.clock
    }

    /// Записать своё событие: узел назначает seq и часы.
    pub fn append_own(&mut self, mut e: BridgeEvent) -> std::io::Result<BridgeEvent> {
        e.share = self.share.clone();
        e.seq = self.vector.iter().find(|(k, _)| crate::eq_ic(k, &e.author)).map(|(_, v)| *v).unwrap_or(0) + 1;
        e.eid = make_eid(&e.author, e.seq);
        e.clock = self.clock + 1;
        self.append_line(&format!("{}.jsonl", e.author), &serde_json::to_string(&e)?)?;
        self.eids.insert(lc(&e.eid));
        self.index(e.clone());
        Ok(e)
    }

    /// Принять событие соседа. false — уже было.
    pub fn ingest(&mut self, e: BridgeEvent) -> bool {
        if self.eids.contains(&lc(&e.eid)) {
            return false;
        }
        let Ok(json) = serde_json::to_string(&e) else { return false };
        if self.append_line(&format!("{}.jsonl", e.author), &json).is_err() {
            return false;
        }
        self.eids.insert(lc(&e.eid));
        let ak = self.author_key(&e.author);
        let seq = e.seq;
        self.index(e);
        // поддерживать порядок по seq у автора
        if let Some(l) = self.by_author.get_mut(&ak) {
            if l.len() > 1 && l[l.len() - 2].seq > seq {
                l.sort_by_key(|x| x.seq);
            }
        }
        true
    }

    /// Запомнить отклонённое событие: курсор сдвигается, событие не применяется.
    pub fn reject(&mut self, e: BridgeEvent, reason: &str, from: &str) {
        if !self.eids.insert(lc(&e.eid)) {
            return;
        }
        let r = RejectedEvent { event: e, reason: reason.into(), from: from.into(), time: time::now() };
        if let Ok(j) = serde_json::to_string(&r) {
            let _ = self.append_line("_rejected.jsonl", &j);
        }
        self.bump(&r.event.author.clone(), r.event.seq);
        self.rejected.push(r);
    }

    pub fn knows(&self, eid: &str) -> bool {
        self.eids.contains(&lc(eid))
    }

    pub fn rejected(&self) -> &[RejectedEvent] {
        &self.rejected
    }

    /// События после курсора (по авторам, по возрастанию seq).
    pub fn since(&self, after: &BTreeMap<String, i64>, limit: usize) -> EventsPage {
        let mut page = EventsPage::default();
        let mut authors: Vec<&String> = self.by_author.keys().collect();
        authors.sort_by(|a, b| crate::cmp_ordinal(a, b));
        for a in authors {
            let from = after.iter().find(|(k, _)| crate::eq_ic(k, a)).map(|(_, v)| *v).unwrap_or(0);
            for e in &self.by_author[a] {
                if e.seq <= from {
                    continue;
                }
                if page.events.len() >= limit {
                    page.more = true;
                    return page;
                }
                page.events.push(e.clone());
            }
        }
        page
    }

    pub fn has_since(&self, after: &BTreeMap<String, i64>) -> bool {
        self.by_author.iter().any(|(a, l)| {
            let from = after.iter().find(|(k, _)| crate::eq_ic(k, a)).map(|(_, v)| *v).unwrap_or(0);
            l.last().map(|e| e.seq > from).unwrap_or(false)
        })
    }

    pub fn record_ids(&self) -> Vec<String> {
        self.by_record.keys().cloned().collect()
    }

    pub fn events_of(&self, record: &str) -> Vec<BridgeEvent> {
        let Some(refs) = self.by_record.get(record) else { return Vec::new() };
        refs.iter()
            .filter_map(|(a, seq)| self.by_author.get(a).and_then(|l| l.binary_search_by_key(seq, |e| e.seq).ok().map(|i| l[i].clone())))
            .collect()
    }

    pub fn has_record(&self, record: &str) -> bool {
        self.by_record.contains_key(record)
    }

    pub fn find(&self, eid: &str) -> Option<&BridgeEvent> {
        let i = eid.rfind(':')?;
        let seq: i64 = eid[i + 1..].parse().ok()?;
        let a = &eid[..i];
        self.by_author.iter().find(|(k, _)| crate::eq_ic(k, a)).and_then(|(_, l)| l.iter().find(|e| e.seq == seq))
    }

    pub fn count(&self) -> usize {
        self.eids.len()
    }
}
