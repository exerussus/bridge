//! Терминал на сервере (только Linux): оболочка в псевдотерминале. Вывод — в канал, ввод и размер — снаружи.

#[cfg(unix)]
mod imp {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};

    pub struct Pty {
        master: File,
        child: Child,
    }

    fn winsize(cols: u16, rows: u16) -> libc::winsize {
        libc::winsize { ws_row: rows.max(2), ws_col: cols.max(10), ws_xpixel: 0, ws_ypixel: 0 }
    }

    impl Pty {
        /// Запустить оболочку входа. Возвращает терминал и читатель его вывода.
        pub fn spawn(cols: u16, rows: u16) -> Result<(Pty, File), String> {
            let mut m: libc::c_int = -1;
            let mut s: libc::c_int = -1;
            let ws = winsize(cols, rows);
            let r = unsafe { libc::openpty(&mut m, &mut s, std::ptr::null_mut(), std::ptr::null(), &ws) };
            if r != 0 {
                return Err(format!("openpty: {}", std::io::Error::last_os_error()));
            }
            let master = unsafe { File::from_raw_fd(m) };
            let slave = unsafe { OwnedFd::from_raw_fd(s) };
            let shell = ["/bin/bash", "/bin/sh"].into_iter().find(|p| std::path::Path::new(p).exists()).unwrap_or("/bin/sh");
            let home = std::env::var("HOME").ok().filter(|h| std::path::Path::new(h).is_dir()).unwrap_or_else(|| "/root".into());
            let mut cmd = Command::new(shell);
            cmd.arg("-l")
                .env("TERM", "xterm-256color")
                .env("HOME", &home)
                .env("LANG", std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into()))
                .current_dir(if std::path::Path::new(&home).is_dir() { home.clone() } else { "/".into() })
                .stdin(Stdio::from(slave.try_clone().map_err(|e| e.to_string())?))
                .stdout(Stdio::from(slave.try_clone().map_err(|e| e.to_string())?))
                .stderr(Stdio::from(slave));
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = cmd.spawn().map_err(|e| format!("{}: {}", shell, e))?;
            let reader = master.try_clone().map_err(|e| e.to_string())?;
            Ok((Pty { master, child }, reader))
        }

        pub fn write(&mut self, data: &[u8]) -> Result<(), String> {
            self.master.write_all(data).map_err(|e| e.to_string())
        }

        pub fn resize(&self, cols: u16, rows: u16) {
            let ws = winsize(cols, rows);
            unsafe {
                libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws);
            }
        }

        pub fn kill(&mut self) {
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGHUP);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// Читать вывод до конца (оболочка закрылась) — в отдельном потоке.
    pub fn read_loop(mut r: File, mut each: impl FnMut(&[u8]) + Send + 'static) {
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 16384];
            loop {
                match r.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => each(&buf[..n]),
                }
            }
            each(&[]);
        });
    }
}

#[cfg(not(unix))]
mod imp {
    pub struct Pty;
    impl Pty {
        pub fn spawn(_c: u16, _r: u16) -> Result<(Pty, std::fs::File), String> {
            Err("терминал есть только на Linux-сервере".into())
        }
        pub fn write(&mut self, _d: &[u8]) -> Result<(), String> {
            Ok(())
        }
        pub fn resize(&self, _c: u16, _r: u16) {}
        pub fn kill(&mut self) {}
    }
    pub fn read_loop(_r: std::fs::File, _each: impl FnMut(&[u8]) + Send + 'static) {}
}

pub use imp::*;
