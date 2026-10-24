//! Связь с соседями: знакомство по адресу и ключу (имя сосед сообщает сам), приглашения, привет-опрос,
//! понятные причины отказов и пошаговая проверка «почему нет синка».

use crate::client::{NetKind, PeerError};
use crate::node::{Node, PeerState};
use bridge_core::config::{self, canon_ip, is_name, parse_address, split_invite, NodeConfig, PeerConfig};
use bridge_core::proto::HelloResponse;
use bridge_core::time::{self};
use bridge_core::{eq_ic, APP_VERSION};
use serde::Serialize;
use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectResult {
    pub ok: bool,
    pub name: String,
    pub pending: bool,
    pub online: bool,
    pub error: String,
    pub hint: String,
    pub updated: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteResult {
    pub name: String,
    pub secret: String,
    pub addresses: Vec<String>,
    pub line: String,
}

#[derive(Clone, Debug, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CheckStep {
    pub title: String,
    /// ok / warn / bad / info
    pub state: String,
    pub text: String,
    pub fix: String,
}

#[derive(Clone, Debug, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PeerCheck {
    pub peer: String,
    pub title: String,
    pub time: String,
    pub state: String,
    pub summary: String,
    pub steps: Vec<CheckStep>,
}

pub const WAITING_INVITE: &str = "ждём, когда сосед подключится сам (приглашение: адрес этого ПК и ключ)";

impl Node {
    pub fn own_port(&self) -> u16 {
        for l in self.urls.read().iter().chain(self.cfg.read().listen.iter()) {
            if let Some((_, p)) = config::parse_listen(l) {
                if p > 0 {
                    return p;
                }
            }
        }
        5390
    }

    /// Свои адреса для соседей (http://ip:порт): из listen без 127.0.0.1; «слушать всё» — адреса сетевых карт.
    pub fn announce_urls(&self) -> Vec<String> {
        let mut list = Vec::new();
        let mut lo = Vec::new();
        let urls = self.urls.read().clone();
        let src = if urls.is_empty() { self.cfg.read().listen.clone() } else { urls };
        for l in src {
            let Some((ip, port)) = config::parse_listen(&l) else { continue };
            if ip.is_unspecified() {
                for x in local_ips() {
                    list.push(format!("http://{}:{}", x, port));
                }
            } else if let IpAddr::V4(v4) = ip {
                if v4.is_loopback() {
                    lo.push(format!("http://{}:{}", v4, port));
                } else {
                    list.push(format!("http://{}:{}", v4, port));
                }
            }
        }
        let mut out = if list.is_empty() { lo } else { list };
        out.dedup();
        out
    }
}

/// IPv4-адреса этого ПК; адреса Radmin VPN (26.x) — первыми.
pub fn local_ips() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    // Без внешних библиотек: адрес, с которого ушёл бы пакет в каждую из типичных сетей
    for probe in ["26.0.0.1:9", "10.255.255.255:9", "192.168.255.255:9", "172.31.255.255:9", "8.8.8.8:9"] {
        if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if s.connect(probe).is_ok() {
                if let Ok(a) = s.local_addr() {
                    let ip = a.ip().to_string();
                    if !a.ip().is_loopback() && !a.ip().is_unspecified() && !v.contains(&ip) {
                        v.push(ip);
                    }
                }
            }
        }
    }
    v.sort_by_key(|ip| if ip.starts_with("26.") { 0 } else { 1 });
    v
}

/// Адреса из hello соседа: «http://ip:порт,…» → (ip, порт). «0.0.0.0» — адрес, с которого пришёл запрос.
pub fn parse_announced(listen: &str, from: Option<IpAddr>) -> Vec<(String, u16)> {
    let mut out = Vec::new();
    for part in listen.split(',').map(str::trim).filter(|s| !s.is_empty()).take(16) {
        let Some((mut ip, port)) = config::parse_url(part) else { continue };
        ip = canon_ip(ip);
        if ip.is_unspecified() {
            if let Some(f) = from {
                ip = f;
            }
        }
        let IpAddr::V4(v4) = ip else { continue };
        if v4.is_unspecified() {
            continue;
        }
        // 127.0.0.1 — только от узла на этом же ПК
        if v4.is_loopback() && !from.map(|f| f.is_loopback()).unwrap_or(false) {
            continue;
        }
        out.push((v4.to_string(), port));
    }
    out
}

/// Запрос пришёл с адреса, которого нет у соседа (ключ и подпись уже проверены). Принять, если сосед сам назвал
/// этот адрес в подписанном hello или это приглашение без адреса.
pub fn learn_address(node: &Arc<Node>, peer_name: &str, ip: Option<IpAddr>, listen: &str) -> bool {
    let Some(ip) = ip.map(canon_ip) else { return false };
    let ip_text = ip.to_string();
    let announced = parse_announced(listen, Some(ip));
    let invite;
    let own_port = node.own_port();
    {
        let _g = node.config_lock.lock();
        let mut cfg = node.cfg.write();
        let Some(p) = cfg.peer_mut(peer_name) else { return false };
        invite = p.pending && p.ips.is_empty();
        if !invite && !announced.iter().any(|a| a.0 == ip_text) {
            return false;
        }
        if !p.ips.contains(&ip_text) {
            p.ips.push(ip_text.clone());
        }
        if p.url.is_empty() {
            let port = announced.iter().find(|a| a.0 == ip_text).map(|a| a.1).or(announced.first().map(|a| a.1))
                // порт из любого объявленного адреса (даже 127.0.0.1): свой порт подставлять нельзя — можно попасть в себя
                .or(listen.split(',').find_map(|u| config::parse_url(u.trim()).map(|x| x.1)))
                .unwrap_or(own_port);
            p.url = format!("http://{}:{}", ip_text, port);
        }
    }
    let err = node.save_config();
    node.info(&format!(
        "Сосед «{}»: адрес {} записан — {}{}",
        peer_name,
        ip_text,
        if invite { "подключился по приглашению" } else { "сосед сам сообщил его, ключ верный" },
        err.map(|e| format!(" (не сохранено: {})", e)).unwrap_or_default()
    ));
    true
}

/// Сосед в hello перечислил свои адреса: недостающие — в белый список.
pub fn learn_announced(node: &Arc<Node>, peer_name: &str, listen: &str, from: Option<IpAddr>) {
    let add: Vec<String>;
    {
        let _g = node.config_lock.lock();
        let mut cfg = node.cfg.write();
        let Some(p) = cfg.peer_mut(peer_name) else { return };
        let mut a: Vec<String> = parse_announced(listen, from).into_iter().map(|x| x.0).filter(|x| !p.ips.contains(x)).collect();
        a.dedup();
        if a.is_empty() {
            return;
        }
        p.ips.extend(a.iter().cloned());
        add = a;
    }
    let err = node.save_config();
    node.info(&format!("Сосед «{}»: добавлены адреса {} (сообщил сам){}", peer_name, add.join(", "), err.map(|e| format!(" (не сохранено: {})", e)).unwrap_or_default()));
}

/// Сосед назвался (подпись верна). Временное имя заменяется настоящим везде; давно знакомый — только предупреждение.
pub fn learn_name(node: &Arc<Node>, peer_name: &str, announced: &str) {
    if !is_name(announced) {
        return;
    }
    let pending = node.cfg.read().peer(peer_name).map(|p| p.pending).unwrap_or(false);
    if eq_ic(announced, peer_name) {
        node.with_peer_state(peer_name, |s| s.warning.clear());
        if pending {
            {
                let _g = node.config_lock.lock();
                if let Some(p) = node.cfg.write().peer_mut(peer_name) {
                    p.pending = false;
                }
            }
            node.save_config();
        }
        return;
    }
    if pending {
        adopt_name(node, peer_name, announced);
        return;
    }
    node.with_peer_state(peer_name, |s| {
        s.warning = format!("сосед называет себя «{}», а здесь записан как «{}» — его задачи и записи не примутся. Нажмите «Принять имя».", announced, peer_name);
        s.announced_name = announced.into();
    });
    node.info_once(
        &format!("name|{}|{}", peer_name, announced),
        &format!("Сосед «{}» теперь называет себя «{}» — переименуйте его в панели («Принять имя»)", peer_name, announced),
        60,
    );
}

/// Временное имя → настоящее. Если сосед с таким именем уже есть (переподключение с новым ключом) — слить в него.
fn adopt_name(node: &Arc<Node>, temp: &str, name: &str) {
    let log;
    {
        let _g = node.config_lock.lock();
        let mut cfg = node.cfg.write();
        let Some(idx) = cfg.peers.iter().position(|p| eq_ic(&p.name, temp) && p.pending) else { return };
        if eq_ic(name, &cfg.node) {
            drop(cfg);
            node.with_peer_state(temp, |s| s.warning = format!("этот сосед называет себя так же, как этот ПК («{}») — переименуйте один из узлов", name));
            return;
        }
        let peer = cfg.peers[idx].clone();
        if let Some(oi) = cfg.peers.iter().position(|p| eq_ic(&p.name, name)) {
            // Тот же ПК, новый ключ: ключ и адреса — старой записи, временная — удаляется
            let other = &mut cfg.peers[oi];
            other.secret = peer.secret.clone();
            for ip in &peer.ips {
                if !other.ips.contains(ip) {
                    other.ips.push(ip.clone());
                }
            }
            if !peer.url.is_empty() {
                other.url = peer.url.clone();
            }
            if other.title.is_empty() {
                other.title = peer.title.clone();
            }
            other.enabled = true;
            other.pending = false;
            let real = other.name.clone();
            cfg.peers.remove(idx);
            rename_refs(node, &mut cfg, temp, &real);
            node.peers.lock().remove(temp);
            log = format!("Сосед «{}» подключён заново: новый ключ и адрес записаны в прежнюю запись", real);
        } else {
            rename_refs(node, &mut cfg, temp, name);
            let p = &mut cfg.peers[idx];
            p.name = name.into();
            p.pending = false;
            let mut peers = node.peers.lock();
            if let Some(mut st) = peers.remove(temp) {
                st.name = name.into();
                peers.insert(name.into(), st);
            }
            log = format!("Сосед назвался: «{}» (было временное имя «{}»)", name, temp);
        }
        node.forget_client(temp);
    }
    node.mailbox.prepare(node);
    node.folders.wake.store(true, Ordering::SeqCst);
    for rt in node.shares() {
        rt.wake.store(true, Ordering::SeqCst);
    }
    node.wake.notify_one();
    let err = node.save_config();
    node.info(&format!("{}{}", log, err.map(|e| format!(" (не сохранено: {})", e)).unwrap_or_default()));
    node.emit("peer-named", serde_json::json!({ "name": name, "temp": temp }));
}

/// Заменить имя соседа во всех ссылках настроек и состояния.
pub fn rename_refs(node: &Node, cfg: &mut NodeConfig, from: &str, to: &str) {
    let swap = |l: &mut Vec<String>| {
        let mut out: Vec<String> = Vec::new();
        for x in l.iter() {
            let v = if eq_ic(x, from) { to.to_string() } else { x.clone() };
            if !out.iter().any(|o| eq_ic(o, &v)) {
                out.push(v);
            }
        }
        *l = out;
    };
    for s in cfg.shares.iter_mut() {
        swap(&mut s.peers);
        swap(&mut s.members);
        for a in s.accept.iter_mut() {
            swap(&mut a.from);
        }
        for r in s.records.accept.iter_mut() {
            swap(&mut r.from);
        }
    }
    for f in cfg.folders.iter_mut() {
        if f.access.keys().any(|k| eq_ic(k, from)) {
            f.access = f.access.iter().map(|(k, v)| (if eq_ic(k, from) { to.to_string() } else { k.clone() }, v.clone())).collect();
        }
    }
    for g in cfg.groups.values_mut() {
        swap(&mut g.members);
    }
    for c in cfg.channels.values_mut() {
        swap(&mut c.members);
    }
    swap(&mut cfg.mailbox.accept_from);
    for r in cfg.runners.values_mut() {
        swap(&mut r.allow_from);
    }
    let prefix = format!("{}/", from.to_lowercase());
    if cfg.mounts.ignore.keys().any(|k| k.to_lowercase().starts_with(&prefix)) {
        cfg.mounts.ignore = cfg
            .mounts
            .ignore
            .iter()
            .map(|(k, v)| (if k.to_lowercase().starts_with(&prefix) { format!("{}{}", to, &k[from.len()..]) } else { k.clone() }, v.clone()))
            .collect();
    }
    for rt in node.shares() {
        let mut rb = rt.records.lock();
        if let Some(k) = rb.peer_cursors.keys().find(|k| eq_ic(k, from)).cloned() {
            let cur = rb.peer_cursors.remove(&k).unwrap();
            rb.peer_cursors.insert(to.into(), cur);
            rt.cursors_changed.store(true, Ordering::SeqCst);
        }
        let mut rep = rt.reports.lock();
        if let Some(v) = rep.remove(from) {
            rep.insert(to.into(), v);
        }
    }
    node.folders.forget_peer(node, from);
    // Очередь отправки по прежнему имени — в папку нового
    let out_root = node.mailbox.outgoing_root(node, cfg);
    let (old, new) = (out_root.join(from), out_root.join(to));
    if old.is_dir() {
        if !new.exists() {
            let _ = std::fs::rename(&old, &new);
        } else if std::fs::read_dir(&old).map(|mut d| d.next().is_none()).unwrap_or(false) {
            let _ = std::fs::remove_dir(&old);
        }
    }
}

/// Переименовать соседа вручную.
pub fn rename_peer(node: &Arc<Node>, name: &str, to: &str) -> Result<(), String> {
    let to = to.trim();
    {
        let _g = node.config_lock.lock();
        let mut cfg = node.cfg.write();
        let Some(idx) = cfg.peers.iter().position(|p| eq_ic(&p.name, name)) else { return Err("нет такого соседа".into()) };
        if !is_name(to) {
            return Err("имя — латиница, цифры, «-», «_»".into());
        }
        if eq_ic(to, &cfg.node) {
            return Err("так называется этот ПК".into());
        }
        if cfg.peers.iter().enumerate().any(|(i, p)| i != idx && eq_ic(&p.name, to)) {
            return Err(format!("сосед «{}» уже есть", to));
        }
        let from = cfg.peers[idx].name.clone();
        rename_refs(node, &mut cfg, &from, to);
        cfg.peers[idx].name = to.into();
        cfg.peers[idx].pending = false;
        let mut peers = node.peers.lock();
        if let Some(mut st) = peers.remove(&from) {
            st.name = to.into();
            st.warning.clear();
            peers.insert(to.into(), st);
        }
        node.forget_client(&from);
    }
    node.mailbox.prepare(node);
    node.folders.wake.store(true, Ordering::SeqCst);
    let err = node.save_config();
    node.info(&format!("Панель/API: сосед «{}» переименован в «{}»", name, to));
    match err {
        None => Ok(()),
        Some(e) => Err(format!("переименован, но не сохранено в файл: {}", e)),
    }
}

// ───────────────────────── Подключение ─────────────────────────

pub async fn connect_peer(node: &Arc<Node>, address: &str, secret: &str, title: &str) -> ConnectResult {
    let (a, s) = split_invite(address, secret);
    let (ip, port) = match parse_address(&a, node.own_port()) {
        Ok(x) => x,
        Err(e) => return ConnectResult { error: e, ..Default::default() },
    };
    if s.chars().count() < 24 {
        return ConnectResult {
            error: "ключ — строка «BRS-…» не короче 24 знаков: скопируйте её с того ПК (там «Пригласить» или ключ из его соседа)".into(),
            ..Default::default()
        };
    }
    let url = format!("http://{}:{}", ip, port);
    if node.urls.read().iter().any(|u| config::parse_url(u).map(|(h, p)| h.to_string() == ip && p == port).unwrap_or(false)) {
        return ConnectResult { error: "это адрес этого же ПК, а нужен адрес соседа (он виден в «Обзоре» его панели)".into(), ..Default::default() };
    }
    let (name, updated);
    {
        let _g = node.config_lock.lock();
        let mut cfg = node.cfg.write();
        if let Some(p) = cfg.peers.iter_mut().find(|p| p.secret == s) {
            p.url = url.clone();
            if !p.ips.contains(&ip) {
                p.ips.insert(0, ip.clone());
            }
            if !title.trim().is_empty() {
                p.title = title.trim().into();
            }
            p.enabled = true;
            name = p.name.clone();
            updated = true;
        } else {
            let mut n = PeerConfig::temp_name(&s);
            let base = n.clone();
            let mut i = 2;
            while cfg.peer(&n).is_some() {
                n = format!("{}-{}", base, i);
                i += 1;
            }
            let peer = PeerConfig { name: n.clone(), pending: true, url: url.clone(), ips: vec![ip.clone()], secret: s.clone(), title: title.trim().into(), ..Default::default() };
            let mut clone = cfg.clone();
            clone.peers.push(peer.clone());
            let errs = clone.validate();
            if !errs.is_empty() {
                return ConnectResult { error: errs.join("; "), ..Default::default() };
            }
            cfg.peers.push(peer);
            node.peers.lock().insert(n.clone(), PeerState { name: n.clone(), ..Default::default() });
            name = n;
            updated = false;
        }
    }
    let save_err = node.save_config();
    node.info(&format!(
        "Панель/API: {}",
        if updated { format!("сосед «{}»: адрес {}", name, url) } else { format!("подключение соседа по адресу {}", url) }
    ));
    let kid = bridge_core::signing::key_id(&s);
    // по ключу: сосед мог уже назваться (его запрос пришёл раньше нашего привета) и переименоваться
    let peer = node.cfg.read().peer_by_key(&kid).cloned();
    let hello = match peer {
        Some(p) => match tokio::time::timeout(Duration::from_secs(8), contact(node, &p)).await {
            Ok(h) => h,
            Err(_) => {
                node.with_peer_state(&p.name, |st| {
                    st.online = false;
                    let (e, h) = explain_timeout(&p);
                    st.error = e;
                    st.hint = h;
                });
                None
            }
        },
        None => None,
    };
    let cur = node.cfg.read().peer_by_key(&kid).cloned();
    let (name, pending) = cur.map(|p| (p.name, p.pending)).unwrap_or((name, true));
    let st = node.peer_state(&name);
    let hello_err = if hello.is_none() && st.error.is_empty() { "сосед не ответил".to_string() } else { st.error.clone() };
    if hello.is_some() {
        node.mailbox.wake.store(true, Ordering::SeqCst);
        node.folders.wake.store(true, Ordering::SeqCst);
        for rt in node.shares() {
            rt.wake.store(true, Ordering::SeqCst);
        }
        node.wake.notify_one();
    }
    ConnectResult {
        ok: true,
        updated,
        name,
        pending,
        online: hello.is_some(),
        error: if hello.is_none() { hello_err } else { save_err.map(|e| format!("не сохранено в файл: {}", e)).unwrap_or_default() },
        hint: st.hint,
    }
}

/// Пригласить: создать ключ и ждать, когда сосед подключится по адресу этого ПК и ключу.
pub fn invite(node: &Arc<Node>, title: &str) -> (InviteResult, String) {
    let secret = NodeConfig::new_secret();
    let name = PeerConfig::temp_name(&secret);
    {
        let _g = node.config_lock.lock();
        node.cfg.write().peers.push(PeerConfig { name: name.clone(), pending: true, secret: secret.clone(), title: title.trim().into(), ..Default::default() });
        node.peers.lock().insert(name.clone(), PeerState { name: name.clone(), error: WAITING_INVITE.into(), ..Default::default() });
    }
    let err = node.save_config();
    let addrs: Vec<String> = node.announce_urls().iter().map(|u| u.trim_start_matches("http://").to_string()).collect();
    node.info(&format!(
        "Панель/API: приглашение «{}»{} — ждём подключения",
        name,
        if title.trim().is_empty() { String::new() } else { format!(" ({})", title.trim()) }
    ));
    let line = format!("{} {}", addrs.first().cloned().unwrap_or_else(|| "<адрес этого ПК>".into()), secret);
    (InviteResult { name, secret, addresses: addrs, line }, err.map(|e| format!("не сохранено в файл: {}", e)).unwrap_or_default())
}

// ───────────────────────── Привет-опрос ─────────────────────────

/// Спросить соседа «ты здесь?»: связь, имя, версия, как он видит нас.
pub async fn contact(node: &Arc<Node>, peer: &PeerConfig) -> Option<HelloResponse> {
    if peer.url.is_empty() {
        node.with_peer_state(&peer.name, |s| {
            s.online = false;
            s.error = WAITING_INVITE.into();
            s.hint = "На том ПК: «Соседи» → «Подключить» → адрес этого ПК и ключ из приглашения.".into();
        });
        return None;
    }
    let client = node.client_for(peer);
    let kid = peer.key_id();
    // сосед мог переименоваться (назваться) во время запроса — состояние пишем под его нынешним именем
    let current = |node: &Arc<Node>| node.cfg.read().peer_by_key(&kid).map(|p| p.name.clone()).unwrap_or_else(|| peer.name.clone());
    let t0 = std::time::Instant::now();
    match client.hello().await {
        Ok(hello) => {
            let name = current(node);
            let before = node.peer_state(&name);
            let was_online = before.online;
            node.with_peer_state(&name, |s| {
                s.latency_ms = Some(t0.elapsed().as_millis() as i64);
                s.online = true;
                s.last_seen = Some(time::now());
                s.status = hello.status.clone();
                s.version = hello.version.clone();
                s.legacy = client.legacy();
                s.remote = hello.you.clone();
                s.channels = hello.channels.clone();
                s.agent = hello.agent.clone();
                s.turn = hello.turn.clone();
                s.assistant = hello.assistant.clone();
                s.runners = hello.runners.clone();
                s.avatar = hello.avatar.clone();
                s.sound = hello.sound.clone();
                s.ping = hello.ping.clone();
                s.tile = hello.tile.clone();
                s.shares = hello.shares.clone();
                s.error.clear();
                s.hint.clear();
                s.last_error.clear();
            });
            if !was_online {
                node.emit("peer-online", serde_json::json!({ "peer": name }));
            }
            // сосед сменил аватарку, плитку или звуки — открытые панели перечитают отпечатки сразу
            if before.avatar != hello.avatar || before.tile != hello.tile || before.sound != hello.sound || before.ping != hello.ping {
                node.media.deliver(serde_json::json!({ "media": "ui", "type": "avatars", "peer": name }));
            }
            if before.online && before.channels != hello.channels {
                node.media.deliver(serde_json::json!({ "media": "ui", "type": "channels", "peer": name }));
            }
            if !before.version.is_empty() && before.version != hello.version {
                node.info(&format!("«{}» теперь на мосте {}", name, hello.version));
                node.emit("peer-version", serde_json::json!({ "peer": name, "version": hello.version, "was": before.version }));
            }
            learn_name(node, &name, &hello.node);
            Some(hello)
        }
        Err(e) => {
            let name = current(node);
            let (err, mut hint) = explain(&e, peer);
            // Связь в одну сторону: сосед к нам ходит, а мы к нему — нет
            if matches!(e, PeerError::Net(..)) {
                let ps = node.peer_state(&name);
                if ps.last_inbound.map(|t| time::now().signed_duration_since(t).num_seconds() < 180).unwrap_or(false) {
                    hint = format!(
                        "Связь в одну сторону: сосед сюда обращается, а отсюда до него по {} — нет. Значит, на том ПК мост не слушает этот адрес или входящие закрыл брандмауэр. Обновите мост там до 2.0 и нажмите «Проверить» у этого ПК — он скажет точно.",
                        peer.url
                    );
                }
            }
            let (was_online, last) = node.with_peer_state(&name, |s| {
                let r = (s.online, s.last_error.clone());
                s.online = false;
                s.error = err.clone();
                s.hint = hint.clone();
                s.last_error = err.clone();
                r
            });
            if was_online || last != err {
                node.info_once(&format!("contact|{}|{}", name, err), &format!("Нет связи с «{}»: {}", name, err), 30);
            }
            if was_online {
                node.emit("peer-offline", serde_json::json!({ "peer": name, "error": err }));
            }
            None
        }
    }
}

/// Привет всем включённым соседям (параллельно).
pub async fn contact_all(node: &Arc<Node>) {
    let peers: Vec<PeerConfig> = node.cfg.read().peers.iter().filter(|p| p.enabled).cloned().collect();
    let tasks = peers.into_iter().map(|p| {
        let node = node.clone();
        async move {
            if tokio::time::timeout(Duration::from_secs(15), contact(&node, &p)).await.is_err() {
                node.with_peer_state(&p.name, |s| {
                    s.online = false;
                    let (e, h) = explain_timeout(&p);
                    s.error = e;
                    s.hint = h;
                });
            }
        }
    });
    futures_util::future::join_all(tasks).await;
}

fn host_of(peer: &PeerConfig) -> String {
    peer.url.trim_start_matches("http://").trim_end_matches('/').to_string()
}

pub fn explain_timeout(peer: &PeerConfig) -> (String, String) {
    explain(&PeerError::Net(NetKind::Timeout, String::new()), peer)
}

/// Ошибка связи → короткая причина и что сделать.
pub fn explain(e: &PeerError, peer: &PeerConfig) -> (String, String) {
    let host = host_of(peer);
    match e {
        PeerError::Http(code, msg) => {
            let hint = match code {
                401 if msg.contains("ключ") => "Ключ на двух ПК должен быть одинаковым. Проще всего — подключить заново: на том ПК «Пригласить», здесь «Подключить» с его строкой.",
                401 if msg.contains("время") => "Включите «Синхронизировать время автоматически» в Windows на обоих ПК.",
                401 if msg.contains("выключен") => "На том ПК включите этот ПК в «Соседях» (кнопка «▶ Включить»).",
                401 => "Обновите мост на обоих ПК до одной версии; если не помогло — подключите заново по адресу и ключу.",
                403 => "На том ПК: «Соседи» → блок «Стучались» → «Добавить адрес». Или обновите мост там — адреса будут записываться сами.",
                404 => "Проверьте адрес и порт: по ним отвечает не мост или мост другой версии.",
                _ => "",
            };
            (format!("«{}» отказал: {}", peer.name, msg), hint.into())
        }
        PeerError::Net(kind, msg) => match kind {
            NetKind::Refused => (
                format!("ПК по адресу {} отвечает, но мост на этом порту не запущен", host),
                "Запустите «Мост» на том ПК. Если запущен — сверьте порт (у него в «Обзоре» адреса, которые он слушает).".into(),
            ),
            NetKind::Reset => (format!("соединение с {} оборвалось", host), "Повторится само. Если часто — проверьте VPN и антивирус на обоих ПК.".into()),
            NetKind::Unreachable => (
                format!("до адреса {} нет маршрута", host),
                "Этот ПК и сосед не в одной сети: включите Radmin VPN (одна сеть) или проверьте адрес соседа.".into(),
            ),
            NetKind::Timeout => (
                format!("ПК по адресу {} не отвечает", host),
                "Тот ПК выключен или спит, Radmin VPN не подключён, адрес сменился — или брандмауэр Windows на том ПК закрыл порт (разрешите «Мост» в брандмауэре — и для общественных сетей: так Windows обычно считает сеть Radmin).".into(),
            ),
            NetKind::Other => (format!("нет связи с {}: {}", host, msg), String::new()),
        },
        PeerError::Io(m) => (m.clone(), String::new()),
    }
}

// ───────────────────────── Проверка «почему нет синка» ─────────────────────────

pub async fn check_all(node: &Arc<Node>) -> Vec<PeerCheck> {
    let names: Vec<String> = node.cfg.read().peers.iter().map(|p| p.name.clone()).collect();
    let mut out = Vec::new();
    for n in names {
        if let Some(c) = check_peer(node, &n).await {
            out.push(c);
        }
    }
    out
}

struct Steps(Vec<CheckStep>);

impl Steps {
    fn add(&mut self, title: impl Into<String>, state: &str, text: impl Into<String>, fix: impl Into<String>) {
        self.0.push(CheckStep { title: title.into(), state: state.into(), text: text.into(), fix: fix.into() });
    }
}

fn finish(peer: &str, title: &str, steps: Steps) -> PeerCheck {
    let s = steps.0;
    let state = if s.iter().any(|x| x.state == "bad") {
        "bad"
    } else if s.iter().any(|x| x.state == "warn") {
        "warn"
    } else {
        "ok"
    };
    let first = s.iter().find(|x| x.state == "bad").or_else(|| s.iter().find(|x| x.state == "warn"));
    PeerCheck {
        peer: peer.into(),
        title: title.into(),
        time: time::format(&time::now()),
        state: state.into(),
        summary: first.map(|f| format!("{}: {}", f.title, f.text)).unwrap_or_else(|| "всё в порядке".into()),
        steps: s,
    }
}

/// Пошаговая проверка связи с соседом: настройки → связь к нему → имя и версия → часы → связь от него →
/// каждое пространство → общие папки → ручная отправка. Каждый шаг — что видно и что сделать.
pub async fn check_peer(node: &Arc<Node>, name: &str) -> Option<PeerCheck> {
    let peer = node.cfg.read().peer(name).cloned()?;
    let mut st = Steps(Vec::new());
    let announce0 = node.announce_urls().first().map(|u| u.trim_start_matches("http://").to_string()).unwrap_or_else(|| "<адрес этого ПК>".into());
    if !peer.enabled {
        st.add("Настройки", "bad", "сосед выключен — мост с ним не связывается", "«▶ Включить» в строке соседа");
        return Some(finish(&peer.name, &peer.title, st));
    }
    if peer.url.is_empty() {
        st.add(
            "Настройки",
            "warn",
            "это приглашение: ключ создан здесь, адрес соседа ещё не известен — ждём, когда он подключится сам",
            format!("На том ПК: «Соседи» → «Подключить» → вставить строку «{} <ключ>»", announce0),
        );
        add_inbound(node, &mut st, &peer, None, &announce0);
        return Some(finish(&peer.name, &peer.title, st));
    }
    st.add(
        "Настройки",
        "ok",
        format!(
            "адрес {}, ключ …{}{}",
            peer.url,
            &peer.key_id()[..6],
            if peer.ips.is_empty() { String::new() } else { format!(", принимаем с {}", peer.ips.join(", ")) }
        ),
        "",
    );
    let kid = peer.key_id();
    let hello = match tokio::time::timeout(Duration::from_secs(10), contact(node, &peer)).await {
        Ok(h) => h,
        Err(_) => {
            node.with_peer_state(&peer.name, |s| {
                s.online = false;
                let (e, h) = explain_timeout(&peer);
                s.error = e;
                s.hint = h;
            });
            None
        }
    };
    let peer = node.cfg.read().peer_by_key(&kid).cloned().unwrap_or(peer);
    let ps = node.peer_state(&peer.name);
    let Some(hello) = hello else {
        st.add("Связь: этот ПК → сосед", "bad", ps.error.clone(), ps.hint.clone());
        add_inbound(node, &mut st, &peer, None, &announce0);
        return Some(finish(&peer.name, &peer.title, st));
    };
    st.add("Связь: этот ПК → сосед", "ok", format!("отвечает за {} мс", ps.latency_ms.unwrap_or(0)), "");
    if !ps.warning.is_empty() {
        st.add("Имя", "warn", ps.warning.clone(), "Кнопка «Принять имя» у соседа");
    } else {
        st.add(
            "Имя",
            "ok",
            format!("назвался «{}»{}", hello.node, if peer.title.is_empty() { String::new() } else { format!(" (у вас — «{}»)", peer.title) }),
            "",
        );
    }
    if ps.legacy || version_lt(&hello.version, 1, 4) {
        st.add(
            "Версия",
            "warn",
            format!("у соседа мост {}, здесь {}: старый мост ищет соседей по имени, адреса вписываются вручную", hello.version, APP_VERSION),
            format!("Обновите мост на «{}» до {}", peer.name, APP_VERSION),
        );
    } else if hello.version != APP_VERSION {
        st.add("Версия", "info", format!("у соседа {}, здесь {}", hello.version, APP_VERSION), "");
    }
    if let Some(s) = &hello.status {
        let skew = time::now().signed_duration_since(s.time).num_seconds();
        if skew.abs() > 240 {
            st.add(
                "Время",
                "bad",
                format!("часы расходятся на {} — при 5 минутах и больше подпись перестаёт приниматься", time::skew_text(skew)),
                "«Синхронизировать время автоматически» в Windows на обоих ПК",
            );
        } else if skew.abs() > 60 {
            st.add("Время", "warn", format!("часы расходятся на {}", time::skew_text(skew)), "Включите синхронизацию времени в Windows");
        }
    }
    add_inbound(node, &mut st, &peer, Some(&hello), &announce0);
    // Пространства
    let cfg = node.cfg.read().clone();
    for s in cfg.shares.iter().filter(|s| cfg.share_has_peer(s, &peer.name)) {
        if let Some(rt) = node.share(&s.id) {
            crate::shares::check_share(node, &mut st.0, &peer, &rt, &hello).await;
        }
    }
    for id in &hello.shares {
        let mine = cfg.share(id);
        if mine.map(|s| cfg.share_has_peer(s, &peer.name)).unwrap_or(false) {
            continue;
        }
        st.add(
            format!("Пространство «{}»", id),
            "warn",
            if mine.is_some() {
                format!("«{}» раздаёт его этому ПК, но здесь он не участник «{}» — его файлы не забираются", peer.name, id)
            } else {
                format!("«{}» раздаёт его этому ПК, а здесь такого пространства нет", peer.name)
            },
            if mine.is_some() {
                format!("Добавьте «{}» в пространство (кнопка «Изменить» у соседа)", peer.name)
            } else {
                format!("Создайте пространство с id «{}» (раздел shares в bridge.json)", id)
            },
        );
    }
    // Общие папки соседа
    let client = node.client_for(&peer);
    match tokio::time::timeout(Duration::from_secs(10), client.folders()).await {
        Ok(Ok(folders)) => {
            for f in folders {
                let title = format!("Папка соседа «{}»", f.id);
                let acc = if f.access == "write" { "запись" } else { "чтение" };
                match node.folders.find_mount(&peer.name, &f.id) {
                    _ if !cfg.mounts.enabled => st.add(title, "warn", format!("доступна ({}), но подключение папок соседей выключено здесь", f.access), "mounts.enabled = true в bridge.json"),
                    None => st.add(title, "info", format!("доступна ({}), появится после ближайшего обмена", acc), ""),
                    Some(m) if m.hold => st.add(title, "bad", format!("остановлена: {}", m.hold_reason), "Решите в «Общие папки» → «Папки соседей»"),
                    Some(m) if !m.error.is_empty() => st.add(title, "bad", m.error.clone(), ""),
                    Some(m) => st.add(
                        title,
                        "ok",
                        format!("{}, файлов {}{}", acc, m.files, m.last_sync.map(|t| format!(", обмен {}", time::ago(&t))).unwrap_or_default()),
                        "",
                    ),
                }
            }
        }
        Ok(Err(e)) => {
            if e.status() != Some(404) {
                st.add("Общие папки соседа", "bad", explain(&e, &peer).0, "");
            }
        }
        Err(_) => st.add("Общие папки соседа", "bad", explain_timeout(&peer).0, ""),
    }
    let mine: Vec<String> = cfg
        .folders
        .iter()
        .filter_map(|f| cfg.access_of(f, &peer.name).map(|a| format!("{} ({})", f.id, if a == "write" { "запись" } else { "чтение" })))
        .collect();
    if !mine.is_empty() {
        st.add("Ваши папки для соседа", "info", mine.join(", "), "");
    }
    // Ручная отправка
    if !cfg.mailbox.enabled {
        st.add("Отправка файлов", "info", "ручная отправка выключена на этом ПК", "");
    } else if !cfg.mailbox.accepts(&peer.name) {
        st.add("Отправка файлов", "warn", format!("файлы от «{}» здесь не принимаются (mailbox.acceptFrom)", peer.name), format!("Добавьте «{}» или «*» в mailbox.acceptFrom", peer.name));
    } else {
        let waiting = node.mailbox.pending(node, &peer).len();
        let state = if waiting > 0 && ps.remote.as_ref().map(|r| !r.online).unwrap_or(false) { "warn" } else { "ok" };
        st.add("Отправка файлов", state, if waiting > 0 { format!("ждут, когда сосед заберёт: {}", waiting) } else { "очередь пуста".into() }, "");
    }
    Some(finish(&peer.name, &peer.title, st))
}

fn version_lt(v: &str, major: u32, minor: u32) -> bool {
    let mut it = v.split('.').map(|x| x.parse::<u32>().unwrap_or(0));
    let (a, b) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    (a, b) < (major, minor)
}

/// Шаг «связь от соседа к нам»: что видит он (из hello) и что видим мы (его запросы сюда).
fn add_inbound(node: &Node, st: &mut Steps, peer: &PeerConfig, hello: Option<&HelloResponse>, announce0: &str) {
    let ps = node.peer_state(&peer.name);
    let title = "Связь: сосед → этот ПК";
    if !ps.inbound_error.is_empty() {
        let fix = if ps.inbound_error.contains("адрес") {
            "«Добавить адрес» в блоке «Стучались» ниже"
        } else if ps.inbound_error.contains("выключен") {
            "«▶ Включить» у соседа"
        } else {
            ""
        };
        st.add(title, "bad", format!("этот ПК отказывает соседу: {}", ps.inbound_error), fix);
        return;
    }
    if let Some(you) = hello.and_then(|h| h.you.as_ref()) {
        if !you.error.is_empty() {
            let fix = reverse_fix(node, &you.url, &you.error);
            st.add(title, "bad", format!("сосед не может забрать отсюда: {}", you.error), fix);
            return;
        }
    }
    let window = (node.cfg.read().interval_sec * 4).max(60);
    match ps.last_inbound {
        Some(t) if time::now().signed_duration_since(t).num_seconds() < window => st.add(title, "ok", format!("забирает отсюда, последний раз {}", time::ago(&t)), ""),
        Some(t) => st.add(title, "warn", format!("давно не обращался: последний раз {}", time::ago(&t)), "Посмотрите на том ПК, что пишет «Проверить» у этого ПК"),
        None => st.add(
            title,
            if hello.is_none() { "info" } else { "warn" },
            "с запуска моста сосед сюда не обращался",
            format!("Если отсюда ничего не забирают — на том ПК должен быть записан этот ПК: «Подключить» → адрес {} и тот же ключ", announce0),
        ),
    }
}

pub fn steps_push(v: &mut Vec<CheckStep>, title: impl Into<String>, state: &str, text: impl Into<String>, fix: impl Into<String>) {
    v.push(CheckStep { title: title.into(), state: state.into(), text: text.into(), fix: fix.into() });
}

/// Сосед не достаёт до нас: слушаем ли мы адрес, по которому он ищет? Нет — сказать, что делать; да — значит, брандмауэр.
fn reverse_fix(node: &Node, url: &str, error: &str) -> String {
    // адрес: из hello (2.0) или из текста ошибки соседа («по адресу 26.1.2.3:5390»)
    let from_err = || {
        let i = error.find("по адресу ")? + "по адресу ".len();
        let rest = &error[i..];
        let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ':')).unwrap_or(rest.len());
        Some(format!("http://{}", &rest[..end]))
    };
    let target = if url.is_empty() { from_err() } else { Some(url.to_string()) };
    let Some((ip, port)) = target.as_deref().and_then(config::parse_url) else {
        return "Это видно на том ПК; часто помогает обновить мост там и нажать «Проверить» у этого ПК".into();
    };
    let ip = canon_ip(ip);
    let bound = node.bound.lock().clone();
    let any = bound.iter().any(|b| b.is_unspecified());
    let mine = crate::http_peer::interface_ips().iter().any(|(x, _)| *x == ip);
    let ports: Vec<u16> = node.urls.read().iter().filter_map(|u| config::parse_url(u).map(|(_, p)| p)).collect();
    if !mine && !ip.is_loopback() {
        return format!(
            "Сосед ищет этот ПК по адресу {}, но у этого ПК такого адреса сейчас нет (Radmin выключен или адрес сменился). На том ПК: «Изменить» у этого соседа → адрес одного из: {}",
            ip,
            crate::http_peer::interface_ips().iter().map(|(x, _)| x.to_string()).collect::<Vec<_>>().join(", ")
        );
    }
    if !ports.contains(&port) {
        return format!("Сосед стучится на порт {}, а мост здесь слушает порт {}. На том ПК исправьте порт у этого соседа", port, ports.first().map(|p| p.to_string()).unwrap_or_default());
    }
    if !any && !bound.contains(&ip) {
        return format!(
            "Мост здесь не слушает адрес {} (слушает: {}). Включите «listenAuto» (по умолчанию) или добавьте http://{}:{} в listen",
            ip,
            node.urls.read().join(", "),
            ip,
            port
        );
    }
    format!(
        "Мост здесь слушает {}:{}, но соединение не доходит — почти наверняка брандмауэр Windows на этом ПК закрывает входящие. Разрешите «Мост» (Мост.exe и bridge.exe) и для частных, и для общественных сетей (сеть Radmin Windows обычно считает «общественной»): Пуск → «Разрешение взаимодействия с приложением через брандмауэр»",
        ip, port
    )
}
