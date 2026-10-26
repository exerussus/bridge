//! Командная строка узла: «bridge <команда>». Без команды — запустить узел.

mod mcp;
use bridge_core::config::{self, FileAcceptRule, NodeConfig, PeerConfig, ShareConfig};
use bridge_node::{Node, StartOptions};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

const HELP: &str = "Exerussus.Bridge — узел моста (файлы, общие папки, записи, команды, экран и голос)

Настройка и запуск:
  bridge [run] [--no-ui]              запустить узел; панель откроется в браузере
  bridge setup                        вопросы первого запуска → bridge.json
  bridge init [имя-узла]              создать bridge.json (шаблон)
  bridge check                        проверить bridge.json
  bridge ui                           открыть панель
  bridge mcp                          инструменты моста для Claude (MCP по stdin/stdout): поручать агентам на других ПК
  bridge pair                         новый ключ пары (для ручной настройки; проще — peer invite)

Соседи (имя сосед сообщает сам — нужны только адрес и ключ):
  bridge peer invite [псевдоним]               создать ключ; выдаст строку «адрес ключ» для того ПК
  bridge peer connect <адрес[:порт]> <ключ> [--title псевдоним]   подключить (или строкой «адрес ключ» в кавычках)
  bridge peer rename <имя> <новое>             переименовать
  bridge peer remove <имя>                     удалить (файлы и история остаются)
  bridge doctor [имя]                          почему нет синка: проверка по шагам с подсказками
  bridge status | peers | sync | folders | trash

Файлы:
  bridge send <сосед> <файл> [путь-у-соседа]   отправить файл
  bridge app new <id> [папка]                   заготовка приложения (веб-интерфейс + SDK)
  bridge app check <папка>                      проверить app.json
  bridge app pack <папка> [--out файл]          упаковать приложение в .most-app
  bridge ignore [add|remove|set|test] …         игнор (--folder id | --mount сосед/id)

Записи:
  bridge records [--type t] [--status s] [--assignee узел] [--q текст] [--share id]
  bridge show <id> [--history] [--at версия]
  bridge task new --to <узел> --title \"…\" [--body \"…\"] [--id Z-001]
  bridge task accept|start|review|done|reject|cancel|reopen <id> [--note \"…\"]
  bridge task return <id> --note \"что доработать\"
  bridge delete <id>   |   bridge restore <id>     удалить запись у всех участников / вернуть
  bridge comment <id> \"текст\"   |   bridge ack <id> accepted|rejected|seen [\"текст\"]
  bridge set <id> поле=значение …   |   bridge attach <id> <путь> [--title \"…\"]
  bridge dto <схема@версия> <файл.json> [--id id]
  bridge command <узел> <исполнитель> [параметр=значение …] [--title \"…\"]
  bridge approve <id>   |   bridge reject <id> [\"причина\"]
  bridge do <файл.json | ->   |   bridge wait [--after вектор] [--timeout с]

Любая функция панели — через API:  bridge api GET|POST <путь> [json | @файл | -]   (перечень: bridge api GET api)

Общие ключи: --config <bridge.json>, --share <id>, --actor \"кто\", --json";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("bridge").build().expect("runtime");
    let code = rt.block_on(run_cli(args));
    std::process::exit(code);
}

struct Opts {
    pos: Vec<String>,
    opt: HashMap<String, String>,
}

fn parse(args: Vec<String>) -> Opts {
    let mut pos = Vec::new();
    let mut opt = HashMap::new();
    let flags = ["json", "no-ui", "history", "tray"];
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some(k) = a.strip_prefix("--") {
            if flags.contains(&k) || i + 1 >= args.len() {
                opt.insert(k.to_string(), "1".into());
            } else {
                opt.insert(k.to_string(), args[i + 1].clone());
                i += 1;
            }
        } else {
            pos.push(a.clone());
        }
        i += 1;
    }
    Opts { pos, opt }
}

fn config_path(o: &Opts) -> PathBuf {
    if let Some(c) = o.opt.get("config") {
        return PathBuf::from(c);
    }
    let here = std::env::current_dir().unwrap_or_default().join("bridge.json");
    if here.exists() {
        return here;
    }
    // рядом с программой
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("bridge.json"))).filter(|p| p.exists()).unwrap_or(here)
}

async fn run_cli(args: Vec<String>) -> i32 {
    let o = parse(args);
    let cmd = if o.opt.contains_key("help") { "help".to_string() } else { o.pos.first().cloned().unwrap_or_else(|| "run".into()) };
    let res: Result<i32, String> = match cmd.as_str() {
        "help" | "-h" | "--help" | "/?" => {
            println!("{}", HELP);
            Ok(0)
        }
        "version" => {
            println!("Exerussus.Bridge {}, протокол v{}", bridge_core::APP_VERSION, bridge_core::PROTOCOL);
            Ok(0)
        }
        "pair" => {
            println!("{}", NodeConfig::new_secret());
            eprintln!("Это ключ пары. Впишите его в peers.<сосед>.secret на ОБОИХ узлах — или проще: bridge peer invite.");
            Ok(0)
        }
        "init" => init(&o),
        "setup" => setup(&config_path(&o)).map(|_| 0),
        "check" => NodeConfig::load(&config_path(&o)).map(|_| {
            println!("Настройки в порядке: {}", config_path(&o).display());
            0
        }),
        "run" => run(&o).await,
        "app" => app_cli(&o),
        "mcp" => mcp::serve(&o).await,
        "ui" | "panel" | "панель" => NodeConfig::load(&config_path(&o)).map(|cfg| {
            let url = cfg.listen.iter().find(|l| l.contains("127.0.0.1") || l.contains("localhost")).cloned().unwrap_or(cfg.listen[0].clone());
            let _ = bridge_node::http_local::open_url(&format!("{}/", url.trim_end_matches('/')));
            0
        }),
        _ => client(&cmd, &o).await,
    };
    match res {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Ошибка: {}", e);
            1
        }
    }
}

fn machine_name() -> String {
    let raw = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    let s: String = raw.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.trim_matches('-').to_string();
    if config::is_name(&s) {
        s
    } else {
        "my-pc".into()
    }
}

fn init(o: &Opts) -> Result<i32, String> {
    let file = config_path(o);
    if file.exists() {
        return Err(format!("Уже есть: {}", file.display()));
    }
    let name = o.pos.get(1).cloned().unwrap_or_else(machine_name);
    let cfg = NodeConfig { node: name, ..Default::default() };
    std::fs::write(&file, bridge_core::to_json(&cfg)).map_err(|e| e.to_string())?;
    println!("Создан {}\nДальше: впишите свой адрес (локальная сеть или Radmin VPN) в listen, затем bridge run.\nСоседа подключите в панели («✉ Пригласить» / «＋ Подключить») или командами bridge peer invite / bridge peer connect.", file.display());
    Ok(0)
}

/// Настройки нового узла: сосед — по адресу и ключу (имя он сообщит сам).
pub fn make_config(node: &str, my_ip: &str, port: u16, peer_address: Option<&str>, secret: Option<&str>, share: Option<(&str, &str, Vec<String>)>) -> NodeConfig {
    let mut cfg = NodeConfig { node: node.into(), listen: vec![format!("http://127.0.0.1:{}", port)], ..Default::default() };
    if !my_ip.is_empty() {
        cfg.listen.push(format!("http://{}:{}", my_ip, port));
    }
    let mut peer_name = None;
    if let Some(s) = secret.filter(|s| !s.is_empty()) {
        let mut p = PeerConfig { name: PeerConfig::temp_name(s), pending: true, secret: s.into(), ..Default::default() };
        if let Some(Ok((ip, pp))) = peer_address.map(|a| config::parse_address(a, port)) {
            p.url = format!("http://{}:{}", ip, pp);
            p.ips = vec![ip];
        }
        peer_name = Some(p.name.clone());
        cfg.peers.push(p);
    }
    if let Some((id, folder, patterns)) = share {
        cfg.shares.push(ShareConfig {
            id: id.into(),
            folder: folder.into(),
            peers: peer_name.iter().cloned().collect(),
            accept: peer_name.iter().flat_map(|n| patterns.iter().map(move |x| FileAcceptRule { pattern: x.replace('\\', "/"), from: vec![n.clone()] })).collect(),
            ..Default::default()
        });
    }
    cfg
}

fn ask(q: &str, def: Option<&str>, check: impl Fn(&str) -> Option<String>) -> String {
    let stdin = std::io::stdin();
    loop {
        print!("{}{}: ", q, def.map(|d| format!(" [{}]", d)).unwrap_or_default());
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        if stdin.lock().read_line(&mut s).unwrap_or(0) == 0 {
            return def.unwrap_or("").to_string();
        }
        let mut s = s.trim().to_string();
        if s.is_empty() {
            if let Some(d) = def {
                s = d.into();
            }
        }
        let why = if s.is_empty() { Some("нужен ответ".into()) } else { check(&s) };
        match why {
            None => return s,
            Some(w) => println!("  ✗ {}", w),
        }
    }
}

/// Первый запуск двойным щелчком: вопросы в окне → bridge.json.
fn setup(file: &Path) -> Result<(), String> {
    println!("Exerussus.Bridge — настройка узла. Enter — согласиться с вариантом в [скобках].\n");
    let node = ask("1. Имя этого ПК в мосте (латиницей)", Some(&machine_name()), |s| (!config::is_name(s)).then(|| "только латиница, цифры, «-», «_»".into()));
    let ips = bridge_node::link::local_ips();
    if !ips.is_empty() {
        println!("   Адреса этого ПК: {}", ips.join(", "));
    }
    let my_ip = ask("2. Свой адрес для соседей", ips.first().map(|s| s.as_str()), |s| s.parse::<std::net::Ipv4Addr>().err().map(|_| "нужен IPv4-адрес, например 26.12.34.56".into()));
    let port: u16 = ask("3. Порт", Some("5390"), |s| (s.parse::<u16>().map(|p| p <= 1024).unwrap_or(true)).then(|| "число 1025–65535".into())).parse().unwrap_or(5390);
    println!("4. Сосед. Имя его вводить не нужно — мосты называют себя сами; нужны адрес и ключ.");
    println!("   • На том ПК уже создано приглашение — вставьте строку «адрес ключ».");
    println!("   • Нет — нажмите Enter: создам приглашение, строку для того ПК покажу в конце.");
    println!("   • Сосед не нужен сейчас — «-».");
    let (mut addr, mut secret, mut invite) = (None, None, false);
    loop {
        print!("   Строка или адрес соседа: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
        let line = line.trim();
        if line == "-" {
            break;
        }
        if line.is_empty() {
            secret = Some(NodeConfig::new_secret());
            invite = true;
            break;
        }
        let (a, mut k) = config::split_invite(line, "");
        if let Err(w) = config::parse_address(&a, port) {
            println!("  ✗ {}", w);
            continue;
        }
        addr = Some(a);
        while k.chars().count() < 24 {
            print!("   Ключ пары (BRS-…; Enter — создать новый): ");
            let _ = std::io::stdout().flush();
            let mut kk = String::new();
            std::io::stdin().lock().read_line(&mut kk).map_err(|e| e.to_string())?;
            k = kk.trim().to_string();
            if k.is_empty() {
                k = NodeConfig::new_secret();
                invite = true;
            }
        }
        secret = Some(k);
        break;
    }
    let share_id = ask("5. Пространство — имя (одинаковое у обоих; «-» — без пространства)", Some("-"), |s| (s != "-" && !config::is_name(s)).then(|| "латиница, цифры, «-», «_»".into()));
    let share = if share_id == "-" {
        None
    } else {
        let folder = ask("6. Папка пространства на этом ПК", Some("CORA-мост"), |_| None);
        let pats = ask("7. Какие файлы принимать от соседа — шаблоны через запятую", Some("**"), |_| None);
        Some((share_id.clone(), folder, pats.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>()))
    };
    let cfg = make_config(&node, &my_ip, port, addr.as_deref(), secret.as_deref(), share.as_ref().map(|(a, b, c)| (a.as_str(), b.as_str(), c.clone())));
    let errs = cfg.validate();
    if !errs.is_empty() {
        return Err(format!("Не получилось: {}", errs.join("; ")));
    }
    std::fs::write(file, bridge_core::to_json(&cfg)).map_err(|e| e.to_string())?;
    println!("\nГотово: {}", file.display());
    if invite {
        if let Some(s) = &secret {
            println!("\nПЕРЕДАЙТЕ НА ТОТ ПК СТРОКУ (ключ — как пароль: лично или в надёжном мессенджере):\n   {}:{} {}", my_ip, port, s);
            println!("Там: «Соседи» → «＋ Подключить» → вставить строку.");
        }
    }
    println!("\nЗапускаю узел…\n");
    Ok(())
}

async fn run(o: &Opts) -> Result<i32, String> {
    let file = config_path(o);
    if !file.exists() {
        if std::io::stdin().is_terminal() {
            setup(&file)?;
        } else {
            init(o)?;
        }
    }
    let cfg = NodeConfig::load(&file)?;
    let base = file.parent().map(|p| p.to_path_buf()).filter(|p| !p.as_os_str().is_empty()).unwrap_or(std::env::current_dir().unwrap_or_default());
    let auto_open = cfg.ui.auto_open && !o.opt.contains_key("no-ui");
    let node = Node::start(cfg, StartOptions { base_dir: base, config_file: Some(file.clone()), ..Default::default() }).await?;
    println!("Панель: {}", node.dashboard_url());
    if auto_open {
        let _ = bridge_node::http_local::open_url(&node.dashboard_url());
    }
    // Подтверждение команд в окне: «да N», «нет N причина»
    if std::io::stdin().is_terminal() && node.cfg.read().console_prompts {
        let n2 = node.clone();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines().map_while(Result::ok) {
                let parts: Vec<&str> = line.trim().splitn(3, ' ').collect();
                match parts.first().map(|s| s.to_lowercase()).as_deref() {
                    Some("панель" | "ui") => {
                        let _ = bridge_node::http_local::open_url(&n2.dashboard_url());
                    }
                    Some(w @ ("да" | "y" | "yes" | "нет" | "n" | "no")) if parts.len() > 1 => {
                        let Some(no) = parts[1].parse::<i64>().ok() else { continue };
                        let Some(cmd) = n2.runner.by_no(no) else {
                            println!("Нет команды №{}", no);
                            continue;
                        };
                        let yes = matches!(w, "да" | "y" | "yes");
                        let r = bridge_node::shares::act(
                            &n2,
                            bridge_core::proto::BridgeAction {
                                action: if yes { "approve" } else { "reject" }.into(),
                                share: Some(cmd.share.clone()),
                                record: Some(cmd.record.clone()),
                                actor: std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default(),
                                note: if yes { "подтверждено в окне узла".into() } else { parts.get(2).map(|s| s.to_string()).unwrap_or_else(|| "отклонено в окне узла".into()) },
                                ..Default::default()
                            },
                        );
                        println!("{}", if r.ok { format!("{}: {}", if yes { "Подтверждено" } else { "Отклонено" }, cmd.record) } else { format!("Ошибка: {}", r.error) });
                    }
                    Some("выход" | "exit" | "quit") => {
                        n2.shutdown.cancel();
                        break;
                    }
                    Some(_) => println!("«панель», «да N», «нет N причина», «выход»."),
                    None => {}
                }
            }
        });
    }
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = node.shutdown.cancelled() => {},
    }
    println!("Остановка…");
    node.stop().await;
    Ok(0)
}

// ───────────── Клиент локального API ─────────────

struct Api {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Api {
    fn open(o: &Opts) -> Result<Api, String> {
        let file = config_path(o);
        let cfg = NodeConfig::load(&file)?;
        let dir = file.parent().map(|p| p.to_path_buf()).unwrap_or_default().join(&cfg.data_dir);
        let token = std::fs::read_to_string(dir.join("local.token")).map_err(|_| format!("Узел ещё не запускался (нет {})", dir.join("local.token").display()))?;
        let url = cfg.listen.iter().find(|l| l.contains("127.0.0.1") || l.contains("localhost")).cloned().unwrap_or(cfg.listen[0].clone());
        Ok(Api {
            http: reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(600)).build().map_err(|e| e.to_string())?,
            base: format!("{}{}/local/", url.trim_end_matches('/'), bridge_core::API_PREFIX),
            token: token.trim().trim_start_matches('\u{feff}').to_string(),
        })
    }

    async fn call(&self, method: &str, rel: &str, body: Option<Vec<u8>>, json_body: bool) -> Result<(u16, String), String> {
        let m = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
        let mut rb = self.http.request(m, format!("{}{}", self.base, rel.trim_start_matches('/'))).header(bridge_core::LOCAL_TOKEN_HEADER, &self.token);
        if let Some(b) = body {
            rb = rb.header("Content-Type", if json_body { "application/json; charset=utf-8" } else { "application/octet-stream" }).body(b);
        }
        let r = rb.send().await.map_err(|e| format!("узел не отвечает ({}): запущен ли мост?", e))?;
        let code = r.status().as_u16();
        Ok((code, r.text().await.unwrap_or_default()))
    }

    async fn get(&self, rel: &str) -> Result<String, String> {
        let (code, text) = self.call("GET", rel, None, false).await?;
        if (200..300).contains(&code) {
            Ok(text)
        } else {
            Err(err_text(&text, code))
        }
    }

    async fn post(&self, rel: &str, v: Option<Value>) -> Result<String, String> {
        let (code, text) = self.call("POST", rel, v.map(|v| v.to_string().into_bytes()), true).await?;
        if (200..300).contains(&code) {
            Ok(text)
        } else {
            Err(err_text(&text, code))
        }
    }
}

fn err_text(text: &str, code: u16) -> String {
    serde_json::from_str::<Value>(text).ok().and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from)).unwrap_or_else(|| format!("{} {}", code, text))
}

fn enc(s: &str) -> String {
    bridge_node::client::esc(s)
}

/// Приложения без запущенного моста: заготовка, проверка, упаковка.
fn app_cli(o: &Opts) -> Result<i32, String> {
    match arg(o, 1, "new | check | pack")? {
        "new" => {
            let id = arg(o, 2, "id приложения")?;
            let dir = std::path::PathBuf::from(o.pos.get(3).cloned().unwrap_or_else(|| id.to_string()));
            bridge_node::apps::template(&dir, id)?;
            println!("Заготовка: {}\nПоложите папку в «приложения» рядом с мостом — он подхватит её сам.", dir.display());
            Ok(0)
        }
        "check" => {
            let dir = std::path::PathBuf::from(arg(o, 2, "папка приложения")?);
            let text = std::fs::read_to_string(dir.join("app.json")).map_err(|e| format!("app.json: {}", e))?;
            let a = bridge_node::apps::parse_manifest(&dir, &text)?;
            println!("app.json в порядке: {} «{}» {}", a.id, a.name, a.version);
            Ok(0)
        }
        "pack" => {
            let dir = std::path::PathBuf::from(arg(o, 2, "папка приложения")?);
            let out = o.opt.get("out").map(std::path::PathBuf::from);
            let (p, n) = bridge_node::apps::pack(&dir, out)?;
            println!("Упаковано: {} ({} файлов)", p.display(), n);
            Ok(0)
        }
        x => Err(format!("bridge app {}: нет такой команды (new, check, pack)", x)),
    }
}

fn arg<'a>(o: &'a Opts, i: usize, what: &str) -> Result<&'a str, String> {
    o.pos.get(i).map(|s| s.as_str()).ok_or_else(|| format!("Не хватает: {}\n\n{}", what, HELP))
}

async fn client(cmd: &str, o: &Opts) -> Result<i32, String> {
    let api = Api::open(o)?;
    let json_out = o.opt.contains_key("json");
    let actor = o.opt.get("actor").cloned().unwrap_or_else(|| std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default());
    let share = o.opt.get("share").cloned();
    let print = |t: String| {
        println!("{}", pretty(&t));
        Ok(0)
    };
    let act = |a: Value| {
        let mut a = a;
        if let Some(s) = &share {
            a["share"] = json!(s);
        }
        if a.get("actor").is_none() {
            a["actor"] = json!(actor);
        }
        a
    };
    match cmd {
        "status" => print(api.get("status").await?),
        "peers" => print(api.get("peers").await?),
        "folders" => print(api.get("folders").await?),
        "trash" => print(api.get("trash").await?),
        "sync" => print(api.post("sync", None).await?),
        "peer" => match arg(o, 1, "invite | connect | rename | remove")? {
            "remove" => print(api.post(&format!("peers/remove?name={}", enc(arg(o, 2, "имя соседа")?)), None).await?),
            "rename" => print(api.post(&format!("peers/rename?name={}&to={}", enc(arg(o, 2, "имя соседа")?), enc(arg(o, 3, "новое имя")?)), None).await?),
            "invite" => {
                let title = o.pos.get(2).cloned().or(o.opt.get("title").cloned()).unwrap_or_default();
                let t = api.post(&format!("peers/invite?title={}", enc(&title)), None).await?;
                if json_out {
                    return print(t);
                }
                let v: Value = serde_json::from_str(&t).unwrap_or_default();
                println!("Передайте на тот ПК строку (ключ — как пароль, лично или в надёжном мессенджере):\n\n    {}", v["line"].as_str().unwrap_or(""));
                let others: Vec<String> = v["addresses"].as_array().map(|a| a.iter().skip(1).filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                if !others.is_empty() {
                    println!("\nДругие адреса этого ПК (если тот ПК в другой сети): {}", others.join(", "));
                }
                println!("\nТам: панель → «Соседи» → «＋ Подключить» → вставить строку (или bridge peer connect \"<строка>\").\nЗдесь сосед появится сам, как только подключится.");
                Ok(0)
            }
            "connect" => {
                let body = json!({ "address": arg(o, 2, "адрес соседа (или строка «адрес ключ»)")?, "secret": o.pos.get(3), "title": o.opt.get("title") });
                let (code, text) = api.call("POST", "peers/connect", Some(body.to_string().into_bytes()), true).await?;
                if json_out {
                    println!("{}", text);
                    return Ok(if (200..300).contains(&code) { 0 } else { 1 });
                }
                let v: Value = serde_json::from_str(&text).unwrap_or_default();
                if !v["ok"].as_bool().unwrap_or(false) {
                    return Err(format!("Не подключено: {}", v["error"].as_str().unwrap_or(&text)));
                }
                if v["online"].as_bool().unwrap_or(false) {
                    println!("✓ На связи: сосед назвался «{}». Дальше — пространства и папки: панель → «Соседи» → «Изменить».", v["name"].as_str().unwrap_or(""));
                } else {
                    println!("Сохранён как «{}», пока нет связи: {}", v["name"].as_str().unwrap_or(""), v["error"].as_str().unwrap_or(""));
                    if let Some(h) = v["hint"].as_str().filter(|h| !h.is_empty()) {
                        println!("  → {}", h);
                    }
                    println!("Связь появится сама; подробно — bridge doctor {}", v["name"].as_str().unwrap_or(""));
                }
                Ok(0)
            }
            other => Err(format!("bridge peer invite | connect | rename | remove (не «{}»)", other)),
        },
        "doctor" | "check-peers" => {
            let t = api.get(&format!("peers/check{}", o.pos.get(1).map(|n| format!("?name={}", enc(n))).unwrap_or_default())).await?;
            if json_out {
                return print(t);
            }
            let v: Value = serde_json::from_str(&t).unwrap_or_default();
            let list = if v.is_array() { v.as_array().cloned().unwrap_or_default() } else { vec![v] };
            if list.is_empty() {
                println!("Соседей нет. Подключить: bridge peer invite (здесь) → bridge peer connect \"<строка>\" (там).");
            }
            let mut bad = false;
            for c in &list {
                let st = c["state"].as_str().unwrap_or("");
                bad |= st == "bad";
                let title = c["title"].as_str().unwrap_or("");
                let peer = c["peer"].as_str().unwrap_or("");
                println!("{} {} — {}", mark(st), if title.is_empty() { peer.to_string() } else { format!("{} ({})", title, peer) }, c["summary"].as_str().unwrap_or(""));
                for s in c["steps"].as_array().cloned().unwrap_or_default() {
                    println!("   [{}] {}: {}", mark(s["state"].as_str().unwrap_or("")), s["title"].as_str().unwrap_or(""), s["text"].as_str().unwrap_or(""));
                    if let Some(f) = s["fix"].as_str().filter(|f| !f.is_empty()) {
                        println!("       → {}", f);
                    }
                }
                println!();
            }
            Ok(if bad { 2 } else { 0 })
        }
        "send" => {
            let peer = arg(o, 1, "сосед")?;
            let src = arg(o, 2, "файл")?;
            let dest = o.pos.get(3).cloned().unwrap_or_else(|| Path::new(src).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            let data = std::fs::read(src).map_err(|e| e.to_string())?;
            let (code, text) = api.call("POST", &format!("mailbox/upload?peer={}&path={}", enc(peer), enc(&dest)), Some(data), false).await?;
            if (200..300).contains(&code) {
                print(text)
            } else {
                Err(err_text(&text, code))
            }
        }
        "ignore" => {
            let sub = o.pos.get(1).map(|s| s.as_str()).unwrap_or("list");
            let mut scope = json!({ "scope": "node", "id": "", "peer": "" });
            if let Some(f) = o.opt.get("folder") {
                scope = json!({ "scope": "folder", "id": f, "peer": "" });
            }
            if let Some(m) = o.opt.get("mount") {
                let (p, i) = m.split_once('/').ok_or("--mount сосед/папка")?;
                scope = json!({ "scope": "mount", "id": i, "peer": p });
            }
            match sub {
                "list" => print(api.get("ignore").await?),
                "test" => print(
                    api.get(&format!(
                        "ignore/test?path={}&scope={}&id={}&peer={}",
                        enc(arg(o, 2, "путь")?),
                        scope["scope"].as_str().unwrap(),
                        enc(scope["id"].as_str().unwrap()),
                        enc(scope["peer"].as_str().unwrap())
                    ))
                    .await?,
                ),
                "add" | "remove" => {
                    let mut b = scope.clone();
                    b["pattern"] = json!(arg(o, 2, "шаблон")?);
                    print(api.post(&format!("ignore/{}", sub), Some(b)).await?)
                }
                "set" => {
                    let mut b = scope.clone();
                    b["patterns"] = json!(o.pos[2..].to_vec());
                    print(api.post("ignore", Some(b)).await?)
                }
                _ => Err("bridge ignore [list|add|remove|set|test]".into()),
            }
        }
        "api" => {
            let method = arg(o, 1, "GET|POST")?.to_uppercase();
            let path = arg(o, 2, "путь")?;
            let body = match o.pos.get(3).map(|s| s.as_str()) {
                None => None,
                Some("-") => {
                    let mut b = Vec::new();
                    std::io::Read::read_to_end(&mut std::io::stdin(), &mut b).map_err(|e| e.to_string())?;
                    Some(b)
                }
                Some(s) if s.starts_with('@') => Some(std::fs::read(&s[1..]).map_err(|e| e.to_string())?),
                Some(s) => Some(s.as_bytes().to_vec()),
            };
            let is_json = body.as_ref().map(|b| b.first().map(|c| *c == b'{' || *c == b'[').unwrap_or(false)).unwrap_or(true);
            let (code, text) = api.call(&method, path, body, is_json).await?;
            println!("{}", pretty(&text));
            Ok(if (200..300).contains(&code) { 0 } else { 1 })
        }
        "records" => {
            let mut qs = Vec::new();
            for k in ["type", "status", "assignee", "q", "share"] {
                if let Some(v) = o.opt.get(k) {
                    qs.push(format!("{}={}", k, enc(v)));
                }
            }
            let t = api.get(&format!("records?{}", qs.join("&"))).await?;
            if json_out {
                return print(t);
            }
            let v: Value = serde_json::from_str(&t).unwrap_or_default();
            for r in v.as_array().cloned().unwrap_or_default() {
                println!(
                    "{:<28} {:<9} {:<12} {:<14} {}",
                    r["id"].as_str().unwrap_or(""),
                    r["type"].as_str().unwrap_or(""),
                    r["status"].as_str().unwrap_or(""),
                    r["assignee"].as_str().unwrap_or(""),
                    r["title"].as_str().unwrap_or("")
                );
            }
            Ok(0)
        }
        "show" => {
            let id = arg(o, 1, "id")?;
            let mut rel = format!("records/{}", enc(id));
            if o.opt.contains_key("history") {
                rel.push_str("/history");
            }
            let mut qs = Vec::new();
            if let Some(a) = o.opt.get("at") {
                qs.push(format!("at={}", a));
            }
            if let Some(s) = &share {
                qs.push(format!("share={}", enc(s)));
            }
            if !qs.is_empty() {
                rel = format!("{}?{}", rel, qs.join("&"));
            }
            print(api.get(&rel).await?)
        }
        "task" => {
            let sub = arg(o, 1, "new | accept | …")?;
            let note = o.opt.get("note").cloned().unwrap_or_default();
            let a = if sub == "new" {
                let mut f = json!({ "title": o.opt.get("title").ok_or("--title")?, "assignee": o.opt.get("to").ok_or("--to <узел>")? });
                if let Some(b) = o.opt.get("body") {
                    f["body"] = json!(b);
                }
                if let Some(r) = o.opt.get("ref") {
                    f["ref"] = json!(r);
                }
                json!({ "action": "create", "type": "task", "record": o.opt.get("id"), "fields": f, "note": note })
            } else {
                let to = match sub {
                    "accept" => "accepted",
                    "start" => "in_progress",
                    "review" => "review",
                    "done" => "done",
                    "reject" => "rejected",
                    "cancel" => "cancelled",
                    "reopen" => "new",
                    "return" => "in_progress",
                    other => return Err(format!("нет такого шага задачи: {}", other)),
                };
                json!({ "action": "transition", "record": arg(o, 2, "id")?, "to": to, "note": note })
            };
            do_action(&api, act(a), json_out).await
        }
        "delete" | "restore" => do_action(&api, act(json!({ "action": cmd, "record": arg(o, 1, "id")?, "note": o.opt.get("note").cloned().unwrap_or_default() })), json_out).await,
        "comment" => do_action(&api, act(json!({ "action": "comment", "record": arg(o, 1, "id")?, "text": arg(o, 2, "текст")?, "replyTo": o.opt.get("reply") })), json_out).await,
        "ack" => do_action(&api, act(json!({ "action": "ack", "record": arg(o, 1, "id")?, "level": arg(o, 2, "accepted|rejected|seen")?, "text": o.pos.get(3) })), json_out).await,
        "set" => {
            let mut f = serde_json::Map::new();
            for kv in &o.pos[2..] {
                let (k, v) = kv.split_once('=').ok_or("поле=значение")?;
                f.insert(k.into(), serde_json::from_str(v).unwrap_or(json!(v)));
            }
            let mut a = json!({ "action": "set", "record": arg(o, 1, "id")?, "fields": f });
            if let Some(e) = o.opt.get("expect") {
                a["expectVersion"] = json!(e.parse::<i64>().unwrap_or(0));
            }
            do_action(&api, act(a), json_out).await
        }
        "attach" => do_action(&api, act(json!({ "action": "attach", "record": arg(o, 1, "id")?, "path": arg(o, 2, "путь")?, "title": o.opt.get("title") })), json_out).await,
        "dto" => {
            let fields: Value = serde_json::from_str(&std::fs::read_to_string(arg(o, 2, "файл.json")?).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            do_action(&api, act(json!({ "action": "create", "type": "dto", "schema": arg(o, 1, "схема@версия")?, "fields": fields, "record": o.opt.get("id") })), json_out).await
        }
        "command" => {
            let mut args = serde_json::Map::new();
            for kv in o.pos.iter().skip(3) {
                let (k, v) = kv.split_once('=').ok_or("параметр=значение")?;
                args.insert(k.into(), json!(v));
            }
            do_action(
                &api,
                act(json!({ "action": "create", "type": "command", "fields": { "assignee": arg(o, 1, "узел")?, "runner": arg(o, 2, "исполнитель")?, "args": args, "title": o.opt.get("title") } })),
                json_out,
            )
            .await
        }
        "approve" => do_action(&api, act(json!({ "action": "approve", "record": arg(o, 1, "id")? })), json_out).await,
        "reject" => do_action(&api, act(json!({ "action": "reject", "record": arg(o, 1, "id")?, "note": o.pos.get(2) })), json_out).await,
        "do" => {
            let src = arg(o, 1, "файл.json | -")?;
            let text = if src == "-" {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).map_err(|e| e.to_string())?;
                s
            } else {
                std::fs::read_to_string(src).map_err(|e| e.to_string())?
            };
            let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let list = if v.is_array() { v.as_array().cloned().unwrap() } else { vec![v] };
            let mut code = 0;
            for a in list {
                code = do_action(&api, act(a), json_out).await?;
                if code != 0 {
                    break;
                }
            }
            Ok(code)
        }
        "wait" => {
            let mut qs = vec![format!("timeout={}", o.opt.get("timeout").cloned().unwrap_or("60".into()))];
            if let Some(a) = o.opt.get("after") {
                qs.push(format!("after={}", enc(a)));
            }
            if let Some(s) = &share {
                qs.push(format!("share={}", enc(s)));
            }
            print(api.get(&format!("wait?{}", qs.join("&"))).await?)
        }
        other => Err(format!("Неизвестная команда «{}».\n\n{}", other, HELP)),
    }
}

fn mark(s: &str) -> &'static str {
    match s {
        "ok" => "✓",
        "warn" => "!",
        "bad" => "✗",
        _ => "i",
    }
}

async fn do_action(api: &Api, a: Value, json_out: bool) -> Result<i32, String> {
    let (code, text) = api.call("POST", "actions", Some(a.to_string().into_bytes()), true).await?;
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    if json_out {
        println!("{}", pretty(&text));
    } else if v["ok"].as_bool().unwrap_or(false) {
        println!("✓ {} (версия {})", v["record"].as_str().unwrap_or(""), v["version"]);
    } else {
        eprintln!("✗ {}", v["error"].as_str().unwrap_or(&text));
    }
    Ok(if (200..300).contains(&code) { 0 } else { 1 })
}

fn pretty(t: &str) -> String {
    serde_json::from_str::<Value>(t).map(|v| serde_json::to_string_pretty(&v).unwrap_or(t.into())).unwrap_or(t.into())
}
