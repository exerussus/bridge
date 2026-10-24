//! Пространства: папка с правилами приёма + записи (задачи, решения, команды). Обмен с соседями.

use crate::client::PeerClient;
use crate::link::{self, steps_push, CheckStep};
use crate::node::Node;
use bridge_core::config::{PeerConfig, ShareConfig};
use bridge_core::files::{Decision, FileSyncState, FolderShare};
use bridge_core::proto::*;
use bridge_core::records::RecordBook;
use bridge_core::schemas::SchemaSet;
use bridge_core::time::{self, Time};
use bridge_core::{eq_ic, to_json};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ShareStateFile {
    files: FileSyncState,
    cursors: BTreeMap<String, BTreeMap<String, i64>>,
}

pub struct Schemas {
    pub set: SchemaSet,
    pub errors: Vec<String>,
    signature: String,
}

pub struct ShareRt {
    pub id: String,
    pub root: PathBuf,
    pub files: Mutex<FolderShare>,
    pub records: Mutex<RecordBook>,
    pub schemas: Mutex<Schemas>,
    state_file: PathBuf,
    pub gate: tokio::sync::Mutex<()>,
    pub last_round: Mutex<Option<Time>>,
    pub reports: Mutex<BTreeMap<String, String>>,
    pub conflicts: Mutex<Vec<String>>,
    pub wake: AtomicBool,
    pub interface_dirty: AtomicBool,
    pub cursors_changed: AtomicBool,
    pub files_dirty: AtomicBool,
    notified_seq: AtomicI64,
    pub outbox_lock: Mutex<()>,
    /// Сосед → отпечаток его файлов, с которым всё забрано: не изменился — список файлов не запрашивается.
    pub pulled_rev: Mutex<BTreeMap<String, String>>,
}

fn schema_signature(root: &std::path::Path) -> String {
    let dir = root.join(bridge_core::SCHEMAS_DIR);
    let Ok(rd) = std::fs::read_dir(dir) else { return String::new() };
    let mut items: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
        .map(|e| {
            let m = e.metadata().ok();
            format!(
                "{}|{}|{:?}",
                e.file_name().to_string_lossy(),
                m.as_ref().map(|m| m.len()).unwrap_or(0),
                m.and_then(|m| m.modified().ok())
            )
        })
        .collect();
    items.sort();
    items.join(";")
}

impl ShareRt {
    pub fn open(node: &Arc<Node>, cfg: &ShareConfig) -> ShareRt {
        let root = node.resolve(&cfg.folder);
        let _ = std::fs::create_dir_all(&root);
        let state_file = node.data_dir.join("state").join(format!("{}.json", cfg.id));
        let state: ShareStateFile = std::fs::read_to_string(&state_file)
            .ok()
            .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default();
        let c = node.cfg.read();
        let mut files = FolderShare::new(root.clone(), node.data_dir.join("history").join(&cfg.id), node.data_dir.join("tmp"), state.files, c.keep_history_days);
        files.settle_secs = node.settle_secs;
        let mut book = RecordBook::open(&c.node, &cfg.id, root.clone(), node.data_dir.join("events").join(&cfg.id));
        book.peer_cursors = state.cursors;
        let mut errors = Vec::new();
        let set = SchemaSet::load(&root, &mut errors);
        let ig = c.global_ignore();
        files.clean_history();
        files.rescan(&c.node, &cfg.id, &ig);
        let own_seq = book.store.vector().iter().find(|(k, _)| eq_ic(k, &c.node)).map(|(_, v)| *v).unwrap_or(0);
        ShareRt {
            id: cfg.id.clone(),
            root: root.clone(),
            files: Mutex::new(files),
            records: Mutex::new(book),
            schemas: Mutex::new(Schemas { set, errors, signature: schema_signature(&root) }),
            state_file,
            gate: tokio::sync::Mutex::new(()),
            last_round: Mutex::new(None),
            reports: Mutex::new(BTreeMap::new()),
            conflicts: Mutex::new(Vec::new()),
            wake: AtomicBool::new(false),
            interface_dirty: AtomicBool::new(true),
            cursors_changed: AtomicBool::new(true),
            files_dirty: AtomicBool::new(false),
            notified_seq: AtomicI64::new(own_seq),
            outbox_lock: Mutex::new(()),
            pulled_rev: Mutex::new(BTreeMap::new()),
        }
    }

    /// Схемы; перечитываются, если файлы в _schemas изменились.
    pub fn schemas(&self) -> SchemaSet {
        let mut g = self.schemas.lock();
        let sig = schema_signature(&self.root);
        if sig != g.signature {
            let mut errors = Vec::new();
            g.set = SchemaSet::load(&self.root, &mut errors);
            g.errors = errors;
            g.signature = sig;
        }
        g.set.clone()
    }

    pub fn config(&self, node: &Node) -> Option<ShareConfig> {
        node.cfg.read().share(&self.id).cloned()
    }

    /// Соседи пространства (группы раскрыты).
    pub fn peers(&self, node: &Node) -> Vec<String> {
        let cfg = node.cfg.read();
        cfg.share(&self.id).map(|s| cfg.share_peers(s)).unwrap_or_default()
    }

    pub fn participants(&self, node: &Node) -> Vec<String> {
        let cfg = node.cfg.read();
        let Some(s) = cfg.share(&self.id) else { return vec![cfg.node.clone()] };
        let mut v = cfg.share_peers(s);
        v.extend(s.members.iter().cloned());
        v.push(cfg.node.clone());
        v
    }

    pub fn save_state(&self, node: &Node) {
        let st = ShareStateFile { files: self.files.lock().state.clone(), cursors: self.records.lock().peer_cursors.clone() };
        if let Err(e) = bridge_core::write_atomic(&self.state_file, &serde_json::to_string(&st).unwrap_or_default()) {
            node.info(&format!("[{}] не удалось сохранить состояние: {}", self.id, e));
        }
    }

    fn rescan(&self, node: &Node) -> bool {
        let ig = node.cfg.read().global_ignore();
        let name = node.node_name();
        let mut f = self.files.lock();
        let changed = f.rescan(&name, &self.id, &ig);
        // файлы ещё пишутся — пересмотреть позже
        self.files_dirty.store(f.unsettled, Ordering::SeqCst);
        changed
    }
}

/// Действие от имени узла (локальный API, outbox, CLI) — и сразу оповестить соседей.
pub fn act(node: &Arc<Node>, a: BridgeAction) -> ActionResult {
    let rt = match resolve_share(node, a.share.as_deref(), a.record.as_deref()) {
        Ok(rt) => rt,
        Err(e) => return ActionResult::err(e),
    };
    let peers = rt.peers(node);
    let schemas = rt.schemas();
    let res = rt.records.lock().act(a, &peers, &schemas);
    if res.ok {
        rt.interface_dirty.store(true, Ordering::SeqCst);
        rt.save_state(node);
        notify_if_new_events(node, &rt);
        node.runner.check(node, &rt);
        node.emit("record", serde_json::json!({ "share": rt.id, "record": res.record, "eid": res.eid }));
    }
    res
}

/// Пространство действия: явно, по id записи или единственное.
pub fn resolve_share(node: &Node, share: Option<&str>, record: Option<&str>) -> Result<Arc<ShareRt>, String> {
    if let Some(s) = share.map(str::trim).filter(|s| !s.is_empty()) {
        return node.share(s).ok_or_else(|| format!("нет пространства «{}»", s));
    }
    let all = node.shares();
    if let Some(r) = record.map(str::trim).filter(|r| !r.is_empty()) {
        let found: Vec<_> = all.iter().filter(|s| s.records.lock().get(r).is_some()).cloned().collect();
        if found.len() == 1 {
            return Ok(found[0].clone());
        }
    }
    if all.len() == 1 {
        return Ok(all[0].clone());
    }
    Err("укажите share: пространств несколько".into())
}

pub fn notify_if_new_events(node: &Arc<Node>, rt: &Arc<ShareRt>) {
    let me = node.node_name();
    let mine = rt.records.lock().store.vector().iter().find(|(k, _)| eq_ic(k, &me)).map(|(_, v)| *v).unwrap_or(0);
    if rt.notified_seq.swap(mine, Ordering::SeqCst) != mine {
        node.notify_peers(rt.peers(node), &rt.id, "events");
    }
}

/// Пересканировать папку и, если что-то изменилось, позвать соседей.
pub async fn local_check(node: &Arc<Node>, rt: &Arc<ShareRt>) {
    let Ok(_g) = rt.gate.try_lock() else { return };
    if rt.rescan(node) {
        rt.interface_dirty.store(true, Ordering::SeqCst);
        rt.save_state(node);
        node.notify_peers(rt.peers(node), &rt.id, "files");
    }
    notify_if_new_events(node, rt);
}

/// Полный обмен пространства со всеми соседями.
pub async fn round(node: &Arc<Node>, rt: &Arc<ShareRt>) {
    let _g = rt.gate.lock().await;
    rt.wake.store(false, Ordering::SeqCst);
    let mut local_changed = rt.rescan(node);
    for name in rt.peers(node) {
        let Some(peer) = node.cfg.read().peer(&name).cloned() else { continue };
        if !peer.enabled {
            continue;
        }
        if peer.url.is_empty() {
            rt.reports.lock().insert(peer.name.clone(), link::WAITING_INVITE.into());
            continue;
        }
        let Some(hello) = link::contact(node, &peer).await else {
            let e = node.peer_state(&peer.name).error;
            rt.reports.lock().insert(peer.name.clone(), if e.is_empty() { "нет связи".into() } else { e });
            continue;
        };
        // мог назваться при этом привете
        let peer = node.cfg.read().peer_by_key(&peer.key_id()).cloned().unwrap_or(peer);
        let name = peer.name.clone();
        if !hello.shares.iter().any(|s| eq_ic(s, &rt.id)) {
            rt.reports.lock().insert(
                name.clone(),
                format!(
                    "«{}» не раздаёт пространство «{}» этому ПК{} — на том ПК добавьте этот ПК в участники «{}»",
                    name,
                    rt.id,
                    if hello.shares.is_empty() { String::new() } else { format!(" (раздаёт: {})", hello.shares.join(", ")) },
                    rt.id
                ),
            );
            continue;
        }
        let client = node.client_for(&peer);
        let rev = hello.revs.iter().find(|(k, _)| eq_ic(k, &rt.id)).map(|(_, v)| v.clone());
        match exchange(node, rt, &peer, &client, rev).await {
            Ok((applied, rejected, downloaded, conflicts, errors, changed)) => {
                if changed {
                    local_changed = true;
                }
                if applied + rejected > 0 || downloaded > 0 || conflicts > 0 {
                    node.info(&format!(
                        "[{}] от {}: событий {}{}, файлов {}{}",
                        rt.id,
                        name,
                        applied,
                        if rejected > 0 { format!(" (отклонено {})", rejected) } else { String::new() },
                        downloaded,
                        if conflicts > 0 { format!(", конфликтов {}", conflicts) } else { String::new() }
                    ));
                }
                node.with_peer_state(&name, |s| s.files_in += downloaded as i64);
                rt.reports.lock().insert(
                    name.clone(),
                    format!("ок: событий +{}, файлов +{}{}", applied, downloaded, if errors > 0 { format!(", ошибок {}", errors) } else { String::new() }),
                );
            }
            Err(e) => {
                let (err, _) = link::explain(&e, &peer);
                rt.reports.lock().insert(name.clone(), format!("ошибка обмена: {}", err));
                node.info_once(&format!("round|{}|{}|{}", rt.id, name, err), &format!("[{}] {}: ошибка обмена — {}", rt.id, name, err), 30);
            }
        }
    }
    *rt.last_round.lock() = Some(time::now());
    rt.interface_dirty.store(true, Ordering::SeqCst);
    rt.save_state(node);
    if local_changed {
        node.notify_peers(rt.peers(node), &rt.id, "files");
    }
    notify_if_new_events(node, rt);
    drop(_g);
    node.runner.check(node, rt);
}

type Exchange = (usize, usize, usize, usize, usize, bool);

async fn exchange(node: &Arc<Node>, rt: &Arc<ShareRt>, peer: &PeerConfig, client: &Arc<PeerClient>, rev: Option<String>) -> Result<Exchange, crate::client::PeerError> {
    let cfg = rt.config(node).unwrap_or_default();
    let participants = rt.participants(node);
    let (mut applied, mut rejected) = (0, 0);
    for _ in 0..1000 {
        let vector = rt.records.lock().store.vector();
        let page = client.events(&rt.id, &vector, 500).await?;
        let empty = page.events.is_empty();
        let (a, r) = rt.records.lock().ingest(&peer.name, page.events, &participants, &cfg);
        applied += a;
        rejected += r;
        // Пустая страница — конец; лишний запрос после непустой сообщает соседу новый курсор (квитанция «доставлено»)
        if empty {
            break;
        }
    }
    if applied > 0 {
        node.emit("records-in", serde_json::json!({ "share": rt.id, "peer": peer.name, "count": applied }));
    }
    // Файлы: у соседа ничего не менялось с прошлого полного забора — список не нужен
    if let Some(r) = &rev {
        if rt.pulled_rev.lock().get(&peer.name) == Some(r) {
            return Ok((applied, rejected, 0, 0, 0, false));
        }
    }
    let remote = client.manifest(&rt.id).await?;
    let mut deferred = 0;
    let ig = node.cfg.read().global_ignore();
    let max = node.max_file_bytes();
    let (mut downloaded, mut conflicts, mut errors) = (0, 0, 0);
    let node_name = node.node_name();
    for r in &remote.files {
        let (decision, p) = rt.files.lock().decide(r, &peer.name, &cfg, &ig);
        let Some(p) = p else { continue };
        match decision {
            Decision::Skip | Decision::Nothing => {}
            Decision::Busy => deferred += 1,
            Decision::Same => rt.files.lock().mark(&peer.name, &p, &r.sha256, true),
            Decision::Seen => rt.files.lock().mark(&peer.name, &p, &r.sha256, false),
            Decision::Download { local_changed } => {
                let tmp = node.tmp_file();
                match client.download(&rt.id, &p, &tmp, max).await {
                    Ok(got) if got == r.sha256 => {
                        let res = rt.files.lock().install(&node_name, &peer.name, &p, &tmp, r, local_changed);
                        match res {
                            Ok(conf) => {
                                downloaded += 1;
                                node.with_peer_state(&peer.name, |s| s.bytes_in += r.size);
                                if let Some(c) = conf {
                                    conflicts += 1;
                                    let mut cl = rt.conflicts.lock();
                                    cl.push(format!("{} {}", chrono::Local::now().format("%d.%m %H:%M"), c));
                                    let n = cl.len();
                                    if n > 50 {
                                        cl.drain(0..n - 50);
                                    }
                                    node.info(&format!("[{}] конфликт: {}", rt.id, c));
                                }
                                node.emit("file-in", serde_json::json!({ "share": rt.id, "peer": peer.name, "path": p }));
                            }
                            Err(e) => {
                                errors += 1;
                                let _ = std::fs::remove_file(&tmp);
                                node.info(&format!("[{}] {}: {}: {}", rt.id, peer.name, p, e));
                            }
                        }
                    }
                    Ok(_) => {
                        errors += 1;
                        let _ = std::fs::remove_file(&tmp);
                        node.info(&format!("[{}] {}: {}: версия у соседа сменилась во время скачивания", rt.id, peer.name, p));
                    }
                    Err(e) => {
                        errors += 1;
                        let _ = std::fs::remove_file(&tmp);
                        node.info_once(&format!("dl|{}|{}", rt.id, p), &format!("[{}] {}: {}: {}", rt.id, peer.name, p, e), 10);
                    }
                }
            }
        }
    }
    if downloaded > 0 || conflicts > 0 {
        rt.schemas();
    }
    // Всё сошлось — запомнить отпечаток соседа (его манифест мог смениться, пока качали: тогда rev не совпадёт и в следующий раз)
    match (&rev, errors == 0 && deferred == 0) {
        (Some(r), true) if bridge_core::proto::files_rev(&remote.files) == *r => {
            rt.pulled_rev.lock().insert(peer.name.clone(), r.clone());
        }
        _ => {
            rt.pulled_rev.lock().remove(&peer.name);
        }
    }
    Ok((applied, rejected, downloaded, conflicts, errors, downloaded > 0 || conflicts > 0))
}

/// Проверка пространства для «🩺 Проверить».
pub async fn check_share(node: &Arc<Node>, steps: &mut Vec<CheckStep>, peer: &PeerConfig, rt: &Arc<ShareRt>, hello: &HelloResponse) {
    let title = format!("Пространство «{}»", rt.id);
    if !hello.shares.iter().any(|s| eq_ic(s, &rt.id)) {
        steps_push(
            steps,
            &title,
            "bad",
            format!(
                "«{}» не раздаёт его этому ПК{}",
                peer.name,
                if hello.shares.is_empty() { String::new() } else { format!(" (раздаёт: {})", hello.shares.join(", ")) }
            ),
            format!("На «{}»: пространство с тем же id «{}», и этот ПК в его участниках", peer.name, rt.id),
        );
        return;
    }
    let cfg = rt.config(node).unwrap_or_default();
    let rules = cfg.accept.iter().filter(|a| a.from.iter().any(|f| f == "*" || eq_ic(f, &peer.name))).count();
    if rules == 0 {
        steps_push(
            steps,
            &title,
            "bad",
            format!("правила приёма не пропускают от «{}» ни одного файла — будут приходить только записи (задачи, события)", peer.name),
            format!("Добавьте правило accept с from: [\"{}\"] (например, pattern «**»)", peer.name),
        );
    }
    let client = node.client_for(peer);
    match tokio::time::timeout(std::time::Duration::from_secs(20), client.manifest(&rt.id)).await {
        Ok(Ok(manifest)) => {
            if let Ok(_g) = tokio::time::timeout(std::time::Duration::from_secs(5), rt.gate.lock()).await {
                if rt.rescan(node) {
                    rt.interface_dirty.store(true, Ordering::SeqCst);
                }
            }
            let ig = node.cfg.read().global_ignore();
            let (mut accepted, mut refused, mut ignored, mut behind) = (0, 0, 0, 0);
            let (mut rs, mut bs) = (Vec::new(), Vec::new());
            {
                let f = rt.files.lock();
                for e in &manifest.files {
                    if ig.is_ignored(&e.path) {
                        ignored += 1;
                        continue;
                    }
                    if !cfg.accepts_file(&e.path, &peer.name) {
                        refused += 1;
                        if rs.len() < 3 {
                            rs.push(e.path.clone());
                        }
                        continue;
                    }
                    accepted += 1;
                    if f.find(&e.path).map(|l| l.sha256 != e.sha256).unwrap_or(true) {
                        behind += 1;
                        if bs.len() < 3 {
                            bs.push(e.path.clone());
                        }
                    }
                }
            }
            let text = format!(
                "у соседа файлов {}: принимаются {}{}{}",
                manifest.files.len(),
                accepted,
                if refused > 0 { format!(", не проходят правила приёма {} (например: {})", refused, rs.join(", ")) } else { String::new() },
                if ignored > 0 { format!(", в игноре {}", ignored) } else { String::new() }
            );
            if behind > 0 {
                steps_push(
                    steps,
                    &title,
                    "warn",
                    format!("{}. Ещё не совпадают с вашими: {} ({}{})", text, behind, bs.join(", "), if behind > 3 { "…" } else { "" }),
                    "Обычно уйдёт за ближайший обмен (кнопка «⟳ Обменяться»). Если не уходит — смотрите отчёт пространства и журнал: файл занят, больше предела, конфликт",
                );
            } else if rules > 0 {
                steps_push(steps, &title, if refused > 0 { "info" } else { "ok" }, format!("{}{}", text, if accepted > 0 { " — все уже здесь" } else { "" }), "");
            }
            if let Some(rep) = rt.reports.lock().get(&peer.name).filter(|r| !r.starts_with("ок")) {
                steps_push(steps, &title, "bad", format!("последний обмен: {}", rep), "");
            }
        }
        Ok(Err(e)) => steps_push(steps, &title, "bad", format!("не удалось получить список файлов: {}", link::explain(&e, peer).0), ""),
        Err(_) => steps_push(steps, &title, "bad", format!("не удалось получить список файлов: {}", link::explain_timeout(peer).0), ""),
    }
    let rejected: Vec<String> = rt.records.lock().store.rejected().iter().filter(|r| eq_ic(&r.from, &peer.name)).map(|r| r.reason.clone()).collect();
    if let Some(last) = rejected.last() {
        steps_push(
            steps,
            &title,
            "warn",
            format!("отклонено записей от соседа: {} — например: {}", rejected.len(), last),
            "Автор записи должен быть участником пространства (peers или members) и проходить records.accept",
        );
    }
}

/// Сводка записи (index.json, списки панели).
pub fn summary(rt_id: &str, s: &RecordState) -> serde_json::Value {
    serde_json::json!({
        "id": s.id,
        "share": rt_id,
        "type": s.type_,
        "schema": s.schema,
        "status": s.status(),
        "title": s.title().map(String::from).or_else(|| s.fields.get("name").map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string()))),
        "assignee": s.assignee(),
        "creator": s.creator,
        "version": s.version,
        "updated": time::format(&s.updated),
        "conflict": if s.conflicts.is_empty() { serde_json::Value::Null } else { serde_json::json!(s.conflicts.keys().collect::<Vec<_>>()) },
        "comments": s.comments.iter().filter(|c| !c.reverted).count(),
    })
}

pub fn history_view(rb: &RecordBook, id: &str) -> serde_json::Value {
    let invalid: BTreeMap<String, String> = rb.get(id).map(|s| s.invalid.iter().map(|i| (i.eid.clone(), i.reason.clone())).collect()).unwrap_or_default();
    let list: Vec<serde_json::Value> = rb
        .history(id)
        .iter()
        .enumerate()
        .map(|(i, e)| {
            serde_json::json!({
                "version": i + 1, "eid": e.eid, "op": e.op, "author": e.author, "actor": e.actor, "time": time::format(&e.time),
                "schema": e.schema, "payload": e.payload, "note": e.note, "invalid": invalid.get(&e.eid),
            })
        })
        .collect();
    serde_json::Value::Array(list)
}

pub fn json_pretty<T: Serialize>(v: &T) -> String {
    to_json(v)
}
