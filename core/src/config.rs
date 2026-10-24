//! Настройки узла (bridge.json) — тот же формат, что у узла 1.4 на .NET.

use crate::eq_ic;
use crate::ignore::{self, IgnoreRules};
use crate::path::glob;
use indexmap::IndexMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

pub const READ: &str = "read";
pub const WRITE: &str = "write";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NodeConfig {
    /// Имя узла: латиница, цифры, «-», «_» (входит в eid событий).
    pub node: String,
    pub listen: Vec<String>,
    pub allow_listen_any: bool,
    /// Слушать ещё и на всех сетях ПК (LAN, Radmin, ZeroTier…), в том числе появившихся позже. Чужих всё равно не пустит белый список и подпись.
    #[serde(skip_serializing_if = "is_true")]
    pub listen_auto: bool,
    pub data_dir: String,
    pub interval_sec: i64,
    /// Предел размера файла, МБ; 0 — без предела.
    pub max_file_mb: i64,
    /// Скорость отдачи и приёма, Мбит/с (на все передачи вместе); 0 — без ограничения.
    #[serde(skip_serializing_if = "is_zero_f")]
    pub max_up_mbit: f64,
    #[serde(skip_serializing_if = "is_zero_f")]
    pub max_down_mbit: f64,
    pub keep_history_days: i64,
    pub console_prompts: bool,
    pub peers: Vec<PeerConfig>,
    pub shares: Vec<ShareConfig>,
    pub runners: IndexMap<String, RunnerConfig>,
    pub mailbox: MailboxConfig,
    pub folders: Vec<SharedFolderConfig>,
    pub mounts: MountsConfig,
    pub ui: UiConfig,
    /// Игнор всего узла.
    pub ignore: Vec<String>,
    /// Группы соседей (2.0): «семья», «коллеги» — им дают доступ к папкам и пространствам целиком.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub groups: IndexMap<String, GroupConfig>,
    /// Голосовые каналы (2.0): зашёл — в разговоре; показ экрана — сразу в канал.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub channels: IndexMap<String, ChannelConfig>,
    /// Агент (2.0): соседи поручают задачи Claude на этом ПК.
    pub agent: AgentConfig,
    /// Телефон (2.7): мобильная страница моста, подключение по QR, HTTPS через Tailscale.
    pub mobile: MobileConfig,
    /// Ретранслятор голоса (2.8, UDP): разговор через этот ПК, когда участники не видят друг друга напрямую. 0 — выключен.
    pub turn_port: u16,
    /// Ассистент (2.8): кто из соседей может писать ассистенту на этом ПК.
    pub assistant: AssistantConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AssistantConfig {
    pub allow_from: Vec<String>,
}

impl Default for AssistantConfig {
    fn default() -> Self {
        AssistantConfig { allow_from: vec!["*".into()] }
    }
}

/// Телефон: https — «off» или «tailscale» (Tailscale раздаёт мобильную страницу по HTTPS: https://<пк>.<сеть>.ts.net/m/).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct MobileConfig {
    pub enabled: bool,
    pub https: String,
    /// Порт HTTPS у Tailscale (443, 8443 или 10000); 0 — выбрать самому.
    pub https_port: u16,
    /// Папка «на телефон»: файлы в «на телефон/<имя телефона>» телефон видит у себя.
    pub outbox: String,
}

impl Default for MobileConfig {
    fn default() -> Self {
        MobileConfig { enabled: true, https: "off".into(), https_port: 0, outbox: "на телефон".into() }
    }
}

/// Исполнитель-агент: mode — off (выключен), auto (мост сам запускает Claude Code), manual (поручение ждёт, пока его возьмёт сессия Claude на этом ПК).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentConfig {
    pub mode: String,
    /// Кто может поручать: соседи, «@группа», «*».
    pub allow_from: Vec<String>,
    /// Где работает Claude (папка проекта). Пусто — папка «агент» рядом с мостом.
    pub work_dir: String,
    /// Путь к claude.exe. Пусто — найти самому (PATH, %USERPROFILE%\\.local\\bin).
    pub claude: String,
    /// Режим разрешений Claude Code: auto (по умолчанию), acceptEdits, bypassPermissions, dontAsk…
    pub permission_mode: String,
    pub max_minutes: i64,
    pub max_parallel: i64,
    /// Предел расходов на одно поручение, $; 0 — без предела.
    pub max_budget_usd: f64,
    pub extra_args: Vec<String>,
    /// Своя общая папка для обмена файлами с исполнителями (id из folders).
    pub exchange: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            mode: "off".into(),
            allow_from: Vec::new(),
            work_dir: String::new(),
            claude: String::new(),
            permission_mode: "auto".into(),
            max_minutes: 60,
            max_parallel: 3,
            max_budget_usd: 0.0,
            extra_args: Vec::new(),
            exchange: String::new(),
        }
    }
}

/// Канал: кто может зайти — соседи, «@группа», «*» (все соседи).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelConfig {
    pub title: String,
    pub members: Vec<String>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        NodeConfig {
            node: String::new(),
            listen: vec!["http://127.0.0.1:5390".into()],
            allow_listen_any: false,
            listen_auto: true,
            data_dir: "bridge-data".into(),
            interval_sec: 10,
            max_file_mb: 0,
            max_up_mbit: 0.0,
            max_down_mbit: 0.0,
            keep_history_days: 30,
            console_prompts: true,
            peers: Vec::new(),
            shares: Vec::new(),
            runners: IndexMap::new(),
            mailbox: MailboxConfig::default(),
            folders: Vec::new(),
            mounts: MountsConfig::default(),
            ui: UiConfig::default(),
            ignore: Vec::new(),
            groups: IndexMap::new(),
            channels: IndexMap::new(),
            agent: AgentConfig::default(),
            mobile: MobileConfig::default(),
            turn_port: 3478,
            assistant: AssistantConfig::default(),
        }
    }
}

/// Группа соседей: подпись для людей и состав.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupConfig {
    pub title: String,
    pub members: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PeerConfig {
    /// Имя узла соседа: мост узнаёт его у соседа сам.
    pub name: String,
    /// Имя ещё не подтверждено соседом (временное «new-…»). Пустой url — приглашение: ждём, когда сосед подключится.
    pub pending: bool,
    /// Псевдоним для людей.
    pub title: String,
    pub url: String,
    /// Белый список адресов, с которых принимаются запросы этого соседа.
    pub ips: Vec<String>,
    pub secret: String,
    pub enabled: bool,
}

impl Default for PeerConfig {
    fn default() -> Self {
        PeerConfig { name: String::new(), pending: false, title: String::new(), url: String::new(), ips: Vec::new(), secret: String::new(), enabled: true }
    }
}

impl PeerConfig {
    pub fn key_id(&self) -> String {
        crate::signing::key_id(&self.secret)
    }

    /// Временное имя соседа, пока он не назвался.
    pub fn temp_name(secret: &str) -> String {
        format!("new-{}", &crate::signing::key_id(secret)[..6])
    }

    pub fn allows_ip(&self, ip: Option<IpAddr>) -> bool {
        let Some(ip) = ip.map(canon_ip) else { return false };
        self.ips.iter().any(|s| s.parse::<IpAddr>().map(canon_ip).ok() == Some(ip))
    }

    pub fn has_url(&self) -> bool {
        !self.url.is_empty()
    }
}

/// IPv4, отображённый в IPv6, — как обычный IPv4.
pub fn canon_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShareConfig {
    pub id: String,
    pub title: String,
    pub folder: String,
    pub peers: Vec<String>,
    pub members: Vec<String>,
    pub accept: Vec<FileAcceptRule>,
    pub records: RecordRules,
}

impl ShareConfig {
    pub fn accepts_file(&self, path: &str, from_peer: &str) -> bool {
        for r in &self.accept {
            if glob(&r.pattern, path) {
                return r.from.iter().any(|f| f == "*" || eq_ic(f, from_peer));
            }
        }
        false
    }

    pub fn has_peer(&self, name: &str) -> bool {
        self.peers.iter().any(|p| eq_ic(p, name))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileAcceptRule {
    pub pattern: String,
    pub from: Vec<String>,
}

impl Default for FileAcceptRule {
    fn default() -> Self {
        FileAcceptRule { pattern: "**".into(), from: vec!["*".into()] }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecordRules {
    pub accept: Vec<RecordAcceptRule>,
}

impl RecordRules {
    pub fn accepts(&self, author: &str, type_: &str, op: &str) -> bool {
        if self.accept.is_empty() {
            return true;
        }
        self.accept.iter().any(|r| {
            r.from.iter().any(|f| f == "*" || eq_ic(f, author))
                && r.types.iter().any(|x| x == "*" || eq_ic(x, type_))
                && r.ops.iter().any(|x| x == "*" || eq_ic(x, op))
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecordAcceptRule {
    pub from: Vec<String>,
    pub types: Vec<String>,
    pub ops: Vec<String>,
}

impl Default for RecordAcceptRule {
    fn default() -> Self {
        RecordAcceptRule { from: vec!["*".into()], types: vec!["*".into()], ops: vec!["*".into()] }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RunnerConfig {
    pub enabled: bool,
    pub title: String,
    pub share: String,
    pub exe: String,
    pub args: Vec<String>,
    /// имя → тип (path / text / int)
    pub params: IndexMap<String, String>,
    pub allow_from: Vec<String>,
    pub confirm: bool,
    pub timeout_sec: i64,
    pub output_dir: String,
    /// Подсказка людям и ассистенту: какие значения допустимы (например «cmd: pause | play»).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        RunnerConfig {
            enabled: false,
            title: String::new(),
            share: String::new(),
            exe: String::new(),
            args: Vec::new(),
            params: IndexMap::new(),
            allow_from: Vec::new(),
            confirm: true,
            timeout_sec: 600,
            output_dir: "_команды".into(),
            hint: String::new(),
            work_dir: None,
        }
    }
}

impl RunnerConfig {
    pub fn allows(&self, node: &str) -> bool {
        self.allow_from.iter().any(|a| a == "*" || eq_ic(a, node))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MailboxConfig {
    pub enabled: bool,
    pub outgoing: String,
    pub incoming: String,
    pub accept_from: Vec<String>,
}

impl Default for MailboxConfig {
    fn default() -> Self {
        MailboxConfig { enabled: true, outgoing: "отправка".into(), incoming: "приём".into(), accept_from: vec!["*".into()] }
    }
}

impl MailboxConfig {
    pub fn accepts(&self, peer: &str) -> bool {
        self.enabled && self.accept_from.iter().any(|a| a == "*" || eq_ic(a, peer))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SharedFolderConfig {
    pub id: String,
    pub title: String,
    pub path: String,
    /// сосед (или «*», или «@группа») → read / write
    pub access: IndexMap<String, String>,
    pub ignore: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MountsConfig {
    pub enabled: bool,
    pub root: String,
    pub trash_minutes: i64,
    /// «сосед/папка» → шаблоны
    pub ignore: IndexMap<String, Vec<String>>,
}

impl Default for MountsConfig {
    fn default() -> Self {
        MountsConfig { enabled: true, root: "общие".into(), trash_minutes: 10, ignore: IndexMap::new() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiConfig {
    pub auto_open: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig { auto_open: true }
    }
}

pub fn is_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Значение в словаре без учёта регистра ключа.
pub fn get_ic<'a, V>(map: &'a IndexMap<String, V>, key: &str) -> Option<&'a V> {
    map.get(key).or_else(|| map.iter().find(|(k, _)| eq_ic(k, key)).map(|(_, v)| v))
}

impl NodeConfig {
    pub fn load(file: &std::path::Path) -> Result<NodeConfig, String> {
        let text = std::fs::read_to_string(file).map_err(|e| format!("Не прочитать {}: {}", file.display(), e))?;
        let text = text.trim_start_matches('\u{feff}');
        let cfg: NodeConfig = parse_json5ish(text).map_err(|e| format!("Ошибка в {}: {}", file.display(), e))?;
        let errors = cfg.validate();
        if !errors.is_empty() {
            return Err(format!("Ошибки в {}:\n- {}", file.display(), errors.join("\n- ")));
        }
        Ok(cfg)
    }

    pub fn peer(&self, name: &str) -> Option<&PeerConfig> {
        self.peers.iter().find(|p| eq_ic(&p.name, name))
    }

    pub fn peer_mut(&mut self, name: &str) -> Option<&mut PeerConfig> {
        self.peers.iter_mut().find(|p| eq_ic(&p.name, name))
    }

    pub fn peer_by_key(&self, kid: &str) -> Option<&PeerConfig> {
        if kid.is_empty() {
            return None;
        }
        self.peers.iter().find(|p| p.key_id() == kid)
    }

    pub fn share(&self, id: &str) -> Option<&ShareConfig> {
        self.shares.iter().find(|s| eq_ic(&s.id, id))
    }

    pub fn folder(&self, id: &str) -> Option<&SharedFolderConfig> {
        self.folders.iter().find(|f| eq_ic(&f.id, id))
    }

    /// Состоит ли сосед в группе.
    /// Сосед может зайти в канал (напрямую, через группу или «*»).
    pub fn channel_has(&self, c: &ChannelConfig, peer: &str) -> bool {
        c.members.iter().any(|m| m == "*" && self.peer(peer).is_some() || eq_ic(m, peer) || m.strip_prefix('@').map(|g| self.in_group(g, peer)).unwrap_or(false))
    }

    /// Участники канала по именам (без этого узла).
    pub fn channel_members(&self, c: &ChannelConfig) -> Vec<String> {
        self.peers.iter().filter(|p| p.enabled && !p.pending && self.channel_has(c, &p.name)).map(|p| p.name.clone()).collect()
    }

    /// Может ли сосед поручать агенту этого ПК.
    pub fn agent_allows(&self, peer: &str) -> bool {
        self.agent.mode != "off" && self.agent.allow_from.iter().any(|m| m == "*" && self.peer(peer).is_some() || eq_ic(m, peer) || m.strip_prefix('@').map(|g| self.in_group(g, peer)).unwrap_or(false))
    }

    /// Список «сосед | @группа | *» включает соседа.
    pub fn list_allows(&self, list: &[String], peer: &str) -> bool {
        list.iter().any(|m| m == "*" && self.peer(peer).is_some() || eq_ic(m, peer) || m.strip_prefix('@').map(|g| self.in_group(g, peer)).unwrap_or(false))
    }

    pub fn in_group(&self, group: &str, peer: &str) -> bool {
        get_ic(&self.groups, group).map(|g| g.members.iter().any(|m| eq_ic(m, peer))).unwrap_or(false)
    }

    /// Право соседа на свою общую папку: прямое → по группе (лучшее из групп) → «*».
    pub fn access_of(&self, f: &SharedFolderConfig, peer: &str) -> Option<&'static str> {
        let norm = |a: &str| if a == WRITE { Some(WRITE) } else if a == READ { Some(READ) } else { None };
        if let Some(a) = f.access.iter().find(|(k, _)| eq_ic(k, peer)).and_then(|(_, v)| norm(v)) {
            return Some(a);
        }
        let mut best: Option<&'static str> = None;
        for (k, v) in &f.access {
            if let Some(g) = k.strip_prefix('@') {
                if self.in_group(g, peer) {
                    match norm(v) {
                        Some(WRITE) => return Some(WRITE),
                        Some(r) => best = Some(r),
                        None => {}
                    }
                }
            }
        }
        best.or_else(|| f.access.get("*").and_then(|v| norm(v)))
    }

    /// Участвует ли сосед в пространстве: прямо или через группу «@имя» в списке peers.
    pub fn share_has_peer(&self, s: &ShareConfig, peer: &str) -> bool {
        s.peers.iter().any(|p| eq_ic(p, peer) || p.strip_prefix('@').map(|g| self.in_group(g, peer)).unwrap_or(false))
    }

    /// Соседи пространства (группы раскрыты).
    pub fn share_peers(&self, s: &ShareConfig) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for p in &s.peers {
            if let Some(g) = p.strip_prefix('@') {
                if let Some(gr) = get_ic(&self.groups, g) {
                    for m in &gr.members {
                        if !out.iter().any(|x| eq_ic(x, m)) {
                            out.push(m.clone());
                        }
                    }
                }
            } else if !out.iter().any(|x| eq_ic(x, p)) {
                out.push(p.clone());
            }
        }
        out
    }

    pub fn global_ignore(&self) -> Arc<IgnoreRules> {
        cached_ignore(self.ignore.iter())
    }

    pub fn validate(&self) -> Vec<String> {
        let mut e = Vec::new();
        if !is_name(&self.node) {
            e.push("node: имя узла — латиница, цифры, «-», «_» (до 64)".into());
        }
        for l in &self.listen {
            match parse_listen(l) {
                Some((ip, _)) if (ip.is_unspecified()) && !self.allow_listen_any => {
                    e.push(format!("listen: «{}» слушает все сети — укажите конкретный адрес (или allowListenAny: true)", l))
                }
                Some(_) => {}
                None => e.push(format!("listen: адрес вида http://ip:порт — {}", l)),
            }
        }
        if self.interval_sec < 1 {
            e.push("intervalSec ≥ 1".into());
        }
        let mut names: Vec<String> = Vec::new();
        let mut keys: Vec<&str> = Vec::new();
        for p in &self.peers {
            if !is_name(&p.name) {
                e.push(format!("peers: имя соседа «{}»", p.name));
            } else if names.iter().any(|n| eq_ic(n, &p.name)) {
                e.push(format!("peers: повтор «{}»", p.name));
            } else {
                names.push(p.name.clone());
            }
            if eq_ic(&p.name, &self.node) {
                e.push("peers: сосед с именем самого узла".into());
            }
            let url_ok = if p.url.is_empty() { p.pending } else { parse_url(&p.url).is_some() };
            if !url_ok {
                e.push(format!("peers.{}.url: адрес вида http://ip:порт", p.name));
            }
            if p.secret.chars().count() < 24 {
                e.push(format!("peers.{}.secret: не короче 24 знаков (bridge pair)", p.name));
            } else if keys.contains(&p.secret.as_str()) {
                e.push(format!("peers.{}.secret: такой же ключ уже у другого соседа — у каждой пары свой ключ", p.name));
            } else {
                keys.push(&p.secret);
            }
            for ip in &p.ips {
                if ip.parse::<IpAddr>().is_err() {
                    e.push(format!("peers.{}.ips: «{}» — не IP-адрес", p.name, ip));
                }
            }
        }
        for (g, gr) in &self.groups {
            if !is_name(g) {
                e.push(format!("groups: имя группы «{}» — латиница, цифры, «-», «_»", g));
            }
            for m in &gr.members {
                if self.peer(m).is_none() {
                    e.push(format!("groups.{}: нет соседа «{}»", g, m));
                }
            }
        }
        let peer_or_group = |p: &str| -> bool {
            if let Some(g) = p.strip_prefix('@') {
                get_ic(&self.groups, g).is_some()
            } else {
                self.peer(p).is_some()
            }
        };
        let mut share_ids: Vec<&str> = Vec::new();
        for s in &self.shares {
            if !is_name(&s.id) {
                e.push(format!("shares: id «{}»", s.id));
            } else if share_ids.iter().any(|x| eq_ic(x, &s.id)) {
                e.push(format!("shares: повтор «{}»", s.id));
            } else {
                share_ids.push(&s.id);
            }
            if s.folder.trim().is_empty() {
                e.push(format!("shares.{}.folder", s.id));
            }
            for p in &s.peers {
                if !peer_or_group(p) {
                    e.push(format!("shares.{}.peers: нет соседа «{}»", s.id, p));
                }
            }
        }
        let mut folder_ids: Vec<&str> = Vec::new();
        for f in &self.folders {
            if !is_name(&f.id) {
                e.push(format!("folders: id «{}» — латиница, цифры, «-», «_»", f.id));
            } else if folder_ids.iter().any(|x| eq_ic(x, &f.id)) {
                e.push(format!("folders: повтор «{}»", f.id));
            } else {
                folder_ids.push(&f.id);
            }
            if f.path.trim().is_empty() {
                e.push(format!("folders.{}.path", f.id));
            }
            for (peer, acc) in &f.access {
                if peer != "*" && !peer_or_group(peer) {
                    e.push(format!("folders.{}.access: нет соседа «{}»", f.id, peer));
                }
                if acc != READ && acc != WRITE {
                    e.push(format!("folders.{}.access.{}: read или write", f.id, peer));
                }
            }
        }
        for (id, c) in &self.channels {
            if !is_name(id) {
                e.push(format!("channels: имя канала «{}» — латиница, цифры, «-», «_»", id));
            }
            for m in &c.members {
                if m != "*" && !peer_or_group(m) {
                    e.push(format!("channels.{}: нет соседа или группы «{}»", id, m));
                }
            }
        }
        if !["off", "auto", "manual"].contains(&self.agent.mode.as_str()) {
            e.push("agent.mode: off, auto или manual".into());
        }
        for m in &self.agent.allow_from {
            if m != "*" && !peer_or_group(m) {
                e.push(format!("agent.allowFrom: нет соседа или группы «{}»", m));
            }
        }
        e.extend(ignore::validate(&self.ignore, "ignore"));
        for f in &self.folders {
            e.extend(ignore::validate(&f.ignore, &format!("folders.{}.ignore", f.id)));
        }
        for (k, v) in &self.mounts.ignore {
            e.extend(ignore::validate(v, &format!("mounts.ignore.{}", k)));
        }
        for (name, r) in &self.runners {
            if !is_name(name) {
                e.push(format!("runners: имя «{}»", name));
            }
            if !r.share.is_empty() && self.share(&r.share).is_none() {
                e.push(format!("runners.{}.share: нет пространства «{}»", name, r.share));
            }
            if r.exe.is_empty() {
                e.push(format!("runners.{}.exe", name));
            }
        }
        e
    }

    /// Новый общий ключ пары (≈190 бит).
    pub fn new_secret() -> String {
        use base64::Engine;
        let bytes: [u8; 24] = rand::random();
        let b = base64::engine::general_purpose::STANDARD.encode(bytes);
        format!("BRS-{}", b.replace('+', "A").replace('/', "B").trim_end_matches('='))
    }
}

/// Разобрать адрес соседа: «26.1.2.3», «26.1.2.3:5391», «http://26.1.2.3:5390/».
pub fn parse_address(text: &str, default_port: u16) -> Result<(String, u16), String> {
    let mut t = text.trim().trim_end_matches('/').to_string();
    if t.len() >= 7 && t[..7].eq_ignore_ascii_case("http://") {
        t = t[7..].to_string();
    }
    if t.is_empty() {
        return Err("нужен адрес соседа, например 26.12.34.56".into());
    }
    let mut port = default_port;
    if let Some(colon) = t.rfind(':') {
        if t.find(':') == Some(colon) && colon > 0 {
            port = t[colon + 1..].parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| "порт — число 1–65535".to_string())?;
            t.truncate(colon);
        }
    }
    let ip: Ipv4Addr = t
        .parse()
        .map_err(|_| "адрес — IPv4, например 26.12.34.56 или 192.168.1.20 (можно с портом: 26.12.34.56:5390)".to_string())?;
    if ip.is_unspecified() {
        return Err("0.0.0.0 — не адрес; нужен адрес соседа".into());
    }
    Ok((ip.to_string(), port))
}

/// «адрес ключ» одной строкой → адрес и ключ.
pub fn split_invite(address: &str, secret: &str) -> (String, String) {
    let mut a = address.trim().to_string();
    let mut s = secret.trim().to_string();
    if s.is_empty() {
        let parts: Vec<&str> = a.split(|c: char| c.is_whitespace() || c == ';').filter(|x| !x.is_empty()).collect();
        if parts.len() == 2 {
            let (x, y) = (parts[0].to_string(), parts[1].to_string());
            a = x;
            s = y;
        }
    }
    (a, s)
}

/// «http://ip:порт» → (ip, порт). «*» и «+» — все адреса.
pub fn parse_listen(l: &str) -> Option<(IpAddr, u16)> {
    let t = l.replace(['*', '+'], "0.0.0.0");
    parse_url(&t)
}

pub fn parse_url(u: &str) -> Option<(IpAddr, u16)> {
    let rest = u.strip_prefix("http://").or_else(|| u.strip_prefix("HTTP://"))?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.split('/').next()?;
    if let Some(stripped) = rest.strip_prefix('[') {
        let end = stripped.find(']')?;
        let ip: IpAddr = stripped[..end].parse().ok()?;
        let port = stripped[end + 1..].strip_prefix(':').map(|p| p.parse().ok()).unwrap_or(Some(80))?;
        return Some((ip, port));
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (rest, 80),
    };
    let ip: IpAddr = if host.eq_ignore_ascii_case("localhost") { IpAddr::V4(Ipv4Addr::LOCALHOST) } else { host.parse().ok()? };
    Some((ip, port))
}

/// JSON с комментариями и висячими запятыми (как разрешает .NET) → значение.
pub fn parse_json5ish<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    match serde_json::from_str(text) {
        Ok(v) => Ok(v),
        Err(first) => {
            let cleaned = strip_comments_and_commas(text);
            serde_json::from_str(&cleaned).map_err(|_| first.to_string())
        }
    }
}

fn strip_comments_and_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_str = false;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

static IGNORE_CACHE: Mutex<Option<HashMap<String, Arc<IgnoreRules>>>> = Mutex::new(None);

/// Скомпилированный набор шаблонов (кэш по содержимому).
pub fn cached_ignore<'a, I: Iterator<Item = &'a String>>(patterns: I) -> Arc<IgnoreRules> {
    let list: Vec<&String> = patterns.collect();
    if list.is_empty() {
        return Arc::new(IgnoreRules::empty());
    }
    let key = list.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n");
    let mut g = IGNORE_CACHE.lock();
    let map = g.get_or_insert_with(HashMap::new);
    if map.len() > 1000 {
        map.clear();
    }
    map.entry(key).or_insert_with(|| Arc::new(IgnoreRules::new(list.iter().map(|s| s.as_str())))).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address() {
        assert_eq!(parse_address("26.1.2.3", 5390).unwrap(), ("26.1.2.3".into(), 5390));
        assert_eq!(parse_address("http://26.1.2.3:5391/", 5390).unwrap(), ("26.1.2.3".into(), 5391));
        assert!(parse_address("0.0.0.0", 5390).is_err());
        assert!(parse_address("host", 5390).is_err());
        assert_eq!(split_invite("26.1.2.3:5390 BRS-x", ""), ("26.1.2.3:5390".into(), "BRS-x".into()));
    }

    #[test]
    fn config_parse_and_groups() {
        let text = r#"{ "node": "a", "listen": ["http://127.0.0.1:5390",], // комментарий
          "peers": [{"name":"b","url":"http://1.2.3.4:5390","ips":["1.2.3.4"],"secret":"BRS-012345678901234567890123"}],
          "groups": {"family": {"title": "Семья", "members": ["b"]}},
          "folders": [{"id":"f","path":"f","access":{"@family":"write"}}] }"#;
        let cfg: NodeConfig = parse_json5ish(text).unwrap();
        assert!(cfg.validate().is_empty(), "{:?}", cfg.validate());
        assert_eq!(cfg.access_of(&cfg.folders[0], "B"), Some(WRITE));
        assert_eq!(cfg.access_of(&cfg.folders[0], "c"), None);
    }

    #[test]
    fn listen_validation() {
        let mut c = NodeConfig { node: "a".into(), ..Default::default() };
        c.listen = vec!["http://0.0.0.0:5390".into()];
        assert!(!c.validate().is_empty());
        c.allow_listen_any = true;
        assert!(c.validate().is_empty());
        assert!(NodeConfig::new_secret().len() >= 24);
    }
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_zero_f(v: &f64) -> bool {
    *v <= 0.0
}
