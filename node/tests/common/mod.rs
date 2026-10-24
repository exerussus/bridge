//! Сеть из нескольких узлов в одном процессе (127.0.0.1, случайные порты, временные папки).
#![allow(dead_code)]

use bridge_core::config::{FileAcceptRule, NodeConfig, PeerConfig, ShareConfig};
use bridge_core::proto::BridgeAction;
use bridge_node::{Node, StartOptions};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub struct Net {
    pub root: PathBuf,
    pub configs: BTreeMap<String, NodeConfig>,
    pub nodes: BTreeMap<String, Arc<Node>>,
    pub logs: Arc<Mutex<Vec<String>>>,
}

pub fn secret(a: &str, b: &str) -> String {
    let mut v = [a, b];
    v.sort();
    format!("BRS-test-secret-{}-{}-0123456789", v[0], v[1])
}

impl Net {
    pub fn new() -> Net {
        let root = std::env::temp_dir().join("bridge-rs-tests").join(&uuid_like());
        std::fs::create_dir_all(&root).unwrap();
        Net { root, configs: BTreeMap::new(), nodes: BTreeMap::new(), logs: Arc::new(Mutex::new(Vec::new())) }
    }

    pub fn add(&mut self, name: &str, peers: &[&str], accept: &[(&str, &str)]) -> &mut NodeConfig {
        let cfg = NodeConfig {
            node: name.into(),
            listen: vec!["http://127.0.0.1:0".into()],
            listen_auto: false,
            // ретранслятор голоса в тестах выключен: узлы иначе делят порт UDP 3478
            turn_port: 0,
            interval_sec: 3600,
            console_prompts: false,
            peers: peers.iter().map(|p| PeerConfig { name: p.to_string(), url: "http://127.0.0.1:1".into(), ips: vec!["127.0.0.1".into()], secret: secret(name, p), ..Default::default() }).collect(),
            shares: vec![ShareConfig {
                id: "s".into(),
                folder: self.root.join(name).join("share").to_string_lossy().into_owned(),
                peers: peers.iter().map(|p| p.to_string()).collect(),
                accept: accept.iter().map(|(p, f)| FileAcceptRule { pattern: p.to_string(), from: vec![f.to_string()] }).collect(),
                ..Default::default()
            }],
            ..Default::default()
        };
        self.configs.insert(name.into(), cfg);
        self.configs.get_mut(name).unwrap()
    }

    pub async fn start(&mut self) {
        for (name, cfg) in self.configs.clone() {
            let dir = self.root.join(&name);
            std::fs::create_dir_all(&dir).unwrap();
            let logs = self.logs.clone();
            let n2 = name.clone();
            let node = Node::start(
                cfg,
                StartOptions {
                    base_dir: dir.clone(),
                    config_file: Some(dir.join("bridge.json")),
                    console: false,
                    log: Some(Arc::new(move |s: &str| logs.lock().push(format!("{} {}: {}", chrono::Local::now().format("%S%.3f"), n2, s)))),
                    settle_secs: 0.0,
                    timer: false,
                },
            )
            .await
            .unwrap();
            self.nodes.insert(name, node);
        }
        // адреса соседей — после запуска (порты случайные)
        let urls: BTreeMap<String, String> = self.nodes.iter().map(|(n, node)| (n.clone(), node.urls.read()[0].clone())).collect();
        for node in self.nodes.values() {
            let mut cfg = node.cfg.write();
            for p in cfg.peers.iter_mut() {
                if let Some(u) = urls.get(&p.name) {
                    p.url = u.clone();
                }
            }
        }
    }

    pub fn n(&self, name: &str) -> &Arc<Node> {
        &self.nodes[name]
    }

    pub fn folder(&self, name: &str) -> PathBuf {
        self.nodes[name].share("s").unwrap().root.clone()
    }

    pub fn write(&self, node: &str, path: &str, text: &str) -> PathBuf {
        let full = self.folder(node).join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, text).unwrap();
        // время — в прошлое на шаг: порядок правок однозначен даже в пределах секунды
        let t = std::time::SystemTime::now() - std::time::Duration::from_millis(5);
        let f = std::fs::OpenOptions::new().write(true).open(&full).unwrap();
        f.set_modified(t).unwrap();
        full
    }

    pub fn read(&self, node: &str, path: &str) -> Option<String> {
        std::fs::read_to_string(self.folder(node).join(path)).ok()
    }

    pub async fn sync(&self, rounds: usize, order: &[&str]) {
        // дать слежению за папками доставить события (как в жизни — за доли секунды)
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        for n in self.nodes.values() {
            n.folders.invalidate_own();
        }
        let names: Vec<String> = if order.is_empty() { self.nodes.keys().cloned().collect() } else { order.iter().map(|s| s.to_string()).collect() };
        for _ in 0..rounds {
            for n in &names {
                self.nodes[n].sync_now().await;
            }
        }
    }

    pub fn act(&self, node: &str, mut a: BridgeAction) -> bridge_core::proto::ActionResult {
        if a.share.is_none() {
            a.share = Some("s".into());
        }
        bridge_node::shares::act(&self.nodes[node], a)
    }

    pub fn log_has(&self, prefix: &str, text: &str) -> bool {
        self.logs.lock().iter().any(|l| l.split_once(' ').map(|x| x.1).unwrap_or(l).starts_with(prefix) && l.contains(text))
    }

    pub async fn stop(self) {
        for n in self.nodes.values() {
            n.stop().await;
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn uuid_like() -> String {
    format!("{:x}", rand::random::<u64>())
}

/// Пара «разработка ↔ стенд» с правилами как у CORA-мост.
pub fn pair() -> Net {
    let mut net = Net::new();
    net.add("dev", &["stand"], &[("отчёты/**", "stand"), ("вопросы/**", "*"), ("состояние.md", "*"), ("_команды/**", "stand")]);
    net.add("stand", &["dev"], &[("задания/**", "dev"), ("пакеты/**", "dev"), ("вопросы/**", "*"), ("состояние.md", "*"), ("_schemas/**", "dev")]);
    net
}

pub fn addr(net: &Net, node: &str) -> String {
    net.n(node).urls.read()[0].trim_start_matches("http://").to_string()
}
