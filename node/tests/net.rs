//! Узлы в одном процессе: файлы, записи, команды, отправка, общие папки, соседи по адресу и ключу, проверка связи.

mod common;

use bridge_core::config::{NodeConfig, RecordAcceptRule, RunnerConfig, SharedFolderConfig};
use bridge_core::proto::{BridgeAction, BridgeEvent};
use bridge_node::link;
use common::*;
use serde_json::{json, Value};
use std::time::Duration;

fn ba(v: Value) -> BridgeAction {
    serde_json::from_value(v).unwrap()
}

fn status(net: &Net, node: &str, id: &str) -> Option<String> {
    net.n(node).share("s").unwrap().records.lock().get(id).and_then(|s| s.status().map(String::from))
}

// ───────────────────────── Файлы ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_follow_accept_rules() {
    let mut net = pair();
    net.start().await;
    net.write("dev", "задания/Z-001.md", "Задание 1");
    net.write("stand", "отчёты/Z-001/итог.md", "Готово");
    net.write("stand", "задания/подмена.md", "стенд пишет в чужую папку");
    net.write("dev", "личное/заметки.md", "не для соседа");
    net.sync(2, &[]).await;
    assert_eq!(net.read("stand", "задания/Z-001.md").as_deref(), Some("Задание 1"), "задание доехало");
    assert_eq!(net.read("dev", "отчёты/Z-001/итог.md").as_deref(), Some("Готово"), "отчёт доехал");
    assert_eq!(net.read("dev", "задания/подмена.md"), None, "чужой путь не принят");
    assert_eq!(net.read("stand", "личное/заметки.md"), None, "без правила — не принято");
    let rt = net.n("stand").share("s").unwrap();
    assert!(rt.files.lock().manifest().files.iter().all(|f| !f.path.starts_with("_bridge")), "служебная папка не в манифесте");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_conflict_keeps_both_versions() {
    let mut net = pair();
    net.start().await;
    net.write("dev", "состояние.md", "v1");
    net.sync(2, &[]).await;
    assert_eq!(net.read("stand", "состояние.md").as_deref(), Some("v1"));
    net.write("dev", "состояние.md", "v2-разработка");
    net.write("stand", "состояние.md", "v2-стенд");
    net.sync(3, &[]).await;
    let (a, b) = (net.read("dev", "состояние.md").unwrap(), net.read("stand", "состояние.md").unwrap());
    assert_eq!(a, b, "после конфликта основной файл одинаков");
    let mut copies = Vec::new();
    for n in ["dev", "stand"] {
        for e in std::fs::read_dir(net.folder(n)).unwrap().flatten() {
            if e.file_name().to_string_lossy().starts_with("состояние.конфликт-") {
                copies.push(e.path());
            }
        }
    }
    // Правки одновременно и обмен мгновенный: копию своей версии может сохранить каждая сторона (1–2 копии)
    assert!((1..=2).contains(&copies.len()), "копии конфликта: {}", copies.len());
    let mut all = vec![a];
    for c in &copies {
        all.push(std::fs::read_to_string(c).unwrap());
    }
    assert!(all.contains(&"v2-разработка".to_string()) && all.contains(&"v2-стенд".to_string()), "обе версии сохранены: {:?}", all);
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_local_delete_is_not_propagated() {
    let mut net = pair();
    net.start().await;
    net.write("dev", "задания/Z-002.md", "a");
    net.sync(2, &[]).await;
    std::fs::remove_file(net.folder("stand").join("задания/Z-002.md")).unwrap();
    net.sync(2, &[]).await;
    assert_eq!(net.read("dev", "задания/Z-002.md").as_deref(), Some("a"));
    assert_eq!(net.read("stand", "задания/Z-002.md"), None, "удалённый локально не вернулся сам");
    tokio::time::sleep(Duration::from_millis(20)).await;
    net.write("dev", "задания/Z-002.md", "b");
    net.sync(2, &[]).await;
    assert_eq!(net.read("stand", "задания/Z-002.md").as_deref(), Some("b"), "новая версия приезжает снова");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_chain_and_big_file() {
    let mut net = Net::new();
    net.add("a", &["b"], &[("**", "b")]);
    net.add("b", &["a", "c"], &[("**", "*")]);
    net.add("c", &["b"], &[("**", "b")]);
    net.start().await;
    let big: Vec<u8> = (0..5 * 1024 * 1024 + 123).map(|i| (i * 7 % 251) as u8).collect();
    let full = net.folder("a").join("данные/большой.bin");
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(&full, &big).unwrap();
    net.sync(2, &["a", "b", "c"]).await;
    let at_c = net.folder("c").join("данные/большой.bin");
    assert_eq!(std::fs::read(&at_c).unwrap(), big, "файл прошёл цепочку a → b → c целиком");
    net.stop().await;
}

// ───────────────────────── Записи ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_full_lifecycle_and_roles() {
    let mut net = pair();
    net.start().await;
    let r = net.act("dev", ba(json!({"action":"create","type":"task","actor":"владелец","fields":{"title":"Прогнать стенды 00–02","assignee":"stand"}})));
    assert!(r.ok, "{}", r.error);
    let id = r.record.clone().unwrap();
    assert_eq!(r.state.unwrap().status.as_deref(), Some("new"));
    assert!(!net.act("dev", ba(json!({"action":"transition","record":id,"to":"accepted"}))).ok, "постановщик не принимает за исполнителя");
    net.sync(1, &[]).await;
    assert!(net.n("stand").share("s").unwrap().records.lock().get(&id).is_some(), "задача доехала");
    for to in ["accepted", "in_progress", "review"] {
        let r = net.act("stand", ba(json!({"action":"transition","record":id,"to":to,"actor":"agent:claude"})));
        assert!(r.ok, "→ {}: {}", to, r.error);
    }
    assert!(!net.act("stand", ba(json!({"action":"transition","record":id,"to":"done"}))).ok, "исполнитель не закрывает сам");
    net.sync(1, &[]).await;
    assert_eq!(status(&net, "dev", &id).as_deref(), Some("review"));
    assert!(!net.act("dev", ba(json!({"action":"transition","record":id,"to":"in_progress"}))).ok, "возврат без пояснения запрещён");
    assert!(net.act("dev", ba(json!({"action":"transition","record":id,"to":"in_progress","note":"нет снимков 02"}))).ok);
    net.sync(1, &[]).await;
    assert!(net.act("stand", ba(json!({"action":"transition","record":id,"to":"review"}))).ok);
    assert!(net.act("stand", ba(json!({"action":"comment","record":id,"text":"Снимки добавил"}))).ok);
    net.sync(1, &[]).await;
    assert!(net.act("dev", ba(json!({"action":"transition","record":id,"to":"done"}))).ok);
    net.sync(1, &[]).await;
    let a = net.n("dev").share("s").unwrap().records.lock().get(&id).cloned().unwrap();
    let b = net.n("stand").share("s").unwrap().records.lock().get(&id).cloned().unwrap();
    assert_eq!(b.status(), Some("done"));
    assert_eq!(a.version, b.version);
    assert_eq!(serde_json::to_string(&a.fields).unwrap(), serde_json::to_string(&b.fields).unwrap());
    assert_eq!(b.comments.len(), 1);
    let rt = net.n("dev").share("s").unwrap();
    let v1 = rt.records.lock().view(&id, Some(1), &[], &rt.schemas()).unwrap();
    assert_eq!(v1.status(), Some("new"), "состояние на версию 1");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_concurrent_set_is_conflict_then_resolved() {
    let mut net = pair();
    net.start().await;
    let id = net.act("dev", ba(json!({"action":"create","type":"note","fields":{"text":"a"}}))).record.unwrap();
    net.sync(1, &[]).await;
    assert!(net.act("dev", ba(json!({"action":"set","record":id,"fields":{"text":"от разработки"}}))).ok);
    assert!(net.act("stand", ba(json!({"action":"set","record":id,"fields":{"text":"от стенда"}}))).ok);
    net.sync(1, &[]).await;
    let get = |n: &str| net.n(n).share("s").unwrap().records.lock().get(&id).cloned().unwrap();
    let (a, b) = (get("dev"), get("stand"));
    assert!(a.conflicts.contains_key("text") && b.conflicts.contains_key("text"), "встречные правки видны обоим");
    assert_eq!(a.fields["text"], b.fields["text"]);
    assert_eq!(a.conflicts["text"].len(), 2);
    assert!(net.act("stand", ba(json!({"action":"set","record":id,"fields":{"text":"сошлись"}}))).ok);
    net.sync(1, &[]).await;
    assert!(!get("dev").conflicts.contains_key("text"), "конфликт снят");
    let v = get("dev").version;
    assert!(!net.act("dev", ba(json!({"action":"set","record":id,"fields":{"x":1},"expectVersion":v-1}))).ok);
    assert!(net.act("dev", ba(json!({"action":"set","record":id,"fields":{"x":1},"expectVersion":v}))).ok);
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_delivery_and_acks() {
    let mut net = pair();
    net.start().await;
    let id = net.act("dev", ba(json!({"action":"create","type":"decision","fields":{"text":"Иерархия"}}))).record.unwrap();
    net.sync(1, &["stand"]).await;
    let rt = net.n("dev").share("s").unwrap();
    let v = rt.records.lock().view(&id, None, &["stand".to_string()], &rt.schemas()).unwrap();
    assert_eq!(v.delivered.unwrap()["stand"], true, "сосед забрал — доставлено");
    assert!(!net.act("dev", ba(json!({"action":"transition","record":id,"to":"accepted"}))).ok);
    assert!(net.act("stand", ba(json!({"action":"transition","record":id,"to":"accepted","note":"уверен"}))).ok);
    assert!(net.act("stand", ba(json!({"action":"ack","record":id,"level":"seen"}))).ok);
    net.sync(1, &[]).await;
    let s = rt.records.lock().get(&id).cloned().unwrap();
    assert_eq!(s.status(), Some("accepted"));
    assert_eq!(s.acks.len(), 1);
    assert_eq!(s.acks[0].author, "stand");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_accept_rules_and_forgery() {
    let mut net = pair();
    net.configs.get_mut("stand").unwrap().shares[0].records.accept.push(RecordAcceptRule { from: vec!["dev".into()], types: vec!["task".into()], ops: vec!["*".into()] });
    net.start().await;
    let note = net.act("dev", ba(json!({"action":"create","type":"note","fields":{"a":1}}))).record.unwrap();
    let task = net.act("dev", ba(json!({"action":"create","type":"task","fields":{"title":"t","assignee":"stand"}}))).record.unwrap();
    net.sync(1, &[]).await;
    let rt = net.n("stand").share("s").unwrap();
    let cfg = rt.config(net.n("stand")).unwrap();
    {
        let rb = rt.records.lock();
        assert!(rb.get(&task).is_some(), "задача принята");
        assert!(rb.get(&note).is_none(), "тип вне правил — не принят");
        assert!(rb.store.rejected().iter().any(|r| r.event.record == note && r.reason.contains("правила приёма")));
    }
    let fake = BridgeEvent { share: "s".into(), record: "x-1".into(), type_: "task".into(), op: "create".into(), author: "stand".into(), seq: 999, eid: "stand:999".into(), payload: Some(Default::default()), ..Default::default() };
    let (ok, bad) = rt.records.lock().ingest("dev", vec![fake], &["dev".into(), "stand".into()], &cfg);
    assert_eq!((ok, bad), (0, 1), "подделка от имени этого узла отклонена");
    let stranger = BridgeEvent { share: "s".into(), record: "x-2".into(), type_: "task".into(), op: "create".into(), author: "чужой".into(), seq: 1, eid: "чужой:1".into(), ..Default::default() };
    assert_eq!(rt.records.lock().ingest("dev", vec![stranger], &["dev".into(), "stand".into()], &cfg).1, 1, "не участник — отклонён");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dto_schema_and_versions() {
    let mut net = pair();
    net.start().await;
    net.write("dev", "_schemas/stand-report@1.json", r#"{ "type": "object", "required": ["stand", "ok"], "additionalProperties": false,
      "properties": { "stand": { "type": "string", "enum": ["00", "01", "02"] }, "ok": { "type": "integer", "minimum": 0 }, "fail": { "type": "integer" } } }"#);
    net.write("dev", "_schemas/stand-report@2.json", r#"{ "type": "object", "required": ["stand", "ok", "bazis"], "properties": { "stand": { "type": "string" }, "ok": { "type": "integer" }, "bazis": { "type": "string" } } }"#);
    net.sync(1, &[]).await;
    let good = net.act("stand", ba(json!({"action":"create","type":"dto","schema":"stand-report@1","fields":{"stand":"01","ok":9}})));
    assert!(good.ok, "{}", good.error);
    let bad = net.act("stand", ba(json!({"action":"create","type":"dto","schema":"stand-report@1","fields":{"stand":"07","ok":-1,"лишнее":1}})));
    assert!(!bad.ok && bad.error.contains("не из списка"), "{}", bad.error);
    assert!(!net.act("stand", ba(json!({"action":"create","type":"dto","fields":{}}))).ok, "dto без схемы — отказ");
    let id = good.record.unwrap();
    assert!(!net.act("stand", ba(json!({"action":"set","record":id,"fields":{"ok":"много"}}))).ok);
    let up = net.act("stand", ba(json!({"action":"set","record":id,"schema":"stand-report@2","fields":{"bazis":"2026.9.4"}})));
    assert!(up.ok, "{}", up.error);
    net.sync(1, &[]).await;
    let rt = net.n("dev").share("s").unwrap();
    let v = rt.records.lock().view(&id, None, &[], &rt.schemas()).unwrap();
    assert_eq!(v.schema.as_deref(), Some("stand-report@2"));
    assert!(v.schema_errors.is_none(), "{:?}", v.schema_errors);
    net.stop().await;
}

// ───────────────────────── Команды ─────────────────────────

fn command_net(confirm: bool, allow: &str) -> Net {
    let mut net = pair();
    let st = net.configs.get_mut("stand").unwrap();
    let exe = if cfg!(windows) { "cmd" } else { "sh" };
    let args: Vec<String> = if cfg!(windows) { vec!["/c".into(), "echo ok.".into()] } else { vec!["-c".into(), "echo ok.".into()] };
    st.runners.insert("ver".into(), RunnerConfig { enabled: true, share: "s".into(), exe: exe.into(), args, allow_from: vec![allow.into()], confirm, timeout_sec: 60, ..Default::default() });
    let mut params = indexmap::IndexMap::new();
    params.insert("file".to_string(), "path".to_string());
    st.runners.insert("open".into(), RunnerConfig { enabled: true, share: "s".into(), exe: exe.into(), args: vec!["{file}".into()], params, allow_from: vec!["dev".into()], confirm: true, ..Default::default() });
    st.runners.insert("off".into(), RunnerConfig { enabled: false, share: "s".into(), exe: exe.into(), allow_from: vec!["dev".into()], ..Default::default() });
    net
}

async fn wait_status(net: &Net, node: &str, id: &str, want: &[&str]) -> String {
    for _ in 0..100 {
        net.sync(1, &[]).await;
        if let Some(s) = status(net, node, id) {
            if want.contains(&s.as_str()) {
                return s;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("не дождались {:?} у {}: {:?}\n{}", want, id, status(net, node, id), net.logs.lock().join("\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn command_waits_for_human_then_runs() {
    let mut net = command_net(true, "dev");
    net.start().await;
    let id = net.act("dev", ba(json!({"action":"create","type":"command","fields":{"assignee":"stand","runner":"ver","title":"версия"}}))).record.unwrap();
    net.sync(1, &[]).await;
    assert_eq!(status(&net, "stand", &id).as_deref(), Some("pending"), "ждёт человека");
    assert_eq!(net.n("stand").runner.waiting_count(), 1);
    bridge_node::iface::write(net.n("stand"), &net.n("stand").share("s").unwrap());
    assert!(net.folder("stand").join("_bridge/inbox").join(format!("{}.json", id)).exists(), "команда во входящих");
    assert!(!net.act("dev", ba(json!({"action":"approve","record":id}))).ok, "отправитель сам не подтверждает");
    assert!(net.act("stand", ba(json!({"action":"approve","record":id,"actor":"технолог"}))).ok);
    assert_eq!(wait_status(&net, "dev", &id, &["done", "failed"]).await, "done");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn command_rejected_when_not_allowed() {
    let mut net = command_net(false, "кто-то-другой");
    net.start().await;
    let mk = |f: Value| net.act("dev", ba(json!({"action":"create","type":"command","fields":f}))).record.unwrap();
    let a = mk(json!({"assignee":"stand","runner":"ver"}));
    let b = mk(json!({"assignee":"stand","runner":"нет-такого"}));
    let c = mk(json!({"assignee":"stand","runner":"off"}));
    let d = mk(json!({"assignee":"stand","runner":"open","args":{"file":"../../etc/passwd"}}));
    let e = mk(json!({"assignee":"stand","runner":"open","args":{"file":"нет.txt","лишний":"1"}}));
    for id in [&a, &b, &c, &d, &e] {
        assert_eq!(wait_status(&net, "dev", id, &["rejected", "done", "failed"]).await, "rejected");
    }
    let rt = net.n("dev").share("s").unwrap();
    assert!(rt.records.lock().history(&a).last().unwrap().note.contains("не разрешено"));
    assert!(rt.records.lock().history(&d).last().unwrap().note.contains("выход за папку"));
    net.stop().await;
}

// ───────────────────────── Безопасность ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn security_whitelist_and_key() {
    let mut net = pair();
    net.configs.get_mut("stand").unwrap().peers[0].ips = vec!["10.1.2.3".into()];
    net.start().await;
    net.write("stand", "отчёты/x.md", "x");
    let peer = net.n("dev").cfg.read().peer("stand").cloned().unwrap();
    let e = net.n("dev").client_for(&peer).manifest("s").await.unwrap_err();
    assert_eq!(e.status(), Some(403), "запрос не с адреса соседа — отказ");
    assert!(e.to_string().contains("не записан") && e.to_string().contains("Добавить адрес"), "{}", e);
    assert!(net.log_has("stand:", "белом списке"));
    net.n("stand").cfg.write().peers[0].ips = vec!["127.0.0.1".into()];
    net.n("dev").cfg.write().peers[0].secret = "BRS-неверный-ключ-неверный-ключ-1".into();
    net.sync(1, &["dev"]).await;
    let err = net.n("dev").peer_state("stand").error;
    assert!(err.contains("нет соседа с таким ключом"), "{}", err);
    net.n("dev").cfg.write().peers[0].secret = secret("dev", "stand");
    net.sync(1, &["dev"]).await;
    assert_eq!(net.read("dev", "отчёты/x.md").as_deref(), Some("x"), "с верным адресом и ключом — работает");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn security_local_api_needs_token_and_local_host() {
    let mut net = pair();
    net.start().await;
    let url = net.n("dev").urls.read()[0].clone();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let r = http.get(format!("{}/bridge/v1/local/status", url)).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401, "без токена — 401");
    let r = http.get(format!("{}/bridge/v1/local/status", url)).header("X-Bridge-Local", &net.n("dev").local_token).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let r = http.get(format!("{}/", url)).header("Host", "evil.example").send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403, "чужой Host (подмена DNS) — 403");
    let page = http.get(format!("{}/", url)).send().await.unwrap().text().await.unwrap();
    assert!(page.contains(&net.n("dev").local_token), "панель отдаётся с токеном");
    net.stop().await;
}

// ───────────────────────── Файловый интерфейс ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_interface_outbox_to_inbox() {
    let mut net = pair();
    net.start().await;
    let outbox = net.folder("dev").join("_bridge/outbox");
    std::fs::create_dir_all(&outbox).unwrap();
    let f = outbox.join("1.json");
    std::fs::write(&f, r#"{"action":"create","type":"task","fields":{"title":"из файла","assignee":"stand"}}"#).unwrap();
    let old = std::time::SystemTime::now() - Duration::from_secs(5);
    std::fs::OpenOptions::new().write(true).open(&f).unwrap().set_modified(old).unwrap();
    for _ in 0..50 {
        bridge_node::iface::process_outbox(net.n("dev"), &net.n("dev").share("s").unwrap());
        if !f.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!f.exists(), "файл действия обработан");
    assert!(std::fs::read_dir(outbox.join("done")).unwrap().count() >= 2, "в done — действие и результат");
    net.sync(1, &[]).await;
    bridge_node::iface::write(net.n("stand"), &net.n("stand").share("s").unwrap());
    let inbox: Vec<_> = std::fs::read_dir(net.folder("stand").join("_bridge/inbox")).unwrap().flatten().collect();
    assert_eq!(inbox.len(), 1, "задача во входящих стенда");
    let idx: Value = serde_json::from_str(&std::fs::read_to_string(net.folder("stand").join("_bridge/index.json")).unwrap()).unwrap();
    assert_eq!(idx[0]["title"], "из файла");
    net.stop().await;
}

// ───────────────────────── Ручная отправка ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mailbox_send_receive_ack_delete() {
    let mut net = pair();
    net.start().await;
    let out_dev = net.root.join("dev/отправка");
    assert!(out_dev.join("stand").is_dir(), "папка соседа создана сама");
    std::fs::create_dir_all(out_dev.join("stand/сервер/data")).unwrap();
    std::fs::write(out_dev.join("stand/сервер/data/users.json"), "{\"a\":1}").unwrap();
    std::fs::write(out_dev.join("stand/модель.bin"), vec![0u8; 300_000]).unwrap();
    net.sync(1, &["stand", "dev"]).await;
    let in_stand = net.root.join("stand/приём/dev");
    assert_eq!(std::fs::read_to_string(in_stand.join("сервер/data/users.json")).unwrap(), "{\"a\":1}");
    assert_eq!(std::fs::metadata(in_stand.join("модель.bin")).unwrap().len(), 300_000);
    assert!(!out_dev.join("stand/модель.bin").exists(), "после подтверждения у отправителя удалён");
    assert!(!out_dev.join("stand/сервер").exists(), "пустые папки убраны");
    assert!(out_dev.join("stand").is_dir(), "папка соседа осталась");
    assert_eq!(net.n("dev").mailbox.history().iter().filter(|h| h.kind == "sent").count(), 2);
    std::fs::create_dir_all(out_dev.join("stand/сервер/data")).unwrap();
    std::fs::write(out_dev.join("stand/сервер/data/users.json"), "{\"a\":2}").unwrap();
    net.sync(1, &["stand"]).await;
    assert!(in_stand.join("сервер/data/users (2).json").exists(), "одноимённый — рядом, не поверх");
    net.n("dev").cfg.write().mailbox.accept_from = vec!["кто-то".into()];
    std::fs::write(net.root.join("stand/отправка/dev/отчёт.txt"), "x").unwrap();
    net.sync(1, &["dev"]).await;
    assert!(!net.root.join("dev/приём/stand/отчёт.txt").exists(), "приём от не разрешённого — нет");
    assert!(net.root.join("stand/отправка/dev/отчёт.txt").exists(), "у отправителя ждёт");
    net.stop().await;
}

// ───────────────────────── Общие папки ─────────────────────────

fn folder_net() -> Net {
    let mut net = pair();
    let dev = net.configs.get_mut("dev").unwrap();
    let mut acc = indexmap::IndexMap::new();
    acc.insert("stand".to_string(), "write".to_string());
    dev.folders.push(SharedFolderConfig { id: "obshee".into(), title: "Общая".into(), path: "мои-общие/obshee".into(), access: acc, ..Default::default() });
    let mut ro = indexmap::IndexMap::new();
    ro.insert("stand".to_string(), "read".to_string());
    dev.folders.push(SharedFolderConfig { id: "chitalka".into(), path: "мои-общие/chitalka".into(), access: ro, ..Default::default() });
    net
}

fn own(net: &Net, id: &str, p: &str) -> std::path::PathBuf {
    net.root.join("dev/мои-общие").join(id).join(p)
}

fn mnt(net: &Net, id: &str, p: &str) -> std::path::PathBuf {
    net.root.join("stand/общие/dev").join(id).join(p)
}

fn put(p: &std::path::Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_folder_two_way_with_deletes() {
    let mut net = folder_net();
    net.start().await;
    put(&own(&net, "obshee", "a.txt"), "хозяин");
    net.sync(1, &["stand"]).await;
    assert_eq!(std::fs::read_to_string(mnt(&net, "obshee", "a.txt")).unwrap(), "хозяин", "копия у соседа");
    put(&mnt(&net, "obshee", "док/b.txt"), "сосед");
    net.sync(1, &["stand"]).await;
    assert_eq!(std::fs::read_to_string(own(&net, "obshee", "док/b.txt")).unwrap(), "сосед", "запись соседа у хозяина");
    std::fs::remove_file(mnt(&net, "obshee", "a.txt")).unwrap();
    net.sync(1, &["stand"]).await;
    assert!(!own(&net, "obshee", "a.txt").exists(), "удаление соседа дошло до хозяина");
    std::fs::remove_file(own(&net, "obshee", "док/b.txt")).unwrap();
    net.sync(1, &["stand"]).await;
    assert!(!mnt(&net, "obshee", "док/b.txt").exists(), "удаление хозяина дошло до соседа");
    let trash = net.n("dev").folders.trash_list();
    assert!(trash.iter().any(|t| t["path"] == "a.txt"), "удалённое — в корзине хозяина");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_folder_read_only_keeps_own_edits() {
    let mut net = folder_net();
    net.start().await;
    put(&own(&net, "chitalka", "a.txt"), "оригинал");
    net.sync(1, &["stand"]).await;
    put(&mnt(&net, "chitalka", "a.txt"), "моя правка");
    let t = std::time::SystemTime::now() + Duration::from_secs(2);
    std::fs::OpenOptions::new().write(true).open(mnt(&net, "chitalka", "a.txt")).unwrap().set_modified(t).unwrap();
    net.sync(1, &["stand"]).await;
    assert_eq!(std::fs::read_to_string(own(&net, "chitalka", "a.txt")).unwrap(), "оригинал", "у хозяина не тронуто");
    assert_eq!(std::fs::read_to_string(mnt(&net, "chitalka", "a.txt")).unwrap(), "оригинал", "копия восстановлена");
    assert_eq!(std::fs::read_to_string(mnt(&net, "chitalka", "_мои-правки/a.txt")).unwrap(), "моя правка", "своя правка сохранена");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_folder_mass_delete_needs_confirmation() {
    let mut net = folder_net();
    net.start().await;
    for i in 0..12 {
        put(&own(&net, "obshee", &format!("f{}.txt", i)), "x");
    }
    net.sync(1, &["stand"]).await;
    for i in 0..12 {
        std::fs::remove_file(own(&net, "obshee", &format!("f{}.txt", i))).unwrap();
    }
    net.sync(1, &["stand"]).await;
    let m = net.n("stand").folders.find_mount("dev", "obshee").unwrap();
    assert!(m.hold, "массовое удаление задержано");
    assert!(mnt(&net, "obshee", "f0.txt").exists(), "у соседа файлы на месте");
    net.n("stand").folders.allow_deletes("dev", "obshee");
    // и «по частям» тоже: 5 удалений, потом ещё 7 — вместе больше половины
    tokio::time::sleep(Duration::from_millis(10)).await;
    net.sync(1, &["stand"]).await;
    assert!(!mnt(&net, "obshee", "f0.txt").exists(), "после подтверждения — удалены");
    for i in 0..14 {
        put(&own(&net, "obshee", &format!("g{}.txt", i)), "x");
    }
    net.sync(1, &["stand"]).await;
    for i in 0..5 {
        std::fs::remove_file(own(&net, "obshee", &format!("g{}.txt", i))).unwrap();
    }
    net.sync(1, &["stand"]).await;
    assert!(!net.n("stand").folders.find_mount("dev", "obshee").unwrap().hold, "5 из 14 — не массовое");
    for i in 5..12 {
        std::fs::remove_file(own(&net, "obshee", &format!("g{}.txt", i))).unwrap();
    }
    net.sync(1, &["stand"]).await;
    assert!(net.n("stand").folders.find_mount("dev", "obshee").unwrap().hold, "по частям: 5 + 7 из 14 — задержано");
    assert!(mnt(&net, "obshee", "g11.txt").exists());
    net.stop().await;
}

// ───────────────────────── Соседи по адресу и ключу ─────────────────────────

async fn lonely(names: &[&str]) -> Net {
    let mut net = Net::new();
    for n in names {
        net.add(n, &[], &[("**", "*")]);
    }
    net.start().await;
    net
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_invite_line_names_learned_both_ways() {
    let net = lonely(&["alfa", "beta"]).await;
    let (inv, warn) = link::invite(net.n("beta"), "Альфа");
    assert_eq!(warn, "");
    assert!(inv.line.contains(&inv.secret) && !inv.addresses.is_empty(), "{}", inv.line);
    assert!(net.n("beta").cfg.read().peers[0].pending);
    let r = link::connect_peer(net.n("alfa"), &format!("{} {}", addr(&net, "beta"), inv.secret), "", "").await;
    assert!(r.ok && r.online, "{}", r.error);
    assert_eq!(r.name, "beta", "alfa узнала имя у соседа");
    let at_beta = net.n("beta").cfg.read().peers[0].clone();
    assert_eq!(at_beta.name, "alfa", "beta узнала имя alfa из подписанного запроса");
    assert_eq!(at_beta.title, "Альфа");
    assert!(at_beta.ips.contains(&"127.0.0.1".to_string()) && !at_beta.url.is_empty());
    assert_eq!(at_beta.url, net.n("alfa").urls.read()[0], "порт — из адресов, которые сообщила alfa");
    let mut ok = false;
    for _ in 0..30 {
        if let Ok(saved) = NodeConfig::load(&net.root.join("beta/bridge.json")) {
            if saved.peer("alfa").map(|p| !p.pending).unwrap_or(false) {
                ok = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(ok, "сохранено в bridge.json");
    net.sync(1, &["beta"]).await;
    assert!(net.n("beta").peer_state("alfa").online, "и обратная связь есть");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_same_key_both_sides_renames_everywhere() {
    let net = lonely(&["alfa", "beta"]).await;
    let key = NodeConfig::new_secret();
    let r1 = link::connect_peer(net.n("beta"), &addr(&net, "alfa"), &key, "ПК Альфа").await;
    assert!(r1.ok && !r1.online && r1.pending, "{}", r1.error);
    assert!(r1.error.contains("ключ"), "{}", r1.error);
    let temp = r1.name.clone();
    assert!(temp.starts_with("new-"));
    {
        let mut c = net.n("beta").cfg.write();
        c.shares[0].peers = vec![temp.clone()];
        c.shares[0].accept = vec![bridge_core::config::FileAcceptRule { pattern: "**".into(), from: vec![temp.clone()] }];
    }
    let t0 = std::time::Instant::now();
    let r2 = link::connect_peer(net.n("alfa"), &addr(&net, "beta"), &key, "").await;
    eprintln!("connect took {:?} at {}", t0.elapsed(), chrono::Local::now().format("%S%.3f"));
    assert!(r2.online && r2.name == "beta", "{:?}\n{}", r2, net.logs.lock().join("\n"));
    let c = net.n("beta").cfg.read().clone();
    assert!(c.peer("alfa").is_some() && c.peer(&temp).is_none());
    assert_eq!(c.shares[0].peers, vec!["alfa".to_string()]);
    assert_eq!(c.shares[0].accept[0].from, vec!["alfa".to_string()]);
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_wrong_key_clear_reason_and_stranger() {
    let net = lonely(&["alfa", "beta"]).await;
    let (inv, _) = link::invite(net.n("beta"), "");
    let wrong = format!("{}zz", &inv.secret[..inv.secret.len() - 2]);
    let r = link::connect_peer(net.n("alfa"), &addr(&net, "beta"), &wrong, "").await;
    assert!(!r.online);
    assert!(r.error.contains("нет соседа с таким ключом") && !r.hint.is_empty(), "{} / {}", r.error, r.hint);
    let ov = bridge_node::http_local::overview_value(net.n("beta"));
    assert!(ov["strangers"].as_array().unwrap().iter().any(|s| s["name"] == "alfa" && s["why"].as_str().unwrap().contains("ключ")));
    net.n("alfa").sync_now().await;
    net.n("alfa").sync_now().await;
    assert_eq!(net.logs.lock().iter().filter(|l| l.contains(" beta:") && l.contains("ключ не подходит")).count(), 1, "повторы не засоряют журнал");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connect_reconnect_with_new_key_merges_into_old_peer() {
    let mut net = pair();
    net.start().await;
    net.n("stand").cfg.write().peers[0].secret = "BRS-сломанный-ключ-сломанный-ключ-1".into();
    let key = NodeConfig::new_secret();
    link::connect_peer(net.n("dev"), &addr(&net, "stand"), &key, "").await;
    let r = link::connect_peer(net.n("stand"), &addr(&net, "dev"), &key, "").await;
    assert!(r.online && r.name == "dev", "{}", r.error);
    assert_eq!(net.n("stand").cfg.read().peers.len(), 1);
    assert_eq!(net.n("stand").cfg.read().peers[0].secret, key);
    assert_eq!(net.n("dev").cfg.read().peers.len(), 1);
    net.write("stand", "отчёты/a.md", "a");
    net.sync(1, &["dev"]).await;
    assert_eq!(net.read("dev", "отчёты/a.md").as_deref(), Some("a"));
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn address_learned_from_signed_hello() {
    let mut net = pair();
    net.configs.get_mut("stand").unwrap().peers[0].ips = vec!["10.9.9.9".into()];
    net.start().await;
    net.write("stand", "отчёты/x.md", "x");
    net.sync(1, &["dev"]).await;
    assert!(net.n("stand").cfg.read().peers[0].ips.contains(&"127.0.0.1".to_string()));
    assert_eq!(net.read("dev", "отчёты/x.md").as_deref(), Some("x"));
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn check_explains_why_no_sync() {
    let mut net = pair();
    net.configs.get_mut("stand").unwrap().shares[0].peers = vec![];
    net.configs.get_mut("dev").unwrap().shares[0].accept = vec![bridge_core::config::FileAcceptRule { pattern: "отчёты/**".into(), from: vec!["stand".into()] }];
    net.start().await;
    let c = link::check_peer(net.n("dev"), "stand").await.unwrap();
    let st = c.steps.iter().find(|s| s.title.contains("«s»")).unwrap();
    assert!(st.state == "bad" && st.text.contains("не раздаёт"), "{:?}", c.steps);
    assert_eq!(c.state, "bad");
    net.n("stand").cfg.write().shares[0].peers = vec!["dev".into()];
    net.write("stand", "отчёты/a.md", "a");
    net.write("stand", "черновики/b.md", "b");
    // сосед замечает новые файлы не мгновенно (наблюдение за папкой) — подождать
    let mut c = link::check_peer(net.n("dev"), "stand").await.unwrap();
    for _ in 0..40 {
        if c.steps.iter().any(|s| s.title.contains("«s»") && s.text.contains("не проходят правила приёма 1")) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        c = link::check_peer(net.n("dev"), "stand").await.unwrap();
    }
    let st = c.steps.iter().find(|s| s.title.contains("«s»")).unwrap();
    // файл, подходящий под правила, мог уже приехать по оповещению соседа — это тоже верно
    assert!(st.text.contains("не проходят правила приёма 1") && (st.text.contains("не совпадают с вашими: 1") || st.text.contains("все уже здесь")), "{}", st.text);
    net.sync(1, &["dev"]).await;
    let c = link::check_peer(net.n("dev"), "stand").await.unwrap();
    assert!(c.steps.iter().find(|s| s.title.contains("«s»")).unwrap().text.contains("все уже здесь"), "{:?}", c.steps);
    net.n("stand").cfg.write().peers[0].enabled = false;
    net.n("dev").sync_now().await;
    let c = link::check_peer(net.n("dev"), "stand").await.unwrap();
    assert!(c.steps.iter().any(|s| s.state == "bad" && s.text.contains("выключен")), "{}", c.summary);
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_without_shares_has_status() {
    let mut net = Net::new();
    net.add("dev", &["stand"], &[]).shares.clear();
    net.add("stand", &["dev"], &[]).shares.clear();
    net.start().await;
    net.sync(1, &["dev"]).await;
    assert!(net.n("dev").peer_state("stand").online);
    assert!(net.n("stand").peer_state("dev").last_inbound.is_some());
    net.stop().await;
}

// ───────────────────────── Группы (2.0) ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn groups_give_folder_access() {
    let mut net = pair();
    let dev = net.configs.get_mut("dev").unwrap();
    dev.groups.insert("family".into(), bridge_core::config::GroupConfig { title: "Семья".into(), members: vec!["stand".into()] });
    let mut acc = indexmap::IndexMap::new();
    acc.insert("@family".to_string(), "write".to_string());
    dev.folders.push(SharedFolderConfig { id: "foto".into(), path: "фото".into(), access: acc, ..Default::default() });
    net.start().await;
    put(&net.root.join("dev/фото/1.jpg"), "jpg");
    net.sync(1, &["stand"]).await;
    assert_eq!(std::fs::read_to_string(net.root.join("stand/общие/dev/foto/1.jpg")).unwrap(), "jpg", "член группы получил папку");
    net.n("dev").cfg.write().groups.get_mut("family").unwrap().members.clear();
    net.sync(1, &["stand"]).await;
    assert!(net.n("stand").folders.find_mount("dev", "foto").unwrap().revoked, "убрали из группы — доступ закрыт");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_keeps_state() {
    let mut net = pair();
    net.start().await;
    let id = net.act("dev", ba(json!({"action":"create","type":"task","fields":{"title":"t","assignee":"stand"}}))).record.unwrap();
    net.write("dev", "задания/a.md", "1");
    net.sync(1, &[]).await;
    let cfg = net.n("dev").cfg.read().clone();
    net.n("dev").stop().await;
    let dir = net.root.join("dev");
    let node = bridge_node::Node::start(cfg, bridge_node::StartOptions { base_dir: dir.clone(), config_file: None, console: false, log: Some(std::sync::Arc::new(|_: &str| {})), settle_secs: 0.0, timer: false })
        .await
        .unwrap();
    assert!(node.share("s").unwrap().records.lock().get(&id).is_some(), "записи после перезапуска");
    let r = bridge_node::shares::act(&node, ba(json!({"action":"comment","record":id,"text":"после перезапуска","share":"s"})));
    assert!(r.ok && r.eid.as_deref().map(|e| e.ends_with(":2")).unwrap_or(false), "seq продолжился: {:?}", r.eid);
    node.stop().await;
    net.stop().await;
}

async fn wait_transfers(net: &Net, node: &str, key: &str) {
    for _ in 0..200 {
        if net.n(node).folders.transfer_view(key).0.is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("передача не закончилась");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_folder_big_file_goes_in_background_small_does_not_wait() {
    let mut net = folder_net();
    net.start().await;
    let big = vec![7u8; 9 * 1024 * 1024];
    std::fs::create_dir_all(mnt(&net, "obshee", "")).unwrap();
    std::fs::write(mnt(&net, "obshee", "big.bin"), &big).unwrap();
    put(&mnt(&net, "obshee", "small.txt"), "маленький");
    net.sync(1, &["stand"]).await;
    assert_eq!(std::fs::read_to_string(own(&net, "obshee", "small.txt")).unwrap(), "маленький", "маленький ушёл сразу");
    wait_transfers(&net, "stand", "dev/obshee").await;
    assert_eq!(std::fs::read(own(&net, "obshee", "big.bin")).unwrap().len(), big.len(), "большой дошёл в фоне");
    net.sync(1, &["stand"]).await;
    // большой у хозяина → скачивание в фоне, установка следующим обменом
    std::fs::write(own(&net, "obshee", "big2.bin"), &big).unwrap();
    net.n("dev").folders.invalidate_own();
    net.sync(1, &["stand"]).await;
    wait_transfers(&net, "stand", "dev/obshee").await;
    net.sync(1, &["stand"]).await;
    assert_eq!(std::fs::read(mnt(&net, "obshee", "big2.bin")).unwrap().len(), big.len(), "большой скачан в фоне и поставлен");
    let m = net.n("stand").folders.find_mount("dev", "obshee").unwrap();
    assert!(m.recent.iter().any(|r| r.contains("отправлен хозяину: big.bin")), "{:?}", m.recent);
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_folder_too_big_is_explained_not_retried() {
    let mut net = folder_net();
    net.configs.get_mut("dev").unwrap().max_file_mb = 1;
    net.start().await;
    std::fs::create_dir_all(mnt(&net, "obshee", "")).unwrap();
    std::fs::write(mnt(&net, "obshee", "huge.bin"), vec![1u8; 2 * 1024 * 1024]).unwrap();
    put(&mnt(&net, "obshee", "ok.txt"), "ок");
    net.sync(1, &["stand"]).await;
    wait_transfers(&net, "stand", "dev/obshee").await;
    net.sync(1, &["stand"]).await;
    assert!(own(&net, "obshee", "ok.txt").exists(), "маленький прошёл");
    assert!(!own(&net, "obshee", "huge.bin").exists(), "большой хозяин не принял");
    let (_, failed) = net.n("stand").folders.transfer_view("dev/obshee");
    assert!(
        failed.iter().any(|f| f["path"] == "huge.bin" && f["reason"].as_str().unwrap().contains("предел")),
        "{:?} / история: {:?} / журнал: {:?}",
        failed,
        net.n("stand").folders.find_mount("dev", "obshee").map(|m| m.recent),
        net.logs.lock().iter().filter(|l| l.contains("obshee") || l.contains("huge")).cloned().collect::<Vec<_>>()
    );
    let m = net.n("stand").folders.find_mount("dev", "obshee").unwrap();
    let notes = m.recent.iter().filter(|r| r.contains("huge.bin") && r.contains("не отправлен")).count();
    assert_eq!(notes, 1, "одна понятная запись, без повторов: {:?}", m.recent);
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_delete_and_restore_everywhere() {
    let mut net = pair();
    net.start().await;
    let r = net.act("dev", ba(json!({ "action": "create", "type": "task", "fields": { "title": "Лишняя", "assignee": "stand" } })));
    assert!(r.ok, "{}", r.error);
    let id = r.record.unwrap();
    net.sync(2, &["stand", "dev"]).await;
    assert!(status(&net, "stand", &id).is_some(), "задача дошла");
    let d = net.act("stand", ba(json!({ "action": "delete", "record": id })));
    assert!(d.ok, "{}", d.error);
    net.sync(2, &["dev", "stand"]).await;
    for n in ["dev", "stand"] {
        let rt = &net.n(n).shares()[0];
        let rb = rt.records.lock();
        assert!(rb.all().iter().all(|s| s.id != id), "{}: удалённой нет в списке", n);
        assert!(rb.deleted().iter().any(|s| s.id == id), "{}: она в удалённых", n);
    }
    let r2 = net.act("dev", ba(json!({ "action": "restore", "record": id })));
    assert!(r2.ok, "{}", r2.error);
    net.sync(2, &["stand", "dev"]).await;
    assert!(net.n("stand").shares()[0].records.lock().all().iter().any(|s| s.id == id), "вернулась у соседа");
    net.stop().await;
}

// ───────────────────────── Агент ─────────────────────────

fn agent_net(mode: &str) -> Net {
    let mut net = pair();
    let st = net.configs.get_mut("stand").unwrap();
    st.agent.mode = mode.into();
    st.agent.allow_from = vec!["dev".into()];
    st.agent.claude = env!("CARGO_BIN_EXE_fake-claude").into();
    st.agent.max_minutes = 2;
    net
}

async fn agent_wait_status(net: &Net, node: &str, id: &str, want: &[&str]) -> bridge_node::agent::Thread {
    for _ in 0..200 {
        if let Some(t) = net.n(node).agent.get(id) {
            if want.contains(&t.status.as_str()) {
                return t;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("{}: нить {} не дошла до {:?}: {:?}", node, id, want, net.n(node).agent.get(id).map(|t| (t.status, t.error)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_auto_runs_claude_and_continues_session() {
    let mut net = agent_net("auto");
    net.start().await;
    net.sync(1, &["dev", "stand"]).await;
    assert_eq!(net.n("dev").peer_state("stand").agent, "auto", "оркестратор видит агента соседа");
    let dev = net.n("dev").clone();
    let t = bridge_node::agent::ask(&dev, "stand", "прогони тесты базиса", None, None).await.expect("поручено");
    let t = agent_wait_status(&net, "dev", &t.id, &["idle", "failed"]).await;
    assert_eq!(t.status, "idle", "{}", t.error);
    assert!(t.result.contains("Готово: прогони тесты базиса"), "{}", t.result);
    assert!(t.events.iter().any(|e| e.kind == "tool" && e.text.contains("run-tests")), "ход: инструменты видны оркестратору");
    assert!(t.cost > 0.0);
    let session = net.n("stand").agent.get(&t.id).unwrap().session;
    assert!(!session.is_empty());
    // продолжение — та же сессия
    bridge_node::agent::ask(&dev, "stand", "теперь только упавшие", Some(&t.id), None).await.expect("продолжено");
    let t2 = agent_wait_status(&net, "dev", &t.id, &["idle", "failed"]).await;
    let t2 = if t2.result.contains("упавшие") { t2 } else { tokio::time::sleep(std::time::Duration::from_millis(800)).await; dev.agent.get(&t.id).unwrap() };
    assert!(t2.result.contains("Продолжаю") || t2.events.iter().any(|e| e.text.contains("Продолжаю: теперь только упавшие")), "{:?}", t2.events.iter().map(|e| &e.text).collect::<Vec<_>>());
    assert_eq!(net.n("stand").agent.get(&t.id).unwrap().session, session, "сессия та же");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_refuses_not_allowed_and_off() {
    let mut net = agent_net("auto");
    net.configs.get_mut("stand").unwrap().agent.allow_from = vec![];
    net.start().await;
    let dev = net.n("dev").clone();
    let e = bridge_node::agent::ask(&dev, "stand", "что-нибудь", None, None).await.unwrap_err();
    assert!(e.1.contains("не разрешено"), "{}", e.1);
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_manual_take_progress_report() {
    let mut net = agent_net("manual");
    net.start().await;
    let dev = net.n("dev").clone();
    let stand = net.n("stand").clone();
    let t = bridge_node::agent::ask(&dev, "stand", "проверь сборку", None, None).await.expect("поручено");
    assert_eq!(t.status, "queued", "ждёт, пока возьмут");
    assert_eq!(bridge_node::agent::inbox(&stand).len(), 1);
    let v = bridge_node::agent::take(&stand, &t.id).expect("взято");
    assert!(v["prompt"].as_str().unwrap().contains("проверь сборку"));
    assert!(bridge_node::agent::take(&stand, &t.id).is_err(), "второй раз не взять");
    bridge_node::agent::progress(&stand, &t.id, "собираю…").unwrap();
    bridge_node::agent::report(&stand, &t.id, "сборка зелёная", true).unwrap();
    let t = agent_wait_status(&net, "dev", &t.id, &["idle"]).await;
    assert_eq!(t.result, "сборка зелёная");
    assert!(t.events.iter().any(|e| e.text == "собираю…"), "ход дошёл");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_stop_from_orchestrator() {
    let mut net = agent_net("auto");
    net.start().await;
    let dev = net.n("dev").clone();
    let t = bridge_node::agent::ask(&dev, "stand", "долго считай", None, None).await.expect("поручено");
    agent_wait_status(&net, "stand", &t.id, &["running"]).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    bridge_node::agent::stop(&dev, &t.id).await.expect("остановка");
    let t = agent_wait_status(&net, "dev", &t.id, &["stopped"]).await;
    assert!(t.events.iter().any(|e| e.text.contains("остановил")), "{:?}", t.events.iter().map(|e| &e.text).collect::<Vec<_>>());
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_exchange_folder_files_reach_orchestrator() {
    let mut net = agent_net("auto");
    let dev = net.configs.get_mut("dev").unwrap();
    let mut acc = indexmap::IndexMap::new();
    acc.insert("stand".to_string(), "write".to_string());
    dev.folders.push(SharedFolderConfig { id: "agent-obmen".into(), path: "обмен".into(), access: acc, ..Default::default() });
    dev.agent.exchange = "agent-obmen".into();
    net.start().await;
    net.sync(1, &["stand"]).await; // обменная папка появилась у исполнителя
    let devn = net.n("dev").clone();
    let t = bridge_node::agent::ask(&devn, "stand", "сделай отчёт", None, None).await.expect("поручено");
    agent_wait_status(&net, "dev", &t.id, &["idle"]).await;
    let on_stand = net.root.join("stand/общие/dev/agent-obmen/report.txt");
    assert!(on_stand.is_file(), "исполнитель положил отчёт в обменную папку: {}", on_stand.display());
    net.sync(1, &["stand"]).await;
    let on_dev = net.root.join("dev/обмен/report.txt");
    assert_eq!(std::fs::read_to_string(&on_dev).unwrap(), "отчёт: сделай отчёт", "отчёт дошёл до оркестратора");
    net.stop().await;
}

// ───────────────────────── Телефон ─────────────────────────

fn phone_http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Подключить телефон по коду: (ключ-cookie, id).
async fn phone_pair(net: &Net, node: &str, name: &str, rights: &str) -> (String, String) {
    let n = net.n(node);
    let url = n.urls.read()[0].clone();
    let (code, _) = n.mobile.new_code(rights);
    let r = phone_http().post(format!("{}/m/pair", url)).header("X-Bridge-M", "1").json(&json!({ "code": code, "name": name })).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let cookie = r.headers().get("set-cookie").unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    let v: serde_json::Value = r.json().await.unwrap();
    (cookie, v["device"]["id"].as_str().unwrap().to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_pair_rights_and_guards() {
    let mut net = pair();
    net.start().await;
    let dev = net.n("dev").clone();
    let url = dev.urls.read()[0].clone();
    let h = phone_http();
    // код — только со страницы моста (заголовок), один раз
    let (code, _) = dev.mobile.new_code("full");
    let r = h.post(format!("{}/m/pair", url)).json(&json!({ "code": code, "name": "x" })).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403, "без заголовка страницы — отказ");
    let (cookie, id) = phone_pair(&net, "dev", "Pixel", "full").await;
    let get = |p: &str| h.get(format!("{}{}", url, p)).header("Cookie", &cookie).send();
    assert_eq!(get("/m/me").await.unwrap().status().as_u16(), 200);
    assert_eq!(h.get(format!("{}/m/me", url)).send().await.unwrap().status().as_u16(), 401, "без ключа — не подключён");
    // полный доступ: как панель, но без ключей, телефонов и пульта
    assert_eq!(get("/m/api/agent").await.unwrap().status().as_u16(), 200);
    assert_eq!(get("/m/api/devices").await.unwrap().status().as_u16(), 403);
    let r = h.post(format!("{}/m/api/secret", url)).header("Cookie", &cookie).header("X-Bridge-M", "1").send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403);
    let r = h.post(format!("{}/m/api/sync", url)).header("Cookie", &cookie).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403, "изменения без заголовка страницы — отказ");
    // только файлы
    dev.mobile.update(&dev, &id, |d| d.rights = "files".into());
    assert_eq!(get("/m/api/agent").await.unwrap().status().as_u16(), 403);
    assert_eq!(get("/m/api/folders").await.unwrap().status().as_u16(), 200);
    // через прокси (Tailscale Serve) — только /m/, даже с 127.0.0.1 и верным токеном
    let r = h.get(format!("{}/", url)).header("X-Forwarded-For", "100.64.0.9").send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403, "панель через прокси — нельзя");
    let r = h.get(format!("{}/bridge/v1/local/status", url)).header("X-Forwarded-For", "100.64.0.9").header("X-Bridge-Local", &dev.local_token).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403, "локальный API через прокси — нельзя");
    let r = h.get(format!("{}/m/me", url)).header("Cookie", &cookie).header("X-Forwarded-For", "100.64.0.9").header("X-Forwarded-Proto", "https").send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200, "страница телефона через прокси — можно");
    // отключили на ПК — ключ не действует
    assert!(dev.mobile.remove(&dev, &id));
    assert_eq!(get("/m/me").await.unwrap().status().as_u16(), 401);
    // подбор кода: 10 ошибок — все коды сгорают
    let (code, _) = dev.mobile.new_code("full");
    for _ in 0..10 {
        let _ = h.post(format!("{}/m/pair", url)).header("X-Bridge-M", "1").json(&json!({ "code": "ZZZZZZZZ" })).send().await.unwrap();
    }
    let r = h.post(format!("{}/m/pair", url)).header("X-Bridge-M", "1").json(&json!({ "code": code })).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 403, "после подбора верный код уже не действует");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_files_both_ways() {
    let mut net = pair();
    net.start().await;
    net.sync(1, &["dev", "stand"]).await;
    let dev = net.n("dev").clone();
    let url = dev.urls.read()[0].clone();
    let h = phone_http();
    let (cookie, id) = phone_pair(&net, "dev", "Pixel", "full").await;
    // с телефона на ПК — в «приём/Pixel»
    let body = vec![7u8; 300_000];
    let r = h.post(format!("{}/m/upload?path=фото.jpg&size=300000", url)).header("Cookie", &cookie).header("X-Bridge-M", "1").body(body.clone()).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    assert_eq!(std::fs::read(net.root.join("dev/приём/Pixel/фото.jpg")).unwrap(), body);
    // недописанный файл не появляется
    let r = h.post(format!("{}/m/upload?path=обрыв.bin&size=999999", url)).header("Cookie", &cookie).header("X-Bridge-M", "1").body(vec![1u8; 10]).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 400);
    assert!(!net.root.join("dev/приём/Pixel/обрыв.bin").exists());
    // с телефона соседу — через этот ПК
    let r = h.post(format!("{}/m/api/mailbox/upload?peer=stand&path=соседу.txt", url)).header("Cookie", &cookie).header("X-Bridge-M", "1").body("привет").send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    net.sync(2, &["stand", "dev"]).await;
    assert_eq!(std::fs::read_to_string(net.root.join("stand/приём/dev/соседу.txt")).unwrap(), "привет", "сосед получил от ПК");
    // с ПК на телефон: ассистент кладёт файл, телефон видит и скачивает
    let src = net.root.join("отчёт.pdf");
    std::fs::write(&src, b"PDF").unwrap();
    let d = dev.mobile.find(&id).unwrap();
    bridge_node::mobile::put_outbox(&dev, &d, "отчёт.pdf", &src, false).unwrap();
    let v: serde_json::Value = h.get(format!("{}/m/outbox", url)).header("Cookie", &cookie).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["files"][0]["path"], "отчёт.pdf");
    assert_eq!(v["files"][0]["got"], false);
    let got = h.get(format!("{}/m/outbox/file?path=отчёт.pdf", url)).header("Cookie", &cookie).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(&got[..], b"PDF");
    let v: serde_json::Value = h.get(format!("{}/m/outbox", url)).header("Cookie", &cookie).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["files"][0]["got"], true, "скачан — отмечен");
    assert!(net.root.join("dev/на телефон/Pixel/отчёт.pdf").is_file(), "папка «на телефон/Pixel» видна в Проводнике");
    net.stop().await;
}

// ───────────────────────── Ассистент (контракт v1) ─────────────────────────

async fn local(net: &Net, node: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let n = net.n(node);
    let url = format!("{}/bridge/v1/local/{}", n.urls.read()[0], path);
    let h = phone_http();
    let rb = if method == "GET" { h.get(url) } else { h.post(url).json(&body.unwrap_or(json!({}))) };
    let r = rb.header("X-Bridge-Local", &n.local_token).send().await.unwrap();
    let code = r.status().as_u16();
    (code, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn assistant_host_and_client() {
    let mut net = pair();
    net.start().await;
    // ассистент на dev: признак, присутствие
    let (c, info) = local(&net, "dev", "GET", "assistant/info", None).await;
    assert_eq!(c, 200);
    assert_eq!(info["version"], 1);
    local(&net, "dev", "POST", "assistant/presence", Some(json!({ "online": true, "title": "Ассистент", "version": 1 }))).await;
    net.sync(1, &["stand", "dev"]).await;
    let a = net.n("stand").peer_state("dev").assistant.expect("клиент видит ассистента в привете");
    assert_eq!((a.host.as_str(), a.online), ("dev", true));
    // клиент пишет — у хозяина peer = stand, даже если в теле подставили другое
    let (c, _) = local(&net, "stand", "POST", "assistant/send", Some(json!({ "kind": "text", "text": "что у меня сегодня?", "peer": "dev" }))).await;
    assert_eq!(c, 200);
    let (_, w) = local(&net, "dev", "GET", "assistant/wait?after=0&timeout=10", None).await;
    let it = &w["items"][0];
    assert_eq!(it["peer"], "stand");
    assert_eq!(it["text"], "что у меня сегодня?");
    let (seq, id) = (it["seq"].as_u64().unwrap(), it["id"].as_str().unwrap().to_string());
    // прямой протокол соседей: подделать peer нельзя
    let dev_peer = net.n("stand").cfg.read().peer("dev").cloned().unwrap();
    let client = net.n("stand").client_for(&dev_peer);
    let (c, _) = client.post_json("/peer/assistant/msg", &json!({ "id": "x-1", "kind": "text", "text": "я хозяин", "peer": "dev", "seq": 999 })).await.unwrap();
    assert_eq!(c, 200);
    let (_, w2) = local(&net, "dev", "GET", &format!("assistant/wait?after={}&timeout=5", seq), None).await;
    assert_eq!(w2["items"][0]["peer"], "stand", "peer ставит хозяин по подписи");
    assert_ne!(w2["items"][0]["seq"], 999);
    // повтор того же id — не дубль
    client.post_json("/peer/assistant/msg", &json!({ "id": "x-1", "kind": "text", "text": "я хозяин" })).await.unwrap();
    let (_, w3) = local(&net, "dev", "GET", &format!("assistant/wait?after={}&timeout=1", seq), None).await;
    assert_eq!(w3["items"].as_array().unwrap().len(), 1, "повтор отброшен");
    // ответ с файлом и кнопками
    let r = json!({ "id": "r-1", "to": "stand", "kind": "reply", "replyTo": id, "text": "Сегодня: полить цветы",
        "files": [{ "name": "Список.md", "mime": "text/markdown", "size": 5, "data": "0KHQv9C4" }],
        "buttons": [[{ "label": "+10 мин", "action": "snooze", "args": { "min": 10 } }]] });
    let (c, d) = local(&net, "dev", "POST", "assistant/reply", Some(r.clone())).await;
    assert_eq!((c, d["delivered"].as_bool()), (200, Some(true)), "{}", d);
    local(&net, "dev", "POST", "assistant/reply", Some(r)).await; // повтор ответа — не дубль
    let (_, chat) = local(&net, "stand", "GET", "assistant/chat", None).await;
    let ins: Vec<&Value> = chat["items"].as_array().unwrap().iter().filter(|v| v["dir"] == "in").collect();
    assert_eq!(ins.len(), 1);
    assert_eq!(ins[0]["buttons"][0][0]["action"], "snooze");
    assert_eq!(std::fs::read_to_string(net.root.join("stand/приём/ассистент/Список.md")).unwrap(), "Спи");
    assert_eq!(chat["unread"], 1);
    // кнопка → входящее action у хозяина, кнопки этого ответа нажаты
    local(&net, "stand", "POST", "assistant/send", Some(json!({ "kind": "action", "action": { "name": "snooze", "args": { "min": 10 } }, "label": "+10 мин", "replyTo": "r-1" }))).await;
    let (_, w4) = local(&net, "dev", "GET", &format!("assistant/wait?after={}&timeout=10", seq + 1), None).await;
    assert_eq!(w4["items"][0]["kind"], "action");
    assert_eq!(w4["items"][0]["action"]["args"]["min"], 10);
    let (_, chat) = local(&net, "stand", "GET", "assistant/chat", None).await;
    assert!(chat["items"].as_array().unwrap().iter().any(|v| v["id"] == "r-1" && v["pressed"] == "+10 мин"));
    // неизвестный узел — 404
    let (c, _) = local(&net, "dev", "POST", "assistant/reply", Some(json!({ "to": "nobody", "kind": "notice", "text": "x" }))).await;
    assert_eq!(c, 404);
    // клиент недоступен — notice в очередь, доставится, когда появится
    let good = net.n("dev").cfg.read().peer("stand").unwrap().url.clone();
    net.n("dev").cfg.write().peers.iter_mut().find(|p| p.name == "stand").unwrap().url = "http://127.0.0.1:9".into();
    let (_, q) = local(&net, "dev", "POST", "assistant/reply", Some(json!({ "id": "n-1", "to": "stand", "kind": "notice", "text": "⏰ подъём" }))).await;
    assert_eq!((q["delivered"].as_bool(), q["queued"].as_bool()), (Some(false), Some(true)), "{}", q);
    net.n("dev").cfg.write().peers.iter_mut().find(|p| p.name == "stand").unwrap().url = good;
    net.sync(1, &["dev"]).await;
    let mut got = false;
    for _ in 0..40 {
        let (_, chat) = local(&net, "stand", "GET", "assistant/chat", None).await;
        if chat["items"].as_array().unwrap().iter().any(|v| v["id"] == "n-1" && v["alarm"] == true) {
            got = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(got, "notice из очереди дошёл");
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runners_and_turn_in_hello() {
    let mut net = pair();
    {
        let st = net.configs.get_mut("stand").unwrap();
        st.runners.insert("tv".into(), RunnerConfig { enabled: true, title: "Телевизор".into(), share: "s".into(), exe: "x".into(), allow_from: vec!["dev".into()], hint: "cmd: pause | play".into(), confirm: true, ..Default::default() });
        st.runners.insert("secret".into(), RunnerConfig { enabled: true, share: "s".into(), exe: "x".into(), allow_from: vec!["other".into()], ..Default::default() });
        st.turn_port = 0;
        net.configs.get_mut("dev").unwrap().turn_port = 0;
    }
    net.start().await;
    net.sync(1, &["dev"]).await;
    let ps = net.n("dev").peer_state("stand");
    assert_eq!(ps.runners.len(), 1, "только разрешённые этому соседу");
    assert_eq!(ps.runners[0].name, "tv");
    assert_eq!(ps.runners[0].hint, "cmd: pause | play");
    let (_, ov) = local(&net, "dev", "GET", "overview", None).await;
    let p = ov["peers"].as_array().unwrap().iter().find(|p| p["name"] == "stand").unwrap();
    assert_eq!(p["runners"][0]["title"], "Телевизор", "ассистент читает peers[].runners в overview");
    assert!(ps.turn.is_none(), "ретранслятор выключен (turnPort 0) — в привете его нет");
    net.stop().await;
}

// ───────────────────────── Чаты ─────────────────────────

async fn chat_wait(net: &Net, node: &str, conv: &str, text: &str) -> Option<Value> {
    for _ in 0..60 {
        if let Some(v) = net.n(node).chat.after(conv, 0).into_iter().find(|v| v["text"] == text) {
            return Some(v);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_direct_and_channel_through_owner() {
    let mut net = Net::new();
    net.add("alfa", &["beta", "gamma"], &[]);
    net.add("beta", &["alfa"], &[]);
    net.add("gamma", &["alfa"], &[]);
    net.configs.get_mut("alfa").unwrap().channels.insert("obshiy".into(), bridge_core::config::ChannelConfig { title: "Общий".into(), members: vec!["*".into()] });
    net.start().await;
    net.sync(1, &["beta", "gamma", "alfa"]).await;
    // личное
    let alfa = net.n("alfa").clone();
    let r = bridge_node::chat::send(&alfa, "dm:beta", "привет, бета", "").await.unwrap();
    assert_eq!(r["delivered"], true);
    let m = chat_wait(&net, "beta", "dm:alfa", "привет, бета").await.expect("бета получила личное");
    assert_eq!(m["from"], "alfa");
    // канал: beta пишет — gamma (не сосед beta) получает через alfa
    let beta = net.n("beta").clone();
    bridge_node::chat::send(&beta, "ch:alfa:obshiy", "всем привет", "Ассистент").await.unwrap();
    let g = chat_wait(&net, "gamma", "ch:alfa:obshiy", "всем привет").await.expect("гамма получила через владельца");
    assert_eq!(g["from"], "beta");
    assert_eq!(g["author"], "Ассистент", "подпись автора (например, ассистент)");
    assert!(chat_wait(&net, "alfa", "ch:alfa:obshiy", "всем привет").await.is_some(), "и у владельца");
    assert_eq!(net.n("beta").chat.after("ch:alfa:obshiy", 0).len(), 1, "отправителю не вернулось дублем");
    // подменить отправителя нельзя: alfa ставит from по подписи
    let ap = net.n("beta").cfg.read().peer("alfa").cloned().unwrap();
    let (c, _) = net.n("beta").client_for(&ap).post_json("/peer/chat/msg", &json!({ "id": "x1", "kind": "ch", "conv": "ch:alfa:obshiy", "from": "gamma", "text": "я гамма" })).await.unwrap();
    assert_eq!(c, 200);
    let s = chat_wait(&net, "gamma", "ch:alfa:obshiy", "я гамма").await.unwrap();
    assert_eq!(s["from"], "beta", "отправитель — по подписи");
    // не владелец рассылать сообщения чужого канала не может
    let (c, _) = net.n("alfa").client_for(&net.n("alfa").cfg.read().peer("beta").cloned().unwrap()).post_json("/peer/chat/msg", &json!({ "id": "x2", "kind": "ch", "conv": "ch:gamma:x", "from": "gamma", "text": "чужое" })).await.unwrap();
    assert_eq!(c, 403);
    // непрочитанное и «прочитано»
    assert!(net.n("gamma").chat.unread_total() >= 1);
    net.n("gamma").chat.mark_read("ch:alfa:obshiy");
    assert_eq!(net.n("gamma").chat.unread_total(), 0);
    // адресат не на связи — очередь, доставка при появлении
    let good = alfa.cfg.read().peer("beta").unwrap().url.clone();
    alfa.cfg.write().peers.iter_mut().find(|p| p.name == "beta").unwrap().url = "http://127.0.0.1:9".into();
    alfa.sync_now().await;
    let r = bridge_node::chat::send(&alfa, "dm:beta", "пока тебя не было", "").await.unwrap();
    assert_eq!(r["delivered"], false);
    alfa.cfg.write().peers.iter_mut().find(|p| p.name == "beta").unwrap().url = good;
    net.sync(1, &["alfa"]).await;
    alfa.chat.kick.notify_one();
    assert!(chat_wait(&net, "beta", "dm:alfa", "пока тебя не было").await.is_some(), "доставлено из очереди");
    net.stop().await;
}

// ───────────────────────── 2.12: обновление, стикеры, звук входа ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_offer_accept_install_and_restart() {
    use sha2::{Digest, Sha256};
    let mut net = pair();
    net.start().await;
    net.sync(1, &["dev", "stand"]).await;
    let (alfa, beta) = (net.n("dev").clone(), net.n("stand").clone());
    let names = bridge_node::update::names();
    // «программы»: у alfa — новые, у beta — старые
    let da = net.folder("dev").join("_exe");
    let db = net.folder("stand").join("_exe");
    std::fs::create_dir_all(&da).unwrap();
    std::fs::create_dir_all(&db).unwrap();
    for n in names {
        std::fs::write(da.join(n), format!("новый {}", n)).unwrap();
        std::fs::write(db.join(n), format!("старый {}", n)).unwrap();
    }
    *alfa.updates.dir_override.lock() = Some(da.clone());
    *beta.updates.dir_override.lock() = Some(db.clone());
    let restarted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let r2 = restarted.clone();
    *beta.updates.restart_hook.lock() = Some(std::sync::Arc::new(move || r2.store(true, std::sync::atomic::Ordering::SeqCst)));
    // та же версия — предлагать нечего
    let r = bridge_node::update::offer(&alfa, &[]).await;
    assert!(r["results"]["stand"].as_str().unwrap().starts_with("уже"), "{}", r);
    // предложение «новее» (как если бы у alfa была 99.0.0)
    let files: Vec<Value> = names.iter().map(|n| { let b = std::fs::read(da.join(n)).unwrap(); json!({ "name": n, "size": b.len(), "sha256": hex::encode(Sha256::digest(&b)) }) }).collect();
    let bp = alfa.cfg.read().peer("stand").cloned().unwrap();
    let c = alfa.client_for(&bp);
    // старее или равная — отказ; чужие имена файлов — отказ
    let (code, _) = c.post_json("/peer/update/offer", &json!({ "version": "1.0.0", "files": files })).await.unwrap();
    assert_eq!(code, 409);
    let (code, _) = c.post_json("/peer/update/offer", &json!({ "version": "99.0.0", "files": [{ "name": "../evil.exe", "size": 1, "sha256": "0".repeat(64) }] })).await.unwrap();
    assert_eq!(code, 400);
    // испорченный отпечаток — ничего не заменено
    let mut bad = files.clone();
    bad[0]["sha256"] = json!("0".repeat(64));
    let (code, _) = c.post_json("/peer/update/offer", &json!({ "version": "99.0.0", "files": bad, "notes": "## 99.0.0" })).await.unwrap();
    assert_eq!(code, 200);
    assert_eq!(beta.updates.state()["offer"]["from"], "dev");
    assert!(bridge_node::update::accept(&beta).await.is_err());
    assert_eq!(beta.updates.state()["status"], "error");
    for n in names {
        assert_eq!(std::fs::read_to_string(db.join(n)).unwrap(), format!("старый {}", n), "при ошибке файлы не тронуты");
    }
    // как надо
    let (code, _) = c.post_json("/peer/update/offer", &json!({ "version": "99.0.0", "files": files, "notes": "## 99.0.0\n- новое" })).await.unwrap();
    assert_eq!(code, 200);
    bridge_node::update::accept(&beta).await.unwrap();
    for n in names {
        assert_eq!(std::fs::read_to_string(db.join(n)).unwrap(), format!("новый {}", n), "заменён {}", n);
        assert_eq!(std::fs::read_to_string(db.join(format!("{}.old", n))).unwrap(), format!("старый {}", n), "старый — рядом, до перезапуска");
    }
    assert!(restarted.load(std::sync::atomic::Ordering::SeqCst), "попросили перезапуск");
    assert_eq!(beta.updates.state()["status"], "restart");
    assert!(beta.updates.state()["offer"].is_null());
    // новый запуск: *.old убраны, в журнале — «обновлён»
    bridge_node::update::cleanup(&beta);
    for n in names {
        assert!(!db.join(format!("{}.old", n)).exists());
    }
    // отдать можно только свои файлы программы
    let ap = beta.cfg.read().peer("dev").cloned().unwrap();
    let (code, _) = beta.client_for(&ap).get_bytes("/peer/update/file?name=bridge.json").await.unwrap();
    assert_eq!(code, 404);
    // что нового: выдержка журнала после версии соседа
    let n = bridge_node::update::notes_since("2.11.0");
    assert!(n.contains(&format!("## {}", bridge_core::APP_VERSION)) && !n.contains("## 2.11.0"), "{}", n);
    assert_eq!(bridge_node::update::cmp("2.10.0", "2.9.1"), std::cmp::Ordering::Greater);
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stickers_shared_and_join_sound() {
    let mut net = pair();
    net.start().await;
    net.sync(1, &["dev", "stand"]).await;
    let (alfa, beta) = (net.n("dev").clone(), net.n("stand").clone());
    // свой стикер-гифка
    let gif: Vec<u8> = [b"GIF89a".as_slice(), &[1, 0, 1, 0, 0x80, 0, 0], &[7u8; 40]].concat();
    let id = bridge_node::stickers::add(&alfa, &gif).unwrap();
    assert!(id.ends_with(".gif"));
    assert!(bridge_node::stickers::add(&alfa, b"not an image").is_err());
    assert_eq!(bridge_node::stickers::list(&alfa)["mine"][0]["id"], id.as_str());
    // отправить — у beta сохранён отдельно, в «Присланные»
    let a = bridge_node::stickers::attachment(&alfa, &id).unwrap();
    let r = bridge_node::chat::send_with(&alfa, "dm:stand", "", "", &[a]).await.unwrap();
    assert_eq!(r["message"]["files"][0]["sticker"], true);
    let mut got = Value::Null;
    for _ in 0..60 {
        got = bridge_node::stickers::list(&beta);
        if !got["got"].as_array().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(got["got"][0]["id"], id.as_str(), "{}", got);
    assert_eq!(got["got"][0]["from"], "dev");
    let m = beta.chat.after("dm:dev", 0).pop().unwrap();
    assert_eq!(m["files"][0]["sticker"], true);
    assert_eq!(bridge_node::chat::preview(&m), "стикер");
    // забрать себе; удалить из присланных
    bridge_node::stickers::keep(&beta, &id).unwrap();
    assert_eq!(bridge_node::stickers::list(&beta)["mine"][0]["id"], id.as_str());
    bridge_node::stickers::remove(&beta, "got", &id).unwrap();
    assert!(bridge_node::stickers::list(&beta)["got"].as_array().unwrap().is_empty());
    assert!(bridge_node::stickers::path(&beta, "", "../x.gif").is_none());
    // звук входа: отпечаток — в привете, beta забирает звук у alfa
    let snd: Vec<u8> = [b"ID3".as_slice(), &[3u8; 200]].concat();
    bridge_node::sound::set(&alfa, &snd).unwrap();
    assert!(bridge_node::sound::set(&alfa, b"nope").is_err());
    net.sync(1, &["stand"]).await;
    assert_eq!(beta.peer_state("dev").sound, bridge_node::sound::hash_of(&snd));
    assert_eq!(bridge_node::sound::of_peer(&beta, "dev").await.unwrap(), snd);
    // аватарка-гифка (до 2 МБ) принимается как есть
    let big_gif: Vec<u8> = [b"GIF89a".as_slice(), &vec![1u8; 900 * 1024]].concat();
    assert!(bridge_node::avatar::set(&alfa, &big_gif).is_ok());
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn board_shared_through_owner_with_images_and_stands() {
    let mut net = pair();
    net.configs.get_mut("dev").unwrap().channels.insert("obshiy".into(), bridge_core::config::ChannelConfig { title: "Общий".into(), members: vec!["*".into()] });
    net.start().await;
    net.sync(1, &["stand", "dev"]).await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    use bridge_node::board;
    assert_eq!(board::room(&stand, "dev:nope").err().unwrap().0, 404, "только каналы");
    let rd = board::room(&dev, "dev:obshiy").unwrap();
    let rs = board::room(&stand, "dev:obshiy").unwrap();
    // участник видит снимок через владельца; доска выключена — рисовать нельзя
    let s0 = board::state(&stand, &rs).await.unwrap();
    assert_eq!(s0["enabled"], false);
    assert_eq!(board::op(&stand, &rs, "x", &[json!({ "t": "put", "o": { "id": "a1", "t": "rect" } })]).await.err().unwrap().0, 409);
    // включить может любой участник
    board::op(&stand, &rs, "x", &[json!({ "t": "enable", "on": true })]).await.unwrap();
    let seq0 = board::state(&stand, &rs).await.unwrap()["seq"].as_u64().unwrap();
    // нарисовать у владельца — участник получает изменение долгим опросом
    let (s2, r2) = (stand.clone(), rs.clone());
    let waiter = tokio::spawn(async move { board::poll(&s2, &r2, seq0, 10).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    board::op(&dev, &rd, "own", &[json!({ "t": "put", "o": { "id": "p1", "t": "path", "pts": [0, 0, 10, 10] } })]).await.unwrap();
    let got = waiter.await.unwrap();
    assert_eq!(got["events"][0]["op"]["o"]["id"], "p1", "{}", got);
    assert_eq!(got["events"][0]["by"], "dev");
    // участник рисует — у владельца объект с автором по подписи
    board::op(&stand, &rs, "s1", &[json!({ "t": "put", "o": { "id": "n1", "t": "note", "text": "привет", "by": "кто-то" } }), json!({ "t": "cur", "x": 5, "y": 6 })]).await.unwrap();
    let snap = board::state(&dev, &rd).await.unwrap();
    let n1 = snap["objs"].as_array().unwrap().iter().find(|o| o["id"] == "n1").unwrap().clone();
    assert_eq!(n1["by"], "stand", "автор — по подписи");
    // картинка: через владельца, вторым узлом читается
    let png: Vec<u8> = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0u8; 64]].concat();
    let id = board::put_image(&stand, &rs, &png).await.unwrap();
    assert_eq!(board::get_image(&dev, &rd, &id).await.unwrap(), png);
    assert!(board::get_image(&dev, &rd, "../board.json").await.is_none());
    board::op(&dev, &rd, "own", &[json!({ "t": "put", "o": { "id": "i1", "t": "img", "src": id } })]).await.unwrap();
    // 3.0: фон и слои — общие, через владельца; неверные отклоняются
    board::op(&stand, &rs, "s1", &[json!({ "t": "bg", "c": "#F4F5FA", "l": 120 }), json!({ "t": "layers", "list": [{ "id": "base", "name": "Фон" }, { "id": "l2", "name": "Пометки" }] })]).await.unwrap();
    let snap = board::state(&dev, &rd).await.unwrap();
    assert_eq!(snap["bg"], json!({ "c": "#F4F5FA", "l": 120 }));
    assert_eq!(snap["layers"][1]["name"], "Пометки");
    assert_eq!(board::op(&stand, &rs, "s1", &[json!({ "t": "bg", "c": "red" })]).await.err().unwrap().0, 400);
    assert_eq!(board::op(&stand, &rs, "s1", &[json!({ "t": "layers", "list": [{ "id": "../x" }] })]).await.err().unwrap().0, 400);
    // стенд: сохранить у участника (с картинкой), очистить, загрузить обратно
    let sv = board::stand_save(&stand, &rs, "Макет").await.unwrap();
    assert_eq!(board::stands(&stand)["stands"][0]["objects"], 3);
    board::op(&dev, &rd, "own", &[json!({ "t": "clear" })]).await.unwrap();
    assert!(board::state(&dev, &rd).await.unwrap()["objs"].as_array().unwrap().is_empty());
    board::op(&dev, &rd, "own", &[json!({ "t": "bg", "c": "", "l": 100 }), json!({ "t": "layers", "list": [{ "id": "base", "name": "Слой 1" }] })]).await.unwrap();
    board::stand_load(&stand, &rs, sv["id"].as_str().unwrap(), "s1").await.unwrap();
    let back = board::state(&dev, &rd).await.unwrap();
    assert_eq!(back["objs"].as_array().unwrap().len(), 3);
    assert_eq!(back["bg"]["c"], "#F4F5FA", "стенд хранит фон");
    assert_eq!(back["layers"].as_array().unwrap().len(), 2, "стенд хранит слои");
    // файл стенда принимается обратно; чужой — нет
    let file = std::fs::read(board::stand_path(&stand, sv["id"].as_str().unwrap()).unwrap()).unwrap();
    assert!(board::stand_import(&dev, &file, "x").is_ok());
    assert!(board::stand_import(&dev, b"{}", "x").is_err());
    // только владелец правит
    board::op(&dev, &rd, "own", &[json!({ "t": "edit", "mode": "owner" })]).await.unwrap();
    assert_eq!(board::op(&stand, &rs, "s1", &[json!({ "t": "del", "ids": ["p1"] })]).await.err().unwrap().0, 403);
    assert_eq!(board::op(&stand, &rs, "s1", &[json!({ "t": "edit", "mode": "all" })]).await.err().unwrap().0, 403);
    board::op(&stand, &rs, "s1", &[json!({ "t": "cur", "x": 1 })]).await.expect("курсор можно всегда");
    // сохранилось на диск у владельца
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(dev.data_dir.join("boards").join("obshiy").join("board.json").exists());
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bridge_locked_while_controlled_and_ui_settings() {
    let mut net = pair();
    net.start().await;
    // настройки панели хранит мост
    let (c, _) = local(&net, "dev", "POST", "ui/settings", Some(json!({ "settings": { "theme": "light", "micGain": 1.5 } }))).await;
    assert_eq!(c, 200);
    let (_, v) = local(&net, "dev", "GET", "ui/settings", None).await;
    assert_eq!(v["settings"]["theme"], "light");
    // пока ПК управляют — мост заперт: права, соседи, папки не меняются, а разговор и чат работают
    bridge_node::remote::grant_for_test(net.n("dev"), "stand");
    let (c, v) = local(&net, "dev", "POST", "groups", Some(json!({ "id": "family", "title": "Семья", "members": ["stand"] }))).await;
    assert_eq!(c, 423, "{}", v);
    assert!(v["error"].as_str().unwrap_or("").contains("заперт"));
    let (c, _) = local(&net, "dev", "POST", "peers/invite?title=x", None).await;
    assert_eq!(c, 423, "приглашение (ключ) — нельзя");
    let (c, _) = local(&net, "dev", "POST", "folders", Some(json!({ "id": "x", "access": { "*": "write" } }))).await;
    assert_eq!(c, 423);
    let (c, _) = local(&net, "dev", "GET", "overview", None).await;
    assert_eq!(c, 200, "смотреть можно");
    let (c, _) = local(&net, "dev", "POST", "ui/settings", Some(json!({ "settings": { "theme": "dark" } }))).await;
    assert_eq!(c, 200, "громкость и вид — можно");
    let (c, _) = local(&net, "dev", "POST", "control/revoke", Some(json!({ "why": "проверка" }))).await;
    assert_eq!(c, 200, "остановить управление — всегда");
    let (c, _) = local(&net, "dev", "POST", "groups", Some(json!({ "id": "family", "title": "Семья", "members": ["stand"] }))).await;
    assert_eq!(c, 200, "после управления — снова можно");
    net.stop().await;
}

/// 3.1: доска для ассистента — понятные вызовы у участника идут через владельца канала; плитка участника.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn board_api_for_assistant_and_tiles() {
    let mut net = pair();
    net.configs.get_mut("dev").unwrap().channels.insert("obshiy".into(), bridge_core::config::ChannelConfig { title: "Общий".into(), members: vec!["*".into()] });
    net.start().await;
    net.sync(1, &["stand", "dev"]).await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    use bridge_node::board_api as ba_;
    // по названию канала; выключена — понятный ответ
    let e = ba_::add(&stand, &json!({ "room": "Общий", "objs": [{ "type": "text", "text": "x" }] })).await.err().unwrap();
    assert_eq!(e.0, 409, "{}", e.1);
    ba_::enable(&stand, &json!({ "room": "Общий", "on": true })).await.unwrap();
    // картинка с диска участника: размер по заголовку файла
    let dir = stand.data_dir.join("t");
    std::fs::create_dir_all(&dir).unwrap();
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&400u32.to_be_bytes());
    png.extend_from_slice(&200u32.to_be_bytes());
    png.extend_from_slice(&[0u8; 32]);
    std::fs::write(dir.join("p.png"), &png).unwrap();
    let r = ba_::add(&stand, &json!({ "room": "dev:obshiy", "at": [0, 0], "objs": [
        { "type": "text", "id": "t1", "text": "Заголовок", "size": 40, "font": "tech", "bold": true, "color": "cyan" },
        { "type": "note", "id": "n1", "text": "купить молоко", "font": "hand" },
        { "type": "image", "id": "i1", "path": dir.join("p.png").display().to_string(), "w": 200 },
        { "type": "line", "id": "l1", "x": 0, "y": 500, "x2": 100, "y2": 500 },
    ] })).await.unwrap();
    assert_eq!(r["objs"].as_array().unwrap().len(), 4, "{}", r);
    assert_eq!(r["objs"][2]["box"][3].as_f64().unwrap() - r["objs"][2]["box"][1].as_f64().unwrap(), 100.0, "пропорции картинки 400×200 при w 200");
    assert!(r["objs"][1]["box"][0].as_f64().unwrap() > r["objs"][0]["box"][2].as_f64().unwrap(), "без x — встают в ряд");
    // у владельца объект — с автором по подписи и стилем
    let rd = bridge_node::board::room(&dev, "dev:obshiy").unwrap();
    let snap = bridge_node::board::state(&dev, &rd).await.unwrap();
    let t1 = snap["objs"].as_array().unwrap().iter().find(|o| o["id"] == "t1").unwrap().clone();
    assert_eq!((t1["ff"].as_str(), t1["fb"].as_bool(), t1["c"].as_str(), t1["by"].as_str()), (Some("tech"), Some(true), Some("#38E1F0"), Some("stand")));
    // поиск, правка, сдвиг, слой, удаление — у владельца сразу
    let f = ba_::find(&dev, &json!({ "room": "obshiy", "q": "МОЛОКО" })).await.unwrap();
    assert_eq!(f["total"], 1);
    ba_::edit(&stand, &json!({ "room": "Общий", "id": "n1", "text": "купить хлеб", "italic": true, "align": "center" })).await.unwrap();
    ba_::move_(&stand, &json!({ "room": "Общий", "ids": ["i1"], "x": 1000, "y": 1000 })).await.unwrap();
    let g = ba_::get(&dev, &json!({ "room": "Общий", "ids": "n1,i1" })).await.unwrap();
    assert_eq!(g["objs"][0]["obj"]["fi"], true);
    assert_eq!(g["objs"][0]["obj"]["fa"], "center");
    assert_eq!(g["objs"][1]["box"][0], 1000.0);
    let l = ba_::layer(&stand, &json!({ "room": "Общий", "action": "add", "name": "Пометки" })).await.unwrap();
    assert_eq!(l["layers"].as_array().unwrap().len(), 2);
    ba_::edit(&stand, &json!({ "room": "Общий", "ids": ["l1"], "layer": "Пометки" })).await.unwrap();
    assert_eq!(ba_::find(&dev, &json!({ "room": "Общий", "layer": "Пометки" })).await.unwrap()["total"], 1);
    let rm = ba_::layer(&stand, &json!({ "room": "Общий", "action": "remove", "layer": "Пометки" })).await.unwrap();
    assert_eq!(rm["layers"].as_array().unwrap().len(), 1);
    assert_eq!(ba_::find(&dev, &json!({ "room": "Общий", "type": "path" })).await.unwrap()["total"], 0, "слой удалён вместе с рисунками");
    assert_eq!(ba_::add(&stand, &json!({ "room": "Общий", "objs": [{ "type": "text", "id": "t1" }] })).await.err().unwrap().0, 409);
    assert_eq!(ba_::add(&stand, &json!({ "room": "Общий", "objs": [{ "type": "image", "url": "http://x" }] })).await.err().unwrap().0, 400);
    assert_eq!(ba_::add(&stand, &json!({ "room": "Нет такого", "objs": [] })).await.err().unwrap().0, 404);
    ba_::bg(&stand, &json!({ "room": "Общий", "color": "white", "light": 90 })).await.unwrap();
    assert_eq!(bridge_node::board::state(&dev, &rd).await.unwrap()["bg"], json!({ "c": "#E8EAFB", "l": 90 }));
    ba_::remove(&stand, &json!({ "room": "Общий", "ids": ["t1"] })).await.unwrap();
    assert_eq!(ba_::describe(&dev, &json!({ "room": "Общий" })).await.unwrap()["objects"], 2);
    // плитка: стиль и картинка видны соседу после обмена
    bridge_node::tile::set_style(&dev, &json!({ "mask": "vignette", "opacity": 60, "glow": "#5DE08E", "evil": 1 })).unwrap();
    let gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;".to_vec();
    bridge_node::tile::set_img(&dev, &gif).unwrap();
    net.sync(1, &["stand", "dev"]).await;
    let t = bridge_node::tile::of_peer(&stand, "dev").await.expect("плитка соседа");
    assert_eq!(t["style"]["mask"], "vignette");
    assert!(t["style"].get("evil").is_none());
    assert_eq!(bridge_node::tile::img_of_peer(&stand, "dev").await.unwrap(), gif);
    assert!(bridge_node::tile::set_style(&dev, &json!({ "fit": "<b>" })).is_err());
}

/// 3.2: этапы и ожидание исполнителя; старый ход не перезаписывает статус нового поручения; запись-задача сама идёт
/// «принята → в работе → на проверке», «готово» ставит заказчик.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_phases_waiting_stale_and_record() {
    let mut net = agent_net("auto");
    net.start().await;
    net.sync(1, &["dev", "stand"]).await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    // задача для агента: исполнитель — ПК агента
    let r = net.act("dev", ba(json!({"action":"create","type":"task","fields":{"title":"Собрать отчёт","assignee":"stand","agent":"stand"}})));
    assert!(r.ok, "{}", r.error);
    let rid = r.record.clone().unwrap();
    net.sync(1, &[]).await;
    assert!(stand.share("s").unwrap().records.lock().get(&rid).is_some());
    let t = bridge_node::agent::ask_full(&dev, "stand", "этапы: собери отчёт", None, None, &json!({ "share": "s", "id": rid })).await.expect("поручено");
    // ожидание видно оркестратору
    let w = agent_wait_status(&net, "dev", &t.id, &["waiting"]).await;
    assert_eq!(w.phase, "сборка проекта");
    assert!(w.events.iter().any(|e| e.kind == "phase" && e.text == "ищу информацию"), "этап в ходе");
    let done = agent_wait_status(&net, "dev", &t.id, &["idle"]).await;
    assert!(done.phase.is_empty(), "после итога этапа нет");
    // запись: у исполнителя — «на проверке» с итогом; у заказчика — после обмена
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let st = stand.share("s").unwrap().records.lock().get(&rid).and_then(|r| r.status().map(String::from));
    assert_eq!(st.as_deref(), Some("review"), "агент сдал задачу на проверку");
    net.sync(1, &[]).await;
    assert_eq!(status(&net, "dev", &rid).as_deref(), Some("review"));
    assert!(net.act("dev", ba(json!({"action":"transition","record":rid,"to":"done"}))).ok, "закрывает заказчик");
    // продолжение: старый ход с «готово» (asks меньше) не делает новое поручение готовым
    let asks = dev.agent.get(&t.id).unwrap().asks;
    dev.agent.update(&t.id, |t| { t.status = "queued".into(); t.asks += 1; });
    bridge_node::agent::on_events(&dev, "stand", &json!({ "thread": t.id, "status": "idle", "result": "старый итог", "asks": asks, "events": [] })).unwrap();
    let t3 = dev.agent.get(&t.id).unwrap();
    assert_eq!(t3.status, "queued", "старый ход не перезаписал статус");
    assert_ne!(t3.result, "старый итог");
    // фразы этапов
    assert_eq!(bridge_node::agent::phase_of("Этап: сверяю данные"), Some(("сверяю данные".into(), false)));
    assert_eq!(bridge_node::agent::phase_of("текст\n**Ожидание:** ответ сервиса"), Some(("ответ сервиса".into(), true)));
    assert_eq!(bridge_node::agent::phase_of("просто текст"), None);
    net.stop().await;
}

/// 3.2: ручной режим — этап и ожидание через agent_progress.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_manual_phase_and_waiting() {
    let mut net = agent_net("manual");
    net.start().await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    let t = bridge_node::agent::ask(&dev, "stand", "проверь", None, None).await.unwrap();
    bridge_node::agent::take(&stand, &t.id).unwrap();
    bridge_node::agent::progress_full(&stand, &t.id, "", "жду сборку", Some(true)).unwrap();
    let w = agent_wait_status(&net, "dev", &t.id, &["waiting"]).await;
    assert_eq!(w.phase, "жду сборку");
    bridge_node::agent::progress_full(&stand, &t.id, "собрано", "проверяю", Some(false)).unwrap();
    let r = agent_wait_status(&net, "dev", &t.id, &["running"]).await;
    assert_eq!(r.phase, "проверяю");
    bridge_node::agent::report(&stand, &t.id, "ок", true).unwrap();
    agent_wait_status(&net, "dev", &t.id, &["idle"]).await;
    net.stop().await;
}

/// 3.3: поручение этому же ПК — без соседа; продолжение в той же сессии; запись-задача ведётся; выключенный агент отказывает.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_local_self_task() {
    let mut net = agent_net("auto");
    net.start().await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    let e = bridge_node::agent::ask(&dev, "", "что-нибудь", None, None).await.unwrap_err();
    assert!(e.1.contains("выключен"), "{}", e.1);
    let r = net.act("stand", ba(json!({"action":"create","type":"task","fields":{"title":"Своя задача","assignee":"stand","agent":"stand"}})));
    assert!(r.ok, "{}", r.error);
    let rid = r.record.clone().unwrap();
    let t = bridge_node::agent::ask_full(&stand, "local", "прогони тесты базиса", None, None, &json!({ "share": "s", "id": rid })).await.expect("поручено себе");
    assert!(t.local && t.role == "in");
    let t = agent_wait_status(&net, "stand", &t.id, &["idle", "failed"]).await;
    assert_eq!(t.status, "idle", "{}", t.error);
    assert!(t.result.contains("Готово: прогони тесты базиса"), "{}", t.result);
    let session = t.session.clone();
    assert!(!session.is_empty());
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert_eq!(status(&net, "stand", &rid).as_deref(), Some("review"), "задачу ведёт мост и при поручении себе");
    // продолжение: по имени узла тоже «себе»
    bridge_node::agent::ask(&stand, "stand", "теперь только упавшие", Some(&t.id), None).await.expect("продолжено");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let t2 = agent_wait_status(&net, "stand", &t.id, &["idle", "failed"]).await;
    assert_eq!(t2.session, session, "сессия та же");
    assert!(t2.events.iter().any(|e| e.text.contains("Продолжаю")), "{:?}", t2.events.iter().map(|e| &e.text).collect::<Vec<_>>());
    // чужую нить «себе» не продолжить
    let e = bridge_node::agent::ask(&stand, "", "x", Some("нет-такой"), None).await.unwrap_err();
    assert_eq!(e.0, 404);
    net.stop().await;
}

/// 3.3: диаграммы для ассистента — фигуры, связи по id (из того же вызова), стороны, концы, сдвиг, удаление со связями.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn board_api_diagrams() {
    let mut net = pair();
    net.configs.get_mut("dev").unwrap().channels.insert("obshiy".into(), bridge_core::config::ChannelConfig { title: "Общий".into(), members: vec!["*".into()] });
    net.start().await;
    net.sync(1, &["stand", "dev"]).await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    use bridge_node::board_api as ba_;
    ba_::enable(&stand, &json!({ "room": "Общий", "on": true })).await.unwrap();
    let r = ba_::add(&stand, &json!({ "room": "Общий", "objs": [
        { "type": "shape", "id": "s1", "kind": "terminal", "text": "Начало", "x": 0, "y": 0 },
        { "type": "shape", "id": "s2", "kind": "decision", "text": "Решение", "x": 300, "y": 0, "dash": "dash" },
        { "type": "frame", "id": "f1", "text": "Группа", "x": -40, "y": -80 },
        { "type": "link", "id": "k1", "from": "s1", "to": "s2", "fromSide": "right", "label": "да", "end": "many", "kind": "elbow" },
        { "type": "link", "id": "k2", "from": "s2", "to": [900, 40] },
    ] })).await.unwrap();
    let objs = r["objs"].as_array().unwrap();
    assert_eq!(objs.len(), 5, "{}", r);
    assert_eq!((objs[1]["kind"].as_str(), objs[2]["kind"].as_str()), (Some("diamond"), Some("frame")));
    assert_eq!((objs[3]["from"].as_str(), objs[3]["to"].as_str(), objs[3]["fromSide"].as_str(), objs[3]["end"].as_str(), objs[3]["kind"].as_str()), (Some("s1"), Some("s2"), Some("right"), Some("many"), Some("elbow")));
    assert_eq!(objs[4]["to"], json!([900.0, 40.0]));
    let rd = bridge_node::board::room(&dev, "dev:obshiy").unwrap();
    let snap = bridge_node::board::state(&dev, &rd).await.unwrap();
    let get = |id: &str| snap["objs"].as_array().unwrap().iter().find(|o| o["id"] == id).unwrap().clone();
    assert_eq!((get("s1")["w"].as_f64(), get("s1")["h"].as_f64()), (Some(160.0), Some(64.0)), "размер по виду фигуры");
    assert_eq!((get("s2")["sd"].as_str(), get("k1")["a"]["s"].as_str(), get("k1")["text"].as_str(), get("k1")["ha"].as_str()), (Some("dash"), Some("r"), Some("да"), Some("none")));
    assert!(ba_::add(&stand, &json!({ "room": "Общий", "objs": [{ "type": "link", "from": "нет", "to": "s1" }] })).await.is_err());
    assert_eq!(ba_::add(&stand, &json!({ "room": "Общий", "objs": [{ "type": "shape", "kind": "квадрат" }] })).await.err().unwrap().0, 400);
    // связь видна в поиске, у фигуры сдвинулась — координаты конца новые
    ba_::move_(&stand, &json!({ "room": "Общий", "ids": ["s2"], "dx": 0, "dy": 400 })).await.unwrap();
    let f = ba_::find(&dev, &json!({ "room": "Общий", "type": "link" })).await.unwrap();
    assert_eq!(f["total"], 2);
    let g = ba_::get(&dev, &json!({ "room": "Общий", "ids": "k1" })).await.unwrap();
    assert!(g["objs"][0]["obj"]["b"]["y"].as_f64().unwrap() > 400.0, "конец связи — у сдвинутой фигуры");
    // правка конца и вида
    ba_::edit(&stand, &json!({ "room": "Общий", "id": "k2", "to": "s1", "toSide": "bottom", "kind": "curve", "start": "circle" })).await.unwrap();
    let k2 = ba_::get(&dev, &json!({ "room": "Общий", "ids": "k2" })).await.unwrap()["objs"][0]["obj"].clone();
    assert_eq!((k2["b"]["id"].as_str(), k2["b"]["s"].as_str(), k2["k"].as_str(), k2["ha"].as_str()), (Some("s1"), Some("b"), Some("curve"), Some("circle")));
    // удалили фигуру — ушли и её связи
    let rm = ba_::remove(&stand, &json!({ "room": "Общий", "ids": ["s2"] })).await.unwrap();
    assert_eq!(rm["removed"].as_array().unwrap().len(), 3, "{}", rm);
    assert_eq!(ba_::describe(&dev, &json!({ "room": "Общий" })).await.unwrap()["objects"], 2);
    net.stop().await;
}

/// 3.3.1: статус участника — текст (до 255 знаков) и значок доходят до соседа; смена стиля плитки статус не стирает.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tile_status_reaches_peer() {
    let mut net = pair();
    net.start().await;
    net.sync(1, &["stand", "dev"]).await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    let long: String = "я".repeat(400);
    let r = bridge_node::tile::set_status(&dev, &json!({ "text": format!("на созвоне\n{}", long), "color": "#5de08e" })).unwrap();
    assert_eq!(r["text"].as_str().unwrap().chars().count(), 255, "до 255 знаков");
    assert!(!r["text"].as_str().unwrap().contains('\n'));
    assert!(bridge_node::tile::set_status(&dev, &json!({ "color": "green" })).is_err());
    bridge_node::tile::set_style(&dev, &json!({ "mask": "top" })).unwrap();
    let gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;".to_vec();
    bridge_node::tile::set_st_img(&dev, &gif).unwrap();
    net.sync(1, &["stand", "dev"]).await;
    let t = bridge_node::tile::of_peer(&stand, "dev").await.expect("плитка соседа");
    assert_eq!(t["style"]["stColor"], "#5DE08E");
    assert_eq!(t["style"]["mask"], "top");
    assert!(t["style"]["stText"].as_str().unwrap().starts_with("на созвоне"), "стиль не стёр статус");
    assert_eq!(bridge_node::tile::st_img_of_peer(&stand, "dev").await.unwrap(), gif);
    bridge_node::tile::set_status(&dev, &json!({ "text": "", "color": "" })).unwrap();
    bridge_node::tile::remove_st_img(&dev);
    net.sync(1, &["stand", "dev"]).await;
    let t = bridge_node::tile::of_peer(&stand, "dev").await.expect("плитка");
    assert!(t["style"].get("stText").is_none() && t["stImg"] == "");
    net.stop().await;
}

/// 3.3.1: агент спрашивает людей — вопрос виден заказчику, его ответ уходит исполнителю; кнопки проверяются.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_asks_people_and_gets_answer() {
    let mut net = agent_net("auto");
    net.start().await;
    net.sync(1, &["dev", "stand"]).await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    let t = bridge_node::agent::ask(&dev, "stand", "пауза: дождись Урбана", None, None).await.expect("поручено");
    agent_wait_status(&net, "stand", &t.id, &["running"]).await;
    let q = bridge_node::agent::request(&stand, &t.id, "Урбан закончил сборку модели?", &["Готово".into(), "Ещё 10 минут".into()], false).expect("вопрос");
    assert_eq!(stand.agent.get(&t.id).unwrap().status, "waiting");
    // заказчик видит вопрос
    for _ in 0..60 {
        if dev.agent.get(&t.id).map(|x| x.requests.iter().any(|r| r.id == q.id)).unwrap_or(false) { break; }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(dev.agent.get(&t.id).unwrap().requests.iter().any(|r| r.q.contains("сборку")), "вопрос дошёл до заказчика");
    assert_eq!(bridge_node::agent::answer(&stand, &t.id, &q.id, "что-то другое", "stand").await.err().unwrap().0, 400, "только кнопки");
    let w = { let s2 = stand.clone(); let (id, rq) = (t.id.clone(), q.id.clone()); tokio::spawn(async move { bridge_node::agent::wait_answer(&s2, &id, &rq, std::time::Duration::from_secs(10)).await }) };
    bridge_node::agent::answer(&dev, &t.id, &q.id, "Готово", "dev").await.expect("ответ заказчика");
    let got = w.await.unwrap().expect("агент получил ответ");
    assert_eq!((got.answer.as_str(), got.by.as_str()), ("Готово", "dev"));
    assert_eq!(bridge_node::agent::answer(&stand, &t.id, &q.id, "Готово", "stand").await.err().unwrap().0, 409, "второй ответ — уже ответили");
    let done = agent_wait_status(&net, "dev", &t.id, &["idle"]).await;
    assert!(done.result.contains(&format!("[нить {}]", t.id)), "мост передал Claude id нити: {}", done.result);
    assert!(done.requests.iter().any(|r| r.answer == "Готово"));
    net.stop().await;
}

/// 3.3.1: сборки — первая отправка целиком, патч несёт только изменённое, чистая заменяет папку (старая — в корзину).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builds_patch_and_clean() {
    let mut net = pair();
    net.start().await;
    net.sync(1, &["stand", "dev"]).await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    let src = net.root.join("dev-build");
    std::fs::create_dir_all(src.join("Game_Data/Managed")).unwrap();
    std::fs::write(src.join("Game.exe"), vec![1u8; 200_000]).unwrap();
    std::fs::write(src.join("Game_Data/Managed/Assembly.dll"), b"v1").unwrap();
    std::fs::write(src.join("Game_Data/level0"), b"lvl").unwrap();
    let wait = |n: &str| {
        let n = n.to_string();
        let dev = dev.clone();
        async move {
            for _ in 0..100 {
                let h = bridge_node::builds::status(&dev);
                if dev.builds.jobs.lock().is_empty() && h["history"].as_array().map(|a| a.len()).unwrap_or(0) >= n.parse::<usize>().unwrap() { return; }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            panic!("сборка {} не упаковалась: {}", n, bridge_node::builds::status(&dev));
        }
    };
    let ready = |p: std::path::PathBuf| async move {
        for _ in 0..100 { if p.exists() { return true; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
        false
    };
    let target = stand.base_dir.join("сборки").join("CORA");
    // первая — целиком, хоть и «патч»
    bridge_node::builds::send(&dev, "stand", src.to_str().unwrap(), "CORA", "patch").unwrap();
    wait("1").await;
    net.sync(1, &["stand"]).await;
    assert!(ready(target.join("Game_Data/Managed/Assembly.dll")).await, "распакована: {}", bridge_node::builds::status(&stand));
    assert_eq!(std::fs::metadata(target.join("Game.exe")).unwrap().len(), 200_000);
    // патч: только изменённый файл
    std::fs::write(src.join("Game_Data/Managed/Assembly.dll"), b"v2").unwrap();
    std::fs::write(target.join("local-note.txt"), b"my").unwrap();
    bridge_node::builds::send(&dev, "stand", src.to_str().unwrap(), "CORA", "patch").unwrap();
    wait("2").await;
    let st = bridge_node::builds::status(&dev);
    assert_eq!(st["history"][0]["files"], 1, "в патче один файл: {}", st);
    net.sync(1, &["stand"]).await;
    for _ in 0..100 { if std::fs::read(target.join("Game_Data/Managed/Assembly.dll")).unwrap() == b"v2" { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
    assert_eq!(std::fs::read(target.join("Game_Data/Managed/Assembly.dll")).unwrap(), b"v2");
    assert!(target.join("local-note.txt").exists(), "патч не трогает остальное");
    // пустой патч — понятная ошибка
    bridge_node::builds::send(&dev, "stand", src.to_str().unwrap(), "CORA", "patch").unwrap();
    for _ in 0..100 { if dev.builds.jobs.lock().is_empty() { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
    assert!(bridge_node::builds::status(&dev)["history"][0]["error"].as_str().unwrap_or("").contains("ничего не изменилось"));
    // чистая: лишнее уходит (в корзину), остаётся ровно присланное
    bridge_node::builds::send(&dev, "stand", src.to_str().unwrap(), "CORA", "clean").unwrap();
    wait("4").await;
    net.sync(1, &["stand"]).await;
    for _ in 0..100 { if !target.join("local-note.txt").exists() && target.join("Game.exe").exists() { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
    assert!(!target.join("local-note.txt").exists(), "чистая — без лишнего");
    assert!(target.join("Game.exe").exists());
    let h = bridge_node::builds::status(&stand);
    let old = h["history"][0]["old"].as_str().unwrap_or("").to_string();
    assert!(std::path::Path::new(&old).join("local-note.txt").exists(), "прежняя папка — в корзине: {}", h);
    assert!(!stand.mailbox.in_root(&stand).join("dev").join(bridge_node::builds::DIR).join("CORA").exists(), "архивы убраны");
    assert!(bridge_node::builds::send(&dev, "stand", src.to_str().unwrap(), "../x", "clean").is_err());
    net.stop().await;
}

/// 3.3.5: заготовки сборок и игнор — кэш не пакуется, «!» возвращает исключённое.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn build_presets_ignore() {
    let mut net = pair();
    net.start().await;
    net.sync(1, &["stand", "dev"]).await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    let src = net.root.join("dev-build2");
    for d in ["Game_Data/Managed", "Game_BurstDebugInformation_DoNotShip/x", "Cache/a/b"] { std::fs::create_dir_all(src.join(d)).unwrap(); }
    std::fs::write(src.join("Game.exe"), b"exe").unwrap();
    std::fs::write(src.join("Game_Data/Managed/A.dll"), b"a").unwrap();
    std::fs::write(src.join("Game_Data/Managed/A.pdb"), b"pdb").unwrap();
    std::fs::write(src.join("Game_Data/Managed/Keep.pdb"), b"keep").unwrap();
    std::fs::write(src.join("Game_BurstDebugInformation_DoNotShip/x/big.bin"), vec![0u8; 5000]).unwrap();
    std::fs::write(src.join("Cache/a/b/c.dat"), b"c").unwrap();
    std::fs::write(src.join("run.sh"), "#!/bin/sh\necho запущено > ran.txt\nsleep 3\n").unwrap();
    let ign = json!(["# кэш", "Cache/", "*_BurstDebugInformation_DoNotShip/", "**/*.pdb", "!Keep.pdb"]);
    assert!(bridge_node::builds::set_presets(&dev, &json!([{ "id": "x", "name": "X", "path": "", "peers": [], "exe": "../evil.exe" }])).is_err(), "программа — только внутри сборки");
    assert!(bridge_node::builds::set_presets(&dev, &json!([{ "id": "bad id", "name": "X", "path": "", "peers": [] }])).is_err());
    let l = bridge_node::builds::set_presets(&dev, &json!([{ "id": "cora", "name": "CORA2", "path": src.to_str().unwrap(), "peers": ["STAND"], "mode": "patch", "ignore": ign, "exe": "run.sh" }])).unwrap();
    assert_eq!(l[0]["exe"], "run.sh");
    assert_eq!(l[0]["peers"], json!(["stand"]), "имя соседа — как в настройках");
    let pv = bridge_node::builds::preview(&dev, src.to_str().unwrap(), &ign.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect::<Vec<_>>()).unwrap();
    assert_eq!(pv["files"], 4, "войдут exe, sh, dll и Keep.pdb: {}", pv);
    assert_eq!(pv["ignoredFiles"], 3, "{}", pv);
    assert_eq!(pv["exes"], json!(["Game.exe", "run.sh"]), "подсказка, что запускать: {}", pv);
    assert_eq!(pv["rules"][0]["rule"], "*_BurstDebugInformation_DoNotShip/", "крупнейшее правило первым: {}", pv);
    let r = bridge_node::builds::send_preset(&dev, "cora", None, "  починил сохранения  ").unwrap();
    assert_eq!(r["started"], json!(["stand"]));
    for _ in 0..100 { if dev.builds.jobs.lock().is_empty() && !bridge_node::builds::status(&dev)["history"].as_array().unwrap().is_empty() { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
    let h = bridge_node::builds::status(&dev)["history"][0].clone();
    assert_eq!(h["files"], 4, "{}", h);
    assert_eq!(h["skipped"], 3);
    assert_eq!(h["preset"], "cora");
    assert_eq!((h["n"].clone(), h["note"].clone()), (json!(1), json!("починил сохранения")));
    net.sync(1, &["stand"]).await;
    let target = stand.base_dir.join("сборки").join("CORA2");
    for _ in 0..100 { if target.join("Game.exe").exists() { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
    assert!(target.join("Game_Data/Managed/Keep.pdb").exists());
    assert!(!target.join("Game_Data/Managed/A.pdb").exists());
    assert!(!target.join("Cache").exists() && !target.join("Game_BurstDebugInformation_DoNotShip").exists());
    assert_eq!(bridge_node::builds::presets(&dev).len(), 1);
    // (3.8) у получателя: версия, что нового, что запускать — и запуск
    let mut got = json!(null);
    for _ in 0..100 { got = bridge_node::builds::status(&stand)["received"][0].clone(); if got["exe"] == "run.sh" { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
    assert_eq!((got["n"].clone(), got["note"].clone(), got["exe"].clone(), got["exeExists"].clone()), (json!(1), json!("починил сохранения"), json!("run.sh"), json!(true)), "{}", got);
    assert!(bridge_node::builds::run(&stand, "dev", "Нет такой").is_err());
    #[cfg(unix)]
    {
        bridge_node::builds::run(&stand, "dev", "CORA2").unwrap();
        for _ in 0..50 { if target.join("ran.txt").exists() { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
        assert!(target.join("ran.txt").exists(), "программа сборки запустилась в её папке");
        assert!(bridge_node::builds::running(&stand, "dev", "CORA2"));
        assert_eq!(bridge_node::builds::run(&stand, "dev", "CORA2").unwrap()["already"], true);
        bridge_node::builds::stop(&stand, "dev", "CORA2").unwrap();
        assert!(!bridge_node::builds::running(&stand, "dev", "CORA2"));
    }
    // вторая версия — №2, история у получателя
    std::fs::write(src.join("Game_Data/Managed/A.dll"), b"a2").unwrap();
    bridge_node::builds::send_preset(&dev, "cora", None, "второй патч").unwrap();
    for _ in 0..100 { if dev.builds.jobs.lock().is_empty() && bridge_node::builds::status(&dev)["history"][0]["n"] == 2 { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
    net.sync(1, &["stand"]).await;
    for _ in 0..100 { got = bridge_node::builds::status(&stand)["received"][0].clone(); if got["n"] == 2 { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
    assert_eq!((got["n"].clone(), got["note"].clone(), got["mode"].clone()), (json!(2), json!("второй патч"), json!("patch")), "{}", got);
    assert_eq!(got["history"].as_array().unwrap().len(), 2);
    assert_eq!(got["history"][1]["note"], "починил сохранения");
    net.stop().await;
}

/// 3.4: приложения — подхват папки и пакета, согласие по отпечатку, хранилище, кнопки, упаковка.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apps_folder_consent_actions() {
    use bridge_node::apps;
    let mut net = pair();
    net.start().await;
    let dev = net.n("dev").clone();
    let dir = apps::folder(&dev).join("hello");
    std::fs::create_dir_all(dir.join("ui")).unwrap();
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::write(dir.join("ui/index.html"), "<b>hi</b>").unwrap();
    std::fs::write(dir.join("app.json"), r#"{"id":"hello","name":"Привет","version":"1.0","ui":"ui/index.html","permissions":["notify"],"actions":[{"id":"hi","title":"Сказать","run":"bin/hi.sh {who}"}]}"#).unwrap();
    std::fs::write(dir.join("bin/hi.sh"), "#!/bin/sh\necho \"привет, $1\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("bin/hi.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // сломанное приложение — в ошибках, не в списке
    std::fs::create_dir_all(apps::folder(&dev).join("bad")).unwrap();
    std::fs::write(apps::folder(&dev).join("bad/app.json"), r#"{"id":"Bad!","name":"x"}"#).unwrap();
    assert!(apps::scan(&dev));
    let st = apps::status(&dev);
    assert_eq!(st["list"].as_array().unwrap().len(), 1, "{}", st);
    assert_eq!(st["list"][0]["state"], "new");
    assert!(st["errors"][0]["error"].as_str().unwrap().contains("id"), "{}", st);
    // без согласия — ни кнопок, ни интерфейса
    assert!(apps::action(&dev, "hello", "hi", &serde_json::json!({"who":"мир"})).await.is_err());
    apps::allow(&dev, "hello", None).unwrap();
    let a = apps::get(&dev, "hello").unwrap();
    assert!(apps::consent_ok(&dev, &a));
    assert_eq!(apps::granted(&dev, &a), vec!["notify".to_string()]);
    #[cfg(unix)]
    {
        let r = apps::action(&dev, "hello", "hi", &serde_json::json!({"who":"мир; rm -rf /"})).await.unwrap();
        assert_eq!(r["out"].as_str().unwrap().trim(), "привет, мир; rm -rf /", "параметр — один аргумент, без оболочки: {}", r);
    }
    // правка интерфейса не требует нового согласия, правка программы — требует
    std::fs::write(dir.join("ui/index.html"), "<b>hi 2</b>").unwrap();
    apps::scan(&dev);
    assert!(apps::consent_ok(&dev, &apps::get(&dev, "hello").unwrap()), "интерфейс поменялся — согласие то же");
    std::fs::write(dir.join("bin/hi.sh"), "#!/bin/sh\necho changed\n").unwrap();
    apps::scan(&dev);
    assert_eq!(apps::status(&dev)["list"][0]["state"], "changed", "программа поменялась — спросить снова");
    // хранилище
    apps::store_set(&dev, "hello", "k", serde_json::json!({"a":1})).unwrap();
    assert_eq!(apps::store_get(&dev, "hello", "k")["a"], 1);
    assert!(apps::store_set(&dev, "hello", "big", serde_json::json!("x".repeat(1_100_000))).is_err());
    // файлы интерфейса — только из ui/
    assert!(apps::ui_file(&dev, "hello", "index.html").is_some());
    assert!(apps::ui_file(&dev, "hello", "../bin/hi.sh").is_none());
    // упаковка и пакет в папке
    let src = net.root.join("pk/hello2");
    std::fs::create_dir_all(src.join("ui")).unwrap();
    std::fs::write(src.join("ui/index.html"), "x").unwrap();
    std::fs::write(src.join("app.json"), r#"{"id":"hello2","name":"Пакет","version":"2.0","ui":"ui/index.html"}"#).unwrap();
    let (pkg, n) = apps::pack(&src, None).unwrap();
    assert_eq!(n, 2);
    let tmp = dev.tmp_file();
    std::fs::copy(&pkg, &tmp).unwrap();
    apps::import(&dev, "hello2-2.0.most-app", &tmp).unwrap();
    apps::scan(&dev);
    let st = apps::status(&dev);
    let p2 = st["list"].as_array().unwrap().iter().find(|x| x["id"] == "hello2").cloned().unwrap();
    assert_eq!(p2["packaged"], true);
    assert!(apps::ui_file(&dev, "hello2", "index.html").is_some(), "пакет распакован");
    apps::remove(&dev, "hello2").unwrap();
    apps::scan(&dev);
    assert!(apps::get(&dev, "hello2").is_none());
    assert!(dev.folders.trash_dir().join("приложения").read_dir().unwrap().count() >= 1, "убрано в корзину");
    net.stop().await;
}

/// 3.5: сеансы приложений — два моста в одном канале: войти или создать, данные звездой через хоста,
/// общее состояние, присутствие, «последнее по ключу», выход при уходе из канала, конец сеанса с уходом хоста.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn app_sessions_two_bridges() {
    use bridge_node::{apps, net as anet};
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as M;
    let mut net = pair();
    net.start().await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    for n in [&dev, &stand] {
        let d = apps::folder(n).join("mice");
        std::fs::create_dir_all(d.join("ui")).unwrap();
        std::fs::write(d.join("ui/index.html"), "x").unwrap();
        std::fs::write(d.join("app.json"), r#"{"id":"mice","name":"Мыши","version":"1","ui":"ui/index.html","net":{"proto":1,"max":4},"permissions":["sessions","activity","identity"]}"#).unwrap();
        apps::scan(n);
        apps::allow(n, "mice", None).unwrap();
    }
    anet::set_channel(&dev, "dev:obshiy", "Общий", vec!["stand".into()]);
    anet::set_channel(&stand, "dev:obshiy", "Общий", vec!["dev".into()]);
    type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
    async fn connect(n: &std::sync::Arc<bridge_node::Node>) -> Ws {
        let t = apps::issue_token(n, "mice");
        let url = format!("{}/bridge/v1/app/net?token={}", n.urls.read()[0].replacen("http://", "ws://", 1), t);
        tokio_tungstenite::connect_async(url).await.unwrap().0
    }
    async fn op(ws: &mut Ws, v: serde_json::Value) -> serde_json::Value {
        let re = rand::random::<u32>();
        let mut v = v;
        v["re"] = serde_json::json!(re);
        ws.send(M::Text(v.to_string())).await.unwrap();
        loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await.expect("ответ").unwrap().unwrap();
            if let M::Text(t) = m {
                let e: serde_json::Value = serde_json::from_str(&t).unwrap();
                if e["ev"] == "re" && e["re"] == re {
                    assert_eq!(e["ok"], true, "{}", e);
                    return e["v"].clone();
                }
            }
        }
    }
    /// Ждать текстовое событие (пропуская прочее) или двоичный кадр.
    async fn wait_ev(ws: &mut Ws, ev: &str) -> serde_json::Value {
        loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await.unwrap_or_else(|_| panic!("нет события {}", ev)).unwrap().unwrap();
            if let M::Text(t) = m {
                let e: serde_json::Value = serde_json::from_str(&t).unwrap();
                if e["ev"] == ev {
                    return e;
                }
            }
        }
    }
    async fn wait_bin(ws: &mut Ws) -> Vec<u8> {
        loop {
            let m = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await.expect("кадр").unwrap().unwrap();
            if let M::Binary(b) = m {
                return b;
            }
        }
    }
    let mut a = connect(&dev).await;
    let mut b = connect(&stand).await;
    let ja = op(&mut a, serde_json::json!({ "op": "joinOrCreate" })).await;
    assert_eq!(ja["you"], 0, "первый — хост: {}", ja);
    assert_eq!(ja["isHost"], true);
    // анонс доходит до соседа по каналу
    let mut found = false;
    for _ in 0..60 {
        let l = op(&mut b, serde_json::json!({ "op": "list" })).await;
        if l.as_array().map(|x| x.iter().any(|s| s["sid"] == ja["sid"] && s["joinable"] == true)).unwrap_or(false) {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(found, "сеанс хоста виден соседу в том же канале");
    let jb = op(&mut b, serde_json::json!({ "op": "joinOrCreate" })).await;
    assert_eq!(jb["sid"], ja["sid"], "вошёл в идущий сеанс, а не создал свой");
    assert_eq!(jb["you"], 1);
    assert_eq!(jb["members"].as_array().unwrap().len(), 2);
    let mj = wait_ev(&mut a, "member").await;
    assert_eq!(mj["op"], "join");
    // ключ участника для издателя (право identity) — у хоста в событии и у вошедшего в снимке
    let kb = bridge_node::identity::get(&stand, "mice").unwrap()["key"].clone();
    let ka = bridge_node::identity::get(&dev, "mice").unwrap()["key"].clone();
    assert_eq!(mj["m"]["key"], kb, "{}", mj);
    assert_eq!(jb["members"][0]["key"], ka, "{}", jb);
    assert_ne!(ka, kb);
    // данные: клиент → хосту (надёжно)
    let mut f = vec![0u8, 0, 0];
    f.extend_from_slice(b"hi");
    b.send(M::Binary(f)).await.unwrap();
    let got = wait_bin(&mut a).await;
    assert_eq!(got, vec![0, 1, 0, b'h', b'i'], "хост получил [надёжно][от 1]hi");
    // хост → всем кроме себя (быстро)
    a.send(M::Binary(vec![1, 0xFE, 0xFF, 7, 7])).await.unwrap();
    assert_eq!(wait_bin(&mut b).await, vec![1, 0, 0, 7, 7]);
    // последнее по ключу: при заторе доходит хотя бы последнее
    for i in 0..50u8 {
        a.send(M::Binary(vec![2, 1, 0, 9, i])).await.unwrap();
    }
    let mut last = 0;
    loop {
        let fr = wait_bin(&mut b).await;
        assert_eq!(&fr[..3], &[2, 0, 0]);
        last = fr[3];
        if last == 49 {
            break;
        }
    }
    assert_eq!(last, 49);
    // общее состояние: клиент пишет — все видят
    op(&mut b, serde_json::json!({ "op": "state", "ops": [{ "op": "set", "path": "score.urban", "value": 3 }] })).await;
    let st = wait_ev(&mut a, "state").await;
    assert_eq!(st["ops"][0]["value"], 3);
    assert_eq!(st["by"], 1);
    // присутствие
    op(&mut a, serde_json::json!({ "op": "presence", "data": { "x": 0.5, "y": 0.25 } })).await;
    let pr = wait_ev(&mut b, "presence").await;
    assert_eq!(pr["cid"], 0);
    assert_eq!(pr["data"]["x"], 0.5);
    // часы и задержки
    let ck = wait_ev(&mut b, "clock").await;
    assert!(ck["rtt"].as_u64().is_some());
    // активность — живьём тем, у кого право
    bridge_node::activity::record(&stand, "voice.minute", serde_json::json!({ "talkSec": 12, "people": 2 }));
    let act = wait_ev(&mut b, "activity").await;
    assert_eq!(act["e"]["kind"], "voice.minute");
    let since = op(&mut b, serde_json::json!({ "op": "activity.since", "cursor": 0 })).await;
    assert!(since["events"].as_array().unwrap().iter().any(|e| e["talkSec"] == 12));
    // ушёл из канала — вышел из сеанса
    anet::set_channel(&stand, "", "", vec![]);
    let ml = wait_ev(&mut a, "member").await;
    assert_eq!(ml["op"], "leave");
    // войти снова и проверить конец сеанса при уходе хоста
    anet::set_channel(&stand, "dev:obshiy", "Общий", vec!["dev".into()]);
    tokio::time::sleep(std::time::Duration::from_millis(3500)).await;
    let jb2 = op(&mut b, serde_json::json!({ "op": "join", "sid": ja["sid"] })).await;
    assert_eq!(jb2["sid"], ja["sid"]);
    assert_eq!(jb2["state"]["score"]["urban"], 3, "новичок получил общее состояние");
    a.close(None).await.unwrap();
    let end = wait_ev(&mut b, "ended").await;
    assert!(end["why"].as_str().unwrap().contains("хост"), "{}", end);
    net.stop().await;
}

/// 3.3.1: план синхронизации папки соседа — что скачается, что уйдёт хозяину, объём.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_folder_plan() {
    let mut net = folder_net();
    net.start().await;
    put(&own(&net, "obshee", "a.txt"), "хозяин");
    net.sync(1, &["stand"]).await;
    let stand = net.n("stand").clone();
    let p = stand.folders.plan(&stand, "dev", "obshee").await.unwrap();
    assert_eq!(p["total"], 0, "всё сошлось: {}", p);
    put(&own(&net, "obshee", "новое/b.bin"), &"x".repeat(5000));
    put(&mnt(&net, "obshee", "моё.txt"), "сосед");
    let p = stand.folders.plan(&stand, "dev", "obshee").await.unwrap();
    let acts: Vec<(String, String)> = p["items"].as_array().unwrap().iter().map(|i| (i["path"].as_str().unwrap().to_string(), i["act"].as_str().unwrap().to_string())).collect();
    assert!(acts.contains(&("новое/b.bin".into(), "download".into())), "{:?}", acts);
    assert!(acts.contains(&("моё.txt".into(), "upload".into())), "{:?}", acts);
    assert_eq!(p["down"], 5000);
    net.sync(1, &["stand"]).await;
    let p = stand.folders.plan(&stand, "dev", "obshee").await.unwrap();
    assert_eq!(p["total"], 0, "после обмена — пусто: {}", p);
    assert_eq!(stand.folders.plan(&stand, "dev", "нет").await.err().unwrap().0, 404);
    net.stop().await;
}

/// 3.3.2: заготовки статуса — выбранная уходит соседу (текст, цвет, значок); «без статуса» всё убирает; стиль плитки статус не трогает.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tile_status_presets() {
    let mut net = pair();
    net.start().await;
    net.sync(1, &["stand", "dev"]).await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    use bridge_node::tile;
    tile::set_presets(&dev, &json!({ "list": [{ "id": "s1", "text": "на созвоне", "color": "#ff4f64" }, { "id": "s2", "text": "собираю билд", "color": "" }] })).unwrap();
    let gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;".to_vec();
    tile::set_preset_img(&dev, "s2", &gif).unwrap();
    assert!(tile::set_presets(&dev, &json!({ "list": [{ "id": "../x", "text": "a", "color": "" }] })).is_err());
    let r = tile::apply_preset(&dev, "s2").unwrap();
    assert_eq!(r["active"], "s2");
    tile::set_style(&dev, &json!({ "mask": "top" })).unwrap();
    net.sync(1, &["stand", "dev"]).await;
    let t = tile::of_peer(&stand, "dev").await.expect("плитка");
    assert_eq!(t["style"]["stText"], "собираю билд");
    assert_eq!(tile::st_img_of_peer(&stand, "dev").await.unwrap(), gif, "значок заготовки у соседа");
    // правка выбранной заготовки — сразу в статусе
    tile::set_presets(&dev, &json!({ "list": [{ "id": "s1", "text": "на созвоне", "color": "#ff4f64" }, { "id": "s2", "text": "собираю билд CORA", "color": "" }] })).unwrap();
    assert_eq!(tile::own_style(&dev)["stText"], "собираю билд CORA");
    tile::apply_preset(&dev, "s1").unwrap();
    assert_eq!((tile::own_style(&dev)["stColor"].as_str(), tile::own_st_img(&dev).is_none()), (Some("#FF4F64"), true));
    // тишина
    tile::set_presets(&dev, &json!({ "list": [{ "id": "s1", "text": "на созвоне", "color": "#ff4f64", "silent": true }, { "id": "s2", "text": "собираю билд CORA", "color": "" }] })).unwrap();
    assert!(tile::silent(&dev), "выбрана s1 с тишиной");
    net.sync(1, &["stand", "dev"]).await;
    assert_eq!(tile::of_peer(&stand, "dev").await.unwrap()["style"]["stSilent"], true, "сосед знает про тишину");
    tile::set_style(&dev, &json!({ "mask": "bottom" })).unwrap();
    assert!(tile::silent(&dev), "стиль плитки тишину не снимает");
    tile::apply_preset(&dev, "s2").unwrap();
    assert!(!tile::silent(&dev));
    tile::apply_preset(&dev, "").unwrap();
    assert!(tile::own_style(&dev).get("stText").is_none() && tile::own_st_img(&dev).is_none());
    assert_eq!(tile::presets(&dev)["active"], "");
    assert!(tile::apply_preset(&dev, "нет").is_err());
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn app_share_offer_accept_and_guards() {
    use bridge_node::{app_share, apps};
    let mut net = pair();
    net.start().await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    let d = apps::folder(&dev).join("gift");
    std::fs::create_dir_all(d.join("ui")).unwrap();
    std::fs::write(d.join("ui/index.html"), "подарок").unwrap();
    std::fs::create_dir_all(d.join("bin/Game_BurstDebugInformation_DoNotShip")).unwrap();
    std::fs::write(d.join("bin/Game_BurstDebugInformation_DoNotShip/x.pdb"), "отладка").unwrap();
    std::fs::write(d.join("app.json"), r#"{"id":"gift","name":"Подарок","version":"1.0.0","ui":"ui/index.html","permissions":["notify"]}"#).unwrap();
    apps::scan(&dev);
    // взять без общего сеанса — нельзя
    let e = app_share::take(&stand, "dev", "gift").await.unwrap_err();
    assert!(e.contains("одном канале"), "{}", e);
    // предложить → у соседа предложение → принять → пакет в «приложениях», ждёт разрешения
    let r = app_share::share(&dev, "gift", &["stand".to_string()]).await.unwrap();
    assert!(r["results"]["stand"].as_str().unwrap().contains("предложено"), "{}", r);
    let inc = app_share::incoming(&stand);
    assert_eq!(inc.len(), 1);
    let offer = inc[0]["offer"].as_str().unwrap().to_string();
    // чужому — не отдаётся, устаревшее — тоже
    assert!(app_share::served(&dev, &offer, "dev").is_none());
    assert!(app_share::served(&dev, "deadbeefdeadbeefdeadbeef", "stand").is_none());
    let p = stand.cfg.read().peer("dev").cloned().unwrap();
    let tmp = stand.data_dir.join("bad.part");
    assert!(stand.client_for(&p).app_package("deadbeefdeadbeefdeadbeef", &tmp, 1 << 20, "x").await.is_err());
    app_share::accept(&stand, &offer).unwrap();
    for _ in 0..100 {
        if app_share::incoming(&stand)[0]["state"] != "download" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(app_share::incoming(&stand)[0]["state"], "done", "{}", app_share::incoming(&stand)[0]);
    apps::scan(&stand);
    let a = apps::get(&stand, "gift").expect("приложение у соседа");
    assert!(a.packaged && a.version == "1.0.0");
    assert!(!apps::consent_ok(&stand, &a), "без разрешения не запускается");
    // отладочное Unity в пакет не попало
    let f = std::fs::File::open(&a.source).unwrap();
    let z = zip::ZipArchive::new(f).unwrap();
    assert!(z.file_names().all(|n| !n.contains("DoNotShip")), "{:?}", z.file_names().collect::<Vec<_>>());
    // share: false — не передаётся
    std::fs::write(d.join("app.json"), r#"{"id":"gift","name":"Подарок","version":"1.0.1","ui":"ui/index.html","share":false}"#).unwrap();
    apps::scan(&dev);
    assert!(app_share::share(&dev, "gift", &["stand".to_string()]).await.unwrap_err().contains("запрещает"));
    net.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identity_keys_signatures_and_publisher() {
    use bridge_node::{app_share, apps, identity};
    let mut net = pair();
    net.start().await;
    let (dev, stand) = (net.n("dev").clone(), net.n("stand").clone());
    // издатель и подписанное приложение с правом identity
    let pk = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
    let other = ed25519_dalek::SigningKey::from_bytes(&[8u8; 32]);
    let d = apps::folder(&dev).join("pets");
    std::fs::create_dir_all(d.join("ui")).unwrap();
    std::fs::write(d.join("ui/index.html"), "питомцы").unwrap();
    std::fs::write(d.join("app.json"), r#"{"id":"pets","name":"Питомцы","version":"1.0.0","ui":"ui/index.html","permissions":["identity","sessions"],"network":["https://pets.example.com"]}"#).unwrap();
    let pubk = apps::sign_dir(&d, &pk).unwrap();
    apps::scan(&dev);
    let a = apps::get(&dev, "pets").unwrap();
    assert_eq!(a.publisher, pubk, "подпись проверена: {}", a.sig_note);
    assert_eq!(a.network, vec!["https://pets.example.com".to_string()]);
    // без разрешения — нет ключа
    assert!(identity::get(&dev, "pets").is_err());
    apps::allow(&dev, "pets", None).unwrap();
    let me = identity::get(&dev, "pets").unwrap();
    let key = me["key"].as_str().unwrap().to_string();
    assert_eq!(key.len(), 43);
    assert_eq!(me["verified"], true);
    // ключ для издателя отличается от основного
    assert_ne!(identity::status(&dev)["key"], me["key"]);
    // вход — без вопроса; подпись проверяется
    let s = identity::sign(&dev, "pets", "login", "{\"nonce\":\"abc\"}").await.unwrap();
    let signed = s["signed"].as_str().unwrap();
    assert!(signed.starts_with(&format!("most-sign/v1\napp=pets\npub={}\npurpose=login\nts=", pubk)), "{}", signed);
    assert!(signed.ends_with("\n{\"nonce\":\"abc\"}"));
    assert!(identity::verify(&key, signed.as_bytes(), s["sig"].as_str().unwrap()));
    // зарезервированное назначение — отказ
    assert!(identity::sign(&dev, "pets", "presence", "x").await.unwrap_err().contains("только сам мост"));
    // другое назначение — спросить хозяина; ответ «да, запомнить» → дальше без вопроса
    let d2 = dev.clone();
    let waiter = tokio::spawn(async move { identity::sign(&d2, "pets", "link-key", "k2").await });
    let mut asked = None;
    for _ in 0..50 {
        if let Some(x) = identity::pending(&dev).into_iter().next() {
            asked = Some(x);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let asked = asked.expect("вопрос хозяину");
    assert_eq!(asked["purpose"], "link-key");
    identity::answer(&dev, asked["ask"].as_str().unwrap(), true, true).unwrap();
    assert!(waiter.await.unwrap().is_ok());
    assert!(identity::sign(&dev, "pets", "link-key", "k3").await.is_ok(), "запомнено до перезапуска");
    // резервная копия: смена ключа → восстановление тем же паролем → прежний ключ для издателя
    let backup = identity::export(&dev, "пароль-123").unwrap();
    assert!(identity::export(&dev, "short").is_err());
    identity::rotate(&dev).unwrap();
    let rotated = identity::get(&dev, "pets").unwrap();
    assert_ne!(rotated["key"], me["key"]);
    let r0 = &rotated["rotated"][0];
    assert!(identity::verify(r0["key"].as_str().unwrap(), r0["signed"].as_str().unwrap().as_bytes(), r0["sig"].as_str().unwrap()), "переход подписан старым ключом");
    assert!(r0["signed"].as_str().unwrap().contains("purpose=revoke"));
    assert!(identity::import(&dev, &backup, "не тот").is_err());
    identity::import(&dev, &backup, "пароль-123").unwrap();
    assert_eq!(identity::get(&dev, "pets").unwrap()["key"], me["key"]);
    // у разных ПК — разные ключи; ключ в сеансе
    // подпись пакета: изменённый пакет не принимается, другой издатель — тоже
    let r = app_share::share(&dev, "pets", &["stand".to_string()]).await.unwrap();
    assert!(r["results"]["stand"].as_str().unwrap().contains("предложено"), "{}", r);
    let offer = app_share::incoming(&stand)[0].clone();
    assert_eq!(offer["publisher"], pubk);
    app_share::accept(&stand, offer["offer"].as_str().unwrap()).unwrap();
    for _ in 0..100 {
        if app_share::incoming(&stand)[0]["state"] != "download" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(app_share::incoming(&stand)[0]["state"], "done", "{}", app_share::incoming(&stand)[0]);
    apps::scan(&stand);
    assert_eq!(apps::get(&stand, "pets").unwrap().publisher, pubk, "издатель проверен и у соседа");
    apps::allow(&stand, "pets", None).unwrap();
    assert_ne!(identity::get(&stand, "pets").unwrap()["key"], me["key"], "у другого ПК — свой ключ");
    // тот же id, другой издатель → не принят
    std::fs::write(d.join("app.json"), r#"{"id":"pets","name":"Питомцы","version":"1.1.0","ui":"ui/index.html","permissions":["identity","sessions"]}"#).unwrap();
    apps::sign_dir(&d, &other).unwrap();
    apps::scan(&dev);
    app_share::share(&dev, "pets", &["stand".to_string()]).await.unwrap();
    let offer2 = app_share::incoming(&stand).into_iter().find(|o| o["version"] == "1.1.0").unwrap();
    app_share::accept(&stand, offer2["offer"].as_str().unwrap()).unwrap();
    for _ in 0..100 {
        let o = app_share::incoming(&stand).into_iter().find(|o| o["version"] == "1.1.0").unwrap();
        if o["state"] != "download" {
            assert_eq!(o["state"], "error");
            assert!(o["error"].as_str().unwrap().contains("другим издателем"), "{}", o);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(apps::get(&stand, "pets").unwrap().version, "1.0.0");
    // пакет с испорченной подписью в папке — не принят
    let bad = apps::folder(&stand).join("bad.most-app");
    let t = stand.data_dir.join("tamper");
    std::fs::create_dir_all(t.join("ui")).unwrap();
    for f in ["app.json", "most-sig.json", "ui/index.html"] {
        std::fs::copy(d.join(f), t.join(f)).unwrap();
    }
    std::fs::write(t.join("app.json"), std::fs::read_to_string(d.join("app.json")).unwrap().replace("\"pets\"", "\"pets2\"")).unwrap();
    std::fs::write(t.join("ui/index.html"), "подмена").unwrap();
    apps::pack(&t, Some(bad.clone())).unwrap();
    apps::scan(&stand);
    let st = apps::status(&stand);
    let e = st["errors"].as_array().unwrap().iter().find(|e| e["source"] == "bad.most-app").cloned().unwrap_or_default();
    assert!(e["error"].as_str().unwrap_or("").contains("подпись издателя"), "{}", st["errors"]);
    // зарезервированный id
    assert!(apps::parse_manifest(&d, r#"{"id":"most","name":"x","ui":"ui/index.html"}"#).unwrap_err().contains("занят мостом"));
    assert!(apps::parse_manifest(&d, r#"{"id":"x","name":"x","ui":"ui/index.html","network":["https://*.evil.com"]}"#).is_err());
    assert!(apps::parse_manifest(&d, r#"{"id":"x","name":"x","ui":"ui/index.html","network":["http://plain.com"]}"#).is_err());
    net.stop().await;
}
