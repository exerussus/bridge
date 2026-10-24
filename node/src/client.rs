//! Клиент к узлу-соседу: каждый запрос подписан ключом пары. Сосед находит нас по отпечатку ключа (kid).
//! Сосед старой версии (до 1.4) отвечает «неверная подпись» — тогда клиент переходит на старую подпись.

use bridge_core::config::PeerConfig;
use bridge_core::proto::*;
use bridge_core::{path, signing, API_PREFIX, AUTH_SCHEME};
use futures_util::StreamExt;
use reqwest::{Method, StatusCode};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// Ошибка обращения к соседу.
#[derive(Debug, Clone)]
pub enum PeerError {
    /// Сосед ответил отказом: код и причина.
    Http(u16, String),
    /// Нет связи: вид и текст.
    Net(NetKind, String),
    /// Локальная ошибка (диск).
    Io(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NetKind {
    Refused,
    Timeout,
    Unreachable,
    Reset,
    Other,
}

impl std::fmt::Display for PeerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerError::Http(_, m) => write!(f, "{}", m),
            PeerError::Net(_, m) => write!(f, "{}", m),
            PeerError::Io(m) => write!(f, "{}", m),
        }
    }
}

impl PeerError {
    pub fn status(&self) -> Option<u16> {
        match self {
            PeerError::Http(c, _) => Some(*c),
            _ => None,
        }
    }
}

fn classify(e: &reqwest::Error) -> PeerError {
    let text = e.to_string();
    let mut chain = String::new();
    let mut src: Option<&dyn std::error::Error> = Some(e);
    while let Some(s) = src {
        chain.push_str(&s.to_string().to_lowercase());
        chain.push(' ');
        if let Some(io) = s.downcast_ref::<std::io::Error>() {
            let k = match io.kind() {
                std::io::ErrorKind::ConnectionRefused => NetKind::Refused,
                std::io::ErrorKind::TimedOut => NetKind::Timeout,
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted => NetKind::Reset,
                _ => NetKind::Other,
            };
            if k != NetKind::Other {
                return PeerError::Net(k, text);
            }
        }
        src = s.source();
    }
    let kind = if e.is_timeout() || chain.contains("timed out") {
        NetKind::Timeout
    } else if chain.contains("refused") {
        NetKind::Refused
    } else if chain.contains("unreachable") || chain.contains("no route") {
        NetKind::Unreachable
    } else if chain.contains("reset") || chain.contains("closed") {
        NetKind::Reset
    } else {
        NetKind::Other
    };
    PeerError::Net(kind, text)
}

/// Кодирование для пути и запроса: всё, кроме незарезервированных знаков (как Uri.EscapeDataString).
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(*b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

pub struct PeerClient {
    http: reqwest::Client,
    self_name: String,
    secret: String,
    base: String,
    announce: String,
    pub peer_name: String,
    legacy: AtomicBool,
    progress: Option<std::sync::Arc<crate::progress::Transfers>>,
    limits: Option<std::sync::Arc<crate::progress::Limits>>,
}

pub enum Body {
    Empty,
    Json(Vec<u8>),
    /// Файл потоком: подписывается его sha из запроса; третье — путь для показа хода передачи.
    File(std::path::PathBuf, String, String),
}

impl PeerClient {
    pub fn new(self_name: &str, peer: &PeerConfig, announce: &[String], timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(timeout)
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(8)
            .tcp_nodelay(true)
            .build()
            .expect("http client");
        PeerClient {
            http,
            self_name: self_name.into(),
            secret: peer.secret.clone(),
            base: peer.url.trim_end_matches('/').to_string(),
            announce: announce.join(","),
            peer_name: peer.name.clone(),
            legacy: AtomicBool::new(false),
            progress: None,
            limits: None,
        }
    }

    pub fn with_progress(mut self, p: std::sync::Arc<crate::progress::Transfers>, l: std::sync::Arc<crate::progress::Limits>) -> Self {
        self.progress = Some(p);
        self.limits = Some(l);
        self
    }

    pub fn legacy(&self) -> bool {
        self.legacy.load(Ordering::Relaxed)
    }

    async fn build(&self, method: &Method, rel: &str, body: &Body) -> Result<reqwest::Request, PeerError> {
        let url = format!("{}{}", self.base, rel);
        let parsed = reqwest::Url::parse(&url).map_err(|e| PeerError::Io(format!("адрес соседа «{}»: {}", url, e)))?;
        let pq = match parsed.query() {
            Some(q) => format!("{}?{}", parsed.path(), q),
            None => parsed.path().to_string(),
        };
        let body_sha = match body {
            Body::Empty => signing::empty_body_sha(),
            Body::Json(b) => path::sha256_bytes(b),
            Body::File(_, sha, _) => sha.clone(),
        };
        let auth = format!("{} {}", AUTH_SCHEME, signing::header(&self.self_name, &self.secret, method.as_str(), &pq, &body_sha, self.legacy()));
        let mut rb = self.http.request(method.clone(), parsed).header("Authorization", auth);
        match body {
            Body::Empty => {}
            Body::Json(b) => rb = rb.header("Content-Type", "application/json; charset=utf-8").body(b.clone()),
            Body::File(p, _, label) => {
                let f = tokio::fs::File::open(p).await.map_err(|e| PeerError::Io(e.to_string()))?;
                let len = f.metadata().await.map(|m| m.len()).unwrap_or(0);
                let guard = self.progress.as_ref().map(|t| t.start("up", &self.peer_name, "папка", label, len as i64));
                let lim = self.limits.clone();
                let stream = tokio_util::io::ReaderStream::with_capacity(f, 64 * 1024).then(move |c| {
                    if let (Some(g), Ok(b)) = (&guard, &c) {
                        g.add(b.len());
                    }
                    let (lim, n) = (lim.clone(), c.as_ref().map(|b| b.len()).unwrap_or(0));
                    async move {
                        if let Some(l) = lim {
                            l.up.take(n).await;
                        }
                        c
                    }
                });
                rb = rb
                    .header("Content-Type", "application/octet-stream")
                    .header("Content-Length", len)
                    .body(reqwest::Body::wrap_stream(stream));
            }
        }
        rb.build().map_err(|e| PeerError::Io(e.to_string()))
    }

    /// Отправить запрос; старому соседу — повторить со старой подписью.
    async fn send(&self, method: Method, rel: &str, body: Body) -> Result<reqwest::Response, PeerError> {
        let req = self.build(&method, rel, &body).await?;
        let resp = self.http.execute(req).await.map_err(|e| classify(&e))?;
        if self.legacy() || resp.status() != StatusCode::UNAUTHORIZED {
            return Ok(resp);
        }
        let text = resp.text().await.unwrap_or_default();
        if !text.contains(&format!("\"{}\"", signing::LEGACY_BAD_SIGNATURE)) {
            return Err(PeerError::Http(401, error_text(&text, 401)));
        }
        self.legacy.store(true, Ordering::Relaxed);
        let req = self.build(&method, rel, &body).await?;
        self.http.execute(req).await.map_err(|e| classify(&e))
    }

    async fn ensure(resp: reqwest::Response) -> Result<reqwest::Response, PeerError> {
        if resp.status().is_success() {
            return Ok(resp);
        }
        let code = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        Err(PeerError::Http(code, error_text(&text, code)))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, rel: &str) -> Result<T, PeerError> {
        let resp = Self::ensure(self.send(Method::GET, rel, Body::Empty).await?).await?;
        let bytes = resp.bytes().await.map_err(|e| classify(&e))?;
        serde_json::from_slice(&bytes).map_err(|e| PeerError::Http(200, format!("непонятный ответ соседа: {}", e)))
    }

    /// POST JSON по пути относительно /bridge/v1 (например «/peer/agent/ask»): (код, тело).
    pub async fn post_json(&self, path: &str, v: &serde_json::Value) -> Result<(u16, String), PeerError> {
        self.post(&format!("{}{}", API_PREFIX, path), v).await
    }

    /// GET по пути относительно /bridge/v1: (код, байты).
    pub async fn get_bytes(&self, path: &str) -> Result<(u16, Vec<u8>), PeerError> {
        let resp = self.send(Method::GET, &format!("{}{}", API_PREFIX, path), Body::Empty).await?;
        let code = resp.status().as_u16();
        Ok((code, resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default()))
    }

    /// GET по пути относительно /bridge/v1: (код, тело).
    pub async fn get_text(&self, path: &str) -> Result<(u16, String), PeerError> {
        let resp = self.send(Method::GET, &format!("{}{}", API_PREFIX, path), Body::Empty).await?;
        let code = resp.status().as_u16();
        Ok((code, resp.text().await.unwrap_or_default()))
    }

    async fn post<T: serde::Serialize>(&self, rel: &str, v: &T) -> Result<(u16, String), PeerError> {
        let body = serde_json::to_vec(v).unwrap_or_default();
        let resp = self.send(Method::POST, rel, Body::Json(body)).await?;
        let code = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        Ok((code, text))
    }

    fn p(share: &str) -> String {
        format!("{}/peer/shares/{}", API_PREFIX, esc(share))
    }

    fn f(id: &str) -> String {
        format!("{}/peer/folders/{}", API_PREFIX, esc(id))
    }

    pub async fn hello(&self) -> Result<HelloResponse, PeerError> {
        let mut rel = format!("{}/peer/hello", API_PREFIX);
        if !self.announce.is_empty() {
            rel.push_str("?listen=");
            rel.push_str(&esc(&self.announce));
        }
        self.get(&rel).await
    }

    pub async fn manifest(&self, share: &str) -> Result<FileManifest, PeerError> {
        self.get(&format!("{}/manifest", Self::p(share))).await
    }

    pub async fn events(&self, share: &str, after: &std::collections::BTreeMap<String, i64>, limit: usize) -> Result<EventsPage, PeerError> {
        self.get(&format!("{}/events?after={}&limit={}", Self::p(share), esc(&vector_text(after)), limit)).await
    }

    /// Скачать файл во временный и проверить sha. Возвращает sha.
    async fn download_to(&self, rel: &str, dest: &Path, max_bytes: i64, what: &str, label: &str) -> Result<String, PeerError> {
        let resp = Self::ensure(self.send(Method::GET, rel, Body::Empty).await?).await?;
        let guard = self.progress.as_ref().map(|t| t.start("down", &self.peer_name, what, label, resp.content_length().unwrap_or(0) as i64));
        if let Some(len) = resp.content_length() {
            if len as i64 > max_bytes {
                return Err(PeerError::Http(413, "файл больше предела".into()));
            }
        }
        let expected = resp.headers().get("X-Sha256").and_then(|v| v.to_str().ok()).map(|s| s.to_lowercase());
        if let Some(d) = dest.parent() {
            tokio::fs::create_dir_all(d).await.map_err(|e| PeerError::Io(e.to_string()))?;
        }
        let mut file = tokio::fs::File::create(dest).await.map_err(|e| PeerError::Io(e.to_string()))?;
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        let mut total: i64 = 0;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| classify(&e))?;
            total += chunk.len() as i64;
            if let Some(g) = &guard {
                g.add(chunk.len());
            }
            if let Some(l) = &self.limits {
                l.down.take(chunk.len()).await;
            }
            if total > max_bytes {
                drop(file);
                let _ = tokio::fs::remove_file(dest).await;
                return Err(PeerError::Http(413, "файл больше предела".into()));
            }
            sha2::Digest::update(&mut hasher, &chunk);
            file.write_all(&chunk).await.map_err(|e| PeerError::Io(e.to_string()))?;
        }
        file.flush().await.map_err(|e| PeerError::Io(e.to_string()))?;
        drop(file);
        let sha = hex::encode(sha2::Digest::finalize(hasher));
        if let Some(exp) = expected {
            if exp != sha {
                let _ = tokio::fs::remove_file(dest).await;
                return Err(PeerError::Http(422, "файл изменился во время передачи".into()));
            }
        }
        Ok(sha)
    }

    pub async fn download(&self, share: &str, p: &str, dest: &Path, max: i64) -> Result<String, PeerError> {
        self.download_to(&format!("{}/file?path={}", Self::p(share), esc(p)), dest, max, "пространство", p).await
    }

    pub async fn mailbox_list(&self) -> Result<MailboxList, PeerError> {
        self.get(&format!("{}/peer/mailbox", API_PREFIX)).await
    }

    pub async fn mailbox_download(&self, p: &str, dest: &Path, max: i64) -> Result<String, PeerError> {
        self.download_to(&format!("{}/peer/mailbox/file?path={}", API_PREFIX, esc(p)), dest, max, "отправка", p).await
    }

    pub async fn mailbox_ack(&self, ack: &MailboxAck) -> Result<(), PeerError> {
        let (code, text) = self.post(&format!("{}/peer/mailbox/ack", API_PREFIX), ack).await?;
        if (200..300).contains(&code) {
            Ok(())
        } else {
            Err(PeerError::Http(code, error_text(&text, code)))
        }
    }

    pub async fn folders(&self) -> Result<Vec<SharedFolderInfo>, PeerError> {
        self.get(&format!("{}/peer/folders", API_PREFIX)).await
    }

    pub async fn folder_manifest(&self, id: &str) -> Result<FolderManifest, PeerError> {
        self.get(&format!("{}/manifest", Self::f(id))).await
    }

    pub async fn folder_download(&self, id: &str, p: &str, dest: &Path, max: i64) -> Result<String, PeerError> {
        self.download_to(&format!("{}/file?path={}", Self::f(id), esc(p)), dest, max, "папка", p).await
    }

    /// Записать файл в общую папку соседа. base — sha, от которой сделана правка ("" — файла не было). 409 — конфликт.
    pub async fn folder_upload(&self, id: &str, p: &str, file: &Path, sha: &str, base: &str) -> Result<u16, PeerError> {
        // size — чтобы хозяин сразу отказал файлу больше своего предела, не принимая его целиком
        let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
        let rel = format!("{}/file?path={}&sha={}&base={}&size={}", Self::f(id), esc(p), sha, base, size);
        let resp = self.send(Method::PUT, &rel, Body::File(file.to_path_buf(), sha.to_string(), p.to_string())).await?;
        let code = resp.status().as_u16();
        if code == 409 || code == 403 || (200..300).contains(&code) {
            return Ok(code);
        }
        Self::ensure(resp).await.map(|_| code)
    }

    pub async fn folder_delete(&self, id: &str, p: &str, base: &str) -> Result<u16, PeerError> {
        let (code, text) = self.post(&format!("{}/delete", Self::f(id)), &FolderDeleteRequest { path: p.into(), base: base.into() }).await?;
        if matches!(code, 409 | 403 | 404) || (200..300).contains(&code) {
            Ok(code)
        } else {
            Err(PeerError::Http(code, error_text(&text, code)))
        }
    }

    pub async fn notify(&self, share: &str, kind: &str) -> Result<(), PeerError> {
        let (code, text) = self.post(&format!("{}/peer/notify", API_PREFIX), &NotifyRequest { share: share.into(), kind: kind.into() }).await?;
        if (200..300).contains(&code) {
            Ok(())
        } else {
            Err(PeerError::Http(code, error_text(&text, code)))
        }
    }

    /// Сообщение для звонка/экрана: сигналинг WebRTC через соседа (2.0).
    pub async fn signal(&self, msg: &serde_json::Value) -> Result<(), PeerError> {
        let (code, text) = self.post(&format!("{}/peer/signal", API_PREFIX), msg).await?;
        if (200..300).contains(&code) {
            Ok(())
        } else {
            Err(PeerError::Http(code, error_text(&text, code)))
        }
    }
}

/// Текст ошибки из ответа соседа (JSON {error} или по коду).
pub fn error_text(text: &str, code: u16) -> String {
    if let Ok(e) = serde_json::from_str::<ErrorResponse>(text) {
        if !e.error.is_empty() {
            return e.error;
        }
    }
    match code {
        // 403 без пояснения отдают узлы до 1.4, когда адреса нет в их белом списке
        403 => "сосед не пускает адрес этого ПК (нет в его белом списке адресов)".into(),
        404 => "по этому адресу отвечает не мост или мост другой версии".into(),
        _ => format!("ответ {}", code),
    }
}

