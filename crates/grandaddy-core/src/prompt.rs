//! Sending a prompt to a session, and bringing its window forward.
//!
//! Delivery, best first:
//! 1. sessions started with `grandaddy run` — typed straight into their terminal
//! 2. iTerm — `write text` into the session that owns the tty
//! 3. tmux — `send-keys` to the pane that owns the tty
//! 4. Terminal.app — `do script … in` the tab that owns the tty
//! 5. anything else (Ghostty, VS Code, Cursor, …) — copied to the clipboard and the window is
//!    brought forward, so you paste it. Reported as such, never as "sent".
use crate::launch;
use crate::managed;
use crate::sessions::Session;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, serde::Serialize)]
pub struct Delivery {
    /// typed | clipboard
    pub how: String,
    pub message: String,
}

fn tty_of(pid: i64) -> Option<String> {
    let o = Command::new("ps").args(["-o", "tty=", "-p", &pid.to_string()]).output().ok()?;
    let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (!t.is_empty() && t != "??" && t != "-").then(|| format!("/dev/{t}"))
}

fn osa_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn osa(script: &str) -> bool {
    Command::new("osascript").args(["-e", script]).output().map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "sent").unwrap_or(false)
}

fn tmux_pane(tty: &str) -> Option<String> {
    let o = Command::new("tmux").args(["list-panes", "-a", "-F", "#{pane_tty} #{pane_id}"]).output().ok()?;
    String::from_utf8_lossy(&o.stdout).lines().find_map(|l| {
        let (t, id) = l.split_once(' ')?;
        (t == tty).then(|| id.to_string())
    })
}

pub fn copy_to_clipboard(text: &str) -> bool {
    let Ok(mut c) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() else { return false };
    let ok = c.stdin.take().map(|mut i| i.write_all(text.as_bytes()).is_ok()).unwrap_or(false);
    ok && c.wait().map(|s| s.success()).unwrap_or(false)
}

/// Can a prompt be typed into this session (rather than copied)?
pub fn typable(s: &Session) -> bool {
    if s.managed.is_some() {
        return true;
    }
    let Some(pid) = s.pid else { return false };
    let Some(tty) = tty_of(pid) else { return false };
    matches!(launch::owner_app(pid), Some("iTerm") | Some("Terminal")) || tmux_pane(&tty).is_some()
}

/// Send `text` to the session. `focus` also brings its window forward.
pub fn send(s: &Session, text: &str, focus: bool) -> Result<Delivery, String> {
    if text.trim().is_empty() {
        return Err("type a prompt first".into());
    }
    let typed = |msg: String| {
        if focus {
            let _ = open(s);
        }
        Ok(Delivery { how: "typed".into(), message: msg })
    };
    if let Some(id) = &s.managed {
        managed::send(id, text, true)?;
        return typed(format!("Sent to {}", s.name));
    }
    if let Some(pid) = s.pid {
        if let Some(tty) = tty_of(pid) {
            if let Some(pane) = tmux_pane(&tty) {
                let ok = Command::new("tmux").args(["send-keys", "-t", &pane, "-l", text]).status().map(|s| s.success()).unwrap_or(false)
                    && Command::new("tmux").args(["send-keys", "-t", &pane, "Enter"]).status().map(|s| s.success()).unwrap_or(false);
                if ok {
                    return typed(format!("Sent to {} (tmux)", s.name));
                }
            }
            match launch::owner_app(pid) {
                Some("iTerm") => {
                    let script = format!(
                        "tell application \"iTerm\"\n repeat with w in windows\n repeat with t in tabs of w\n repeat with x in sessions of t\n if tty of x is {} then\n tell x to write text {}\n return \"sent\"\n end if\n end repeat\n end repeat\n end repeat\nend tell",
                        osa_quote(&tty),
                        osa_quote(text)
                    );
                    if osa(&script) {
                        return typed(format!("Sent to {} (iTerm)", s.name));
                    }
                }
                Some("Terminal") => {
                    let script = format!(
                        "tell application \"Terminal\"\n repeat with w in windows\n repeat with t in tabs of w\n if tty of t is {} then\n do script {} in t\n return \"sent\"\n end if\n end repeat\n end repeat\nend tell",
                        osa_quote(&tty),
                        osa_quote(text)
                    );
                    if osa(&script) {
                        return typed(format!("Sent to {} (Terminal)", s.name));
                    }
                }
                _ => {}
            }
        }
    }
    if !copy_to_clipboard(text) {
        return Err("couldn't reach that session or the clipboard".into());
    }
    let _ = open(s);
    let hint = if s.tool == "claude" && s.surface == "CLI" {
        " — start it with `grandaddy run claude` next time and prompts go straight in"
    } else {
        ""
    };
    Ok(Delivery { how: "clipboard".into(), message: format!("Copied — paste it into {}{hint}", s.name) })
}

/// Bring the session's own window forward (or attach to a background session).
pub fn open(s: &Session) -> Result<String, String> {
    if let Some(run) = s.managed.as_ref().and_then(|id| managed::get(id)) {
        let (ok, msg) = launch::jump(run.host_pid);
        return if ok { Ok(msg) } else { Err(msg) };
    }
    if s.bg {
        if let Some(id) = &s.short {
            let (ok, msg) = launch::open_terminal(&format!("claude attach {id}"), Some(&s.cwd));
            return if ok { Ok(msg) } else { Err(msg) };
        }
    }
    if let Some(pid) = s.pid {
        let (ok, msg) = launch::jump(pid);
        return if ok { Ok(msg) } else { Err(msg) };
    }
    let app = match (s.tool.as_str(), s.surface.as_str()) {
        ("cursor", _) => "Cursor",
        ("antigravity", "IDE") => "Antigravity IDE",
        ("antigravity", "app") => "Antigravity",
        ("codex", "IDE") => "Visual Studio Code",
        ("codex", "app") => "Codex",
        ("claude", "app") => "Claude",
        _ => return Err(format!("{} isn't running in a window grandaddy can find", s.name)),
    };
    let ok = Command::new("open").args(["-a", app]).status().map(|s| s.success()).unwrap_or(false);
    if ok { Ok(format!("Opened {app}")) } else { Err(format!("couldn't open {app}")) }
}
