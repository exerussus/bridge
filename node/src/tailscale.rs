//! Tailscale: связь с телефоном вне дома. Мост не держит своих сертификатов — HTTPS раздаёт сама Tailscale
//! (`tailscale serve`: https://<пк>.<сеть>.ts.net → мост на этом ПК). Сертификат она получает и продлевает сама.

use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TsStatus {
    /// Программа Tailscale есть на ПК.
    pub installed: bool,
    /// Вошли и на связи (BackendState = Running).
    pub running: bool,
    pub state: String,
    /// Имя ПК в Tailscale: pc.tail1234.ts.net
    pub dns: String,
    pub ips: Vec<String>,
    /// HTTPS включён в настройках сети Tailscale (есть домен для сертификата).
    pub https_allowed: bool,
    pub error: String,
}

pub fn cli() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["tailscale.exe"] } else { &["tailscale"] };
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            for n in names {
                let p = d.join(n);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    let extra: Vec<PathBuf> = if cfg!(windows) {
        let mut v = vec![PathBuf::from(r"C:\Program Files\Tailscale\tailscale.exe"), PathBuf::from(r"C:\Program Files (x86)\Tailscale\tailscale.exe")];
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            v.push(PathBuf::from(pf).join("Tailscale").join("tailscale.exe"));
        }
        v
    } else {
        vec![PathBuf::from("/usr/bin/tailscale"), PathBuf::from("/usr/local/bin/tailscale"), PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale")]
    };
    extra.into_iter().find(|p| p.is_file())
}

/// Запустить tailscale с аргументами: (код, вывод). Если команда ждёт дольше `wait` — вернуть то, что успела написать.
pub async fn run(args: &[&str], wait: Duration) -> Result<(i32, String), String> {
    let exe = cli().ok_or("Tailscale не установлен")?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("не удалось запустить tailscale: {}", e))?;
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let out = std::sync::Arc::new(parking_lot::Mutex::new(String::new()));
    let (o1, o2) = (out.clone(), out.clone());
    let r1 = tokio::spawn(async move {
        if let Some(s) = so.as_mut() {
            let mut b = vec![0u8; 8192];
            while let Ok(n) = s.read(&mut b).await {
                if n == 0 {
                    break;
                }
                o1.lock().push_str(&String::from_utf8_lossy(&b[..n]));
            }
        }
    });
    let r2 = tokio::spawn(async move {
        if let Some(s) = se.as_mut() {
            let mut b = vec![0u8; 8192];
            while let Ok(n) = s.read(&mut b).await {
                if n == 0 {
                    break;
                }
                o2.lock().push_str(&String::from_utf8_lossy(&b[..n]));
            }
        }
    });
    let code = match tokio::time::timeout(wait, child.wait()).await {
        Ok(Ok(st)) => st.code().unwrap_or(-1),
        Ok(Err(e)) => return Err(e.to_string()),
        Err(_) => {
            let _ = child.kill().await;
            -2
        }
    };
    let _ = tokio::time::timeout(Duration::from_millis(500), async {
        let _ = r1.await;
        let _ = r2.await;
    })
    .await;
    let text = out.lock().clone();
    Ok((code, text))
}

pub async fn status() -> TsStatus {
    let mut st = TsStatus { installed: cli().is_some(), ..Default::default() };
    if !st.installed {
        return st;
    }
    match run(&["status", "--json"], Duration::from_secs(6)).await {
        Ok((_, text)) => {
            let start = text.find('{').unwrap_or(0);
            match serde_json::from_str::<Value>(&text[start..]) {
                Ok(v) => {
                    st.state = v["BackendState"].as_str().unwrap_or("").to_string();
                    st.running = st.state == "Running";
                    st.dns = v["Self"]["DNSName"].as_str().unwrap_or("").trim_end_matches('.').to_string();
                    st.ips = v["Self"]["TailscaleIPs"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).filter(|x| !x.contains(':')).map(String::from).collect()).unwrap_or_default();
                    st.https_allowed = v["CertDomains"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
                }
                Err(_) => st.error = first_line(&text),
            }
        }
        Err(e) => st.error = e,
    }
    st
}

fn first_line(t: &str) -> String {
    t.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(300).collect()
}

/// Что сейчас раздаёт Tailscale: (порт HTTPS, куда ведёт «/»).
pub async fn serve_list() -> Vec<(u16, String)> {
    let Ok((_, text)) = run(&["serve", "status", "--json"], Duration::from_secs(6)).await else { return Vec::new() };
    let start = text.find('{').unwrap_or(text.len());
    let Ok(v) = serde_json::from_str::<Value>(text.get(start..).unwrap_or("")) else { return Vec::new() };
    let mut out = Vec::new();
    if let Some(web) = v["Web"].as_object() {
        for (hostport, h) in web {
            let port = hostport.rsplit(':').next().and_then(|p| p.parse().ok()).unwrap_or(443);
            let proxy = h["Handlers"]["/"]["Proxy"].as_str().unwrap_or("").to_string();
            out.push((port, proxy));
        }
    }
    out
}

pub enum ServeResult {
    On(u16),
    /// HTTPS выключен в сети Tailscale — ссылка, где его включить.
    NeedEnable(String),
    Failed(String),
}

/// Включить HTTPS: Tailscale раздаёт `target` (адрес моста на этом ПК). Порт — свободный из 443/8443/10000 или уже наш.
pub async fn serve_on(target: &str, want: u16) -> ServeResult {
    let used = serve_list().await;
    let mine = |p: u16| used.iter().any(|(x, t)| *x == p && t.trim_end_matches('/') == target.trim_end_matches('/'));
    let busy = |p: u16| used.iter().any(|(x, _)| *x == p);
    let port = if want != 0 {
        want
    } else if let Some(p) = [443u16, 8443, 10000].into_iter().find(|p| mine(*p)) {
        return ServeResult::On(p);
    } else {
        [443u16, 8443, 10000].into_iter().find(|p| !busy(*p)).unwrap_or(8443)
    };
    if mine(port) {
        return ServeResult::On(port);
    }
    let https = format!("--https={}", port);
    match run(&["serve", "--bg", "--yes", &https, target], Duration::from_secs(10)).await {
        Ok((code, text)) => {
            if let Some(url) = text.split_whitespace().find(|w| w.starts_with("https://login.tailscale.com/")) {
                return ServeResult::NeedEnable(url.to_string());
            }
            if code == 0 {
                return ServeResult::On(port);
            }
            // старые версии не знают --yes
            if text.contains("--yes") || text.contains("flag provided but not defined") {
                if let Ok((c2, t2)) = run(&["serve", "--bg", &https, target], Duration::from_secs(10)).await {
                    if let Some(url) = t2.split_whitespace().find(|w| w.starts_with("https://login.tailscale.com/")) {
                        return ServeResult::NeedEnable(url.to_string());
                    }
                    if c2 == 0 {
                        return ServeResult::On(port);
                    }
                    return ServeResult::Failed(first_line(&t2));
                }
            }
            ServeResult::Failed(if text.trim().is_empty() { format!("tailscale serve завершился с кодом {}", code) } else { first_line(&text) })
        }
        Err(e) => ServeResult::Failed(e),
    }
}

pub async fn serve_off(port: u16) -> Result<(), String> {
    let https = format!("--https={}", port);
    match run(&["serve", &https, "off"], Duration::from_secs(8)).await {
        Ok((0, _)) => Ok(()),
        Ok((_, t)) => Err(first_line(&t)),
        Err(e) => Err(e),
    }
}

/// Адрес Tailscale (100.64.0.0/10).
pub fn is_ts_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => {
            let o = v.octets();
            o[0] == 100 && (64..128).contains(&o[1])
        }
        _ => false,
    }
}
