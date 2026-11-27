//! Sessions from coding tools other than the Claude Code CLI. Each provider reads the tool's own
//! files read-only (logs, transcripts, SQLite) and reports recent sessions in one shape.
//!
//! | tool        | sessions from                                          | surfaces        |
//! |-------------|--------------------------------------------------------|-----------------|
//! | claude app  | ~/Library/Application Support/Claude/claude-code-sessions | app          |
//! | codex       | ~/.codex/sessions rollouts                             | CLI, IDE, app   |
//! | antigravity | ~/.gemini/antigravity{,-cli,-ide}/brain/*/…/transcript | app, CLI, IDE   |
//! | gemini/qwen | ~/.gemini/tmp/*/chats, ~/.qwen/tmp/*/chats             | CLI             |
//! | opencode    | ~/.local/share/opencode/opencode.db                    | CLI             |
//! | kilo        | ~/.local/share/kilo/kilo.db                            | CLI             |
//! | cursor      | Cursor's state.vscdb composers                         | IDE             |
//! | others      | the process table (aider, amp, goose, crush, droid…)   | CLI             |
use crate::claude::Activity;
use crate::util::{self, now};
use serde::Serialize;
use std::path::Path;

pub mod antigravity;
pub mod claude_app;
pub mod codex;
pub mod cursor;
pub mod gemini;
pub mod opencode;
pub mod procs;

/// Sessions untouched for longer than this aren't listed as live.
pub const RECENT: f64 = 3.0 * 3600.0;
/// A session whose log changed this recently is treated as working.
pub const ACTIVE: f64 = 25.0;

/// What grandaddy can answer for this tool, given its hook support.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    /// permission prompts and questions (Claude Code)
    Full,
    /// allow / deny only (Codex, Antigravity, Cursor, Gemini)
    Permissions,
    /// nothing: answer in the tool itself
    None,
}

#[derive(Debug, Clone)]
pub struct Found {
    pub key: String,
    pub tool: &'static str,
    /// CLI, IDE or app
    pub surface: &'static str,
    pub sid: Option<String>,
    pub pid: Option<i64>,
    pub name: String,
    pub cwd: String,
    pub status: String,
    pub since: f64,
    pub started: f64,
    pub model: Option<String>,
    /// where the activity lives (a path, or "db|id" for SQLite-backed tools)
    pub source: Option<String>,
    pub answer: Answer,
}

pub fn label(tool: &str) -> &'static str {
    match tool {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "antigravity" => "Antigravity",
        "gemini" => "Gemini CLI",
        "qwen" => "Qwen Code",
        "opencode" => "OpenCode",
        "kilo" => "Kilo",
        "cursor" => "Cursor",
        "aider" => "Aider",
        "amp" => "Amp",
        "goose" => "Goose",
        "crush" => "Crush",
        "droid" => "Droid",
        "copilot" => "Copilot CLI",
        _ => "Agent",
    }
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

/// What our hook last learned about a session (cwd, model) — fills gaps in tools whose logs lack it.
pub fn seen(tool: &str, sid: &str) -> Option<serde_json::Value> {
    util::read_json(&util::state().join("seen").join(format!("{tool}-{sid}.json")))
}

pub fn remember(tool: &str, sid: &str, cwd: Option<&str>, model: Option<&str>) {
    if sid.is_empty() {
        return;
    }
    let p = util::state().join("seen").join(format!("{tool}-{}.json", sid.replace('/', "_")));
    let _ = util::write_json(&p, &serde_json::json!({"cwd": cwd, "model": model, "updated": now()}));
}

/// Every recent session from every supported tool except the Claude Code CLI.
pub fn all() -> Vec<Found> {
    let timing = std::env::var_os("GRANDADDY_TIMING").is_some();
    let mut t0 = std::time::Instant::now();
    let mut lap = |what: &str| {
        if timing {
            eprintln!("  {what:<12} {:>5} ms", t0.elapsed().as_millis());
        }
        t0 = std::time::Instant::now();
    };
    let procs = procs::processes();
    lap("processes");
    let mut out = vec![];
    out.extend(claude_app::sessions());
    lap("claude app");
    out.extend(codex::sessions(&procs));
    lap("codex");
    out.extend(antigravity::sessions(&procs));
    lap("antigravity");
    out.extend(gemini::sessions(&procs));
    lap("gemini");
    out.extend(opencode::sessions(&procs));
    lap("opencode");
    out.extend(cursor::sessions(&procs));
    lap("cursor");
    // live processes of known tools that no log above accounted for
    for p in procs {
        let claimed = out.iter().any(|f| f.pid == Some(p.pid) || (f.tool == p.tool && !f.cwd.is_empty() && f.cwd == p.cwd && f.status == "working"));
        if p.tool != "claude" && !claimed && !out.iter().any(|f| f.pid == Some(p.pid)) {
            let base = Path::new(p.cwd.trim_end_matches('/')).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            out.push(Found {
                key: format!("{}:{}", p.tool, p.pid),
                tool: p.tool,
                surface: "CLI",
                sid: None,
                pid: Some(p.pid),
                name: if base.is_empty() { label(p.tool).to_string() } else { base },
                cwd: p.cwd,
                status: "running".into(),
                since: p.started,
                started: p.started,
                model: None,
                source: None,
                answer: Answer::None,
            });
        }
    }
    out
}

pub fn activity(tool: &str, source: &str) -> Activity {
    match tool {
        "codex" => codex::activity(Path::new(source)),
        "antigravity" => antigravity::activity(Path::new(source)),
        "gemini" | "qwen" => gemini::activity(Path::new(source)),
        "opencode" | "kilo" => opencode::activity(source),
        "cursor" => cursor::activity(source),
        "claude" => crate::claude::activity(Path::new(source)),
        _ => Activity::default(),
    }
}

/// Run sqlite3 read-only and parse its JSON output (macOS ships sqlite3; no C build needed).
pub fn sqlite_json(db: &str, sql: &str) -> Vec<serde_json::Map<String, serde_json::Value>> {
    let out = std::process::Command::new("sqlite3").args(["-readonly", "-json", db, sql]).output();
    match out {
        Ok(o) if o.status.success() && !o.stdout.is_empty() => serde_json::from_slice(&o.stdout).unwrap_or_default(),
        _ => vec![],
    }
}

/// Newest modification time of a SQLite database including its WAL.
pub fn db_mtime(db: &str) -> f64 {
    let a = util::file_mtime(Path::new(db)).unwrap_or(0.0);
    let b = util::file_mtime(Path::new(&format!("{db}-wal"))).unwrap_or(0.0);
    a.max(b)
}
