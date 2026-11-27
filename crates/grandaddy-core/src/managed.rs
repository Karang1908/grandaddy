//! `grandaddy run <agent> [args…]` — runs any agent CLI inside a pseudo-terminal grandaddy owns.
//! You use the agent exactly as usual in your terminal; grandaddy can also type prompts into it
//! (from the hub, the app, loops or other sessions) and knows which window it lives in.
//!
//! Each run registers itself in ~/.grandaddy/runs/<id>.json and listens on a private Unix
//! socket for {"text": "...", "submit": true} lines.
use crate::providers::procs;
use crate::util::{self, now};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// BSD tty ioctls (libc doesn't export these for Apple targets)
const TIOCSCTTY: libc::c_ulong = 0x2000_7461;
const TIOCGWINSZ: libc::c_ulong = 0x4008_7468;
const TIOCSWINSZ: libc::c_ulong = 0x8008_7467;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    /// the agent process
    pub pid: i64,
    /// the `grandaddy run` process, whose terminal window is the session's window
    pub host_pid: i64,
    pub tool: String,
    pub command: Vec<String>,
    pub cwd: String,
    pub tty: String,
    pub started: f64,
    pub sock: String,
}

fn dir() -> PathBuf {
    util::state().join("runs")
}

/// Socket paths are limited to ~104 bytes on macOS, so keep them short and out of $HOME.
fn sock_path(id: &str) -> String {
    format!("/tmp/gd-{}-{id}.sock", unsafe { libc::getuid() })
}

/// Live runs; entries whose process has gone are removed.
pub fn list() -> Vec<Run> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(dir()) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        match util::read_json::<Run>(&p) {
            Some(r) if util::alive(r.pid) => out.push(r),
            Some(r) => {
                util::remove(&p);
                let _ = std::fs::remove_file(&r.sock);
            }
            None => {}
        }
    }
    out
}

/// The run hosting a session: same process, or the only run of that tool in that folder.
pub fn match_session(runs: &[Run], pid: Option<i64>, tool: &str, cwd: &str) -> Option<String> {
    if let Some(pid) = pid {
        if let Some(r) = runs.iter().find(|r| r.pid == pid || r.host_pid == pid) {
            return Some(r.id.clone());
        }
    }
    let same: Vec<&Run> = runs.iter().filter(|r| r.tool == tool && !cwd.is_empty() && r.cwd == cwd).collect();
    (same.len() == 1).then(|| same[0].id.clone())
}

pub fn get(id: &str) -> Option<Run> {
    list().into_iter().find(|r| r.id == id)
}

/// Type `text` into a managed session (and press return if `submit`).
pub fn send(id: &str, text: &str, submit: bool) -> Result<(), String> {
    let run = get(id).ok_or("that session has ended")?;
    let mut s = UnixStream::connect(&run.sock).map_err(|e| format!("couldn't reach the session: {e}"))?;
    s.set_write_timeout(Some(Duration::from_secs(3))).ok();
    let line = serde_json::json!({"text": text, "submit": submit}).to_string() + "\n";
    s.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    let mut ack = String::new();
    s.set_read_timeout(Some(Duration::from_secs(3))).ok();
    let _ = BufReader::new(s).read_line(&mut ack);
    if ack.trim() == "ok" { Ok(()) } else { Err(format!("the session didn't accept it: {}", ack.trim())) }
}

struct RawMode(Option<libc::termios>);
impl RawMode {
    fn enter(fd: RawFd) -> RawMode {
        unsafe {
            if libc::isatty(fd) != 1 {
                return RawMode(None);
            }
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut t) != 0 {
                return RawMode(None);
            }
            let saved = t;
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(fd, libc::TCSANOW, &t);
            RawMode(Some(saved))
        }
    }
}
impl Drop for RawMode {
    fn drop(&mut self) {
        if let Some(t) = self.0 {
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &t) };
        }
    }
}

fn winsize(fd: RawFd) -> Option<libc::winsize> {
    unsafe {
        let mut w: libc::winsize = std::mem::zeroed();
        (libc::ioctl(fd, TIOCGWINSZ, &mut w) == 0 && w.ws_col > 0).then_some(w)
    }
}

/// Run the agent; returns its exit code.
pub fn run(args: &[String]) -> Result<i32, String> {
    let Some(program) = args.first() else { return Err("usage: grandaddy run <agent> [args…]".into()) };
    let tool = procs::tool_of(program).unwrap_or("agent").to_string();
    let (mut master, mut slave): (RawFd, RawFd) = (-1, -1);
    unsafe {
        let mut term: libc::termios = std::mem::zeroed();
        let have_term = libc::tcgetattr(0, &mut term) == 0;
        let mut ws = winsize(0).unwrap_or(libc::winsize { ws_row: 40, ws_col: 120, ws_xpixel: 0, ws_ypixel: 0 });
        let termp = if have_term { &mut term as *mut _ } else { std::ptr::null_mut() };
        if libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), termp, &mut ws) != 0 {
            return Err(format!("couldn't open a terminal: {}", std::io::Error::last_os_error()));
        }
    }
    let child = unsafe {
        let s = slave;
        Command::new(program)
            .args(&args[1..])
            .stdin(Stdio::from_raw_fd(libc::dup(s)))
            .stdout(Stdio::from_raw_fd(libc::dup(s)))
            .stderr(Stdio::from_raw_fd(libc::dup(s)))
            .env("GRANDADDY_RUN", "1")
            .pre_exec(move || {
                // own session + controlling terminal, so ctrl-c and resizes reach the agent
                libc::setsid();
                libc::ioctl(0, TIOCSCTTY, 0);
                Ok(())
            })
            .spawn()
    };
    unsafe { libc::close(slave) };
    let mut child = child.map_err(|e| format!("couldn't start {program}: {e}"))?;

    let id = format!("{}{}", (now() * 1000.0) as u64 % 100_000_000, std::process::id() % 1000);
    let tty = unsafe {
        let p = libc::ttyname(0);
        if p.is_null() { String::new() } else { std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned() }
    };
    let run = Run {
        id: id.clone(),
        pid: child.id() as i64,
        host_pid: std::process::id() as i64,
        tool,
        command: args.to_vec(),
        cwd: std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
        tty,
        started: now(),
        sock: sock_path(&id),
    };
    let reg = dir().join(format!("{id}.json"));
    let _ = util::write_json(&reg, &run);

    let writer = Arc::new(Mutex::new(unsafe { File::from_raw_fd(libc::dup(master)) }));
    let paste_mode = Arc::new(AtomicBool::new(false));
    let _ = std::fs::remove_file(&run.sock);
    if let Ok(listener) = UnixListener::bind(&run.sock) {
        unsafe { libc::chmod(std::ffi::CString::new(run.sock.clone()).unwrap().as_ptr(), 0o600) };
        let (w, pm) = (writer.clone(), paste_mode.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let (w, pm) = (w.clone(), pm.clone());
                std::thread::spawn(move || serve(conn, w, pm));
            }
        });
    }

    let raw = RawMode::enter(0);
    let mut reader = unsafe { File::from_raw_fd(libc::dup(master)) };
    let mut stdout = std::io::stdout();
    let mut stdin = unsafe { File::from_raw_fd(libc::dup(0)) };
    let mut last_ws = winsize(0);
    let mut buf = [0u8; 16384];
    let mut stdin_open = true;
    loop {
        let mut fds = [
            libc::pollfd { fd: reader.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: if stdin_open { stdin.as_raw_fd() } else { -1 }, events: libc::POLLIN, revents: 0 },
        ];
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, 200) };
        if let Some(ws) = winsize(0) {
            if last_ws.map(|l| l.ws_col != ws.ws_col || l.ws_row != ws.ws_row).unwrap_or(true) {
                unsafe { libc::ioctl(master, TIOCSWINSZ, &ws) };
                last_ws = Some(ws);
            }
        }
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break, // the agent exited
                Ok(k) => {
                    let chunk = &buf[..k];
                    if contains(chunk, b"\x1b[?2004h") {
                        paste_mode.store(true, Ordering::Relaxed);
                    } else if contains(chunk, b"\x1b[?2004l") {
                        paste_mode.store(false, Ordering::Relaxed);
                    }
                    let _ = stdout.write_all(chunk);
                    let _ = stdout.flush();
                }
            }
        }
        if stdin_open && fds[1].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => stdin_open = false,
                Ok(k) => {
                    let _ = writer.lock().unwrap().write_all(&buf[..k]);
                }
            }
        }
    }
    drop(raw);
    let code = child.wait().map(|s| s.code().unwrap_or(1)).unwrap_or(1);
    util::remove(&reg);
    let _ = std::fs::remove_file(&run.sock);
    unsafe { libc::close(master) };
    Ok(code)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn serve(conn: UnixStream, writer: Arc<Mutex<File>>, paste_mode: Arc<AtomicBool>) {
    let mut out = match conn.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut line = String::new();
    if BufReader::new(conn).read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<serde_json::Value>(&line) {
        Ok(v) => {
            let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
            let submit = v.get("submit").and_then(|t| t.as_bool()).unwrap_or(true);
            let mut w = writer.lock().unwrap();
            // a paste keeps multi-line prompts in one message; plain typing otherwise
            let body = if paste_mode.load(Ordering::Relaxed) { format!("\x1b[200~{text}\x1b[201~") } else { text.replace('\n', " ") };
            let ok = w.write_all(body.as_bytes()).is_ok();
            if submit && ok {
                drop(w);
                std::thread::sleep(Duration::from_millis(120)); // let the TUI take the paste before return
                let _ = writer.lock().unwrap().write_all(b"\r");
            }
            if ok { "ok" } else { "write failed" }
        }
        Err(_) => "bad request",
    };
    let _ = out.write_all(format!("{reply}\n").as_bytes());
}
