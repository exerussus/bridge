//! Свой сервер-ретранслятор (3.16, `most-relay` на VPS) — опция, не замена: без сервера мост работает как раньше
//! (прямые адреса, Radmin). Подключение — одноразовым приглашением (`join`): мост меняет его на свой ключ участника.
//! Дальше мост раз в несколько часов берёт у сервера временный ключ TURN и отдаёт его панелям (`media/ice`) — WebRTC
//! сам пробует прямой путь первым, сервер подхватывает то, что не прошло.
//! Если разрешено (`relay.share`), ключ уходит и соседям в привете: настроил один — работает у всех.

use crate::node::Node;
use bridge_core::config::RelayConfig;
use bridge_core::relay::{JoinKeys, JoinRequest, JoinResponse, RelayHello, RelayInfo, RelayInvite, HELLO_PATH, JOIN_PATH};
use bridge_core::signing;
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Ключ обновлять, когда до конца срока осталось меньше этого.
const REFRESH_BEFORE: Duration = Duration::from_secs(6 * 3600);
/// После ошибки — повтор не раньше.
const RETRY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatus {
    /// Сервер подключён (есть адрес и ключ участника).
    pub configured: bool,
    pub mode: String,
    pub share: bool,
    /// Ключ получен и действует.
    pub ok: bool,
    pub error: String,
    /// Адрес сервера «адрес:порт».
    pub address: String,
    pub member: String,
    pub name: String,
    pub version: String,
    pub host: String,
    pub udp: u16,
    pub tcp: Vec<u16>,
    /// Время ответа сервера на запрос ключа, мс.
    pub rtt: Option<u64>,
    /// Адрес этого ПК, каким его видит сервер.
    pub you: String,
    /// Когда последний раз спрашивали (unix, с).
    pub checked: i64,
    /// До какого времени действует ключ (unix, с).
    pub expires: i64,
    /// Сервер разрешает файлы и синхронизацию через свою сеть (3.19).
    pub files_allowed: bool,
    /// Ограничение скорости своей сети на участника, Мбит/с (0 — нет).
    pub net_mbit: u32,
}

#[derive(Default)]
pub struct Relay {
    pub status: Mutex<RelayStatus>,
    info: Mutex<Option<RelayInfo>>,
    /// Для каких «сервер/участник/ключ» получен текущий ключ TURN (сменились — спросить заново).
    for_invite: Mutex<String>,
    /// С какими «сервер/участник/ключ» спрашивали последний раз (удачно или нет).
    tried: Mutex<String>,
    last_try: Mutex<Option<Instant>>,
    busy: tokio::sync::Mutex<()>,
}

impl Relay {
    /// Действующий сервер этого моста (для панели и соседей), если он включён.
    pub fn info(&self, cfg: &RelayConfig) -> Option<RelayInfo> {
        if cfg.mode == "off" || !cfg.configured() {
            return None;
        }
        let st = self.status.lock();
        (st.ok && st.expires > chrono::Utc::now().timestamp()).then(|| self.info.lock().clone()).flatten()
    }
}

fn ident(c: &RelayConfig) -> String {
    format!("{}\n{}\n{}", c.server, c.member, c.secret)
}

/// Подключиться по одноразовому приглашению: получить ключ участника и сохранить сервер в настройках.
pub async fn join(node: &Arc<Node>, line: &str) -> Result<RelayStatus, String> {
    let inv = RelayInvite::parse(line)?;
    let keys = JoinKeys::new();
    let req = JoinRequest { code: inv.code.clone(), name: node.node_name(), key: keys.public.clone() };
    let http = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?;
    let url = format!("{}{}", bridge_core::relay::base_url(&inv.host, inv.port), JOIN_PATH);
    let r = http.post(url).json(&req).send().await.map_err(|e| net_text(&e, &inv.host, inv.port))?;
    let code = r.status();
    let text = r.text().await.map_err(|e| e.to_string())?;
    if !code.is_success() {
        return Err(format!("сервер отказал: {}", err_text(&text)));
    }
    let resp: JoinResponse = serde_json::from_str(&text).map_err(|_| "ответ не похож на most-relay — проверьте адрес и порт".to_string())?;
    let secret = keys.open(&inv.code, &resp)?;
    {
        let _g = node.config_lock.lock();
        {
            let mut c = node.cfg.write();
            c.relay.server = format!("{}:{}", inv.host, inv.port);
            c.relay.member = resp.member.clone();
            c.relay.secret = secret;
        }
        if let Some(e) = node.save_config() {
            return Err(e);
        }
    }
    node.info(&format!("Свой сервер-ретранслятор «{}» подключён ({}:{}, участник «{}»)", resp.server, inv.host, inv.port, resp.member));
    Ok(check(node).await)
}

/// Отключить сервер на этом ПК (ключ участника забывается).
pub fn forget(node: &Arc<Node>) -> Option<String> {
    let _g = node.config_lock.lock();
    {
        let mut c = node.cfg.write();
        c.relay.server.clear();
        c.relay.member.clear();
        c.relay.secret.clear();
    }
    *node.relay.info.lock() = None;
    *node.relay.status.lock() = RelayStatus::default();
    node.save_config()
}

fn net_text(e: &reqwest::Error, host: &str, port: u16) -> String {
    if e.is_timeout() {
        format!("сервер {}:{} не отвечает — он выключен, адрес неверный или порт закрыт брандмауэром хостинга", host, port)
    } else if e.is_connect() {
        format!("не удаётся подключиться к {}:{} — служба most-relay на сервере не запущена или порт закрыт", host, port)
    } else {
        format!("сервер {}:{}: {}", host, port, e)
    }
}

fn err_text(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text).ok().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or_else(|| text.chars().take(200).collect())
}

/// Спросить ключ у сервера (сразу, без оглядки на срок).
pub async fn check(node: &Arc<Node>) -> RelayStatus {
    let _g = node.relay.busy.lock().await;
    let cfg = node.cfg.read().relay.clone();
    *node.relay.last_try.lock() = Some(Instant::now());
    *node.relay.tried.lock() = ident(&cfg);
    let mut st = RelayStatus { configured: cfg.configured(), mode: cfg.mode.clone(), share: cfg.share, checked: chrono::Utc::now().timestamp(), ..Default::default() };
    if !st.configured {
        *node.relay.info.lock() = None;
        *node.relay.status.lock() = st.clone();
        return st;
    }
    let (host, port) = match bridge_core::relay::split_addr(&cfg.server) {
        Ok(x) => x,
        Err(e) => {
            st.error = e;
            *node.relay.info.lock() = None;
            *node.relay.status.lock() = st.clone();
            return st;
        }
    };
    st.address = cfg.server.clone();
    st.member = cfg.member.clone();
    let t0 = Instant::now();
    match fetch(node, &host, port, &cfg.secret).await {
        Ok(h) => {
            st.rtt = Some(t0.elapsed().as_millis() as u64);
            st.ok = true;
            st.name = h.name.clone();
            st.version = h.version.clone();
            st.host = h.host.clone();
            st.udp = h.udp;
            st.tcp = h.tcp.clone();
            st.you = h.you.clone();
            st.expires = chrono::Utc::now().timestamp() + h.ttl as i64;
            st.files_allowed = h.files.unwrap_or(true);
            st.net_mbit = h.net_mbit;
            *node.relay.info.lock() = Some(RelayInfo::from_hello(&h));
            *node.relay.for_invite.lock() = ident(&cfg);
        }
        Err((e, refused)) => {
            st.error = e;
            // сеть подвела, а прежний ключ ещё действует — пусть работает, пока не истёк (отказ сервера — не держим)
            let old = node.relay.status.lock().clone();
            if !refused && old.ok && old.expires > chrono::Utc::now().timestamp() && *node.relay.for_invite.lock() == ident(&cfg) {
                st.ok = true;
                st.name = old.name;
                st.version = old.version;
                st.host = old.host;
                st.udp = old.udp;
                st.tcp = old.tcp;
                st.you = old.you;
                st.expires = old.expires;
                st.rtt = old.rtt;
                st.files_allowed = old.files_allowed;
                st.net_mbit = old.net_mbit;
            } else {
                *node.relay.info.lock() = None;
            }
        }
    }
    *node.relay.status.lock() = st.clone();
    st
}

/// Err((причина, сервер_отказал)).
async fn fetch(node: &Arc<Node>, host: &str, port: u16, secret: &str) -> Result<RelayHello, (String, bool)> {
    let net = |e: String| (e, false);
    let http = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(8)).build().map_err(|e| net(e.to_string()))?;
    let auth = format!("{} {}", bridge_core::AUTH_SCHEME, signing::header(&node.node_name(), secret, "GET", HELLO_PATH, &signing::empty_body_sha(), false));
    let r = http.get(format!("{}{}", bridge_core::relay::base_url(host, port), HELLO_PATH)).header("Authorization", auth).send().await.map_err(|e| net(net_text(&e, host, port)))?;
    let code = r.status();
    let text = r.text().await.map_err(|e| net(e.to_string()))?;
    if !code.is_success() {
        return Err((format!("сервер отказал ({}): {}", code.as_u16(), err_text(&text)), code.is_client_error()));
    }
    let h: RelayHello = serde_json::from_str(&text).map_err(|_| ("ответ не похож на most-relay — проверьте адрес и порт в строке приглашения".to_string(), true))?;
    if h.host.is_empty() || h.username.is_empty() {
        return Err(("сервер не прислал ключ".into(), true));
    }
    Ok(h)
}

/// Раз в минуту: пора ли спросить ключ (сменилось приглашение, ключ скоро истечёт, была ошибка).
pub fn schedule(node: &Arc<Node>) {
    let n = node.clone();
    tokio::spawn(async move {
        loop {
            let due = {
                let cfg = n.cfg.read().relay.clone();
                let st = n.relay.status.lock().clone();
                let last = *n.relay.last_try.lock();
                let changed = ident(&cfg) != *n.relay.tried.lock();
                let soon = st.ok && st.expires - chrono::Utc::now().timestamp() < REFRESH_BEFORE.as_secs() as i64;
                let retry = !st.ok && st.configured && last.map(|t| t.elapsed() > RETRY).unwrap_or(true);
                (changed && last.map(|t| t.elapsed() > Duration::from_secs(5)).unwrap_or(true)) || soon || retry || (last.is_none() && cfg.configured())
            };
            if due {
                check(&n).await;
            }
            tokio::select! {
                _ = n.shutdown.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(15)) => {}
            }
        }
    });
}
