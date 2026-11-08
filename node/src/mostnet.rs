//! Своя локальная сеть MostNet (3.17): мост управляет службой `most-net` на этом ПК (локальный API 127.0.0.1:47791).
//! Служба получает от моста адрес сервера и ключ участника (тот же, что у моста для своего сервера) и включается,
//! когда в мосте включена «Своя локальная сеть». Ставится служба один раз с подтверждением администратора (UAC).

use crate::node::Node;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const API: &str = "http://127.0.0.1:47791";

fn client(secs: u64) -> reqwest::Client {
    reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(secs)).build().unwrap_or_default()
}

/// Состояние службы; None — не отвечает (не установлена или не запущена).
pub async fn status() -> Option<Value> {
    let r = client(2).get(format!("{}/status", API)).send().await.ok()?;
    r.json().await.ok()
}

pub async fn logs() -> Vec<String> {
    let Ok(r) = client(2).get(format!("{}/log", API)).send().await else { return Vec::new() };
    let v: Value = r.json().await.unwrap_or_default();
    v["lines"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// Передать службе сервер, ключ участника и «включено ли».
pub async fn push(node: &Arc<Node>) -> Result<Value, String> {
    let rc = node.cfg.read().relay.clone();
    let on = rc.net && rc.configured() && rc.mode != "off";
    let body = json!({ "server": rc.server, "member": rc.member, "secret": rc.secret, "enabled": on });
    let r = client(5).post(format!("{}/config", API)).json(&body).send().await.map_err(|_| "служба MostNet не отвечает — установите её".to_string())?;
    r.json().await.map_err(|e| e.to_string())
}

/// Запомнить, работает ли своя сеть (от этого зависит, гнать ли файлы через сервер).
pub fn note(node: &Node, svc: &Option<Value>) {
    let up = svc.as_ref().map(|s| s["state"] == "on").unwrap_or(false);
    node.mostnet_up.store(up, std::sync::atomic::Ordering::Relaxed);
}

/// Раз в 20 с: служба отвечает — сверить с ней настройки (она сама ничего не делает, если они не изменились).
pub fn schedule(node: &Arc<Node>) {
    let n = node.clone();
    tokio::spawn(async move {
        let mut tries = 0u32;
        loop {
            let s = status().await;
            note(&n, &s);
            // (3.22.1) служба могла ещё не подняться после перезагрузки — пробуем первые 2 минуты (7 опросов)
            if tries < REFRESH_TRIES && n.data_dir.join(crate::update::NET_REFRESH).exists() {
                tries += 1;
                refresh_after_update(&n, &s, tries >= REFRESH_TRIES);
            }
            if s.is_some() {
                let _ = push(&n).await;
            }
            tokio::select! {
                _ = n.shutdown.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(20)) => {}
            }
        }
    });
}

const REFRESH_TRIES: u32 = 7;

/// (3.21.1) С обновлением моста от соседа пришла новая служба своей сети: если она установлена здесь и старее
/// пришедшей — переставить (Windows спросит подтверждение администратора; один раз на версию). (3.22.1) Метка
/// снимается только после ответа службы (или когда она так и не ответила за `REFRESH_TRIES` опросов); установка — в
/// отдельной задаче: пока окно Windows открыто, сверка настроек со службой идёт своим чередом.
fn refresh_after_update(node: &Arc<Node>, s: &Option<Value>, last: bool) {
    let mark = node.data_dir.join(crate::update::NET_REFRESH);
    let Some(s) = s else {
        if last {
            let _ = std::fs::remove_file(&mark);
            node.info("Своя сеть: служба не отвечает — не обновлена; если она установлена: «Соседи» → «Сеть и сервер» → «Обновить службу»");
        }
        return; // ещё не поднялась (или не ставили) — следующий опрос
    };
    let want = std::fs::read_to_string(&mark).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| bridge_core::APP_VERSION.to_string());
    let _ = std::fs::remove_file(&mark);
    let ver = s["version"].as_str().unwrap_or("").to_string();
    if !ver.is_empty() && crate::update::cmp(&ver, &want) != std::cmp::Ordering::Less {
        return; // уже такая же или новее — не трогать
    }
    let node = node.clone();
    tokio::spawn(async move {
        node.info(&format!("Своя сеть: служба {} старше пришедшей {} — обновляю (Windows спросит подтверждение)", if ver.is_empty() { "?" } else { &ver }, want));
        match install(false).await {
            Ok(t) => node.info(&format!("Своя сеть: служба обновлена — {}", t)),
            Err(e) => node.info(&format!("Своя сеть: службу не обновить ({}) — «Соседи» → «Сеть и сервер» → «Обновить службу»", e)),
        }
    });
}

/// most-net.exe рядом с программой моста (его кладёт build.cmd в publish).
pub fn exe() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.with_file_name(if cfg!(windows) { "most-net.exe" } else { "most-net" });
    p.is_file().then_some(p)
}

pub fn install_log() -> String {
    let dir = PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into())).join("MostNet");
    std::fs::read_to_string(dir.join("install.log")).ok().and_then(|t| t.lines().last().map(String::from)).unwrap_or_default()
}

/// Поставить или убрать службу: Windows спросит подтверждение администратора.
pub async fn install(remove: bool) -> Result<String, String> {
    if !cfg!(windows) {
        return Err("служба своей сети ставится только на Windows".into());
    }
    let exe = exe().ok_or("рядом с Most.exe нет most-net.exe и wintun.dll — положите их из publish (их собирает build.cmd)")?;
    if !exe.with_file_name("wintun.dll").is_file() {
        return Err("рядом с Most.exe нет wintun.dll — положите его из publish (его скачивает build.cmd)".into());
    }
    let before = install_log();
    // установка — с путём к этому Most.exe: служба разрешит ему входящие из своей сети (брандмауэр)
    let args = if remove {
        "uninstall".to_string()
    } else {
        match std::env::current_exe() {
            Ok(me) => format!("install --allow \"{}\"", me.display()),
            Err(_) => "install".to_string(),
        }
    };
    let script = format!("Start-Process -FilePath '{}' -ArgumentList '{}' -Verb RunAs -Wait -WindowStyle Hidden", exe.display().to_string().replace('\'', "''"), args.replace('\'', "''"));
    let mut c = tokio::process::Command::new("powershell");
    c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script]);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000);
    let o = c.output().await.map_err(|e| format!("powershell: {}", e))?;
    let after = install_log();
    if !o.status.success() || after == before {
        let err = String::from_utf8_lossy(&o.stderr).to_string();
        if err.contains("canceled") || err.contains("отмен") {
            return Err("установку отменили в окне Windows (нужно подтверждение администратора)".into());
        }
        return Err(if after != before { after } else { format!("не вышло: {}", err.trim()) });
    }
    if after.contains("ошибка") {
        return Err(after);
    }
    Ok(after)
}
