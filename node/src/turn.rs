//! Ретранслятор WebRTC на этом ПК (TURN; сам движок — общий ящик `bridge-turn`, тот же, что у сервера `most-relay`).
//! Нужен, когда участники разговора не видят друг друга напрямую: телефон в Wi-Fi или Tailscale, а сосед — только
//! в Radmin. Тогда звук идёт через этот ПК: оба браузера подключаются к ретранслятору моста, мост пересылает пакеты.
//! Пускает только по временным ключам: их выдаёт сам мост — своим панелям, своим телефонам и соседям (в подписанном
//! привете). С 3.15 — UDP и TCP на одном порту: TCP выручает, когда UDP между ПК не проходит (бывает в Radmin).

use crate::node::Node;
use bridge_turn::Turn as Core;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, UdpSocket};

#[derive(Default)]
pub struct Turn {
    pub core: Arc<Core>,
    pub port: AtomicU16,
    bound: Mutex<HashSet<IpAddr>>,
    failed: Mutex<HashSet<IpAddr>>,
    bound_tcp: Mutex<HashSet<IpAddr>>,
    failed_tcp: Mutex<HashSet<IpAddr>>,
    /// Слушает ли TCP хоть на одной сети, кроме 127.0.0.1 (тогда соседям — и адрес `?transport=tcp`).
    pub tcp: AtomicBool,
    linked: AtomicBool,
}

impl Turn {
    pub fn load(&self, node: &Node) {
        let f = node.data_dir.join("turn.key");
        let key = match std::fs::read(&f) {
            Ok(k) if k.len() >= 32 => k,
            _ => {
                let k: [u8; 32] = rand::random();
                let _ = std::fs::write(&f, k);
                k.to_vec()
            }
        };
        self.core.set_secret(key);
    }

    /// Временный ключ ретранслятора (как TURN REST API): имя «срок:кто», пароль — HMAC от имени.
    pub fn creds(&self, who: &str, ttl: Duration) -> (String, String) {
        self.core.creds(who, ttl)
    }

    pub fn running(&self) -> bool {
        self.port.load(Ordering::SeqCst) != 0
    }

    pub fn allocations(&self) -> usize {
        self.core.allocations()
    }
}

/// Слушать на 127.0.0.1 и на каждой сети ПК (у каждой — свой адрес ретрансляции: телефону в Wi-Fi — адрес Wi-Fi,
/// соседу в Radmin — адрес Radmin). Повторяется вместе с проверкой сетей.
pub async fn bind_all(node: &Arc<Node>) {
    let port = node.cfg.read().turn_port;
    if port == 0 {
        return;
    }
    if !node.turn.core.has_secret() {
        node.turn.load(node);
    }
    if !node.turn.linked.swap(true, Ordering::SeqCst) {
        let (a, b) = (node.shutdown.clone(), node.turn.core.stop.clone());
        tokio::spawn(async move {
            a.cancelled().await;
            b.cancel();
        });
    }
    let mut ips: Vec<IpAddr> = vec![IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)];
    ips.extend(crate::http_peer::interface_ips().into_iter().map(|x| x.0));
    for ip in ips {
        if node.turn.bound.lock().contains(&ip) || node.turn.failed.lock().contains(&ip) {
            continue;
        }
        match UdpSocket::bind(SocketAddr::new(ip, port)).await {
            Ok(s) => {
                let real = s.local_addr().map(|a| a.port()).unwrap_or(port);
                node.turn.port.store(real, Ordering::SeqCst);
                node.turn.bound.lock().insert(ip);
                tokio::spawn(bridge_turn::serve_udp(node.turn.core.clone(), Arc::new(s)));
            }
            Err(e) => {
                node.turn.failed.lock().insert(ip);
                if ip.is_loopback() {
                    node.info(&format!("Ретранслятор голоса: порт UDP {} занят ({}) — разговоры через этот ПК не пойдут", port, e));
                }
            }
        }
    }
    // TCP — на том же номере порта, что получил UDP
    let tport = node.turn.port.load(Ordering::SeqCst);
    if tport == 0 {
        return;
    }
    let bound: Vec<IpAddr> = node.turn.bound.lock().iter().copied().collect();
    for ip in bound {
        if node.turn.bound_tcp.lock().contains(&ip) || node.turn.failed_tcp.lock().contains(&ip) {
            continue;
        }
        match TcpListener::bind(SocketAddr::new(ip, tport)).await {
            Ok(l) => {
                node.turn.bound_tcp.lock().insert(ip);
                if !ip.is_loopback() {
                    node.turn.tcp.store(true, Ordering::SeqCst);
                }
                tokio::spawn(bridge_turn::serve_tcp(node.turn.core.clone(), l));
            }
            Err(e) => {
                node.turn.failed_tcp.lock().insert(ip);
                if ip.is_loopback() {
                    node.info(&format!("Ретранслятор голоса: порт TCP {} занят ({}) — через Radmin без UDP разговор может не пойти", tport, e));
                }
            }
        }
    }
}
