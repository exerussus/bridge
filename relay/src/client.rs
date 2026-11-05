//! Клиент канала администратора: держит шифрованное соединение с сервером, шлёт команды, копит вывод терминалов.

use crate::proto::{ClientStart, ServerHello, ADMIN_PATH};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio_tungstenite::tungstenite::Message;

/// Сколько кусков вывода держать на терминал (старые отбрасываются).
const KEEP_CHUNKS: usize = 4000;

#[derive(Default)]
pub struct TermBuf {
    chunks: Mutex<Vec<(u64, Vec<u8>)>>,
    seq: AtomicU64,
    pub exited: AtomicBool,
    notify: Notify,
}

impl TermBuf {
    fn push(&self, d: Vec<u8>) {
        let n = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let mut g = self.chunks.lock();
        g.push((n, d));
        if g.len() > KEEP_CHUNKS {
            let cut = g.len() - KEEP_CHUNKS;
            g.drain(..cut);
        }
        drop(g);
        self.notify.notify_waiters();
    }

    /// Куски после номера `after`: (куски, последний номер, закрыт ли).
    pub async fn read(&self, after: u64, wait: Duration) -> (Vec<Vec<u8>>, u64, bool) {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = self.notify.notified();
            {
                let g = self.chunks.lock();
                let out: Vec<Vec<u8>> = g.iter().filter(|(n, _)| *n > after).map(|(_, d)| d.clone()).collect();
                let last = g.last().map(|x| x.0).unwrap_or(after).max(after);
                let ex = self.exited.load(Ordering::SeqCst);
                if !out.is_empty() || ex {
                    return (out, last, ex);
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return (Vec::new(), after, self.exited.load(Ordering::SeqCst));
            }
        }
    }
}

pub struct Admin {
    tx: mpsc::Sender<Value>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    next: AtomicU64,
    pub closed: AtomicBool,
    pub why: Mutex<String>,
    pub terms: Mutex<HashMap<String, Arc<TermBuf>>>,
}

impl Admin {
    pub fn alive(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }

    pub async fn call(&self, op: &str, args: Value) -> Result<Value, String> {
        if !self.alive() {
            return Err(self.why.lock().clone());
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        if op == "term.open" {
            // буфер создаём заранее: вывод может прийти раньше ответа
        }
        self.tx.send(json!({ "id": id, "op": op, "args": args })).await.map_err(|_| "соединение с сервером закрыто".to_string())?;
        match tokio::time::timeout(Duration::from_secs(20), rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(self.why.lock().clone()),
            Err(_) => {
                self.pending.lock().remove(&id);
                Err("сервер не ответил за 20 с".into())
            }
        }
    }

    pub fn term(&self, tid: &str) -> Arc<TermBuf> {
        self.terms.lock().entry(tid.to_string()).or_default().clone()
    }

    fn close(&self, why: &str) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.why.lock() = why.to_string();
        for (_, tx) in self.pending.lock().drain() {
            let _ = tx.send(Err(why.to_string()));
        }
        for t in self.terms.lock().values() {
            t.exited.store(true, Ordering::SeqCst);
            t.notify.notify_waiters();
        }
    }
}

/// Подключиться к серверу по мастер-ключу.
pub async fn connect(host: &str, port: u16, master: &str) -> Result<Arc<Admin>, String> {
    let h = if host.contains(':') { format!("[{}]", host) } else { host.to_string() };
    let url = format!("ws://{}:{}{}", h, port, ADMIN_PATH);
    let (mut ws, _) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(url))
        .await
        .map_err(|_| format!("сервер {}:{} не отвечает", host, port))?
        .map_err(|e| format!("не удаётся подключиться к {}:{}: {}", host, port, e))?;
    let start = ClientStart::new(master);
    ws.send(Message::Text(serde_json::to_string(&start.hello).unwrap())).await.map_err(|e| e.to_string())?;
    let first = tokio::time::timeout(Duration::from_secs(10), ws.next()).await.map_err(|_| "сервер не ответил на рукопожатие".to_string())?;
    let sh: ServerHello = match first {
        Some(Ok(Message::Text(t))) => serde_json::from_str(&t).map_err(|_| "ответ не похож на most-relay".to_string())?,
        _ => return Err("сервер закрыл соединение — это точно most-relay 3.16+?".into()),
    };
    let ch = start.finish(master, &sh)?;
    let (mut sealer, mut opener) = ch.split();
    let (tx, mut rx) = mpsc::channel::<Value>(1024);
    let admin = Arc::new(Admin { tx, pending: Mutex::new(HashMap::new()), next: AtomicU64::new(1), closed: AtomicBool::new(false), why: Mutex::new(String::new()), terms: Mutex::new(HashMap::new()) });
    let (mut sink, mut stream) = ws.split();
    let a1 = admin.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                m = rx.recv() => {
                    let Some(v) = m else { break };
                    if sink.send(Message::Binary(sealer.seal(v.to_string().as_bytes()))).await.is_err() {
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(25)) => {
                    if !a1.alive() || sink.send(Message::Ping(Vec::new())).await.is_err() {
                        break;
                    }
                }
            }
        }
        a1.close("соединение с сервером закрыто");
    });
    let a2 = admin.clone();
    tokio::spawn(async move {
        while let Some(m) = stream.next().await {
            let data = match m {
                Ok(Message::Binary(b)) => b,
                Ok(Message::Close(_)) | Err(_) => break,
                _ => continue,
            };
            let plain = match opener.open(&data) {
                Ok(p) => p,
                Err(e) => {
                    a2.close(&e);
                    return;
                }
            };
            let Ok(v) = serde_json::from_slice::<Value>(&plain) else { continue };
            if let Some(re) = v["re"].as_u64() {
                if let Some(tx) = a2.pending.lock().remove(&re) {
                    let r = if v["ok"] == true { Ok(v["data"].clone()) } else { Err(v["error"].as_str().unwrap_or("ошибка").to_string()) };
                    let _ = tx.send(r);
                }
            } else if v["ev"] == "term.data" {
                use base64::Engine;
                if let Ok(d) = base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap_or("")) {
                    a2.term(v["tid"].as_str().unwrap_or("")).push(d);
                }
            } else if v["ev"] == "term.exit" {
                let t = a2.term(v["tid"].as_str().unwrap_or(""));
                t.exited.store(true, Ordering::SeqCst);
                t.notify.notify_waiters();
            }
        }
        a2.close("сервер закрыл соединение");
    });
    Ok(admin)
}
