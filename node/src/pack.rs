//! Передача пачкой (3.11): несколько файлов — одним архивом. Мелкие файлы не гоняют по запросу на каждый, а то, что
//! жмётся (тексты, исходники, данные), идёт меньше. Внутри — файлы под своими путями и `most-pack.json`:
//! `[{path, sha256, size}]` — что на самом деле легло в архив (файл мог измениться, пока его читали).
//! Получатель распаковывает каждый файл во временный, сверяет sha с тем, что ждал, и ставит как обычно.

use bridge_core::path;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const META: &str = "most-pack.json";
/// Сколько файлов и байт в одном архиве.
pub const MAX_FILES: usize = 1000;
pub const MAX_BYTES: i64 = 256 * 1024 * 1024;
/// Файлы крупнее идут по одному (в фоне, с прогрессом), а не в архиве.
pub const MAX_ITEM: i64 = 16 * 1024 * 1024;

/// Что положить в архив.
pub struct Item {
    pub path: String,
    pub full: PathBuf,
    /// Доп. поле для получателя (у общих папок — base, от какой версии правка).
    pub base: String,
}

/// Уже сжатое — без сжатия (тратить время незачем).
fn stored(p: &str) -> bool {
    let ext = p.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "zip" | "7z" | "rar" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "lz4" | "jpg" | "jpeg" | "png" | "webp" | "gif" | "avif" | "heic" | "mp3" | "ogg" | "opus" | "m4a" | "aac" | "flac" | "mp4" | "mkv" | "webm" | "avi" | "mov" | "docx" | "xlsx" | "pptx" | "apk" | "jar" | "unitypackage" | "bundle"
    )
}

struct Hashing<R> {
    inner: R,
    h: sha2::Sha256,
    n: u64,
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        sha2::Digest::update(&mut self.h, &buf[..n]);
        self.n += n as u64;
        Ok(n)
    }
}

/// Собрать архив в `dest`. Файлы, которые сейчас заняты, пропускаются. Возвращает, что легло: [{path, sha256, size, base}].
pub fn build(items: &[Item], dest: &Path) -> std::io::Result<Vec<Value>> {
    let f = std::fs::File::create(dest)?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(f));
    let deflate = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(3)).large_file(true);
    let store = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    let mut meta = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let Ok(file) = path::open_settled(&it.full) else { continue };
        // имя внутри — порядковый номер: путь хранится в most-pack.json (никаких путей из архива на диск)
        z.start_file(format!("{}", i), if stored(&it.path) { store } else { deflate }).map_err(std::io::Error::other)?;
        let mut r = Hashing { inner: file, h: <sha2::Sha256 as sha2::Digest>::new(), n: 0 };
        std::io::copy(&mut r, &mut z)?;
        let sha = hex::encode(sha2::Digest::finalize(r.h));
        meta.push(json!({ "i": i, "path": it.path, "sha256": sha, "size": r.n, "base": it.base }));
    }
    z.start_file(META, deflate).map_err(std::io::Error::other)?;
    z.write_all(serde_json::to_string(&meta).unwrap_or_default().as_bytes())?;
    z.finish().map_err(std::io::Error::other)?.flush()?;
    Ok(meta)
}

/// Распакованный файл: во временном файле, с проверенной sha.
pub struct Got {
    /// Путь как в архиве (с исходным регистром).
    pub path: String,
    pub tmp: PathBuf,
    pub sha: String,
    pub size: i64,
    pub base: String,
}

/// Распаковать архив: каждый файл — во временный (`tmp()`), sha считается заново. `want` — какие пути ждём (остальное
/// игнорируется). Порченые и лишние записи пропускаются; путь → файл.
pub fn extract(zip_file: &Path, want: &dyn Fn(&str) -> bool, tmp: &dyn Fn() -> PathBuf, max_item: i64) -> Result<HashMap<String, Got>, String> {
    let f = std::fs::File::open(zip_file).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f)).map_err(|e| format!("не архив: {}", e))?;
    let meta: Vec<Value> = {
        let mut e = z.by_name(META).map_err(|_| "в архиве нет most-pack.json".to_string())?;
        let mut s = String::new();
        e.by_ref().take(16 * 1024 * 1024).read_to_string(&mut s).map_err(|e| e.to_string())?;
        serde_json::from_str(&s).map_err(|e| e.to_string())?
    };
    let mut out = HashMap::new();
    for m in meta {
        let Some(p) = m["path"].as_str().and_then(path::norm) else { continue };
        if !want(&p) || out.contains_key(&p.to_lowercase()) {
            continue;
        }
        let Ok(mut e) = z.by_name(&m["i"].as_u64().map(|i| i.to_string()).unwrap_or_default()) else { continue };
        if e.size() as i64 > max_item {
            continue;
        }
        let t = tmp();
        let ok = (|| -> std::io::Result<(String, u64)> {
            let mut w = std::fs::File::create(&t)?;
            let mut r = Hashing { inner: e.by_ref().take(max_item as u64 + 1), h: <sha2::Sha256 as sha2::Digest>::new(), n: 0 };
            std::io::copy(&mut r, &mut w)?;
            w.flush()?;
            Ok((hex::encode(sha2::Digest::finalize(r.h)), r.n))
        })();
        match ok {
            Ok((sha, n)) if sha.eq_ignore_ascii_case(m["sha256"].as_str().unwrap_or("")) && n as i64 <= max_item => {
                out.insert(p.to_lowercase(), Got { path: p.clone(), tmp: t, sha, size: n as i64, base: m["base"].as_str().unwrap_or("").to_string() });
            }
            _ => {
                let _ = std::fs::remove_file(&t);
            }
        }
    }
    Ok(out)
}

/// Удалить временный файл, когда он больше не нужен (отданный архив — после отправки).
pub struct TmpGuard(pub PathBuf);

impl Drop for TmpGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Убрать брошенные временные файлы (старше суток) — после сбоя или выключения посреди передачи.
pub fn clean_tmp(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let day = std::time::Duration::from_secs(24 * 3600);
    for e in rd.flatten() {
        let Ok(m) = e.metadata() else { continue };
        if m.is_file() && m.modified().ok().and_then(|t| t.elapsed().ok()).map(|a| a > day).unwrap_or(false) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let d = std::env::temp_dir().join(format!("most-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.txt"), "привет ".repeat(1000)).unwrap();
        std::fs::write(d.join("b.png"), [1u8, 2, 3]).unwrap();
        std::fs::write(d.join("пусто.txt"), "").unwrap();
        let items: Vec<Item> = ["a.txt", "b.png", "пусто.txt", "нет.txt"].iter().map(|p| Item { path: p.to_string(), full: d.join(p), base: "b0".into() }).collect();
        let z = d.join("p.zip");
        let meta = build(&items, &z).unwrap();
        assert_eq!(meta.len(), 3, "несуществующий пропущен");
        assert!(std::fs::metadata(&z).unwrap().len() < 2000, "текст сжат");
        let n = std::sync::atomic::AtomicUsize::new(0);
        let tmp = || d.join(format!("t{}", n.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let got = extract(&z, &|p| p != "b.png", &tmp, 1 << 20).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(std::fs::read_to_string(&got["a.txt"].tmp).unwrap(), "привет ".repeat(1000));
        assert_eq!(got["пусто.txt"].size, 0);
        assert_eq!(got["a.txt"].base, "b0");
        let _ = std::fs::remove_dir_all(&d);
    }
}
