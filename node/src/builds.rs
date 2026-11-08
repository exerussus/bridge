//! Сборки (3.3.1): папка программы (например, сборка Unity) уходит соседу одним zip и распаковывается у него.
//! Два режима:
//! - патч — только файлы, изменившиеся с прошлой отправки этой сборки этому соседу, распаковка с заменой;
//! - чистая — всё целиком; у получателя прежняя папка сборки уходит в корзину, на её место — присланное.
//! Транспорт — ручная отправка: архив кладётся в «отправка/<сосед>/_сборки/<имя>/<режим>-<время>.zip»,
//! получатель, приняв такой файл, распаковывает его в папку сборки (по умолчанию «сборки/<имя>», можно задать свою).
//!
//! (3.3.5) Заготовки сборок: папка, кому (можно нескольким), режим и игнор — шаблоны как в .gitignore
//! (`Library/`, `*_BurstDebugInformation_DoNotShip/`, `**/*.pdb`, `!нужное.pdb` — вернуть исключённое).

use crate::node::Node;
use bridge_core::time;
use bridge_core::{eq_ic, path};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Папка в отправке/приёме, по которой узнаются сборки.
pub const DIR: &str = "_builds";
/// Прежнее имя (до 3.13): так шлём мостам старше 3.13 и так же принимаем от них.
pub const DIR_LEGACY: &str = "_сборки";

/// Папка сборок в отправке для этого соседа: старым мостам — прежнее имя, иначе они не распакуют.
fn dir_for(node: &Node, peer: &str) -> &'static str {
    let v = node.peer_state(peer).version;
    if !v.is_empty() && crate::update::cmp(&v, "3.13.0") == std::cmp::Ordering::Less { DIR_LEGACY } else { DIR }
}
const META: &str = "_most-build.json";

#[derive(Default)]
pub struct Builds {
    /// Что сейчас упаковывается: ключ «сосед/имя» → ход.
    pub jobs: Mutex<BTreeMap<String, Value>>,
    lock: Mutex<()>,
    /// (3.8) Запущенные присланные программы: «сосед/имя» → процесс; свои (3.21) — «@/имя».
    runs: Mutex<BTreeMap<String, std::process::Child>>,
    /// (3.21) Реестр версий: номер решается под замком — несколько соседей пакуются параллельно.
    ver_lock: Mutex<()>,
    /// (3.22) История и реестр общих сборок пишутся «прочитать — дописать — записать»: две упаковки, закончившие
    /// одновременно, теряли одну запись (у отправителя пропадала строка «№N → сосед»).
    hist_lock: Mutex<()>,
    /// (3.22.1) Заготовки: выдача GUID старым, сохранение и «Патчить эту сборку» — «прочитать — записать» под одним замком
    /// (иначе две одновременные выдачи давали заготовке два разных GUID).
    preset_lock: Mutex<()>,
}

/// Что запускать: полный путь внутри папки сборки `src` (D:\\Сборки\\Игра\\Game.exe) — в путь от неё (Game.exe).
/// Раньше полный путь отвергался, и заготовка не сохранялась целиком — программа «слетала» (3.11).
pub fn exe_in(src: &str, exe: &str) -> Result<String, String> {
    let e = exe.trim().trim_matches('"').replace('\\', "/");
    let abs = e.starts_with('/') || e.chars().nth(1) == Some(':');
    if !abs {
        return clean_exe(&e);
    }
    let base = src.trim().trim_matches('"').replace('\\', "/");
    let base = base.trim_end_matches('/');
    if !base.is_empty() && e.len() > base.len() + 1 && e[..base.len()].eq_ignore_ascii_case(base) && e.as_bytes()[base.len()] == b'/' {
        return clean_exe(&e[base.len() + 1..]);
    }
    Err("программа должна лежать в папке сборки".into())
}

/// Что запускать в сборке: путь внутри папки сборки (без «..» и абсолютных), пусто — ничего.
pub fn clean_exe(p: &str) -> Result<String, String> {
    let p = p.trim().replace('\\', "/");
    let p = p.trim_start_matches("./");
    if p.is_empty() {
        return Ok(String::new());
    }
    if p.chars().count() > 300 {
        return Err("путь программы — до 300 знаков".into());
    }
    path::norm(p).ok_or_else(|| "программа — путь внутри папки сборки (например, Game.exe или bin/Game.exe)".to_string())
}

/// (3.23) Сведения о сборке программы — `version.json` в корне папки сборки (его пишет AppCore при сборке Unity).
/// Файла может не быть — это нормально (None). Битый, слишком большой или без `version` — тоже None.
pub const APP_FILE: &str = "version.json";
const APP_MAX_BYTES: u64 = 64 * 1024;

pub fn app_info(dir: &Path) -> Option<Value> {
    let f = dir.join(APP_FILE);
    let md = std::fs::metadata(&f).ok()?;
    if !md.is_file() || md.len() > APP_MAX_BYTES {
        return None;
    }
    let s = std::fs::read(&f).ok()?;
    let s = s.strip_prefix(b"\xEF\xBB\xBF".as_slice()).unwrap_or(&s);
    let v: Value = serde_json::from_slice(s).ok()?;
    clean_app(&v)
}

/// Только известные поля, строки — без управляющих символов и ограниченной длины (META из чужого архива — не доверяем).
pub fn clean_app(v: &Value) -> Option<Value> {
    let o = v.as_object()?;
    let txt = |k: &str, max: usize| -> Option<String> {
        let t: String = o.get(k)?.as_str()?.trim().chars().filter(|c| !c.is_control()).take(max).collect();
        (!t.is_empty()).then_some(t)
    };
    let version = txt("version", 64)?;
    let mut out = serde_json::Map::new();
    out.insert("version".into(), json!(version));
    if let Some(x) = txt("fullVersion", 128) {
        out.insert("fullVersion".into(), json!(x));
    }
    if let Some(x) = txt("hash", 64).filter(|h| h.chars().all(|c| c.is_ascii_alphanumeric())) {
        out.insert("hash".into(), json!(x.to_lowercase()));
    }
    for k in ["profile", "platform"] {
        if let Some(x) = txt(k, 64) {
            out.insert(k.into(), json!(x));
        }
    }
    for k in ["dirty", "development"] {
        if let Some(b) = o.get(k).and_then(|b| b.as_bool()) {
            out.insert(k.into(), json!(b));
        }
    }
    if let Some(t) = txt("builtAtUtc", 40).filter(|t| chrono::DateTime::parse_from_rfc3339(t).is_ok()) {
        out.insert("builtAtUtc".into(), json!(t));
    }
    Some(Value::Object(out))
}

/// Похожее на запускаемое в папке сборки (для подсказки в заготовке).
fn exe_candidates(src: &Path, ig: &BuildIgnore) -> Vec<String> {
    let mut v: Vec<String> = bridge_core::scanner::walk(src, &[])
        .into_iter()
        .filter(|w| ig.which(&w.rel).is_none() && w.rel.split('/').count() <= 3)
        .map(|w| w.rel)
        .filter(|r| {
            let l = r.to_lowercase();
            (l.ends_with(".exe") || l.ends_with(".x86_64") || l.ends_with(".sh") || l.ends_with(".bat") || l.ends_with(".cmd")) && !l.contains("unitycrashhandler") && !l.contains("/monobleedingedge/")
        })
        .collect();
    v.sort_by_key(|r| (r.matches('/').count(), r.to_lowercase()));
    v.truncate(20);
    v
}

fn dir(node: &Node) -> PathBuf {
    node.data_dir.join("builds")
}
fn read(p: &Path) -> Value {
    std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}
fn write(p: &Path, v: &Value) {
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = bridge_core::write_atomic(p, &v.to_string());
}

/// Имя сборки: буквы, цифры, пробел, . _ - (до 60).
pub fn clean_name(n: &str) -> Result<String, String> {
    let n = n.trim();
    if n.is_empty() || n.chars().count() > 60 || n.starts_with('.') || !n.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-')) {
        return Err("имя сборки: буквы, цифры, пробел, точка, _ и - (до 60)".into());
    }
    Ok(n.to_string())
}

fn sent_file(node: &Node, peer: &str, name: &str) -> PathBuf {
    dir(node).join("sent").join(format!("{}--{}.json", crate::mobile::folder_name(peer), crate::mobile::folder_name(name)))
}
fn versions_file(node: &Node) -> PathBuf {
    dir(node).join("versions.json")
}
fn history_file(node: &Node) -> PathBuf {
    dir(node).join("history.json")
}
fn targets_file(node: &Node) -> PathBuf {
    dir(node).join("targets.json")
}
/// (3.11) Что запускать у присланных сборок: «сосед/имя» → {exe (как прислал отправитель), own (своё)}.
fn exes_file(node: &Node) -> PathBuf {
    dir(node).join("exes.json")
}
fn exe_key(peer: &str, name: &str) -> String {
    format!("{}/{}", peer.to_lowercase(), name)
}
/// Запомнить программу, которую указал отправитель (пустую — не затирать прежнюю).
fn remember_exe(node: &Node, peer: &str, name: &str, exe: &str) {
    if exe.is_empty() {
        return;
    }
    let f = exes_file(node);
    let mut t = read(&f);
    if !t.is_object() {
        t = json!({});
    }
    t[exe_key(peer, name)]["exe"] = json!(exe);
    write(&f, &t);
}
/// Своя программа для запуска присланной сборки (пусто — как у отправителя).
pub fn set_exe(node: &Node, peer: &str, name: &str, exe: &str) -> Result<String, String> {
    let name = clean_name(name)?;
    let target = target_dir(node, peer, &name);
    let exe = exe_in(&target.display().to_string(), exe)?;
    let f = exes_file(node);
    let mut t = read(&f);
    if !t.is_object() {
        t = json!({});
    }
    t[exe_key(peer, &name)]["own"] = json!(exe);
    write(&f, &t);
    node.emit("builds", json!({}));
    Ok(exe)
}
fn presets_file(node: &Node) -> PathBuf {
    dir(node).join("presets.json")
}

// ───────────── (3.22) общие сборки ─────────────
// У заготовки есть GUID (`gid`), автор (`owner`, имя узла) и соавторы (`authors`). GUID, автор, соавторы, получатели,
// игнор и полный набор файлов версии (путь → sha) едут в META архива. Получатель держит реестр gids.json:
// GUID → под каким «соседом» и именем лежит сборка (у всех соавторов — одна папка), кто автор и соавторы
// (список меняет только автор), номер и отпечаток того, что лежит. Патч от соавтора принимается, только если он
// в списке; версия старше уже стоящей не ставится. Получатель сообщает в привете {gid, n, digest} автору и
// соавторам — по отпечатку отправитель находит набор файлов в кэше maps/ и шлёт только отличия от того,
// что лежит у получателя на самом деле, а номер берёт следующим за самым большим известным.

fn gids_file(node: &Node) -> PathBuf {
    dir(node).join("gids.json")
}
fn maps_dir(node: &Node) -> PathBuf {
    dir(node).join("maps")
}
fn me(node: &Node) -> String {
    node.node_name()
}
/// Запись реестра общей сборки у получателя (или Null).
pub fn registry(node: &Node, gid: &str) -> Value {
    if gid.is_empty() {
        return Value::Null;
    }
    read(&gids_file(node))[gid].clone()
}
fn set_registry(node: &Node, gid: &str, v: Value) {
    let _g = node.builds.hist_lock.lock();
    let f = gids_file(node);
    let mut all = read(&f);
    if !all.is_object() {
        all = json!({});
    }
    all[gid] = v;
    write(&f, &all);
}
/// Набор файлов по отпечатку — в кэш (последние 60 наборов).
fn save_map(node: &Node, files: &BTreeMap<String, String>) {
    let d = maps_dir(node);
    let _ = std::fs::create_dir_all(&d);
    let digest = digest_of(files);
    let f = d.join(format!("{}.json", &digest[..32]));
    if !f.exists() {
        write(&f, &json!(files));
    }
    bridge_core::files::set_mtime(&f, &time::now());
    if let Ok(rd) = std::fs::read_dir(&d) {
        let mut all: Vec<(std::time::SystemTime, PathBuf)> = rd.filter_map(|e| e.ok()).filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
        if all.len() > 60 {
            all.sort();
            for (_, p) in all.iter().take(all.len() - 60) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}
fn load_map(node: &Node, digest: &str) -> Option<BTreeMap<String, String>> {
    if !is_digest(digest) {
        return None;
    }
    let v = read(&maps_dir(node).join(format!("{}.json", &digest[..32])));
    let m: BTreeMap<String, String> = serde_json::from_value(v).ok()?;
    (digest_of(&m) == digest).then_some(m)
}
/// Имена узлов из списка (чужие данные — только правильные имена, без повторов, до 64).
fn names_of(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in v.as_array().map(|a| a.as_slice()).unwrap_or(&[]).iter().filter_map(|x| x.as_str()).map(|s| s.trim()) {
        if bridge_core::config::is_name(s) && !out.iter().any(|o| eq_ic(o, s)) && out.len() < 64 {
            out.push(s.to_string());
        }
    }
    out
}
/// Отпечаток набора файлов — sha256 в hex (64 знака); остальное от соседей не принимается.
fn is_digest(d: &str) -> bool {
    d.len() == 64 && d.bytes().all(|c| c.is_ascii_hexdigit())
}
/// Предел номера версии: больше — порча или подделка (иначе номер «улетал» и переполнялся).
const MAX_N: u64 = 1_000_000_000;
/// Как этот сосед может быть записан у автора: имя здесь и имя, которым он сам себя называет.
fn ids_of(node: &Node, peer: &str) -> Vec<String> {
    let mut v = vec![peer.to_string()];
    let a = node.peer_state(peer).announced_name;
    if !a.is_empty() && !eq_ic(&a, peer) {
        v.push(a);
    }
    v
}
fn may_patch_any(owner: &str, authors: &[String], ids: &[String]) -> bool {
    ids.iter().any(|w| may_patch(owner, authors, w))
}
/// Кто вправе сообщать номера этой общей сборки: автор, соавторы и получатели заготовки.
fn trusted_of(pre: &Value) -> Vec<String> {
    let mut v = names_of(&pre["authors"]);
    v.extend(names_of(&pre["peers"]));
    if let Some(o) = pre["owner"].as_str() {
        v.push(o.to_string());
    }
    v
}
fn is_gid(g: &str) -> bool {
    (8..=64).contains(&g.len()) && g.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}
/// Может ли `who` патчить общую сборку с этим автором и соавторами.
fn may_patch(owner: &str, authors: &[String], who: &str) -> bool {
    eq_ic(owner, who) || authors.iter().any(|a| eq_ic(a, who))
}
/// Номера и отпечатки этой общей сборки, которые известны этому ПК: у себя как у получателя и в отчётах соседей.
/// (3.22.1) Отчёты — только от тех, кто вправе (`trusted`: автор, соавторы, получатели), с правдоподобным номером:
/// раньше любой сосед, знающий GUID, мог «раздуть» номер или переполнить его.
fn known_versions(node: &Node, gid: &str, trusted: &[String]) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    if gid.is_empty() {
        return out;
    }
    let r = registry(node, gid);
    if let (Some(n), Some(d)) = (r["n"].as_u64().filter(|n| *n <= MAX_N), r["digest"].as_str().filter(|d| is_digest(d))) {
        out.push((n, d.to_string()));
    }
    let peers: Vec<String> = node.cfg.read().peers.iter().map(|p| p.name.clone()).filter(|p| trusted.iter().any(|t| eq_ic(t, p))).collect();
    for p in peers {
        for b in node.peer_state(&p).builds.iter().filter(|b| b.gid == gid && b.ok && b.n <= MAX_N && is_digest(&b.digest)) {
            out.push((b.n, b.digest.clone()));
        }
    }
    out
}

/// (3.22.1) Сосед переименован: в заготовках (получатели, автор, соавторы), реестре общих сборок, истории,
/// папках и программах присланных сборок — новое имя (иначе общие сборки от него переставали приниматься).
/// Зовётся из `link::rename_refs` под замком настроек — поэтому только файлы, без чтения настроек.
pub fn rename_peer(node: &Node, from: &str, to: &str) {
    fn swap_list(v: &mut Value, from: &str, to: &str) {
        if let Some(a) = v.as_array_mut() {
            let mut out: Vec<Value> = Vec::new();
            for x in a.iter() {
                let y = if x.as_str().map(|s| eq_ic(s, from)).unwrap_or(false) { json!(to) } else { x.clone() };
                if !out.contains(&y) {
                    out.push(y);
                }
            }
            *a = out;
        }
    }
    fn swap_str(v: &mut Value, from: &str, to: &str) {
        if v.as_str().map(|s| eq_ic(s, from)).unwrap_or(false) {
            *v = json!(to);
        }
    }
    let pf = presets_file(node);
    let mut pl = read(&pf);
    if let Some(a) = pl.as_array_mut() {
        for p in a.iter_mut() {
            swap_list(&mut p["peers"], from, to);
            swap_list(&mut p["authors"], from, to);
            swap_str(&mut p["owner"], from, to);
        }
        write(&pf, &pl);
    }
    let _g = node.builds.hist_lock.lock();
    let gf = gids_file(node);
    let mut g = read(&gf);
    if let Some(o) = g.as_object_mut() {
        for r in o.values_mut() {
            swap_str(&mut r["peer"], from, to);
            swap_str(&mut r["owner"], from, to);
            swap_str(&mut r["by"], from, to);
            swap_list(&mut r["authors"], from, to);
            swap_list(&mut r["peers"], from, to);
        }
        write(&gf, &g);
    }
    let hf = history_file(node);
    let mut h = read(&hf);
    if let Some(a) = h.as_array_mut() {
        for e in a.iter_mut() {
            swap_str(&mut e["peer"], from, to);
            swap_str(&mut e["by"], from, to);
        }
        write(&hf, &h);
    }
    let prefix = format!("{}/", from.to_lowercase());
    let starts = |k: &str, p: &str| k.get(..p.len()).map(|x| x.eq_ignore_ascii_case(p)).unwrap_or(false);
    for f in [targets_file(node), exes_file(node)] {
        let t = read(&f);
        let Some(o) = t.as_object() else { continue };
        if !o.keys().any(|k| starts(k, &prefix)) {
            continue;
        }
        let m: serde_json::Map<String, Value> = o.iter().map(|(k, v)| (if starts(k, &prefix) { format!("{}/{}", to.to_lowercase(), &k[prefix.len()..]) } else { k.clone() }, v.clone())).collect();
        write(&f, &Value::Object(m));
    }
    // что уходило соседу (база патча) — под новым именем
    let sp = format!("{}--", crate::mobile::folder_name(from));
    if let Ok(rd) = std::fs::read_dir(dir(node).join("sent")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.len() > sp.len() && starts(&n, &sp) {
                let to_p = e.path().with_file_name(format!("{}--{}", crate::mobile::folder_name(to), &n[sp.len()..]));
                if !to_p.exists() {
                    let _ = std::fs::rename(e.path(), to_p);
                }
            }
        }
    }
}

// ───────────── заготовки ─────────────

/// Игнор сборки: общий игнор моста + свои шаблоны; `!шаблон` возвращает исключённое.
pub struct BuildIgnore {
    global: Arc<bridge_core::ignore::IgnoreRules>,
    own: bridge_core::ignore::IgnoreRules,
    keep: bridge_core::ignore::IgnoreRules,
}
impl BuildIgnore {
    pub fn new(node: &Node, lines: &[String]) -> Self {
        let own = lines.iter().filter(|l| !l.trim_start().starts_with('!')).cloned();
        let keep = lines.iter().filter_map(|l| l.trim_start().strip_prefix('!').map(|x| x.to_string()));
        BuildIgnore { global: node.cfg.read().global_ignore(), own: bridge_core::ignore::IgnoreRules::new(own), keep: bridge_core::ignore::IgnoreRules::new(keep) }
    }
    /// Какое правило исключает файл (None — файл идёт в архив).
    pub fn which(&self, rel: &str) -> Option<String> {
        if let Some(r) = self.global.which(rel) {
            return Some(format!("общий игнор: {}", r));
        }
        let r = self.own.which(rel)?;
        if self.keep.is_ignored(rel) {
            return None;
        }
        Some(r.to_string())
    }
}

fn lines_of(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).map(|x| x.to_string()).collect(),
        Value::String(t) => t.lines().map(|x| x.to_string()).collect(),
        _ => vec![],
    }
}

pub fn presets(node: &Node) -> Vec<Value> {
    let _g = node.builds.preset_lock.lock();
    presets_locked(node)
}
/// То же под уже взятым `preset_lock`.
fn presets_locked(node: &Node) -> Vec<Value> {
    let mut list = read(&presets_file(node)).as_array().cloned().unwrap_or_default();
    // (3.22) заготовкам до 3.22 — GUID и автор (этот ПК); записать сразу, чтобы GUID не менялся
    if list.iter().any(|p| !is_gid(p["gid"].as_str().unwrap_or(""))) {
        let mine = me(node);
        for p in list.iter_mut() {
            if !is_gid(p["gid"].as_str().unwrap_or("")) {
                p["gid"] = json!(uuid::Uuid::new_v4().to_string());
            }
            if p["owner"].as_str().unwrap_or("").is_empty() {
                p["owner"] = json!(mine);
            }
            if !p["authors"].is_array() {
                p["authors"] = json!([]);
            }
        }
        write(&presets_file(node), &json!(list));
    }
    list
}
fn preset(node: &Node, id: &str) -> Option<Value> {
    presets(node).into_iter().find(|p| p["id"] == id)
}

/// Сохранить заготовки целиком: [{id, name, path, peers: [..], mode, ignore: [строки]}] (до 50).
pub fn set_presets(node: &Node, list: &Value) -> Result<Vec<Value>, String> {
    let _g = node.builds.preset_lock.lock();
    set_presets_locked(node, list, false)
}
/// `adopt` — новая заготовка может прийти со своим GUID и автором (подхват общей сборки); из панели — нет:
/// у новой заготовки GUID новый и автор — этот ПК, у прежней (по id) GUID и автор не меняются.
fn set_presets_locked(node: &Node, list: &Value, adopt: bool) -> Result<Vec<Value>, String> {
    let a = list.as_array().ok_or("list: массив заготовок")?;
    if a.len() > 50 {
        return Err("заготовок — не больше 50".into());
    }
    let mine = me(node); // до замка настроек: имя узла читается из них же
    // (3.22) панель старше 3.22 (или телефон) шлёт заготовки без GUID, автора и соавторов — взять прежние по id
    let old: BTreeMap<String, Value> = presets_locked(node).into_iter().filter_map(|p| Some((p["id"].as_str()?.to_string(), p))).collect();
    let cfg = node.cfg.read();
    let mut out: Vec<Value> = Vec::new();
    for p in a {
        let id = p["id"].as_str().unwrap_or("").trim().to_string();
        if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("id заготовки «{}»: латиница, цифры, - и _ (до 40)", id));
        }
        if out.iter().any(|x| x["id"] == id.as_str()) {
            return Err(format!("id «{}» повторяется", id));
        }
        let name = clean_name(p["name"].as_str().unwrap_or("")).map_err(|e| format!("«{}»: {}", p["name"].as_str().unwrap_or(""), e))?;
        let path = p["path"].as_str().unwrap_or("").trim().to_string();
        if path.chars().count() > 1000 {
            return Err(format!("«{}»: слишком длинный путь", name));
        }
        let mut peers: Vec<String> = Vec::new();
        for x in p["peers"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str()) {
            let real = cfg.peer(x).map(|pc| pc.name.clone()).unwrap_or_else(|| x.to_string());
            if !peers.iter().any(|y| eq_ic(y, &real)) {
                peers.push(real);
            }
        }
        let mode = if p["mode"] == "clean" { "clean" } else { "patch" };
        let exe = exe_in(&path, p["exe"].as_str().unwrap_or("")).map_err(|e| format!("«{}»: {}", name, e))?;
        let ignore: Vec<String> = lines_of(&p["ignore"]).into_iter().map(|l| l.trim_end().to_string()).collect();
        if ignore.len() > 500 {
            return Err(format!("«{}»: в игноре не больше 500 строк", name));
        }
        let pats: Vec<String> = ignore.iter().map(|l| l.trim_start().trim_start_matches('!').to_string()).collect();
        if let Some(e) = bridge_core::ignore::validate(&pats, &format!("игнор «{}»", name)).into_iter().next() {
            return Err(e);
        }
        // (3.22) общая сборка: GUID (нет — новый), автор (нет — этот ПК), соавторы — соседи (без автора)
        let was = old.get(&id).cloned().unwrap_or(Value::Null);
        // (3.22.1) GUID и автор прежней заготовки — всегда сохранённые: устаревшая панель не подменит их
        let (want_gid, want_owner) = if was.is_null() && adopt { (p["gid"].as_str(), p["owner"].as_str()) } else { (was["gid"].as_str(), was["owner"].as_str()) };
        let gid = want_gid.map(|g| g.trim().to_lowercase()).filter(|g| is_gid(g)).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if out.iter().any(|x| x["gid"] == gid.as_str()) {
            return Err(format!("«{}»: GUID {} уже у другой заготовки", name, gid));
        }
        let owner = want_owner.map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).unwrap_or_else(|| mine.clone());
        let mut authors: Vec<String> = Vec::new();
        for a in names_of(if p["authors"].is_array() { &p["authors"] } else { &was["authors"] }) {
            let real = cfg.peer(&a).map(|pc| pc.name.clone()).unwrap_or(a);
            if !eq_ic(&real, &owner) && !authors.iter().any(|y| eq_ic(y, &real)) {
                authors.push(real);
            }
        }
        if authors.len() > 50 {
            return Err(format!("«{}»: соавторов — не больше 50", name));
        }
        out.push(json!({ "id": id, "name": name, "path": path, "peers": peers, "mode": mode, "ignore": ignore, "exe": exe, "gid": gid, "owner": owner, "authors": authors }));
    }
    drop(cfg);
    write(&presets_file(node), &json!(out));
    node.emit("builds", json!({}));
    Ok(out)
}

/// Что войдёт в архив: файлы и объём; что отсеет игнор — по правилам. Тяжёлое — звать из spawn_blocking.
pub fn preview(node: &Node, src: &str, ignore: &[String]) -> Result<Value, String> {
    let src = PathBuf::from(src.trim());
    if !src.is_absolute() || !src.is_dir() {
        return Err(format!("нет папки «{}» (нужен полный путь)", src.display()));
    }
    let ig = BuildIgnore::new(node, ignore);
    let (mut files, mut bytes) = (0i64, 0i64);
    let mut by: BTreeMap<String, (i64, i64, String)> = BTreeMap::new();
    let mut biggest: Vec<(i64, String)> = Vec::new();
    for w in bridge_core::scanner::walk(&src, &[]) {
        match ig.which(&w.rel) {
            Some(r) => {
                let e = by.entry(r).or_insert((0, 0, w.rel.clone()));
                e.0 += 1;
                e.1 += w.size;
            }
            None => {
                files += 1;
                bytes += w.size;
                biggest.push((w.size, w.rel.clone()));
            }
        }
    }
    biggest.sort_by(|a, b| b.0.cmp(&a.0));
    let mut rules: Vec<Value> = by.into_iter().map(|(r, (n, b, ex))| json!({ "rule": r, "files": n, "bytes": b, "example": ex })).collect();
    rules.sort_by(|a, b| b["bytes"].as_i64().cmp(&a["bytes"].as_i64()));
    let (ign_f, ign_b) = rules.iter().fold((0i64, 0i64), |a, r| (a.0 + r["files"].as_i64().unwrap_or(0), a.1 + r["bytes"].as_i64().unwrap_or(0)));
    Ok(json!({ "files": files, "bytes": bytes, "ignoredFiles": ign_f, "ignoredBytes": ign_b, "rules": rules, "biggest": biggest.into_iter().take(8).map(|(s, p)| json!({ "path": p, "bytes": s })).collect::<Vec<_>>(), "exes": exe_candidates(&src, &ig), "app": app_info(&src) }))
}

/// Отправить по заготовке всем её получателям.
pub fn send_preset(node: &Arc<Node>, id: &str, mode: Option<&str>, note: &str) -> Result<Value, (u16, String)> {
    send_preset_to(node, id, mode, note, "")
}

/// (3.21) То же, но только одному получателю заготовки (`only`; пусто — всем).
pub fn send_preset_to(node: &Arc<Node>, id: &str, mode: Option<&str>, note: &str, only: &str) -> Result<Value, (u16, String)> {
    let p = preset(node, id).ok_or((404, "нет такой заготовки".to_string()))?;
    let peers: Vec<String> = p["peers"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(String::from)).filter(|x| only.is_empty() || eq_ic(x, only)).collect();
    if peers.is_empty() && !only.is_empty() {
        return Err((400, format!("«{}» не среди получателей заготовки", only)));
    }
    if peers.is_empty() {
        return Err((400, "у заготовки не выбрано, кому отправлять".into()));
    }
    let mode = mode.filter(|m| !m.is_empty()).unwrap_or(p["mode"].as_str().unwrap_or("patch")).to_string();
    let ignore = lines_of(&p["ignore"]);
    let (mut ok, mut errs) = (Vec::new(), Vec::new());
    for peer in &peers {
        match send_full(node, peer, p["path"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or(""), &mode, &ignore, id, p["exe"].as_str().unwrap_or(""), note) {
            Ok(_) => ok.push(peer.clone()),
            Err((_, e)) => errs.push(format!("{}: {}", peer, e)),
        }
    }
    if ok.is_empty() {
        return Err((400, errs.join("; ")));
    }
    Ok(json!({ "ok": true, "started": ok, "errors": errs }))
}

/// Отпечаток набора файлов сборки (после игнора): путь и sha каждого.
pub fn digest_of(files: &BTreeMap<String, String>) -> String {
    let mut s = String::new();
    for (k, v) in files {
        s.push_str(k);
        s.push(':');
        s.push_str(v);
        s.push('\n');
    }
    path::sha256_bytes(s.as_bytes())
}

/// Версии сборки `name` у отправителя (старые первыми). Нет записи — засеять из того, что уже уходило соседям
/// (до 3.21 номер шёл свой у каждого соседа): берутся их наборы файлов и номера.
/// (3.22) Ключ реестра — GUID общей сборки (до 3.22 — имя; такая запись подхватывается, нумерация продолжается).
/// (3.22.1) `seed = false` — подхваченная общая сборка (автор не этот ПК): чужую историю по имени не подмешивать —
/// у соавтора могла быть своя, не связанная сборка с тем же именем.
fn versions(node: &Node, key: &str, name: &str, seed: bool) -> Vec<Value> {
    let all = read(&versions_file(node));
    if let Some(a) = all[key].as_array().or(if seed { all[name].as_array() } else { None }) {
        return a.clone();
    }
    if !seed {
        return Vec::new();
    }
    let mut seed: Vec<Value> = Vec::new();
    let peers: Vec<String> = node.cfg.read().peers.iter().map(|p| p.name.clone()).collect();
    for p in peers {
        let v = read(&sent_file(node, &p, name));
        let Some(n) = v["n"].as_u64() else { continue };
        let files: BTreeMap<String, String> = serde_json::from_value(v["files"].clone()).unwrap_or_default();
        if files.is_empty() {
            continue;
        }
        seed.push(json!({ "n": n, "digest": digest_of(&files), "at": v["at"], "files": files.len(), "note": "" }));
    }
    seed.sort_by_key(|x| x["n"].as_u64().unwrap_or(0));
    seed.dedup_by_key(|x| x["n"].as_u64().unwrap_or(0));
    seed
}

/// Номер для набора `now`: тот же, что у последней версии, если файлы те же; иначе следующий.
/// Заметка: своя у версии; пустая при повторе — берётся записанная.
/// (3.22) `extra` — номера и отпечатки, известные по получателям (общая сборка: её патчат и соавторы):
/// следующий номер — за самым большим из всех, а тот же набор файлов, что у последней известной, — её номер.
#[allow(clippy::too_many_arguments)]
fn version_for(node: &Node, key: &str, name: &str, seed: bool, now: &BTreeMap<String, String>, note: &str, extra: &[(u64, String)], app: &Option<Value>) -> (u64, String) {
    let _g = node.builds.ver_lock.lock();
    let d = digest_of(now);
    let mut list = versions(node, key, name, seed);
    let own_max = list.iter().filter_map(|x| x["n"].as_u64()).max().unwrap_or(0);
    let max = extra.iter().map(|x| x.0).max().unwrap_or(0).max(own_max);
    let mut last = list.iter().max_by_key(|x| x["n"].as_u64().unwrap_or(0)).cloned();
    if let Some((n, dg)) = extra.iter().filter(|x| x.0 > own_max).max_by_key(|x| x.0) {
        // у получателей версия новее своих — её выпустил соавтор: она «последняя»
        last = Some(json!({ "n": n, "digest": dg, "note": "" }));
    }
    let (n, note) = match last {
        Some(l) if l["digest"] == d.as_str() => {
            let n = l["n"].as_u64().unwrap_or(max);
            if !list.iter().any(|x| x["n"].as_u64() == Some(n)) {
                list.push(json!({ "n": n, "digest": d, "at": time::format(&time::now()), "files": now.len(), "note": note }));
            }
            // (3.23) сведения о программе — у версии (version.json мог лежать в игноре и не менять отпечаток)
            if let (Some(a), Some(e)) = (app, list.iter_mut().find(|x| x["n"].as_u64() == Some(n))) {
                e["app"] = a.clone();
            }
            let kept = l["note"].as_str().unwrap_or("").to_string();
            if !note.is_empty() && kept.is_empty() {
                if let Some(e) = list.iter_mut().find(|x| x["n"].as_u64() == Some(n)) {
                    e["note"] = json!(note);
                }
            }
            (n, if note.is_empty() { kept } else { note.to_string() })
        }
        _ => {
            let n = max.saturating_add(1).min(MAX_N);
            let mut e = json!({ "n": n, "digest": d, "at": time::format(&time::now()), "files": now.len(), "note": note });
            if let Some(a) = app {
                e["app"] = a.clone();
            }
            list.push(e);
            (n, note.to_string())
        }
    };
    if list.len() > 100 {
        list.drain(0..list.len() - 100);
    }
    let f = versions_file(node);
    let mut all = read(&f);
    if !all.is_object() {
        all = json!({});
    }
    all[key] = json!(list);
    write(&f, &all);
    save_map(node, now);
    (n, note)
}

/// Последняя версия сборки у отправителя (для панели): номер, заметка, время.
pub fn latest_version(node: &Node, key: &str, name: &str, seed: bool) -> Value {
    versions(node, key, name, seed).into_iter().max_by_key(|x| x["n"].as_u64().unwrap_or(0)).map(|mut x| {
        x.as_object_mut().map(|o| o.remove("digest"));
        x
    }).unwrap_or(Value::Null)
}

fn remember(node: &Node, e: Value) {
    let _g = node.builds.hist_lock.lock();
    let f = history_file(node);
    let mut h: Vec<Value> = read(&f).as_array().cloned().unwrap_or_default();
    h.push(e);
    if h.len() > 200 {
        h.drain(0..h.len() - 200);
    }
    write(&f, &json!(h));
    node.emit("builds", json!({}));
}

/// Куда распаковывать сборку `name` от `peer`.
pub fn target_dir(node: &Node, peer: &str, name: &str) -> PathBuf {
    let t = read(&targets_file(node));
    if let Some(p) = t[format!("{}/{}", peer.to_lowercase(), name)].as_str().filter(|p| !p.trim().is_empty()) {
        return node.resolve(p);
    }
    node.base_dir.join("builds").join(crate::mobile::folder_name(name))
}

pub fn set_target(node: &Node, peer: &str, name: &str, p: &str) -> Result<(), String> {
    let name = clean_name(name)?;
    let f = targets_file(node);
    let mut t = read(&f);
    if !t.is_object() {
        t = json!({});
    }
    let key = format!("{}/{}", peer.to_lowercase(), name);
    if p.trim().is_empty() {
        t.as_object_mut().unwrap().remove(&key);
    } else {
        let pb = PathBuf::from(p.trim());
        if !pb.is_absolute() {
            return Err("нужен полный путь, например D:\\Сборки\\Игра".into());
        }
        t[key] = json!(p.trim());
    }
    write(&f, &t);
    Ok(())
}

/// Состояние для панели.
pub fn status(node: &Node) -> Value {
    let mine = me(node);
    let own_gids: BTreeMap<String, String> = presets(node).iter().filter_map(|p| Some((p["gid"].as_str()?.to_string(), p["id"].as_str()?.to_string()))).collect();
    let h = read(&history_file(node));
    let mut recv: BTreeMap<String, Value> = BTreeMap::new();
    for e in h.as_array().cloned().unwrap_or_default().iter().filter(|e| e["dir"] == "in") {
        let k = format!("{}/{}", e["peer"].as_str().unwrap_or(""), e["name"].as_str().unwrap_or(""));
        recv.insert(k, e.clone());
    }
    let exes = read(&exes_file(node));
    let all_in: Vec<Value> = h.as_array().cloned().unwrap_or_default().into_iter().filter(|e| e["dir"] == "in").collect();
    let mut received: Vec<Value> = recv
        .values()
        .map(|e| {
            let (p, n) = (e["peer"].as_str().unwrap_or(""), e["name"].as_str().unwrap_or(""));
            let mut e = e.clone();
            let target = target_dir(node, p, n);
            e["target"] = json!(target.display().to_string());
            // история версий этой сборки (новые — первыми) и что запускать (из последней удачной)
            let hist: Vec<Value> = all_in.iter().filter(|x| x["peer"] == p && x["name"] == n).rev().take(30).cloned().collect();
            let last_ok = hist.iter().find(|x| x["error"].is_null());
            // что запускать: своё > последнее указанное отправителем (в истории или запомненное) — не «слетает»
            let ex = &exes[exe_key(p, n)];
            let sender = hist.iter().filter_map(|x| x["exe"].as_str()).find(|x| !x.is_empty()).or(ex["exe"].as_str()).unwrap_or("").to_string();
            let own = ex["own"].as_str().unwrap_or("").to_string();
            let exe = if own.is_empty() { sender.clone() } else { own.clone() };
            e["exe"] = json!(exe);
            e["exeSender"] = json!(sender);
            e["exeOwn"] = json!(own);
            e["exeExists"] = json!(!exe.is_empty() && path::to_local(&target, &exe).map(|f| f.is_file()).unwrap_or(false));
            e["lastOk"] = last_ok.cloned().unwrap_or(Value::Null);
            e["history"] = json!(hist);
            e["running"] = json!(running(node, p, n));
            // (3.22) общая: автор, соавторы, можно ли патчить отсюда, своя ли уже заготовка
            if let Some(gid) = hist.iter().find_map(|x| x["gid"].as_str().filter(|g| !g.is_empty())) {
                let r = registry(node, gid);
                let (owner, authors) = (r["owner"].as_str().unwrap_or("").to_string(), names_of(&r["authors"]));
                e["gid"] = json!(gid);
                e["owner"] = json!(owner);
                e["authors"] = json!(authors);
                e["canPatch"] = json!(!r.is_null() && may_patch(&owner, &authors, &mine));
                e["preset"] = json!(own_gids.get(gid).cloned().unwrap_or_default());
                e["by"] = hist.iter().find(|x| x["error"].is_null()).map(|x| x["by"].clone()).unwrap_or(Value::Null);
            }
            e
        })
        .collect();
    received.sort_by(|a, b| b["at"].as_str().unwrap_or("").cmp(a["at"].as_str().unwrap_or("")));
    // (3.21) свои заготовки: последняя версия, доставка каждому получателю, запущена ли у себя
    let tr = node.transfers.list();
    let presets: Vec<Value> = presets(node)
        .into_iter()
        .map(|mut p| {
            let (id, name) = (p["id"].as_str().unwrap_or("").to_string(), p["name"].as_str().unwrap_or("").to_string());
            let gid = p["gid"].as_str().unwrap_or("").to_string();
            let mine_p = eq_ic(p["owner"].as_str().unwrap_or(""), &mine);
            let latest = latest_version(node, if gid.is_empty() { &name } else { &gid }, &name, gid.is_empty() || mine_p);
            // (3.22) у получателей может стоять номер новее своего — его выпустил соавтор
            let known = known_versions(node, &gid, &trusted_of(&p)).into_iter().map(|x| x.0).max().unwrap_or(0);
            let ln = latest["n"].as_u64().unwrap_or(0).max(known);
            let peers: Vec<String> = p["peers"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_str().map(String::from)).collect();
            p["latest"] = latest;
            p["latestKnown"] = json!(known);
            p["mine"] = json!(mine_p);
            p["delivery"] = json!(peers.iter().map(|peer| delivery(node, &name, &gid, peer, ln, &tr)).collect::<Vec<_>>());
            p["running"] = json!(running(node, OWN, &id));
            // (3.23) version.json в папке сборки сейчас (что уйдёт со следующей отправкой)
            let path = p["path"].as_str().unwrap_or("").trim().to_string();
            p["app"] = json!(if path.is_empty() { None } else { app_info(Path::new(&path)) });
            p
        })
        .collect();
    json!({
        "history": h.as_array().cloned().unwrap_or_default().into_iter().rev().take(50).collect::<Vec<_>>(),
        "received": received,
        "jobs": node.builds.jobs.lock().values().cloned().collect::<Vec<_>>(),
        "presets": presets,
        "defaultRoot": node.base_dir.join("builds").display().to_string(),
    })
}

/// (3.21) Где сборка `name` у соседа `peer`: этап доставки последнего, что ему ушло.
/// state: packing → waiting (лежит в отправке) → transfer (забирает, pct) → received (забрал) → unpacked | error; none — не отправляли.
fn delivery(node: &Node, name: &str, gid: &str, peer: &str, latest: u64, tr: &[Value]) -> Value {
    let ps = node.peer_state(peer);
    let sent = read(&sent_file(node, peer, name));
    let sent_n = sent["n"].as_u64();
    let rep = (!gid.is_empty()).then(|| ps.builds.iter().find(|b| b.gid == gid)).flatten().or_else(|| ps.builds.iter().find(|b| b.name == name && b.gid.is_empty())).cloned();
    let has = rep.as_ref().filter(|r| r.ok).map(|r| r.n);
    let mut d = json!({ "peer": peer, "online": ps.online, "sent": sent_n, "has": has, "latest": latest, "pct": Value::Null, "error": "" });
    let job = node.builds.jobs.lock().iter().find(|(k, _)| eq_ic(k, &format!("{}/{}", peer, name))).map(|(_, v)| v.clone());
    if let Some(j) = job {
        d["state"] = json!("packing");
        let (done, total) = (j["done"].as_i64().unwrap_or(0), j["total"].as_i64().unwrap_or(0));
        d["pct"] = json!(if total > 0 { done * 100 / total } else { 0 });
        d["detail"] = json!(format!("{} · {} из {} файлов", j["stage"].as_str().unwrap_or(""), done, total));
        return d;
    }
    let base = node.mailbox.out_root(node).join(peer);
    let queued = [DIR, DIR_LEGACY].iter().any(|dn| std::fs::read_dir(base.join(dn).join(name)).map(|mut r| r.any(|e| e.map(|e| e.path().extension().map(|x| x == "zip").unwrap_or(false)).unwrap_or(false))).unwrap_or(false));
    let live = tr.iter().find(|t| t["kind"] == "serve" && eq_ic(t["peer"].as_str().unwrap_or(""), peer) && {
        let p = t["path"].as_str().unwrap_or("");
        [DIR, DIR_LEGACY].iter().any(|dn| p.starts_with(&format!("{}/{}/", dn, name)))
    });
    let state = if let Some(t) = live {
        let (done, total) = (t["done"].as_i64().unwrap_or(0), t["total"].as_i64().unwrap_or(0));
        d["pct"] = json!(if total > 0 { done * 100 / total } else { 0 });
        d["speed"] = t["speed"].clone();
        d["eta"] = t["eta"].clone();
        "transfer"
    } else if queued {
        "waiting"
    } else if sent_n.is_none() {
        // отсюда не слали, но сборка у него есть (прислал соавтор)
        if rep.as_ref().map(|r| r.ok && r.n > 0).unwrap_or(false) { "unpacked" } else { "none" }
    } else {
        match &rep {
            Some(r) if r.ok && r.n >= sent_n.unwrap_or(0) => "unpacked",
            Some(r) if Some(r.n) == sent_n => {
                d["error"] = json!(r.error);
                "error"
            }
            _ => "received",
        }
    };
    d["state"] = json!(state);
    if let Some(r) = rep {
        d["at"] = json!(r.at);
        // (3.23) какая версия программы у него стоит
        if !r.app.is_empty() {
            d["app"] = json!({ "version": r.app, "dirty": r.app_dirty });
        }
    }
    d
}

/// (3.22) «Патчить эту сборку»: присланная общая сборка → своя заготовка с тем же GUID. Папка — где сборка
/// получается у этого разработчика; получатели (и автор) и игнор — из сборки, их можно поправить потом.
pub fn adopt(node: &Node, gid: &str, path: &str) -> Result<Value, String> {
    let gid = gid.trim().to_lowercase();
    let r = registry(node, &gid);
    if r.is_null() {
        return Err("это не общая сборка (или пришла до 3.22) — попросите автора отправить её заново".into());
    }
    let mine = me(node);
    let (owner, authors) = (r["owner"].as_str().unwrap_or("").to_string(), names_of(&r["authors"]));
    if !may_patch(&owner, &authors, &mine) {
        return Err(format!("{} не добавил вас в соавторы этой сборки — попросите его", owner));
    }
    let _g = node.builds.preset_lock.lock();
    let mut list = presets_locked(node);
    if let Some(p) = list.iter().find(|p| p["gid"] == gid.as_str()) {
        return Err(format!("эта сборка уже у вас в заготовках — «{}»", p["name"].as_str().unwrap_or("")));
    }
    let src = PathBuf::from(path.trim());
    if !src.is_absolute() || !src.is_dir() {
        return Err(format!("нет папки «{}» (нужен полный путь к папке, куда собирается проект)", path.trim()));
    }
    let name = r["name"].as_str().unwrap_or("").to_string();
    let mut peers: Vec<String> = Vec::new();
    {
        let cfg = node.cfg.read();
        for x in names_of(&r["peers"]).into_iter().chain(std::iter::once(owner.clone())) {
            if let Some(pc) = cfg.peer(&x) {
                if !eq_ic(&pc.name, &mine) && !peers.iter().any(|y| eq_ic(y, &pc.name)) {
                    peers.push(pc.name.clone());
                }
            }
        }
    }
    let exe = read(&exes_file(node))[exe_key(r["peer"].as_str().unwrap_or(""), &name)]["exe"].as_str().unwrap_or("").to_string();
    let exe = exe_in(&src.display().to_string(), &exe).unwrap_or_default();
    let mut id = format!("s-{}", &gid[..gid.len().min(8)]);
    while list.iter().any(|p| p["id"] == id.as_str()) {
        id.push('x');
    }
    list.push(json!({ "id": id, "name": name, "path": src.display().to_string(), "peers": peers, "mode": "patch", "ignore": r["ignore"], "exe": exe, "gid": gid, "owner": owner, "authors": authors }));
    set_presets_locked(node, &json!(list), true)?;
    node.info(&format!("[сборки] «{}» — теперь патчите и вы (автор {}), папка {}", name, owner, src.display()));
    Ok(json!({ "ok": true, "id": id, "name": name, "peers": peers }))
}

/// (3.23) Короткая версия программы для отчёта: «0.11.0» и «с незакоммиченными правками».
fn app_of(a: &Value) -> (String, bool) {
    (a["version"].as_str().unwrap_or("").chars().take(64).collect(), a["dirty"].as_bool().unwrap_or(false))
}

/// (3.21) Отчёт для привета соседу `peer`: его сборки у нас — последняя версия каждой и распаковалась ли.
pub fn report_for(node: &Node, peer: &str) -> Vec<bridge_core::proto::BuildReport> {
    let h = read(&history_file(node));
    let mut by: BTreeMap<String, bridge_core::proto::BuildReport> = BTreeMap::new();
    let hist = h.as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    for e in hist.iter().filter(|e| e["dir"] == "in" && e["gid"].as_str().unwrap_or("").is_empty() && eq_ic(e["peer"].as_str().unwrap_or(""), peer)) {
        let name = e["name"].as_str().unwrap_or("").to_string();
        let ok = e["error"].is_null();
        let n = e["n"].as_u64().unwrap_or_else(|| by.get(&name).map(|b| b.n).unwrap_or(0));
        // (3.23) версия программы — та, что стоит: при ошибке остаётся от прошлой удачной
        let (app, app_dirty) = if ok { app_of(&e["app"]) } else { by.get(&name).map(|b| (b.app.clone(), b.app_dirty)).unwrap_or_default() };
        by.insert(name.clone(), bridge_core::proto::BuildReport { name, n, ok, error: e["error"].as_str().unwrap_or("").chars().take(300).collect(), at: e["at"].as_str().unwrap_or("").into(), app, app_dirty, ..Default::default() });
    }
    let mut out: Vec<_> = by.into_values().collect();
    // (3.22) общие сборки — автору и каждому соавтору: что стоит (номер и отпечаток набора)
    // (3.22.1) отказ виден тому, чей патч отвергнут (номер — тот, что он прислал), остальным — что стоит
    let ids = ids_of(node, peer);
    for (gid, r) in read(&gids_file(node)).as_object().cloned().unwrap_or_default() {
        if !may_patch_any(r["owner"].as_str().unwrap_or(""), &names_of(&r["authors"]), &ids) {
            continue;
        }
        let last = hist.iter().rev().find(|e| e["dir"] == "in" && e["gid"] == gid.as_str() && (e["error"].is_null() || eq_ic(e["by"].as_str().unwrap_or(""), peer)));
        let err = last.and_then(|e| e["error"].as_str()).unwrap_or("");
        let n = if err.is_empty() { r["n"].as_u64() } else { last.and_then(|e| e["n"].as_u64()) };
        out.push(bridge_core::proto::BuildReport {
            name: r["name"].as_str().unwrap_or("").into(),
            n: n.unwrap_or(0),
            ok: err.is_empty(),
            error: err.chars().take(300).collect(),
            at: last.and_then(|e| e["at"].as_str()).or(r["at"].as_str()).unwrap_or("").into(),
            gid: gid.clone(),
            digest: r["digest"].as_str().unwrap_or("").into(),
            app: app_of(&r["app"]).0,
            app_dirty: app_of(&r["app"]).1,
        });
    }
    out
}

// ───────────── запуск присланного ─────────────

/// Ключ «соседа» для своих сборок в `runs` (3.21): запуск из папки заготовки.
const OWN: &str = "@";

/// (3.21) Запустить свою сборку — программу заготовки из её папки на этом ПК.
pub fn run_own(node: &Node, id: &str) -> Result<Value, String> {
    if let Some(g) = node.remote.active() {
        return Err(format!("пока этим ПК управляет {}, программы не запускаются", g.peer));
    }
    let p = preset(node, id).ok_or("нет такой заготовки")?;
    if running(node, OWN, id) {
        return Ok(json!({ "ok": true, "already": true }));
    }
    let src = PathBuf::from(p["path"].as_str().unwrap_or("").trim());
    let exe = p["exe"].as_str().unwrap_or("");
    if exe.is_empty() {
        return Err("не указано, что запускать: «Папка и запуск» у сборки".into());
    }
    let full = path::to_local(&src, exe).map_err(|_| "программа вне папки сборки".to_string())?;
    if !full.is_file() {
        return Err(format!("нет файла «{}» в папке сборки", exe));
    }
    let child = std::process::Command::new(&full)
        .current_dir(full.parent().unwrap_or(&src))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("не запустилось «{}»: {}", exe, e))?;
    let pid = child.id();
    node.builds.runs.lock().insert(run_key(OWN, id), child);
    node.info(&format!("[сборки] запущено у себя «{}» из сборки «{}» (pid {})", exe, p["name"].as_str().unwrap_or(""), pid));
    node.emit("builds", json!({}));
    Ok(json!({ "ok": true, "pid": pid }))
}

pub fn stop_own(node: &Node, id: &str) -> Result<Value, String> {
    stop(node, OWN, id)
}

fn run_key(peer: &str, name: &str) -> String {
    format!("{}/{}", peer.to_lowercase(), name)
}

pub fn running(node: &Node, peer: &str, name: &str) -> bool {
    let mut g = node.builds.runs.lock();
    let k = run_key(peer, name);
    match g.get_mut(&k).map(|c| c.try_wait()) {
        Some(Ok(None)) => true,
        Some(_) => {
            g.remove(&k);
            false
        }
        None => false,
    }
}

/// Запустить программу присланной сборки (то, что указал отправитель). Только с этого ПК, по нажатию человека.
pub fn run(node: &Node, peer: &str, name: &str) -> Result<Value, String> {
    if let Some(g) = node.remote.active() {
        return Err(format!("пока этим ПК управляет {}, программы не запускаются", g.peer));
    }
    let name = clean_name(name)?;
    if running(node, peer, &name) {
        return Ok(json!({ "ok": true, "already": true }));
    }
    let st = status(node);
    let r = st["received"].as_array().and_then(|a| a.iter().find(|x| eq_ic(x["peer"].as_str().unwrap_or(""), peer) && x["name"] == name.as_str()).cloned()).ok_or("такой сборки здесь нет")?;
    let exe = r["exe"].as_str().unwrap_or("");
    if exe.is_empty() {
        return Err("не указано, что запускать: «Что запускать…» у сборки".into());
    }
    let target = target_dir(node, peer, &name);
    let full = path::to_local(&target, exe).map_err(|_| "программа вне папки сборки".to_string())?;
    if !full.is_file() {
        return Err(format!("нет файла «{}» в папке сборки", exe));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&full, std::fs::Permissions::from_mode(0o755));
    }
    let child = std::process::Command::new(&full)
        .current_dir(full.parent().unwrap_or(&target))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("не запустилось «{}»: {}", exe, e))?;
    let pid = child.id();
    node.builds.runs.lock().insert(run_key(peer, &name), child);
    node.info(&format!("[сборки] запущено «{}» из сборки «{}» от {} (pid {})", exe, name, peer, pid));
    node.emit("builds", json!({}));
    Ok(json!({ "ok": true, "pid": pid }))
}

pub fn stop(node: &Node, peer: &str, name: &str) -> Result<Value, String> {
    let c = node.builds.runs.lock().remove(&run_key(peer, name));
    match c {
        Some(mut c) => {
            let _ = c.kill();
            let _ = c.wait();
            node.info(&format!("[сборки] остановлено: сборка «{}» от {}", name, peer));
            node.emit("builds", json!({}));
            Ok(json!({ "ok": true }))
        }
        None => Ok(json!({ "ok": true, "already": true })),
    }
}

// ───────────── отправка ─────────────

/// Упаковать папку и поставить в отправку. Долгое — идёт в фоне; ход виден в `jobs`.
pub fn send(node: &Arc<Node>, peer: &str, src: &str, name: &str, mode: &str) -> Result<Value, (u16, String)> {
    send_ex(node, peer, src, name, mode, &[], "")
}

pub fn send_ex(node: &Arc<Node>, peer: &str, src: &str, name: &str, mode: &str, ignore: &[String], preset: &str) -> Result<Value, (u16, String)> {
    send_full(node, peer, src, name, mode, ignore, preset, "", "")
}

/// Отправка с программой для запуска (`exe` — путь внутри сборки) и заметкой «что нового» (до 2000 знаков).
#[allow(clippy::too_many_arguments)]
pub fn send_full(node: &Arc<Node>, peer: &str, src: &str, name: &str, mode: &str, ignore: &[String], preset: &str, exe: &str, note: &str) -> Result<Value, (u16, String)> {
    let exe = exe_in(src, exe).map_err(|e| (400, e))?;
    let note: String = note.trim().chars().take(2000).collect();
    let pc = node.cfg.read().peer(peer).cloned().ok_or((404, format!("нет соседа «{}»", peer)))?;
    let name = clean_name(name).map_err(|e| (400, e))?;
    if mode != "patch" && mode != "clean" {
        return Err((400, "mode: patch (только изменённое) или clean (целиком)".into()));
    }
    let src = PathBuf::from(src.trim());
    if !src.is_absolute() || !src.is_dir() {
        return Err((400, format!("нет папки «{}» (нужен полный путь)", src.display())));
    }
    let key = format!("{}/{}", pc.name, name);
    if node.builds.jobs.lock().contains_key(&key) {
        return Err((409, "эта сборка этому соседу уже упаковывается".into()));
    }
    node.builds.jobs.lock().insert(key.clone(), json!({ "peer": pc.name, "name": name, "mode": mode, "preset": preset, "stage": "считаю файлы", "done": 0, "total": 0 }));
    let n2 = node.clone();
    let (peer, mode2, ignore, preset) = (pc.name.clone(), mode.to_string(), ignore.to_vec(), preset.to_string());
    tokio::task::spawn_blocking(move || {
        let pre = if preset.is_empty() { Value::Null } else { presets(&n2).into_iter().find(|p| p["id"] == preset.as_str()).unwrap_or(Value::Null) };
        let r = pack(&n2, &peer, &src, &name, &mode2, &key, &ignore, &exe, &note, &pre).map(|mut e| {
            e["preset"] = json!(preset);
            e
        });
        n2.builds.jobs.lock().remove(&key);
        match r {
            Ok(e) if e["already"] == true => {
                n2.info(&format!("[сборки] «{}» → {}: у соседа уже №{} — отправлять нечего", name, peer, e["n"]));
                remember(&n2, e);
            }
            Ok(e) => {
                n2.info(&format!("[сборки] «{}» №{} → {}: {} ({} файлов, {})", name, e["n"], peer, if mode2 == "clean" { "чистая" } else { "патч" }, e["files"], crate::folders::human(e["bytes"].as_i64().unwrap_or(0))));
                remember(&n2, e);
                n2.mailbox.dirty.store(true, std::sync::atomic::Ordering::SeqCst);
                n2.notify_peers(vec![peer.clone()], "", "mailbox");
            }
            Err(e) => {
                n2.info(&format!("[сборки] «{}» → {}: не упаковалась — {}", name, peer, e));
                remember(&n2, json!({ "dir": "out", "peer": peer, "name": name, "mode": mode2, "preset": preset, "at": time::format(&time::now()), "error": e }));
            }
        }
    });
    Ok(json!({ "ok": true, "started": true }))
}

#[allow(clippy::too_many_arguments)]
fn pack(node: &Arc<Node>, peer: &str, src: &Path, name: &str, mode: &str, key: &str, ignore: &[String], exe: &str, note: &str, pre: &Value) -> Result<Value, String> {
    let ig = BuildIgnore::new(node, ignore);
    let all = bridge_core::scanner::walk(src, &[]);
    let n_all = all.len();
    let files: Vec<_> = all.into_iter().filter(|w| ig.which(&w.rel).is_none()).collect();
    let skipped = n_all - files.len();
    if files.is_empty() {
        return Err(if skipped > 0 { "всё в папке отсеяно игнором".into() } else { "папка пуста".into() });
    }
    let total = files.len();
    let set_job = |stage: &str, done: usize| {
        if let Some(j) = node.builds.jobs.lock().get_mut(key) {
            j["stage"] = json!(stage);
            j["done"] = json!(done);
            j["total"] = json!(total);
        }
    };
    // отпечатки — чтобы патч взял только изменённое
    let mut now: BTreeMap<String, String> = BTreeMap::new();
    for (i, w) in files.iter().enumerate() {
        let sha = path::sha256_file(&w.full).map_err(|e| format!("{}: {}", w.rel, e))?;
        now.insert(w.rel.clone(), sha);
        if i % 50 == 0 {
            set_job("сверяю файлы", i);
        }
    }
    let sf = sent_file(node, peer, name);
    let prev_v = read(&sf);
    let mut prev: BTreeMap<String, String> = serde_json::from_value(prev_v["files"].clone()).unwrap_or_default();
    // (3.21) номер ведёт отправитель: один на сборку, растёт только при изменении файлов
    // (3.22) общая сборка: номер — по GUID, с учётом версий соавторов; база патча — то, что лежит у получателя
    let gid = pre["gid"].as_str().unwrap_or("").to_string();
    let vkey = if gid.is_empty() { name.to_string() } else { gid.clone() };
    let seed = gid.is_empty() || eq_ic(pre["owner"].as_str().unwrap_or(""), &node.node_name());
    // (3.23) version.json сборки (AppCore) — едет в META, в историю и к версии
    let app = app_info(src);
    let (n, note) = version_for(node, &vkey, name, seed, &now, note, &known_versions(node, &gid, &trusted_of(pre)), &app);
    let mut first = prev.is_empty();
    let has = if gid.is_empty() { None } else { node.peer_state(peer).builds.iter().find(|b| b.gid == gid && is_digest(&b.digest)).cloned() };
    if let Some(r) = has {
        if prev.is_empty() || r.digest != digest_of(&prev) {
            // у получателя не то, что ему слали отсюда (патчил соавтор): набор по отпечатку; не знаем — все файлы
            prev = load_map(node, &r.digest).unwrap_or_default();
        }
        first = false; // сборка у него уже есть: патч поверх, а не чистая
    }
    let pick: Vec<_> = files.iter().filter(|w| mode == "clean" || first || prev.get(&w.rel) != now.get(&w.rel)).collect();
    if pick.is_empty() {
        // у соседа уже ровно эта версия — не ошибка
        write(&sf, &json!({ "files": now, "at": prev_v["at"], "n": n }));
        return Ok(json!({ "dir": "out", "peer": peer, "name": name, "mode": mode, "at": time::format(&time::now()), "files": 0, "all": now.len(), "bytes": 0, "zip": 0, "src": src.display().to_string(), "already": true, "exe": exe, "note": note, "n": n, "gid": gid, "app": app }));
    }
    let note = note.as_str();
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let rel = format!("{}/{}/{}-{}.zip", dir_for(node, peer), name, mode, ts);
    let out = path::to_local(&node.mailbox.out_root(node).join(peer), &rel).map_err(|_| "путь".to_string())?;
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = node.tmp_file();
    let mut bytes: i64 = 0;
    {
        let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut z = zip::ZipWriter::new(std::io::BufWriter::new(f));
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(3)).large_file(true);
        let mut meta = json!({ "name": name, "mode": mode, "from": node.node_name(), "at": time::format(&time::now()), "files": pick.len(), "all": now.len(), "first": first, "exe": exe, "note": note, "n": n });
        // (3.23) всегда — и null: так получатель знает, что отправитель version.json искал и не нашёл
        meta["app"] = json!(app);
        if !gid.is_empty() {
            meta["gid"] = json!(gid);
            meta["owner"] = pre["owner"].clone();
            meta["authors"] = pre["authors"].clone();
            meta["peers"] = pre["peers"].clone();
            meta["ignore"] = pre["ignore"].clone();
            meta["digest"] = json!(digest_of(&now));
            meta["manifest"] = json!(now);
        }
        z.start_file(META, opt).map_err(|e| e.to_string())?;
        z.write_all(meta.to_string().as_bytes()).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 1 << 20];
        for (i, w) in pick.iter().enumerate() {
            z.start_file(w.rel.as_str(), opt).map_err(|e| e.to_string())?;
            let mut f = std::fs::File::open(&w.full).map_err(|e| format!("{}: {}", w.rel, e))?;
            loop {
                let n = f.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                z.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            }
            bytes += w.size;
            if i % 20 == 0 {
                set_job("упаковываю", i);
            }
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    let zsize = std::fs::metadata(&tmp).map(|m| m.len() as i64).unwrap_or(0);
    let max = node.max_file_bytes();
    if zsize > max {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("архив {} больше предела файла (maxFileMb)", crate::folders::human(zsize)));
    }
    bridge_core::files::move_replace(&tmp, &out).map_err(|e| e.to_string())?;
    bridge_core::files::set_mtime(&out, &(time::now() - chrono::Duration::seconds(5)));
    node.mailbox.mark_ready(&out);
    // отпечатки — «что у соседа» после этой отправки
    write(&sf, &json!({ "files": now, "at": time::format(&time::now()), "n": n }));
    Ok(json!({ "dir": "out", "peer": peer, "name": name, "mode": if first && mode == "patch" { "clean" } else { mode }, "at": time::format(&time::now()), "files": pick.len(), "all": now.len(), "bytes": bytes, "zip": zsize, "src": src.display().to_string(), "first": first, "skipped": skipped, "exe": exe, "note": note, "n": n, "gid": gid, "app": app }))
}

// ───────────── приём ─────────────

/// Принятый файл ручной отправки: если это сборка — распаковать. `rel` — путь в «приём/<сосед>/».
pub fn on_received(node: &Arc<Node>, peer: &str, file: &Path, rel_in_peer: &str) {
    let parts: Vec<&str> = rel_in_peer.split('/').collect();
    if parts.len() != 3 || !(eq_ic(parts[0], DIR) || eq_ic(parts[0], DIR_LEGACY)) || !parts[2].to_lowercase().ends_with(".zip") {
        return;
    }
    let (n2, peer, file, name) = (node.clone(), peer.to_string(), file.to_path_buf(), parts[1].to_string());
    tokio::task::spawn_blocking(move || {
        let _g = n2.builds.lock.lock();
        // (3.22.1) чья это запись при отказе: общая сборка — под тем же «соседом», с GUID и номером (отказ видят
        // автор и соавторы в доставке), иначе — под отправителем
        let mut ctx = json!({ "peer": peer, "name": name, "by": peer });
        let r = install(&n2, &peer, &file, &name, &mut ctx);
        match r {
            Ok(e) => {
                n2.info(&format!("[сборки] «{}» от {}: распакована ({}, {} файлов) → {}", name, peer, if e["mode"] == "clean" { "чистая" } else { "патч" }, e["files"], e["target"].as_str().unwrap_or("")));
                remember(&n2, e);
                // архив своё дело сделал
                let _ = std::fs::remove_file(&file);
                if let Some(d) = file.parent() {
                    crate::mailbox::remove_empty_dirs(&n2.mailbox.in_root(&n2), d);
                }
            }
            Err(e) => {
                n2.info(&format!("[сборки] «{}» от {}: не распаковалась — {} (архив оставлен: {})", name, peer, e, file.display()));
                let mut rec = json!({ "dir": "in", "at": time::format(&time::now()), "error": e, "zip": file.display().to_string() });
                for (k, v) in ctx.as_object().cloned().unwrap_or_default() {
                    rec[k] = v;
                }
                remember(&n2, rec);
            }
        }
    });
}

/// Папки совпадают (на Windows — без учёта регистра).
fn same_dir(a: &Path, b: &Path) -> bool {
    let k = |p: &Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
    k(a) == k(b)
}

fn install(node: &Arc<Node>, peer: &str, zipf: &Path, name: &str, ctx: &mut Value) -> Result<Value, String> {
    let name = clean_name(name)?;
    let f = std::fs::File::open(zipf).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f)).map_err(|e| format!("не zip: {}", e))?;
    let meta: Value = match z.by_name(META) {
        Ok(mut e) => {
            let mut s = String::new();
            let _ = e.read_to_string(&mut s);
            serde_json::from_str(&s).unwrap_or(Value::Null)
        }
        Err(_) => Value::Null,
    };
    // (3.22) общая сборка: где она лежит (у всех соавторов — одна папка) и можно ли этому отправителю её патчить
    let sender = peer;
    let ids = ids_of(node, sender);
    if let Some(n) = meta["n"].as_u64() {
        ctx["n"] = json!(n);
    }
    let gid = meta["gid"].as_str().map(|g| g.to_lowercase()).filter(|g| is_gid(g)).unwrap_or_default();
    let mut reg = registry(node, &gid);
    // (3.22.1) номер общей сборки — только число в разумных пределах (иначе защита «старше не ставлю» не работала)
    let num = || meta["n"].as_u64().filter(|n| (1..=MAX_N).contains(n)).ok_or_else(|| "номер версии в архиве испорчен — сборку не ставлю".to_string());
    let mut got = 0;
    let (peer, name) = if gid.is_empty() {
        (sender.to_string(), name)
    } else if !reg.is_null() {
        let (owner, authors) = (reg["owner"].as_str().unwrap_or("").to_string(), names_of(&reg["authors"]));
        let rname = reg["name"].as_str().unwrap_or(&name).to_string();
        if !may_patch_any(&owner, &authors, &ids) {
            return Err(format!("{} не соавтор общей сборки «{}» — патч не принят (соавторов назначает автор, {})", sender, rname, owner));
        }
        let rpeer = reg["peer"].as_str().filter(|p| !p.is_empty()).unwrap_or(sender).to_string();
        *ctx = json!({ "peer": rpeer, "name": rname, "by": sender, "gid": gid, "n": meta["n"].as_u64() });
        got = num()?;
        let has = reg["n"].as_u64().unwrap_or(0);
        if got < has {
            return Err(format!("пришёл №{}, а здесь уже №{} — более старую версию не ставлю", got, has));
        }
        (rpeer, rname)
    } else {
        // (3.22.1) первую версию общей сборки ставит только автор: иначе любой сосед мог назваться чужим автором
        // (сборка вставала в папку того соседа и числилась «от него») или записать автором себя
        // Исключение — автор сам получает версию соавтора: его заготовка с этим GUID и решает (автор, соавторы, получатели)
        let mine = me(node);
        let own = presets(node).into_iter().find(|p| p["gid"] == gid.as_str() && eq_ic(p["owner"].as_str().unwrap_or(""), &mine));
        let owner = meta["owner"].as_str().map(str::trim).filter(|o| !o.is_empty()).unwrap_or(sender).to_string();
        if let Some(p) = own {
            if !may_patch_any(&mine, &names_of(&p["authors"]), &ids) {
                return Err(format!("{} не соавтор вашей сборки «{}» — версия не принята", sender, p["name"].as_str().unwrap_or(&name)));
            }
            *ctx = json!({ "peer": mine, "name": name, "by": sender, "gid": gid, "n": meta["n"].as_u64() });
            got = num()?;
            reg = json!({ "owner": mine, "authors": p["authors"], "peers": p["peers"], "ignore": p["ignore"] });
            (mine, name)
        } else {
            if !ids.iter().any(|w| eq_ic(w, &owner)) {
                return Err(format!("первую версию общей сборки «{}» ставит её автор ({}) — попросите его отправить; патчи соавтора {} примутся после этого", name, owner, sender));
            }
            *ctx = json!({ "peer": sender, "name": name, "by": sender, "gid": gid, "n": meta["n"].as_u64() });
            got = num()?;
            (sender.to_string(), name)
        }
    };
    let peer = peer.as_str();
    let mode = if meta["mode"] == "clean" || meta["first"] == true { "clean" } else { "patch" };
    let target = target_dir(node, peer, &name);
    // (3.22.1) в этой папке лежит чужая общая сборка, а отправитель — не её автор и не соавтор: не трогать
    // (раньше обычная сборка с тем же именем или сборка с другим GUID обходила права и могла убрать её в корзину)
    for (og, r) in read(&gids_file(node)).as_object().cloned().unwrap_or_default() {
        if og == gid {
            continue;
        }
        let (op, on) = (r["peer"].as_str().unwrap_or(""), r["name"].as_str().unwrap_or(""));
        if op.is_empty() || on.is_empty() || !same_dir(&target_dir(node, op, on), &target) {
            continue;
        }
        let (owner, authors) = (r["owner"].as_str().unwrap_or(""), names_of(&r["authors"]));
        if !may_patch_any(owner, &authors, &ids) {
            return Err(format!("в папке {} лежит общая сборка «{}» (автор {}), а {} не её соавтор — не ставлю. Укажите для «{}» от {} другую папку", target.display(), on, owner, sender, name, sender));
        }
    }
    // чистая: прежняя папка — в корзину (вернуть можно оттуда)
    let mut old = String::new();
    if mode == "clean" && target.is_dir() && std::fs::read_dir(&target).map(|mut d| d.next().is_some()).unwrap_or(false) {
        let to = node.folders.trash_dir().join("builds").join(format!("{}--{}", crate::mobile::folder_name(&name), chrono::Local::now().format("%Y%m%d-%H%M%S")));
        if let Some(d) = to.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&target, &to).map_err(|e| format!("прежнюю папку сборки не убрать в корзину ({}): {}", target.display(), e))?;
        old = to.display().to_string();
    }
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let ig = node.cfg.read().global_ignore();
    let (mut files, mut bytes) = (0i64, 0i64);
    let mut buf = vec![0u8; 1 << 20];
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| e.to_string())?;
        if e.is_dir() || e.name() == META {
            continue;
        }
        // защита: только внутрь папки сборки, без «..» и абсолютных путей
        let Some(rel) = e.enclosed_name() else { continue };
        let rel_s = rel.to_string_lossy().replace('\\', "/");
        let Some(norm) = path::norm(&rel_s) else { continue };
        if ig.which(&norm).is_some() {
            continue;
        }
        let Ok(dest) = path::to_local(&target, &norm) else { continue };
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = dest.with_extension("most-part");
        {
            let mut out = std::fs::File::create(&tmp).map_err(|e| format!("{}: {}", norm, e))?;
            loop {
                let n = e.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                bytes += n as i64;
            }
        }
        bridge_core::files::move_replace(&tmp, &dest).map_err(|e| format!("{}: {} (файл занят? закройте программу)", norm, e))?;
        files += 1;
    }
    let exe = clean_exe(meta["exe"].as_str().unwrap_or("")).unwrap_or_default();
    remember_exe(node, peer, &name, &exe);
    let note: String = meta["note"].as_str().unwrap_or("").chars().take(2000).collect();
    // (3.23) сведения о программе: из META (отправитель 3.23+, null — файла не было), у старого отправителя —
    // version.json в распакованной папке
    let app = if meta.get("app").is_some() { clean_app(&meta["app"]) } else { app_info(&target) };
    if !gid.is_empty() {
        // реестр: автор и соавторы, получатели и игнор меняются только по слову автора
        let manifest: BTreeMap<String, String> = serde_json::from_value(meta["manifest"].clone()).unwrap_or_default();
        let owner = reg["owner"].as_str().or(meta["owner"].as_str()).map(str::trim).filter(|o| !o.is_empty()).unwrap_or(sender).to_string();
        let by_owner = reg.is_null() || ids.iter().any(|w| eq_ic(&owner, w));
        let clean = |k: &str, v: &Value| -> Value {
            match k {
                "ignore" => json!(lines_of(v).into_iter().take(500).map(|l| l.chars().take(300).collect::<String>()).collect::<Vec<_>>()),
                _ => json!(names_of(v)),
            }
        };
        let pick = |k: &str| if by_owner || reg[k].is_null() { clean(k, &meta[k]) } else { reg[k].clone() };
        let digest = meta["digest"].as_str().filter(|d| is_digest(d)).map(|d| d.to_lowercase()).unwrap_or_else(|| digest_of(&manifest));
        set_registry(node, &gid, json!({
            "peer": peer, "name": name, "owner": owner, "authors": pick("authors"), "peers": pick("peers"), "ignore": pick("ignore"),
            "n": got, "digest": digest, "at": time::format(&time::now()), "by": sender, "app": app,
        }));
        if !manifest.is_empty() {
            save_map(node, &manifest);
        }
    }
    Ok(json!({ "dir": "in", "peer": peer, "name": name, "mode": mode, "at": time::format(&time::now()), "files": files, "bytes": bytes, "target": target.display().to_string(), "old": old, "sentAt": meta["at"], "exe": exe, "note": note, "n": meta["n"].as_u64(), "gid": gid, "by": sender, "app": app }))
}
