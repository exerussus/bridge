//! Панель (ui/dist, собирается `npm run build` в папке ui) встраивается в мост: index.html и всё из /app/.
//! Нет сборки — встраивается страница-заглушка с подсказкой, как собрать.
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let dist = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../ui/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    println!("cargo:rerun-if-env-changed=BRIDGE_UI_DIST");
    let dist = std::env::var("BRIDGE_UI_DIST").map(PathBuf::from).unwrap_or(dist);
    let mut files = Vec::new();
    if dist.join("index.html").is_file() {
        walk(&dist, &dist, &mut files);
    }
    files.sort();
    let mut out = String::from("/// Файлы панели: (путь от корня, содержимое).\npub static UI_FILES: &[(&str, &[u8])] = &[\n");
    for (rel, abs) in &files {
        println!("cargo:rerun-if-changed={}", abs.display());
        let _ = writeln!(out, "    ({:?}, include_bytes!({:?})),", rel, abs.display().to_string());
    }
    out.push_str("];\n");
    let _ = writeln!(out, "pub const UI_BUILT: bool = {};", !files.is_empty());
    let dst = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ui_files.rs");
    std::fs::write(dst, out).unwrap();
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            println!("cargo:rerun-if-changed={}", p.display());
            walk(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if rel.ends_with(".map") {
                continue;
            }
            out.push((rel, p));
        }
    }
}
