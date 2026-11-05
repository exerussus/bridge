//! Служба приложения «Сервер» в Мосте (`most-relay app-service`, на ПК под Windows или Linux).
//! Мост запускает её с `MOST_PORT` и `MOST_DATA`; веб-интерфейс приложения ходит сюда через `most.service.fetch`.
//! Здесь хранятся серверы с их мастер-ключами (`MOST_DATA/servers.json`, наружу ключ не отдаётся) и держатся
//! шифрованные каналы до серверов.

use crate::client::{self, Admin};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use bridge_core::relay::RelayInvite;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Server {
    id: String,
    title: String,
    host: String,
    port: u16,
    master: String,
    added: String,
}

struct Svc {
    file: PathBuf,
    servers: Mutex<Vec<Server>>,
    conns: Mutex<HashMap<String, Arc<Admin>>>,
    errors: Mutex<HashMap<String, String>>,
    /// Одно подключение к серверу за раз.
    connecting: tokio::sync::Mutex<()>,
}

type St = State<Arc<Svc>>;

struct Fail(String);

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": self.0 }))).into_response()
    }
}

impl From<String> for Fail {
    fn from(s: String) -> Self {
        Fail(s)
    }
}

impl Svc {
    fn save(&self) -> Result<(), String> {
        let list = self.servers.lock().clone();
        let tmp = self.file.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&list).unwrap()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.file).map_err(|e| e.to_string())
    }

    fn get(&self, id: &str) -> Result<Server, String> {
        self.servers.lock().iter().find(|s| s.id == id).cloned().ok_or_else(|| "такого сервера в списке нет".to_string())
    }

    async fn conn(&self, id: &str) -> Result<Arc<Admin>, String> {
        if let Some(a) = self.conns.lock().get(id).filter(|a| a.alive()).cloned() {
            return Ok(a);
        }
        let _g = self.connecting.lock().await;
        if let Some(a) = self.conns.lock().get(id).filter(|a| a.alive()).cloned() {
            return Ok(a);
        }
        let s = self.get(id)?;
        match client::connect(&s.host, s.port, &s.master).await {
            Ok(a) => {
                self.conns.lock().insert(id.to_string(), a.clone());
                self.errors.lock().remove(id);
                Ok(a)
            }
            Err(e) => {
                self.errors.lock().insert(id.to_string(), e.clone());
                Err(e)
            }
        }
    }
}

async fn list(State(svc): St) -> Json<Value> {
    let list = svc.servers.lock().clone();
    let conns = svc.conns.lock();
    let errors = svc.errors.lock();
    Json(json!(list
        .iter()
        .map(|s| json!({ "id": s.id, "title": s.title, "host": s.host, "port": s.port, "added": s.added, "online": conns.get(&s.id).map(|a| a.alive()).unwrap_or(false), "error": errors.get(&s.id) }))
        .collect::<Vec<_>>()))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AddReq {
    line: String,
    title: String,
}

async fn add(State(svc): St, Json(r): Json<AddReq>) -> Result<Json<Value>, Fail> {
    let inv = RelayInvite::parse(&r.line)?;
    if !crate::proto::is_master(&inv.code) {
        return Err(Fail("это одноразовое приглашение для моста («MRI-…») — сюда нужен мастер-ключ («MRM-…»): most-relay master на сервере".into()));
    }
    let a = client::connect(&inv.host, inv.port, &inv.code).await?;
    let st = a.call("status", json!({})).await?;
    let title = if r.title.trim().is_empty() { st["name"].as_str().unwrap_or(&inv.host).to_string() } else { r.title.trim().to_string() };
    let id = {
        let mut g = svc.servers.lock();
        let id = match g.iter_mut().find(|s| s.host == inv.host && s.port == inv.port) {
            Some(s) => {
                s.master = inv.code.clone();
                s.title = title.clone();
                s.id.clone()
            }
            None => {
                let id = hex::encode(rand::random::<[u8; 6]>());
                g.push(Server { id: id.clone(), title, host: inv.host.clone(), port: inv.port, master: inv.code.clone(), added: crate::now_text() });
                id
            }
        };
        id
    };
    svc.save()?;
    svc.conns.lock().insert(id.clone(), a);
    Ok(Json(json!({ "id": id, "status": st })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct IdReq {
    id: String,
}

async fn remove(State(svc): St, Json(r): Json<IdReq>) -> Result<Json<Value>, Fail> {
    svc.servers.lock().retain(|s| s.id != r.id);
    svc.conns.lock().remove(&r.id);
    svc.save()?;
    Ok(Json(json!(true)))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CallReq {
    id: String,
    op: String,
    args: Value,
}

async fn call(State(svc): St, Json(r): Json<CallReq>) -> Result<Json<Value>, Fail> {
    if r.op.starts_with("term.") {
        return Err(Fail("терминал — через /term/…".into()));
    }
    let a = svc.conn(&r.id).await?;
    let args = if r.args.is_null() { json!({}) } else { r.args };
    let v = a.call(&r.op, args).await?;
    if r.op == "status" {
        // имя сервера могли сменить — в списке показываем актуальное
        if let Some(n) = v["name"].as_str() {
            let changed = {
                let mut g = svc.servers.lock();
                match g.iter_mut().find(|s| s.id == r.id && s.title != n) {
                    Some(s) => {
                        s.title = n.to_string();
                        true
                    }
                    None => false,
                }
            };
            if changed {
                svc.save()?;
            }
        }
    }
    if r.op == "master.rotate" {
        // новый ключ — сразу в список, старый канал сервер закроет сам
        if let Some(code) = v["line"].as_str().and_then(|l| RelayInvite::parse(l).ok()) {
            if let Some(s) = svc.servers.lock().iter_mut().find(|s| s.id == r.id) {
                s.master = code.code;
            }
            svc.save()?;
            svc.conns.lock().remove(&r.id);
        }
    }
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TermReq {
    id: String,
    tid: String,
    data: String,
    cols: u16,
    rows: u16,
}

async fn term_open(State(svc): St, Json(r): Json<TermReq>) -> Result<Json<Value>, Fail> {
    let a = svc.conn(&r.id).await?;
    let v = a.call("term.open", json!({ "cols": r.cols.max(20), "rows": r.rows.max(5) })).await?;
    if let Some(t) = v["tid"].as_str() {
        a.term(t);
    }
    Ok(Json(v))
}

async fn term_op(svc: &Svc, op: &str, r: TermReq) -> Result<Json<Value>, Fail> {
    let a = svc.conn(&r.id).await?;
    let mut args = json!({ "tid": r.tid });
    match op {
        "term.write" => args["data"] = json!(r.data),
        "term.resize" => {
            args["cols"] = json!(r.cols);
            args["rows"] = json!(r.rows);
        }
        _ => {}
    }
    Ok(Json(a.call(op, args).await?))
}

async fn term_write(State(svc): St, Json(r): Json<TermReq>) -> Result<Json<Value>, Fail> {
    term_op(&svc, "term.write", r).await
}
async fn term_resize(State(svc): St, Json(r): Json<TermReq>) -> Result<Json<Value>, Fail> {
    term_op(&svc, "term.resize", r).await
}
async fn term_close(State(svc): St, Json(r): Json<TermReq>) -> Result<Json<Value>, Fail> {
    term_op(&svc, "term.close", r).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReadQ {
    id: String,
    tid: String,
    after: u64,
}

/// Долгий опрос: ждёт вывод до 20 с.
async fn term_read(State(svc): St, Query(q): Query<ReadQ>) -> Result<Json<Value>, Fail> {
    let a = svc.conns.lock().get(&q.id).cloned().ok_or_else(|| Fail("нет соединения с сервером".into()))?;
    let t = a.terms.lock().get(&q.tid).cloned().ok_or_else(|| Fail("терминал закрыт".into()))?;
    let (chunks, next, exited) = t.read(q.after, Duration::from_secs(20)).await;
    let b64 = base64::engine::general_purpose::STANDARD;
    Ok(Json(json!({ "chunks": chunks.iter().map(|c| b64.encode(c)).collect::<Vec<_>>(), "next": next, "exited": exited })))
}

async fn ping() -> &'static str {
    "ok"
}

pub async fn run() -> Result<(), String> {
    let port: u16 = std::env::var("MOST_PORT").ok().and_then(|p| p.parse().ok()).ok_or("нет MOST_PORT — службу запускает Мост (приложение «Сервер»)")?;
    let data = std::env::var("MOST_DATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("data"));
    std::fs::create_dir_all(&data).map_err(|e| format!("{}: {}", data.display(), e))?;
    let file = data.join("servers.json");
    let servers: Vec<Server> = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let svc = Arc::new(Svc { file, servers: Mutex::new(servers), conns: Mutex::new(HashMap::new()), errors: Mutex::new(HashMap::new()), connecting: tokio::sync::Mutex::new(()) });
    let app = Router::new()
        .route("/", get(ping))
        .route("/servers", get(list).post(add))
        .route("/servers/remove", post(remove))
        .route("/call", post(call))
        .route("/term/open", post(term_open))
        .route("/term/write", post(term_write))
        .route("/term/resize", post(term_resize))
        .route("/term/close", post(term_close))
        .route("/term/read", get(term_read))
        .with_state(svc);
    let l = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| format!("127.0.0.1:{}: {}", port, e))?;
    crate::log(&format!("служба приложения «Сервер» слушает 127.0.0.1:{}", port));
    axum::serve(l, app).await.map_err(|e| e.to_string())
}
