//! Передача приложений соседям (3.6). Хозяин предлагает приложение соседям; у соседа появляется предложение,
//! он сам решает — принять или нет. Принял — его мост скачивает пакет .most-app у нас по подписанной связи,
//! сверяет sha256 и id в app.json, кладёт в «приложения». Запустится оно только после его разрешения, как любое новое.
//! «Взять у хоста»: участник канала, где сейчас идёт сеанс приложения, может попросить пакет сам.

use crate::node::Node;
use bridge_core::time;
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Пакет приложения — до 4 ГБ (сборки Unity бывают большими).
pub const PKG_MAX: i64 = 4 << 30;
/// Предложение живёт сутки.
const KEEP: Duration = Duration::from_secs(24 * 3600);
const INCOMING_MAX: usize = 30;

#[derive(Default)]
pub struct AppShare {
    /// Наши предложения: offer → что и кому отдаём.
    out: Mutex<BTreeMap<String, Out>>,
    /// Предложения нам (новые первыми в конце списка).
    inc: Mutex<Vec<Value>>,
    /// sha256 готовых пакетов: путь → (размер, время изменения, sha).
    sums: Mutex<BTreeMap<PathBuf, (u64, std::time::SystemTime, String)>>,
    /// Одна упаковка за раз.
    pack_lock: tokio::sync::Mutex<()>,
}

struct Out {
    peer: String,
    path: PathBuf,
    sha: String,
    at: Instant,
}

fn sha_file(p: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(p).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Пакет приложения: готовый .most-app как есть, папку — упаковать (кэш по отпечатку кода и интерфейса).
async fn package(node: &Arc<Node>, a: &crate::apps::App) -> Result<(PathBuf, u64, String), String> {
    let _g = node.share.pack_lock.lock().await;
    let path = if a.packaged {
        a.source.clone()
    } else {
        let dir = crate::apps::data_root(node).join("out");
        let _ = std::fs::create_dir_all(&dir);
        let name = format!("{}-{}-{}.most-app", a.id, a.version, &format!("{}{}", a.hash, a.ui_stamp).chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect::<String>());
        let p = dir.join(&name);
        if !p.is_file() {
            // старые упаковки этого приложения больше не нужны (это кэш моста)
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if n.starts_with(&format!("{}-", a.id)) && n != name {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            let (root, tmp) = (a.root.clone(), dir.join(format!("{}.part", name)));
            let t2 = tmp.clone();
            tokio::task::spawn_blocking(move || crate::apps::pack(&root, Some(t2))).await.map_err(|e| e.to_string())??;
            std::fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
        }
        p
    };
    let meta = std::fs::metadata(&path).map_err(|e| format!("пакет: {}", e))?;
    let (len, mt) = (meta.len(), meta.modified().map_err(|e| e.to_string())?);
    if len as i64 > PKG_MAX {
        return Err(format!("пакет {} МБ — больше предела 4 ГБ", len >> 20));
    }
    if let Some((l, m, s)) = node.share.sums.lock().get(&path).cloned() {
        if l == len && m == mt {
            return Ok((path, len, s));
        }
    }
    let p2 = path.clone();
    let sha = tokio::task::spawn_blocking(move || sha_file(&p2)).await.map_err(|e| e.to_string())??;
    node.share.sums.lock().insert(path.clone(), (len, mt, sha.clone()));
    Ok((path, len, sha))
}

fn describe(node: &Node, a: &crate::apps::App, offer: &str, size: u64, sha: &str) -> Value {
    let m = &a.manifest;
    json!({
        "offer": offer, "id": a.id, "name": a.name, "version": a.version, "description": a.description, "author": a.author,
        "size": size, "sha256": sha, "perms": a.perms, "from": node.node_name(), "publisher": a.publisher,
        "kinds": { "ui": !a.ui_entry.is_empty(), "native": m["native"].is_object(), "service": m["service"].is_object(), "actions": m["actions"].is_array(), "net": m["net"].is_object() },
    })
}

fn sharable(a: &crate::apps::App) -> Result<(), String> {
    if a.manifest["share"] == false {
        return Err(format!("«{}» запрещает передачу (share: false в app.json)", a.name));
    }
    Ok(())
}

fn new_offer(node: &Node, peer: &str, path: PathBuf, sha: &str) -> String {
    let offer = hex::encode(rand::random::<[u8; 12]>());
    let mut g = node.share.out.lock();
    g.retain(|_, o| o.at.elapsed() < KEEP);
    g.insert(offer.clone(), Out { peer: peer.to_string(), path, sha: sha.to_string(), at: Instant::now() });
    offer
}

/// «Отправить соседям»: предложить приложение `id` выбранным соседям.
pub async fn share(node: &Arc<Node>, id: &str, peers: &[String]) -> Result<Value, String> {
    if let Some(g) = node.remote.active() {
        return Err(format!("пока этим ПК управляет {}, приложения не передаются", g.peer));
    }
    let a = crate::apps::get(node, id).ok_or("нет такого приложения")?;
    sharable(&a)?;
    if peers.is_empty() {
        return Err("выберите соседей".into());
    }
    let (path, size, sha) = package(node, &a).await?;
    let list: Vec<_> = node.cfg.read().peers.iter().filter(|p| p.enabled && p.has_url() && peers.iter().any(|o| bridge_core::eq_ic(o, &p.name))).cloned().collect();
    let mut results = serde_json::Map::new();
    for p in list {
        let res = {
            let offer = new_offer(node, &p.name, path.clone(), &sha);
            let body = describe(node, &a, &offer, size, &sha);
            let c = node.client_for(&p);
            match tokio::time::timeout(Duration::from_secs(10), c.post_json("/peer/apps/offer", &body)).await {
                Ok(Ok((code, _))) if (200..300).contains(&code) => "предложено — ждём, когда примет".to_string(),
                Ok(Ok((404, _))) => "у соседа мост старше 3.6 — пусть обновится («Поделиться версией»)".to_string(),
                Ok(Ok((code, t))) => crate::client::error_text(&t, code),
                Ok(Err(e)) => crate::link::explain(&e, &p).0,
                Err(_) => "сосед не ответил".into(),
            }
        };
        results.insert(p.name.clone(), json!(res));
    }
    node.info(&format!("[приложения] «{}» {} ({} МБ) предложено: {}", a.name, a.version, size >> 20, results.iter().map(|(k, v)| format!("{} — {}", k, v.as_str().unwrap_or(""))).collect::<Vec<_>>().join("; ")));
    Ok(json!({ "results": results, "size": size }))
}

fn check_offer(v: &Value) -> Result<(), String> {
    let s = |k: &str| v[k].as_str().unwrap_or("");
    if !crate::apps::valid_id(s("id")) {
        return Err("неверный id приложения".into());
    }
    let o = s("offer");
    if o.len() < 16 || o.len() > 64 || !o.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("неверное предложение".into());
    }
    if s("sha256").len() != 64 || !s("sha256").chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("нет sha256 пакета".into());
    }
    let size = v["size"].as_i64().unwrap_or(0);
    if size <= 0 || size > PKG_MAX {
        return Err("размер пакета вне пределов".into());
    }
    Ok(())
}

fn clean(v: &Value, from: &str) -> Value {
    let t = |k: &str, n: usize| v[k].as_str().unwrap_or("").chars().take(n).collect::<String>();
    let perms: Vec<String> = v["perms"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter(|p| crate::apps::PERMS.contains(p)).map(String::from).collect()).unwrap_or_default();
    let k = &v["kinds"];
    json!({
        "offer": t("offer", 64), "id": t("id", 40), "name": if t("name", 80).is_empty() { t("id", 40) } else { t("name", 80) }, "version": t("version", 40),
        "description": t("description", 400), "author": t("author", 80), "size": v["size"], "sha256": t("sha256", 64).to_lowercase(), "perms": perms,
        "publisher": t("publisher", 64), "publisherFp": crate::identity::fingerprint_b64(&t("publisher", 64)),
        "kinds": { "ui": k["ui"] == true, "native": k["native"] == true, "service": k["service"] == true, "actions": k["actions"] == true, "net": k["net"] == true },
        "from": from, "at": time::format(&time::now()), "state": "new", "error": "",
    })
}

/// Сосед предлагает нам приложение.
pub fn on_offer(node: &Node, from: &str, v: &Value) -> Result<(), (u16, String)> {
    if node.remote.active().is_some() {
        return Err((423, "этим ПК сейчас управляют по сети — предложения не принимаются".into()));
    }
    check_offer(v).map_err(|e| (400, e))?;
    let e = clean(v, from);
    {
        let mut g = node.share.inc.lock();
        g.retain(|x| !(bridge_core::eq_ic(x["from"].as_str().unwrap_or(""), from) && x["id"] == e["id"] && x["state"] != "download"));
        g.push(e.clone());
        while g.len() > INCOMING_MAX {
            g.remove(0);
        }
    }
    node.info(&format!("[приложения] «{}» предлагает приложение «{}» {} ({} МБ)", from, e["name"].as_str().unwrap_or(""), e["version"].as_str().unwrap_or(""), e["size"].as_i64().unwrap_or(0) >> 20));
    node.media.deliver(json!({ "media": "apps", "type": "offer", "from": from, "name": e["name"], "version": e["version"] }));
    Ok(())
}

/// Отдать пакет соседу, которому мы его предложили.
pub fn served(node: &Node, offer: &str, peer: &str) -> Option<(PathBuf, String)> {
    let g = node.share.out.lock();
    let o = g.get(offer)?;
    (bridge_core::eq_ic(&o.peer, peer) && o.at.elapsed() < KEEP && o.path.is_file()).then(|| (o.path.clone(), o.sha.clone()))
}

/// Сосед просит приложение сам («Взять у хоста»): только то, что сейчас идёт в сеансе в нашем общем канале.
pub async fn on_request(node: &Arc<Node>, from: &str, id: &str) -> Result<Value, (u16, String)> {
    if node.remote.active().is_some() {
        return Err((423, "этим ПК сейчас управляют по сети".into()));
    }
    let a = crate::apps::get(node, id).ok_or((404, "у хоста нет такого приложения".to_string()))?;
    sharable(&a).map_err(|e| (403, e))?;
    if !crate::net::shared_in_channel(node, id, from) {
        return Err((403, format!("«{}» отдаётся само только тем, кто в одном канале с идущим сеансом — попросите хозяина отправить его («Отправить соседям»)", a.name)));
    }
    let (path, size, sha) = package(node, &a).await.map_err(|e| (500, e))?;
    let offer = new_offer(node, from, path, &sha);
    node.info(&format!("[приложения] «{}» берёт «{}» {} (идёт в общем канале)", from, a.name, a.version));
    Ok(describe(node, &a, &offer, size, &sha))
}

/// Предложения нам — с тем, что уже стоит здесь.
pub fn incoming(node: &Node) -> Vec<Value> {
    node.share.inc.lock().iter().rev().map(|e| {
        let mut e = e.clone();
        if let Some(a) = crate::apps::get(node, e["id"].as_str().unwrap_or("")) {
            e["have"] = json!({ "version": a.version, "folder": !a.packaged });
        }
        e
    }).collect()
}

fn set_state(node: &Node, offer: &str, st: &str, error: &str) {
    if let Some(e) = node.share.inc.lock().iter_mut().find(|e| e["offer"] == offer) {
        e["state"] = json!(st);
        e["error"] = json!(error);
    }
    node.media.deliver(json!({ "media": "apps", "type": "offer", "state": st }));
}

pub fn decline(node: &Node, offer: &str) {
    node.share.inc.lock().retain(|e| !(e["offer"] == offer && e["state"] != "download"));
    node.media.deliver(json!({ "media": "apps", "type": "offer", "state": "declined" }));
}

/// id в app.json пакета и проверенный издатель ("" — без подписи; неверная подпись — ошибка).
fn package_info(p: &Path) -> Result<(String, String), String> {
    let (tree, sig, man) = crate::apps::tree_hash_zip(p)?;
    if man.is_null() {
        return Err("в пакете нет app.json".into());
    }
    let id = man["id"].as_str().unwrap_or("").to_string();
    let publisher = if sig.is_null() && !man["publisher"].is_string() { String::new() } else { crate::apps::check_sig(&sig, &tree, &man).map_err(|e| format!("подпись издателя: {}", e))? };
    Ok((id, publisher))
}

/// Принять предложение: скачать, проверить, положить в «приложения». Ответ — сразу; ход — событиями панели.
pub fn accept(node: &Arc<Node>, offer: &str) -> Result<(), String> {
    let e = {
        let mut g = node.share.inc.lock();
        let e = g.iter_mut().find(|e| e["offer"] == offer).ok_or("предложение устарело")?;
        if e["state"] == "download" {
            return Err("уже скачивается".into());
        }
        e["state"] = json!("download");
        e["error"] = json!("");
        e.clone()
    };
    if let Some(a) = crate::apps::get(node, e["id"].as_str().unwrap_or("")) {
        if !a.packaged {
            let msg = format!("«{}» у вас лежит папкой (копия для разработки) — обновите её сами или уберите", a.name);
            set_state(node, offer, "error", &msg);
            return Err(msg);
        }
    }
    let (n, offer) = (node.clone(), offer.to_string());
    tokio::spawn(async move {
        let r = fetch(&n, &e).await;
        match r {
            Ok(()) => {
                set_state(&n, &offer, "done", "");
                n.info(&format!("[приложения] «{}» {} от «{}» — в папке «приложения»; откройте его и разрешите", e["name"].as_str().unwrap_or(""), e["version"].as_str().unwrap_or(""), e["from"].as_str().unwrap_or("")));
            }
            Err(err) => {
                n.info(&format!("[приложения] «{}» от «{}» не принято: {}", e["name"].as_str().unwrap_or(""), e["from"].as_str().unwrap_or(""), err));
                set_state(&n, &offer, "error", &err);
            }
        }
    });
    Ok(())
}

async fn fetch(node: &Arc<Node>, e: &Value) -> Result<(), String> {
    let from = e["from"].as_str().unwrap_or("");
    let (id, version, offer) = (e["id"].as_str().unwrap_or(""), e["version"].as_str().unwrap_or(""), e["offer"].as_str().unwrap_or(""));
    let p = node.cfg.read().peer(from).cloned().ok_or("соседа больше нет")?;
    let dir = crate::apps::data_root(node).join("in");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{}.part", offer));
    let c = node.client_for(&p);
    let size = e["size"].as_i64().unwrap_or(0);
    let r = c.app_package(offer, &tmp, size.max(1), e["name"].as_str().unwrap_or(id)).await;
    let sha = match r {
        Ok(s) => s,
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(crate::link::explain(&err, &p).0);
        }
    };
    if sha != e["sha256"].as_str().unwrap_or("") {
        let _ = std::fs::remove_file(&tmp);
        return Err("пакет пришёл повреждённым (sha256 не совпал)".into());
    }
    let t2 = tmp.clone();
    let got = tokio::task::spawn_blocking(move || package_info(&t2)).await.map_err(|e| e.to_string())?;
    let (got, publisher) = match got {
        Ok(g) => g,
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
    };
    if got != id {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("в пакете другое приложение («{}» вместо «{}»)", got, id));
    }
    // издатель закреплён: приложение, подписанное издателем, обновляется только пакетом того же издателя
    if let Some(a) = crate::apps::get(node, id) {
        if !a.publisher.is_empty() && a.publisher != publisher {
            let _ = std::fs::remove_file(&tmp);
            return Err(if publisher.is_empty() {
                format!("у вас «{}» подписано издателем {}, а пришёл пакет без подписи — не принят", a.name, crate::identity::fingerprint_b64(&a.publisher))
            } else {
                format!("пакет подписан другим издателем ({} вместо {}) — не принят", crate::identity::fingerprint_b64(&publisher), crate::identity::fingerprint_b64(&a.publisher))
            });
        }
    }
    if node.remote.active().is_some() {
        let _ = std::fs::remove_file(&tmp);
        return Err("этим ПК начали управлять по сети — приложение не установлено".into());
    }
    crate::apps::retire(node, id)?;
    let safe_ver: String = version.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-').take(30).collect();
    crate::apps::import(node, &format!("{}-{}.most-app", id, if safe_ver.is_empty() { "0" } else { &safe_ver }), &tmp)?;
    Ok(())
}

/// «Взять у хоста»: попросить пакет у соседа и сразу скачать (человек сам нажал кнопку).
pub async fn take(node: &Arc<Node>, peer: &str, id: &str) -> Result<String, String> {
    if let Some(g) = node.remote.active() {
        return Err(format!("пока этим ПК управляет {}, приложения не принимаются", g.peer));
    }
    let p = node.cfg.read().peer(peer).cloned().ok_or("нет такого соседа")?;
    let c = node.client_for(&p);
    let r = tokio::time::timeout(Duration::from_secs(120), c.post_json("/peer/apps/request", &json!({ "id": id }))).await;
    let v: Value = match r {
        Ok(Ok((code, t))) if (200..300).contains(&code) => serde_json::from_str(&t).map_err(|_| "непонятный ответ соседа".to_string())?,
        Ok(Ok((404, t))) if !t.contains("error") => return Err("у соседа мост старше 3.6 — пусть обновится".into()),
        Ok(Ok((code, t))) => return Err(crate::client::error_text(&t, code)),
        Ok(Err(e)) => return Err(crate::link::explain(&e, &p).0),
        Err(_) => return Err("сосед не ответил (большое приложение упаковывается — попробуйте ещё раз через минуту)".into()),
    };
    check_offer(&v)?;
    let e = clean(&v, &p.name);
    let offer = e["offer"].as_str().unwrap_or("").to_string();
    {
        let mut g = node.share.inc.lock();
        g.retain(|x| !(x["id"] == e["id"] && bridge_core::eq_ic(x["from"].as_str().unwrap_or(""), &p.name) && x["state"] != "download"));
        g.push(e);
    }
    accept(node, &offer)?;
    Ok(offer)
}
