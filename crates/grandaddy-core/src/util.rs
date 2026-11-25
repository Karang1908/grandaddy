//! Paths, small file helpers, and formatting shared by the CLI and the app.
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const ALL: &str = "ALL";

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub fn state() -> PathBuf {
    home().join(".grandaddy")
}
pub fn auto_dir() -> PathBuf {
    state().join("auto")
}
pub fn pending_dir() -> PathBuf {
    state().join("pending")
}
pub fn answers_dir() -> PathBuf {
    state().join("answers")
}
pub fn log_path() -> PathBuf {
    state().join("activity.log")
}
pub fn claude_dir() -> PathBuf {
    home().join(".claude")
}

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let s = fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

/// Atomic write, so a reader never sees half a file.
pub fn write_json<T: Serialize>(path: &Path, data: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(serde_json::to_string(data)?.as_bytes())?;
    }
    fs::rename(&tmp, path)
}

pub fn remove(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Is this process still running? (EPERM means it exists but isn't ours.)
pub fn alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn auto_ids() -> std::collections::HashSet<String> {
    fs::read_dir(auto_dir())
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default()
}

pub fn set_auto(sid: &str, on: bool) {
    let p = auto_dir().join(sid);
    if on {
        let _ = fs::create_dir_all(auto_dir());
        let _ = fs::File::create(p);
    } else {
        remove(&p);
    }
}

pub fn is_auto(sid: &str) -> bool {
    auto_dir().join(sid).exists() || auto_dir().join(ALL).exists()
}

pub fn log(line: &str) {
    let _ = fs::create_dir_all(state());
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "{}  {}", stamp, one_line(line, 300));
    }
}

pub fn log_tail(n: usize) -> Vec<String> {
    let s = fs::read_to_string(log_path()).unwrap_or_default();
    let lines: Vec<String> = s.lines().map(String::from).collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

/// `~/x` for paths under your home folder.
pub fn short(path: &str) -> String {
    let h = home().to_string_lossy().into_owned();
    if !h.is_empty() && h != "/" && path.starts_with(&h) {
        format!("~{}", &path[h.len()..])
    } else {
        path.to_string()
    }
}

pub fn expand(path: &str) -> String {
    if let Some(rest) = path.strip_prefix('~') {
        format!("{}{}", home().to_string_lossy(), rest)
    } else {
        path.to_string()
    }
}

pub fn ago(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// Collapse whitespace and cut to `n` characters.
pub fn one_line(text: &str, n: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(n).collect()
}

pub fn tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.0}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// HH:MM today, otherwise "25 Nov".
pub fn clock(ts: Option<f64>) -> String {
    use chrono::{Local, TimeZone};
    let Some(ts) = ts else { return "     ".into() };
    let Some(t) = Local.timestamp_opt(ts as i64, 0).single() else { return "     ".into() };
    if t.date_naive() == Local::now().date_naive() {
        t.format("%H:%M").to_string()
    } else {
        t.format("%d %b").to_string()
    }
}

/// Today's local date as YYYY-MM-DD (the key `claude::usage` groups by).
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

pub fn parse_ts(ts: &serde_json::Value) -> Option<f64> {
    let s = ts.as_str()?;
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis() as f64 / 1000.0)
}

pub fn file_mtime(path: &Path) -> Option<f64> {
    let m = fs::metadata(path).ok()?.modified().ok()?;
    Some(m.duration_since(UNIX_EPOCH).ok()?.as_secs_f64())
}

/// 18 random bytes from the OS as 24 URL-safe base64 characters.
pub fn random_token() -> String {
    let mut buf = [0u8; 18];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut buf);
    }
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(24);
    for c in buf.chunks(3) {
        let n = (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
        for shift in [18, 12, 6, 0] {
            out.push(A[((n >> shift) & 63) as usize] as char);
        }
    }
    out
}
