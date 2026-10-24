// Подставной Claude Code для тестов агента: ведёт себя как `claude -p --output-format stream-json --verbose`.
// Программа, а не скрипт, — чтобы тесты шли одинаково на Windows и Linux.
use serde_json::{json, Value};
use std::io::{Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut prompt = String::new();
    let _ = std::io::stdin().read_to_string(&mut prompt);
    let prompt = prompt.trim().to_string();
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let cont = opt("--resume").is_some();
    let session = opt("--resume").unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let system = opt("--append-system-prompt").unwrap_or_default();
    let out = |v: Value| {
        let mut so = std::io::stdout().lock();
        let _ = writeln!(so, "{}", v);
        let _ = so.flush();
    };
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    out(json!({"type": "system", "subtype": "init", "session_id": session, "cwd": cwd}));
    let first = format!("{}{}", if cont { "Продолжаю: " } else { "Принял: " }, prompt);
    out(json!({"type": "assistant", "message": {"content": [{"type": "text", "text": first}]}, "parent_tool_use_id": null, "session_id": session}));
    if prompt.contains("этапы") {
        // этапы и ожидание — как их пишет исполнитель по инструкции моста
        out(json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Этап: ищу информацию\nСмотрю, что есть."}]}, "parent_tool_use_id": null, "session_id": session}));
        std::thread::sleep(std::time::Duration::from_millis(900));
        out(json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "**Ожидание:** сборка проекта"}]}, "parent_tool_use_id": null, "session_id": session}));
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
    out(json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {"command": "run-tests --all"}}]}, "parent_tool_use_id": null, "session_id": session}));
    if prompt.contains("пауза") {
        std::thread::sleep(std::time::Duration::from_secs(4));
    }
    if prompt.contains("долго") {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
    // «Обменная папка (общая с «…», он видит её у себя): <путь>. Клади…»
    let marker = "он видит её у себя): ";
    if let Some(i) = system.find(marker) {
        let rest = &system[i + marker.len()..];
        if let Some(j) = rest.find(". Клади") {
            let dir = std::path::PathBuf::from(&rest[..j]);
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("report.txt"), format!("отчёт: {}", prompt));
        }
    }
    let err = prompt.contains("ошибка");
    out(json!({"type": "result", "subtype": if err { "error_during_execution" } else { "success" }, "is_error": err,
        "result": format!("{}{} [нить {}]", if err { "Не вышло: " } else { "Готово: " }, prompt, std::env::var("BRIDGE_AGENT_THREAD").unwrap_or_default()),
        "session_id": session, "total_cost_usd": 0.0123, "num_turns": 2}));
}
