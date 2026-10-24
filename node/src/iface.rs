//! Файловый интерфейс узла: «<папка пространства>/_bridge/». Для программ, которые в сеть не ходят:
//! состояние — файлами JSON, действия — файлами в outbox/.

use crate::node::Node;
use crate::shares::{self, ShareRt};
use bridge_core::proto::{record_types, BridgeAction};
use bridge_core::reducer::roles;
use bridge_core::{time, to_json, write_atomic};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub const README: &str = "Папка _bridge — файловый интерфейс узла моста (Exerussus.Bridge). Пишет узел; не правьте файлы, кроме outbox/.

status.json                 — узел, соседи (онлайн, ошибки), последний обмен, конфликты файлов, команды на подтверждении
index.json                  — все записи пространства: id, тип, статус, заголовок, версия, исполнитель
records/<тип>/<id>.json     — запись целиком: поля, статус, версия, конфликты, комментарии, вложения, подтверждения, доставка
records/<тип>/<id>.history.json — все события записи по порядку (кто, когда, что)
inbox/                      — то, что ждёт действия ЭТОГО узла (по файлу на запись)
rejected.json               — события соседей, которые узел не принял, и почему
outbox/                     — сюда класть действия: файл *.json с одним действием или массивом действий.
                              Узел выполнит и перенесёт файл в outbox/done/ (+ .result.json) или outbox/failed/ (+ .error.txt).

Действие (JSON):
  {\"action\":\"create\",\"type\":\"task\",\"fields\":{\"title\":\"…\",\"assignee\":\"<узел>\",\"body\":\"…\"},\"actor\":\"кто\",\"note\":\"…\"}
  {\"action\":\"transition\",\"record\":\"<id>\",\"to\":\"accepted|in_progress|review|done|…\",\"note\":\"…\"}
  {\"action\":\"set\",\"record\":\"<id>\",\"fields\":{\"поле\":\"значение\"},\"expectVersion\":3}
  {\"action\":\"comment\",\"record\":\"<id>\",\"text\":\"…\",\"replyTo\":\"<eid>\"}
  {\"action\":\"ack\",\"record\":\"<id>\",\"level\":\"accepted|rejected|seen\",\"text\":\"…\"}
  {\"action\":\"attach\",\"record\":\"<id>\",\"path\":\"папка/файл.ext\",\"title\":\"…\"}
  {\"action\":\"link\",\"record\":\"<id>\",\"target\":\"<другой id>\",\"rel\":\"relates\"}
  {\"action\":\"create\",\"type\":\"dto\",\"schema\":\"имя@1\",\"fields\":{…}}         — проверяется по _schemas/имя@1.json
  {\"action\":\"create\",\"type\":\"command\",\"fields\":{\"assignee\":\"<узел>\",\"runner\":\"<имя>\",\"args\":{…},\"title\":\"…\"}}
  {\"action\":\"approve\",\"record\":\"<id>\"}  /  {\"action\":\"reject\",\"record\":\"<id>\",\"note\":\"причина\"}

Задача: new → accepted → in_progress → review → done. Принимает, начинает и сдаёт исполнитель; закрывает (done)
или возвращает на доработку (in_progress, с note) постановщик. Отказ — rejected (исполнитель), отмена — cancelled (постановщик).
";

fn safe_name(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect()
}

/// Переписать изменившиеся файлы интерфейса.
pub fn write(node: &Arc<Node>, rt: &Arc<ShareRt>) {
    rt.interface_dirty.store(false, Ordering::SeqCst);
    if let Err(e) = write_inner(node, rt) {
        node.info(&format!("[{}] файловый интерфейс: {}", rt.id, e));
        rt.interface_dirty.store(true, Ordering::SeqCst);
    }
}

fn write_inner(node: &Arc<Node>, rt: &Arc<ShareRt>) -> std::io::Result<()> {
    let dir = rt.root.join(bridge_core::SERVICE_DIR);
    std::fs::create_dir_all(dir.join("outbox"))?;
    let readme = dir.join("README.txt");
    if !readme.exists() {
        write_atomic(&readme, &README.replace('\n', "\r\n"))?;
    }
    let peers = rt.peers(node);
    let schemas = rt.schemas();
    let me = node.node_name();
    let (changed, all, rejected) = {
        let mut rb = rt.records.lock();
        let mut changed = rb.take_changed();
        if rt.cursors_changed.swap(false, Ordering::SeqCst) || !dir.join("records").exists() {
            changed = rb.all().iter().map(|s| s.id.clone()).collect();
        }
        let views: Vec<_> = changed.iter().filter_map(|id| rb.view(id, None, &peers, &schemas).map(|v| (v, shares::history_view(&rb, id)))).collect::<Vec<_>>();
        let all: Vec<_> = rb.all().into_iter().cloned().collect();
        (views, all, rb.store.rejected().to_vec())
    };
    for (view, hist) in &changed {
        let tdir = dir.join("records").join(safe_name(if view.type_.is_empty() { "_без-типа" } else { &view.type_ }));
        write_atomic(&tdir.join(format!("{}.json", view.id)), &to_json(view))?;
        write_atomic(&tdir.join(format!("{}.history.json", view.id)), &to_json(hist))?;
    }
    let mut sorted = all.clone();
    sorted.sort_by(|a, b| b.updated.cmp(&a.updated));
    let index: Vec<Value> = sorted.iter().map(|s| shares::summary(&rt.id, s)).collect();
    write_atomic(&dir.join("index.json"), &to_json(&index))?;
    // inbox
    let inbox = dir.join("inbox");
    std::fs::create_dir_all(&inbox)?;
    let mut want: Vec<(String, Value)> = Vec::new();
    for s in &all {
        let (creator, assignee) = roles(s, &me);
        let st = s.status().unwrap_or("");
        let mut why: Option<String> = match s.type_.as_str() {
            record_types::TASK if assignee && st == "new" => Some("новая задача: принять (accepted) или отказать (rejected)".into()),
            record_types::TASK if assignee && st == "accepted" => Some("принята: начать (in_progress)".into()),
            record_types::TASK if assignee && st == "in_progress" => Some("в работе: сдать (review)".into()),
            record_types::TASK if creator && st == "review" => Some("сдана: проверить — done или вернуть in_progress с пояснением".into()),
            record_types::COMMAND if assignee && st == "pending" && node.runner.is_waiting(&rt.id, &s.id) => Some("команда ждёт подтверждения человека".into()),
            record_types::DECISION if !creator && st == "proposed" => Some("решение предложено: accepted или rejected".into()),
            _ => None,
        };
        if why.is_none() && !s.conflicts.is_empty() {
            why = Some(format!("встречные правки полей: {}", s.conflicts.keys().cloned().collect::<Vec<_>>().join(", ")));
        }
        if let Some(w) = why {
            want.push((s.id.clone(), json!({ "why": w, "record": shares::summary(&rt.id, s) })));
        }
    }
    for (id, item) in &want {
        write_atomic(&inbox.join(format!("{}.json", id)), &to_json(item))?;
    }
    if let Ok(rd) = std::fs::read_dir(&inbox) {
        for e in rd.flatten() {
            let p = e.path();
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            if p.extension().map(|x| x == "json").unwrap_or(false) && !want.iter().any(|(id, _)| *id == stem) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    write_atomic(&dir.join("rejected.json"), &to_json(&rejected))?;
    let reports = rt.reports.lock().clone();
    let peer_list: Vec<Value> = peers
        .iter()
        .map(|p| {
            let ps = node.peer_state(p);
            json!({ "name": p, "online": ps.online, "lastSeen": ps.last_seen.map(|t| time::format(&t)), "error": ps.error, "status": ps.status, "report": reports.get(p) })
        })
        .collect();
    let status = json!({
        "node": node.status(),
        "share": rt.id,
        "lastRound": rt.last_round.lock().map(|t| time::format(&t)),
        "peers": peer_list,
        "files": rt.files.lock().manifest().files.len(),
        "records": all.len(),
        "conflicts": rt.conflicts.lock().clone(),
        "rejectedEvents": rejected.len(),
        "schemaErrors": rt.schemas.lock().errors.clone(),
        "waitingApproval": node.runner.waiting_for_share(&rt.id),
        "updated": time::format(&time::now()),
    });
    write_atomic(&dir.join("status.json"), &to_json(&status))?;
    Ok(())
}

/// Выполнить действия из outbox/ (файлы старше секунды — их уже дописали).
pub fn process_outbox(node: &Arc<Node>, rt: &Arc<ShareRt>) {
    let Some(_g) = rt.outbox_lock.try_lock() else { return };
    let outbox = rt.root.join(bridge_core::SERVICE_DIR).join("outbox");
    let Ok(rd) = std::fs::read_dir(&outbox) else { return };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().map(|x| x == "json").unwrap_or(false)).collect();
    if files.is_empty() {
        return;
    }
    files.sort();
    for file in files {
        let young = std::fs::metadata(&file).and_then(|m| m.modified()).map(|m| m.elapsed().map(|e| e.as_secs_f64() < 1.0).unwrap_or(true)).unwrap_or(true);
        if young {
            node.wake.notify_one();
            continue;
        }
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("x.json").to_string();
        let mut results = Vec::new();
        let mut error: Option<String> = None;
        match std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())) {
            Ok(v) => {
                let list: Vec<Value> = match v {
                    Value::Array(a) => a,
                    other => vec![other],
                };
                for item in list {
                    match serde_json::from_value::<BridgeAction>(item) {
                        Ok(mut a) => {
                            if a.share.is_none() {
                                a.share = Some(rt.id.clone());
                            }
                            let label = format!("{} {}", a.action, a.record.clone().or(a.type_.clone()).unwrap_or_default());
                            let r = shares::act(node, a);
                            let ok = r.ok;
                            let err = r.error.clone();
                            results.push(json!({ "ok": r.ok, "error": r.error, "record": r.record, "eid": r.eid, "version": r.version }));
                            if !ok {
                                error = Some(format!("{}: {}", label, err));
                                break;
                            }
                        }
                        Err(e) => {
                            error = Some(format!("не удалось прочитать действие: {}", e));
                            break;
                        }
                    }
                }
            }
            Err(e) => error = Some(format!("не удалось прочитать действие: {}", e)),
        }
        let target = outbox.join(if error.is_none() { "done" } else { "failed" });
        let _ = std::fs::create_dir_all(&target);
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-").to_string();
        let dest = target.join(format!("{}{}", stamp, name));
        if let Err(e) = bridge_core::files::move_replace(&file, &dest) {
            node.info(&format!("[{}] outbox {}: {}", rt.id, name, e));
            continue;
        }
        let _ = write_atomic(&target.join(format!("{}{}.result.json", stamp, name)), &to_json(&results));
        if let Some(e) = &error {
            let _ = write_atomic(&target.join(format!("{}{}.error.txt", stamp, name)), e);
        }
        node.info(&format!("[{}] outbox {}: {}", rt.id, name, error.as_deref().unwrap_or("выполнено")));
    }
}
