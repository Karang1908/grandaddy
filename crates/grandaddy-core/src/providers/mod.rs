//! Where sessions come from besides `claude agents --json`: Claude Code sessions run by the
//! Claude desktop app, read from its session records. (The process helpers in `procs` are used
//! by `grandaddy run`.)
use crate::util::{self, now};
use serde::Serialize;

pub mod claude_app;
pub mod procs;

/// Sessions untouched for longer than this aren't listed as live.
pub const RECENT: f64 = 3.0 * 3600.0;
/// A session whose log changed this recently is treated as working.
pub const ACTIVE: f64 = 25.0;

/// What grandaddy can answer for a session.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    /// permission prompts and questions
    Full,
}

#[derive(Debug, Clone)]
pub struct Found {
    pub key: String,
    pub tool: &'static str,
    /// CLI or app
    pub surface: &'static str,
    pub sid: Option<String>,
    pub pid: Option<i64>,
    pub name: String,
    pub cwd: String,
    pub status: String,
    pub since: f64,
    pub started: f64,
    pub model: Option<String>,
    /// the transcript
    pub source: Option<String>,
    pub answer: Answer,
}

pub fn label(_tool: &str) -> &'static str {
    "Claude Code"
}

/// Working if the log moved in the last few seconds, idle otherwise.
pub fn status_from_mtime(mtime: f64) -> String {
    if now() - mtime < ACTIVE { "working".into() } else { "idle".into() }
}

/// Trim a prompt down to a usable session name.
pub fn name_from(text: &str) -> String {
    let t = util::one_line(text, 200);
    if t.chars().count() > 60 { format!("{}…", t.chars().take(59).collect::<String>().trim_end()) } else { t }
}

/// Claude Code sessions the CLI list doesn't include: the desktop app's.
pub fn all() -> Vec<Found> {
    claude_app::sessions()
}
