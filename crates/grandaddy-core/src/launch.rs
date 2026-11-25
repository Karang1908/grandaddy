//! Start, resume, stop, open and jump to sessions. Starting goes through Claude Code's own
//! `claude --bg` (background sessions), so new sessions show up like any other.
use crate::util;
use std::path::Path;
use std::process::{Command, Stdio};

/// (ok, combined output)
pub fn claude(args: &[&str], cwd: Option<&str>) -> (bool, String) {
    let mut c = Command::new("claude");
    c.args(args).stdin(Stdio::null());
    if let Some(d) = cwd.filter(|d| Path::new(d).is_dir()) {
        c.current_dir(d);
    }
    match c.output() {
        Ok(o) => {
            let out = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            (o.status.success(), out.trim().to_string())
        }
        Err(e) => (false, format!("couldn't run claude: {e}")),
    }
}

/// Start a background session. `worktree`: None = no, Some("") = unnamed, Some(name).
pub fn new(cwd: &str, prompt: &str, worktree: Option<&str>, model: Option<&str>) -> (bool, String) {
    if !Path::new(cwd).is_dir() {
        return (false, format!("no such folder: {cwd}"));
    }
    let mut args = vec!["--bg"];
    if let Some(w) = worktree {
        args.push("--worktree");
        if !w.is_empty() {
            args.push(w);
        }
    }
    if let Some(m) = model.filter(|m| !m.is_empty()) {
        args.extend(["--model", m]);
    }
    args.push(prompt);
    claude(&args, Some(cwd))
}

/// Continue a session in the background (Claude starts a copy if it's still running).
pub fn resume(sid: &str, cwd: Option<&str>, prompt: Option<&str>) -> (bool, String) {
    let mut args = vec!["--bg", "--resume", sid];
    if let Some(p) = prompt.filter(|p| !p.is_empty()) {
        args.push(p);
    }
    claude(&args, cwd)
}

pub fn stop(short: &str) -> (bool, String) {
    claude(&["stop", short], None)
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
fn osa_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn osa(script: &str) -> (bool, String) {
    match Command::new("osascript").args(["-e", script]).output() {
        Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => (false, e.to_string()),
    }
}

/// Which terminal app to open new windows in: the one we're running in, else Terminal.
pub fn terminal_app() -> &'static str {
    match std::env::var("TERM_PROGRAM").as_deref() {
        Ok("ghostty") => "Ghostty",
        Ok("iTerm.app") => "iTerm",
        _ => "Terminal",
    }
}

/// Run `cmd` in a new window of your terminal.
pub fn open_terminal(cmd: &str, cwd: Option<&str>) -> (bool, String) {
    let dir = cwd.map(String::from).unwrap_or_else(|| util::home().to_string_lossy().into_owned());
    let line = format!("cd {} && {}", shell_quote(&dir), cmd);
    match terminal_app() {
        // Ghostty ignores --working-directory together with -e, hence the cd in the command
        "Ghostty" => match Command::new("open").args(["-na", "Ghostty.app", "--args", "-e", "/bin/zsh", "-lc", &line]).status() {
            Ok(s) if s.success() => (true, format!("opened in a new Ghostty window: {cmd}")),
            Ok(_) | Err(_) => (false, "couldn't open Ghostty".into()),
        },
        "iTerm" => {
            let inner = format!("/bin/zsh -lc {}", shell_quote(&line));
            let (ok, err) = osa(&format!("tell application \"iTerm\" to create window with default profile command {}", osa_quote(&inner)));
            (ok, if ok { format!("opened in a new iTerm window: {cmd}") } else { err })
        }
        _ => {
            let (ok, err) = osa(&format!("tell application \"Terminal\"\n do script {}\n activate\nend tell", osa_quote(&line)));
            (ok, if ok { format!("opened in a new Terminal window: {cmd}") } else { err })
        }
    }
}

fn ps(pid: i64, fields: &str) -> String {
    Command::new("ps")
        .args(["-o", fields, "-p", &pid.to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// The terminal app a session runs in, found by walking up its parent processes.
pub fn owner_app(mut pid: i64) -> Option<&'static str> {
    const APPS: &[(&str, &str)] = &[
        ("ghostty", "Ghostty"), ("iTerm2", "iTerm"), ("Terminal", "Terminal"), ("Code Helper", "Visual Studio Code"),
        ("Electron", "Visual Studio Code"), ("Cursor Helper", "Cursor"), ("WezTerm", "WezTerm"), ("wezterm-gui", "WezTerm"),
        ("kitty", "kitty"), ("alacritty", "Alacritty"), ("Warp", "Warp"), ("stable", "Warp"),
    ];
    for _ in 0..30 {
        if pid <= 1 {
            return None;
        }
        let line = ps(pid, "ppid=,comm=");
        let (ppid, comm) = line.split_once(char::is_whitespace)?;
        let name = Path::new(comm.trim()).file_name()?.to_string_lossy().into_owned();
        if let Some((_, app)) = APPS.iter().find(|(p, _)| name == *p || name.starts_with(p)) {
            return Some(app);
        }
        pid = ppid.trim().parse().ok()?;
    }
    None
}

/// Bring the session's terminal to the front; select its exact tab where the app allows it.
pub fn jump(pid: i64) -> (bool, String) {
    let Some(app) = owner_app(pid) else { return (false, "couldn't tell which app this session runs in".into()) };
    let tty = ps(pid, "tty=");
    let dev = (!tty.is_empty() && tty != "??" && tty != "-").then(|| format!("/dev/{tty}"));
    match (app, &dev) {
        ("Terminal", Some(dev)) => {
            let (ok, _) = osa(&format!(
                "tell application \"Terminal\"\n activate\n repeat with w in windows\n repeat with t in tabs of w\n if tty of t is {} then\n set selected of t to true\n set index of w to 1\n end if\n end repeat\n end repeat\nend tell",
                osa_quote(dev)
            ));
            (ok, format!("switched to its Terminal tab ({tty})"))
        }
        ("iTerm", Some(dev)) => {
            let (ok, _) = osa(&format!(
                "tell application \"iTerm\"\n activate\n repeat with w in windows\n repeat with t in tabs of w\n repeat with s in sessions of t\n if tty of s is {} then\n select t\n select w\n end if\n end repeat\n end repeat\n end repeat\nend tell",
                osa_quote(dev)
            ));
            (ok, format!("switched to its iTerm tab ({tty})"))
        }
        _ => {
            let ok = Command::new("open").args(["-a", app]).status().map(|s| s.success()).unwrap_or(false);
            (ok, format!("brought {app} to the front — it's the tab on {}", if tty.is_empty() { "its terminal" } else { &tty }))
        }
    }
}

/// macOS notification (fire and forget).
pub fn notify(title: &str, body: &str) {
    let script = format!("display notification {} with title \"grandaddy\" subtitle {}", osa_quote(body), osa_quote(title));
    let _ = Command::new("osascript").args(["-e", &script]).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}
