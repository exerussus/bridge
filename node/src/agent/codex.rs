//! (3.24) Исполнитель Codex: `codex exec --json` вместо `claude -p`.
//!
//! Поручение — через stdin (`-`): флаг `-i` жадный и съедает промпт, если тот идёт аргументом.
//! Продолжение нити — `codex exec … resume <thread_id> -`. Картинок в потоке `--json` нет: после хода мост сверяет
//! папку генерации Codex (`.codex/generated_images/<thread_id>/`) со строками `IMAGE:` и дозабирает то, что Codex не сохранил.

use super::*;
use sha2::{Digest, Sha256};
use std::time::SystemTime;

/// Где codex: настройка, PATH, официальный установщик, npm.
pub fn find_codex(node: &Node) -> Option<PathBuf> {
    let set = node.cfg.read().agent.codex.clone();
    if !set.trim().is_empty() {
        let p = PathBuf::from(set.trim());
        return p.is_file().then_some(p);
    }
    let names: &[&str] = if cfg!(windows) { &["codex.exe", "codex.cmd"] } else { &["codex"] };
    if let Some(path) = std::env::var_os("PATH") {
        for n in names {
            for d in std::env::split_paths(&path) {
                let f = d.join(n);
                if f.is_file() {
                    return Some(f);
                }
            }
        }
    }
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(l) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        cands.push(l.join("Programs").join("OpenAI").join("Codex").join("bin").join("codex.exe"));
    }
    if let Some(a) = std::env::var_os("APPDATA").map(PathBuf::from) {
        cands.push(a.join("npm").join("codex.cmd"));
    }
    if let Some(h) = std::env::var_os("HOME").map(PathBuf::from) {
        cands.push(h.join(".local").join("bin").join("codex"));
    }
    cands.into_iter().find(|f| f.is_file())
}

/// Папка Codex: CODEX_HOME или ~/.codex.
fn codex_home() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("CODEX_HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(h));
    }
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(|h| PathBuf::from(h).join(".codex"))
}

/// Проверка для панели: найден ли codex, версия, вошёл ли.
pub async fn codex_check(node: &Node) -> Value {
    let Some(exe) = find_codex(node) else {
        return json!({ "found": false, "error": "не найден Codex. Установите: PowerShell → irm https://chatgpt.com/codex/install.ps1 | iex (или npm i -g @openai/codex), затем один раз codex login" });
    };
    let run = |args: &'static [&'static str]| {
        let exe = exe.clone();
        async move {
            let mut cmd = tokio::process::Command::new(&exe);
            cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
            #[cfg(windows)]
            {
                cmd.creation_flags(0x0800_0000);
            }
            match tokio::time::timeout(Duration::from_secs(15), cmd.output()).await {
                Ok(Ok(o)) => {
                    let mut t = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if t.is_empty() {
                        t = String::from_utf8_lossy(&o.stderr).trim().to_string();
                    }
                    Ok(t.lines().next().unwrap_or("").to_string())
                }
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("нет ответа за 15 с".to_string()),
            }
        }
    };
    let version = run(&["--version"]).await;
    let login = run(&["login", "status"]).await;
    json!({
        "found": true, "path": exe.display().to_string(),
        "version": version.as_ref().ok(), "login": login.as_ref().ok(),
        "error": version.err().or(login.err()),
    })
}

/// Что накопилось за один запуск codex.
#[derive(Default)]
struct Turn {
    t0: Option<SystemTime>,
    messages: Vec<String>,
    completed: bool,
    failed: Option<String>,
    usage: Tokens,
}

enum Outcome {
    Done,
    /// codex не принял параметры (старая версия, `resume` с другими флагами) — попробовать проще
    BadArgs,
    /// codex не увидел поручение в stdin — передать аргументом
    NoPrompt,
}

pub(super) async fn run_codex(node: &Arc<Node>, id: &str, cancel: tokio_util::sync::CancellationToken) {
    let Some(t) = node.agent.get(id) else { return };
    let task = t.events.iter().rev().find(|e| e.kind == "prompt").map(|e| e.text.clone()).unwrap_or_default();
    let Some(exe) = find_codex(node) else {
        fail(node, id, "не найден Codex. Установите: PowerShell → irm https://chatgpt.com/codex/install.ps1 | iex (или npm i -g @openai/codex), затем один раз codex login. Или укажите путь в «Агенты» → «Путь к codex».");
        return;
    };
    let inputs = match input_paths(node, &t).await {
        Ok(l) => l,
        Err(why) => {
            fail(node, id, &why);
            return;
        }
    };
    let cfg = node.cfg.read().agent.clone();
    let ex = exchange_path(node, &t);
    if let Some(e) = &ex {
        let _ = std::fs::create_dir_all(e);
    }
    let mut prompt = context_prompt(node, &t);
    if !inputs.is_empty() {
        prompt.push_str(&format!("\n\nВходные картинки (приложены к поручению по порядку):\n{}", inputs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n")));
    }
    prompt.push_str("\n\n");
    prompt.push_str(&task);
    let mut dirs: Vec<PathBuf> = ex.iter().cloned().collect();
    for d in &cfg.codex_dirs {
        if !d.trim().is_empty() {
            dirs.push(node.resolve(d.trim()));
        }
    }
    // 0 — полный набор флагов; 1 — только необходимые (resume старых версий); 2 — поручение аргументом
    let mut level = 0;
    loop {
        let (args, via_stdin) = build_args(&cfg, &t.session, &dirs, &inputs, level, &prompt);
        node.agent.update(id, |t| {
            t.push(
                "info",
                format!("Codex запущен (песочница {}, сеть {}{})", cfg.codex_sandbox(), if cfg.codex_network { "вкл" } else { "выкл" }, if t.session.is_empty() { String::new() } else { ", продолжение нити".into() }),
            )
        });
        match run_once_codex(node, id, &exe, &args, if via_stdin { Some(prompt.as_str()) } else { None }, &cfg, cancel.clone()).await {
            Outcome::Done => return,
            Outcome::BadArgs if level == 0 => level = 1,
            Outcome::NoPrompt if level < 2 => level = 2,
            _ => {
                fail(node, id, "Codex не принял параметры запуска — обновите его (codex update или заново установщиком)");
                return;
            }
        }
    }
}

fn build_args(cfg: &bridge_core::config::AgentConfig, session: &str, dirs: &[PathBuf], inputs: &[PathBuf], level: u8, prompt: &str) -> (Vec<String>, bool) {
    let mut a: Vec<String> = vec!["exec".into(), "--json".into(), "--skip-git-repo-check".into()];
    if level == 0 {
        a.push("-s".into());
        a.push(cfg.codex_sandbox().into());
        for d in dirs {
            a.push("--add-dir".into());
            a.push(d.display().to_string());
        }
        for i in inputs {
            a.push("-i".into());
            a.push(i.display().to_string());
        }
    } else {
        // флаги, которые принимает и `resume`: песочница и папки — через настройки Codex
        a.push("-c".into());
        a.push(format!("sandbox_mode=\"{}\"", cfg.codex_sandbox()));
        if !dirs.is_empty() {
            let list: Vec<String> = dirs.iter().map(|d| format!("'{}'", d.display())).collect();
            a.push("-c".into());
            a.push(format!("sandbox_workspace_write.writable_roots=[{}]", list.join(",")));
        }
    }
    if cfg.codex_network {
        a.push("-c".into());
        a.push("sandbox_workspace_write.network_access=true".into());
    }
    if !cfg.codex_model.trim().is_empty() {
        a.push("-m".into());
        a.push(cfg.codex_model.trim().into());
    }
    a.extend(cfg.extra_args.iter().cloned());
    if !session.is_empty() {
        a.push("resume".into());
        a.push(session.into());
    }
    if level >= 2 {
        a.push("--".into());
        a.push(prompt.into());
        (a, false)
    } else {
        a.push("-".into());
        (a, true)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_once_codex(node: &Arc<Node>, id: &str, exe: &Path, args: &[String], stdin: Option<&str>, cfg: &bridge_core::config::AgentConfig, cancel: tokio_util::sync::CancellationToken) -> Outcome {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.env("BRIDGE_AGENT_THREAD", id);
    cmd.args(args).current_dir(work_dir(node)).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            fail(node, id, &format!("не удалось запустить {}: {}", exe.display(), e));
            return Outcome::Done;
        }
    };
    if let Some(p) = node.agent.procs.lock().get_mut(id) {
        p.pid = child.id();
    }
    if let (Some(mut si), Some(p)) = (child.stdin.take(), stdin) {
        let p = p.to_string();
        tokio::spawn(async move {
            let _ = si.write_all(p.as_bytes()).await;
            let _ = si.shutdown().await;
        });
    }
    let err_buf = Arc::new(Mutex::new(String::new()));
    if let Some(mut se) = child.stderr.take() {
        let eb = err_buf.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            while let Ok(n) = se.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut g = eb.lock();
                g.push_str(&String::from_utf8_lossy(&buf[..n]));
                if g.len() > 8000 {
                    let cut = g.len() - 8000;
                    let cut = (cut..g.len()).find(|i| g.is_char_boundary(*i)).unwrap_or(g.len());
                    g.drain(..cut);
                }
            }
        });
    }
    let raw_path = node.agent.dir.join(format!("{}.codex.jsonl", safe(id)));
    let _ = std::fs::create_dir_all(&node.agent.dir);
    let mut raw = std::fs::OpenOptions::new().create(true).append(true).open(&raw_path).ok();
    let stdout = child.stdout.take().expect("stdout");
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let deadline = tokio::time::Instant::now() + Duration::from_secs((cfg.max_minutes.max(1) * 60) as u64);
    let mut turn = Turn { t0: Some(SystemTime::now() - Duration::from_secs(2)), ..Default::default() };
    let mut timed_out = false;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => { if let Some(pid) = child.id() { kill_tree(pid); } let _ = child.start_kill(); break; }
            _ = tokio::time::sleep_until(deadline) => { timed_out = true; if let Some(pid) = child.id() { kill_tree(pid); } let _ = child.start_kill(); break; }
            l = lines.next_line() => match l {
                Ok(Some(line)) => {
                    if let Some(f) = raw.as_mut() {
                        use std::io::Write;
                        let _ = writeln!(f, "{}", line);
                    }
                    handle_line(node, id, &line, &mut turn);
                }
                _ => break,
            }
        }
    }
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.ok().and_then(|r| r.ok());
    if cancel.is_cancelled() {
        return Outcome::Done;
    }
    let err_text = err_buf.lock().clone();
    if timed_out {
        fail(node, id, &format!("превышено время ({} мин) — остановлено; продолжите поручением в эту же нить", cfg.max_minutes));
        return Outcome::Done;
    }
    if turn.completed || turn.failed.is_some() {
        finish(node, id, turn).await;
        return Outcome::Done;
    }
    let low = err_text.to_lowercase();
    if low.contains("no prompt provided") {
        return Outcome::NoPrompt;
    }
    if low.contains("unexpected argument") || low.contains("unrecognized") || low.contains("unknown option") {
        node.agent.update(id, |t| t.push("info", format!("Codex: {}", err_text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim())));
        return Outcome::BadArgs;
    }
    let tail: String = err_text.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    let hint = if low.contains("login") || low.contains("auth") || low.contains("401") { " — войдите в Codex на этом ПК: в консоли codex login" } else { "" };
    fail(
        node,
        id,
        &format!("Codex завершился без итога (код {}){}{}", status.and_then(|s| s.code()).map(|c| c.to_string()).unwrap_or("?".into()), hint, if tail.is_empty() { String::new() } else { format!(":\n{}", tail) }),
    );
    Outcome::Done
}

/// Строка `--json`. Итог хода собирается в `turn`, статус ставит `finish` — после картинок.
fn handle_line(node: &Arc<Node>, id: &str, line: &str, turn: &mut Turn) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let ty = v.get("type").and_then(|s| s.as_str()).unwrap_or("");
    match ty {
        "thread.started" => {
            if let Some(s) = v.get("thread_id").and_then(|s| s.as_str()) {
                let s = s.to_string();
                node.agent.update(id, |t| t.session = s);
            }
        }
        "item.started" | "item.updated" | "item.completed" => {
            let it = v.get("item").cloned().unwrap_or(Value::Null);
            let ity = it.get("type").and_then(|s| s.as_str()).unwrap_or("");
            let done = ty == "item.completed";
            let st = |k: &str| it.get(k).and_then(|s| s.as_str()).unwrap_or("").to_string();
            match ity {
                "agent_message" if done => {
                    let txt = st("text").trim().to_string();
                    if !txt.is_empty() {
                        turn.messages.push(txt.clone());
                        node.agent.update(id, |t| {
                            t.push("text", txt.clone());
                            if let Some((p, w)) = phase_of(&txt) {
                                t.set_phase(&p, w);
                            }
                        });
                    }
                }
                "command_execution" if ty == "item.started" => {
                    let c = short_command(&st("command"));
                    node.agent.update(id, |t| {
                        if t.status == "waiting" {
                            t.status = "running".into();
                        }
                        t.push("tool", format!("Команда: {}", c));
                    });
                }
                "command_execution" if done => {
                    let code = it.get("exit_code").and_then(|x| x.as_i64()).unwrap_or(0);
                    if code != 0 || st("status") == "failed" {
                        let out: String = st("aggregated_output").chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
                        node.agent.update(id, |t| t.push("info", format!("команда: код {} — {}", code, out.trim())));
                    }
                }
                "file_change" if done => {
                    let paths: Vec<String> = it.get("changes").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|c| c.get("path").and_then(|p| p.as_str()).map(String::from)).collect()).unwrap_or_default();
                    if !paths.is_empty() {
                        node.agent.update(id, |t| t.push("tool", format!("Файлы: {}", paths.join(", "))));
                    }
                }
                "mcp_tool_call" if ty == "item.started" => {
                    let n = format!("{}.{}", st("server"), st("tool"));
                    node.agent.update(id, |t| t.push("tool", n));
                }
                "web_search" if ty == "item.started" => {
                    let q = st("query");
                    node.agent.update(id, |t| t.push("tool", format!("Поиск: {}", q)));
                }
                "todo_list" => {
                    let next = it.get("items").and_then(|a| a.as_array()).and_then(|a| a.iter().find(|x| !x.get("completed").and_then(|c| c.as_bool()).unwrap_or(false)).and_then(|x| x.get("text").and_then(|s| s.as_str()).map(String::from)));
                    if let Some(n) = next {
                        node.agent.update(id, |t| {
                            let n: String = n.chars().take(160).collect();
                            if t.phase != n {
                                t.phase = n;
                                t.phase_at = Some(time::now());
                            }
                        });
                    }
                }
                "error" if done => {
                    let m = st("message");
                    node.agent.update(id, |t| t.push("info", format!("Codex: {}", m)));
                }
                _ => {}
            }
        }
        "turn.completed" => {
            let u = v.get("usage").cloned().unwrap_or(Value::Null);
            let g = |k: &str| u.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
            turn.usage = Tokens { input: g("input_tokens"), cached: g("cached_input_tokens"), output: g("output_tokens") + g("reasoning_output_tokens") };
            turn.completed = true;
        }
        "turn.failed" => {
            turn.failed = Some(v.pointer("/error/message").and_then(|s| s.as_str()).unwrap_or("ход не удался").to_string());
        }
        "error" => {
            turn.failed = Some(v.get("message").and_then(|s| s.as_str()).unwrap_or("ошибка Codex").to_string());
        }
        _ => {}
    }
}

/// «Copy-Item …» вместо «"C:\…\powershell.exe" -Command "Copy-Item …"».
fn short_command(c: &str) -> String {
    let mut s = c.trim().to_string();
    for key in ["-Command ", "-command ", "/C ", "/c ", "-lc ", "-c "] {
        if let Some(i) = s.find(key) {
            let head = &s[..i];
            if head.to_lowercase().contains("powershell") || head.to_lowercase().contains("pwsh") || head.to_lowercase().contains("cmd") || head.contains("bash") || head.ends_with("sh\" ") || head.ends_with("sh ") {
                s = s[i + key.len()..].to_string();
                break;
            }
        }
    }
    let s = s.trim().trim_matches('"').trim().replace("\\\\", "\\");
    s.lines().next().unwrap_or("").chars().take(160).collect()
}

/// Конец хода: картинки, токены, статус, уведомление.
async fn finish(node: &Arc<Node>, id: &str, turn: Turn) {
    let images = if turn.completed {
        let n2 = node.clone();
        let id2 = id.to_string();
        let msgs = turn.messages.clone();
        let t0 = turn.t0;
        tokio::task::spawn_blocking(move || collect_images(&n2, &id2, &msgs, t0)).await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let is_err = turn.failed.is_some() || !turn.completed;
    let result = turn.messages.last().cloned().unwrap_or_default();
    node.agent.update(id, |t| {
        for (img, note) in &images {
            if let Some(n) = note {
                t.push("info", n.clone());
            }
            t.push("tool", format!("Картинка: {}", img.path));
            if !t.images.iter().any(|x| x.path == img.path) {
                t.images.push(img.clone());
            } else if let Some(x) = t.images.iter_mut().find(|x| x.path == img.path) {
                *x = img.clone();
            }
        }
        t.tokens.input += turn.usage.input;
        t.tokens.cached += turn.usage.cached;
        t.tokens.output += turn.usage.output;
        t.turns += 1;
        if is_err {
            t.status = "failed".into();
            t.error = turn.failed.clone().unwrap_or_else(|| "Codex: ход не завершён".into());
            t.push("error", t.error.clone());
        } else {
            t.status = "idle".into();
            t.result = result.clone();
            t.push("result", result.clone());
        }
        t.phase.clear();
    });
    let (title, tok) = node.agent.get(id).map(|t| (t.title, t.tokens.input + t.tokens.output)).unwrap_or_default();
    node.info(&format!("Агент: {} «{}» (Codex, {} ток.{})", if is_err { "не получилось" } else { "готово" }, title, tok, if images.is_empty() { String::new() } else { format!(", картинок: {}", images.len()) }));
    node.emit("agent", json!({ "thread": id, "role": "in", "status": if is_err { "failed" } else { "idle" }, "title": title }));
}

/// Картинки хода: строки `IMAGE:` + то, что Codex сгенерировал, но не сохранил (мост кладёт сам).
/// Путь в нити — относительно обменной папки, если файл в ней; иначе полный (виден только исполнителю).
fn collect_images(node: &Arc<Node>, id: &str, messages: &[String], t0: Option<SystemTime>) -> Vec<(ImageOut, Option<String>)> {
    let Some(t) = node.agent.get(id) else { return Vec::new() };
    let ex = exchange_path(node, &t);
    let cfg = node.cfg.read().agent.clone();
    let mut roots: Vec<PathBuf> = ex.iter().cloned().collect();
    roots.push(work_dir(node));
    for d in &cfg.codex_dirs {
        if !d.trim().is_empty() {
            roots.push(node.resolve(d.trim()));
        }
    }
    let roots_c: Vec<PathBuf> = roots.iter().filter_map(|r| std::fs::canonicalize(r).ok()).collect();
    let mut out: Vec<(ImageOut, Option<String>)> = Vec::new();
    let mut saved: Vec<(PathBuf, String)> = Vec::new(); // (файл, sha256)
    for m in messages {
        for l in m.lines() {
            let l = l.trim().trim_start_matches(['-', '*', ' ']);
            let Some(rest) = l.strip_prefix("IMAGE:").or_else(|| l.strip_prefix("Image:")) else { continue };
            let p = rest.trim().trim_matches(['`', '"', '\'', '<', '>']).trim();
            if p.is_empty() {
                continue;
            }
            let pb = PathBuf::from(p);
            let pb = if pb.is_absolute() { pb } else { work_dir(node).join(pb) };
            let Ok(pc) = std::fs::canonicalize(&pb) else { continue };
            if !pc.is_file() || saved.iter().any(|(x, _)| *x == pc) {
                continue;
            }
            if !roots_c.iter().any(|r| pc.starts_with(r)) {
                continue;
            }
            saved.push((pc.clone(), sha(&pc)));
        }
    }
    for (p, _) in &saved {
        out.push((describe(p, ex.as_deref()), None));
    }
    // картинки прошлых ходов этой нити — уже учтены, их сгенерированные оригиналы не дублировать
    let mut known: Vec<String> = saved.iter().map(|(_, h)| h.clone()).collect();
    for im in &t.images {
        let p = PathBuf::from(&im.path);
        let p = if p.is_absolute() { p } else if let Some(e) = &ex { e.join(&im.path) } else { continue };
        known.push(sha(&p));
    }
    // сгенерированное Codex за этот ход, которого нет среди сохранённого
    if let (Some(home), false) = (codex_home(), t.session.is_empty()) {
        let gen = home.join("generated_images").join(&t.session);
        if let Ok(rd) = std::fs::read_dir(&gen) {
            let mut files: Vec<(SystemTime, PathBuf)> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("png")).unwrap_or(false))
                .filter_map(|p| std::fs::metadata(&p).and_then(|m| m.modified()).ok().map(|m| (m, p)))
                .filter(|(m, _)| t0.map(|t0| *m >= t0).unwrap_or(true))
                .collect();
            files.sort();
            for (_, g) in files {
                let h = sha(&g);
                if known.contains(&h) {
                    continue;
                }
                known.push(h);
                let name = format!("codex-{}", g.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "image.png".into()));
                let (dir, note) = match &ex {
                    Some(e) => (e.join(safe(id)), None),
                    None => (work_dir(node).join("images").join(safe(id)), Some("обменная папка не настроена — картинка только на исполнителе".to_string())),
                };
                let _ = std::fs::create_dir_all(&dir);
                let dst = dir.join(&name);
                if std::fs::copy(&g, &dst).is_ok() {
                    let mut im = describe(&dst, ex.as_deref());
                    if im.size == 0 {
                        im.size = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
                    }
                    out.push((im, Some(note.unwrap_or_else(|| format!("Codex не сохранил картинку — мост положил её сам: {}", name)))));
                }
            }
        }
    }
    out
}

fn sha(p: &Path) -> String {
    std::fs::read(p).map(|b| hex::encode(Sha256::digest(&b))).unwrap_or_default()
}

/// Путь (относительно обменной папки, «/»), размер и w×h из заголовка PNG.
fn describe(p: &Path, ex: Option<&Path>) -> ImageOut {
    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let (w, h) = png_size(p).unwrap_or((0, 0));
    let rel = ex
        .and_then(|e| std::fs::canonicalize(e).ok())
        .and_then(|e| std::fs::canonicalize(p).ok().and_then(|pc| pc.strip_prefix(&e).ok().map(|r| r.to_string_lossy().replace('\\', "/"))));
    ImageOut { path: rel.unwrap_or_else(|| p.display().to_string()), size, w, h }
}

fn png_size(p: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut b = [0u8; 24];
    f.read_exact(&mut b).ok()?;
    if &b[..8] != b"\x89PNG\r\n\x1a\n" || &b[12..16] != b"IHDR" {
        return None;
    }
    Some((u32::from_be_bytes([b[16], b[17], b[18], b[19]]), u32::from_be_bytes([b[20], b[21], b[22], b[23]])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_command_strips_shell() {
        let c = "\"C:\\\\Windows\\\\System32\\\\WindowsPowerShell\\\\v1.0\\\\powershell.exe\" -Command \"Copy-Item -LiteralPath 'a.png' -Destination '.\\\\b.png' -Force\"";
        assert_eq!(short_command(c), "Copy-Item -LiteralPath 'a.png' -Destination '.\\b.png' -Force");
        assert_eq!(short_command("ls -la"), "ls -la");
    }

    #[test]
    fn args_new_and_resume() {
        let cfg = bridge_core::config::AgentConfig::default();
        let (a, stdin) = build_args(&cfg, "", &[PathBuf::from("X:/ex")], &[PathBuf::from("X:/ex/p.png")], 0, "p");
        assert!(stdin);
        assert_eq!(a.last().unwrap(), "-");
        assert!(a.windows(2).any(|w| w[0] == "-i" && w[1] == "X:/ex/p.png"));
        assert!(a.windows(2).any(|w| w[0] == "--add-dir"));
        let (a, _) = build_args(&cfg, "th1", &[], &[], 0, "p");
        let n = a.len();
        assert_eq!(&a[n - 3..], &["resume".to_string(), "th1".into(), "-".into()]);
        let (a, stdin) = build_args(&cfg, "th1", &[PathBuf::from("X:/ex")], &[], 2, "привет");
        assert!(!stdin);
        assert_eq!(a.last().unwrap(), "привет");
        assert!(a.iter().any(|x| x.starts_with("sandbox_workspace_write.writable_roots=")));
    }
}
