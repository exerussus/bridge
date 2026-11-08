//! Ядро приложения «Мост» для телефона (3.23).
//!
//! Внутри приложения работает настоящий узел моста (`bridge-node`): ключи, соседи, свой сервер, туннель, чат,
//! почта, каналы и сигналинг голоса. Интерфейс (Kotlin, позже Swift) говорит с ним по тому же локальному API,
//! что и панель ПК: `http://127.0.0.1:<порт>/bridge/v1/local/…` с заголовком `X-Bridge-Local: <токен>`.
//! Поэтому граница Rust ↔ Kotlin маленькая: запустить, остановить, узнать адрес и токен, сообщить о смене сети.

use bridge_core::config::NodeConfig;
use bridge_node::{Node, StartOptions};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

uniffi::setup_scaffolding!();

/// Запущенный узел: адрес локального API и токен к нему.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Started {
    /// Адрес локального API, например `http://127.0.0.1:41873`.
    pub url: String,
    /// Токен для заголовка `X-Bridge-Local` (или `?token=` для потоков событий).
    pub token: String,
    /// Имя узла (как его видят соседи).
    pub node: String,
    /// Версия моста.
    pub version: String,
}

#[derive(Debug, uniffi::Error)]
pub enum MobileError {
    /// Не запустилось: текст — для человека.
    Failed { msg: String },
}

impl std::fmt::Display for MobileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MobileError::Failed { msg } => f.write_str(msg),
        }
    }
}

impl std::error::Error for MobileError {}

fn fail(msg: impl Into<String>) -> MobileError {
    MobileError::Failed { msg: msg.into() }
}

struct Running {
    node: Arc<Node>,
    started: Started,
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("most-node")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

fn current() -> &'static Mutex<Option<Running>> {
    static CUR: OnceLock<Mutex<Option<Running>>> = OnceLock::new();
    CUR.get_or_init(|| Mutex::new(None))
}

/// Настройки узла-телефона при первом запуске: только локальный адрес на случайном порту, без своего
/// ретранслятора, без окна и вопросов в консоли. Соседи — через свой сервер.
pub fn phone_config(name: &str) -> NodeConfig {
    let mut cfg = NodeConfig { node: name.trim().to_string(), listen: vec!["http://127.0.0.1:0".into()], ..Default::default() };
    cfg.listen_auto = false;
    cfg.turn_port = 0;
    cfg.console_prompts = false;
    cfg.ui.auto_open = false;
    cfg
}

/// Имя узла пригодно: латиница, цифры, «-», «_», 1–40 знаков.
#[uniffi::export]
pub fn valid_name(name: String) -> bool {
    let n = name.trim();
    !n.is_empty() && n.len() <= 40 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Версия моста, из которого собрано ядро.
#[uniffi::export]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Запустить узел в папке `dir` (внутренняя папка приложения). `name` нужно только при первом запуске —
/// потом имя берётся из `bridge.json`. Уже запущен — вернуть тот же.
#[uniffi::export]
pub fn start(dir: String, name: String) -> Result<Started, MobileError> {
    if let Some(r) = current().lock().as_ref() {
        return Ok(r.started.clone());
    }
    let base = PathBuf::from(&dir);
    std::fs::create_dir_all(&base).map_err(|e| fail(format!("нет доступа к папке {}: {}", dir, e)))?;
    let file = base.join("bridge.json");
    if !file.exists() {
        if !valid_name(name.clone()) {
            return Err(fail("имя — латиницей, цифрами, «-» или «_», до 40 знаков"));
        }
        std::fs::write(&file, bridge_core::to_json(&phone_config(&name))).map_err(|e| fail(e.to_string()))?;
    }
    let cfg = NodeConfig::load(&file).map_err(fail)?;
    let node = runtime()
        .block_on(Node::start(cfg, StartOptions { base_dir: base.clone(), config_file: Some(file.clone()), console: false, ..Default::default() }))
        .map_err(fail)?;
    let started = started_of(&node)?;
    *current().lock() = Some(Running { node, started: started.clone() });
    Ok(started)
}

fn started_of(node: &Arc<Node>) -> Result<Started, MobileError> {
    let url = node
        .urls
        .read()
        .iter()
        .find(|u| u.starts_with("http://127.0.0.1:"))
        .cloned()
        .ok_or_else(|| fail("узел не слушает 127.0.0.1"))?;
    Ok(Started { url, token: node.local_token.clone(), node: node.node_name(), version: version() })
}

/// Запущенный узел или ничего.
#[uniffi::export]
pub fn status() -> Option<Started> {
    current().lock().as_ref().map(|r| r.started.clone())
}

/// Остановить узел (выход из приложения, «Выключить»).
#[uniffi::export]
pub fn stop() {
    let Some(r) = current().lock().take() else { return };
    runtime().block_on(r.node.stop());
}

/// Сеть сменилась (Wi-Fi ↔ мобильная): сразу переподключиться к серверу и соседям, не дожидаясь таймеров.
#[uniffi::export]
pub fn network_changed() {
    let Some(node) = current().lock().as_ref().map(|r| r.node.clone()) else { return };
    node.wake.notify_one();
}

/// Папка данных по умолчанию для тестов и настольной проверки.
pub fn data_dir_of(dir: &Path) -> PathBuf {
    dir.join("bridge-data")
}
