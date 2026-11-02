//! Файловый канал efc/1 (3.12) — для программ и агентов, у которых нет сети до моста (VM, песочница).
//!
//! - **Пространства.** У каждого пространства канал в его `_bridge/` (папка не синхронизируется): запрос
//!   `requests/<id>.json` с `command` = действие записи (`create`, `transition`, `comment`…) и `params` = поля
//!   действия, итог — `done/<id>.json`. Пакеты, шаги с подстановкой, отмена, пульс, корзина — по стандарту
//!   (efc-core). Старый `outbox/` работает как раньше, но устарел. Снимки (`status.json`, `index.json`,
//!   `records/`, `rejected.json`) объявлены в паспорте как `state`, `inbox/` — по стандарту.
//! - **Канал агента** — папка «agent-channel» рядом с мостом: команды = инструменты `bridge mcp` (те же
//!   правила: управлять ПК нельзя). Выключен по умолчанию, включается в панели («Агенты»).
//!
//! Указатель `.efc-channels.json` — в папке моста: `efc list` из неё находит все каналы.

use crate::node::Node;
use crate::tools::{self, LocalApi, Seen};
use bridge_core::proto::BridgeAction;
use efc_core::model::{CommandInfo, Op};
use efc_core::server::{Ctx, Handler, Outcome, Server, ServerConfig, StartError};
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

/// Папка канала агента рядом с мостом.
pub const AGENT_DIR: &str = "agent-channel";
const HOW_TO_START: &str = "Запустите Мост на этом ПК (Most.exe или bridge run)";
const RETRY: Duration = Duration::from_secs(60);

/// Клиент efc для Linux (статический) — кладётся в каждый канал, агенту в VM ничего ставить не нужно.
static EFC_LINUX: &[u8] = include_bytes!("../assets/efc");

struct Slot {
    server: Option<Server>,
    root: PathBuf,
    error: Option<String>,
    retry_at: Instant,
}

/// Каналы узла: по пространствам и канал агента. Владелец — узел; запускаются и гасятся в его цикле.
#[derive(Default)]
pub struct Channels {
    spaces: Mutex<HashMap<String, Slot>>,
    agent: Mutex<Option<Slot>>,
    client: Mutex<Option<PathBuf>>,
}

impl Channels {
    /// Состояние канала агента для панели.
    pub fn agent_view(&self, node: &Node) -> Value {
        let on = node.cfg.read().agent.file_channel;
        let g = self.agent.lock();
        let path = agent_root(node);
        json!({
            "on": on,
            "path": path.display().to_string(),
            "running": g.as_ref().map(|s| s.server.is_some()).unwrap_or(false),
            "error": g.as_ref().and_then(|s| s.error.clone()),
        })
    }

    /// Канал пространства для status.json: работает ли и почему нет.
    pub fn space_view(&self, share: &str) -> Value {
        match self.spaces.lock().get(share) {
            Some(s) => json!({ "running": s.server.is_some(), "error": s.error }),
            None => Value::Null,
        }
    }

    /// Погасить всё (остановка моста): `alive.json` убирается, клиенты увидят «сервер не запущен».
    pub fn stop_all(&self) {
        let mut servers: Vec<Server> = self.spaces.lock().drain().filter_map(|(_, s)| s.server).collect();
        if let Some(s) = self.agent.lock().take().and_then(|s| s.server) {
            servers.push(s);
        }
        for s in servers {
            s.stop(Duration::from_secs(2));
        }
    }
}

pub fn agent_root(node: &Node) -> PathBuf {
    node.base_dir.join(AGENT_DIR)
}

/// Привести каналы к настройкам: новые пространства — открыть, убранные — закрыть, канал агента — по флажку.
/// Зовётся из цикла узла (внутри рантайма tokio); дешёвый, если ничего не менялось.
pub fn sync(node: &Arc<Node>) {
    let ch = &node.efc;
    let shares = node.shares();
    let now = Instant::now();

    // пространства
    let mut gone: Vec<Server> = Vec::new();
    {
        let mut map = ch.spaces.lock();
        let ids: Vec<String> = shares.iter().map(|s| s.id.clone()).collect();
        let stale: Vec<String> = map.keys().filter(|k| !ids.contains(k)).cloned().collect();
        for k in stale {
            if let Some(s) = map.remove(&k).and_then(|s| s.server) {
                gone.push(s);
            }
        }
        for rt in &shares {
            let root = rt.root.join(bridge_core::SERVICE_DIR);
            let due = match map.get(&rt.id) {
                Some(s) if s.server.is_some() && s.root == root => false,
                Some(s) if s.server.is_none() && s.root == root => now >= s.retry_at,
                _ => true,
            };
            if !due {
                continue;
            }
            if let Some(old) = map.remove(&rt.id).and_then(|s| s.server) {
                gone.push(old); // папка пространства переехала
            }
            let cfg = space_config(node, &rt.id, &root);
            let handler = Arc::new(SpaceHandler { node: Arc::downgrade(node), share: rt.id.clone() });
            let slot = start(node, cfg, handler, &format!("пространство «{}»", rt.id), root);
            map.insert(rt.id.clone(), slot);
            // сервер efc при открытии кладёт свой общий README — вернуть полный README пространства
            rt.interface_dirty.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    // канал агента
    {
        let want = node.cfg.read().agent.file_channel;
        let mut g = ch.agent.lock();
        match (&*g, want) {
            (Some(s), true) if s.server.is_some() || now < s.retry_at => {}
            (None, false) => {}
            (Some(_), false) => {
                if let Some(s) = g.take().and_then(|s| s.server) {
                    gone.push(s);
                    node.info("Канал агента выключен");
                }
            }
            _ => {
                let root = agent_root(node);
                *g = Some(match AgentHandler::new(node) {
                    Ok(h) => start(node, agent_config(node, &root), Arc::new(h), "канал агента", root),
                    Err(e) => Slot { server: None, root, error: Some(e), retry_at: now + RETRY },
                });
            }
        }
    }

    if !gone.is_empty() {
        // остановка ждёт текущие вызовы — не в цикле узла
        std::thread::spawn(move || {
            for s in gone {
                s.stop(Duration::from_secs(5));
            }
        });
    }
}

fn start(node: &Arc<Node>, mut cfg: ServerConfig, handler: Arc<dyn Handler>, what: &str, root: PathBuf) -> Slot {
    let retry_at = Instant::now() + RETRY;
    // папку перенесли (переименование 3.13, другая папка пространства) — паспорт этого же моста на этом ПК
    // со старым путём: канал наш, забираем; паспорт с другого ПК — нет
    let mine = format!("bridge@{}:", efc_core::server::hostname());
    if efc_core::fsx::read_json::<efc_core::model::Channel>(&root.join("channel.json")).map(|c| c.instance.starts_with(&mine)).unwrap_or(false) {
        cfg.take_over = true;
    }
    match Server::start(cfg, handler) {
        Ok(s) => {
            node.info(&format!("Файловый канал efc/1: {} — {}", what, root.display()));
            Slot { server: Some(s), root, error: None, retry_at }
        }
        Err(e) => {
            let text = match e {
                StartError::ForeignInstance(other) => format!("в папке канал другого моста ({}) — удалите channel.json, если папку скопировали", other),
                other => other.to_string(),
            };
            node.info(&format!("Файловый канал efc/1: {} не открыт: {}", what, text));
            Slot { server: None, root, error: Some(text), retry_at }
        }
    }
}

/// Клиент efc для VM — один файл в папке данных, из него сервер раскладывает копии по каналам.
fn client_file(node: &Node) -> Vec<PathBuf> {
    let mut g = node.efc.client.lock();
    if g.is_none() {
        let p = node.data_dir.join("efc");
        if std::fs::read(&p).map(|b| b != EFC_LINUX).unwrap_or(true) {
            let _ = efc_core::fsx::write_atomic(&p, EFC_LINUX);
        }
        *g = Some(p);
    }
    g.iter().filter(|p| p.exists()).cloned().collect()
}

fn base(node: &Node, root: &Path, name: &str, title: &str) -> ServerConfig {
    let mut c = ServerConfig::new(root, "bridge", bridge_core::APP_VERSION);
    c.name = name.into();
    c.title = title.into();
    c.how_to_start = HOW_TO_START.into();
    c.index_dir = Some(node.base_dir.clone());
    c.clients = client_file(node);
    c.poll = Duration::from_millis(150);
    c
}

fn space_config(node: &Node, share: &str, root: &Path) -> ServerConfig {
    let mut c = base(node, root, &format!("space-{}", share), &format!("Мост — пространство «{}»", share));
    c.roots = BTreeMap::from([("space".to_string(), "..".to_string())]);
    c.state = BTreeMap::from([
        ("status".to_string(), "status.json".to_string()),
        ("index".to_string(), "index.json".to_string()),
        ("records".to_string(), "records/".to_string()),
        ("rejected".to_string(), "rejected.json".to_string()),
    ]);
    c.features = vec!["inbox".into(), "state".into()];
    c.reasons = vec!["action-failed".into()];
    c
}

fn agent_config(node: &Node, root: &Path) -> ServerConfig {
    let mut c = base(node, root, "bridge-agent", "Мост — канал агента (инструменты bridge mcp)");
    c.how_to_start = "Запустите Мост на этом ПК и включите в панели: «Агенты» → «Канал для агента без сети»".into();
    c.roots = BTreeMap::from([("bridge".to_string(), "..".to_string())]);
    c.reasons = vec!["tool-failed".into()];
    c
}

// ───────────── пространство: действия записей ─────────────

/// (команда, кратко, пример параметров)
const ACTIONS: &[(&str, &str, &str)] = &[
    ("create", "Создать запись", r#"{"type":"task","fields":{"title":"…","assignee":"<узел>","body":"…"},"note":"…"}"#),
    ("transition", "Сменить статус записи", r#"{"record":"<id>","to":"accepted|in_progress|review|done|…","note":"…"}"#),
    ("set", "Поменять поля записи", r#"{"record":"<id>","fields":{"поле":"значение"},"expectVersion":3}"#),
    ("comment", "Комментарий к записи", r#"{"record":"<id>","text":"…","replyTo":"<eid>"}"#),
    ("ack", "Подтвердить получение или отказать", r#"{"record":"<id>","level":"accepted|rejected|seen","text":"…"}"#),
    ("attach", "Приложить файл пространства", r#"{"record":"<id>","path":"папка/файл.ext","title":"…"}"#),
    ("link", "Связать записи", r#"{"record":"<id>","target":"<другой id>","rel":"relates"}"#),
    ("unlink", "Убрать связь", r#"{"record":"<id>","target":"<другой id>","rel":"relates"}"#),
    ("revert", "Отменить событие записи", r#"{"record":"<id>","target":"<eid>","note":"…"}"#),
    ("approve", "Разрешить команду (запись command)", r#"{"record":"<id>"}"#),
    ("reject", "Отклонить команду", r#"{"record":"<id>","note":"причина"}"#),
];

struct SpaceHandler {
    node: Weak<Node>,
    share: String,
}

impl Handler for SpaceHandler {
    fn commands(&self) -> Vec<CommandInfo> {
        ACTIONS
            .iter()
            .map(|(name, summary, usage)| CommandInfo { name: name.to_string(), summary: summary.to_string(), usage: format!("efc --params '{}' call {}", usage, name), ..Default::default() })
            .collect()
    }

    fn run(&self, ctx: &Ctx, op: &Op) -> Outcome {
        let Some(node) = self.node.upgrade() else {
            return Outcome::fail(4, "Мост останавливается", None);
        };
        let mut obj = match &op.params {
            Value::Object(m) => m.clone(),
            Value::Null => Map::new(),
            _ => return Outcome::fail(2, "params — объект с полями действия", Some("bad-request")),
        };
        obj.insert("action".into(), json!(op.command));
        // канал отвечает только за своё пространство
        obj.insert("share".into(), json!(self.share));
        let actor = obj.get("actor").and_then(|a| a.as_str()).unwrap_or("").to_string();
        if actor.is_empty() {
            obj.insert("actor".into(), json!(ctx.from.clone().filter(|f| !f.is_empty()).unwrap_or_else(|| "файловый канал".into())));
        }
        let a: BridgeAction = match serde_json::from_value(Value::Object(obj)) {
            Ok(a) => a,
            Err(e) => return Outcome::fail(2, format!("не удалось прочитать действие: {}", e), Some("bad-request")),
        };
        let label = format!("{} {}", a.action, a.record.clone().or(a.type_.clone()).unwrap_or_default());
        let r = crate::shares::act(&node, a);
        node.info(&format!("[{}] efc {}: {}", self.share, label.trim(), if r.ok { "выполнено" } else { r.error.as_str() }));
        let data = json!({ "record": r.record, "eid": r.eid, "version": r.version });
        if r.ok {
            let mut text = format!("{}: выполнено", label.trim());
            if let Some(id) = &r.record {
                text.push_str(&format!(" · запись {}", id));
            }
            Outcome::ok(text).with_data(data)
        } else {
            Outcome::fail(1, r.error, Some("action-failed")).with_data(data)
        }
    }

    fn detail(&self) -> Value {
        json!({ "share": self.share })
    }
}

// ───────────── канал агента: инструменты bridge mcp ─────────────

/// Только читают — проходят мимо блокировки `lock`.
const READ_ONLY: &[&str] = &["bridge_peers", "agent_status", "agent_threads", "exchange_folder", "agent_inbox"];
/// Ждут (до `wait_seconds`) — `long` в паспорте, ожидание урезается до срока вызова.
const LONG: &[&str] = &["agent_ask", "agent_wait", "agent_wait_task", "agent_request"];

struct AgentHandler {
    api: LocalApi,
    seen: Seen,
    rt: tokio::runtime::Handle,
}

impl AgentHandler {
    fn new(node: &Node) -> Result<AgentHandler, String> {
        Ok(AgentHandler {
            api: LocalApi::for_node(node)?,
            seen: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            rt: tokio::runtime::Handle::try_current().map_err(|e| e.to_string())?,
        })
    }
}

/// Первое предложение описания — для `help`.
fn summary(d: &str) -> String {
    let first = d.split(". ").next().unwrap_or(d);
    let mut s: String = first.chars().take(160).collect();
    if s.chars().count() < first.chars().count() {
        s.push('…');
    }
    s
}

fn usage(name: &str, schema: &Value) -> String {
    let req: Vec<&str> = schema["required"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    let props: Vec<String> = schema["properties"].as_object().map(|m| m.keys().map(|k| if req.contains(&k.as_str()) { k.clone() } else { format!("{}?", k) }).collect()).unwrap_or_default();
    if props.is_empty() {
        name.to_string()
    } else {
        format!("efc --params '{{{}}}' call {}", props.join(", "), name)
    }
}

impl Handler for AgentHandler {
    fn commands(&self) -> Vec<CommandInfo> {
        tools::tools()
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|t| {
                let name = t["name"].as_str().unwrap_or("").to_string();
                CommandInfo {
                    summary: summary(t["description"].as_str().unwrap_or("")),
                    usage: usage(&name, &t["inputSchema"]),
                    long: LONG.contains(&name.as_str()),
                    read_only: READ_ONLY.contains(&name.as_str()),
                    name,
                    ..Default::default()
                }
            })
            .collect()
    }

    fn run(&self, ctx: &Ctx, op: &Op) -> Outcome {
        let mut args = match &op.params {
            Value::Object(m) => Value::Object(m.clone()),
            Value::Null => json!({}),
            _ => return Outcome::fail(2, "params — объект с аргументами инструмента", Some("bad-request")),
        };
        if LONG.contains(&op.command.as_str()) {
            // ждать не дольше срока вызова (оставить 5 с на ответ)
            let left = ctx.remaining().as_secs().saturating_sub(5);
            let want = args.get("wait_seconds").and_then(|x| x.as_u64()).unwrap_or(if op.command == "agent_request" { 1800 } else { 300 });
            args["wait_seconds"] = json!(want.min(left));
        }
        let res = self.rt.block_on(async {
            tokio::select! {
                r = tools::call(&self.api, &self.seen, &op.command, &args) => Some(r),
                _ = async { while !ctx.cancelled() { tokio::time::sleep(Duration::from_millis(200)).await; } } => None,
            }
        });
        match res {
            None => Outcome::cancelled(),
            Some(Ok(text)) => {
                let data = serde_json::from_str::<Value>(&text).ok().filter(|v| v.is_object() || v.is_array()).unwrap_or(Value::Null);
                Outcome::ok(text).with_data(data)
            }
            Some(Err(e)) => Outcome::fail(1, e, Some("tool-failed")),
        }
    }
}
