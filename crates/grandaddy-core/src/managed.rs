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

/// One request to a session's host; returns its one-line reply.
fn request(id: &str, msg: serde_json::Value) -> Result<String, String> {
    let run = get(id).ok_or("that session has ended")?;
    let mut s = UnixStream::connect(&run.sock).map_err(|e| format!("couldn't reach the session: {e}"))?;
    s.set_write_timeout(Some(Duration::from_secs(3))).ok();
    s.write_all((msg.to_string() + "\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let _ = BufReader::new(s).read_line(&mut reply);
    Ok(reply.trim().to_string())
}

/// Starts the error `send` returns when the session's input box already holds text.
pub const UNSENT: &str = "The session already has unsent text in its input box";

/// Type `text` into a managed session (and press return if `submit`). Text already typed in the
/// session's input box is replaced if `replace`; otherwise the send is refused with [`UNSENT`].
pub fn send(id: &str, text: &str, submit: bool, replace: bool) -> Result<(), String> {
    let ack = request(id, serde_json::json!({"text": text, "submit": submit, "replace": replace}))?;
    match ack.strip_prefix("unsent:") {
        _ if ack == "ok" => Ok(()),
        Some(there) => Err(format!("{UNSENT}: “{}”", util::one_line(there, 80))),
        None => Err(format!("the session didn't accept it: {ack}")),
    }
}

/// Press keys in the session exactly as given (Esc is "\x1b", Shift+Tab "\x1b[Z").
pub fn keys(id: &str, keys: &str) -> Result<(), String> {
    let ack = request(id, serde_json::json!({"keys": keys}))?;
    if ack == "ok" { Ok(()) } else { Err(format!("the session didn't accept it: {ack}")) }
}

/// What's on the session's screen right now, as text.
pub fn screen(id: &str) -> Result<String, String> {
    let reply = request(id, serde_json::json!({"screen": true}))?;
    serde_json::from_str::<serde_json::Value>(&reply).ok().and_then(|v| v.get("screen")?.as_str().map(String::from)).ok_or_else(|| "the session didn't answer".into())
}

/// Claude Code's permission modes, as its footer names them, in Shift+Tab order.
pub const MODES: &[(&str, &str)] = &[("auto", "auto mode on"), ("manual", "manual mode on"), ("acceptEdits", "accept edits on"), ("plan", "plan mode on"), ("bypassPermissions", "bypass permissions on")];

/// The mode Claude Code's footer shows on this screen.
pub fn mode_of(screen: &str) -> Option<&'static str> {
    screen.lines().rev().take(8).find_map(|l| MODES.iter().find(|(_, footer)| l.contains(footer)).map(|(m, _)| *m))
}

/// The effort level Claude Code's footer shows ("◐ medium · /effort").
pub fn effort_of(screen: &str) -> Option<String> {
    screen.lines().rev().take(14).find_map(|l| {
        let i = l.find("· /effort")?;
        l[..i].split_whitespace().last().map(String::from)
    })
}

/// Switch the session to `target` by pressing Shift+Tab and reading the footer until it shows it.
pub fn set_mode(id: &str, target: &str) -> Result<String, String> {
    if !MODES.iter().any(|(m, _)| *m == target) {
        return Err(format!("unknown mode {target}"));
    }
    for _ in 0..MODES.len() + 1 {
        let now_mode = mode_of(&screen(id)?);
        if now_mode == Some(target) {
            return Ok(format!("Switched to {}", label(target)));
        }
        keys(id, "\x1b[Z")?;
        std::thread::sleep(Duration::from_millis(450));
    }
    Err(format!("{} isn't available in this session", label(target)))
}

pub fn label(mode: &str) -> &'static str {
    match mode {
        "auto" => "auto mode",
        "manual" => "manual mode",
        "acceptEdits" => "accept edits",
        "plan" => "plan mode",
        "bypassPermissions" => "bypass permissions",
        _ => "that mode",
    }
}

/// `claude` uses that don't need a terminal of their own run directly, untouched.
fn passthrough(args: &[String]) -> bool {
    const SUBCOMMANDS: &[&str] = &[
        "agents", "attach", "auth", "auto-mode", "doctor", "gateway", "import", "install", "logs", "mcp", "plugin", "plugins", "purge", "respawn",
        "rm", "setup-token", "stop", "kill", "update", "config", "migrate-installer",
    ];
    let interactive = unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 };
    let nested = std::env::var_os("GRANDADDY_RUN").is_some();
    let flags = args.iter().skip(1).any(|a| matches!(a.as_str(), "-p" | "--print" | "-v" | "--version" | "-h" | "--help" | "--bg" | "--background"));
    let sub = args.get(1).map(|a| SUBCOMMANDS.contains(&a.as_str())).unwrap_or(false);
    !interactive || nested || flags || sub
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
    let Some(program) = args.first() else { return Err("usage: grandaddy run claude [args…]".into()) };
    if passthrough(args) {
        let err = Command::new(program).args(&args[1..]).exec(); // only returns on failure
        return Err(format!("couldn't start {program}: {err}"));
    }
    let tool = procs::tool_of(program).unwrap_or("agent").to_string();
    let first = winsize(0).unwrap_or(libc::winsize { ws_row: 40, ws_col: 120, ws_xpixel: 0, ws_ypixel: 0 });
    let nz = |v: u16, d: u16| std::num::NonZeroU16::new(v).unwrap_or(std::num::NonZeroU16::new(d).unwrap());
    let screen = Arc::new(Mutex::new(vt100::Parser::new(nz(first.ws_row, 40), nz(first.ws_col, 120), 0)));
    let (mut master, mut slave): (RawFd, RawFd) = (-1, -1);
    unsafe {
        let mut term: libc::termios = std::mem::zeroed();
        let have_term = libc::tcgetattr(0, &mut term) == 0;
        let mut ws = winsize(0).unwrap_or(libc::winsize { ws_row: 40, ws_col: 120, ws_xpixel: 0, ws_ypixel: 0 });
        let termp = if have_term { &mut term as *mut _ } else { std::ptr::null_mut() };
        if libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), termp, &mut ws) != 0 {
            return Err(format!("couldn't open a terminal: {}", std::io::Error::last_os_error()));
        }
        // the agent gets the terminal through its stdio copies only. If it inherited the master
        // too, it would hold its own terminal open: closing the window would never hang it up,
        // and it would run on headless after grandaddy is gone
        libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(slave, libc::F_SETFD, libc::FD_CLOEXEC);
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
        let (w, pm, sc) = (writer.clone(), paste_mode.clone(), screen.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let (w, pm, sc) = (w.clone(), pm.clone(), sc.clone());
                std::thread::spawn(move || serve(conn, w, pm, sc));
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
                screen.lock().unwrap().set_size(nz(ws.ws_row, 40), nz(ws.ws_col, 120));
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
                    screen.lock().unwrap().process(chunk);
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

/// What's typed in Claude Code's input box (the rows between its last two rules), without the
/// dim placeholder hint it shows when the box is empty.
fn typed(p: &vt100::Parser) -> String {
    let sc = p.screen();
    let (rows, cols) = sc.size();
    let (rows, cols) = (rows.get(), cols.get());
    let row = |r: u16, all: bool| -> String {
        (0..cols)
            .filter_map(|c| sc.cell(r, c))
            .filter(|c| !c.is_wide_continuation())
            .map(|c| if !all && c.dim() { " " } else if c.has_contents() { c.contents() } else { " " })
            .collect::<String>()
    };
    let rules: Vec<u16> = (0..rows).filter(|&r| row(r, true).trim_start().starts_with("────────────")).collect();
    let [.., top, bottom] = rules[..] else { return String::new() };
    let text = (top + 1..bottom).map(|r| row(r, false).trim().to_string()).collect::<Vec<_>>().join("\n");
    text.trim().trim_start_matches('❯').trim_matches(|c: char| c.is_whitespace()).to_string()
}

fn serve(conn: UnixStream, writer: Arc<Mutex<File>>, paste_mode: Arc<AtomicBool>, screen: Arc<Mutex<vt100::Parser>>) {
    let mut out = match conn.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut line = String::new();
    if BufReader::new(conn).read_line(&mut line).is_err() {
        return;
    }
    let reply: String = match serde_json::from_str::<serde_json::Value>(&line) {
        Ok(v) if v.get("screen").is_some() => serde_json::json!({"screen": screen.lock().unwrap().screen().contents()}).to_string(),
        Ok(v) if v.get("keys").is_some() => {
            let k = v.get("keys").and_then(|t| t.as_str()).unwrap_or("");
            if writer.lock().unwrap().write_all(k.as_bytes()).is_ok() { "ok".into() } else { "write failed".into() }
        }
        Ok(v) => {
            let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
            let submit = v.get("submit").and_then(|t| t.as_bool()).unwrap_or(true);
            let replace = v.get("replace").and_then(|t| t.as_bool()).unwrap_or(false);
            let now_typed = || typed(&screen.lock().unwrap());
            // never glue a message onto what's already in the box (a draft, or the prompt Claude
            // puts back after an interrupt): replace it only when asked to
            let there = now_typed();
            if !there.is_empty() {
                if !replace {
                    let _ = out.write_all(format!("unsent:{}\n", there.replace('\n', " ")).as_bytes());
                    return;
                }
                // ctrl-k, ctrl-u and backspace clear a line and join the one above; repeat until empty
                for _ in 0..60 {
                    let _ = writer.lock().unwrap().write_all(b"\x0b\x15\x08");
                    std::thread::sleep(Duration::from_millis(60));
                    if now_typed().is_empty() {
                        break;
                    }
                }
                if !now_typed().is_empty() {
                    let _ = out.write_all(b"couldn't clear the session's input box\n");
                    return;
                }
            }
            let mut w = writer.lock().unwrap();
            // a slash command is typed, so the command menu sees it; a message is pasted, which
            // keeps a multi-line prompt in one message
            let command = text.starts_with('/') && !text.contains('\n');
            let body = if !command && paste_mode.load(Ordering::Relaxed) { format!("\x1b[200~{text}\x1b[201~") } else { text.replace('\n', " ") };
            let ok = w.write_all(body.as_bytes()).is_ok();
            if submit && ok {
                drop(w);
                // let the TUI take it all in before return
                std::thread::sleep(Duration::from_millis(250 + (text.len() as u64 / 40).min(1500)));
                let _ = writer.lock().unwrap().write_all(b"\r");
                // answer now: a command like /exit ends the session, and this host with it
                let _ = out.write_all(b"ok\n");
                // the TUI can drop a return that lands while it's still busy; if the text is still
                // sitting in the box, press it again
                let start: String = text.split_whitespace().take(4).collect::<Vec<_>>().join(" ");
                for _ in 0..3 {
                    std::thread::sleep(Duration::from_millis(600));
                    let left = now_typed();
                    if left.is_empty() || !left.split_whitespace().collect::<Vec<_>>().join(" ").starts_with(&start) {
                        break;
                    }
                    let _ = writer.lock().unwrap().write_all(b"\r");
                }
                return;
            }
            if ok { "ok".into() } else { "write failed".into() }
        }
        Err(_) => "bad request".into(),
    };
    let _ = out.write_all(format!("{reply}\n").as_bytes());
}
