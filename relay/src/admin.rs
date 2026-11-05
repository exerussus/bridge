//! Сервер: канал администратора по мастер-ключу (`/relay/v1/admin`). Команды — состояние, приглашения, отзыв,
//! настройки, журнал, перезапуск; терминал root (сервер работает от root — так решили: мастер-ключ = доступ к VPS).

use crate::proto::{self, ClientHello, ServerHello};
use crate::pty::{self, Pty};
use crate::{load, log, make_invite, now_text, save, Srv, SERVICE, VERSION};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::Extension;
use axum::response::Response;
use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const MAX_TERMS: usize = 4;

pub async fn ws(Extension(srv): Extension<Arc<Srv>>, Extension(from): Extension<SocketAddr>, up: WebSocketUpgrade) -> Response {
    up.max_message_size(4 << 20).on_upgrade(move |s| session(srv, from, s))
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

async fn refuse(sock: &mut WebSocket, srv: &Srv, from: SocketAddr, why: &str) {
    srv.tries.lock().entry(from.ip()).or_insert((0, chrono::Utc::now().timestamp())).0 += 1;
    let _ = sock.send(Message::Text(serde_json::to_string(&ServerHello { error: why.into(), ..Default::default() }).unwrap())).await;
    log(&format!("администратор: отказ {} — {}", from.ip(), why));
}

async fn session(srv: Arc<Srv>, from: SocketAddr, mut sock: WebSocket) {
    let now = chrono::Utc::now().timestamp();
    let blocked = {
        let mut t = srv.tries.lock();
        let e = t.entry(from.ip()).or_insert((0, now));
        if now - e.1 > 60 {
            *e = (0, now);
        }
        e.0 >= 20
    };
    if blocked {
        let _ = sock.send(Message::Text(serde_json::to_string(&ServerHello { error: "слишком много попыток — подождите минуту".into(), ..Default::default() }).unwrap())).await;
        return;
    }
    let first = tokio::time::timeout(Duration::from_secs(10), sock.recv()).await;
    let Ok(Some(Ok(Message::Text(t)))) = first else { return };
    let Ok(h) = serde_json::from_str::<ClientHello>(&t) else { return refuse(&mut sock, &srv, from, "неверное начало канала").await };
    let master = srv.cfg.read().master.clone();
    if master.is_empty() {
        return refuse(&mut sock, &srv, from, "мастер-ключа на сервере нет — создайте: most-relay master --new").await;
    }
    if !srv.nonces.try_use(&format!("admin/{}", h.nonce), now) {
        return refuse(&mut sock, &srv, from, "повтор запроса").await;
    }
    let (sh, ch) = match proto::server_accept(&master, &h, now) {
        Ok(x) => x,
        Err(e) => return refuse(&mut sock, &srv, from, &e).await,
    };
    if sock.send(Message::Text(serde_json::to_string(&sh).unwrap())).await.is_err() {
        return;
    }
    log(&format!("администратор подключился ({})", from.ip()));
    let (mut sealer, mut opener) = ch.split();
    let (out, mut out_rx) = mpsc::channel::<Value>(1024);
    let epoch = srv.admin_epoch.lock().clone();
    let mut terms: HashMap<String, Pty> = HashMap::new();
    let mut next_tid = 1u32;
    loop {
        tokio::select! {
            _ = epoch.cancelled() => break,
            _ = srv.turn.stop.cancelled() => break,
            m = out_rx.recv() => {
                let Some(v) = m else { break };
                // событие терминала «закрылся» — убрать его
                if v["ev"] == "term.exit" {
                    if let Some(mut p) = terms.remove(v["tid"].as_str().unwrap_or("")) {
                        p.kill();
                    }
                }
                let ct = sealer.seal(v.to_string().as_bytes());
                if sock.send(Message::Binary(ct)).await.is_err() {
                    break;
                }
            }
            m = sock.recv() => {
                let Some(Ok(m)) = m else { break };
                let data = match m {
                    Message::Binary(b) => b,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let plain = match opener.open(&data) {
                    Ok(p) => p,
                    Err(e) => {
                        log(&format!("администратор {}: {}", from.ip(), e));
                        break;
                    }
                };
                let Ok(req) = serde_json::from_slice::<Value>(&plain) else { continue };
                let id = req["id"].clone();
                let op = req["op"].as_str().unwrap_or("").to_string();
                let args = req["args"].clone();
                let res = match op.as_str() {
                    "term.open" => term_open(&mut terms, &mut next_tid, &args, out.clone()),
                    "term.write" => term_with(&mut terms, &args, |p| {
                        let d = b64().decode(args["data"].as_str().unwrap_or("")).map_err(|_| "данные не base64".to_string())?;
                        p.write(&d).map(|_| json!(true))
                    }),
                    "term.resize" => term_with(&mut terms, &args, |p| {
                        p.resize(args["cols"].as_u64().unwrap_or(80) as u16, args["rows"].as_u64().unwrap_or(24) as u16);
                        Ok(json!(true))
                    }),
                    "term.close" => {
                        if let Some(mut p) = terms.remove(args["tid"].as_str().unwrap_or("")) {
                            p.kill();
                        }
                        Ok(json!(true))
                    }
                    _ => op_call(&srv, &op, &args).await,
                };
                let reply = match res {
                    Ok(data) => json!({ "re": id, "ok": true, "data": data }),
                    Err(e) => json!({ "re": id, "ok": false, "error": e }),
                };
                let ct = sealer.seal(reply.to_string().as_bytes());
                if sock.send(Message::Binary(ct)).await.is_err() {
                    break;
                }
            }
        }
    }
    for (_, mut p) in terms {
        p.kill();
    }
    log(&format!("администратор отключился ({})", from.ip()));
}

fn term_open(terms: &mut HashMap<String, Pty>, next: &mut u32, args: &Value, out: mpsc::Sender<Value>) -> Result<Value, String> {
    if terms.len() >= MAX_TERMS {
        return Err(format!("открыто уже {} терминала — закройте лишний", MAX_TERMS));
    }
    let (p, reader) = Pty::spawn(args["cols"].as_u64().unwrap_or(100) as u16, args["rows"].as_u64().unwrap_or(30) as u16)?;
    let tid = format!("t{}", next);
    *next += 1;
    let t2 = tid.clone();
    pty::read_loop(reader, move |chunk| {
        let v = if chunk.is_empty() { json!({ "ev": "term.exit", "tid": t2 }) } else { json!({ "ev": "term.data", "tid": t2, "data": b64().encode(chunk) }) };
        let _ = out.blocking_send(v);
    });
    terms.insert(tid.clone(), p);
    log(&format!("администратор открыл терминал {}", tid));
    Ok(json!({ "tid": tid }))
}

fn term_with(terms: &mut HashMap<String, Pty>, args: &Value, f: impl FnOnce(&mut Pty) -> Result<Value, String>) -> Result<Value, String> {
    let p = terms.get_mut(args["tid"].as_str().unwrap_or("")).ok_or("терминал закрыт")?;
    f(p)
}

fn sh_out(cmd: &str, args: &[&str]) -> Result<String, String> {
    let o = std::process::Command::new(cmd).args(args).output().map_err(|e| format!("{}: {}", cmd, e))?;
    let mut s = String::from_utf8_lossy(&o.stdout).to_string();
    if !o.status.success() {
        s.push_str(&String::from_utf8_lossy(&o.stderr));
    }
    Ok(s)
}

async fn op_call(srv: &Arc<Srv>, op: &str, args: &Value) -> Result<Value, String> {
    match op {
        "status" => Ok(status(srv)),
        "invite" => {
            let name = args["name"].as_str().unwrap_or("").trim().to_string();
            let _g = srv.join_lock.lock().await;
            let (line, expires) = make_invite(&srv.dir, &name)?;
            *srv.cfg.write() = load(&srv.dir)?;
            log(&format!("администратор выдал приглашение{}", if name.is_empty() { String::new() } else { format!(" для «{}»", name) }));
            Ok(json!({ "line": line, "expires": expires }))
        }
        "revoke" | "remove" | "restore" => {
            let name = args["name"].as_str().unwrap_or("").to_string();
            let _g = srv.join_lock.lock().await;
            let mut c = load(&srv.dir)?;
            let i = c.members.iter().position(|m| m.name == name).ok_or_else(|| format!("участника «{}» нет", name))?;
            match op {
                "revoke" => c.members[i].enabled = false,
                "restore" => c.members[i].enabled = true,
                _ => {
                    c.members.remove(i);
                }
            }
            save(&srv.dir, &c)?;
            *srv.cfg.write() = c;
            if op != "restore" {
                srv.turn.drop_who(&name);
                srv.net.drop_member(&name);
            }
            log(&format!("администратор: {} «{}»", match op { "revoke" => "отозван", "restore" => "возвращён", _ => "удалён" }, name));
            Ok(json!(true))
        }
        "settings" => {
            let c = srv.cfg.read().clone();
            Ok(json!({ "name": c.name, "publicIp": c.public_ip, "udpPort": c.udp_port, "tcpPorts": c.tcp_ports, "relayPorts": c.relay_ports, "maxPerMember": c.max_per_member, "netPort": c.net_port, "netFiles": c.net_files, "netMbit": c.net_mbit }))
        }
        "settings.set" => {
            let _g = srv.join_lock.lock().await;
            let mut c = load(&srv.dir)?;
            let mut restart = false;
            if let Some(n) = args["name"].as_str() {
                let n = n.trim();
                if n.is_empty() || n.chars().count() > 40 {
                    return Err("имя сервера — от 1 до 40 знаков".into());
                }
                c.name = n.to_string();
            }
            if let Some(m) = args["maxPerMember"].as_u64() {
                if !(2..=256).contains(&m) {
                    return Err("разговоров на участника — от 2 до 256".into());
                }
                c.max_per_member = m as usize;
                restart = true;
            }
            if let Some(f) = args["netFiles"].as_bool() {
                c.net_files = f;
            }
            if let Some(m) = args["netMbit"].as_u64() {
                if m > 10_000 {
                    return Err("ограничение — от 0 до 10000 Мбит/с".into());
                }
                c.net_mbit = m as u32;
            }
            if let Some(ip) = args["publicIp"].as_str() {
                let v: std::net::Ipv4Addr = ip.trim().parse().map_err(|_| format!("«{}» — не IPv4-адрес", ip))?;
                restart |= c.public_ip != v.to_string();
                c.public_ip = v.to_string();
            }
            save(&srv.dir, &c)?;
            *srv.cfg.write() = c;
            Ok(json!({ "restart": restart }))
        }
        "logs" => {
            let n = args["lines"].as_u64().unwrap_or(200).clamp(10, 2000).to_string();
            if std::path::Path::new("/run/systemd/system").exists() {
                sh_out("journalctl", &["-u", SERVICE, "-n", &n, "--no-pager", "-o", "short-iso"]).map(|t| json!({ "text": t }))
            } else {
                Ok(json!({ "text": "журнал systemd недоступен — сервер запущен не службой" }))
            }
        }
        "restart" => {
            if !std::path::Path::new("/run/systemd/system").exists() {
                return Err("сервер запущен не службой — перезапустите его вручную".into());
            }
            tokio::spawn(async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let _ = std::process::Command::new("systemctl").args(["restart", SERVICE]).spawn();
            });
            log("администратор перезапускает службу");
            Ok(json!(true))
        }
        "master.rotate" => {
            let _g = srv.join_lock.lock().await;
            let mut c = load(&srv.dir)?;
            c.master = proto::new_master();
            save(&srv.dir, &c)?;
            let line = bridge_core::relay::RelayInvite { host: c.public_ip.clone(), port: c.http_port(), code: c.master.clone() }.line();
            *srv.cfg.write() = c;
            log("администратор сменил мастер-ключ");
            // текущие каналы со старым ключом закроются после ответа
            let s2 = srv.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(800)).await;
                s2.rotate_epoch();
            });
            Ok(json!({ "line": line }))
        }
        _ => Err(format!("неизвестная команда «{}»", op)),
    }
}

fn status(srv: &Srv) -> Value {
    let c = srv.cfg.read().clone();
    let usage = srv.turn.usage();
    let seen = srv.seen.lock().clone();
    let now = chrono::Utc::now().timestamp();
    let online = srv.net.online();
    let members: Vec<Value> = c
        .members
        .iter()
        .map(|m| {
            let u = usage.get(&m.name).copied().unwrap_or_default();
            json!({ "name": m.name, "enabled": m.enabled, "created": m.created, "up": u.up, "down": u.down, "allocs": u.allocs, "lastSeen": seen.get(&m.name), "vip": m.vip, "net": online.get(&m.name) })
        })
        .collect();
    let service = if std::path::Path::new("/run/systemd/system").exists() { sh_out("systemctl", &["is-active", SERVICE]).unwrap_or_default().trim().to_string() } else { String::new() };
    json!({
        "name": c.name, "version": VERSION, "publicIp": c.public_ip, "udpPort": c.udp_port, "tcpPorts": c.tcp_ports, "netPort": c.net_port, "netFiles": c.net_files, "netMbit": c.net_mbit,
        "started": srv.started, "now": now_text(), "allocations": srv.turn.allocations(), "tcpConnections": srv.turn.tcp_connections(),
        "members": members, "invites": c.invites.iter().filter(|p| p.expires > now).count(), "service": service,
        "load": std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| s.split_whitespace().next().map(String::from)),
    })
}
