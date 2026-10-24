//! Exerussus.Bridge — ядро: протокол, пути, игнор, настройки, подпись, файлы пространств, журнал записей.
//! Совместимо по проводу и по файлам с узлом 1.4 на .NET: можно обновлять ПК по одному.

pub mod config;
pub mod events;
pub mod files;
pub mod ignore;
pub mod path;
pub mod proto;
pub mod records;
pub mod reducer;
pub mod scanner;
pub mod schemas;
pub mod signing;
pub mod time;

pub use proto::*;

/// Версия приложения (из Cargo).
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Версия протокола между узлами.
pub const PROTOCOL: i32 = 1;
pub const API_PREFIX: &str = "/bridge/v1";
pub const AUTH_SCHEME: &str = "Bridge";
pub const LOCAL_TOKEN_HEADER: &str = "X-Bridge-Local";
/// Служебная папка внутри пространства: не синхронизируется как файлы.
pub const SERVICE_DIR: &str = "_bridge";
/// Папка схем DTO внутри пространства.
pub const SCHEMAS_DIR: &str = "_schemas";

/// JSON с отступами (как пишет узел .NET) — для файлов, которые читают люди.
pub fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| "null".into())
}

/// Записать файл атомарно: во временный рядом, потом замена (на Windows — с повторами: файл могут держать антивирус и индексатор).
pub fn write_atomic(file: &std::path::Path, text: &str) -> std::io::Result<()> {
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = file.with_file_name(format!(
        "{}.tmp-{}",
        file.file_name().and_then(|n| n.to_str()).unwrap_or("f"),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ));
    std::fs::write(&tmp, text.as_bytes())?;
    let mut attempt = 0;
    loop {
        match std::fs::rename(&tmp, file) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < 10 => {
                attempt += 1;
                let _ = e;
                std::thread::sleep(std::time::Duration::from_millis(50 * attempt));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
}

/// Сравнение строк без учёта регистра (как StringComparer.OrdinalIgnoreCase для латиницы и кириллицы).
#[inline]
pub fn eq_ic(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(b)
    } else {
        a.to_lowercase() == b.to_lowercase()
    }
}

/// Порядок строк как string.CompareOrdinal в .NET (по UTF-16).
pub fn cmp_ordinal(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}
