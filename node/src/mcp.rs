//! `bridge mcp` — инструменты моста для Claude (Model Context Protocol, JSON-RPC по stdin/stdout).
//! Подключение в проекте Claude Code (.mcp.json):  { "mcpServers": { "bridge": { "command": "C:\\Bridge\\bridge.exe", "args": ["mcp"] } } }
//! Узел должен быть запущен на этом ПК (трей «Мост» или bridge run): инструменты ходят в его локальный API.

use super::{Api, Opts};
use bridge_node::tools::{call, tools, LocalApi, Seen, INSTRUCTIONS};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

pub async fn serve(o: &Opts) -> Result<i32, String> {
    let a = Api::open(o).map_err(|e| format!("{} — запустите мост на этом ПК", e))?;
    let api = Arc::new(LocalApi::new(a.base.trim_end_matches('/').trim_end_matches("/local").trim_end_matches(bridge_core::API_PREFIX), &a.token)?);
    let out = Arc::new(Mutex::new(tokio::io::stdout()));
    let seen: Seen = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else { continue };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let (api, out, seen) = (api.clone(), out.clone(), seen.clone());
        tokio::spawn(async move {
            let reply = match method.as_str() {
                "initialize" => Some(Ok(json!({
                    "protocolVersion": params.get("protocolVersion").cloned().unwrap_or(json!("2025-06-18")),
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "bridge", "version": bridge_core::APP_VERSION },
                    "instructions": INSTRUCTIONS,
                }))),
                "ping" => Some(Ok(json!({}))),
                "tools/list" => Some(Ok(json!({ "tools": tools() }))),
                "tools/call" => {
                    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    let args = params.get("arguments").cloned().unwrap_or(json!({}));
                    let r = call(&api, &seen, &name, &args).await;
                    Some(Ok(match r {
                        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
                    }))
                }
                m if m.starts_with("notifications/") => None,
                _ => Some(Err(json!({ "code": -32601, "message": format!("нет метода {}", method) }))),
            };
            if let (Some(r), Some(id)) = (reply, id) {
                let msg = match r {
                    Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
                };
                let mut o = out.lock().await;
                let _ = o.write_all(format!("{}\n", msg).as_bytes()).await;
                let _ = o.flush().await;
            }
        });
    }
    Ok(0)
}

