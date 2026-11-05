//! most-relay — свой сервер-ретранслятор для Моста (3.16).
//!
//! Один файл на Linux-сервере (VPS). Слушает TURN на UDP 3478 и TCP 3478/443; на TCP по первому байту отличает TURN
//! от HTTP — по HTTP мосты получают временные ключи TURN (`GET /relay/v1/hello`, подпись ключом участника — как между
//! соседями). Участники приходят по одноразовым приглашениям (`most-relay invite`: 5 минут, один раз) — мост меняет
//! приглашение на свой ключ участника (`POST /relay/v1/join`, ключ приходит зашифрованным). Отзыв —
//! `most-relay revoke <имя>`: новые разговоры у отозванного не начнутся, начатые обрываются.
//!
//! Сервер видит только «от кого и кому» и сколько байт: голос и экран шифрует сам WebRTC (DTLS-SRTP).
//!
//! Мастер-ключ (`most-relay master`) — для приложения «Сервер» в Мосте: управление участниками и настройками и терминал
//! root по шифрованному каналу (`admin.rs`, `proto.rs`). Им же можно подключить мост, как приглашением (многоразово).
//! Тот же файл под Windows работает службой приложения «Сервер» (`most-relay app-service`, `service.rs`).

mod admin;
mod client;
mod net;
mod proto;
mod pty;
mod service;

use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bridge_core::relay::{JoinRequest, JoinResponse, RelayHello, RelayInvite, HELLO_PATH, INVITE_TTL, JOIN_PATH};
use bridge_core::signing::{self, NonceCache};
use bridge_turn::{Opts, Turn};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio_util::sync::CancellationToken;

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
const CONFIG: &str = "relay.json";
const KEY: &str = "turn.key";
const STATS: &str = "stats.json";
/// Срок ключа TURN, который получает мост (он обновляет его заранее).
const CRED_TTL: Duration = Duration::from_secs(24 * 3600);
pub(crate) const SERVICE: &str = "most-relay";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Config {
    /// Имя сервера — видно в панели моста.
    name: String,
    /// Внешний IPv4 сервера: его сообщаем как адрес ретрансляции.
    public_ip: String,
    udp_port: u16,
    tcp_ports: Vec<u16>,
    /// Порты адресов ретрансляции (их надо открыть в брандмауэре по UDP).
    relay_ports: [u16; 2],
    /// Не больше одновременных размещений (≈ участников разговора через сервер) на одного участника.
    max_per_member: usize,
    members: Vec<Member>,
    /// Неиспользованные приглашения (хранится только хеш кода).
    invites: Vec<Pending>,
    /// Мастер-ключ: приложение «Сервер» (управление и терминал root) и подключение моста без одноразового приглашения.
    master: String,
    /// Порт своей сети MostNet (UDP); 0 — сеть выключена.
    net_port: u16,
    /// Разрешено ли мостам гнать файлы и синхронизацию через свою сеть (3.19). Мосты это соблюдают сами.
    net_files: bool,
    /// Ограничение скорости своей сети на участника, Мбит/с (0 — без ограничения) — его сервер соблюдает сам.
    net_mbit: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Pending {
    hash: String,
    /// До какого времени действует (unix, с).
    expires: i64,
    /// Имя участника, если задано при выдаче; иначе — имя ПК, которое сообщит мост.
    name: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { name: "most-relay".into(), public_ip: String::new(), udp_port: 3478, tcp_ports: vec![3478, 443], relay_ports: [49152, 65535], max_per_member: 32, members: Vec::new(), invites: Vec::new(), master: String::new(), net_port: bridge_core::mostnet::NET_PORT, net_files: true, net_mbit: 0 }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Member {
    name: String,
    secret: String,
    enabled: bool,
    created: String,
    /// Адрес в своей сети MostNet (10.77.x.y), выдаётся при первом подключении службы most-net.
    vip: String,
    /// Открытый ключ ПК в сети (X25519, base64).
    net_key: String,
}

impl Config {
    fn http_port(&self) -> u16 {
        if self.tcp_ports.contains(&443) {
            443
        } else {
            self.tcp_ports.first().copied().unwrap_or(443)
        }
    }

    fn prune(&mut self) {
        let now = chrono::Utc::now().timestamp();
        self.invites.retain(|p| p.expires > now);
    }
}

pub(crate) fn load(dir: &Path) -> Result<Config, String> {
    let f = dir.join(CONFIG);
    let text = std::fs::read_to_string(&f).map_err(|e| format!("нет {} ({}) — сначала: most-relay init", f.display(), e))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {}", f.display(), e))
}

pub(crate) fn save(dir: &Path, c: &Config) -> Result<(), String> {
    let f = dir.join(CONFIG);
    let tmp = dir.join(format!("{}.tmp", CONFIG));
    std::fs::write(&tmp, serde_json::to_string_pretty(c).unwrap()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // в файле — ключи участников и мастер-ключ: читать может только владелец
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &f).map_err(|e| e.to_string())
}

/// Новое одноразовое приглашение: (строка, до какого времени).
pub(crate) fn make_invite(dir: &Path, name: &str) -> Result<(String, i64), String> {
    if !name.is_empty() && !bridge_core::config::is_name(name) {
        return Err(format!("имя «{}» — латиница, цифры, «-» и «_»", name));
    }
    let mut c = load(dir)?;
    c.prune();
    let code = bridge_core::relay::new_code();
    let expires = chrono::Utc::now().timestamp() + INVITE_TTL;
    c.invites.push(Pending { hash: bridge_core::relay::code_hash(&code), expires, name: name.to_string() });
    save(dir, &c)?;
    Ok((RelayInvite { host: c.public_ip.clone(), port: c.http_port(), code }.line(), expires))
}

pub(crate) fn now_text() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

pub(crate) fn log(msg: &str) {
    println!("{} {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
}

/// Внешний IPv4: первый публичный адрес сети сервера; если таких нет — первый не-loopback (сервер за NAT: укажите --ip).
fn detect_ip() -> Option<(Ipv4Addr, bool)> {
    let mut private = None;
    for i in if_addrs::get_if_addrs().unwrap_or_default() {
        if let IpAddr::V4(v) = i.ip() {
            if v.is_loopback() || v.is_link_local() || v.is_unspecified() {
                continue;
            }
            if v.is_private() || v.octets()[0] == 100 && (v.octets()[1] & 0xC0) == 64 {
                private.get_or_insert(v);
                continue;
            }
            return Some((v, true));
        }
    }
    private.map(|v| (v, false))
}

// ---------- сервер ----------

pub(crate) struct Srv {
    dir: PathBuf,
    cfg: Arc<RwLock<Config>>,
    turn: Arc<Turn>,
    nonces: NonceCache,
    /// Когда участник последний раз брал ключ.
    seen: Mutex<HashMap<String, String>>,
    started: String,
    /// Неудачные попытки подключения по адресам: (сколько, с какого момента) — не больше 20 в минуту.
    tries: Mutex<HashMap<IpAddr, (u32, i64)>>,
    /// Одно подключение за раз: relay.json читается и пишется целиком.
    join_lock: tokio::sync::Mutex<()>,
    /// Каналы администратора закрываются, когда сменился мастер-ключ.
    admin_epoch: Mutex<CancellationToken>,
    /// Своя сеть MostNet: токены сеансов и куда слать каждому адресу.
    net: net::NetState,
}

impl Srv {
    fn rotate_epoch(&self) {
        let old = std::mem::replace(&mut *self.admin_epoch.lock(), CancellationToken::new());
        old.cancel();
    }
}

#[derive(Debug)]
pub(crate) struct Fail(StatusCode, String);

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

/// Кто прислал запрос: подпись ключом участника (как между соседями).
pub(crate) fn auth_member(srv: &Srv, headers: &HeaderMap, method: &str, path: &str, body_sha: &str) -> Result<Member, Fail> {
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let p = auth
        .strip_prefix(&format!("{} ", bridge_core::AUTH_SCHEME))
        .and_then(signing::parse)
        .ok_or_else(|| Fail(StatusCode::UNAUTHORIZED, "нужна подпись участника (строка приглашения most-relay invite)".into()))?;
    let member = {
        let c = srv.cfg.read();
        c.members.iter().find(|m| signing::key_id(&m.secret) == p.kid).cloned()
    };
    let Some(member) = member else { return Err(Fail(StatusCode::FORBIDDEN, "такого участника на сервере нет — попросите новое приглашение".into())) };
    if !member.enabled {
        return Err(Fail(StatusCode::FORBIDDEN, format!("участник «{}» отозван на сервере", member.name)));
    }
    signing::verify(&p, &member.secret, method, path, body_sha, &srv.nonces).map_err(|e| Fail(StatusCode::UNAUTHORIZED, e))?;
    Ok(member)
}

async fn hello(Extension(srv): Extension<Arc<Srv>>, Extension(from): Extension<SocketAddr>, headers: HeaderMap) -> Result<Json<RelayHello>, Fail> {
    let member = auth_member(&srv, &headers, "GET", HELLO_PATH, &signing::empty_body_sha())?;
    let (username, credential) = srv.turn.creds(&member.name, CRED_TTL);
    srv.seen.lock().insert(member.name.clone(), now_text());
    let c = srv.cfg.read();
    Ok(Json(RelayHello {
        name: c.name.clone(),
        version: VERSION.into(),
        host: c.public_ip.clone(),
        udp: c.udp_port,
        tcp: c.tcp_ports.clone(),
        username,
        credential,
        ttl: CRED_TTL.as_secs(),
        member: member.name,
        you: from.ip().to_string(),
        files: Some(c.net_files),
        net_mbit: c.net_mbit,
    }))
}

async fn join(Extension(srv): Extension<Arc<Srv>>, Extension(from): Extension<SocketAddr>, Json(req): Json<JoinRequest>) -> Result<Json<JoinResponse>, Fail> {
    let now = chrono::Utc::now().timestamp();
    {
        let mut t = srv.tries.lock();
        let e = t.entry(from.ip()).or_insert((0, now));
        if now - e.1 > 60 {
            *e = (0, now);
        }
        if e.0 >= 20 {
            return Err(Fail(StatusCode::TOO_MANY_REQUESTS, "слишком много попыток — подождите минуту".into()));
        }
    }
    let _g = srv.join_lock.lock().await;
    let mut c = load(&srv.dir).map_err(|e| Fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    c.prune();
    let h = bridge_core::relay::code_hash(&req.code);
    let by_master = !c.master.is_empty() && proto::same(&c.master, &req.code);
    let p = match c.invites.iter().position(|p| p.hash == h) {
        Some(i) => c.invites.remove(i),
        // мастер-ключ подключает многоразово
        None if by_master => Pending::default(),
        None => {
            srv.tries.lock().entry(from.ip()).or_insert((0, now)).0 += 1;
            return Err(Fail(StatusCode::FORBIDDEN, "приглашение не подходит: оно уже использовано, просрочено (живёт 5 минут) или набрано с ошибкой — получите новое: most-relay invite".into()));
        }
    };
    let name = if !p.name.is_empty() {
        p.name.clone()
    } else {
        let n: String = req.name.to_lowercase().chars().map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '-' }).collect();
        let n = n.trim_matches('-').chars().take(48).collect::<String>();
        if n.is_empty() {
            "pc".into()
        } else {
            n
        }
    };
    let secret = bridge_core::config::NodeConfig::new_secret();
    let (key, sealed) = bridge_core::relay::seal(&req.key, &req.code, &secret).map_err(|e| Fail(StatusCode::BAD_REQUEST, e))?;
    match c.members.iter_mut().find(|m| m.name == name) {
        // тот же ПК подключается заново (переустановка): новый ключ, старый больше не действует
        Some(m) => {
            m.secret = secret;
            m.enabled = true;
        }
        None => c.members.push(Member { name: name.clone(), secret, enabled: true, created: now_text(), ..Default::default() }),
    }
    save(&srv.dir, &c).map_err(|e| Fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let server = c.name.clone();
    *srv.cfg.write() = c;
    log(&format!("подключился участник «{}» ({})", name, from.ip()));
    Ok(Json(JoinResponse { member: name, server, key, sealed }))
}

async fn root() -> String {
    format!("most-relay {} — сервер-ретранслятор Моста. Подключение — строкой приглашения в панели моста.\n", VERSION)
}

fn router(srv: Arc<Srv>) -> Router {
    Router::new()
        .route("/", get(root))
        .route(HELLO_PATH, get(hello))
        .route(JOIN_PATH, post(join))
        .route(proto::ADMIN_PATH, get(admin::ws))
        .route(bridge_core::mostnet::JOIN_PATH, post(net::join))
        .route(bridge_core::mostnet::PEERS_PATH, get(net::peers))
        .route(bridge_core::mostnet::WS_PATH, get(net::ws))
        .layer(Extension(srv))
}

/// Одно TCP-соединение: первый байт < 0x40 — TURN (STUN), иначе — HTTP.
async fn tcp_conn(srv: Arc<Srv>, app: Router, s: TcpStream, local: SocketAddr, from: SocketAddr) {
    let mut b = [0u8; 1];
    match tokio::time::timeout(Duration::from_secs(10), s.peek(&mut b)).await {
        Ok(Ok(1)) => {}
        _ => return,
    }
    if b[0] < 0x40 {
        bridge_turn::serve_tcp_conn(srv.turn.clone(), s, local, from).await;
        return;
    }
    let svc = hyper_util::service::TowerToHyperService::new(app.layer(Extension(from)));
    // with_upgrades — для WebSocket администратора (он живёт долго, поэтому без общего срока на соединение)
    // keep_alive(false) нельзя: hyper добавит «Connection: upgrade, close», и клиенты WebSocket такой ответ не примут
    let conn = hyper::server::conn::http1::Builder::new()
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(Duration::from_secs(20))
        .serve_connection(hyper_util::rt::TokioIo::new(s), svc)
        .with_upgrades();
    let _ = conn.await;
}

async fn serve_tcp(srv: Arc<Srv>, l: TcpListener) {
    let Ok(local) = l.local_addr() else { return };
    let app = router(srv.clone());
    loop {
        let r = tokio::select! {
            _ = srv.turn.stop.cancelled() => break,
            r = l.accept() => r,
        };
        let Ok((s, from)) = r else { continue };
        let _ = s.set_nodelay(true);
        tokio::spawn(tcp_conn(srv.clone(), app.clone(), s, local, from));
    }
}

/// Перечитать relay.json при изменении (invite/revoke из другой консоли); отозванных — отключить сразу.
async fn watch_config(srv: Arc<Srv>) {
    let f = srv.dir.join(CONFIG);
    let mtime = |f: &Path| std::fs::metadata(f).and_then(|m| m.modified()).ok();
    let mut last: Option<SystemTime> = mtime(&f);
    loop {
        tokio::select! {
            _ = srv.turn.stop.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(3)) => {}
        }
        let now = mtime(&f);
        if now == last {
            continue;
        }
        last = now;
        match load(&srv.dir) {
            Ok(c) => {
                let (gone, master_changed): (Vec<String>, bool) = {
                    let old = srv.cfg.read();
                    (old.members.iter().filter(|m| m.enabled && !c.members.iter().any(|n| n.name == m.name && n.enabled)).map(|m| m.name.clone()).collect(), old.master != c.master)
                };
                *srv.cfg.write() = c;
                if master_changed {
                    srv.rotate_epoch();
                    log("мастер-ключ сменился — каналы администратора со старым ключом закрыты");
                }
                for g in gone {
                    srv.turn.drop_who(&g);
                    srv.net.drop_member(&g);
                    log(&format!("участник «{}» отозван — его разговоры через сервер закрыты", g));
                }
                log("relay.json перечитан");
            }
            Err(e) => log(&format!("relay.json не прочитан, работаю по-старому: {}", e)),
        }
    }
}

/// Раз в минуту — stats.json (для most-relay status и list).
async fn write_stats(srv: Arc<Srv>) {
    loop {
        tokio::select! {
            _ = srv.turn.stop.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(60)) => {}
        }
        let usage = srv.turn.usage();
        let seen = srv.seen.lock().clone();
        let mut members = serde_json::Map::new();
        let names: Vec<String> = srv.cfg.read().members.iter().map(|m| m.name.clone()).collect();
        for n in names {
            let u = usage.get(&n).copied().unwrap_or_default();
            members.insert(n.clone(), json!({ "up": u.up, "down": u.down, "allocs": u.allocs, "lastSeen": seen.get(&n) }));
        }
        let st = json!({ "updated": now_text(), "started": srv.started, "version": VERSION, "allocations": srv.turn.allocations(), "tcpConnections": srv.turn.tcp_connections(), "members": members });
        let _ = std::fs::write(srv.dir.join(STATS), serde_json::to_string_pretty(&st).unwrap());
    }
}

async fn run(dir: PathBuf) -> Result<(), String> {
    let cfg = load(&dir)?;
    let ip: IpAddr = cfg.public_ip.parse().map_err(|_| format!("publicIp «{}» в relay.json — не IP-адрес", cfg.public_ip))?;
    let key = match std::fs::read(dir.join(KEY)) {
        Ok(k) if k.len() >= 32 => k,
        _ => {
            let k: [u8; 32] = rand::random();
            std::fs::write(dir.join(KEY), k).map_err(|e| format!("{}: {}", dir.join(KEY).display(), e))?;
            k.to_vec()
        }
    };
    let cfg = Arc::new(RwLock::new(cfg));
    let c2 = cfg.clone();
    let allow: bridge_turn::AllowFn = Arc::new(move |who: &str| c2.read().members.iter().any(|m| m.name == who && m.enabled));
    let (udp_port, tcp_ports, rp, per) = {
        let c = cfg.read();
        (c.udp_port, c.tcp_ports.clone(), c.relay_ports, c.max_per_member)
    };
    let opts = Opts { relay_ip: Some(ip), relay_ports: Some((rp[0].min(rp[1]), rp[0].max(rp[1]))), max_allocs: 1024, max_per_who: per.max(2), allow: Some(allow) };
    let turn = Turn::new(key, opts, CancellationToken::new());
    let srv = Arc::new(Srv { dir: dir.clone(), cfg, turn: turn.clone(), nonces: NonceCache::default(), seen: Mutex::new(HashMap::new()), started: now_text(), tries: Mutex::new(HashMap::new()), join_lock: tokio::sync::Mutex::new(()), admin_epoch: Mutex::new(CancellationToken::new()), net: net::NetState::default() });
    let any = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    if udp_port != 0 {
        let s = UdpSocket::bind(SocketAddr::new(any, udp_port)).await.map_err(|e| format!("UDP {}: {}", udp_port, e))?;
        tokio::spawn(bridge_turn::serve_udp(turn.clone(), Arc::new(s)));
    }
    for p in &tcp_ports {
        let l = TcpListener::bind(SocketAddr::new(any, *p)).await.map_err(|e| {
            format!("TCP {}: {}{}", p, e, if *p < 1024 { " (порты ниже 1024 — от root или со службой: most-relay install)" } else { "" })
        })?;
        tokio::spawn(serve_tcp(srv.clone(), l));
    }
    let net_port = srv.cfg.read().net_port;
    if net_port != 0 {
        let s = UdpSocket::bind(SocketAddr::new(any, net_port)).await.map_err(|e| format!("UDP {} (сеть MostNet): {}", net_port, e))?;
        tokio::spawn(net::serve_udp(srv.clone(), Arc::new(s)));
    }
    tokio::spawn(watch_config(srv.clone()));
    tokio::spawn(write_stats(srv.clone()));
    {
        let c = srv.cfg.read();
        log(&format!(
            "most-relay {} «{}»: адрес {}, TURN UDP {}, TCP {:?}, ретрансляция UDP {}–{}, участников {}",
            VERSION,
            c.name,
            c.public_ip,
            udp_port,
            tcp_ports,
            rp[0],
            rp[1],
            c.members.iter().filter(|m| m.enabled).count()
        ));
    }
    let stop = turn.stop.clone();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term() => {}
    }
    stop.cancel();
    log("остановлен");
    Ok(())
}

#[cfg(unix)]
async fn term() {
    if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        s.recv().await;
    } else {
        std::future::pending::<()>().await;
    }
}

#[cfg(not(unix))]
async fn term() {
    std::future::pending::<()>().await;
}

// ---------- команды ----------

fn opt<'a>(args: &'a [String], k: &str) -> Option<&'a str> {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

fn pos(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i].starts_with("--") {
            i += 2;
            continue;
        }
        out.push(args[i].as_str());
        i += 1;
    }
    out
}

fn default_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| PathBuf::from("."))
}

fn mb(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} ГБ", b as f64 / (1u64 << 30) as f64)
    } else {
        format!("{:.1} МБ", b as f64 / (1u64 << 20) as f64)
    }
}

const HELP: &str = "most-relay — свой сервер-ретранслятор для Моста

  most-relay init [--ip <внешний IP>] [--name <имя>]   создать relay.json рядом с программой
  sudo most-relay install                              служба systemd (автозапуск) и правила ufw
  most-relay invite [имя]                              одноразовое приглашение (5 минут); имя — по умолчанию имя ПК
  most-relay revoke <имя>                              отозвать участника (разговоры через сервер у него закроются)
  most-relay master [--new]                            мастер-ключ для приложения «Сервер» в Мосте (--new — сменить)
  most-relay list                                      участники и их трафик
  most-relay status                                    что делает сервер сейчас
  most-relay run                                       запустить в этой консоли (служба делает то же сама)

  --dir <папка>   где relay.json (по умолчанию — рядом с программой)

Строку приглашения вставьте в панели моста: «Соседи» → «Свой сервер-ретранслятор» (в течение 5 минут).
Порты: UDP 3478, TCP 3478 и 443, UDP 49152–65535 (адреса ретрансляции).";

fn cmd_init(dir: &Path, args: &[String]) -> Result<(), String> {
    if dir.join(CONFIG).exists() {
        return Err(format!("уже есть {} — участники: most-relay invite <имя>", dir.join(CONFIG).display()));
    }
    let mut c = Config::default();
    match opt(args, "--ip") {
        Some(ip) => {
            let v: Ipv4Addr = ip.parse().map_err(|_| format!("«{}» — не IPv4-адрес", ip))?;
            c.public_ip = v.to_string();
        }
        None => match detect_ip() {
            Some((v, true)) => c.public_ip = v.to_string(),
            Some((v, false)) => {
                c.public_ip = v.to_string();
                eprintln!("Внимание: у сервера только внутренний адрес {} (он за NAT). Узнайте внешний IP в панели хостинга и пересоздайте: most-relay init --ip <IP>", v);
            }
            None => return Err("не нашёл сетевой адрес — укажите: most-relay init --ip <внешний IP>".into()),
        },
    }
    if let Some(n) = opt(args, "--name") {
        c.name = n.to_string();
    }
    c.master = proto::new_master();
    save(dir, &c)?;
    let ml = RelayInvite { host: c.public_ip.clone(), port: c.http_port(), code: c.master.clone() }.line();
    println!("Создан {}\nВнешний адрес: {}\n\nМастер-ключ — для приложения «Сервер» в Мосте (управление и терминал root). Берегите его как пароль root:\n\n    {}\n\nДальше:\n  sudo {} install        — служба и брандмауэр\n  {} invite               — приглашение для моста (5 минут, один раз)\n  {} master               — показать мастер-ключ снова (--new — сменить)", dir.join(CONFIG).display(), c.public_ip, ml, exe_name(), exe_name(), exe_name());
    Ok(())
}

fn exe_name() -> String {
    std::env::current_exe().ok().map(|p| p.display().to_string()).unwrap_or_else(|| "./most-relay".into())
}

fn cmd_invite(dir: &Path, args: &[String]) -> Result<(), String> {
    let p = pos(args);
    let name = p.get(1).copied().unwrap_or("");
    let (line, _) = make_invite(dir, name)?;
    let who = if name.is_empty() { "под именем ПК".to_string() } else { format!("как «{}»", name) };
    println!("Приглашение {} — действует 5 минут и срабатывает один раз:\n\n    {}\n\nВставьте строку в панели моста: «Соседи» → «Свой сервер-ретранслятор» → «Подключить».\nМост получит свой постоянный ключ сам — строку потом хранить не нужно.", who, line);
    Ok(())
}

fn cmd_master(dir: &Path, args: &[String]) -> Result<(), String> {
    let mut c = load(dir)?;
    let fresh = args.iter().any(|a| a == "--new") || c.master.is_empty();
    if fresh {
        c.master = proto::new_master();
        save(dir, &c)?;
    }
    let line = RelayInvite { host: c.public_ip.clone(), port: c.http_port(), code: c.master.clone() }.line();
    println!(
        "{}Мастер-ключ — вставьте в приложение «Сервер» в Мосте (управление, терминал root; подключает и сам мост):\n\n    {}\n\nБерегите его как пароль root. Сменить: most-relay master --new (старый сразу перестанет действовать).",
        if fresh { "Создан новый мастер-ключ. " } else { "" },
        line
    );
    Ok(())
}

fn cmd_revoke(dir: &Path, args: &[String]) -> Result<(), String> {
    let p = pos(args);
    let name = p.get(1).ok_or("most-relay revoke <имя>")?;
    let mut c = load(dir)?;
    let m = c.members.iter_mut().find(|m| m.name == *name).ok_or_else(|| format!("участника «{}» нет", name))?;
    m.enabled = false;
    save(dir, &c)?;
    println!("«{}» отозван. Работающий сервер подхватит это за несколько секунд. Вернуть — новое приглашение: most-relay invite {}", name, name);
    Ok(())
}

fn stats(dir: &Path) -> serde_json::Value {
    std::fs::read_to_string(dir.join(STATS)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::Value::Null)
}

fn cmd_list(dir: &Path) -> Result<(), String> {
    let c = load(dir)?;
    let st = stats(dir);
    if c.members.is_empty() {
        println!("Участников нет — most-relay invite");
    }
    let now = chrono::Utc::now().timestamp();
    let open = c.invites.iter().filter(|p| p.expires > now).count();
    if open > 0 {
        println!("Неиспользованных приглашений: {} (каждое живёт 5 минут)", open);
    }
    for m in &c.members {
        let s = &st["members"][&m.name];
        let up = s["up"].as_u64().unwrap_or(0);
        let down = s["down"].as_u64().unwrap_or(0);
        println!(
            "{} {:<20} {}  трафик с запуска: {} к серверу, {} от сервера{}",
            if m.enabled { "✓" } else { "✗" },
            m.name,
            if m.enabled { "действует" } else { "отозван" },
            mb(up),
            mb(down),
            s["lastSeen"].as_str().map(|t| format!(", последний раз: {}", t)).unwrap_or_default()
        );
    }
    Ok(())
}

fn cmd_status(dir: &Path) -> Result<(), String> {
    let c = load(dir)?;
    let st = stats(dir);
    println!("Сервер «{}», адрес {}, TURN UDP {}, TCP {:?}", c.name, c.public_ip, c.udp_port, c.tcp_ports);
    println!("Участников: {} (отозвано: {})", c.members.iter().filter(|m| m.enabled).count(), c.members.iter().filter(|m| !m.enabled).count());
    if st.is_null() {
        println!("Статистики нет — сервер не запущен или работает меньше минуты.");
    } else {
        println!("Запущен: {}, обновлено: {}", st["started"].as_str().unwrap_or("?"), st["updated"].as_str().unwrap_or("?"));
        println!("Сейчас разговоров через сервер (размещений): {}, TCP-соединений: {}", st["allocations"], st["tcpConnections"]);
    }
    if Path::new("/run/systemd/system").exists() {
        let out = std::process::Command::new("systemctl").args(["is-active", SERVICE]).output();
        if let Ok(o) = out {
            println!("Служба {}: {}", SERVICE, String::from_utf8_lossy(&o.stdout).trim());
        }
    }
    Ok(())
}

fn sh(cmd: &str, args: &[&str]) -> Result<String, String> {
    let o = std::process::Command::new(cmd).args(args).output().map_err(|e| format!("{}: {}", cmd, e))?;
    if !o.status.success() {
        return Err(format!("{} {}: {}", cmd, args.join(" "), String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&o.stdout).to_string())
}

fn cmd_install(dir: &Path) -> Result<(), String> {
    let c = load(dir)?;
    if sh("id", &["-u"]).map(|s| s.trim() != "0").unwrap_or(true) {
        return Err("нужны права root: sudo most-relay install".into());
    }
    if !Path::new("/run/systemd/system").exists() {
        return Err("systemd не найден — запускайте most-relay run сами (например, в screen/tmux)".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = std::fs::canonicalize(dir).map_err(|e| e.to_string())?;
    if dir.starts_with("/tmp") {
        return Err("папка /tmp очищается — положите программу в /opt/most-relay (см. relay/README.md) и запустите install оттуда".into());
    }
    let unit = format!(
        "[Unit]\nDescription=most-relay — сервер-ретранслятор Моста\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nUser=root\nExecStart={0} run --dir {1}\nWorkingDirectory={1}\nRestart=always\nRestartSec=3\nLimitNOFILE=65536\n\n[Install]\nWantedBy=multi-user.target\n",
        exe.display(),
        dir.display()
    );
    std::fs::write(format!("/etc/systemd/system/{}.service", SERVICE), unit).map_err(|e| e.to_string())?;
    sh("systemctl", &["daemon-reload"])?;
    sh("systemctl", &["enable", "--now", SERVICE])?;
    sh("systemctl", &["restart", SERVICE])?;
    println!("✓ служба {} запущена и включена при загрузке (журнал: journalctl -u {} -f)", SERVICE, SERVICE);
    let ufw = sh("ufw", &["status"]).unwrap_or_default();
    let rules: Vec<String> = {
        let mut r = vec![format!("{}/udp", c.udp_port), format!("{}:{}/udp", c.relay_ports[0], c.relay_ports[1])];
        if c.net_port != 0 {
            r.push(format!("{}/udp", c.net_port));
        }
        r.extend(c.tcp_ports.iter().map(|p| format!("{}/tcp", p)));
        r
    };
    if ufw.contains("Status: active") {
        for r in &rules {
            sh("ufw", &["allow", r])?;
        }
        println!("✓ брандмауэр ufw: открыто {}", rules.join(", "));
    } else {
        println!("ufw не включён — ничего не трогал. Если у хостинга свой брандмауэр (в панели), откройте там: {}", rules.join(", "));
    }
    println!("\nДальше: {} invite — приглашение для моста (5 минут, один раз).", exe.display());
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = opt(&args, "--dir").map(PathBuf::from).unwrap_or_else(default_dir);
    let cmd = pos(&args).first().copied().unwrap_or("help").to_string();
    let res = match cmd.as_str() {
        "init" => cmd_init(&dir, &args),
        "invite" => cmd_invite(&dir, &args),
        "master" => cmd_master(&dir, &args),
        "app-service" => {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio");
            rt.block_on(service::run())
        }
        "revoke" => cmd_revoke(&dir, &args),
        "list" => cmd_list(&dir),
        "status" => cmd_status(&dir),
        "install" => cmd_install(&dir),
        "version" | "--version" => {
            println!("most-relay {}", VERSION);
            Ok(())
        }
        "run" => {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio");
            rt.block_on(run(dir))
        }
        _ => {
            println!("{}", HELP);
            Ok(())
        }
    };
    if let Err(e) = res {
        eprintln!("Ошибка: {}", e);
        std::process::exit(1);
    }
}
