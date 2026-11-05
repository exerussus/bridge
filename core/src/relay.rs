//! Свой сервер-ретранслятор (`most-relay`, с 3.16): общее для моста и сервера.
//! Подключение: на сервере `most-relay invite` печатает одноразовое приглашение (5 минут, один раз). Мост меняет его
//! на свой ключ участника (`POST /relay/v1/join`): ключ приходит зашифрованным (X25519 + ChaCha20-Poly1305, ключ
//! шифрования — из общего секрета и кода приглашения), подсмотреть его в сети нельзя. Дальше мост раз в несколько
//! часов спрашивает временный ключ TURN (`GET /relay/v1/hello`, подпись — как между соседями, ключом участника),
//! отдаёт его своим панелям и, если разрешено, соседям в привете.

use serde::{Deserialize, Serialize};

pub const HELLO_PATH: &str = "/relay/v1/hello";
pub const JOIN_PATH: &str = "/relay/v1/join";
/// Срок жизни приглашения, секунд.
pub const INVITE_TTL: i64 = 300;
/// Порт HTTP по умолчанию: TCP 443 сервера делят TURN и HTTP (различаются по первому байту).
pub const DEFAULT_PORT: u16 = 443;

/// Ответ сервера мосту.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RelayHello {
    /// Имя сервера (для людей).
    pub name: String,
    pub version: String,
    /// Внешний адрес сервера (IPv4).
    pub host: String,
    /// Порт TURN по UDP (0 — нет).
    pub udp: u16,
    /// Порты TURN по TCP.
    pub tcp: Vec<u16>,
    pub username: String,
    pub credential: String,
    /// Срок ключа, секунд.
    pub ttl: u64,
    /// Под каким именем этот мост у сервера.
    pub member: String,
    /// Адрес моста, каким его видит сервер.
    pub you: String,
    /// Разрешает ли сервер гнать файлы и синхронизацию через свою сеть (3.19); None — сервер старше 3.19 (не против).
    pub files: Option<bool>,
    /// Ограничение скорости своей сети на участника, Мбит/с (0 — без ограничения).
    pub net_mbit: u32,
}

/// Сервер, которым мост делится с соседями (в привете), и он же — для `media/ice`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RelayInfo {
    pub name: String,
    pub host: String,
    pub udp: u16,
    pub tcp: Vec<u16>,
    pub username: String,
    pub credential: String,
}

impl RelayInfo {
    pub fn from_hello(h: &RelayHello) -> Self {
        RelayInfo { name: h.name.clone(), host: h.host.clone(), udp: h.udp, tcp: h.tcp.clone(), username: h.username.clone(), credential: h.credential.clone() }
    }

    /// Адреса для WebRTC: TURN по UDP, затем по TCP (443 — последним: он для самых строгих сетей).
    pub fn urls(&self) -> Vec<String> {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let mut v = Vec::new();
        if self.udp != 0 {
            v.push(format!("turn:{}:{}?transport=udp", host, self.udp));
        }
        let mut tcp = self.tcp.clone();
        tcp.sort_by_key(|p| (*p == 443, *p));
        for p in tcp {
            v.push(format!("turn:{}:{}?transport=tcp", host, p));
        }
        v
    }

    pub fn stun_url(&self) -> Option<String> {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        (self.udp != 0).then(|| format!("stun:{}:{}", host, self.udp))
    }
}

/// Приглашение на сервер: «most-relay 203.0.113.5:443 MRI-…» — одноразовое, живёт 5 минут.
/// Та же строка с мастер-ключом («MRM-…», 64 hex) — многоразовая: приложение «Сервер» и подключение моста.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RelayInvite {
    pub host: String,
    pub port: u16,
    pub code: String,
}

/// Новый код приглашения: «MRI-» и 32 hex (128 бит).
pub fn new_code() -> String {
    format!("MRI-{}", hex::encode(rand::random::<[u8; 16]>()).to_uppercase())
}

/// Как код хранится на сервере (сам код — только в напечатанной строке).
pub fn code_hash(code: &str) -> String {
    crate::path::sha256_bytes(format!("most-relay-invite\n{}", code.trim().to_uppercase()).as_bytes())
}

/// «адрес:порт» → (адрес, порт); без порта — 443.
pub fn split_addr(addr: &str) -> Result<(String, u16), String> {
    let (host, port) = match addr.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && !h.ends_with(':') => (h.trim_matches(['[', ']']).to_string(), p.parse::<u16>().map_err(|_| format!("порт «{}» — не число", p))?),
        _ => (addr.trim_matches(['[', ']']).to_string(), DEFAULT_PORT),
    };
    if host.is_empty() || host.parse::<std::net::IpAddr>().is_err() && !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Err(format!("адрес сервера «{}» не похож на IP или имя", host));
    }
    if port == 0 {
        return Err("порт — от 1 до 65535".into());
    }
    Ok((host, port))
}

pub fn base_url(host: &str, port: u16) -> String {
    let h = if host.contains(':') { format!("[{}]", host) } else { host.to_string() };
    format!("http://{}:{}", h, port)
}

impl RelayInvite {
    pub fn line(&self) -> String {
        format!("most-relay {}:{} {}", self.host, self.port, self.code)
    }

    pub fn parse(text: &str) -> Result<RelayInvite, String> {
        let mut parts: Vec<&str> = text.split_whitespace().collect();
        if parts.first().map(|p| p.eq_ignore_ascii_case("most-relay")) == Some(true) {
            parts.remove(0);
        }
        let up = parts.get(1).map(|p| p.to_uppercase()).unwrap_or_default();
        if parts.len() != 2 || !(up.starts_with("MRI-") || up.starts_with("MRM-")) {
            return Err("нужна строка приглашения целиком: «most-relay адрес:порт MRI-…» — её печатает most-relay invite на сервере (живёт 5 минут)".into());
        }
        let (host, port) = split_addr(parts[0])?;
        let code = up;
        let want = if code.starts_with("MRM-") { 68 } else { 36 };
        if code.len() != want || !code[4..].chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("код неполный — скопируйте строку целиком".into());
        }
        Ok(RelayInvite { host, port, code })
    }
}

/// Мост → сервер: код приглашения, имя этого ПК, открытый ключ X25519 (на один раз).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JoinRequest {
    pub code: String,
    pub name: String,
    pub key: String,
}

/// Сервер → мост: под каким именем он участник, открытый ключ сервера (на один раз) и зашифрованный ключ участника.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JoinResponse {
    pub member: String,
    pub server: String,
    pub key: String,
    pub sealed: String,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn aead_key(shared: &[u8; 32], code: &str) -> chacha20poly1305::Key {
    let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(code.trim().to_uppercase().as_bytes()), shared);
    let mut k = [0u8; 32];
    hk.expand(b"most-relay join v1", &mut k).expect("hkdf");
    k.into()
}

/// Сторона, которая ждёт ключ (мост): одноразовый секрет и открытый ключ для запроса.
pub struct JoinKeys {
    secret: x25519_dalek::EphemeralSecret,
    pub public: String,
}

impl JoinKeys {
    pub fn new() -> Self {
        use base64::Engine;
        let secret = x25519_dalek::EphemeralSecret::random_from_rng(rand::rngs::OsRng);
        let public = b64().encode(x25519_dalek::PublicKey::from(&secret).as_bytes());
        JoinKeys { secret, public }
    }

    /// Расшифровать ключ участника из ответа сервера.
    pub fn open(self, code: &str, resp: &JoinResponse) -> Result<String, String> {
        use base64::Engine;
        use chacha20poly1305::aead::Aead;
        use chacha20poly1305::KeyInit;
        let pk: [u8; 32] = b64().decode(&resp.key).ok().and_then(|v| v.try_into().ok()).ok_or("сервер прислал неверный ключ")?;
        let shared = self.secret.diffie_hellman(&x25519_dalek::PublicKey::from(pk));
        let raw = b64().decode(&resp.sealed).map_err(|_| "сервер прислал неверный ответ")?;
        if raw.len() < 13 {
            return Err("сервер прислал неверный ответ".into());
        }
        let c = chacha20poly1305::ChaCha20Poly1305::new(&aead_key(shared.as_bytes(), code));
        let plain = c.decrypt(chacha20poly1305::Nonce::from_slice(&raw[..12]), &raw[12..]).map_err(|_| "ключ не расшифровался — ответ подменили по дороге?")?;
        String::from_utf8(plain).map_err(|_| "ключ не расшифровался".into())
    }
}

impl Default for JoinKeys {
    fn default() -> Self {
        Self::new()
    }
}

/// Сервер: зашифровать ключ участника для моста. Возвращает (открытый ключ сервера, шифротекст).
pub fn seal(client_key: &str, code: &str, member_secret: &str) -> Result<(String, String), String> {
    use base64::Engine;
    use chacha20poly1305::aead::Aead;
    use chacha20poly1305::KeyInit;
    let pk: [u8; 32] = b64().decode(client_key).ok().and_then(|v| v.try_into().ok()).ok_or("неверный ключ моста")?;
    let mine = x25519_dalek::EphemeralSecret::random_from_rng(rand::rngs::OsRng);
    let public = b64().encode(x25519_dalek::PublicKey::from(&mine).as_bytes());
    let shared = mine.diffie_hellman(&x25519_dalek::PublicKey::from(pk));
    let c = chacha20poly1305::ChaCha20Poly1305::new(&aead_key(shared.as_bytes(), code));
    let nonce: [u8; 12] = rand::random();
    let mut out = nonce.to_vec();
    out.extend(c.encrypt(chacha20poly1305::Nonce::from_slice(&nonce), member_secret.as_bytes()).map_err(|_| "шифрование")?);
    Ok((public, b64().encode(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_roundtrip() {
        let i = RelayInvite { host: "203.0.113.5".into(), port: 443, code: new_code() };
        assert_eq!(RelayInvite::parse(&i.line()).unwrap(), i);
        let lower = format!("  203.0.113.5:443 {} ", i.code.to_lowercase());
        assert_eq!(RelayInvite::parse(&lower).unwrap(), i);
        let no_port = RelayInvite::parse(&format!("most-relay relay.example.net {}", i.code)).unwrap();
        assert_eq!((no_port.host.as_str(), no_port.port), ("relay.example.net", 443));
        assert!(RelayInvite::parse("most-relay 1.2.3.4:443").is_err());
        assert!(RelayInvite::parse("most-relay 1.2.3.4:443 MRI-1234").is_err());
        assert!(RelayInvite::parse(&format!("most-relay 1.2.3.4:x {}", i.code)).is_err());
        assert_eq!(code_hash(&i.code), code_hash(&i.code.to_lowercase()));
    }

    #[test]
    fn join_seal_open() {
        let code = new_code();
        let k = JoinKeys::new();
        let (key, sealed) = seal(&k.public, &code, "BRS-secret").unwrap();
        let resp = JoinResponse { member: "pc".into(), server: "r".into(), key: key.clone(), sealed: sealed.clone() };
        assert_eq!(k.open(&code, &resp).unwrap(), "BRS-secret");
        // другой код — не расшифровать
        let k2 = JoinKeys::new();
        let (key2, sealed2) = seal(&k2.public, &code, "BRS-secret").unwrap();
        assert!(k2.open(&new_code(), &JoinResponse { key: key2, sealed: sealed2, ..Default::default() }).is_err());
    }

    #[test]
    fn urls_order() {
        let r = RelayInfo { host: "203.0.113.5".into(), udp: 3478, tcp: vec![443, 3478], ..Default::default() };
        assert_eq!(r.urls(), vec!["turn:203.0.113.5:3478?transport=udp", "turn:203.0.113.5:3478?transport=tcp", "turn:203.0.113.5:443?transport=tcp"]);
        assert_eq!(r.stun_url().as_deref(), Some("stun:203.0.113.5:3478"));
    }
}
