//! Сообщения протокола и записи. Имена полей и формат — как у узла 1.4 (.NET), чтобы узлы понимали друг друга.

use crate::time::{self, Time};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub type JsonObject = Map<String, Value>;

// ───────────────────────── Файлы ─────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
    pub size: i64,
    #[serde(with = "time::ser")]
    pub modified: Time,
    pub rev: i64,
}

impl Default for FileEntry {
    fn default() -> Self {
        FileEntry { path: String::new(), sha256: String::new(), size: 0, modified: time::min(), rev: 0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileManifest {
    pub node: String,
    pub share: String,
    #[serde(with = "time::ser")]
    pub time: Time,
    pub files: Vec<FileEntry>,
}

impl Default for FileManifest {
    fn default() -> Self {
        FileManifest { node: String::new(), share: String::new(), time: time::min(), files: Vec::new() }
    }
}

// ───────────────────────── Соседи ─────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NodeStatus {
    pub node: String,
    pub version: String,
    #[serde(with = "time::ser")]
    pub started: Time,
    #[serde(with = "time::ser")]
    pub time: Time,
    /// ok / error
    pub state: String,
    pub message: String,
    pub waiting_approval: i64,
}

impl Default for NodeStatus {
    fn default() -> Self {
        NodeStatus {
            node: String::new(),
            version: crate::APP_VERSION.into(),
            started: time::min(),
            time: time::min(),
            state: "ok".into(),
            message: String::new(),
            waiting_approval: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HelloResponse {
    pub protocol: i32,
    pub node: String,
    pub version: String,
    pub shares: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<NodeStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub you: Option<PeerView>,
    /// (2.0) Отпечаток файлов каждого пространства: не изменился — список файлов можно не запрашивать.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub revs: std::collections::BTreeMap<String, String>,
    /// (2.0) Каналы этого узла, куда может зайти спрашивающий.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ChannelInfo>,
    /// (2.0) Агент этого узла для спрашивающего: «auto», «manual» или пусто (не принимает поручения от него).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent: String,
    /// (2.8) Ретранслятор голоса этого узла для спрашивающего: порт UDP и временный ключ.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<TurnInfo>,
    /// (2.8) Ассистент живёт на этом узле — спрашивающему можно писать ему.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant: Option<AssistantInfo>,
    /// (2.8) Исполнители этого узла, которые спрашивающему можно запускать.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub runners: Vec<RunnerInfo>,
    /// (2.9) Отпечаток аватарки (картинку отдаёт GET /peer/avatar).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub avatar: String,
    /// (2.12) Отпечаток звука входа в канал (звук отдаёт GET /peer/joinsound).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sound: String,
    /// (2.14) Отпечаток своего звука пинга (его слышит и тот, кто пингует: GET /peer/pingsound).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ping: String,
    /// (3.1) Отпечаток плитки участника (стиль и картинка: GET /peer/tile, /peer/tile/img).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tile: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AssistantInfo {
    pub host: String,
    pub title: String,
    pub online: bool,
    pub version: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RunnerInfo {
    pub name: String,
    pub title: String,
    pub share: String,
    pub params: indexmap::IndexMap<String, String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
    pub confirm: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TurnInfo {
    pub port: u16,
    pub username: String,
    pub credential: String,
}

/// Канал для соседей: ключ «владелец:id», участники — имена узлов (вместе с владельцем).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelInfo {
    pub owner: String,
    pub id: String,
    pub title: String,
    pub members: Vec<String>,
    /// (3.2) Отпечаток оформления канала (иконка, подложка): GET /peer/chan/art, /peer/chan/img.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub art: String,
}

impl Default for HelloResponse {
    fn default() -> Self {
        HelloResponse {
            protocol: crate::PROTOCOL,
            node: String::new(),
            version: crate::APP_VERSION.into(),
            shares: Vec::new(),
            status: None,
            you: None,
            revs: std::collections::BTreeMap::new(),
            channels: Vec::new(),
            agent: String::new(),
            turn: None,
            assistant: None,
            runners: Vec::new(),
            avatar: String::new(),
            sound: String::new(),
            ping: String::new(),
            tile: String::new(),
        }
    }
}

/// Связь соседа с нами глазами соседа.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PeerView {
    pub name: String,
    pub online: bool,
    #[serde(with = "time::opt")]
    pub last_seen: Option<Time>,
    pub error: String,
    /// По какому адресу сосед ищет нас (2.0): если мы там не слушаем — понятно, что чинить.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotifyRequest {
    pub share: String,
    /// files / events / all / mailbox / folders
    pub kind: String,
}

impl Default for NotifyRequest {
    fn default() -> Self {
        NotifyRequest { share: String::new(), kind: "all".into() }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MailboxList {
    pub from: String,
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MailboxAck {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MailboxEvent {
    #[serde(with = "time::ser")]
    pub time: Time,
    /// sent / received / error
    pub kind: String,
    pub peer: String,
    pub path: String,
    pub size: i64,
    pub message: String,
}

impl Default for MailboxEvent {
    fn default() -> Self {
        MailboxEvent { time: time::now(), kind: String::new(), peer: String::new(), path: String::new(), size: 0, message: String::new() }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SharedFolderInfo {
    pub id: String,
    pub title: String,
    pub access: String,
    pub files: i64,
    pub bytes: i64,
    pub ignore: Vec<String>,
    /// (2.0) Отпечаток файлов папки: не изменился — список можно не запрашивать.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub rev: String,
    /// (2.6) Предел размера файла у хозяина, байт; 0 — без предела. Больше — не отправлять (хозяин всё равно откажет).
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub max_bytes: i64,
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

/// Отпечаток списка файлов (путь + sha): меняется при любом изменении.
pub fn files_rev(files: &[FileEntry]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for f in files {
        h.update(f.path.to_lowercase().as_bytes());
        h.update([0]);
        h.update(f.sha256.as_bytes());
        h.update([1]);
    }
    hex::encode(&h.finalize()[..12])
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FolderManifest {
    pub folder: String,
    pub access: String,
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FolderDeleteRequest {
    pub path: String,
    pub base: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EventsPage {
    pub events: Vec<BridgeEvent>,
    pub more: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ErrorResponse {
    pub error: String,
}

// ───────────────────────── Записи ─────────────────────────

pub mod ops {
    pub const CREATE: &str = "create";
    pub const SET: &str = "set";
    pub const TRANSITION: &str = "transition";
    pub const COMMENT: &str = "comment";
    pub const LINK: &str = "link";
    pub const UNLINK: &str = "unlink";
    pub const ATTACH: &str = "attach";
    pub const ACK: &str = "ack";
    pub const REVERT: &str = "revert";
    pub const ALL: &[&str] = &[CREATE, SET, TRANSITION, COMMENT, LINK, UNLINK, ATTACH, ACK, REVERT];
}

pub mod record_types {
    pub const TASK: &str = "task";
    pub const COMMENT: &str = "comment";
    pub const STATUS: &str = "status";
    pub const DTO: &str = "dto";
    pub const DECISION: &str = "decision";
    pub const COMMAND: &str = "command";
}

/// Событие записи: неизменяемо, уникально по eid = «автор:seq». Порядок применения — (clock, author, seq).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BridgeEvent {
    pub eid: String,
    pub share: String,
    pub record: String,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub op: String,
    pub base: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev: Option<BTreeMap<String, String>>,
    pub clock: i64,
    pub author: String,
    pub seq: i64,
    pub actor: String,
    #[serde(with = "time::ser")]
    pub time: Time,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<JsonObject>,
    pub note: String,
}

impl Default for BridgeEvent {
    fn default() -> Self {
        BridgeEvent {
            eid: String::new(),
            share: String::new(),
            record: String::new(),
            type_: String::new(),
            schema: None,
            op: String::new(),
            base: 0,
            prev: None,
            clock: 0,
            author: String::new(),
            seq: 0,
            actor: String::new(),
            time: time::min(),
            payload: None,
            note: String::new(),
        }
    }
}

pub fn make_eid(author: &str, seq: i64) -> String {
    format!("{}:{}", author, seq)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConflictValue {
    pub eid: String,
    pub author: String,
    pub actor: String,
    pub value: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AckInfo {
    pub eid: String,
    pub author: String,
    pub actor: String,
    pub level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub version: i64,
    pub text: String,
    #[serde(with = "time::ser")]
    pub time: Time,
}

impl Default for AckInfo {
    fn default() -> Self {
        AckInfo { eid: String::new(), author: String::new(), actor: String::new(), level: String::new(), target: None, version: 0, text: String::new(), time: time::min() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommentInfo {
    pub eid: String,
    pub author: String,
    pub actor: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(with = "time::ser")]
    pub time: Time,
    pub reverted: bool,
}

impl Default for CommentInfo {
    fn default() -> Self {
        CommentInfo { eid: String::new(), author: String::new(), actor: String::new(), text: String::new(), reply_to: None, time: time::min(), reverted: false }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AttachmentInfo {
    pub eid: String,
    pub path: String,
    pub sha256: String,
    pub size: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub present: Option<bool>,
    pub reverted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LinkInfo {
    pub target: String,
    pub rel: String,
    pub eid: String,
}

impl Default for LinkInfo {
    fn default() -> Self {
        LinkInfo { target: String::new(), rel: "relates".into(), eid: String::new() }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InvalidEvent {
    pub eid: String,
    pub reason: String,
}

/// Состояние записи — свёртка её событий.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecordState {
    pub id: String,
    pub share: String,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub creator: String,
    pub creator_actor: String,
    #[serde(with = "time::ser")]
    pub created: Time,
    #[serde(with = "time::ser")]
    pub updated: Time,
    pub version: i64,
    pub last_eid: String,
    pub fields: JsonObject,
    pub setters: BTreeMap<String, String>,
    pub conflicts: BTreeMap<String, Vec<ConflictValue>>,
    pub comments: Vec<CommentInfo>,
    pub attachments: Vec<AttachmentInfo>,
    pub links: Vec<LinkInfo>,
    pub acks: Vec<AckInfo>,
    pub invalid: Vec<InvalidEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered: Option<BTreeMap<String, bool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_errors: Option<Vec<String>>,
    // Производные (как вычисляемые свойства в .NET): обновляются по полям
    #[serde(skip_serializing_if = "Option::is_none", skip_deserializing)]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", skip_deserializing)]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", skip_deserializing)]
    pub assignee: Option<String>,
}

impl Default for RecordState {
    fn default() -> Self {
        RecordState {
            id: String::new(),
            share: String::new(),
            type_: String::new(),
            schema: None,
            creator: String::new(),
            creator_actor: String::new(),
            created: time::min(),
            updated: time::min(),
            version: 0,
            last_eid: String::new(),
            fields: JsonObject::new(),
            setters: BTreeMap::new(),
            conflicts: BTreeMap::new(),
            comments: Vec::new(),
            attachments: Vec::new(),
            links: Vec::new(),
            acks: Vec::new(),
            invalid: Vec::new(),
            delivered: None,
            schema_errors: None,
            status: None,
            title: None,
            assignee: None,
        }
    }
}

impl RecordState {
    fn str_field(&self, k: &str) -> Option<String> {
        match self.fields.get(k) {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        }
    }

    /// Обновить производные поля (status, title, assignee).
    pub fn sync_derived(&mut self) {
        self.status = self.str_field("status");
        self.title = self.str_field("title");
        self.assignee = self.str_field("assignee");
    }

    pub fn status(&self) -> Option<&str> {
        self.fields.get("status").and_then(|v| v.as_str())
    }

    pub fn title(&self) -> Option<&str> {
        self.fields.get("title").and_then(|v| v.as_str())
    }

    /// Удалена (поле deleted = true): скрыта из списков на всех узлах, вернуть — действие restore.
    pub fn is_deleted(&self) -> bool {
        self.fields.get("deleted").and_then(|v| v.as_bool()).unwrap_or(false)
    }

    pub fn assignee(&self) -> Option<&str> {
        self.fields.get("assignee").and_then(|v| v.as_str())
    }

    pub fn is_created(&self) -> bool {
        !time::is_min(&self.created)
    }
}

/// Действие человека или программы: узел превращает его в событие, сам заполняя автора, часы и prev.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BridgeAction {
    /// create / set / transition / comment / link / unlink / attach / ack / revert / approve / reject
    pub action: String,
    pub share: Option<String>,
    pub record: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub schema: Option<String>,
    pub fields: Option<JsonObject>,
    pub to: Option<String>,
    pub text: Option<String>,
    pub reply_to: Option<String>,
    pub level: Option<String>,
    pub target: Option<String>,
    pub rel: Option<String>,
    pub path: Option<String>,
    pub title: Option<String>,
    pub actor: String,
    pub note: String,
    pub expect_version: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionResult {
    pub ok: bool,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eid: Option<String>,
    pub version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<RecordState>,
}

impl ActionResult {
    pub fn err(e: impl Into<String>) -> Self {
        ActionResult { error: e.into(), ..Default::default() }
    }
}

/// Вектор «автор → seq» в строку «a:5,b:3».
pub fn vector_text(v: &BTreeMap<String, i64>) -> String {
    let mut items: Vec<(&String, &i64)> = v.iter().collect();
    items.sort_by(|a, b| crate::cmp_ordinal(a.0, b.0));
    items.iter().map(|(k, s)| format!("{}:{}", k, s)).collect::<Vec<_>>().join(",")
}

pub fn parse_vector(text: &str) -> BTreeMap<String, i64> {
    let mut v = BTreeMap::new();
    for part in text.split(',').filter(|p| !p.is_empty()) {
        if let Some(i) = part.rfind(':') {
            if i > 0 {
                if let Ok(n) = part[i + 1..].parse::<i64>() {
                    v.insert(part[..i].to_string(), n);
                }
            }
        }
    }
    v
}
