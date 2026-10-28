//! Ключ пользователя (3.7). Постоянная пара Ed25519 — основной секрет хранится в bridge-data/identity
//! (в Windows — под защитой DPAPI, на остальных системах — файлом с правами только для владельца).
//! Приложениям основной ключ не виден: каждому издателю — свой выведенный ключ (HKDF от основного секрета и
//! ключа издателя), так разные издатели не могут связать одного человека. Секрет никогда не отдаётся —
//! приложения только просят подписать строку вида:
//!   most-sign/v1\napp=<id>\npub=<ключ издателя>\npurpose=<назначение>\nts=<время UTC>\n<payload>
//! app, pub и ts подставляет мост. Подпись «login» от проверенного издателя — без вопроса; остальное —
//! с подтверждением хозяина ПК в панели (запоминается до перезапуска моста).

use crate::node::Node;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::oneshot;

/// Назначения, которые подписывает только сам мост.
pub const RESERVED_PURPOSES: &[&str] = &["activity", "presence", "receipt", "revoke", "app", "most"];
/// Подпись без вопроса — только для этого назначения и только у приложения с проверенным издателем.
pub const SILENT_PURPOSE: &str = "login";
const PAYLOAD_MAX: usize = 16 * 1024;
const SALT: &[u8] = b"most-identity/v1";

#[derive(Default)]
pub struct Identity {
    seed: Mutex<Option<[u8; 32]>>,
    /// Ждут ответа хозяина: id → канал ответа.
    asks: Mutex<BTreeMap<String, (Value, oneshot::Sender<bool>)>>,
    /// Разрешено до перезапуска: (приложение, назначение).
    allowed: Mutex<BTreeSet<(String, String)>>,
}

pub fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}
pub fn unb64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s.trim().trim_end_matches('=')).ok()
}

/// Отпечаток для людей: первые 8 байт SHA-256 открытого ключа, группами по 4 hex.
pub fn fingerprint(pubkey: &[u8]) -> String {
    let h = hex::encode(&Sha256::digest(pubkey)[..8]);
    format!("{}-{}-{}-{}", &h[0..4], &h[4..8], &h[8..12], &h[12..16])
}
pub fn fingerprint_b64(pubkey: &str) -> String {
    unb64(pubkey).filter(|k| k.len() == 32).map(|k| fingerprint(&k)).unwrap_or_default()
}

/// Ключ, выведенный из основного секрета для издателя (`publisher` — его ключ base64url; "" — без подписи).
pub fn derive(seed: &[u8; 32], publisher: &str) -> SigningKey {
    let hk = hkdf::Hkdf::<Sha256>::new(Some(SALT), seed);
    let mut okm = [0u8; 32];
    let info = format!("publisher:{}", if publisher.is_empty() { "-" } else { publisher });
    hk.expand(info.as_bytes(), &mut okm).expect("hkdf 32");
    SigningKey::from_bytes(&okm)
}

/// Проверить подпись Ed25519 (base64url).
pub fn verify(pubkey: &str, msg: &[u8], sig: &str) -> bool {
    let (Some(k), Some(s)) = (unb64(pubkey), unb64(sig)) else { return false };
    let (Ok(k), Ok(s)) = (<[u8; 32]>::try_from(k.as_slice()), <[u8; 64]>::try_from(s.as_slice())) else { return false };
    let Ok(vk) = VerifyingKey::from_bytes(&k) else { return false };
    vk.verify(msg, &ed25519_dalek::Signature::from_bytes(&s)).is_ok()
}

/// Время подписи: UTC с точностью до секунды, вид 2026-10-28T11:41:57Z.
pub fn ts_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Строка, которую подписывает мост.
pub fn signed_line(app: &str, publisher: &str, purpose: &str, ts: &str, payload: &str) -> String {
    format!("most-sign/v1\napp={}\npub={}\npurpose={}\nts={}\n{}", app, publisher, purpose, ts, payload)
}

// ───────────── хранение секрета ─────────────

fn dir(node: &Node) -> PathBuf {
    node.data_dir.join("identity")
}
fn key_file(node: &Node) -> PathBuf {
    dir(node).join("key")
}

#[cfg(any(windows, feature = "check-win"))]
#[cfg_attr(not(windows), allow(dead_code))]
mod protect {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};
    const UI_FORBIDDEN: u32 = 0x1;
    pub fn seal(b: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            let inp = CRYPT_INTEGER_BLOB { cbData: b.len() as u32, pbData: b.as_ptr() as *mut u8 };
            let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
            if CryptProtectData(&inp, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), UI_FORBIDDEN, &mut out) == 0 {
                return None;
            }
            let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData as _);
            Some(v)
        }
    }
    pub fn open(b: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            let inp = CRYPT_INTEGER_BLOB { cbData: b.len() as u32, pbData: b.as_ptr() as *mut u8 };
            let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
            if CryptUnprotectData(&inp, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), UI_FORBIDDEN, &mut out) == 0 {
                return None;
            }
            let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData as _);
            Some(v)
        }
    }
}

/// Запечатать секрет для хранения на этом ПК: "dpapi1:<hex>" (Windows) или "raw1:<hex>".
fn seal(seed: &[u8; 32]) -> String {
    #[cfg(windows)]
    if let Some(b) = protect::seal(seed) {
        return format!("dpapi1:{}", hex::encode(b));
    }
    format!("raw1:{}", hex::encode(seed))
}

fn unseal(text: &str) -> Result<[u8; 32], String> {
    let t = text.trim();
    let bytes = if let Some(h) = t.strip_prefix("raw1:") {
        hex::decode(h).map_err(|_| "файл ключа повреждён".to_string())?
    } else if let Some(h) = t.strip_prefix("dpapi1:") {
        let b = hex::decode(h).map_err(|_| "файл ключа повреждён".to_string())?;
        #[cfg(windows)]
        {
            protect::open(&b).ok_or("ключ защищён другой учётной записью Windows — восстановите его из резервной копии (Настройки → Ключ → Импорт)")?
        }
        #[cfg(not(windows))]
        {
            let _ = b;
            return Err("ключ защищён Windows (DPAPI) — на этой системе его не прочитать; восстановите из резервной копии".into());
        }
    } else {
        return Err("неизвестный формат файла ключа".into());
    };
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| "файл ключа повреждён".to_string())
}

fn write_private(p: &std::path::Path, text: &str) -> Result<(), String> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    bridge_core::write_atomic(p, text).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Основной секрет (создаётся при первом обращении).
fn seed(node: &Node) -> Result<[u8; 32], String> {
    let mut g = node.identity.seed.lock();
    if let Some(s) = *g {
        return Ok(s);
    }
    let f = key_file(node);
    let s = match std::fs::read_to_string(&f) {
        Ok(t) => unseal(&t)?,
        Err(_) => {
            let s: [u8; 32] = rand::random();
            write_private(&f, &seal(&s))?;
            node.info(&format!("[ключ] создан ключ пользователя {}", fingerprint(SigningKey::from_bytes(&s).verifying_key().as_bytes())));
            s
        }
    };
    *g = Some(s);
    Ok(s)
}

/// Прежние секреты (после смены или импорта): чтобы подписать «старый ключ → новый».
fn old_seeds(node: &Node) -> Vec<(String, [u8; 32])> {
    let mut v: Vec<(String, [u8; 32])> = std::fs::read_dir(dir(node).join("old"))
        .map(|r| r.flatten().filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), unseal(&std::fs::read_to_string(e.path()).ok()?).ok()?))).collect())
        .unwrap_or_default();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn retire_current(node: &Node) -> Result<(), String> {
    if let Ok(t) = std::fs::read_to_string(key_file(node)) {
        let name = format!("{}.key", chrono::Local::now().format("%Y%m%d-%H%M%S"));
        write_private(&dir(node).join("old").join(name), &t)?;
    }
    Ok(())
}

fn replace_seed(node: &Node, s: [u8; 32], why: &str) -> Result<(), String> {
    let _ = seed(node); // прежний создан/прочитан — его и уберём в old
    retire_current(node)?;
    write_private(&key_file(node), &seal(&s))?;
    *node.identity.seed.lock() = Some(s);
    node.identity.allowed.lock().clear();
    node.info(&format!("[ключ] {} — новый ключ {}", why, fingerprint(SigningKey::from_bytes(&s).verifying_key().as_bytes())));
    Ok(())
}

// ───────────── для панели ─────────────

pub fn status(node: &Node) -> Value {
    match seed(node) {
        Ok(s) => {
            let k = SigningKey::from_bytes(&s).verifying_key();
            json!({ "ok": true, "key": b64(k.as_bytes()), "fingerprint": fingerprint(k.as_bytes()), "protected": cfg!(windows), "previous": old_seeds(node).len() })
        }
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

/// Сменить ключ: прежний остаётся, чтобы подписать переход «старый → новый».
pub fn rotate(node: &Node) -> Result<Value, String> {
    replace_seed(node, rand::random(), "ключ сменён")?;
    Ok(status(node))
}

/// Резервная копия: секрет под паролем (Argon2id → XChaCha20-Poly1305).
pub fn export(node: &Node, password: &str) -> Result<Value, String> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    if password.chars().count() < 8 {
        return Err("пароль — не короче 8 знаков".into());
    }
    let s = seed(node)?;
    let salt: [u8; 16] = rand::random();
    let nonce: [u8; 24] = rand::random();
    let key = kdf(password, &salt)?;
    let ct = chacha20poly1305::XChaCha20Poly1305::new(&key.into()).encrypt(&nonce.into(), s.as_slice()).map_err(|_| "не удалось зашифровать".to_string())?;
    let k = SigningKey::from_bytes(&s).verifying_key();
    Ok(json!({ "v": 1, "kind": "most-key", "kdf": "argon2id", "m": 65536, "t": 3, "p": 1, "salt": b64(&salt), "nonce": b64(&nonce), "ct": b64(&ct), "fingerprint": fingerprint(k.as_bytes()) }))
}

fn kdf(password: &str, salt: &[u8]) -> Result<[u8; 32], String> {
    let params = argon2::Params::new(65536, 3, 1, Some(32)).map_err(|e| e.to_string())?;
    let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = [0u8; 32];
    a.hash_password_into(password.as_bytes(), salt, &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Восстановить ключ из резервной копии (прежний остаётся в old).
pub fn import(node: &Node, file: &Value, password: &str) -> Result<Value, String> {
    use chacha20poly1305::aead::{Aead, KeyInit};
    if file["kind"] != "most-key" || file["v"] != 1 {
        return Err("это не резервная копия ключа моста".into());
    }
    let g = |k: &str| unb64(file[k].as_str().unwrap_or("")).ok_or(format!("резервная копия повреждена ({})", k));
    let (salt, nonce, ct) = (g("salt")?, g("nonce")?, g("ct")?);
    let nonce = <[u8; 24]>::try_from(nonce.as_slice()).map_err(|_| "резервная копия повреждена".to_string())?;
    let key = kdf(password, &salt)?;
    let pt = chacha20poly1305::XChaCha20Poly1305::new(&key.into()).decrypt(&nonce.into(), ct.as_slice()).map_err(|_| "неверный пароль или повреждённый файл".to_string())?;
    let s = <[u8; 32]>::try_from(pt.as_slice()).map_err(|_| "резервная копия повреждена".to_string())?;
    if seed(node).ok() == Some(s) {
        return Ok(json!({ "same": true, "status": status(node) }));
    }
    replace_seed(node, s, "ключ восстановлен из резервной копии")?;
    Ok(json!({ "same": false, "status": status(node) }))
}

// ───────────── для приложений ─────────────

/// Ключ этого пользователя для издателя приложения `app` (base64url) — если у приложения есть право `identity`.
pub fn app_key(node: &Node, app: &str) -> String {
    let Some(a) = crate::apps::get(node, app) else { return String::new() };
    if !crate::apps::granted(node, &a).iter().any(|p| p == "identity") {
        return String::new();
    }
    seed(node).map(|s| b64(derive(&s, &a.publisher).verifying_key().as_bytes())).unwrap_or_default()
}

/// Identity.Get: {key, fingerprint, name, publisher, verified, rotated: [{signed, sig}]}.
pub fn get(node: &Node, app: &str) -> Result<Value, String> {
    let a = crate::apps::get(node, app).ok_or("нет такого приложения")?;
    if !crate::apps::granted(node, &a).iter().any(|p| p == "identity") {
        return Err("у приложения нет права «Ваш ключ для этого издателя»".into());
    }
    let s = seed(node)?;
    let k = derive(&s, &a.publisher);
    let key = b64(k.verifying_key().as_bytes());
    // переходы со старых ключей на этот: каждый подписан прежним ключом для этого же издателя
    let mut rotated = Vec::new();
    let olds = old_seeds(node);
    for (i, (_, o)) in olds.iter().enumerate() {
        let ok = derive(o, &a.publisher);
        let next = olds.get(i + 1).map(|(_, n)| b64(derive(n, &a.publisher).verifying_key().as_bytes())).unwrap_or_else(|| key.clone());
        let payload = json!({ "v": 1, "old": b64(ok.verifying_key().as_bytes()), "new": next }).to_string();
        let line = signed_line(&a.id, &a.publisher, "revoke", &ts_now(), &payload);
        rotated.push(json!({ "signed": line, "sig": b64(&ok.sign(line.as_bytes()).to_bytes()), "key": b64(ok.verifying_key().as_bytes()) }));
    }
    Ok(json!({ "key": key, "fingerprint": fingerprint(k.verifying_key().as_bytes()), "name": node.node_name(), "publisher": a.publisher, "verified": !a.publisher.is_empty(), "rotated": rotated }))
}

fn purpose_ok(p: &str) -> Result<(), String> {
    let ok = !p.is_empty() && p.len() <= 40 && p.chars().next().map(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).unwrap_or(false) && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-' || c == '_');
    if !ok {
        return Err("назначение подписи — латиница в нижнем регистре, цифры, . - _ (до 40)".into());
    }
    if RESERVED_PURPOSES.contains(&p) {
        return Err(format!("назначение «{}» подписывает только сам мост", p));
    }
    Ok(())
}

/// Identity.Sign: подписать строку от имени пользователя ключом для издателя приложения.
pub async fn sign(node: &std::sync::Arc<Node>, app: &str, purpose: &str, payload: &str) -> Result<Value, String> {
    if let Some(g) = node.remote.active() {
        return Err(format!("пока этим ПК управляет {}, подписи не выдаются", g.peer));
    }
    let a = crate::apps::get(node, app).ok_or("нет такого приложения")?;
    if !crate::apps::granted(node, &a).iter().any(|p| p == "identity") {
        return Err("у приложения нет права «Ваш ключ для этого издателя»".into());
    }
    purpose_ok(purpose)?;
    if payload.len() > PAYLOAD_MAX {
        return Err("подписываемые данные — до 16 КБ".into());
    }
    let silent = purpose == SILENT_PURPOSE && !a.publisher.is_empty();
    let remembered = node.identity.allowed.lock().contains(&(a.id.clone(), purpose.to_string())) || crate::apps::sign_allowed(node, &a.id, purpose);
    if !silent && !remembered {
        if !ask(node, &a, purpose, payload).await? {
            return Err("хозяин ПК не разрешил подпись".into());
        }
    }
    let s = seed(node)?;
    let k = derive(&s, &a.publisher);
    let line = signed_line(&a.id, &a.publisher, purpose, &ts_now(), payload);
    let sig = k.sign(line.as_bytes());
    node.info(&format!("[ключ] «{}» подписало «{}»{}", a.name, purpose, if silent { " (вход, без вопроса)" } else { "" }));
    Ok(json!({ "key": b64(k.verifying_key().as_bytes()), "sig": b64(&sig.to_bytes()), "signed": line }))
}

/// Спросить хозяина ПК в панели; ответ — через answer().
async fn ask(node: &Node, a: &crate::apps::App, purpose: &str, payload: &str) -> Result<bool, String> {
    let id = hex::encode(rand::random::<[u8; 8]>());
    let (tx, rx) = oneshot::channel();
    let preview: String = payload.chars().take(300).collect();
    let info = json!({ "ask": id, "id": a.id, "name": a.name, "purpose": purpose, "preview": preview, "publisher": a.publisher, "publisherFp": fingerprint_b64(&a.publisher) });
    {
        let mut g = node.identity.asks.lock();
        if g.len() >= 5 {
            return Err("слишком много запросов подписи ждут ответа".into());
        }
        g.insert(id.clone(), (info.clone(), tx));
    }
    let mut m = info.clone();
    m["media"] = json!("apps");
    m["type"] = json!("sign");
    node.media.deliver(m);
    let r = tokio::time::timeout(Duration::from_secs(90), rx).await;
    node.identity.asks.lock().remove(&id);
    match r {
        Ok(Ok(v)) => Ok(v),
        _ => Err("нет ответа хозяина ПК — подпись не выдана".into()),
    }
}

/// Ответ хозяина: разрешить (и запомнить до перезапуска или навсегда — в настройках приложения) или отказать.
pub fn answer(node: &Node, ask: &str, ok: bool, remember: bool, always: bool) -> Result<(), String> {
    let (info, tx) = node.identity.asks.lock().remove(ask).ok_or("запрос устарел")?;
    let (id, purpose) = (info["id"].as_str().unwrap_or("").to_string(), info["purpose"].as_str().unwrap_or("").to_string());
    if ok && always {
        crate::apps::add_sign_allowed(node, &id, &purpose)?;
        node.info(&format!("[ключ] «{}»: подписи «{}» — всегда без вопроса (снять — в настройках приложения)", info["name"].as_str().unwrap_or(""), purpose));
    } else if ok && remember {
        node.identity.allowed.lock().insert((id, purpose));
    }
    let _ = tx.send(ok);
    Ok(())
}

pub fn pending(node: &Node) -> Vec<Value> {
    node.identity.asks.lock().values().map(|(v, _)| v.clone()).collect()
}

/// Подписать «как издатель» (CLI `bridge app sign`): секрет издателя — base64url 32 байта.
pub fn publisher_key(secret_b64: &str) -> Result<SigningKey, String> {
    let b = unb64(secret_b64).ok_or("ключ издателя: не base64url")?;
    Ok(SigningKey::from_bytes(&<[u8; 32]>::try_from(b.as_slice()).map_err(|_| "ключ издателя — 32 байта")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn derive_sign_verify() {
        let s = [7u8; 32];
        let a = derive(&s, "pubA");
        let b = derive(&s, "pubB");
        assert_ne!(a.verifying_key(), b.verifying_key(), "разным издателям — разные ключи");
        assert_eq!(derive(&s, "pubA").verifying_key(), a.verifying_key(), "детерминированно");
        let line = signed_line("guildpets", "pubA", "login", "2026-10-28T10:00:00Z", "{\"nonce\":\"x\"}");
        let sig = b64(&a.sign(line.as_bytes()).to_bytes());
        assert!(verify(&b64(a.verifying_key().as_bytes()), line.as_bytes(), &sig));
        assert!(!verify(&b64(b.verifying_key().as_bytes()), line.as_bytes(), &sig));
        assert!(!verify(&b64(a.verifying_key().as_bytes()), b"other", &sig));
        assert_eq!(fingerprint(&[0u8; 32]).len(), 19);
        assert!(purpose_ok("login").is_ok() && purpose_ok("presence").is_err() && purpose_ok("Login").is_err() && purpose_ok("").is_err());
        assert_eq!(unseal(&seal(&s)).unwrap(), s);
    }
}
