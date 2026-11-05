//! most-net — своя локальная сеть Моста (MostNet, 3.17).
//!
//! Работает на ПК службой (Windows: задание Планировщика от SYSTEM при загрузке — адаптеру Wintun нужны права
//! администратора). Мост настраивает её через локальный API 127.0.0.1:47791: какой сервер, ключ участника, включена ли.
//!   most-net run [--dir <папка>]   — работать (служба делает это сама)
//!   most-net install               — (администратор) поставить службу: файлы в %ProgramData%\MostNet, задание, брандмауэр
//!   most-net uninstall             — (администратор) убрать службу
//!   most-net status                — спросить работающую службу

mod engine;
mod tun;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use engine::Engine;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

pub const API_PORT: u16 = 47791;

fn default_dir() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into())).join("MostNet")
    } else {
        PathBuf::from("/var/lib/most-net")
    }
}

fn opt<'a>(args: &'a [String], k: &str) -> Option<&'a str> {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

// ---------- локальный API ----------

type St = State<Arc<Engine>>;

async fn status(State(e): St) -> Json<Value> {
    Json(json!(e.status()))
}

async fn logs(State(e): St) -> Json<Value> {
    Json(json!({ "lines": e.logs() }))
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct CfgReq {
    server: Option<String>,
    member: Option<String>,
    secret: Option<String>,
    enabled: Option<bool>,
}

async fn config(State(e): St, Json(r): Json<CfgReq>) -> Response {
    let mut c = e.cfg.lock().clone();
    if let Some(s) = r.server {
        if !s.is_empty() {
            if let Err(err) = bridge_core::relay::split_addr(&s) {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": err }))).into_response();
            }
        }
        c.server = s;
    }
    if let Some(m) = r.member {
        c.member = m;
    }
    if let Some(s) = r.secret {
        c.secret = s;
    }
    if let Some(en) = r.enabled {
        c.enabled = en;
    }
    match e.set_cfg(c) {
        Ok(()) => Json(json!(e.status())).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": err }))).into_response(),
    }
}

async fn run(dir: PathBuf, port: u16) -> Result<(), String> {
    let e = Engine::load(dir)?;
    e.log(&format!("most-net {} запущен, папка {}", env!("CARGO_PKG_VERSION"), e.dir.display()));
    tokio::spawn(engine::run(e.clone()));
    let app = Router::new().route("/status", get(status)).route("/log", get(logs)).route("/config", axum::routing::post(config)).with_state(e.clone());
    let l = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|err| format!("127.0.0.1:{}: {} — служба уже запущена?", port, err))?;
    axum::serve(l, app).await.map_err(|e| e.to_string())
}

// ---------- установка (Windows) ----------

#[cfg(windows)]
fn ps(script: &str) -> Result<String, String> {
    tun::sh("powershell", &["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script])
}

#[cfg(windows)]
fn install(allow: Option<String>) -> Result<(), String> {
    let dir = default_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let src_dll = me.with_file_name("wintun.dll");
    if !src_dll.is_file() {
        return Err(format!("рядом с most-net.exe нет wintun.dll ({}) — её кладёт build.cmd в publish", src_dll.display()));
    }
    let exe = dir.join("most-net.exe");
    // старую службу — остановить, чтобы файл можно было заменить
    let _ = ps("Stop-ScheduledTask -TaskName 'MostNet' -ErrorAction SilentlyContinue");
    let _ = ps(&format!("Get-Process most-net -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -eq '{}' }} | Stop-Process -Force", exe.display()));
    std::thread::sleep(std::time::Duration::from_millis(800));
    if std::fs::canonicalize(&me).ok() != std::fs::canonicalize(&exe).ok() {
        std::fs::copy(&me, &exe).map_err(|e| format!("{}: {}", exe.display(), e))?;
    }
    std::fs::copy(&src_dll, dir.join("wintun.dll")).map_err(|e| format!("wintun.dll: {}", e))?;
    let script = format!(
        "$a = New-ScheduledTaskAction -Execute '{exe}' -Argument 'run' -WorkingDirectory '{dir}'; \
         $t = New-ScheduledTaskTrigger -AtStartup; \
         $s = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -MultipleInstances IgnoreNew; \
         $p = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest; \
         Register-ScheduledTask -TaskName 'MostNet' -Description 'Своя локальная сеть Моста (MostNet)' -Action $a -Trigger $t -Settings $s -Principal $p -Force | Out-Null; \
         Remove-NetFirewallRule -DisplayName 'MostNet: ping' -ErrorAction SilentlyContinue; \
         New-NetFirewallRule -DisplayName 'MostNet: ping' -Direction Inbound -Protocol ICMPv4 -IcmpType 8 -RemoteAddress 10.77.0.0/16 -Action Allow | Out-Null; \
         Remove-NetFirewallRule -DisplayName 'MostNet: Most.exe' -ErrorAction SilentlyContinue; \
         {allow} \
         Start-ScheduledTask -TaskName 'MostNet'",
        exe = exe.display(),
        dir = dir.display(),
        // (3.19.2) Мост этого ПК — входящие из своей сети разрешены при любом профиле сети: иначе брандмауэр
        // пускает Most.exe только из «частных» сетей, а соседи по 10.77.x.y не достучатся
        allow = allow
            .filter(|p| !p.trim().is_empty())
            .map(|p| format!("New-NetFirewallRule -DisplayName 'MostNet: Most.exe' -Direction Inbound -Program '{}' -RemoteAddress 10.77.0.0/16 -Action Allow -Profile Any | Out-Null;", p.replace('\'', "''")))
            .unwrap_or_default()
    );
    ps(&script)?;
    println!("Служба MostNet поставлена и запущена ({}).", dir.display());
    Ok(())
}

#[cfg(windows)]
fn uninstall() -> Result<(), String> {
    let exe = default_dir().join("most-net.exe");
    let _ = ps("Stop-ScheduledTask -TaskName 'MostNet' -ErrorAction SilentlyContinue");
    let _ = ps(&format!("Get-Process most-net -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -eq '{}' }} | Stop-Process -Force", exe.display()));
    ps("Unregister-ScheduledTask -TaskName 'MostNet' -Confirm:$false -ErrorAction SilentlyContinue; Remove-NetFirewallRule -DisplayName 'MostNet: ping' -ErrorAction SilentlyContinue; Remove-NetFirewallRule -DisplayName 'MostNet: Most.exe' -ErrorAction SilentlyContinue")?;
    println!("Служба MostNet убрана (папка с ключом сети осталась: {}).", default_dir().display());
    Ok(())
}

#[cfg(not(windows))]
fn install(_allow: Option<String>) -> Result<(), String> {
    Err("установка службой — только на Windows; на Linux запускайте: most-net run --dir <папка>".into())
}

#[cfg(not(windows))]
fn uninstall() -> Result<(), String> {
    install(None)
}

/// Итог установки — в файл рядом с данными службы: мост покажет его, если что-то пошло не так.
fn note(kind: &str, r: &Result<(), String>) {
    let line = format!("{} {} — {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), kind, match r {
        Ok(()) => "готово".to_string(),
        Err(e) => format!("ошибка: {}", e),
    });
    let _ = std::fs::create_dir_all(default_dir());
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(default_dir().join("install.log")) {
        let _ = f.write_all(line.as_bytes());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    let res = match cmd {
        "run" => {
            let dir = opt(&args, "--dir").map(PathBuf::from).unwrap_or_else(default_dir);
            let port = opt(&args, "--api-port").and_then(|p| p.parse().ok()).unwrap_or(API_PORT);
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio");
            rt.block_on(run(dir, port))
        }
        "install" => {
            let r = install(opt(&args, "--allow").map(String::from));
            note("install", &r);
            r
        }
        "uninstall" => {
            let r = uninstall();
            note("uninstall", &r);
            r
        }
        "status" => {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio");
            rt.block_on(async {
                let r = reqwest::Client::builder().no_proxy().build().map_err(|e| e.to_string())?.get(format!("http://127.0.0.1:{}/status", API_PORT)).send().await.map_err(|_| "служба не отвечает — не установлена или не запущена".to_string())?;
                println!("{}", r.text().await.unwrap_or_default());
                Ok(())
            })
        }
        "version" | "--version" => {
            println!("most-net {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => {
            println!("most-net — своя локальная сеть Моста\n\n  most-net run [--dir <папка>] [--api-port <порт>]\n  most-net install | uninstall   (Windows, администратор)\n  most-net status");
            Ok(())
        }
    };
    if let Err(e) = res {
        eprintln!("Ошибка: {}", e);
        std::process::exit(1);
    }
}

