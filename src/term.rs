use std::ffi::{c_int, c_ulong, c_void};
use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

#[repr(C)]
#[derive(Clone, Copy)]
struct Termios([u32; 16]);

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

#[repr(C)]
#[derive(Default)]
struct Winsize {
    rows: u16,
    cols: u16,
    xpix: u16,
    ypix: u16,
}

unsafe extern "C" {
    fn tcgetattr(fd: c_int, t: *mut Termios) -> c_int;
    fn tcsetattr(fd: c_int, act: c_int, t: *const Termios) -> c_int;
    fn cfmakeraw(t: *mut Termios);
    fn ioctl(fd: c_int, req: c_ulong, ...) -> c_int;
    fn poll(fds: *mut PollFd, n: c_ulong, timeout: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
    fn signal(sig: c_int, handler: extern "C" fn(c_int)) -> usize;
}

#[cfg(any(target_arch = "powerpc", target_arch = "powerpc64", target_arch = "mips", target_arch = "mips64", target_arch = "sparc64"))]
const TIOCGWINSZ: c_ulong = 0x4008_7468;
#[cfg(not(any(target_arch = "powerpc", target_arch = "powerpc64", target_arch = "mips", target_arch = "mips64", target_arch = "sparc64")))]
const TIOCGWINSZ: c_ulong = 0x5413;
const SIGWINCH: c_int = 28;

static ORIG: OnceLock<Termios> = OnceLock::new();
static RESIZED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_winch(_: c_int) {
    RESIZED.store(true, Ordering::Relaxed);
}

pub struct Term;

impl Term {
    pub fn enter() -> Result<Term, String> {
        let mut t = Termios([0; 16]);
        if unsafe { tcgetattr(0, &mut t) } != 0 {
            return Err("stdin is not a terminal".into());
        }
        let _ = ORIG.set(t);
        unsafe {
            cfmakeraw(&mut t);
            tcsetattr(0, 0, &t);
            signal(SIGWINCH, on_winch);
        }
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            hook(info);
        }));
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?1002h\x1b[?1006h");
        let _ = out.flush();
        Ok(Term)
    }

    pub fn size(&self) -> (usize, usize) {
        let mut w = Winsize::default();
        if unsafe { ioctl(1, TIOCGWINSZ, &mut w) } == 0 && w.cols > 0 && w.rows > 0 {
            (w.cols as usize, w.rows as usize)
        } else {
            (80, 24)
        }
    }

    pub fn read(&self, buf: &mut [u8], timeout_ms: c_int) -> Option<usize> {
        loop {
            let mut p = PollFd { fd: 0, events: 1, revents: 0 };
            let r = unsafe { poll(&mut p, 1, timeout_ms) };
            if RESIZED.swap(false, Ordering::Relaxed) || r == 0 {
                return Some(0);
            }
            if r > 0 {
                let n = unsafe { read(0, buf.as_mut_ptr().cast(), buf.len()) };
                return (n > 0).then_some(n as usize);
            }
            if r < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return None;
            }
        }
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        restore();
    }
}

fn restore() {
    if let Some(t) = ORIG.get() {
        unsafe { tcsetattr(0, 0, t) };
    }
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(b"\x1b[0m\x1b[?1002l\x1b[?1006l\x1b[?25h\x1b[?1049l");
    let _ = out.flush();
}

pub enum Key {
    Left(bool),
    Right(bool),
    Up(bool),
    Down(bool),
    Tab,
    BackTab,
    Enter,
    Press(usize, usize),
    Drag(usize, usize),
    Release,
    Quit,
    Esc,
    Char(char),
}

pub fn parse_keys(b: &[u8], keys: &mut Vec<Key>) {
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        i += 1;
        let key = match c {
            0x1b if i < b.len() && (b[i] == b'[' || b[i] == b'O') => {
                i += 1;
                if b[i - 1] == b'[' && b.get(i) == Some(&b'<') {
                    let start = i + 1;
                    while i < b.len() && !matches!(b[i], b'M' | b'm') {
                        i += 1;
                    }
                    let Some(&fin) = b.get(i) else { break };
                    i += 1;
                    let f: Vec<usize> = std::str::from_utf8(&b[start..i - 1])
                        .unwrap_or("")
                        .split(';')
                        .filter_map(|n| n.parse().ok())
                        .collect();
                    let [btn, col, row] = f[..] else { continue };
                    let (col, row) = (col.saturating_sub(1), row.saturating_sub(1));
                    keys.push(match (fin, btn) {
                        (b'm', _) => Key::Release,
                        (_, 0) => Key::Press(col, row),
                        (_, 32) => Key::Drag(col, row),
                        _ => continue,
                    });
                    continue;
                }
                let start = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b';') {
                    i += 1;
                }
                let Some(&fin) = b.get(i) else { break };
                i += 1;
                let m = std::str::from_utf8(&b[start..i - 1])
                    .ok()
                    .and_then(|p| p.rsplit(';').next())
                    .and_then(|m| m.parse::<u8>().ok())
                    .unwrap_or(1);
                let shift = m > 1 && (m - 1) & 1 == 1;
                match fin {
                    b'A' => Key::Up(shift),
                    b'B' => Key::Down(shift),
                    b'C' => Key::Right(shift),
                    b'D' => Key::Left(shift),
                    b'Z' => Key::BackTab,
                    _ => continue,
                }
            }
            0x1b => Key::Esc,
            0x03 => Key::Quit,
            b'\t' => Key::Tab,
            b'\r' | b'\n' => Key::Enter,
            0x20..=0x7e => Key::Char(c as char),
            _ => continue,
        };
        keys.push(key);
    }
}
