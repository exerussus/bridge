//! Агент: соседи поручают задачи Claude на этом ПК, этот ПК — их Claude.
//!
//! Поручение — «нить» (thread): инструкция → работа → итог; продолжение — в той же сессии Claude (он помнит контекст).
//! У оркестратора нить с ролью out, у исполнителя — in. Исполнитель шлёт ход работы оркестратору (POST /peer/agent/events);
//! оркестратор может и сам забрать нить целиком (GET /peer/agent/thread), если что-то не дошло.
//!
//! Режимы исполнителя: auto — мост сам запускает Claude Code (`claude -p … --output-format stream-json`, `--resume` для продолжения);
//! manual — поручение ждёт, пока его возьмёт сессия Claude на этом ПК (инструменты моста: agent_inbox / agent_take / agent_report).

use crate::node::Node;
use bridge_core::time::{self, Time};
use bridge_core::{eq_ic, to_json, write_atomic};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

/// Сколько событий хода держим в нити (старые — в начале — отбрасываются).
const MAX_EVENTS: usize = 3000;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Ev {
    pub n: i64,
    #[serde(with = "time::opt")]
    pub t: Option<Time>,
    /// prompt — поручение, text — Claude пишет, tool — вызывает инструмент, result — итог, error, info
    pub kind: String,
    pub text: String,
}

/// Вопрос агента людям (заказчику и тем, кто у ПК исполнителя).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Req {
    pub id: String,
    pub q: String,
    pub buttons: Vec<String>,
    pub input: bool,
    #[serde(with = "time::opt")]
    pub at: Option<Time>,
    pub answer: String,
    pub by: String,
    #[serde(with = "time::opt")]
    pub answered: Option<Time>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Thread {
    pub id: String,
    /// out — мы поручили соседу; in — сосед поручил нам
    pub role: String,
    pub peer: String,
    pub title: String,
    /// queued — ждёт, пока возьмут; running — в работе; idle — ход завершён, можно продолжать; failed; stopped
    pub status: String,
    pub session: String,
    #[serde(with = "time::opt")]
    pub created: Option<Time>,
    #[serde(with = "time::opt")]
    pub updated: Option<Time>,
    pub cost: f64,
    pub turns: i64,
    pub result: String,
    pub error: String,
    /// Обменная папка: общая папка оркестратора (owner/id)
    pub exchange_owner: String,
    pub exchange_id: String,
    /// manual: кто взял (сессия Claude на этом ПК)
    pub taken: bool,
    /// (3.2) Сколько поручений в нити: ход старого поручения не перезапишет статус нового.
    pub asks: i64,
    /// (3.2) Чем агент занят — пишет сам, на этап («ищет информацию», «сверяет данные»).
    pub phase: String,
    #[serde(with = "time::opt")]
    pub phase_at: Option<Time>,
    /// (3.2) Запись-задача, которую решает это поручение: {share, id}. Исполнитель сам ведёт её статус.
    pub record: Value,
    /// (3.3.1) Вопросы агента людям: кнопки и/или поле; первый ответ уходит агенту.
    pub requests: Vec<Req>,
    /// (3.3) Поручение себе: Claude запускается на этом же ПК, соседа нет (роль in, peer — этот ПК).
    pub local: bool,
    pub events: Vec<Ev>,
}

impl Thread {
    fn push(&mut self, kind: &str, text: impl Into<String>) {
        let n = self.events.last().map(|e| e.n + 1).unwrap_or(0);
        self.events.push(Ev { n, t: Some(time::now()), kind: kind.into(), text: text.into() });
        if self.events.len() > MAX_EVENTS {
            let cut = self.events.len() - MAX_EVENTS;
            self.events.drain(0..cut);
        }
        self.updated = Some(time::now());
    }

    pub fn last_n(&self) -> i64 {
        self.events.last().map(|e| e.n).unwrap_or(-1)
    }

    /// В работе: ждёт, пока возьмут, работает или явно ждёт (ожидание).
    pub fn busy(&self) -> bool {
        self.status == "running" || self.status == "queued" || self.status == "waiting"
    }

    /// Этап от агента: «Этап: …» — работает, «Ожидание: …» — ждёт. Пишется в ход отдельной строкой.
    fn set_phase(&mut self, text: &str, waiting: bool) {
        let text: String = text.trim().chars().take(160).collect();
        if text.is_empty() {
            return;
        }
        if self.phase != text || (self.status == "waiting") != waiting {
            self.phase = text.clone();
            self.phase_at = Some(time::now());
            self.push("phase", if waiting { format!("ожидание: {}", text) } else { text });
        }
        if self.status == "running" || self.status == "waiting" {
            self.status = if waiting { "waiting".into() } else { "running".into() };
        }
    }

    /// Кратко — для списков.
    pub fn summary(&self) -> Value {
        let last = self.events.iter().rev().find(|e| e.kind == "text" || e.kind == "result" || e.kind == "error").map(|e| e.text.chars().take(200).collect::<String>()).unwrap_or_default();
        json!({ "id": self.id, "role": self.role, "peer": self.peer, "title": self.title, "status": self.status, "created": self.created.map(|t| time::format(&t)),
                "updated": self.updated.map(|t| time::format(&t)), "cost": self.cost, "turns": self.turns, "error": self.error, "last": last, "events": self.events.len(), "taken": self.taken,
                "phase": self.phase, "phaseAt": self.phase_at.map(|t| time::format(&t)), "asks": self.asks, "record": self.record, "local": self.local,
                "requests": self.requests.iter().rev().take(20).rev().collect::<Vec<_>>(), "open": self.requests.iter().filter(|r| r.answered.is_none()).count() })
    }

    /// Нить с событиями после `after` (для панели и инструментов).
    pub fn since(&self, after: i64) -> Value {
        let mut v = self.summary();
        v["result"] = json!(self.result);
        v["session"] = json!(self.session);
        v["exchange"] = json!({ "owner": self.exchange_owner, "id": self.exchange_id });
        v["events"] = json!(self.events.iter().filter(|e| e.n > after).collect::<Vec<_>>());
        v["lastN"] = json!(self.last_n());
        v
    }
}

struct Proc {
    pid: Option<u32>,
    cancel: tokio_util::sync::CancellationToken,
}

pub struct Agent {
    dir: PathBuf,
    pub threads: Mutex<BTreeMap<String, Thread>>,
    procs: Mutex<HashMap<String, Proc>>,
    /// Нити, ход которых надо дослать оркестратору: нить → последний доставленный n
    pushed: Mutex<HashMap<String, i64>>,
    dirty: Mutex<HashSet<String>>,
    /// Будит ждущих (agent/wait, инструменты).
    pub changed: tokio::sync::Notify,
    /// Найденный claude: путь и проверена ли поддержка --permission-prompts
    no_prompts_flag: std::sync::atomic::AtomicBool,
}

impl Agent {
    pub fn new(data_dir: &Path) -> Self {
        Agent {
            dir: data_dir.join("agent"),
            threads: Mutex::new(BTreeMap::new()),
            procs: Mutex::new(HashMap::new()),
            pushed: Mutex::new(HashMap::new()),
            dirty: Mutex::new(HashSet::new()),
            changed: tokio::sync::Notify::new(),
            no_prompts_flag: std::sync::atomic::AtomicBool::new(true),
        }
    }

    /// Загрузить нити с диска. Нити, что были в работе при остановке моста, — прерваны.
    pub fn load(&self) {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return };
        let mut g = self.threads.lock();
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let Ok(mut t) = serde_json::from_str::<Thread>(text.trim_start_matches('\u{feff}')) else { continue };
            if t.role == "in" && (t.status == "running" || t.status == "waiting") {
                t.status = "failed".into();
                t.error = "мост был остановлен во время работы — поручите ещё раз (сессия сохранилась)".into();
                t.push("error", t.error.clone());
            }
            g.insert(t.id.clone(), t);
        }
    }

    fn save(&self, t: &Thread) {
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = write_atomic(&self.dir.join(format!("{}.json", safe(&t.id))), &to_json(t));
    }

    /// Изменить нить, сохранить, разбудить ждущих; у исполнителя — отметить для отправки оркестратору.
    pub fn update<R>(&self, id: &str, f: impl FnOnce(&mut Thread) -> R) -> Option<R> {
        let (r, snapshot) = {
            let mut g = self.threads.lock();
            let t = g.get_mut(id)?;
            let r = f(t);
            (r, t.clone())
        };
        self.save(&snapshot);
        if snapshot.role == "in" {
            self.dirty.lock().insert(id.to_string());
        }
        self.changed.notify_waiters();
        Some(r)
    }

    pub fn get(&self, id: &str) -> Option<Thread> {
        self.threads.lock().get(id).cloned()
    }

    pub fn list(&self) -> Vec<Value> {
        let mut v: Vec<Thread> = self.threads.lock().values().cloned().collect();
        v.sort_by(|a, b| b.updated.cmp(&a.updated));
        v.iter().map(|t| t.summary()).collect()
    }

    fn running_count(&self) -> usize {
        self.procs.lock().len()
    }
}

fn safe(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect()
}

pub fn new_thread_id(node: &Node) -> String {
    format!("{}-{}", node.node_name(), chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f"))
}

// ───────────────────────── Оркестратор ─────────────────────────

/// Поручить соседу (новая нить или продолжение). Возвращает нить.
pub async fn ask(node: &Arc<Node>, peer: &str, prompt: &str, thread: Option<&str>, title: Option<&str>) -> Result<Thread, (u16, String)> {
    ask_full(node, peer, prompt, thread, title, &Value::Null).await
}

/// Поручение с привязкой к записи-задаче `record` ({share, id}): исполнитель сам ведёт её статус
/// (принята → в работе → на проверке); «готово» ставит только заказчик.
pub async fn ask_full(node: &Arc<Node>, peer: &str, prompt: &str, thread: Option<&str>, title: Option<&str>, record: &Value) -> Result<Thread, (u16, String)> {
    let record = clean_record(record);
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err((400, "пустое поручение".into()));
    }
    if is_local(node, peer) {
        return ask_local(node, prompt, thread, title, &record);
    }
    let pc = node.cfg.read().peer(peer).cloned().ok_or((404, format!("нет соседа «{}»", peer)))?;
    if !pc.has_url() {
        return Err((409, format!("«{}» ещё не подключился", peer)));
    }
    let id = match thread {
        Some(t) if !t.is_empty() => {
            let t0 = node.agent.get(t).ok_or((404, format!("нет нити {}", t)))?;
            if t0.role != "out" || !eq_ic(&t0.peer, &pc.name) {
                return Err((409, "эта нить — не ваше поручение этому соседу".into()));
            }
            if t0.busy() {
                return Err((409, format!("«{}» ещё работает над этой нитью — дождитесь (agent_wait) или остановите", peer)));
            }
            t0.id
        }
        _ => new_thread_id(node),
    };
    let (ex_owner, ex_id) = {
        let c = node.cfg.read();
        let ex = c.agent.exchange.clone();
        if !ex.is_empty() && c.folder(&ex).is_some() {
            (c.node.clone(), ex)
        } else {
            (String::new(), String::new())
        }
    };
    let title = title.map(|s| s.to_string()).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| prompt.lines().next().unwrap_or("").chars().take(80).collect());
    // Сначала у себя — чтобы ход, пришедший раньше ответа, было куда записать
    {
        let mut g = node.agent.threads.lock();
        let t = g.entry(id.clone()).or_insert_with(|| Thread {
            id: id.clone(),
            role: "out".into(),
            peer: pc.name.clone(),
            title: title.clone(),
            created: Some(time::now()),
            exchange_owner: ex_owner.clone(),
            exchange_id: ex_id.clone(),
            ..Default::default()
        });
        t.status = "queued".into();
        t.error.clear();
        t.phase.clear();
        t.asks += 1;
        if !record.is_null() {
            t.record = record.clone();
        }
        t.push("prompt", prompt.to_string());
    }
    let snapshot = node.agent.get(&id).unwrap();
    node.agent.save(&snapshot);
    let client = node.client_for(&pc);
    let body = json!({ "thread": id, "title": title, "prompt": prompt, "exchangeOwner": ex_owner, "exchangeId": ex_id, "ask": snapshot.asks, "record": snapshot.record });
    match tokio::time::timeout(Duration::from_secs(20), client.post_json("/peer/agent/ask", &body)).await {
        Ok(Ok((code, text))) if (200..300).contains(&code) => {
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let st = v.get("status").and_then(|s| s.as_str()).unwrap_or("running").to_string();
            node.agent.update(&id, |t| {
                if t.status == "queued" {
                    t.status = st.clone();
                }
            });
            node.info(&format!("Агент: поручение «{}» → {} ({})", title, pc.name, if st == "queued" { "ждёт, пока возьмут" } else { "в работе" }));
            Ok(node.agent.get(&id).unwrap())
        }
        other => {
            let why = match other {
                Ok(Ok((code, text))) => crate::client::error_text(&text, code),
                Ok(Err(e)) => crate::link::explain(&e, &pc).0,
                Err(_) => crate::link::explain_timeout(&pc).0,
            };
            node.agent.update(&id, |t| {
                t.status = "failed".into();
                t.error = why.clone();
                t.push("error", format!("не доставлено: {}", why));
            });
            Err((502, why))
        }
    }
}

/// Поручение этому же ПК: пусто, «этот», «local» или собственное имя узла.
pub fn is_local(node: &Node, peer: &str) -> bool {
    let p = peer.trim();
    p.is_empty() || eq_ic(p, "local") || eq_ic(p, "этот") || eq_ic(p, "этот пк") || eq_ic(p, &node.node_name())
}

/// (3.3) Поручить Claude на этом ПК: нить in без соседа. Режим агента тот же (auto — мост запускает Claude сам,
/// manual — ждёт сессию Claude здесь); «кто может поручать» не нужен — поручает сам хозяин ПК.
fn ask_local(node: &Arc<Node>, prompt: &str, thread: Option<&str>, title: Option<&str>, record: &Value) -> Result<Thread, (u16, String)> {
    let me = node.node_name();
    if node.cfg.read().agent.mode == "off" {
        return Err((403, "агент на этом ПК выключен (панель → «Агенты» → режим)".into()));
    }
    let id = match thread {
        Some(t) if !t.is_empty() => {
            let t0 = node.agent.get(t).ok_or((404, format!("нет нити {}", t)))?;
            if !t0.local {
                return Err((409, "эта нить — не поручение этому ПК".into()));
            }
            if t0.busy() {
                return Err((409, "агент ещё работает над этой нитью — дождитесь (agent_wait) или остановите".into()));
            }
            t0.id
        }
        _ => new_thread_id(node),
    };
    let (ex_owner, ex_id) = {
        let c = node.cfg.read();
        let ex = c.agent.exchange.clone();
        if !ex.is_empty() && c.folder(&ex).is_some() { (c.node.clone(), ex) } else { (String::new(), String::new()) }
    };
    let title = title.map(|s| s.to_string()).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| prompt.lines().next().unwrap_or("").chars().take(80).collect());
    let v = json!({ "thread": id, "title": title, "prompt": prompt, "exchangeOwner": ex_owner, "exchangeId": ex_id, "record": record });
    enqueue(node, &me, &v, true)?;
    Ok(node.agent.get(&id).unwrap())
}

/// Остановить: свою нить-поручение — попросить соседа; принятую — прервать здесь.
pub async fn stop(node: &Arc<Node>, id: &str) -> Result<(), (u16, String)> {
    let t = node.agent.get(id).ok_or((404, "нет такой нити".to_string()))?;
    if t.role == "in" {
        stop_local(node, id, "остановлено");
        return Ok(());
    }
    let pc = node.cfg.read().peer(&t.peer).cloned().ok_or((404, "нет соседа".to_string()))?;
    let client = node.client_for(&pc);
    match tokio::time::timeout(Duration::from_secs(15), client.post_json("/peer/agent/stop", &json!({ "thread": id }))).await {
        Ok(Ok((code, _))) if (200..300).contains(&code) => Ok(()),
        Ok(Ok((code, text))) => Err((code, crate::client::error_text(&text, code))),
        Ok(Err(e)) => Err((502, crate::link::explain(&e, &pc).0)),
        Err(_) => Err((504, crate::link::explain_timeout(&pc).0)),
    }
}

/// Ход от исполнителя (наша нить-поручение).
pub fn on_events(node: &Arc<Node>, from: &str, v: &Value) -> Result<(), (u16, String)> {
    let id = v.get("thread").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let exists = node.agent.get(&id);
    match &exists {
        Some(t) if t.role == "out" && eq_ic(&t.peer, from) => {}
        Some(_) => return Err((403, "чужая нить".into())),
        None => {
            // нити нет (мост переустановлен?) — завести по данным исполнителя
            node.agent.threads.lock().insert(
                id.clone(),
                Thread { id: id.clone(), role: "out".into(), peer: from.to_string(), title: v.get("title").and_then(|s| s.as_str()).unwrap_or("").into(), created: Some(time::now()), ..Default::default() },
            );
        }
    }
    let evs: Vec<Ev> = v.get("events").and_then(|e| serde_json::from_value(e.clone()).ok()).unwrap_or_default();
    // ход прошлого поручения (пришёл позже нового) — события берём, а статус и итог нет: иначе «готово», пока агент работает
    let mine_asks = exists.as_ref().map(|t| t.asks).unwrap_or(0);
    let stale = v.get("asks").and_then(|x| x.as_i64()).map(|a| a < mine_asks).unwrap_or(false);
    let status = if stale { String::new() } else { v.get("status").and_then(|s| s.as_str()).unwrap_or("").to_string() };
    let prev_status = exists.map(|t| t.status).unwrap_or_default();
    node.agent.update(&id, |t| {
        let have = t.last_n();
        // события исполнителя нумеруются у него; у нас — свои n, но порядок тот же: берём только новые по его n
        let their_last = v.get("theirLast").and_then(|x| x.as_i64()).unwrap_or(-1);
        let _ = (have, their_last);
        for e in evs {
            if e.kind == "prompt" {
                continue; // поручения у нас уже записаны
            }
            t.push(&e.kind, e.text);
        }
        if !status.is_empty() {
            t.status = status.clone();
        }
        if !stale {
            if let Some(p) = v.get("phase").and_then(|s| s.as_str()) {
                if p != t.phase {
                    t.phase = p.to_string();
                    t.phase_at = Some(time::now());
                }
            }
        }
        for k in ["session", "result", "error"] {
            if stale && k != "session" {
                continue;
            }
            if let Some(s) = v.get(k).and_then(|s| s.as_str()) {
                match k {
                    "session" => t.session = s.into(),
                    "result" => t.result = s.into(),
                    _ => t.error = s.into(),
                }
            }
        }
        if let Some(c) = v.get("cost").and_then(|c| c.as_f64()) {
            t.cost = c;
        }
        if let Some(rq) = v.get("requests").and_then(|x| serde_json::from_value::<Vec<Req>>(x.clone()).ok()) {
            t.requests = rq;
        }
        if let Some(c) = v.get("turns").and_then(|c| c.as_i64()) {
            t.turns = c;
        }
    });
    if status != prev_status && !status.is_empty() {
        let title = node.agent.get(&id).map(|t| t.title).unwrap_or_default();
        node.emit("agent", json!({ "thread": id, "role": "out", "peer": from, "status": status, "title": title }));
        if status == "idle" || status == "failed" || status == "stopped" {
            node.info(&format!("Агент: {} — {} «{}»", from, match status.as_str() { "idle" => "ход завершён", "failed" => "не получилось", _ => "остановлено" }, title));
        }
    }
    Ok(())
}

// ───────────────────────── Исполнитель ─────────────────────────

/// Поручение от соседа.
pub fn on_ask(node: &Arc<Node>, from: &str, v: &Value) -> Result<Value, (u16, String)> {
    let allowed = node.cfg.read().agent_allows(from);
    if node.cfg.read().agent.mode != "off" && !allowed {
        return Err((403, format!("«{}» не разрешено поручать агенту на «{}» (панель → «Агенты» → кто может поручать)", from, node.node_name())));
    }
    enqueue(node, from, v, false)
}

/// Поставить поручение в очередь исполнителя (от соседа или себе — `local`).
fn enqueue(node: &Arc<Node>, from: &str, v: &Value, local: bool) -> Result<Value, (u16, String)> {
    let (mode, max_par) = {
        let c = node.cfg.read();
        (c.agent.mode.clone(), c.agent.max_parallel.max(1) as usize)
    };
    if mode == "off" {
        return Err((403, format!("агент на «{}» выключен (панель → «Агенты» → включить)", node.node_name())));
    }
    let id = v.get("thread").and_then(|s| s.as_str()).unwrap_or("").trim().to_string();
    if id.is_empty() || id.len() > 120 {
        return Err((400, "нужен id нити".into()));
    }
    let prompt = v.get("prompt").and_then(|s| s.as_str()).unwrap_or("").to_string();
    if prompt.trim().is_empty() {
        return Err((400, "пустое поручение".into()));
    }
    if let Some(t) = node.agent.get(&id) {
        if t.role != "in" || !eq_ic(&t.peer, from) || t.local != local {
            return Err((409, "такая нить уже есть у другого".into()));
        }
        if t.busy() {
            return Err((409, "агент ещё работает над этой нитью".into()));
        }
    }
    let auto = mode == "auto";
    if auto && node.agent.running_count() >= max_par {
        // не отказ: встанет в очередь и начнётся, когда освободится место
    }
    {
        let mut g = node.agent.threads.lock();
        let t = g.entry(id.clone()).or_insert_with(|| Thread {
            id: id.clone(),
            role: "in".into(),
            peer: from.to_string(),
            title: v.get("title").and_then(|s| s.as_str()).unwrap_or("").into(),
            created: Some(time::now()),
            exchange_owner: v.get("exchangeOwner").and_then(|s| s.as_str()).unwrap_or("").into(),
            exchange_id: v.get("exchangeId").and_then(|s| s.as_str()).unwrap_or("").into(),
            local,
            ..Default::default()
        });
        t.status = "queued".into();
        t.taken = false;
        t.error.clear();
        t.phase.clear();
        t.asks = v.get("ask").and_then(|x| x.as_i64()).unwrap_or(t.asks + 1).max(t.asks + 1);
        let rec = clean_record(&v["record"]);
        if !rec.is_null() {
            t.record = rec;
        }
        t.push("prompt", prompt.clone());
    }
    node.agent.update(&id, |_| ());
    let title = node.agent.get(&id).map(|t| t.title).unwrap_or_default();
    if local {
        node.info(&format!("Агент: поручение этому ПК: {}", title));
    } else {
        node.info(&format!("Агент: поручение от «{}»: {}", from, title));
    }
    node.emit("agent", json!({ "thread": id, "role": "in", "peer": from, "local": local, "status": "queued", "title": title, "mode": mode }));
    if auto {
        schedule(node);
    }
    Ok(json!({ "ok": true, "thread": id, "status": "queued", "mode": mode }))
}

/// Запустить ждущие поручения (auto), сколько позволяет max_parallel.
pub fn schedule(node: &Arc<Node>) {
    let (mode, max_par) = {
        let c = node.cfg.read();
        (c.agent.mode.clone(), c.agent.max_parallel.max(1) as usize)
    };
    if mode != "auto" {
        return;
    }
    let queued: Vec<String> = {
        let g = node.agent.threads.lock();
        let mut q: Vec<&Thread> = g.values().filter(|t| t.role == "in" && t.status == "queued").collect();
        q.sort_by(|a, b| a.updated.cmp(&b.updated));
        q.iter().map(|t| t.id.clone()).collect()
    };
    for id in queued {
        if node.agent.running_count() >= max_par {
            break;
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        node.agent.procs.lock().insert(id.clone(), Proc { pid: None, cancel: cancel.clone() });
        node.agent.update(&id, |t| t.status = "running".into());
        let n2 = node.clone();
        tokio::spawn(async move {
            run_claude(&n2, &id, cancel).await;
            n2.agent.procs.lock().remove(&id);
            schedule(&n2);
        });
    }
}

pub fn stop_local(node: &Arc<Node>, id: &str, why: &str) {
    let p = node.agent.procs.lock().remove(id);
    if let Some(p) = p {
        p.cancel.cancel();
        if let Some(pid) = p.pid {
            kill_tree(pid);
        }
    }
    node.agent.update(id, |t| {
        if t.busy() {
            t.status = "stopped".into();
            t.push("info", why.to_string());
        }
    });
}

fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).creation_flags(0x0800_0000).status();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status();
    }
}

/// Где claude: настройка, PATH, обычные места установки.
pub fn find_claude(node: &Node) -> Option<PathBuf> {
    let set = node.cfg.read().agent.claude.clone();
    if !set.trim().is_empty() {
        let p = PathBuf::from(set.trim());
        return p.is_file().then_some(p);
    }
    let names: &[&str] = if cfg!(windows) { &["claude.exe"] } else { &["claude"] };
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            for n in names {
                let f = d.join(n);
                if f.is_file() {
                    return Some(f);
                }
            }
        }
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)?;
    for rel in [".local/bin/claude.exe", ".local/bin/claude", ".claude/local/claude.exe", ".claude/local/claude"] {
        let f = home.join(rel);
        if f.is_file() {
            return Some(f);
        }
    }
    None
}

/// Папка, где работает Claude.
pub fn work_dir(node: &Node) -> PathBuf {
    let w = node.cfg.read().agent.work_dir.clone();
    let p = if w.trim().is_empty() { node.base_dir.join("агент") } else { node.resolve(w.trim()) };
    let _ = std::fs::create_dir_all(&p);
    p
}

/// Обменная папка нити на этом ПК (у исполнителя — копия общей папки оркестратора).
pub fn exchange_path(node: &Node, t: &Thread) -> Option<PathBuf> {
    if t.exchange_id.is_empty() {
        return None;
    }
    if eq_ic(&t.exchange_owner, &node.node_name()) {
        let c = node.cfg.read().clone();
        return c.folder(&t.exchange_id).map(|f| node.folders.own_root(node, f));
    }
    Some(node.folders.mount_root(node).join(&t.exchange_owner).join(&t.exchange_id))
}

/// Что Claude-исполнитель должен знать о своём положении.
pub fn context_prompt(node: &Node, t: &Thread) -> String {
    let ex = exchange_path(node, t);
    if t.local {
        let mut s = format!(
            "Ты — Claude-исполнитель на ПК «{me}». Поручение поставил хозяин этого ПК (или его Claude) через «Мост». \
             Человека рядом может не быть: не задавай вопросов и не жди подтверждений — делай, а если чего-то не хватает, так и напиши в отчёте.\n\
             Рабочая папка: {wd}.\n",
            me = node.node_name(),
            wd = work_dir(node).display()
        );
        if let Some(p) = ex {
            s.push_str(&format!("Обменная папка этого ПК: {}. Клади туда логи, отчёты, скриншоты, патчи — всё, что нужно показать.\n", p.display()));
        }
        return finish_prompt(s, t);
    }
    let mut s = format!(
        "Ты — Claude-исполнитель на ПК «{me}». Поручения приходят от Claude-оркестратора на ПК «{from}» через «Мост» (связь между ПК). \
         Человека рядом нет: не задавай вопросов и не жди подтверждений — делай, а если чего-то не хватает, так и напиши в отчёте.\n\
         Рабочая папка: {wd}.\n",
        me = node.node_name(),
        from = t.peer,
        wd = work_dir(node).display()
    );
    if let Some(p) = ex {
        s.push_str(&format!(
            "Обменная папка (общая с «{from}», он видит её у себя): {p}. Клади туда всё, что нужно показать оркестратору: логи, отчёты, скриншоты, \
             патчи. Файлы от оркестратора тоже появляются там.\n",
            from = t.peer,
            p = p.display()
        ));
    }
    finish_prompt(s, t)
}

/// Сервер инструментов моста для запущенного Claude (если bridge.exe рядом).
fn mcp_config(node: &Node) -> Option<String> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let exe = dir.join(if cfg!(windows) { "bridge.exe" } else { "bridge" });
    if !exe.is_file() {
        return None;
    }
    let cfgf = node.config_file.clone().unwrap_or_else(|| node.base_dir.join("bridge.json"));
    Some(json!({ "mcpServers": { "most": { "command": exe.display().to_string(), "args": ["mcp", "--config", cfgf.display().to_string()] } } }).to_string())
}

fn finish_prompt(mut s: String, t: &Thread) -> String {
    s.push_str("Отмечай этапы работы — не на каждое действие, а когда переходишь к новой части: отдельной строкой в начале сообщения «Этап: <коротко, что делаешь>», например «Этап: ищу информацию», «Этап: сверяю данные». \
                Если ждёшь (долгая сборка, ответ сервиса, процесс в фоне) — строкой «Ожидание: <чего ждёшь>». Это видно в мосте как твой статус.\n\
                Если нужно решение или действие человека (дождаться, пока он закончит, выбрать вариант, подтвердить) — не жди вслепую: вызови инструмент моста agent_request(question, buttons, input) — \
                люди увидят вопрос с кнопками и полем ответа, а ответ вернётся тебе. Пока ждёшь ответа, не гадай.\n");
    if t.record.is_object() {
        s.push_str(&format!("Поручение — запись-задача {} в пространстве «{}»: её статус мост ведёт сам (в работе → на проверке), «выполнено» ставит заказчик.\n", t.record["id"].as_str().unwrap_or(""), t.record["share"].as_str().unwrap_or("")));
    }
    s.push_str("В конце каждого хода — короткий отчёт: что сделано, итог (получилось / нет и почему), какие файлы где лежат.");
    s
}

async fn run_claude(node: &Arc<Node>, id: &str, cancel: tokio_util::sync::CancellationToken) {
    let Some(t) = node.agent.get(id) else { return };
    let prompt = t.events.iter().rev().find(|e| e.kind == "prompt").map(|e| e.text.clone()).unwrap_or_default();
    let Some(exe) = find_claude(node) else {
        fail(node, id, "не найден Claude Code (claude.exe). Установите: PowerShell → irm https://claude.ai/install.ps1 | iex, затем один раз запустите claude и войдите. Или укажите путь в «Агенты» → «Путь к claude».");
        return;
    };
    let cfg = node.cfg.read().agent.clone();
    let mut first = true;
    loop {
        let mut args: Vec<String> = vec!["-p".into(), "--output-format".into(), "stream-json".into(), "--verbose".into()];
        let pm = if cfg.permission_mode.trim().is_empty() { "auto".to_string() } else { cfg.permission_mode.clone() };
        args.push("--permission-mode".into());
        args.push(pm);
        let with_flag = node.agent.no_prompts_flag.load(std::sync::atomic::Ordering::Relaxed);
        if with_flag {
            args.push("--permission-prompts".into());
            args.push("none".into());
        }
        if cfg.max_budget_usd > 0.0 {
            args.push("--max-budget-usd".into());
            args.push(format!("{}", cfg.max_budget_usd));
        }
        // инструменты моста — чтобы агент мог спросить людей (agent_request)
        if let Some(mc) = mcp_config(node) {
            args.push("--mcp-config".into());
            args.push(mc);
            args.push("--allowedTools".into());
            args.push("mcp__most__agent_request".into());
        }
        args.push("--append-system-prompt".into());
        args.push(context_prompt(node, &t));
        if !t.session.is_empty() {
            args.push("--resume".into());
            args.push(t.session.clone());
        }
        args.extend(cfg.extra_args.iter().cloned());
        let outcome = run_once(node, id, &exe, &args, &prompt, &cfg, cancel.clone()).await;
        match outcome {
            RunOutcome::Done => return,
            RunOutcome::UnknownFlag if first && with_flag => {
                // старая версия Claude Code без --permission-prompts — без него
                node.agent.no_prompts_flag.store(false, std::sync::atomic::Ordering::Relaxed);
                first = false;
                continue;
            }
            RunOutcome::UnknownFlag => {
                fail(node, id, "Claude Code не принял параметры запуска — обновите его: claude update");
                return;
            }
        }
    }
}

enum RunOutcome {
    Done,
    UnknownFlag,
}

fn fail(node: &Arc<Node>, id: &str, why: &str) {
    node.info(&format!("Агент: {} — {}", id, why));
    node.agent.update(id, |t| {
        t.status = "failed".into();
        t.error = why.to_string();
        t.push("error", why.to_string());
    });
}

#[allow(clippy::too_many_arguments)]
async fn run_once(node: &Arc<Node>, id: &str, exe: &Path, args: &[String], prompt: &str, cfg: &bridge_core::config::AgentConfig, cancel: tokio_util::sync::CancellationToken) -> RunOutcome {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.env("BRIDGE_AGENT_THREAD", id);
    cmd.args(args).current_dir(work_dir(node)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            fail(node, id, &format!("не удалось запустить {}: {}", exe.display(), e));
            return RunOutcome::Done;
        }
    };
    if let Some(p) = node.agent.procs.lock().get_mut(id) {
        p.pid = child.id();
    }
    node.agent.update(id, |t| t.push("info", "Claude запущен"));
    // поручение — через stdin (длинный текст не упрётся в предел командной строки)
    if let Some(mut si) = child.stdin.take() {
        let p = prompt.to_string();
        tokio::spawn(async move {
            let _ = si.write_all(p.as_bytes()).await;
            let _ = si.shutdown().await;
        });
    }
    let stderr = child.stderr.take();
    let err_buf = Arc::new(Mutex::new(String::new()));
    if let Some(mut se) = stderr {
        let eb = err_buf.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            while let Ok(n) = se.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut g = eb.lock();
                g.push_str(&String::from_utf8_lossy(&buf[..n]));
                if g.len() > 8000 {
                    let cut = g.len() - 8000;
                    let cut = (cut..g.len()).find(|i| g.is_char_boundary(*i)).unwrap_or(g.len());
                    g.drain(..cut);
                }
            }
        });
    }
    let stdout = child.stdout.take().expect("stdout");
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let deadline = tokio::time::Instant::now() + Duration::from_secs((cfg.max_minutes.max(1) * 60) as u64);
    let mut got_result = false;
    let mut timed_out = false;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => { let _ = child.start_kill(); break; }
            _ = tokio::time::sleep_until(deadline) => { timed_out = true; if let Some(pid) = child.id() { kill_tree(pid); } let _ = child.start_kill(); break; }
            l = lines.next_line() => match l {
                Ok(Some(line)) => { if handle_line(node, id, &line) { got_result = true; } }
                _ => break,
            }
        }
    }
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.ok().and_then(|r| r.ok());
    if cancel.is_cancelled() {
        return RunOutcome::Done; // stop_local уже отметил
    }
    let err_text = err_buf.lock().clone();
    if timed_out {
        fail(node, id, &format!("превышено время ({} мин) — остановлено; продолжите поручением в эту же нить", cfg.max_minutes));
        return RunOutcome::Done;
    }
    if !got_result {
        let low = err_text.to_lowercase();
        if low.contains("unknown option") && low.contains("permission-prompts") {
            return RunOutcome::UnknownFlag;
        }
        let tail: String = err_text.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        let hint = if low.contains("login") || low.contains("authenticat") || low.contains("/login") {
            " — войдите в Claude Code на этом ПК: запустите claude в консоли и выполните вход"
        } else {
            ""
        };
        fail(node, id, &format!("Claude завершился без итога (код {}){}{}", status.and_then(|s| s.code()).map(|c| c.to_string()).unwrap_or("?".into()), hint, if tail.is_empty() { String::new() } else { format!(":\n{}", tail) }));
    }
    RunOutcome::Done
}

/// Одна строка stream-json. true — пришёл итог.
fn handle_line(node: &Arc<Node>, id: &str, line: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return false };
    let ty = v.get("type").and_then(|s| s.as_str()).unwrap_or("");
    match ty {
        "system" => {
            if v.get("subtype").and_then(|s| s.as_str()) == Some("init") {
                if let Some(s) = v.get("session_id").and_then(|s| s.as_str()) {
                    let s = s.to_string();
                    node.agent.update(id, |t| t.session = s);
                }
            }
            false
        }
        "assistant" => {
            let blocks = v.pointer("/message/content").and_then(|c| c.as_array()).cloned().unwrap_or_default();
            // ход субагентов — только инструменты, без текста (иначе лог тонет)
            let sub = v.get("parent_tool_use_id").map(|p| !p.is_null()).unwrap_or(false);
            node.agent.update(id, |t| {
                for b in &blocks {
                    match b.get("type").and_then(|s| s.as_str()) {
                        Some("text") if !sub => {
                            let txt = b.get("text").and_then(|s| s.as_str()).unwrap_or("").trim();
                            if !txt.is_empty() {
                                t.push("text", txt.to_string());
                                if let Some((p, w)) = phase_of(txt) {
                                    t.set_phase(&p, w);
                                }
                            }
                        }
                        Some("tool_use") => {
                            // снова действует — уже не ждёт
                            if t.status == "waiting" {
                                t.status = "running".into();
                            }
                            t.push("tool", tool_line(b))
                        }
                        _ => {}
                    }
                }
            });
            false
        }
        "user" => {
            // ошибки инструментов — коротко
            let blocks = v.pointer("/message/content").and_then(|c| c.as_array()).cloned().unwrap_or_default();
            for b in blocks {
                if b.get("type").and_then(|s| s.as_str()) == Some("tool_result") && b.get("is_error").and_then(|x| x.as_bool()) == Some(true) {
                    let txt = match b.get("content") {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Array(a)) => a.iter().filter_map(|x| x.get("text").and_then(|s| s.as_str())).collect::<Vec<_>>().join(" "),
                        _ => String::new(),
                    };
                    let short: String = txt.chars().take(300).collect();
                    node.agent.update(id, |t| t.push("info", format!("инструмент: {}", short)));
                }
            }
            false
        }
        "result" => {
            let is_err = v.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false) || v.get("subtype").and_then(|s| s.as_str()).map(|s| s != "success").unwrap_or(false);
            let result = v.get("result").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let cost = v.get("total_cost_usd").and_then(|c| c.as_f64()).unwrap_or(0.0);
            let turns = v.get("num_turns").and_then(|c| c.as_i64()).unwrap_or(0);
            let session = v.get("session_id").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let sub = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("").to_string();
            node.agent.update(id, |t| {
                if !session.is_empty() {
                    t.session = session;
                }
                t.cost = cost;
                t.turns += turns.max(1);
                if is_err {
                    t.status = "failed".into();
                    t.error = if result.is_empty() { format!("Claude: {}", sub) } else { result.clone() };
                    t.push("error", t.error.clone());
                } else {
                    t.status = "idle".into();
                    t.result = result.clone();
                    t.push("result", result.clone());
                }
                t.phase.clear();
            });
            let title = node.agent.get(id).map(|t| t.title).unwrap_or_default();
            node.info(&format!("Агент: {} «{}» (${:.2})", if is_err { "не получилось" } else { "готово" }, title, cost));
            node.emit("agent", json!({ "thread": id, "role": "in", "status": if is_err { "failed" } else { "idle" }, "title": title }));
            true
        }
        _ => false,
    }
}

/// «Bash: cargo test», «Read: src/main.rs» — коротко, что делает инструмент.
fn tool_line(b: &Value) -> String {
    let name = b.get("name").and_then(|s| s.as_str()).unwrap_or("?");
    let inp = b.get("input").cloned().unwrap_or(Value::Null);
    let pick = ["command", "file_path", "path", "pattern", "url", "query", "description", "prompt"].iter().find_map(|k| inp.get(*k).and_then(|s| s.as_str()).map(String::from)).unwrap_or_default();
    let short: String = pick.lines().next().unwrap_or("").chars().take(160).collect();
    if short.is_empty() {
        name.to_string()
    } else {
        format!("{}: {}", name, short)
    }
}

// ───────────────────────── Ручной режим (сессия Claude на этом ПК) ─────────────────────────

pub fn inbox(node: &Node) -> Vec<Value> {
    let g = node.agent.threads.lock();
    g.values().filter(|t| t.role == "in" && t.status == "queued").map(|t| t.since(-1)).collect()
}

/// Взять поручение: вернуть его текст и контекст.
pub fn take(node: &Arc<Node>, id: &str) -> Result<Value, (u16, String)> {
    let t = node.agent.get(id).ok_or((404, "нет такого поручения".to_string()))?;
    if t.role != "in" {
        return Err((409, "это не поручение этому ПК".into()));
    }
    if t.status != "queued" {
        return Err((409, format!("поручение уже {}", match t.status.as_str() { "running" => "в работе", "idle" => "выполнено", _ => "закрыто" })));
    }
    node.agent.update(id, |t| {
        t.status = "running".into();
        t.taken = true;
        t.push("info", "взято в работу (сессия Claude на этом ПК)");
    });
    let t = node.agent.get(id).unwrap();
    let prompt = t.events.iter().rev().find(|e| e.kind == "prompt").map(|e| e.text.clone()).unwrap_or_default();
    Ok(json!({ "thread": id, "from": t.peer, "title": t.title, "prompt": prompt, "context": context_prompt(node, &t),
               "exchange": exchange_path(node, &t).map(|p| p.display().to_string()), "workDir": work_dir(node).display().to_string(),
               "history": t.events.iter().filter(|e| e.kind == "prompt" || e.kind == "result").map(|e| json!({ "kind": e.kind, "text": e.text })).collect::<Vec<_>>() }))
}

pub fn progress(node: &Arc<Node>, id: &str, text: &str) -> Result<(), (u16, String)> {
    progress_full(node, id, text, "", None)
}

/// Ход (ручной режим): text — что делается; phase — этап (пишется не на каждое действие, а на «абзац»);
/// waiting — явно ждёт (true), снова работает (false), не менять (None).
pub fn progress_full(node: &Arc<Node>, id: &str, text: &str, phase: &str, waiting: Option<bool>) -> Result<(), (u16, String)> {
    let t = node.agent.get(id).ok_or((404, "нет такого поручения".to_string()))?;
    if t.role != "in" || !(t.status == "running" || t.status == "waiting") {
        return Err((409, "поручение не в работе — сначала agent_take".into()));
    }
    node.agent.update(id, |t| {
        if !text.trim().is_empty() {
            t.push("text", text.to_string());
        }
        let w = waiting.unwrap_or(t.status == "waiting");
        if !phase.trim().is_empty() {
            t.set_phase(phase, w);
        } else if let Some(w) = waiting {
            t.status = if w { "waiting".into() } else { "running".into() };
        }
    });
    Ok(())
}

pub fn report(node: &Arc<Node>, id: &str, result: &str, ok: bool) -> Result<(), (u16, String)> {
    let t = node.agent.get(id).ok_or((404, "нет такого поручения".to_string()))?;
    if t.role != "in" || !(t.status == "running" || t.status == "waiting") {
        return Err((409, "поручение не в работе — сначала agent_take".into()));
    }
    node.agent.update(id, |t| {
        t.turns += 1;
        t.phase.clear();
        if ok {
            t.status = "idle".into();
            t.result = result.to_string();
            t.push("result", result.to_string());
        } else {
            t.status = "failed".into();
            t.error = result.to_string();
            t.push("error", result.to_string());
        }
    });
    node.emit("agent", json!({ "thread": id, "role": "in", "status": if ok { "idle" } else { "failed" }, "title": t.title }));
    Ok(())
}

// ───────────────────────── Вопросы агента людям (3.3.1) ─────────────────────────

/// Агент (исполнитель или поручение себе) спрашивает людей: вопрос, кнопки, поле ответа.
pub fn request(node: &Arc<Node>, id: &str, q: &str, buttons: &[String], input: bool) -> Result<Req, (u16, String)> {
    let t = node.agent.get(id).ok_or((404, "нет такой нити".to_string()))?;
    if t.role != "in" {
        return Err((409, "спрашивает исполнитель — это нить поручения соседу".into()));
    }
    if !t.busy() {
        return Err((409, "нить не в работе".into()));
    }
    let q = q.trim();
    if q.is_empty() || q.chars().count() > 2000 {
        return Err((400, "вопрос: от 1 до 2000 знаков".into()));
    }
    let buttons: Vec<String> = buttons.iter().map(|b| b.trim().chars().take(60).collect::<String>()).filter(|b| !b.is_empty()).take(8).collect();
    let input = input || buttons.is_empty();
    let rq = Req { id: format!("q{}", t.requests.len() + 1), q: q.to_string(), buttons, input, at: Some(time::now()), ..Default::default() };
    let r2 = rq.clone();
    node.agent.update(id, move |t| {
        // прошлые неотвеченные — закрыть: агент спрашивает заново
        for o in t.requests.iter_mut().filter(|o| o.answered.is_none()) {
            o.answered = Some(time::now());
            o.by = "—".into();
        }
        t.requests.push(r2.clone());
        if t.requests.len() > 100 {
            t.requests.remove(0);
        }
        t.push("ask", r2.q.clone());
        let short: String = r2.q.chars().take(120).collect();
        t.set_phase(&format!("ждёт ответа: {}", short), true);
    });
    node.info(&format!("Агент ждёт ответа людей: {}", q.chars().take(120).collect::<String>()));
    node.emit("agent", json!({ "thread": id, "role": "in", "ask": q, "title": t.title }));
    Ok(rq)
}

/// Ответ на вопрос агента. У исполнителя — записать; у заказчика — переслать исполнителю.
pub async fn answer(node: &Arc<Node>, id: &str, req: &str, text: &str, by: &str) -> Result<(), (u16, String)> {
    let t = node.agent.get(id).ok_or((404, "нет такой нити".to_string()))?;
    let text = text.trim();
    if text.is_empty() || text.chars().count() > 4000 {
        return Err((400, "ответ: от 1 до 4000 знаков".into()));
    }
    if t.role == "out" {
        let pc = node.cfg.read().peer(&t.peer).cloned().ok_or((404, "нет соседа".to_string()))?;
        let client = node.client_for(&pc);
        return match tokio::time::timeout(Duration::from_secs(15), client.post_json("/peer/agent/answer", &json!({ "thread": id, "req": req, "answer": text }))).await {
            Ok(Ok((code, _))) if (200..300).contains(&code) => {
                let _ = pull(node, id).await;
                Ok(())
            }
            Ok(Ok((code, body))) => Err((code, crate::client::error_text(&body, code))),
            Ok(Err(e)) => Err((502, crate::link::explain(&e, &pc).0)),
            Err(_) => Err((504, crate::link::explain_timeout(&pc).0)),
        };
    }
    let rq = t.requests.iter().find(|r| r.id == req).ok_or((404, "нет такого вопроса".to_string()))?;
    if rq.answered.is_some() {
        return Err((409, format!("уже ответили{}", if rq.by.is_empty() || rq.by == "—" { String::new() } else { format!(" ({})", rq.by) })));
    }
    if !rq.input && !rq.buttons.iter().any(|b| b == text) {
        return Err((400, "ответ — одна из кнопок".into()));
    }
    let (text, by) = (text.to_string(), by.to_string());
    node.agent.update(id, |t| {
        if let Some(r) = t.requests.iter_mut().find(|r| r.id == req) {
            r.answer = text.clone();
            r.by = by.clone();
            r.answered = Some(time::now());
        }
        t.push("answer", format!("{}: {}", by, text));
        if t.status == "waiting" {
            t.status = "running".into();
        }
        t.phase.clear();
        t.phase_at = Some(time::now());
    });
    node.emit("agent", json!({ "thread": id, "role": "in", "answered": req }));
    Ok(())
}

/// Дождаться ответа (до timeout). None — ответа нет (или нить закончилась).
pub async fn wait_answer(node: &Arc<Node>, id: &str, req: &str, timeout: Duration) -> Option<Req> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let t = node.agent.get(id)?;
        let r = t.requests.iter().find(|r| r.id == req)?.clone();
        if r.answered.is_some() {
            return Some(r);
        }
        if !t.busy() {
            return None;
        }
        let notified = node.agent.changed.notified();
        tokio::select! {
            _ = notified => {}
            _ = tokio::time::sleep_until(deadline) => return None,
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    }
}

// ───────────────────────── Доставка хода оркестратору ─────────────────────────

/// Раз в полсекунды: ход принятых нитей — оркестратору. Не дошло — повторим (позже он может и сам забрать нить).
pub async fn pusher(node: Arc<Node>) {
    loop {
        tokio::select! {
            _ = node.shutdown.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
        let ids: Vec<String> = node.agent.dirty.lock().drain().collect();
        for id in ids {
            let Some(t) = node.agent.get(&id) else { continue };
            let sent = node.agent.pushed.lock().get(&id).copied().unwrap_or(-1);
            let evs: Vec<&Ev> = t.events.iter().filter(|e| e.n > sent).collect();
            // запись-задача: исполнитель сам ведёт её статус (и при поручении себе)
            if t.role == "in" && t.record.is_object() {
                crate::agent::record_follow(&node, &t);
            }
            if t.local {
                continue; // соседа нет — ход уже здесь
            }
            let Some(pc) = node.cfg.read().peer(&t.peer).cloned() else { continue };
            if !pc.has_url() {
                continue;
            }
            let body = json!({ "thread": t.id, "title": t.title, "status": t.status, "session": t.session, "result": t.result, "error": t.error, "cost": t.cost, "turns": t.turns, "events": evs, "asks": t.asks, "phase": t.phase, "requests": t.requests });
            let client = node.client_for(&pc);
            match tokio::time::timeout(Duration::from_secs(15), client.post_json("/peer/agent/events", &body)).await {
                Ok(Ok((code, _))) if (200..300).contains(&code) => {
                    node.agent.pushed.lock().insert(id.clone(), t.last_n());
                }
                _ => {
                    node.agent.dirty.lock().insert(id.clone()); // повторить
                }
            }
        }
    }
}

/// Дождаться перемен в нити: пока она в работе и новых событий после `after` нет — ждать до `timeout`.
pub async fn wait(node: &Arc<Node>, id: &str, after: i64, timeout: Duration) -> Option<Thread> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let t = node.agent.get(id)?;
        let fresh = t.events.iter().any(|e| e.n > after && (e.kind == "result" || e.kind == "error"));
        if !t.busy() || fresh {
            return Some(t);
        }
        let notified = node.agent.changed.notified();
        tokio::select! {
            _ = notified => {}
            _ = tokio::time::sleep_until(deadline) => return node.agent.get(id),
            _ = tokio::time::sleep(Duration::from_secs(15)) => {
                // давно тихо — вдруг ход не дошёл: забрать нить у исполнителя
                if t.role == "out" { let _ = pull(node, id).await; }
            }
        }
    }
}

/// Забрать нить у исполнителя целиком (если ход не доходил).
pub async fn pull(node: &Arc<Node>, id: &str) -> Result<(), String> {
    let t = node.agent.get(id).ok_or("нет нити")?;
    let pc = node.cfg.read().peer(&t.peer).cloned().ok_or("нет соседа")?;
    let client = node.client_for(&pc);
    let (code, text) = tokio::time::timeout(Duration::from_secs(15), client.get_text(&format!("/peer/agent/thread?id={}", crate::client::esc(id)))).await.map_err(|_| "нет ответа".to_string())?.map_err(|e| e.to_string())?;
    if !(200..300).contains(&code) {
        return Err(crate::client::error_text(&text, code));
    }
    let remote: Thread = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    // ход исполнителя целиком, наши поручения — из его копии (там они тоже есть)
    let mut g = node.agent.threads.lock();
    if let Some(t) = g.get_mut(id) {
        if remote.asks < t.asks {
            return Ok(()); // у исполнителя ещё прошлое поручение
        }
        if remote.events.len() > t.events.len() || remote.status != t.status {
            t.events = remote.events;
            t.status = remote.status;
            t.session = remote.session;
            t.result = remote.result;
            t.error = remote.error;
            t.cost = remote.cost;
            t.turns = remote.turns;
            t.phase = remote.phase;
            t.phase_at = remote.phase_at;
            t.requests = remote.requests;
            t.updated = Some(time::now());
            let snap = t.clone();
            drop(g);
            node.agent.save(&snap);
            node.agent.changed.notify_waiters();
        }
    }
    Ok(())
}

/// Строка этапа в тексте агента: «Этап: …» / «Статус: …» → (текст, false); «Ожидание: …» / «Жду: …» → (текст, true).
pub fn phase_of(text: &str) -> Option<(String, bool)> {
    let mut found = None;
    for l in text.lines() {
        let l = l.trim().trim_start_matches(['*', '_', '#', '>', '-', ' ']);
        let low = l.to_lowercase();
        for (p, w) in [("этап:", false), ("статус:", false), ("phase:", false), ("ожидание:", true), ("жду:", true), ("waiting:", true)] {
            if low.starts_with(p) {
                let rest: String = l.chars().skip(p.chars().count()).collect::<String>().trim().trim_matches(['*', '_']).trim().to_string();
                if !rest.is_empty() {
                    found = Some((rest, w));
                }
            }
        }
    }
    found
}

/// Ссылка на запись: {share, id} (только понятные значения), иначе Null.
pub fn clean_record(v: &Value) -> Value {
    let share = v["share"].as_str().unwrap_or("").trim();
    let id = v["id"].as_str().unwrap_or("").trim();
    if id.is_empty() || id.len() > 120 || share.len() > 80 {
        return Value::Null;
    }
    json!({ "share": share, "id": id })
}

/// Исполнитель ведёт статус записи-задачи по ходу поручения: в работе / ожидание → «в работе» (через «принята»),
/// ход завершён → «на проверке» с итогом, не получилось → комментарий. «Готово» ставит только заказчик.
pub fn record_follow(node: &Arc<Node>, t: &Thread) {
    let key = format!("{}#{}#{}", t.id, t.asks, t.status);
    {
        let mut g = RECORD_SEEN.lock();
        if g.get(&t.id) == Some(&key) {
            return;
        }
        g.insert(t.id.clone(), key);
    }
    let share = t.record["share"].as_str().unwrap_or("").to_string();
    let rid = t.record["id"].as_str().unwrap_or("").to_string();
    let rt = match crate::shares::resolve_share(node, Some(share.as_str()).filter(|s| !s.is_empty()), Some(&rid)) {
        Ok(rt) => rt,
        Err(_) => return, // пространства нет на этом ПК — статус ведёт заказчик
    };
    let cur = rt.records.lock().get(&rid).and_then(|r| r.status().map(String::from)).unwrap_or_default();
    if cur.is_empty() {
        return;
    }
    let act = |to: Option<&str>, kind: &str, text: String| {
        let a = bridge_core::proto::BridgeAction {
            action: kind.into(),
            share: Some(rt.id.clone()),
            record: Some(rid.clone()),
            to: to.map(String::from),
            text: if kind == "comment" { Some(text.clone()) } else { None },
            note: if kind == "transition" { text } else { String::new() },
            actor: "агент".into(),
            ..Default::default()
        };
        crate::shares::act(node, a)
    };
    match t.status.as_str() {
        "running" | "waiting" => {
            if cur == "new" {
                act(Some("accepted"), "transition", "Агент взял задачу".into());
            }
            if cur == "new" || cur == "accepted" {
                act(Some("in_progress"), "transition", "Агент начал работу".into());
            }
        }
        "idle" => {
            if cur == "new" || cur == "accepted" {
                act(Some("accepted"), "transition", String::new());
                act(Some("in_progress"), "transition", String::new());
            }
            let res: String = t.result.chars().take(1500).collect();
            let r = act(Some("review"), "transition", format!("Агент: ход завершён — проверьте.\n{}", res));
            if !r.ok {
                act(None, "comment", format!("Агент: ход завершён.\n{}", res));
            }
        }
        "failed" => {
            act(None, "comment", format!("Агент: не получилось — {}", t.error.chars().take(800).collect::<String>()));
        }
        "stopped" => {
            act(None, "comment", "Агент: работа остановлена".into());
        }
        _ => {}
    }
}

static RECORD_SEEN: std::sync::LazyLock<Mutex<HashMap<String, String>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
