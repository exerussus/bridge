// Подставной Codex для тестов агента: ведёт себя как `codex exec --json … [resume <id>] -`.
// Картинку «генерирует» в $CODEX_HOME/generated_images/<нить>/, как настоящий; «сохрани» в поручении — копирует в первую --add-dir.
use serde_json::{json, Value};
use std::io::{Read, Write};

// PNG 3×2 (только заголовок и IHDR важны для моста)
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\x00\x00\x00\x03\x00\x00\x00\x02\x08\x06\x00\x00\x00\x00\x00\x00\x00";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a == "--version").unwrap_or(false) {
        println!("codex-cli 0.0.0-test");
        return;
    }
    if args.first().map(|a| a == "login").unwrap_or(false) {
        println!("Logged in using ChatGPT");
        return;
    }
    let mut prompt = String::new();
    if args.last().map(|a| a == "-").unwrap_or(false) {
        let _ = std::io::stdin().read_to_string(&mut prompt);
    } else {
        eprintln!("No prompt provided. Either specify one as an argument or pipe the prompt into stdin.");
        std::process::exit(1);
    }
    let task = prompt.rsplit("\n\n").next().unwrap_or("").trim().to_string();
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let cont = opt("resume");
    let thread = cont.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let add_dir = opt("--add-dir");
    let images: Vec<String> = args.windows(2).filter(|w| w[0] == "-i").map(|w| w[1].clone()).collect();
    let out = |v: Value| {
        let mut so = std::io::stdout().lock();
        let _ = writeln!(so, "{}", v);
        let _ = so.flush();
    };
    out(json!({"type": "thread.started", "thread_id": thread}));
    out(json!({"type": "turn.started"}));
    let first = format!("{}{}{}", if cont.is_some() { "Продолжаю: " } else { "Принял: " }, task, if images.is_empty() { String::new() } else { format!(" (картинок на входе: {})", images.len()) });
    out(json!({"type": "item.completed", "item": {"id": "item_0", "type": "agent_message", "text": first}}));
    out(json!({"type": "item.started", "item": {"id": "item_1", "type": "command_execution", "command": "\"C:\\\\Windows\\\\System32\\\\WindowsPowerShell\\\\v1.0\\\\powershell.exe\" -Command \"Get-ChildItem\"", "status": "in_progress"}}));
    out(json!({"type": "item.completed", "item": {"id": "item_1", "type": "command_execution", "command": "x", "aggregated_output": "", "exit_code": 0, "status": "completed"}}));
    if task.contains("долго") {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
    let mut last = format!("Готово: {} [нить {}]", task, std::env::var("BRIDGE_AGENT_THREAD").unwrap_or_default());
    if task.contains("картинк") {
        let home = std::path::PathBuf::from(std::env::var("CODEX_HOME").expect("CODEX_HOME"));
        let gen = home.join("generated_images").join(&thread);
        let _ = std::fs::create_dir_all(&gen);
        let name = format!("exec-{}.png", uuid::Uuid::new_v4());
        let mut png = PNG.to_vec();
        png.extend_from_slice(task.as_bytes()); // у каждой картинки своё содержимое
        std::fs::write(gen.join(&name), &png).unwrap();
        if task.contains("сохрани") {
            let dir = std::path::PathBuf::from(add_dir.expect("--add-dir"));
            let _ = std::fs::create_dir_all(&dir);
            let dst = dir.join("saved.png");
            std::fs::write(&dst, &png).unwrap();
            last.push_str(&format!("\n\nIMAGE: {}", dst.display()));
        }
    }
    if task.contains("ошибка") {
        out(json!({"type": "turn.failed", "error": {"message": "не вышло"}}));
        return;
    }
    out(json!({"type": "item.completed", "item": {"id": "item_2", "type": "agent_message", "text": last}}));
    out(json!({"type": "turn.completed", "usage": {"input_tokens": 1000, "cached_input_tokens": 400, "output_tokens": 50, "reasoning_output_tokens": 5}}));
}
