//! Узел: состояние, запуск, главный цикл, журнал, сохранение настроек.
//! Главный цикл просыпается по событиям (изменились файлы, сосед сообщил «у меня новое») или раз в intervalSec —
//! в покое узел почти не тратит процессор: папки не обходятся, пока в них ничего не меняется.

use crate::client::PeerClient;
use crate::folders::Folders;
use crate::mailbox::Mailbox;
use crate::media::Media;
use crate::runner::Runner;
use crate::shares::ShareRt;
use crate::watch::Watcher;
use bridge_core::config::{NodeConfig, PeerConfig};
use bridge_core::proto::{NodeStatus, PeerView};
use bridge_core::signing::NonceCache;
use bridge_core::time::{self, Time};
use bridge_core::{eq_ic, APP_VERSION};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Notify};
use tokio_util::sync::CancellationToken;

/// Состояние соседа глазами этого узла.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerState {
    pub name: String,
    pub online: bool,
    #[serde(with = "time::opt")]
    pub last_seen: Option<Time>,
    pub error: String,
    pub hint: String,
    pub warning: String,
    pub announced_name: String,
    pub version: String,
    pub legacy: bool,
    pub remote: Option<PeerView>,
    #[serde(with = "time::opt")]
    pub last_inbound: Option<Time>,
    pub inbound_error: String,
    pub inbound_legacy: bool,
    #[serde(skip)]
    pub last_error: String,
    pub status: Option<NodeStatus>,
    pub latency_ms: Option<i64>,
    pub files_in: i64,
    pub bytes_in: i64,
    /// Агент соседа для нас: auto, manual или пусто.
    pub agent: String,
    /// Каналы соседа, куда можно зайти (из его привета).
    #[serde(skip)]
    pub channels: Vec<bridge_core::proto::ChannelInfo>,
    /// Ретранслятор голоса соседа для нас (из его привета).
    #[serde(skip)]
    pub turn: Option<bridge_core::proto::TurnInfo>,
    /// Свой сервер соседа, которым он делится (3.16).
    pub relay: Option<bridge_core::relay::RelayInfo>,
    /// Через какой адрес сейчас связь (с 3.19.2 — всегда; `via_main` — это основной адрес).
    pub via: String,
    pub via_main: bool,
    /// Через какой адрес идут файлы, если через сервер (3.19).
    pub files_via: String,
    /// Сосед просит гонять файлы через сервер (его галочка, 3.19).
    #[serde(skip)]
    pub wants_bulk: bool,
    /// (3.21) Что у соседа распаковалось из наших сборок (из его привета).
    #[serde(skip)]
    pub builds: Vec<bridge_core::proto::BuildReport>,
    /// Ассистент живёт у соседа (из его привета).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant: Option<bridge_core::proto::AssistantInfo>,
    /// Исполнители соседа, которые нам можно запускать (из его привета).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub runners: Vec<bridge_core::proto::RunnerInfo>,
    /// Отпечаток аватарки соседа.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub avatar: String,
    /// Отпечаток звука входа соседа.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sound: String,
    /// Отпечаток звука пинга соседа.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ping: String,
    /// Отпечаток плитки соседа.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tile: String,
    /// Пространства, которые сосед открыл этому ПК (из его привета).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shares: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StrangerInfo {
    pub name: String,
    pub ip: String,
    pub why: String,
    pub kid: String,
    #[serde(with = "time::ser")]
    pub time: Time,
}

pub struct StartOptions {
    pub base_dir: PathBuf,
    pub config_file: Option<PathBuf>,
    /// Писать журнал в консоль.
    pub console: bool,
    /// Свой обработчик журнала (тесты).
    pub log: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    /// Время «устоявшегося» файла, с (тесты — 0).
    pub settle_secs: f64,
    /// Опрашивать соседей по таймеру (тесты — нет: обмен только явно).
    pub timer: bool,
}

impl Default for StartOptions {
    fn default() -> Self {
        StartOptions { base_dir: PathBuf::from("."), config_file: None, console: true, log: None, settle_secs: 2.0, timer: true }
    }
}

pub struct Node {
    pub cfg: RwLock<NodeConfig>,
    pub config_file: Option<PathBuf>,
    pub base_dir: PathBuf,
    pub data_dir: PathBuf,
    pub local_token: String,
    pub urls: RwLock<Vec<String>>,
    /// Ход передач файлов (для панели).
    pub transfers: Arc<crate::progress::Transfers>,
    /// Агент: поручения Claude между ПК.
    pub agent: crate::agent::Agent,
    pub limits: Arc<crate::progress::Limits>,
    /// Адреса, на которых уже слушаем, и где не вышло (чтобы не повторять в журнале).
    pub bound: Mutex<std::collections::HashSet<std::net::IpAddr>>,
    pub bind_failed: Mutex<std::collections::HashSet<std::net::IpAddr>>,
    last_bind_check: Mutex<Instant>,
    pub started: Time,
    pub peers: Mutex<BTreeMap<String, PeerState>>,
    pub shares: RwLock<Vec<Arc<ShareRt>>>,
    /// Файловые каналы efc/1: пространства и канал агента.
    pub efc: crate::efc_chan::Channels,
    pub strangers: Mutex<BTreeMap<String, StrangerInfo>>,
    pub nonces: NonceCache,
    clients: Mutex<HashMap<String, (String, Arc<PeerClient>)>>,
    /// Каким адресом соседа сейчас пользуемся, если не основным (3.17.1: основной не отвечает — запасной).
    active_url: Mutex<HashMap<String, String>>,
    /// Когда последний раз проверяли, не ожил ли основной адрес.
    primary_try: Mutex<HashMap<String, Instant>>,
    /// Какую дату использования уже записали для (сосед, адрес) — чтобы писать bridge.json не чаще раза в сутки.
    ip_marked: Mutex<HashMap<(String, String), String>>,
    /// Своя сеть на этом ПК работает (служба most-net отвечает «on») — можно гнать файлы через сервер.
    pub mostnet_up: std::sync::atomic::AtomicBool,
    last_ip_prune: Mutex<Option<Instant>>,
    once: Mutex<HashMap<String, Instant>>,
    pub mailbox: Mailbox,
    pub builds: crate::builds::Builds,
    pub apps: crate::apps::Apps,
    pub net: crate::net::Net,
    pub activity: crate::activity::Activity,
    pub share: crate::app_share::AppShare,
    pub identity: crate::identity::Identity,
    pub folders: Folders,
    pub runner: Runner,
    pub media: Media,
    /// Телефоны: подключённые устройства, коды подключения, Tailscale.
    pub mobile: crate::mobile::Mobile,
    /// Ретранслятор голоса (TURN).
    pub turn: crate::turn::Turn,
    /// Свой сервер-ретранслятор (most-relay): ключ с сервера и состояние.
    pub relay: crate::relay::Relay,
    /// Связь с соседями через свой сервер (3.20).
    pub tunnel: crate::tunnel::Tunnel,
    /// Ассистент: очередь входящих (на хозяине) и чат панели.
    pub assistant: crate::assistant::Assistant,
    /// Чаты: личные и каналов (сутки).
    pub chat: crate::chat::Chat,
    /// Обновление моста от соседа.
    pub updates: crate::update::Updates,
    /// Доски каналов.
    pub boards: crate::board::Boards,
    /// Управление этим ПК по разрешению и «заглушили».
    pub remote: crate::remote::Remote,
    pub wake: Notify,
    /// Строки журнала (для окна и панели).
    pub log_tx: broadcast::Sender<String>,
    log_hook: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    console: bool,
    log_lock: Mutex<()>,
    pub shutdown: CancellationToken,
    pub settle_secs: f64,
    pub timer: bool,
    pub config_lock: Mutex<()>,
    pub watcher: Mutex<Option<Watcher>>,
    pub sync_requested: AtomicBool,
    /// (3.21) Проверка связи со всеми идёт в фоне — обмен с теми, кто ответил, не ждёт молчащих.
    contacting: Arc<AtomicBool>,
    /// (3.21) Когда последний раз проверяли связь с соседем, постучавшимся к нам «из офлайна».
    probes: Mutex<std::collections::HashMap<String, Instant>>,
    last_round: Mutex<Option<Instant>>,
    last_full_scan: Mutex<Instant>,
    /// Последнее событие файловой системы: изменения обрабатываются, когда в папках «стихло» (удаление тысячи файлов —
    /// одно изменение, а не тысяча обменов; защита от массового удаления видит его целиком).
    pub last_fs_event: Mutex<Instant>,
    /// Входящие события для ассистентов и панели (сосед на связи, пришёл файл…).
    pub events_tx: broadcast::Sender<serde_json::Value>,
}

impl Node {
    /// Запустить узел: прочитать пространства, поднять HTTP, начать цикл.
    pub async fn start(cfg: NodeConfig, opt: StartOptions) -> Result<Arc<Node>, String> {
        let errors = cfg.validate();
        if !errors.is_empty() {
            return Err(format!("Ошибки настроек:\n- {}", errors.join("\n- ")));
        }
        let base_dir = plain_path(std::fs::canonicalize(&opt.base_dir).unwrap_or(opt.base_dir.clone()));
        let mut cfg = cfg;
        // 512 МБ — прежний предел по умолчанию: теперь по умолчанию без предела
        let lifted = cfg.max_file_mb == 512;
        if lifted {
            cfg.max_file_mb = 0;
        }
        let data_dir = base_dir.join(&cfg.data_dir);
        std::fs::create_dir_all(&data_dir).map_err(|e| format!("Папка данных {}: {}", data_dir.display(), e))?;
        crate::pack::clean_tmp(&data_dir.join("tmp"));
        // 3.13: прежние русские имена папок → английские (до того, как открыты папки и пространства)
        let renamed = crate::rename::migrate(&base_dir, &data_dir, &mut cfg);
        let token = load_or_create_token(&data_dir);
        let (log_tx, _) = broadcast::channel(256);
        let (events_tx, _) = broadcast::channel(256);
        let node = Arc::new(Node {
            cfg: RwLock::new(cfg),
            config_file: opt.config_file.clone(),
            base_dir: base_dir.clone(),
            data_dir: data_dir.clone(),
            local_token: token,
            urls: RwLock::new(Vec::new()),
            transfers: Arc::new(crate::progress::Transfers::default()),
            agent: crate::agent::Agent::new(&data_dir),
            limits: Arc::new(crate::progress::Limits::default()),
            bound: Mutex::new(Default::default()),
            bind_failed: Mutex::new(Default::default()),
            last_bind_check: Mutex::new(Instant::now()),
            started: time::now(),
            peers: Mutex::new(BTreeMap::new()),
            shares: RwLock::new(Vec::new()),
            efc: Default::default(),
            strangers: Mutex::new(BTreeMap::new()),
            nonces: NonceCache::default(),
            clients: Mutex::new(HashMap::new()),
            active_url: Mutex::new(HashMap::new()),
            primary_try: Mutex::new(HashMap::new()),
            ip_marked: Mutex::new(HashMap::new()),
            mostnet_up: std::sync::atomic::AtomicBool::new(false),
            last_ip_prune: Mutex::new(None),
            once: Mutex::new(HashMap::new()),
            mailbox: Mailbox::new(),
            builds: Default::default(),
            apps: Default::default(),
            net: Default::default(),
            activity: Default::default(),
            share: Default::default(),
            identity: Default::default(),
            folders: Folders::new(&data_dir),
            runner: Runner::new(),
            media: Media::new(),
            mobile: Default::default(),
            turn: Default::default(),
            relay: Default::default(),
            tunnel: Default::default(),
            assistant: Default::default(),
            chat: Default::default(),
            updates: Default::default(),
            boards: Default::default(),
            remote: Default::default(),
            wake: Notify::new(),
            log_tx,
            log_hook: opt.log.clone(),
            console: opt.console,
            log_lock: Mutex::new(()),
            shutdown: CancellationToken::new(),
            settle_secs: opt.settle_secs,
            timer: opt.timer,
            config_lock: Mutex::new(()),
            watcher: Mutex::new(None),
            sync_requested: AtomicBool::new(false),
            contacting: Arc::new(AtomicBool::new(false)),
            probes: Mutex::new(Default::default()),
            last_round: Mutex::new(None),
            last_full_scan: Mutex::new(Instant::now()),
            last_fs_event: Mutex::new(Instant::now() - Duration::from_secs(60)),
            events_tx,
        });
        {
            let cfg = node.cfg.read().clone();
            let mut peers = node.peers.lock();
            for p in &cfg.peers {
                peers.insert(p.name.clone(), PeerState { name: p.name.clone(), ..Default::default() });
            }
            drop(peers);
            for s in &cfg.shares {
                node.open_share(s);
            }
        }
        node.apply_limits();
        for l in &renamed.lines {
            node.info(&format!("Имена папок (3.13): {}", l));
        }
        if renamed.config_changed {
            if let Some(e) = node.save_config() {
                node.info(&format!("Имена папок (3.13): настройки не сохранены — {}", e));
            }
        }
        if lifted {
            let e = node.save_config();
            node.info(&format!("Предел размера файла снят (было 512 МБ — прежнее значение по умолчанию){}", e.map(|e| format!(" — не сохранено: {}", e)).unwrap_or_default()));
        }
        node.mailbox.prepare(&node);
        node.folders.load(&node);
        node.agent.load();
        node.mobile.load(&node);
        node.assistant.load(&node);
        crate::assistant::schedule(&node);
        node.chat.load(&node);
        crate::chat::schedule(&node);
        crate::update::cleanup(&node);
        crate::board::schedule(&node);
        crate::mobile::schedule(&node);
        crate::apps::schedule(&node);
        crate::net::schedule(&node);
        tokio::spawn(crate::agent::pusher(node.clone()));
        crate::agent::schedule(&node);
        for f in node.cfg.read().folders.clone() {
            let _ = std::fs::create_dir_all(node.resolve(&f.path));
        }
        crate::http_peer::start_http(&node).await?;
        node.turn.load(&node);
        crate::turn::bind_all(&node).await;
        crate::relay::schedule(&node);
        crate::mostnet::schedule(&node);
        crate::tunnel::schedule(&node);
        node.runner.recover(&node);
        *node.watcher.lock() = Watcher::start(&node);
        let n2 = node.clone();
        tokio::spawn(async move { n2.main_loop().await });
        let (name, urls, sh, pe) = {
            let c = node.cfg.read();
            (c.node.clone(), node.urls.read().join(", "), c.shares.len(), c.peers.len())
        };
        node.info(&format!("Узел «{}» {} слушает {}; пространств: {}, соседей: {}", name, APP_VERSION, urls, sh, pe));
        Ok(node)
    }

    pub fn open_share(self: &Arc<Self>, s: &bridge_core::config::ShareConfig) {
        let rt = ShareRt::open(self, s);
        self.shares.write().push(Arc::new(rt));
    }

    pub fn share(&self, id: &str) -> Option<Arc<ShareRt>> {
        self.shares.read().iter().find(|s| eq_ic(&s.id, id)).cloned()
    }

    pub fn shares(&self) -> Vec<Arc<ShareRt>> {
        self.shares.read().clone()
    }

    pub fn resolve(&self, p: &str) -> PathBuf {
        let path = Path::new(p);
        if path.is_absolute() || (p.len() > 2 && p.as_bytes()[1] == b':') {
            path.to_path_buf()
        } else {
            self.base_dir.join(path)
        }
    }

    pub fn tmp_file(&self) -> PathBuf {
        let d = self.data_dir.join("tmp");
        let _ = std::fs::create_dir_all(&d);
        d.join(uuid::Uuid::new_v4().simple().to_string())
    }

    /// Предел размера файла в байтах (0 в настройках — без предела).
    pub fn max_file_bytes(&self) -> i64 {
        let mb = self.cfg.read().max_file_mb;
        if mb <= 0 {
            i64::MAX
        } else {
            mb * 1024 * 1024
        }
    }

    pub fn apply_limits(&self) {
        let c = self.cfg.read();
        self.limits.up.set_mbit(c.max_up_mbit);
        self.limits.down.set_mbit(c.max_down_mbit);
    }

    pub fn node_name(&self) -> String {
        self.cfg.read().node.clone()
    }

    pub fn dashboard_url(&self) -> String {
        let urls = self.urls.read();
        let u = urls.iter().find(|u| u.contains("127.0.0.1")).or(urls.first()).cloned().unwrap_or_else(|| "http://127.0.0.1:5390".into());
        format!("{}/", u.trim_end_matches('/'))
    }

    // ───────────── Журнал ─────────────

    pub fn info(&self, text: &str) {
        let line = format!("{}  {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), text);
        if let Some(h) = &self.log_hook {
            h(text);
        } else {
            let _g = self.log_lock.lock();
            if self.console {
                println!("{}", line);
            }
            let file = self.data_dir.join("log.txt");
            if std::fs::metadata(&file).map(|m| m.len() > 1_000_000).unwrap_or(false) {
                let _ = std::fs::rename(&file, self.data_dir.join("log.txt.1"));
            }
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
                let _ = writeln!(f, "{}", line);
            }
        }
        let _ = self.log_tx.send(line);
    }

    /// Записать не чаще раза в `minutes` минут на ключ.
    pub fn info_once(&self, key: &str, text: &str, minutes: u64) {
        {
            let mut once = self.once.lock();
            if let Some(t) = once.get(key) {
                if t.elapsed() < Duration::from_secs(minutes * 60) {
                    return;
                }
            }
            if once.len() > 500 {
                once.clear();
            }
            once.insert(key.to_string(), Instant::now());
        }
        self.info(text);
    }

    /// Событие для ассистента и панели (поток /local/events).
    pub fn emit(&self, kind: &str, data: serde_json::Value) {
        let _ = self.events_tx.send(serde_json::json!({ "kind": kind, "time": time::format(&time::now()), "data": data }));
    }

    // ───────────── Настройки ─────────────

    /// Записать bridge.json (прежний — в bridge.json.bak). None — успешно.
    pub fn save_config(&self) -> Option<String> {
        let Some(file) = &self.config_file else { return Some("узел запущен без файла настроек".into()) };
        // правила могли измениться — отпечатки соседей больше не значат «всё забрано»
        for rt in self.shares() {
            rt.pulled_rev.lock().clear();
        }
        let text = bridge_core::to_json(&*self.cfg.read());
        if file.exists() {
            let _ = std::fs::copy(file, file.with_extension("json.bak"));
        }
        bridge_core::write_atomic(file, &text).err().map(|e| e.to_string())
    }

    pub fn peer_state(&self, name: &str) -> PeerState {
        self.peers.lock().get(name).cloned().unwrap_or_else(|| PeerState { name: name.into(), ..Default::default() })
    }

    pub fn with_peer_state<R>(&self, name: &str, f: impl FnOnce(&mut PeerState) -> R) -> R {
        let mut g = self.peers.lock();
        let st = g.entry(name.to_string()).or_insert_with(|| PeerState { name: name.into(), ..Default::default() });
        f(st)
    }

    // ───────────── Клиенты соседей ─────────────

    /// Все адреса соседа по порядку: основной, запасной из настроек, его адреса в своей сети (10.77.x.y) из привета.
    pub fn peer_urls(&self, peer: &PeerConfig) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        if !peer.url.is_empty() {
            v.push(peer.url.trim_end_matches('/').to_string());
        }
        let port = bridge_core::config::parse_url(&peer.url).map(|(_, p)| p).unwrap_or(5390);
        if !peer.alt.trim().is_empty() {
            if let Ok((h, p)) = bridge_core::config::parse_address(peer.alt.trim().trim_start_matches("http://").trim_end_matches('/'), port) {
                v.push(format!("http://{}:{}", h, p));
            }
        }
        for ip in &peer.ips {
            if let Ok(a) = ip.parse::<std::net::Ipv4Addr>() {
                if bridge_core::mostnet::in_net(a) {
                    v.push(format!("http://{}:{}", a, port));
                }
            }
        }
        // (3.20) последним — через свой сервер, если сосед на том же сервере
        if let Some(u) = self.tunnel.url_for(self, peer) {
            v.push(u);
        }
        let mut seen = std::collections::HashSet::new();
        v.retain(|u| seen.insert(u.clone()));
        v
    }

    /// Адрес соседа, которым сейчас пользуемся (основной, пока не переключились на запасной).
    pub fn peer_url(&self, peer: &PeerConfig) -> String {
        let cur = self.active_url.lock().get(&peer.name).cloned();
        match cur {
            Some(u) if self.peer_urls(peer).contains(&u) => u,
            _ => peer.url.clone(),
        }
    }

    /// Пользоваться другим адресом соседа (None — основным).
    pub fn set_peer_url(&self, name: &str, url: Option<String>) {
        match url {
            Some(u) => self.active_url.lock().insert(name.to_string(), u),
            None => self.active_url.lock().remove(name),
        };
        self.forget_client(name);
    }

    /// Пора ли снова попробовать основной адрес (раз в 2 минуты, пока работаем через запасной).
    pub fn primary_due(&self, name: &str) -> bool {
        let mut g = self.primary_try.lock();
        let due = g.get(name).map(|t| t.elapsed() > Duration::from_secs(120)).unwrap_or(true);
        if due {
            g.insert(name.to_string(), Instant::now());
        }
        due
    }

    /// Адрес соседа использован (пришёл подписанный запрос или мы по нему достучались): дата — в `ip_seen`.
    pub fn mark_ip(&self, peer: &str, ip: &str) {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        {
            let mut g = self.ip_marked.lock();
            let k = (peer.to_string(), ip.to_string());
            if g.get(&k) == Some(&today) {
                return;
            }
            g.insert(k, today.clone());
        }
        let changed = {
            let _g = self.config_lock.lock();
            let mut c = self.cfg.write();
            match c.peer_mut(peer) {
                Some(p) if p.ips.iter().any(|x| x == ip) && p.ip_seen.get(ip) != Some(&today) => {
                    p.ip_seen.insert(ip.to_string(), today);
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.save_config();
        }
    }

    /// Раз в час: убрать дополнительные адреса соседей, не использованные больше 3 дней.
    pub fn prune_peer_ips(&self) {
        {
            let mut g = self.last_ip_prune.lock();
            if g.map(|t| t.elapsed() < Duration::from_secs(3600)).unwrap_or(false) {
                return;
            }
            *g = Some(Instant::now());
        }
        let today = chrono::Utc::now().date_naive();
        let mut log: Vec<String> = Vec::new();
        let changed = {
            let _g = self.config_lock.lock();
            let mut c = self.cfg.write();
            let mut changed = false;
            for p in c.peers.iter_mut() {
                let before = (p.ips.clone(), p.ip_seen.clone());
                let gone = p.prune_ips(today);
                if !gone.is_empty() {
                    log.push(format!("Сосед «{}»: убраны адреса {} — не использовались больше {} дней", p.name, gone.join(", "), bridge_core::config::IP_KEEP_DAYS));
                }
                changed |= before != (p.ips.clone(), p.ip_seen.clone());
            }
            changed
        };
        if changed {
            self.save_config();
        }
        for l in log {
            self.info(&l);
        }
    }

    /// Адрес соседа для файлов (3.19): включено «Файлы через сервер», сервер разрешает, своя сеть здесь работает,
    /// а у соседа есть адрес в своей сети — файлы идут по нему (через сервер). Иначе — None (как обычно).
    /// Своя сеть поднята и сервер разрешает файлы (без учёта галочки).
    pub fn bulk_ready(&self) -> bool {
        let rc = self.cfg.read().relay.clone();
        if !rc.net || rc.mode == "off" || !rc.configured() || !self.mostnet_up.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        let st = self.relay.status.lock();
        st.ok && st.files_allowed
    }

    /// Своя галочка «файлы через сервер» и всё готово — сообщаем соседям в привете.
    pub fn bulk_wanted(&self) -> bool {
        self.cfg.read().relay.files && self.bulk_ready()
    }

    pub fn bulk_url(&self, peer: &PeerConfig) -> Option<String> {
        let mine = self.cfg.read().relay.files;
        // через сервер — если так хочу я или сосед (у файла два конца, решает любой)
        if !(mine || self.peer_state(&peer.name).wants_bulk) || !self.bulk_ready() {
            return None;
        }
        let port = bridge_core::config::parse_url(&peer.url).map(|(_, p)| p).unwrap_or(5390);
        let ip = peer.ips.iter().find(|i| i.parse::<std::net::Ipv4Addr>().map(bridge_core::mostnet::in_net).unwrap_or(false))?;
        let u = format!("http://{}:{}", ip, port);
        (u != self.peer_url(peer)).then_some(u)
    }

    /// Клиент к соседу по конкретному адресу (для проверки запасного) — не кешируется.
    pub fn client_at(&self, peer: &PeerConfig, url: &str) -> PeerClient {
        let mut q = peer.clone();
        q.url = url.to_string();
        PeerClient::new(&self.node_name(), &q, &self.announce_urls(), Duration::from_secs(30))
    }

    /// Клиент к соседу — один на соседа (соединения переиспользуются). Пересоздаётся при смене адреса или ключа.
    pub fn client_for(&self, peer: &PeerConfig) -> Arc<PeerClient> {
        let mut q = peer.clone();
        q.url = self.peer_url(peer);
        let bulk = self.bulk_url(peer);
        let key = format!("{}|{}|{}", q.url, q.secret, bulk.as_deref().unwrap_or(""));
        let mut g = self.clients.lock();
        if let Some((k, c)) = g.get(&peer.name) {
            if *k == key {
                return c.clone();
            }
        }
        let c = Arc::new(PeerClient::new(&self.node_name(), &q, &self.announce_urls(), Duration::from_secs(30 * 60)).with_progress(self.transfers.clone(), self.limits.clone()).with_bulk(bulk));
        g.insert(peer.name.clone(), (key, c.clone()));
        c
    }

    pub fn forget_client(&self, name: &str) {
        self.clients.lock().remove(name);
    }

    /// «У меня новое» — соседям (не дожидаясь ответа).
    pub fn notify_peers(self: &Arc<Self>, names: Vec<String>, share: &str, kind: &str) {
        let cfg = self.cfg.read();
        for n in names {
            if let Some(p) = cfg.peer(&n).filter(|p| p.enabled && p.has_url()).cloned() {
                let c = self.client_for(&p);
                let (share, kind) = (share.to_string(), kind.to_string());
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(10), c.notify(&share, &kind)).await;
                });
            }
        }
    }

    pub fn notify_all(self: &Arc<Self>, kind: &str) {
        let names: Vec<String> = self.cfg.read().peers.iter().filter(|p| p.enabled).map(|p| p.name.clone()).collect();
        self.notify_peers(names, "", kind);
    }

    // ───────────── Статус ─────────────

    pub fn status(&self) -> NodeStatus {
        let cfg = self.cfg.read();
        let peers = self.peers.lock();
        let errs: Vec<String> = cfg
            .peers
            .iter()
            .filter(|p| p.enabled && p.has_url())
            .filter_map(|p| peers.get(&p.name).filter(|s| !s.error.is_empty()).map(|s| format!("{}: {}", s.name, s.error)))
            .collect();
        NodeStatus {
            node: cfg.node.clone(),
            version: APP_VERSION.into(),
            started: self.started,
            time: time::now(),
            state: if errs.is_empty() { "ok".into() } else { "error".into() },
            message: errs.join("; "),
            waiting_approval: self.runner.waiting_count() as i64,
        }
    }

    // ───────────── Главный цикл ─────────────

    async fn main_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_millis(1000));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => break,
                _ = tick.tick() => {},
                _ = self.wake.notified() => {},
            }
            if let Err(e) = self.step().await {
                self.info(&format!("Ошибка цикла: {}", e));
            }
        }
    }

    /// Один шаг цикла: то, что накопилось (изменения файлов, просьбы соседей), и обмен по расписанию.
    pub async fn step(self: &Arc<Self>) -> Result<(), String> {
        let interval = Duration::from_secs(self.cfg.read().interval_sec.max(1) as u64);
        let due = self.timer && self.last_round.lock().map(|t| t.elapsed() >= interval).unwrap_or(true);
        let requested = self.sync_requested.swap(false, Ordering::SeqCst);
        // Новые сети (Radmin поднялся после моста, сменился Wi-Fi) — слушать и на них
        let check_binds = {
            let mut t = self.last_bind_check.lock();
            if t.elapsed() > Duration::from_secs(30) {
                *t = Instant::now();
                true
            } else {
                false
            }
        };
        if check_binds {
            crate::http_peer::bind_auto(self).await;
            crate::turn::bind_all(self).await;
            self.prune_peer_ips();
        }
        // Страховка от пропущенных событий файловой системы (сетевые диски и т. п.): полный просмотр раз в 5 минут
        let full_scan = {
            let mut t = self.last_full_scan.lock();
            if t.elapsed() > Duration::from_secs(300) || self.watcher.lock().is_none() && t.elapsed() > Duration::from_secs(5) {
                *t = Instant::now();
                true
            } else {
                false
            }
        };
        if full_scan {
            for rt in self.shares() {
                rt.files_dirty.store(true, Ordering::SeqCst);
            }
            self.mailbox.dirty.store(true, Ordering::SeqCst);
            self.folders.own_dirty.store(true, Ordering::SeqCst);
            self.folders.mounts_dirty.store(true, Ordering::SeqCst);
        }
        crate::efc_chan::sync(self);
        for rt in self.shares() {
            crate::iface::process_outbox(self, &rt);
        }
        // Пока в папках идут изменения — ждём тишины (1 с), кроме обмена по расписанию и по просьбе
        let quiet = self.last_fs_event.lock().elapsed() >= Duration::from_millis(1000);
        if due || requested {
            *self.last_round.lock() = Some(Instant::now());
            // (3.21) в фоне: раньше обмен ждал самого медленного соседа (до 20 с на неотвечающего),
            // и только потом забирал отправку у тех, кто ответил сразу. Ответивший «из офлайна» будит обмен сам.
            if !self.contacting.swap(true, Ordering::SeqCst) {
                let (n, flag) = (self.clone(), self.contacting.clone());
                tokio::spawn(async move {
                    crate::link::contact_all(&n).await;
                    flag.store(false, Ordering::SeqCst);
                });
            }
        }
        for rt in self.shares() {
            if due || requested || rt.wake.swap(false, Ordering::SeqCst) {
                crate::shares::round(self, &rt).await;
            } else if quiet && rt.files_dirty.load(Ordering::SeqCst) {
                crate::shares::local_check(self, &rt).await;
            }
            if rt.interface_dirty.load(Ordering::SeqCst) {
                crate::iface::write(self, &rt);
            }
        }
        if quiet && (self.mailbox.dirty.swap(false, Ordering::SeqCst) || self.mailbox.waiting.load(Ordering::SeqCst)) && self.mailbox.outgoing_changed(self) {
            self.notify_all("mailbox");
        }
        if due || requested || self.mailbox.wake.swap(false, Ordering::SeqCst) {
            self.mailbox.round(self).await;
        }
        if quiet && self.folders.own_dirty.swap(false, Ordering::SeqCst) {
            self.folders.check_own_changes(self);
        }
        if quiet && self.folders.mounts_dirty.swap(false, Ordering::SeqCst) && self.folders.local_mounts_changed() {
            self.folders.mounts_touched.store(true, Ordering::SeqCst);
            self.folders.wake.store(true, Ordering::SeqCst);
        }
        self.folders.sweep_trash(self);
        if due || requested || self.folders.wake.swap(false, Ordering::SeqCst) {
            self.folders.round(self).await;
        }
        Ok(())
    }

    /// Перезапустить слежение за папками (после правки настроек: новая папка, пространство).
    pub fn rewatch(self: &Arc<Self>) {
        let w = Watcher::start(self);
        *self.watcher.lock() = w;
    }

    /// Обменяться со всеми сейчас (CLI, тесты, кнопка «Обменяться»).
    pub async fn sync_now(self: &Arc<Self>) {
        self.folders.invalidate_own();
        self.folders.mounts_touched.store(true, Ordering::SeqCst);
        for rt in self.shares() {
            rt.files_dirty.store(true, Ordering::SeqCst);
        }
        crate::link::contact_all(self).await;
        for rt in self.shares() {
            crate::shares::round(self, &rt).await;
            crate::iface::write(self, &rt);
        }
        self.mailbox.round(self).await;
        self.folders.round(self).await;
    }

    /// (3.21) Сосед появился на связи: обменяться с ним сразу и сказать ему, что у нас для него может быть отправка.
    pub fn on_peer_online(self: &Arc<Self>, name: &str) {
        self.mailbox.wake.store(true, Ordering::SeqCst);
        self.folders.wake.store(true, Ordering::SeqCst);
        for rt in self.shares() {
            rt.wake.store(true, Ordering::SeqCst);
        }
        self.notify_peers(vec![name.to_string()], "", "mailbox");
        self.wake.notify_one();
    }

    /// (3.21) Сосед постучался, а у нас он «не на связи» — проверить связь с ним сейчас (не чаще раза в 5 с).
    pub fn probe_peer(self: &Arc<Self>, name: &str) {
        if self.peer_state(name).online {
            return;
        }
        {
            let mut g = self.probes.lock();
            if g.get(name).map(|t| t.elapsed() < Duration::from_secs(5)).unwrap_or(false) {
                return;
            }
            g.insert(name.to_string(), Instant::now());
        }
        let Some(pc) = self.cfg.read().peer(name).filter(|p| p.enabled).cloned() else { return };
        let n = self.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(20), crate::link::contact(&n, &pc)).await;
        });
    }

    /// (3.21) «Отправить сейчас»: связаться с одним соседем и попросить его забрать отправку и сборки прямо сейчас.
    pub async fn sync_peer(self: &Arc<Self>, name: &str) -> Result<bool, String> {
        let pc = self.cfg.read().peer(name).cloned().ok_or_else(|| format!("нет соседа «{}»", name))?;
        if !pc.enabled {
            return Err(format!("«{}» выключен", pc.name));
        }
        let _ = tokio::time::timeout(Duration::from_secs(20), crate::link::contact(self, &pc)).await;
        let st = self.peer_state(&pc.name);
        if !st.online {
            return Ok(false);
        }
        self.notify_peers(vec![pc.name.clone()], "", "mailbox");
        self.notify_peers(vec![pc.name.clone()], "", "folders");
        self.mailbox.wake.store(true, Ordering::SeqCst);
        self.folders.wake.store(true, Ordering::SeqCst);
        self.wake.notify_one();
        Ok(true)
    }

    pub fn request_sync(&self) {
        self.sync_requested.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    pub async fn stop(&self) {
        self.shutdown.cancel();
        self.runner.stop_all().await;
        self.efc.stop_all();
        for rt in self.shares() {
            rt.save_state(self);
        }
        self.folders.save(self);
    }
}

fn load_or_create_token(data_dir: &Path) -> String {
    let file = data_dir.join("local.token");
    if let Ok(t) = std::fs::read_to_string(&file) {
        let t = t.trim().trim_start_matches('\u{feff}').to_string();
        if t.len() >= 32 {
            return t;
        }
    }
    let token = hex::encode(rand::random::<[u8; 24]>());
    let _ = std::fs::write(&file, &token);
    token
}

/// Без «\\?\\» в начале (Windows: canonicalize даёт такой путь — в панели он пугает и мешает сравнивать пути).
pub fn plain_path(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        if rest.len() > 1 && rest.as_bytes()[1] == b':' {
            return PathBuf::from(rest.to_string());
        }
    }
    p
}
