//! Ход передач: каждая скачка и отдача файла считает байты — панель показывает процент, скорость и сколько осталось.

use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

struct Item {
    kind: &'static str,
    peer: String,
    what: String,
    path: String,
    total: i64,
    done: Arc<AtomicI64>,
    started: Instant,
}

#[derive(Default)]
pub struct Transfers {
    map: Mutex<BTreeMap<u64, Item>>,
    next: AtomicU64,
    /// Последняя скорость с соседом, байт/с: «сосед/down|up».
    speeds: Mutex<BTreeMap<String, i64>>,
}

/// Пока жива — передача видна; счётчик байтов — `add`.
pub struct Guard {
    owner: Arc<Transfers>,
    id: u64,
    pub done: Arc<AtomicI64>,
}

impl Guard {
    pub fn add(&self, n: usize) {
        self.done.fetch_add(n as i64, Ordering::Relaxed);
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(i) = self.owner.map.lock().remove(&self.id) {
            // скорость заметной передачи — для прогноза синхронизации
            let done = i.done.load(Ordering::Relaxed);
            let secs = i.started.elapsed().as_secs_f64();
            if done > 512 * 1024 && secs > 0.3 {
                let dir = if matches!(i.kind, "down" | "recv") { "down" } else { "up" };
                self.owner.speeds.lock().insert(format!("{}/{}", i.peer.to_lowercase(), dir), (done as f64 / secs) as i64);
            }
        }
    }
}

impl Transfers {
    /// kind: up — отправляем, down — скачиваем, serve — сосед забирает у нас, recv — сосед присылает нам.
    /// what: «папка», «отправка», «пространство».
    pub fn start(self: &Arc<Self>, kind: &'static str, peer: &str, what: &str, path: &str, total: i64) -> Guard {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let done = Arc::new(AtomicI64::new(0));
        self.map.lock().insert(id, Item { kind, peer: peer.into(), what: what.into(), path: path.into(), total, done: done.clone(), started: Instant::now() });
        Guard { owner: self.clone(), id, done }
    }

    /// Скорость с соседом (байт/с): идущая передача, иначе последняя заметная; 0 — неизвестно.
    pub fn speed(&self, peer: &str, down: bool) -> i64 {
        let kinds: &[&str] = if down { &["down", "recv"] } else { &["up", "serve"] };
        let live: Vec<i64> = self.map.lock().values().filter(|i| bridge_core::eq_ic(&i.peer, peer) && kinds.contains(&i.kind) && i.started.elapsed().as_secs_f64() > 1.0).map(|i| (i.done.load(Ordering::Relaxed) as f64 / i.started.elapsed().as_secs_f64()) as i64).collect();
        if let Some(m) = live.into_iter().max() {
            return m;
        }
        self.speeds.lock().get(&format!("{}/{}", peer.to_lowercase(), if down { "down" } else { "up" })).copied().unwrap_or(0)
    }

    /// Для панели. Мелкие и мгновенные (меньше секунды) не показываем — мельтешат.
    pub fn list(&self) -> Vec<Value> {
        self.map
            .lock()
            .values()
            .filter(|i| i.started.elapsed().as_millis() >= 700)
            .map(|i| {
                let done = i.done.load(Ordering::Relaxed);
                let secs = i.started.elapsed().as_secs_f64().max(0.001);
                let speed = done as f64 / secs;
                let eta = if speed > 1.0 && i.total > done { ((i.total - done) as f64 / speed) as i64 } else { -1 };
                json!({ "kind": i.kind, "peer": i.peer, "what": i.what, "path": i.path, "total": i.total, "done": done, "secs": secs as i64, "speed": speed as i64, "eta": eta })
            })
            .collect()
    }
}

/// Ограничение скорости (общее на все передачи в одну сторону). rate — байт/с, 0 — без ограничения.
#[derive(Default)]
pub struct Limiter {
    pub rate: AtomicU64,
    st: tokio::sync::Mutex<Option<(f64, Instant)>>,
}

impl Limiter {
    pub fn set_mbit(&self, mbit: f64) {
        self.rate.store(if mbit > 0.0 { (mbit * 1_000_000.0 / 8.0) as u64 } else { 0 }, Ordering::Relaxed);
    }

    /// Подождать, пока можно отправить/принять n байт.
    pub async fn take(&self, n: usize) {
        let rate = self.rate.load(Ordering::Relaxed) as f64;
        if rate <= 0.0 {
            return;
        }
        let mut g = self.st.lock().await;
        let now = Instant::now();
        let (tokens, last) = g.unwrap_or((rate * 0.25, now));
        // запас — не больше четверти секунды, чтобы скорость была ровной
        let tokens = (tokens + now.duration_since(last).as_secs_f64() * rate).min(rate * 0.25) - n as f64;
        *g = Some((tokens, now));
        if tokens < 0.0 {
            tokio::time::sleep(std::time::Duration::from_secs_f64(-tokens / rate)).await;
        }
    }
}

#[derive(Default)]
pub struct Limits {
    pub up: Limiter,
    pub down: Limiter,
}
