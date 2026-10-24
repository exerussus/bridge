//! Исполнение команд, назначенных этому узлу. Выполняется только то, что описано в runners этого узла, от тех, кому разрешено,
//! с проверкой параметров; по умолчанию — после подтверждения человека.

use crate::node::Node;
use crate::shares::{self, ShareRt};
use bridge_core::config::RunnerConfig;
use bridge_core::proto::{record_types, BridgeAction, RecordState};
use bridge_core::{eq_ic, path, time};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingCommand {
    pub no: i64,
    pub share: String,
    pub record: String,
    pub from: String,
    pub runner: String,
    pub args: String,
    pub note: String,
    #[serde(with = "time::ser")]
    pub since: time::Time,
}

pub struct Runner {
    pub waiting: Mutex<BTreeMap<String, WaitingCommand>>,
    running: Mutex<HashSet<String>>,
    waiting_files: Mutex<HashSet<String>>,
    no: AtomicI64,
    cancel: tokio_util::sync::CancellationToken,
}

impl Default for Runner {
    fn default() -> Self {
        Self::new()
    }
}

fn key(share: &str, id: &str) -> String {
    format!("{}/{}", share, id)
}

impl Runner {
    pub fn new() -> Self {
        Runner {
            waiting: Mutex::new(BTreeMap::new()),
            running: Mutex::new(HashSet::new()),
            waiting_files: Mutex::new(HashSet::new()),
            no: AtomicI64::new(0),
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    pub fn waiting_count(&self) -> usize {
        self.waiting.lock().len()
    }

    pub fn is_waiting(&self, share: &str, id: &str) -> bool {
        self.waiting.lock().contains_key(&key(share, id))
    }

    pub fn waiting_list(&self) -> Vec<WaitingCommand> {
        self.waiting.lock().values().cloned().collect()
    }

    pub fn waiting_for_share(&self, share: &str) -> Vec<WaitingCommand> {
        self.waiting.lock().values().filter(|w| w.share == share).cloned().collect()
    }

    pub fn by_no(&self, no: i64) -> Option<WaitingCommand> {
        self.waiting.lock().values().find(|w| w.no == no).cloned()
    }

    fn mine(node: &Node, s: &RecordState) -> bool {
        s.type_ == record_types::COMMAND && eq_ic(s.assignee().unwrap_or(""), &node.node_name())
    }

    /// После перезапуска: «выполняется», но процесса нет — значит, прервано.
    pub fn recover(&self, node: &Arc<Node>) {
        for rt in node.shares() {
            let list: Vec<String> = rt.records.lock().all().into_iter().filter(|s| Self::mine(node, s) && s.status() == Some("running")).map(|s| s.id.clone()).collect();
            for id in list {
                let mut f = serde_json::Map::new();
                f.insert("result".into(), json!({ "message": "прервано перезапуском узла" }));
                shares::act(
                    node,
                    BridgeAction {
                        action: "transition".into(),
                        share: Some(rt.id.clone()),
                        record: Some(id),
                        to: Some("failed".into()),
                        actor: node.node_name(),
                        note: "узел перезапущен во время выполнения".into(),
                        fields: Some(f),
                        ..Default::default()
                    },
                );
            }
        }
    }

    /// Просмотреть команды пространства и сделать следующий шаг.
    pub fn check(&self, node: &Arc<Node>, rt: &Arc<ShareRt>) {
        let list: Vec<RecordState> = rt.records.lock().all().into_iter().filter(|s| Self::mine(node, s)).cloned().collect();
        for s in list {
            let k = key(&rt.id, &s.id);
            match s.status() {
                Some("pending") => {
                    if !self.waiting.lock().contains_key(&k) {
                        self.decide(node, rt, &s);
                    }
                }
                Some("accepted") => {
                    self.waiting.lock().remove(&k);
                    if !self.running.lock().contains(&k) {
                        self.start(node, rt, &s);
                    }
                }
                _ => {
                    self.waiting.lock().remove(&k);
                }
            }
        }
    }

    fn reject(&self, node: &Arc<Node>, rt: &Arc<ShareRt>, s: &RecordState, why: &str) {
        node.info(&format!("[{}] команда {} отклонена: {}", rt.id, s.id, why));
        shares::act(node, BridgeAction { action: "reject".into(), share: Some(rt.id.clone()), record: Some(s.id.clone()), actor: node.node_name(), note: why.into(), ..Default::default() });
    }

    fn decide(&self, node: &Arc<Node>, rt: &Arc<ShareRt>, s: &RecordState) {
        let runner_name = s.fields.get("runner").map(val_text).unwrap_or_default();
        let cfg = node.cfg.read().clone();
        let me = cfg.node.clone();
        let Some(rc) = bridge_core::config::get_ic(&cfg.runners, &runner_name).cloned() else {
            return self.reject(node, rt, s, &format!("исполнитель «{}» не настроен на узле {}", runner_name, me));
        };
        if !rc.enabled {
            return self.reject(node, rt, s, &format!("исполнитель «{}» выключен на узле {}", runner_name, me));
        }
        if !rc.share.is_empty() && !eq_ic(&rc.share, &rt.id) {
            return self.reject(node, rt, s, &format!("исполнитель «{}» не для этого пространства", runner_name));
        }
        if !rc.allows(&s.creator) {
            return self.reject(node, rt, s, &format!("узлу {} не разрешено просить «{}»", s.creator, runner_name));
        }
        let k = key(&rt.id, &s.id);
        if let Err(why) = build_args(rt, &rc, s) {
            // Файл-параметр мог ещё не доехать по мосту — ждём до 10 минут
            let age = time::now().signed_duration_since(s.created).num_seconds();
            if why.contains("нет ") && why.contains("в папке пространства") && age < 600 {
                if self.waiting_files.lock().insert(k) {
                    node.info(&format!("[{}] команда {} ждёт файл: {}", rt.id, s.id, why));
                }
                return;
            }
            self.waiting_files.lock().remove(&k);
            return self.reject(node, rt, s, &why);
        }
        self.waiting_files.lock().remove(&k);
        if !rc.confirm {
            shares::act(
                node,
                BridgeAction { action: "approve".into(), share: Some(rt.id.clone()), record: Some(s.id.clone()), actor: me, note: "без подтверждения (настройка исполнителя)".into(), ..Default::default() },
            );
            return;
        }
        let w = WaitingCommand {
            no: self.no.fetch_add(1, Ordering::SeqCst) + 1,
            share: rt.id.clone(),
            record: s.id.clone(),
            from: if s.creator_actor.is_empty() { s.creator.clone() } else { format!("{} ({})", s.creator, s.creator_actor) },
            runner: runner_name.clone(),
            args: s.fields.get("args").map(|v| v.to_string()).unwrap_or_else(|| "{}".into()),
            note: s.fields.get("title").map(val_text).unwrap_or_default(),
            since: time::now(),
        };
        self.waiting.lock().insert(k, w.clone());
        rt.interface_dirty.store(true, Ordering::SeqCst);
        node.info(&format!(
            "ЖДЁТ ПОДТВЕРЖДЕНИЯ [{}] {} просит «{}» {}{}\n    bridge approve {}  /  bridge reject {}",
            w.no,
            w.from,
            runner_name,
            w.args,
            if w.note.is_empty() { String::new() } else { format!(" — {}", w.note) },
            s.id,
            s.id
        ));
        node.emit("command-waiting", serde_json::to_value(&w).unwrap_or_default());
    }

    fn start(&self, node: &Arc<Node>, rt: &Arc<ShareRt>, s: &RecordState) {
        let k = key(&rt.id, &s.id);
        let runner_name = s.fields.get("runner").map(val_text).unwrap_or_default();
        let rc = bridge_core::config::get_ic(&node.cfg.read().runners, &runner_name).cloned();
        let Some(rc) = rc.filter(|r| r.enabled) else {
            fail(node, rt, &s.id, None, &format!("исполнитель «{}» недоступен", runner_name), None);
            return;
        };
        let args = match build_args(rt, &rc, s) {
            Ok(a) => a,
            Err(why) => {
                fail(node, rt, &s.id, None, &why, None);
                return;
            }
        };
        let res = shares::act(node, BridgeAction { action: "transition".into(), share: Some(rt.id.clone()), record: Some(s.id.clone()), to: Some("running".into()), actor: node.node_name(), ..Default::default() });
        if !res.ok {
            node.info(&format!("[{}] не удалось начать {}: {}", rt.id, s.id, res.error));
            return;
        }
        self.running.lock().insert(k.clone());
        let (node2, rt2, id) = (node.clone(), rt.clone(), s.id.clone());
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            execute(&node2, &rt2, &id, &rc, args, cancel).await;
            node2.runner.running.lock().remove(&k);
        });
    }

    pub async fn stop_all(&self) {
        self.cancel.cancel();
    }
}

fn val_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Аргументы процесса из шаблона и параметров команды.
pub fn build_args(rt: &ShareRt, rc: &RunnerConfig, s: &RecordState) -> Result<Vec<String>, String> {
    let empty = serde_json::Map::new();
    let given = s.fields.get("args").and_then(|v| v.as_object()).unwrap_or(&empty);
    for k in given.keys() {
        if bridge_core::config::get_ic(&rc.params, k).is_none() {
            return Err(format!("параметр «{}» не предусмотрен исполнителем", k));
        }
    }
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    for (name, type_) in &rc.params {
        let Some(raw) = given.get(name).filter(|v| !v.is_null()).map(val_text) else { return Err(format!("нет параметра «{}»", name)) };
        match type_.as_str() {
            "path" => {
                let p = path::normalize(&raw).map_err(|w| format!("параметр «{}»: {}", name, w))?;
                let full = path::to_local(&rt.root, &p).map_err(|e| e.to_string())?;
                if !full.exists() {
                    return Err(format!("параметр «{}»: нет {} в папке пространства", name, p));
                }
                values.insert(name.clone(), full.to_string_lossy().into_owned());
            }
            "int" => {
                let n: i64 = raw.trim().parse().map_err(|_| format!("параметр «{}»: нужно целое", name))?;
                values.insert(name.clone(), n.to_string());
            }
            "text" => {
                if raw.chars().count() > 2000 || raw.chars().any(|c| c.is_control()) {
                    return Err(format!("параметр «{}»: недопустимый текст", name));
                }
                values.insert(name.clone(), raw);
            }
            other => return Err(format!("тип параметра «{}» не поддерживается (path, int, text)", other)),
        }
    }
    let mut args = Vec::new();
    for t in &rc.args {
        let mut out = String::new();
        let mut rest = t.as_str();
        while let Some(i) = rest.find('{') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            match after.find('}') {
                Some(j) if after[..j].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && j > 0 => {
                    let name = &after[..j];
                    match values.get(name) {
                        Some(v) => out.push_str(v),
                        None => return Err(format!("в шаблоне исполнителя «{{{}}}», но такого параметра нет", name)),
                    }
                    rest = &after[j + 1..];
                }
                _ => {
                    out.push('{');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        args.push(out);
    }
    Ok(args)
}

async fn execute(node: &Arc<Node>, rt: &Arc<ShareRt>, id: &str, rc: &RunnerConfig, args: Vec<String>, cancel: tokio_util::sync::CancellationToken) {
    let out_dir = path::norm(&rc.output_dir).unwrap_or_else(|| "_команды".into());
    let log_rel = format!("{}/{}.log", out_dir, id);
    let Ok(log_full) = path::to_local(&rt.root, &log_rel) else { return };
    if let Some(d) = log_full.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    node.info(&format!("[{}] выполняю {}: {} {}", rt.id, id, rc.exe, args.iter().map(|a| if a.contains(' ') { format!("\"{}\"", a) } else { a.clone() }).collect::<Vec<_>>().join(" ")));
    let mut cmd = tokio::process::Command::new(&rc.exe);
    cmd.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null()).kill_on_drop(true);
    cmd.current_dir(match &rc.work_dir {
        Some(w) => node.resolve(w),
        None => rt.root.clone(),
    });
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut exit: Option<i32> = None;
    let message: String;
    match cmd.spawn() {
        Err(e) => message = format!("не удалось запустить: {}", e),
        Ok(mut child) => {
            let log = Arc::new(tokio::sync::Mutex::new(tokio::fs::File::create(&log_full).await.ok()));
            if let Some(f) = log.lock().await.as_mut() {
                let _ = f.write_all(format!("\u{feff}# {} · {} · {}\n", id, chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), rc.exe).as_bytes()).await;
            }
            let pipe = |r: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>, prefix: &'static str| {
                let log = log.clone();
                tokio::spawn(async move {
                    let Some(r) = r else { return };
                    let mut lines = BufReader::new(r).lines();
                    while let Ok(Some(l)) = lines.next_line().await {
                        if let Some(f) = log.lock().await.as_mut() {
                            let _ = f.write_all(format!("{}{}\n", prefix, l).as_bytes()).await;
                        }
                    }
                })
            };
            let t1 = pipe(child.stdout.take().map(|x| Box::new(x) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), "");
            let t2 = pipe(child.stderr.take().map(|x| Box::new(x) as Box<dyn tokio::io::AsyncRead + Unpin + Send>), "! ");
            let timeout = std::time::Duration::from_secs(rc.timeout_sec.max(1) as u64);
            tokio::select! {
                r = child.wait() => {
                    exit = r.ok().and_then(|s| s.code());
                    message = match exit { Some(0) => "готово".into(), Some(c) => format!("код выхода {}", c), None => "процесс прерван".into() };
                }
                _ = tokio::time::sleep(timeout) => {
                    let _ = child.kill().await;
                    message = format!("остановлено по таймауту ({} с)", rc.timeout_sec);
                }
                _ = cancel.cancelled() => {
                    let _ = child.kill().await;
                    message = "остановлено: узел завершает работу".into();
                }
            }
            let _ = t1.await;
            let _ = t2.await;
            {
                let mut g = log.lock().await;
                if let Some(f) = g.as_mut() {
                    let _ = f.write_all(format!("# {}\n", message).as_bytes()).await;
                }
            }
        }
    }
    if exit == Some(0) {
        let mut f = serde_json::Map::new();
        f.insert("result".into(), result(exit, &message, Some(&log_rel)));
        let r = shares::act(node, BridgeAction { action: "transition".into(), share: Some(rt.id.clone()), record: Some(id.into()), to: Some("done".into()), actor: node.node_name(), fields: Some(f), ..Default::default() });
        if !r.ok {
            node.info(&format!("[{}] не удалось отметить {} выполненной: {}", rt.id, id, r.error));
        }
    } else {
        fail(node, rt, id, exit, &message, Some(&log_rel));
    }
}

fn result(exit: Option<i32>, message: &str, log: Option<&str>) -> Value {
    let mut o = json!({ "message": message });
    if let Some(e) = exit {
        o["exitCode"] = json!(e);
    }
    if let Some(l) = log {
        o["log"] = json!(l);
    }
    o
}

fn fail(node: &Arc<Node>, rt: &Arc<ShareRt>, id: &str, exit: Option<i32>, message: &str, log: Option<&str>) {
    node.info(&format!("[{}] команда {}: {}", rt.id, id, message));
    let mut f = serde_json::Map::new();
    f.insert("result".into(), result(exit, message, log));
    let r = shares::act(
        node,
        BridgeAction { action: "transition".into(), share: Some(rt.id.clone()), record: Some(id.into()), to: Some("failed".into()), actor: node.node_name(), note: message.into(), fields: Some(f), ..Default::default() },
    );
    if !r.ok {
        node.info(&format!("[{}] не удалось отметить {} проваленной: {}", rt.id, id, r.error));
    }
}
