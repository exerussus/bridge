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
    pub strangers: Mutex<BTreeMap<String, StrangerInfo>>,
    pub nonces: NonceCache,
    clients: Mutex<HashMap<String, (String, Arc<PeerClient>)>>,
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
            strangers: Mutex::new(BTreeMap::new()),
            nonces: NonceCache::default(),
            clients: Mutex::new(HashMap::new()),
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

    /// Клиент к соседу — один на соседа (соединения переиспользуются). Пересоздаётся при смене адреса или ключа.
    pub fn client_for(&self, peer: &PeerConfig) -> Arc<PeerClient> {
        let key = format!("{}|{}", peer.url, peer.secret);
        let mut g = self.clients.lock();
        if let Some((k, c)) = g.get(&peer.name) {
            if *k == key {
                return c.clone();
            }
        }
        let c = Arc::new(PeerClient::new(&self.node_name(), peer, &self.announce_urls(), Duration::from_secs(30 * 60)).with_progress(self.transfers.clone(), self.limits.clone()));
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
        for rt in self.shares() {
            crate::iface::process_outbox(self, &rt);
        }
        // Пока в папках идут изменения — ждём тишины (1 с), кроме обмена по расписанию и по просьбе
        let quiet = self.last_fs_event.lock().elapsed() >= Duration::from_millis(1000);
        if due || requested {
            *self.last_round.lock() = Some(Instant::now());
            crate::link::contact_all(self).await;
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
        if quiet && self.mailbox.dirty.swap(false, Ordering::SeqCst) && self.mailbox.outgoing_changed(self) {
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

    pub fn request_sync(&self) {
        self.sync_requested.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    pub async fn stop(&self) {
        self.shutdown.cancel();
        self.runner.stop_all().await;
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
