//! Сетевой адаптер: на Windows — Wintun (адаптер «MostNet»), на Linux — TUN `mostnet0` (для проверки и серверов).
//! Читатель пакетов — свой поток на всё время работы; пакеты он отдаёт в текущий «приёмник» (его меняет сеанс).

use parking_lot::Mutex;
use std::net::Ipv4Addr;
use std::sync::Arc;
use tokio::sync::mpsc;

pub type Sink = Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>;

pub const IFNAME: &str = if cfg!(windows) { "MostNet" } else { "mostnet0" };

#[allow(dead_code)]
fn mask(prefix: u8) -> Ipv4Addr {
    Ipv4Addr::from(if prefix == 0 { 0 } else { u32::MAX << (32 - prefix as u32) })
}

fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let mut c = std::process::Command::new(cmd);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // без окна консоли
    }
    let o = c.output().map_err(|e| format!("{}: {}", cmd, e))?;
    if !o.status.success() {
        return Err(format!("{} {}: {}{}", cmd, args.join(" "), String::from_utf8_lossy(&o.stdout).trim(), String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&o.stdout).to_string())
}

// ---------------- Linux ----------------
#[cfg(target_os = "linux")]
pub struct Tun {
    fd: std::os::fd::OwnedFd,
}

#[cfg(target_os = "linux")]
impl Tun {
    pub fn open(vip: Ipv4Addr, prefix: u8) -> Result<Arc<Tun>, String> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let f = unsafe { libc::open(c"/dev/net/tun".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if f < 0 {
            return Err(format!("/dev/net/tun: {}", std::io::Error::last_os_error()));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(f) };
        #[repr(C)]
        struct IfReq {
            name: [u8; 16],
            flags: libc::c_short,
            _pad: [u8; 22],
        }
        let mut req = IfReq { name: [0; 16], flags: 0x0001 | 0x1000, _pad: [0; 22] }; // IFF_TUN | IFF_NO_PI
        req.name[..IFNAME.len()].copy_from_slice(IFNAME.as_bytes());
        const TUNSETIFF: libc::c_ulong = 0x4004_54ca;
        if unsafe { libc::ioctl(fd.as_raw_fd(), TUNSETIFF as _, &mut req) } < 0 {
            return Err(format!("TUNSETIFF: {}", std::io::Error::last_os_error()));
        }
        let t = Arc::new(Tun { fd });
        t.set_addr(vip, prefix)?;
        Ok(t)
    }

    /// На Linux адаптер остаётся (стенд проверки) — только без адреса.
    pub fn close(&self) {
        let _ = run("ip", &["addr", "flush", "dev", IFNAME]);
        let _ = run("ip", &["link", "set", IFNAME, "down"]);
    }

    pub fn set_addr(&self, vip: Ipv4Addr, prefix: u8) -> Result<(), String> {
        let _ = run("ip", &["addr", "flush", "dev", IFNAME]);
        run("ip", &["addr", "add", &format!("{}/{}", vip, prefix), "brd", "+", "dev", IFNAME])?;
        run("ip", &["link", "set", IFNAME, "mtu", &bridge_core::mostnet::MTU.to_string(), "up"])?;
        Ok(())
    }

    pub fn write(&self, p: &[u8]) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::write(self.fd.as_raw_fd(), p.as_ptr() as *const _, p.len());
        }
    }

    pub fn start_reader(self: &Arc<Self>, sink: Sink) {
        use std::os::fd::AsRawFd;
        let t = self.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 65536];
            loop {
                let n = unsafe { libc::read(t.fd.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len()) };
                if n <= 0 {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
                if let Some(tx) = sink.lock().as_ref() {
                    let _ = tx.try_send(buf[..n as usize].to_vec());
                }
            }
        });
    }
}

// ---------------- Windows ----------------
#[cfg(windows)]
pub struct Tun {
    session: Arc<wintun::Session>,
    _adapter: Arc<wintun::Adapter>,
    /// Какой адрес уже выставлен: тот же — систему не трогаем (каждая перенастройка адаптера —
    /// событие «сеть изменилась» для всех программ, включая Radmin).
    addr: Mutex<Option<Ipv4Addr>>,
    alive: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(windows)]
impl Tun {
    pub fn open(vip: Ipv4Addr, prefix: u8) -> Result<Arc<Tun>, String> {
        let dll = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("wintun.dll");
        if !dll.is_file() {
            return Err(format!("нет {} — переустановите службу из Моста", dll.display()));
        }
        let w = unsafe { wintun::load_from_path(&dll) }.map_err(|e| format!("wintun.dll: {}", e))?;
        let adapter = match wintun::Adapter::open(&w, IFNAME) {
            Ok(a) => a,
            Err(_) => wintun::Adapter::create(&w, IFNAME, "MostNet", None).map_err(|e| format!("адаптер MostNet: {}", e))?,
        };
        let session = Arc::new(adapter.start_session(wintun::MAX_RING_CAPACITY).map_err(|e| format!("адаптер MostNet: {}", e))?);
        let t = Arc::new(Tun { session, _adapter: adapter, addr: Mutex::new(None), alive: Arc::new(std::sync::atomic::AtomicBool::new(true)) });
        t.set_addr(vip, prefix)?;
        Ok(t)
    }

    pub fn set_addr(&self, vip: Ipv4Addr, prefix: u8) -> Result<(), String> {
        let mut cur = self.addr.lock();
        if *cur == Some(vip) {
            return Ok(());
        }
        let first = cur.is_none();
        *cur = Some(vip);
        drop(cur);
        if let Err(e) = run("netsh", &["interface", "ipv4", "set", "address", &format!("name={}", IFNAME), "static", &vip.to_string(), &mask(prefix).to_string()]) {
            *self.addr.lock() = None;
            return Err(e);
        }
        if !first {
            return Ok(());
        }
        let _ = run("netsh", &["interface", "ipv4", "set", "subinterface", IFNAME, &format!("mtu={}", bridge_core::mostnet::MTU), "store=active"]);
        // низкая метрика — чтобы широковещательные пакеты игр (255.255.255.255) шли и сюда; сеть — «частная»
        // (иначе брандмауэр Windows режет входящие); профиль сети появляется не сразу — пробуем минуту
        let alive = self.alive.clone();
        std::thread::spawn(move || {
            let _ = run("powershell", &["-NoProfile", "-Command", &format!("Set-NetIPInterface -InterfaceAlias '{}' -InterfaceMetric 5", IFNAME)]);
            // (3.19.2) и дальше следить: Windows может опознать сеть заново и сделать её «общественной» —
            // тогда брандмауэр режет Мост и игры из своей сети. Проверка ничего не меняет, если уже «частная».
            let cmd = format!("$p = Get-NetConnectionProfile -InterfaceAlias '{0}' -ErrorAction Stop; if ($p.NetworkCategory -ne 'Private') {{ Set-NetConnectionProfile -InterfaceAlias '{0}' -NetworkCategory Private -ErrorAction Stop }}", IFNAME);
            let mut n = 0u32;
            while alive.load(std::sync::atomic::Ordering::Relaxed) {
                let ok = run("powershell", &["-NoProfile", "-Command", &cmd]).is_ok();
                n += 1;
                // первую минуту — часто (профиль появляется не сразу), потом — раз в 5 минут
                let wait = if !ok && n < 30 { 2 } else { 300 };
                for _ in 0..wait {
                    if !alive.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        });
        Ok(())
    }

    pub fn write(&self, p: &[u8]) {
        if let Ok(mut pk) = self.session.allocate_send_packet(p.len() as u16) {
            pk.bytes_mut().copy_from_slice(p);
            self.session.send_packet(pk);
        }
    }

    pub fn start_reader(self: &Arc<Self>, sink: Sink) {
        let s = self.session.clone();
        std::thread::spawn(move || loop {
            match s.receive_blocking() {
                Ok(p) => {
                    if let Some(tx) = sink.lock().as_ref() {
                        let _ = tx.try_send(p.bytes().to_vec());
                    }
                }
                // сеанс закрыт (сеть выключили) — поток заканчивается, адаптер уходит вместе с последней ссылкой
                Err(_) => break,
            }
        });
    }

    /// Закрыть адаптер: сеанс завершается, адаптер MostNet исчезает из системы.
    pub fn close(&self) {
        self.alive.store(false, std::sync::atomic::Ordering::Relaxed);
        let _ = self.session.shutdown();
    }
}

// ---------------- прочие ----------------
#[cfg(not(any(windows, target_os = "linux")))]
pub struct Tun;

#[cfg(not(any(windows, target_os = "linux")))]
impl Tun {
    pub fn open(_vip: Ipv4Addr, _prefix: u8) -> Result<Arc<Tun>, String> {
        Err("своя сеть есть только на Windows и Linux".into())
    }
    pub fn set_addr(&self, _vip: Ipv4Addr, _prefix: u8) -> Result<(), String> {
        Ok(())
    }
    pub fn write(&self, _p: &[u8]) {}
    pub fn start_reader(self: &Arc<Self>, _sink: Sink) {}
    pub fn close(&self) {}
}

#[allow(dead_code)]
pub fn sh(cmd: &str, args: &[&str]) -> Result<String, String> {
    run(cmd, args)
}

#[cfg(test)]
mod tests {
    #[test]
    fn masks() {
        assert_eq!(super::mask(16).to_string(), "255.255.0.0");
        assert_eq!(super::mask(24).to_string(), "255.255.255.0");
    }
}
