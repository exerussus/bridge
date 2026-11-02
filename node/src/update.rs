//! Обновление моста от соседа. «Поделиться версией»: соседям на связи со старой версией уходит предложение (версия,
//! размеры и sha256 файлов, что нового). Сосед подтверждает — его мост скачивает у нас Most.exe и bridge.exe, сверяет
//! sha256, переименовывает свои запущенные файлы в *.old (Windows это разрешает), кладёт новые и перезапускается.
//! Новый запуск убирает *.old. Соседу со старой версией (без этой возможности) файлы уходят обычной отправкой.

use crate::node::Node;
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::path::PathBuf;
use std::sync::Arc;

/// С этой версии мост умеет принимать обновление по сети.
pub const MIN_AUTO: &str = "2.12.0";

/// Программа с окном и значком (с 3.13 — латиницей).
pub const APP_EXE: &str = "Most.exe";
/// Прежнее имя (до 3.13). Мостам старше 3.13 версия уходит под ним — другого имени они не примут; запустившись,
/// новая версия сама переименует себя в Most.exe.
pub const LEGACY_EXE: &str = "Мост.exe";
/// С этой версии мост знает имя Most.exe.
pub const ASCII_EXE_SINCE: &str = "3.13.0";

/// Имя файла для соседа версии `ver`: старым — прежнее имя программы.
pub fn wire_name<'a>(name: &'a str, ver: &str) -> &'a str {
    if name == APP_EXE && !ver.is_empty() && cmp(ver, ASCII_EXE_SINCE) == Ordering::Less {
        LEGACY_EXE
    } else {
        name
    }
}

/// Имя, под которым файл лежит здесь (прежнее имя программы — то же, что Most.exe).
pub fn local_name(name: &str) -> Option<&'static str> {
    if name == LEGACY_EXE {
        return Some(if cfg!(windows) { APP_EXE } else { "bridge-app" });
    }
    names().iter().find(|n| **n == name).copied()
}

/// Путь к своему файлу программы: Most.exe, а если мост ещё не переименовался — Мост.exe.
pub fn local_path(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    let n = local_name(name)?;
    let p = dir.join(n);
    if !p.exists() && n == APP_EXE && dir.join(LEGACY_EXE).exists() {
        return Some(dir.join(LEGACY_EXE));
    }
    Some(p)
}

pub struct Updates {
    /// Предложение от соседа: {from, version, files, notes, time}.
    offer: Mutex<Option<Value>>,
    /// Ход: idle | offer | download | install | restart | done | error.
    status: Mutex<(String, String)>,
    /// Папка с программой (для тестов — своя).
    pub dir_override: Mutex<Option<PathBuf>>,
    /// Перезапустить программу (ставит трей или консольный запуск). Нет — «перезапустите мост вручную».
    pub restart_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    exe_at_start: Option<PathBuf>,
}

impl Default for Updates {
    fn default() -> Self {
        Updates { offer: Mutex::new(None), status: Mutex::new(("idle".into(), String::new())), dir_override: Mutex::new(None), restart_hook: Mutex::new(None), exe_at_start: std::env::current_exe().ok() }
    }
}

/// Сравнить версии «2.11.0».
pub fn cmp(a: &str, b: &str) -> Ordering {
    let p = |s: &str| s.trim().trim_start_matches('v').split('.').map(|x| x.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (x, y) = (p(a), p(b));
    for i in 0..3 {
        let o = x.get(i).unwrap_or(&0).cmp(y.get(i).unwrap_or(&0));
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

pub fn names() -> &'static [&'static str] {
    if cfg!(windows) {
        &[APP_EXE, "bridge.exe"]
    } else {
        &["bridge-app", "bridge"]
    }
}

impl Updates {
    pub fn dir(&self) -> PathBuf {
        if let Some(d) = self.dir_override.lock().clone() {
            return d;
        }
        self.exe_at_start.as_ref().and_then(|e| e.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn state(&self) -> Value {
        let (s, e) = self.status.lock().clone();
        json!({ "version": bridge_core::APP_VERSION, "status": s, "error": e, "offer": *self.offer.lock(), "restartable": self.restart_hook.lock().is_some() })
    }

    fn set(&self, node: &Node, s: &str, e: &str) {
        *self.status.lock() = (s.into(), e.into());
        node.emit("update-state", json!({ "status": s, "error": e }));
    }
}

/// Свои файлы программы: (имя, путь, размер, sha256).
pub fn own_files(node: &Node) -> Vec<(String, PathBuf, u64, String)> {
    let dir = node.updates.dir();
    names()
        .iter()
        .filter_map(|n| {
            let p = local_path(&dir, n)?;
            let b = std::fs::read(&p).ok()?;
            Some((n.to_string(), p, b.len() as u64, hex::encode(Sha256::digest(&b))))
        })
        .collect()
}

/// Что нового с версии `since` (выдержка из журнала изменений).
pub fn notes_since(since: &str) -> String {
    let text = include_str!("../../CHANGELOG.md");
    let mut out = String::new();
    for block in text.split("\n## ").skip(1) {
        let ver = block.split_whitespace().next().unwrap_or("");
        if cmp(ver, since) != Ordering::Greater {
            break;
        }
        out.push_str("## ");
        out.push_str(block.trim_end());
        out.push_str("\n\n");
        if out.len() > 6000 {
            break;
        }
    }
    out
}

/// После перезапуска: убрать *.old от прошлого обновления.
pub fn cleanup(node: &Node) {
    let dir = node.updates.dir();
    for n in names().iter().chain([LEGACY_EXE].iter()) {
        let _ = std::fs::remove_file(dir.join(format!("{}.old", n)));
    }
    let marker = node.data_dir.join("update-done.json");
    if let Ok(t) = std::fs::read_to_string(&marker) {
        let _ = std::fs::remove_file(&marker);
        if let Ok(v) = serde_json::from_str::<Value>(&t) {
            node.info(&format!("Мост обновлён до {} (от {})", bridge_core::APP_VERSION, v["from"].as_str().unwrap_or("соседа")));
            node.emit("update-done", json!({ "version": bridge_core::APP_VERSION, "from": v["from"] }));
        }
    }
}

/// «Поделиться версией»: предложить обновление соседям (по списку или всем на связи).
pub async fn offer(node: &Arc<Node>, only: &[String]) -> Value {
    let mine = bridge_core::APP_VERSION;
    let files = own_files(node);
    let mut results = serde_json::Map::new();
    if files.is_empty() {
        return json!({ "error": "не найдены файлы программы рядом с мостом", "results": results });
    }
    let peers: Vec<_> = node.cfg.read().peers.iter().filter(|p| p.enabled && p.has_url() && (only.is_empty() || only.iter().any(|o| bridge_core::eq_ic(o, &p.name)))).cloned().collect();
    for p in peers {
        let st = node.peer_state(&p.name);
        let res = if !st.online {
            "не на связи".to_string()
        } else if st.version.is_empty() {
            "версия неизвестна".to_string()
        } else if cmp(&st.version, mine) != Ordering::Less {
            format!("уже {}", st.version)
        } else if cmp(&st.version, MIN_AUTO) == Ordering::Less {
            // старый мост сам обновиться не умеет — файлы обычной отправкой
            let old: Vec<_> = files.iter().map(|(n, p, s, h)| (wire_name(n, &st.version).to_string(), p.clone(), *s, h.clone())).collect();
            match send_files(node, &p.name, &old) {
                Ok(dir) => {
                    if cmp(&st.version, "2.10.0") != Ordering::Less {
                        let _ = crate::chat::send(node, &format!("dm:{}", p.name), &format!("Новая версия моста {}: файлы — в «{}». Закройте мост (трей → «Выход»), замените Мост.exe и bridge.exe этими и запустите снова. Дальше обновления будут приходить сами.", mine, dir), "").await;
                    }
                    format!("версия {} не умеет обновляться сама — файлы отправлены в «{}»", st.version, dir)
                }
                Err(e) => format!("не удалось отправить файлы: {}", e),
            }
        } else {
            let body = json!({
                "version": mine,
                "files": files.iter().map(|(n, _, s, h)| json!({ "name": wire_name(n, &st.version), "size": s, "sha256": h })).collect::<Vec<_>>(),
                "notes": notes_since(&st.version),
            });
            let c = node.client_for(&p);
            match tokio::time::timeout(std::time::Duration::from_secs(10), c.post_json("/peer/update/offer", &body)).await {
                Ok(Ok((code, _))) if (200..300).contains(&code) => "предложено — ждём подтверждения".to_string(),
                Ok(Ok((code, t))) => crate::client::error_text(&t, code),
                Ok(Err(e)) => crate::link::explain(&e, &p).0,
                Err(_) => "сосед не ответил".into(),
            }
        };
        results.insert(p.name.clone(), json!(res));
    }
    node.info(&format!("Поделиться версией {}: {}", mine, results.iter().map(|(k, v)| format!("{} — {}", k, v.as_str().unwrap_or(""))).collect::<Vec<_>>().join("; ")));
    json!({ "version": mine, "results": results })
}

/// Старому мосту — файлы через «Отправку» (в его «приём/<мы>/Мост <версия>/»).
fn send_files(node: &Node, peer: &str, files: &[(String, PathBuf, u64, String)]) -> Result<String, String> {
    let sub = format!("Most {}", bridge_core::APP_VERSION);
    let dir = node.mailbox.out_root(node).join(peer).join(&sub);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for (n, p, _, _) in files {
        std::fs::copy(p, dir.join(n)).map_err(|e| e.to_string())?;
    }
    node.mailbox.dirty.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(format!("приём/{}/{}", node.node_name(), sub))
}

/// Пришло предложение от соседа.
pub fn on_offer(node: &Node, from: &str, v: &Value) -> Result<(), (u16, String)> {
    let ver = v["version"].as_str().unwrap_or("");
    if cmp(ver, bridge_core::APP_VERSION) != Ordering::Greater {
        return Err((409, format!("здесь уже {}", bridge_core::APP_VERSION)));
    }
    let files = v["files"].as_array().cloned().unwrap_or_default();
    if files.is_empty() || files.iter().any(|f| local_name(f["name"].as_str().unwrap_or("")).is_none() || f["sha256"].as_str().map(|h| h.len() != 64).unwrap_or(true)) {
        return Err((400, "неверный список файлов".into()));
    }
    let st = node.updates.status.lock().0.clone();
    if ["download", "install", "restart"].contains(&st.as_str()) {
        return Err((409, "обновление уже идёт".into()));
    }
    let notes: String = v["notes"].as_str().unwrap_or("").chars().take(8000).collect();
    *node.updates.offer.lock() = Some(json!({ "from": from, "version": ver, "files": files, "notes": notes, "time": bridge_core::time::format(&bridge_core::time::now()) }));
    node.updates.set(node, "offer", "");
    node.info(&format!("«{}» предлагает обновить мост до {}", from, ver));
    node.emit("update-offer", json!({ "from": from, "version": ver }));
    Ok(())
}

pub fn decline(node: &Node) {
    *node.updates.offer.lock() = None;
    node.updates.set(node, "idle", "");
}

/// Подтвердили: скачать, проверить, заменить, перезапустить.
pub async fn accept(node: &Arc<Node>) -> Result<(), String> {
    let offer = node.updates.offer.lock().clone().ok_or("предложения нет")?;
    let from = offer["from"].as_str().unwrap_or("").to_string();
    let p = node.cfg.read().peer(&from).cloned().ok_or("соседа больше нет")?;
    node.updates.set(node, "download", "");
    let tmp = node.data_dir.join("update");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let client = node.client_for(&p);
    let files = offer["files"].as_array().cloned().unwrap_or_default();
    let fail = |e: String| {
        node.updates.set(node, "error", &e);
        node.info(&format!("Обновление не удалось: {}", e));
        e
    };
    for f in &files {
        let name = f["name"].as_str().unwrap_or("");
        let size = f["size"].as_i64().unwrap_or(0);
        let g = node.transfers.start("down", &from, "обновление", name, size);
        let r = tokio::time::timeout(std::time::Duration::from_secs(600), client.get_bytes(&format!("/peer/update/file?name={}", crate::client::esc(name)))).await;
        drop(g);
        let b = match r {
            Ok(Ok((200, b))) => b,
            Ok(Ok((c, b))) => return Err(fail(crate::client::error_text(&String::from_utf8_lossy(&b), c))),
            Ok(Err(e)) => return Err(fail(crate::link::explain(&e, &p).0)),
            Err(_) => return Err(fail("скачивание слишком долгое".into())),
        };
        if hex::encode(Sha256::digest(&b)) != f["sha256"].as_str().unwrap_or("") {
            return Err(fail(format!("«{}» пришёл повреждённым (sha256 не совпал) — обновление отменено", name)));
        }
        std::fs::write(tmp.join(name), &b).map_err(|e| fail(e.to_string()))?;
    }
    node.updates.set(node, "install", "");
    let dir = node.updates.dir();
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    for f in &files {
        let wire = f["name"].as_str().unwrap_or("");
        let name = local_name(wire).unwrap_or(wire);
        let target = dir.join(name);
        let old = dir.join(format!("{}.old", name));
        let _ = std::fs::remove_file(&old);
        if target.exists() {
            if let Err(e) = std::fs::rename(&target, &old) {
                // вернуть уже заменённое
                for (t, o) in done.iter().rev() {
                    let _ = std::fs::remove_file(t);
                    let _ = std::fs::rename(o, t);
                }
                return Err(fail(format!("не удалось заменить «{}»: {}", name, e)));
            }
        }
        if let Err(e) = std::fs::rename(tmp.join(wire), &target).or_else(|_| std::fs::copy(tmp.join(wire), &target).map(|_| ())) {
            let _ = std::fs::rename(&old, &target);
            for (t, o) in done.iter().rev() {
                let _ = std::fs::remove_file(t);
                let _ = std::fs::rename(o, t);
            }
            return Err(fail(format!("не удалось положить «{}»: {}", name, e)));
        }
        done.push((target, old));
    }
    let _ = std::fs::write(node.data_dir.join("update-done.json"), json!({ "from": from, "version": offer["version"] }).to_string());
    *node.updates.offer.lock() = None;
    node.info(&format!("Мост {} установлен (от {}) — перезапускаюсь", offer["version"].as_str().unwrap_or(""), from));
    let hook = node.updates.restart_hook.lock().clone();
    match hook {
        Some(h) => {
            save_rejoin(node);
            node.updates.set(node, "restart", "");
            tokio::time::sleep(std::time::Duration::from_millis(600)).await; // панель успеет показать «перезапускаюсь»
            h();
        }
        None => node.updates.set(node, "done", "установлено — перезапустите мост"),
    }
    Ok(())
}

// ───────────── возврат в канал после обновления (3.10) ─────────────

fn rejoin_file(node: &Node) -> std::path::PathBuf {
    node.data_dir.join("rejoin.json")
}

/// Перед перезапуском: если ПК сидел в канале — запомнить, куда вернуться.
pub fn save_rejoin(node: &Node) {
    let ch = crate::net::channel(node);
    if ch.room.is_empty() {
        return;
    }
    let v = json!({ "room": ch.room, "title": ch.title, "at": chrono::Utc::now().timestamp() });
    let _ = std::fs::write(rejoin_file(node), v.to_string());
}

fn read_rejoin(node: &Node) -> Option<Value> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(rejoin_file(node)).ok()?).ok()?;
    let age = chrono::Utc::now().timestamp() - v["at"].as_i64().unwrap_or(0);
    // вернуться можно в течение 10 минут после перезапуска
    (v["room"].as_str().map(|r| !r.is_empty()).unwrap_or(false) && (0..600).contains(&age)).then_some(v)
}

/// Есть куда вернуться: окно приложения после обновления откроется, даже если запуск «к часам».
pub fn rejoin_pending(node: &Node) -> bool {
    read_rejoin(node).is_some()
}

/// Панель забирает канал для возврата (один раз).
pub fn take_rejoin(node: &Node) -> Option<Value> {
    let v = read_rejoin(node);
    let _ = std::fs::remove_file(rejoin_file(node));
    v
}

#[cfg(test)]
mod exe_name_tests {
    use super::*;

    #[test]
    fn legacy_exe_name_for_old_peers() {
        // мостам до 3.13 программа уходит под прежним именем, остальным — Most.exe; bridge.exe не меняется
        assert_eq!(wire_name(APP_EXE, "3.12.0"), LEGACY_EXE);
        assert_eq!(wire_name(APP_EXE, "3.13.0"), APP_EXE);
        assert_eq!(wire_name(APP_EXE, ""), APP_EXE);
        assert_eq!(wire_name("bridge.exe", "3.0.0"), "bridge.exe");
        // прежнее имя принимается и кладётся как своя программа
        assert!(local_name(LEGACY_EXE).is_some());
        assert!(local_name("evil.exe").is_none());
    }

    #[test]
    fn serves_legacy_file_until_renamed() {
        let d = std::env::temp_dir().join(format!("bridge-exe-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        if cfg!(windows) {
            std::fs::write(d.join(LEGACY_EXE), b"old").unwrap();
            assert_eq!(local_path(&d, APP_EXE).unwrap(), d.join(LEGACY_EXE));
            std::fs::write(d.join(APP_EXE), b"new").unwrap();
            assert_eq!(local_path(&d, LEGACY_EXE).unwrap(), d.join(APP_EXE));
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
