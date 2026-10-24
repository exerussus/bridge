//! Относительный путь внутри пространства: «папка/файл.md», прямые слэши, NFC.
//! Всё, что может выйти за пределы папки (.., абсолютные пути, диски, управляющие символы), отвергается.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

pub const MAX_LENGTH: usize = 400;
const BAD: &[char] = &[':', '*', '?', '"', '<', '>', '|'];
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Нормализовать путь; Err — причина, почему недопустим.
pub fn normalize(raw: &str) -> Result<String, &'static str> {
    if raw.trim().is_empty() {
        return Err("пустой путь");
    }
    let p: String = raw.replace('\\', "/").nfc().collect();
    if p.starts_with('/') || p.starts_with('~') {
        return Err("абсолютный путь");
    }
    if p.chars().count() > MAX_LENGTH {
        return Err("слишком длинный путь");
    }
    for part in p.split('/') {
        if part.is_empty() {
            return Err("пустой сегмент");
        }
        if part == "." || part == ".." {
            return Err("выход за папку (..)");
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err("сегмент оканчивается точкой или пробелом");
        }
        if part.contains(BAD) {
            return Err("недопустимый символ");
        }
        if part.chars().any(|c| c.is_control()) {
            return Err("управляющий символ");
        }
        let stem = part.split('.').next().unwrap_or("");
        if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
            return Err("зарезервированное имя Windows");
        }
    }
    Ok(p)
}

pub fn norm(raw: &str) -> Option<String> {
    normalize(raw).ok()
}

/// Ключ сравнения: Windows не различает регистр — и мост тоже.
pub fn key(path: &str) -> String {
    path.to_lowercase()
}

/// Служебный путь узла — не синхронизируется.
pub fn is_service(path: &str) -> bool {
    let s = crate::SERVICE_DIR.as_bytes();
    let p = path.as_bytes();
    p.len() >= s.len() && p[..s.len()].eq_ignore_ascii_case(s) && (p.len() == s.len() || p[s.len()] == b'/')
}

/// Временные файлы редакторов и систем синхронизации.
pub fn is_junk(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_lowercase();
    name.starts_with("~$")
        || name.starts_with(".~")
        || lower.ends_with(".tmp")
        || lower == "thumbs.db"
        || lower == "desktop.ini"
        || name == ".DS_Store"
}

/// Полный путь на диске; Err — если он вышел бы за пределы корня.
pub fn to_local(root: &Path, path: &str) -> std::io::Result<PathBuf> {
    let mut full = root.to_path_buf();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(['\\', ':']) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("Путь вне папки: {}", path)));
        }
        full.push(part);
    }
    Ok(full)
}

/// Относительный путь «a/b.txt» от корня (None — не внутри).
pub fn relative(root: &Path, full: &Path) -> Option<String> {
    let rel = full.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            std::path::Component::Normal(s) => parts.push(s.to_str()?.to_string()),
            _ => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Совпадение с шаблоном: «**» — любая глубина, «*» — в пределах сегмента, «?» — один знак. Регистр не важен.
pub fn glob(pattern: &str, path: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let s: Vec<char> = path.to_lowercase().chars().collect();
    glob_at(&p, &s)
}

fn glob_at(p: &[char], s: &[char]) -> bool {
    // Итеративно с возвратом для одиночной «*» и рекурсией для «**»
    let (mut pi, mut si) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() || pi < p.len() {
        if pi < p.len() {
            match p[pi] {
                '*' if pi + 1 < p.len() && p[pi + 1] == '*' => {
                    // «**/» — ноль и более сегментов; «**» — всё, что угодно
                    let rest_start = if pi + 2 < p.len() && p[pi + 2] == '/' { pi + 3 } else { pi + 2 };
                    let slash_form = rest_start == pi + 3;
                    let rest = &p[rest_start..];
                    let mut k = si;
                    loop {
                        if (!slash_form || k == si || s[k - 1] == '/') && glob_at(rest, &s[k..]) {
                            return true;
                        }
                        if k >= s.len() {
                            return false;
                        }
                        k += 1;
                    }
                }
                '*' => {
                    star = Some((pi, si));
                    pi += 1;
                    continue;
                }
                '?' if si < s.len() && s[si] != '/' => {
                    pi += 1;
                    si += 1;
                    continue;
                }
                c if si < s.len() && s[si] == c => {
                    pi += 1;
                    si += 1;
                    continue;
                }
                _ => {}
            }
        }
        // несовпадение: откат к последней «*», если она может съесть ещё один знак (не «/»)
        match star {
            Some((sp, ss)) if ss < s.len() && s[ss] != '/' => {
                pi = sp + 1;
                si = ss + 1;
                star = Some((sp, ss + 1));
            }
            _ => return false,
        }
    }
    true
}

pub fn sha256_bytes(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

pub fn sha256_reader<R: Read>(mut r: R) -> std::io::Result<String> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

pub fn sha256_file(file: &Path) -> std::io::Result<String> {
    sha256_reader(std::fs::File::open(file)?)
}

/// Открыть файл, который никто не пишет. На Windows — без разрешения записи другим: если файл ещё копируют,
/// открыть не выйдет, и он возьмётся в следующий круг. Время изменения тут не помогает: при копировании
/// Windows сразу ставит время источника.
pub fn open_settled(file: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(file)
    }
    #[cfg(not(windows))]
    {
        std::fs::File::open(file)
    }
}

pub fn sha256_file_settled(file: &Path) -> std::io::Result<String> {
    sha256_reader(open_settled(file)?)
}

/// Ссылка на папку (symlink / junction) — не обходим: можно выйти за пределы.
/// Только «суррогаты имени» (symlink, junction): у папок OneDrive тоже есть reparse point, но это обычные папки.
pub fn is_link(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_safe() {
        assert!(normalize("../x").is_err());
        assert!(normalize("/abs").is_err());
        assert!(normalize("a//b").is_err());
        assert!(normalize("con.txt").is_err());
        assert!(normalize("a/b.").is_err());
        assert!(normalize("a:b").is_err());
        assert_eq!(normalize("a\\b.txt").unwrap(), "a/b.txt");
        assert!(normalize("отчёты/файл.md").is_ok());
    }

    #[test]
    fn globbing() {
        assert!(glob("**", "a/b/c"));
        assert!(glob("отчёты/**", "Отчёты/a/b.md"));
        assert!(!glob("отчёты/**", "задания/a.md"));
        assert!(glob("*.md", "a.md"));
        assert!(!glob("*.md", "x/a.md"));
        assert!(glob("**/*.md", "a.md"));
        assert!(glob("**/*.md", "x/y/a.md"));
        assert!(!glob("**/*.md", "x/y/a.txt"));
        assert!(glob("a/**/b", "a/b"));
        assert!(glob("a/**/b", "a/x/y/b"));
        assert!(!glob("a/**/b", "a/xb"));
        assert!(glob("f?le.txt", "file.txt"));
        assert!(!glob("f?le.txt", "f/le.txt"));
        assert!(glob("состояние.md", "состояние.md"));
        assert!(!glob("состояние.md", "x/состояние.md"));
        assert!(glob("a*b*c", "axxbyyc"));
        assert!(!glob("a*b*c", "axxbyy"));
        assert!(glob("x/**", "x/"));
    }

    #[test]
    fn junk_and_service() {
        assert!(is_junk("a/~$doc.docx"));
        assert!(is_junk("Thumbs.db"));
        assert!(is_service("_bridge/x"));
        assert!(is_service("_Bridge"));
        assert!(!is_service("_bridgex"));
    }
}
