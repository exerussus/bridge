//! `bridge mcp` — инструменты моста для Claude (Model Context Protocol, JSON-RPC по stdin/stdout).
//! Подключение в проекте Claude Code (.mcp.json):  { "mcpServers": { "bridge": { "command": "C:\\Bridge\\bridge.exe", "args": ["mcp"] } } }
//! Узел должен быть запущен на этом ПК (трей «Мост» или bridge run): инструменты ходят в его локальный API.

use super::{enc, Api, Opts};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

const INSTRUCTIONS: &str = "Мост связывает этот ПК с соседними ПК. Через него можно поручать задачи Claude на другом ПК или на этом же \
(agent_ask без peer — мост запустит отдельный Claude здесь; agent_ask → agent_wait), получать от него ход работы и итог, продолжать в той же нити (agent_ask с thread — он помнит контекст), \
останавливать (agent_stop). Файлы — через обменную папку (exchange_folder): что кладёте туда, видно у соседа, и наоборот. \
Если поручения приходят на этот ПК (ручной режим): agent_wait_task / agent_inbox → agent_take → работа → agent_progress → agent_report.";

fn tools() -> Value {
    let s = |props: Value, req: &[&str]| json!({ "type": "object", "properties": props, "required": req });
    json!([
        { "name": "bridge_peers", "description": "Соседи моста: кто на связи и можно ли поручать их агенту (agent: auto — Claude запускается сам, manual — ждёт, пока возьмёт сессия Claude там).",
          "inputSchema": s(json!({}), &[]) },
        { "name": "agent_ask", "description": "Поручить задачу Claude на соседнем ПК или на этом (peer не указан или «local» — мост запустит отдельный Claude на этом ПК в своей сессии) и дождаться итога (до wait_seconds). Несколько поручений идут параллельно (сколько разрешено в «Агентах»). Без thread — новая нить; с thread — продолжение той же сессии (исполнитель помнит контекст). Пиши поручение полностью: что сделать, где, какой итог и какие файлы положить в обменную папку.",
          "inputSchema": s(json!({ "peer": { "type": "string", "description": "имя соседа; пусто или «local» — этот ПК" }, "prompt": { "type": "string" }, "thread": { "type": "string", "description": "id нити для продолжения" },
                                   "title": { "type": "string" }, "wait_seconds": { "type": "integer", "description": "сколько ждать итога, по умолчанию 300; 0 — не ждать" },
                                   "record": { "type": "object", "description": "запись-задача {share, id}: исполнитель сам поведёт её статус (в работе → на проверке)" } }), &["prompt"]) },
        { "name": "agent_wait", "description": "Ждать итога по нити (до wait_seconds). Возвращает новый ход работы с прошлого вызова и итог, если он есть.",
          "inputSchema": s(json!({ "thread": { "type": "string" }, "wait_seconds": { "type": "integer" } }), &["thread"]) },
        { "name": "agent_status", "description": "Нить целиком: статус, весь ход, итог, стоимость.", "inputSchema": s(json!({ "thread": { "type": "string" } }), &["thread"]) },
        { "name": "agent_stop", "description": "Остановить работу по нити (свою — попросит соседа; принятую — прервёт здесь).", "inputSchema": s(json!({ "thread": { "type": "string" } }), &["thread"]) },
        { "name": "agent_request", "description": "Спросить людей и дождаться ответа (исполнитель поручения): вопрос, кнопки-варианты и/или поле для свободного ответа. Вопрос видят заказчик и те, кто у этого ПК; первый ответ возвращается сюда. Используйте, когда ждёте действия человека (закончить сборку, проверить, выбрать вариант) — вместо ожидания вслепую.",
          "inputSchema": s(json!({ "question": { "type": "string" }, "buttons": { "type": "array", "items": { "type": "string" }, "description": "варианты ответа, до 8 (например «Готово», «Ещё 10 минут»)" }, "input": { "type": "boolean", "description": "поле для свободного ответа (без кнопок — всегда есть)" },
                                   "thread": { "type": "string", "description": "id поручения; без него — текущее" }, "wait_seconds": { "type": "integer", "description": "сколько ждать ответа, по умолчанию 1800" } }), &["question"]) },
        { "name": "agent_threads", "description": "Все нити: поручения соседям (out), соседей этому ПК (in) и себе (in, local).", "inputSchema": s(json!({}), &[]) },
        { "name": "exchange_folder", "description": "Путь обменной папки на этом ПК (общая с соседом-агентом). Без thread — своя обменная папка; с thread — где лежит обмен для этого поручения.",
          "inputSchema": s(json!({ "thread": { "type": "string" } }), &[]) },
        { "name": "send_file", "description": "Отправить файл соседу (ручная отправка: появится у него в «приём/<этот ПК>»).",
          "inputSchema": s(json!({ "peer": { "type": "string" }, "path": { "type": "string", "description": "путь к файлу на этом ПК" }, "dest": { "type": "string", "description": "имя у соседа (необязательно)" } }), &["peer", "path"]) },
        { "name": "bridge_ui", "description": "Пульт панели моста (голос и экран живут в окне панели; если оно закрыто — мост откроет). action: state, join {channel, quiet?}, leave, mic {on}, deafen {on}, share {quality?: text|motion|low} (человек выбирает, что показать), unshare {id?}, watch {peer|stream, on?}, popout {peer|stream}, call {peers:[…]}, answer {room?}, decline {room?}, volume {peer, value 0..1}, ask_screen {peer}, tab {tab}; доска — board_png, board_view, board_pdf (удобнее bridge_board); показ — screen_shot, screen_point, screen_draw, screen_clear (удобнее bridge_screen).",
          "inputSchema": s(json!({ "action": { "type": "string" }, "args": { "type": "object", "description": "параметры действия" } }), &["action"]) },
        { "name": "bridge_board", "description": "Доска канала: смотреть, искать, класть, двигать, править, слои, фон, картинка доски. Координаты — единицы доски (y вниз). room — название или id канала, либо «владелец:канал» (список — action rooms). action и args: rooms; describe {room} — обзор (слои, границы, сколько чего); find {room, q?, type?: text,note,shape,link,rect,arrow,path,image, layer?, by?, x0,y0,x1,y1?, limit?}; get {room, ids}; add {room, objs: [{type: text|note|shape|link|frame|rect|arrow|line|path|image|pdf|diagram, x?, y?, …}], layer?, at?: [x,y], layout?: row|column|grid, gap?} — без x, y объекты встают рядом с тем, что есть. Диаграммы: shape {kind: rect|round|ellipse|diamond|triangle|hexagon|parallelogram|trapezoid|terminal|document|database|predefined|manual|delay|offpage|cloud|callout|star|arrowR|cross|frame|lane, w, h, text, fill, color (контур), textColor, dash: solid|dash|dot, valign}; link {from, to — id объекта (можно из этого же add) или [x,y], fromSide/toSide: top|right|bottom|left|auto, kind: straight|elbow|curve, start/end: none|arrow|triangle|diamond|circle|one|many, label, labelAt 0..1, dash} — связь едет за фигурами; diagram {mermaid: «flowchart TD …»} — фигуры и связи из текста Mermaid, панель раскладывает их сама (группы subgraph — рамки). Поля: text, size, color (#rrggbb или white/black/cyan/red/orange/yellow/green/purple/blue/gray/pink), font (default|simple|tech|hand), bold, italic, underline, align (left|center|right); rect: w, h, fill, width; arrow: x,y,x2,y2 или from/to; line: x,y,x2,y2; path: points [[x,y],…]; image: path (файл на этом ПК) или data (base64), w/h/maxSize; pdf: path, pages ('1-3,5'), w — страницы рисует панель. edit {room, id|ids, …те же поля, layer?, toFront?}; move {room, ids, dx, dy} или {room, ids, x, y}; remove {room, ids}; clear {room, layer?}; layer {room, action: add|rename|remove|up|down|top|bottom, layer?, name?, to?}; bg {room, color?, light? 20..200}; enable {room, on}; png {room, ids? | area? [x0,y0,x1,y1], scale?} — картинка доски в файл (панель в этом канале); view {room, ids? | area? | x,y,zoom?} — показать место человеку в панели.",
          "inputSchema": s(json!({ "action": { "type": "string", "enum": ["rooms", "describe", "find", "get", "add", "edit", "move", "remove", "clear", "layer", "bg", "enable", "png", "view"] }, "args": { "type": "object", "description": "параметры действия (room и т. д.)" } }), &["action"]) },
        { "name": "bridge_screen", "description": "Показ экрана в канале, где сейчас панель: shot {peer|stream|mine?, maxSize?, marks?} — снимок кадра в PNG (вернёт путь к файлу; можно прочитать как картинку), point {peer, x, y} — пинг точки (x, y — доли экрана 0…1), draw {peer, points: [[x,y],…]} — линия поверх (через несколько секунд гаснет), clear {peer} — стереть свои пометки. Список показов — bridge_ui state. Управлять чужим ПК ассистент не может.",
          "inputSchema": s(json!({ "action": { "type": "string", "enum": ["shot", "point", "draw", "clear"] }, "args": { "type": "object" } }), &["action"]) },
        { "name": "bridge_api", "description": "Любой вызов локального API моста (полный перечень: GET api). path — без /bridge/v1/local/, например «channels», «folders», «records?type=task».",
          "inputSchema": s(json!({ "method": { "type": "string", "enum": ["GET", "POST"] }, "path": { "type": "string" }, "body": { "type": "object" } }), &["method", "path"]) },
        { "name": "agent_wait_task", "description": "Исполнитель (ручной режим): ждать поручение от соседа до wait_seconds. Вернёт список ждущих поручений.",
          "inputSchema": s(json!({ "wait_seconds": { "type": "integer" } }), &[]) },
        { "name": "agent_inbox", "description": "Исполнитель: поручения этому ПК, которые ждут, пока их возьмут.", "inputSchema": s(json!({}), &[]) },
        { "name": "agent_take", "description": "Исполнитель: взять поручение в работу. Вернёт текст поручения, контекст и пути (рабочая и обменная папки).",
          "inputSchema": s(json!({ "thread": { "type": "string" } }), &["thread"]) },
        { "name": "agent_progress", "description": "Исполнитель: сообщить оркестратору ход работы. text — что сделано или делается; phase — этап, твой статус для оркестратора (пиши на смену этапа, не на каждое действие: «ищу информацию», «сверяю данные»); waiting=true — явно ждёшь (сборка, ответ), false — снова работаешь.",
          "inputSchema": s(json!({ "thread": { "type": "string" }, "text": { "type": "string" }, "phase": { "type": "string" }, "waiting": { "type": "boolean" } }), &["thread"]) },
        { "name": "agent_report", "description": "Исполнитель: итог поручения (что сделано, результат, где файлы). ok=false — не получилось.",
          "inputSchema": s(json!({ "thread": { "type": "string" }, "result": { "type": "string" }, "ok": { "type": "boolean" } }), &["thread", "result"]) },
    ])
}

/// Что уже показали Claude по каждой нити — чтобы agent_wait отдавал только новое.
type Seen = Arc<Mutex<HashMap<String, i64>>>;

pub async fn serve(o: &Opts) -> Result<i32, String> {
    let api = Arc::new(Api::open(o).map_err(|e| format!("{} — запустите мост на этом ПК", e))?);
    let out = Arc::new(Mutex::new(tokio::io::stdout()));
    let seen: Seen = Arc::new(Mutex::new(HashMap::new()));
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

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn wait_secs(v: &Value) -> u64 {
    v.get("wait_seconds").and_then(|x| x.as_u64()).unwrap_or(300).min(3600)
}

async fn get_json(api: &Api, rel: &str) -> Result<Value, String> {
    serde_json::from_str(&api.get(rel).await?).map_err(|e| e.to_string())
}

async fn post_json(api: &Api, rel: &str, body: Option<Value>) -> Result<Value, String> {
    serde_json::from_str(&api.post(rel, body).await?).map_err(|e| e.to_string())
}

const STATUS: &[(&str, &str)] = &[("queued", "ждёт, пока возьмут"), ("running", "в работе"), ("waiting", "в работе, ждёт (ожидание)"), ("idle", "ход завершён (можно продолжить в этой нити)"), ("failed", "не получилось"), ("stopped", "остановлено")];

fn status_text(st: &str) -> &str {
    STATUS.iter().find(|(k, _)| *k == st).map(|(_, v)| *v).unwrap_or(st)
}

/// Нить — текстом для Claude: новое с прошлого раза, итог, что делать дальше.
async fn render(api: &Api, seen: &Seen, t: &Value, full: bool) -> String {
    let id = s(t, "id").to_string();
    let after = if full { -1 } else { seen.lock().await.get(&id).copied().unwrap_or(-1) };
    let events = t.get("events").and_then(|e| e.as_array()).cloned().unwrap_or_default();
    let mut lines: Vec<String> = Vec::new();
    let mut last = after;
    for e in &events {
        let n = e.get("n").and_then(|x| x.as_i64()).unwrap_or(0);
        last = last.max(n);
        if n <= after {
            continue;
        }
        let text = s(e, "text");
        let line = match s(e, "kind") {
            "prompt" => format!("[поручение] {}", text),
            "phase" => format!("[этап] {}", text),
            "text" => format!("[Claude] {}", text),
            "tool" => format!("  · {}", text),
            "result" => continue, // итог — ниже отдельно
            "error" => format!("[ошибка] {}", text),
            _ => format!("[{}] {}", s(e, "kind"), text),
        };
        lines.push(line);
    }
    seen.lock().await.insert(id.clone(), last);
    // длинный ход — последние строки
    let mut body = lines.join("\n");
    if body.chars().count() > 8000 {
        let tail: String = body.chars().rev().take(8000).collect::<Vec<_>>().into_iter().rev().collect();
        body = format!("… (начало хода пропущено)\n{}", tail);
    }
    let st = s(t, "status");
    let local = t.get("local").and_then(|x| x.as_bool()).unwrap_or(false);
    let mut out = format!("Нить {} · {} · {}: {}", id, if local { "поручение этому ПК" } else if s(t, "role") == "out" { "поручение соседу" } else { "поручение от соседа" }, s(t, "peer"), status_text(st));
    if let Some(c) = t.get("cost").and_then(|c| c.as_f64()).filter(|c| *c > 0.0) {
        out.push_str(&format!(" · ${:.2}", c));
    }
    if (st == "running" || st == "waiting") && !s(t, "phase").is_empty() {
        out.push_str(&format!(" · сейчас: {}", s(t, "phase")));
    }
    if !body.is_empty() {
        out.push_str("\n\nХод работы:\n");
        out.push_str(&body);
    }
    if st == "idle" && !s(t, "result").is_empty() {
        out.push_str("\n\nИтог:\n");
        out.push_str(s(t, "result"));
    }
    if st == "failed" && !s(t, "error").is_empty() {
        out.push_str("\n\nОшибка: ");
        out.push_str(s(t, "error"));
    }
    let _ = api;
    if let Some(p) = t.get("exchangePath").and_then(|p| p.as_str()) {
        out.push_str(&format!("\n\nОбменная папка на этом ПК: {}", p));
    }
    match st {
        "running" | "queued" => out.push_str(&format!("\n\nЕщё не готово. Ждать дальше: agent_wait(thread=\"{}\").", id)),
        "idle" => out.push_str(&format!("\n\nПродолжить в той же сессии: agent_ask(peer=\"{}\", thread=\"{}\", prompt=…).", if local { "local" } else { s(t, "peer") }, id)),
        _ => {}
    }
    out
}

async fn call(api: &Api, seen: &Seen, name: &str, a: &Value) -> Result<String, String> {
    match name {
        "bridge_peers" => {
            let v = get_json(api, "agent").await?;
            let peers = v.get("peers").and_then(|p| p.as_array()).cloned().unwrap_or_default();
            let lm = v["local"]["mode"].as_str().unwrap_or("off");
            let head = format!("Этот ПК ({}) — agent_ask без peer: {}", v["local"]["name"].as_str().unwrap_or(""), match lm {
                "auto" => "Claude запустится сам",
                "manual" => "поручение ждёт сессию Claude на этом ПК (agent_inbox → agent_take)",
                _ => "агент выключен (панель → «Агенты» → режим)",
            });
            if peers.is_empty() {
                return Ok(format!("{}\nСоседей нет: подключите в панели моста («Соседи»).", head));
            }
            Ok(head + "\n" + &peers
                .iter()
                .map(|p| {
                    let ag = match s(p, "agent") {
                        "auto" => "агент: сам запускает Claude",
                        "manual" => "агент: ждёт, пока возьмёт сессия Claude там",
                        _ => "агент: не принимает поручения от этого ПК",
                    };
                    format!("{} — {} — {}", s(p, "name"), if p.get("online").and_then(|x| x.as_bool()).unwrap_or(false) { "на связи" } else { "нет связи" }, ag)
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "agent_ask" => {
            let mut body = json!({ "peer": s(a, "peer"), "prompt": s(a, "prompt") });
            for k in ["thread", "title"] {
                if !s(a, k).is_empty() {
                    body[k] = json!(s(a, k));
                }
            }
            if a.get("record").map(|r| r.is_object()).unwrap_or(false) {
                body["record"] = a["record"].clone();
            }
            let t = post_json(api, "agent/ask", Some(body)).await?;
            let id = s(&t, "id").to_string();
            // поручение уже показали — с него не начинать
            seen.lock().await.insert(id.clone(), t.get("lastN").and_then(|x| x.as_i64()).unwrap_or(-1));
            let w = wait_secs(a);
            let after = t.get("lastN").and_then(|x| x.as_i64()).unwrap_or(-1);
            let t = if w > 0 { get_json(api, &format!("agent/wait?id={}&after={}&timeout={}", enc(&id), after, w)).await? } else { t };
            Ok(render(api, seen, &t, false).await)
        }
        "agent_wait" => {
            let id = s(a, "thread");
            let after = seen.lock().await.get(id).copied().unwrap_or(-1);
            let t = get_json(api, &format!("agent/wait?id={}&after={}&timeout={}", enc(id), after, wait_secs(a))).await?;
            Ok(render(api, seen, &t, false).await)
        }
        "agent_status" => {
            let t = get_json(api, &format!("agent/thread?id={}", enc(s(a, "thread")))).await?;
            Ok(render(api, seen, &t, true).await)
        }
        "agent_stop" => {
            post_json(api, &format!("agent/stop?id={}", enc(s(a, "thread"))), None).await?;
            Ok("Остановка отправлена.".into())
        }
        "agent_threads" => {
            let v = get_json(api, "agent").await?;
            let th = v.get("threads").and_then(|p| p.as_array()).cloned().unwrap_or_default();
            if th.is_empty() {
                return Ok("Нитей пока нет.".into());
            }
            Ok(th.iter().take(50).map(|t| format!("{} · {} {} · {} · {}", s(t, "id"), if s(t, "role") == "out" { "→" } else { "←" }, s(t, "peer"), status_text(s(t, "status")), s(t, "title"))).collect::<Vec<_>>().join("\n"))
        }
        "exchange_folder" => {
            if !s(a, "thread").is_empty() {
                let t = get_json(api, &format!("agent/thread?id={}", enc(s(a, "thread")))).await?;
                return match t.get("exchangePath").and_then(|p| p.as_str()) {
                    Some(p) => Ok(p.to_string()),
                    None => Ok("У этой нити нет обменной папки.".into()),
                };
            }
            let v = get_json(api, "agent").await?;
            match v.pointer("/exchange/path").and_then(|p| p.as_str()) {
                Some(p) => Ok(p.to_string()),
                None => Err("Обменная папка не настроена: панель моста → «Агенты» → «Обменная папка».".into()),
            }
        }
        "send_file" => {
            let path = s(a, "path");
            let data = tokio::fs::read(path).await.map_err(|e| format!("{}: {}", path, e))?;
            let name = if s(a, "dest").is_empty() { std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { s(a, "dest").to_string() };
            let (code, text) = api.call("POST", &format!("mailbox/upload?peer={}&path={}", enc(s(a, "peer")), enc(&name)), Some(data), false).await?;
            if (200..300).contains(&code) {
                Ok(format!("Поставлено в отправку: {} → {} (придёт, когда сосед заберёт; ход — в панели моста)", name, s(a, "peer")))
            } else {
                Err(super::err_text(&text, code))
            }
        }
        "bridge_ui" => {
            let mut body = a.get("args").cloned().filter(|v| v.is_object()).unwrap_or(json!({}));
            body["action"] = json!(s(a, "action"));
            let v = post_json(api, "ui", Some(body)).await?;
            Ok(serde_json::to_string_pretty(v.get("data").unwrap_or(&Value::Null)).unwrap_or_default())
        }
        "bridge_board" => {
            let mut b = a.get("args").cloned().filter(|v| v.is_object()).unwrap_or(json!({}));
            let act = s(a, "action").to_string();
            let v = match act.as_str() {
                "rooms" => get_json(api, "board/rooms").await?,
                "describe" | "find" | "get" => {
                    let q: Vec<String> = b
                        .as_object()
                        .map(|m| m.iter().map(|(k, v)| format!("{}={}", k, bridge_node::client::esc(&match v { Value::String(t) => t.clone(), Value::Array(a) => a.iter().map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(","), o => o.to_string() }))).collect())
                        .unwrap_or_default();
                    get_json(api, &format!("board/{}?{}", act, q.join("&"))).await?
                }
                "add" | "edit" | "move" | "remove" | "clear" | "layer" | "bg" | "enable" => post_json(api, &format!("board/{}", act), Some(b)).await?,
                "png" | "view" => {
                    b["action"] = json!(format!("board_{}", act));
                    post_json(api, "ui", Some(b)).await?.get("data").cloned().unwrap_or(Value::Null)
                }
                _ => return Err(format!("нет действия «{}»", act)),
            };
            Ok(serde_json::to_string_pretty(&v).unwrap_or_default())
        }
        "bridge_screen" => {
            let mut b = a.get("args").cloned().filter(|v| v.is_object()).unwrap_or(json!({}));
            let act = s(a, "action");
            if !["shot", "point", "draw", "clear"].contains(&act) {
                return Err(format!("нет действия «{}»: shot, point, draw, clear", act));
            }
            b["action"] = json!(format!("screen_{}", act));
            let v = post_json(api, "ui", Some(b)).await?;
            Ok(serde_json::to_string_pretty(v.get("data").unwrap_or(&Value::Null)).unwrap_or_default())
        }
        "bridge_api" => {
            let path = s(a, "path").trim_start_matches('/').to_string();
            let text = if s(a, "method").eq_ignore_ascii_case("POST") { api.post(&path, a.get("body").cloned()).await? } else { api.get(&path).await? };
            Ok(text)
        }
        "agent_wait_task" | "agent_inbox" => {
            let v = if name == "agent_inbox" { get_json(api, "agent/inbox").await? } else { get_json(api, &format!("agent/wait-task?timeout={}", wait_secs(a))).await? };
            let list = v.as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                return Ok("Поручений нет.".into());
            }
            Ok(list
                .iter()
                .map(|t| {
                    let p = t.get("events").and_then(|e| e.as_array()).and_then(|e| e.iter().rev().find(|x| s(x, "kind") == "prompt")).map(|x| s(x, "text").to_string()).unwrap_or_default();
                    format!("{} от {}: {}\n{}", s(t, "id"), s(t, "peer"), s(t, "title"), p)
                })
                .collect::<Vec<_>>()
                .join("\n\n")
                + "\n\nВзять: agent_take(thread=…).")
        }
        "agent_take" => {
            let v = post_json(api, &format!("agent/take?id={}", enc(s(a, "thread"))), None).await?;
            let mut out = format!("Поручение от «{}»:\n{}\n\n{}", s(&v, "from"), s(&v, "prompt"), s(&v, "context"));
            out.push_str(&format!("\n\nПо ходу — agent_progress(thread=\"{0}\", text=…, phase=«этап» на смену этапа, waiting=true когда ждёшь); в конце — agent_report(thread=\"{0}\", result=…, ok=true|false).", s(a, "thread")));
            Ok(out)
        }
        "agent_request" => {
            // нить: явно, из окружения (мост запустил этого Claude) или единственная взятая в работу
            let mut thread = s(a, "thread").to_string();
            if thread.is_empty() {
                thread = std::env::var("BRIDGE_AGENT_THREAD").unwrap_or_default();
            }
            if thread.is_empty() {
                let v = get_json(api, "agent").await?;
                let busy: Vec<String> = v["threads"].as_array().cloned().unwrap_or_default().iter().filter(|t| t["role"] == "in" && matches!(t["status"].as_str(), Some("running") | Some("waiting"))).filter_map(|t| t["id"].as_str().map(String::from)).collect();
                if busy.len() != 1 {
                    return Err("укажите thread — id поручения, по которому спрашиваете".into());
                }
                thread = busy[0].clone();
            }
            let mut body = json!({ "thread": thread, "question": s(a, "question"), "input": a.get("input").and_then(|x| x.as_bool()).unwrap_or(false) });
            if let Some(b) = a.get("buttons").filter(|b| b.is_array()) {
                body["buttons"] = b.clone();
            }
            let r = post_json(api, "agent/request", Some(body)).await?;
            let id = s(&r, "id").to_string();
            let secs = a.get("wait_seconds").and_then(|x| x.as_u64()).unwrap_or(1800).min(3 * 3600);
            let mut left = secs;
            while left > 0 {
                let step = left.min(3600);
                let w = get_json(api, &format!("agent/request/wait?thread={}&id={}&timeout={}", enc(&thread), enc(&id), step)).await?;
                if w["answered"].as_bool().unwrap_or(false) {
                    let rq = &w["req"];
                    return Ok(format!("Ответ ({}): {}", s(rq, "by"), s(rq, "answer")));
                }
                if w["thread"].as_str().map(|st| !matches!(st, "running" | "waiting" | "queued")).unwrap_or(true) {
                    return Ok("Ответа нет: поручение остановлено или закончилось.".into());
                }
                left -= step;
            }
            Ok(format!("За {} мин ответа нет. Вопрос остаётся у людей; можно спросить снова (agent_request) или продолжить по своему решению и написать об этом в отчёте.", secs / 60))
        }
        "agent_progress" => {
            let mut b = json!({ "thread": s(a, "thread"), "text": s(a, "text"), "phase": s(a, "phase") });
            if let Some(w) = a.get("waiting").and_then(|x| x.as_bool()) {
                b["waiting"] = json!(w);
            }
            post_json(api, "agent/progress", Some(b)).await?;
            Ok("Отправлено.".into())
        }
        "agent_report" => {
            let ok = a.get("ok").and_then(|x| x.as_bool()).unwrap_or(true);
            post_json(api, "agent/report", Some(json!({ "thread": s(a, "thread"), "result": s(a, "result"), "ok": ok }))).await?;
            Ok("Итог отправлен оркестратору.".into())
        }
        _ => Err(format!("нет инструмента {}", name)),
    }
}
