//! One list of sessions across tools, in the single shape the CLI, hub and app all use.
use crate::claude::{self, Activity, Request};
use crate::managed;
use crate::providers::{self, Answer};
use crate::util::{self, now};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub key: String,
    /// tool id: claude, codex, antigravity, cursor, gemini, opencode, kilo, …
    pub tool: String,
    /// "Claude Code", "Codex", …
    pub tool_label: String,
    /// CLI, IDE or app
    pub surface: String,
    pub pid: Option<i64>,
    pub sid: Option<String>,
    /// background session id (what `claude attach` takes)
    pub short: Option<String>,
    pub name: String,
    pub cwd: String,
    pub kind: String,
    pub bg: bool,
    pub raw: Option<String>,
    pub status: String,
    pub since: f64,
    pub started: f64,
    pub auto: bool,
    pub pending: Vec<Request>,
    pub transcript: Option<PathBuf>,
    /// where a non-Claude-CLI session's activity is read from
    pub source: Option<String>,
    pub model: Option<String>,
    /// what grandaddy can answer for it: full, permissions or none
    pub answer: Answer,
    pub can_answer: bool,
    /// set when the session runs under `grandaddy run`, so prompts can be typed into it
    pub managed: Option<String>,
    /// stopped on a usage limit: when it lifts, and what grandaddy will do then
    pub limit: Option<crate::claude::Limit>,
    pub limit_plan: Option<String>,
    /// it finished its turn by asking you something and is waiting for your reply
    pub question: Option<Question>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Question {
    /// stable for this particular question, so a popup shows once
    pub id: String,
    /// its whole last reply (Markdown)
    pub text: String,
    pub at: f64,
}

/// The question an idle session ended its turn with, if it did.
fn waiting_question(s: &Session) -> Option<Question> {
    if s.status != "idle" || !FILE_BACKED.contains(&s.tool.as_str()) {
        return None;
    }
    let act = activity(s);
    let last = act.events.iter().rev().find(|e| matches!(e.k.as_str(), "you" | "text" | "tool" | "limit"))?;
    if last.k != "text" {
        return None;
    }
    // the reply ends by asking: a question mark on its last line, outside code
    let tail = last.text.trim_end().trim_end_matches(['*', '_', '`', ')', '"', '\'']);
    let last_line = tail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    let at = last.ts.unwrap_or(0.0);
    (tail.ends_with('?') && !last_line.trim_start().starts_with("```") && util::now() - at < 12.0 * 3600.0).then(|| Question { id: format!("{}@{}", s.key, at as i64), text: last.text.clone(), at })
}

/// Every live session across tools: the Claude Code CLI first, then everything else.
pub fn collect() -> Vec<Session> {
    let auto = util::auto_ids();
    let reqs = claude::pending();
    let runs = managed::list();
    let mut out: Vec<Session> = vec![];
    let t0 = std::time::Instant::now();
    let live = claude::live();
    if std::env::var_os("GRANDADDY_TIMING").is_some() {
        eprintln!("  {:<12} {:>5} ms", "claude cli", t0.elapsed().as_millis());
    }
    for s in live {
        let g = |k: &str| s.get(k).and_then(|v| v.as_str()).map(String::from);
        let f = |k: &str| s.get(k).and_then(|v| v.as_f64());
        let Some(sid) = g("sessionId") else { continue };
        let raw = g("status").or_else(|| g("state"));
        let kind = g("kind").unwrap_or_else(|| "interactive".into());
        out.push(Session {
            key: format!("claude:{sid}"),
            tool: "claude".into(),
            tool_label: "Claude Code".into(),
            surface: if g("entrypoint").as_deref() == Some("claude-desktop") { "app".into() } else { "CLI".into() },
            pid: s.get("pid").and_then(|v| v.as_i64()),
            short: g("id"),
            name: g("name").unwrap_or_else(|| sid.chars().take(8).collect()),
            cwd: g("cwd").unwrap_or_default(),
            bg: kind == "background",
            kind,
            status: claude::status_word(raw.as_deref().unwrap_or("")),
            raw,
            since: f("statusUpdatedAt").or_else(|| f("startedAt")).unwrap_or(0.0) / 1000.0,
            started: f("startedAt").unwrap_or(0.0) / 1000.0,
            auto: false,
            pending: vec![],
            transcript: claude::transcript(&sid),
            source: None,
            model: None,
            answer: Answer::Full,
            can_answer: true,
            managed: None,
            limit: None,
            limit_plan: None,
            question: None,
            sid: Some(sid),
        });
    }
    let seen: HashSet<String> = out.iter().filter_map(|s| s.sid.clone()).collect();
    for f in providers::all() {
        if f.sid.as_ref().map(|s| seen.contains(s)).unwrap_or(false) {
            continue; // the Claude app's session is already listed from the CLI side
        }
        out.push(Session {
            key: f.key,
            tool_label: providers::label(f.tool).into(),
            tool: f.tool.into(),
            surface: f.surface.into(),
            pid: f.pid,
            sid: f.sid,
            short: None,
            name: f.name,
            cwd: f.cwd,
            kind: "interactive".into(),
            bg: false,
            raw: Some(f.status.clone()),
            status: f.status,
            since: f.since,
            started: f.started,
            auto: false,
            pending: vec![],
            transcript: f.source.as_ref().filter(|s| !s.contains('|') && s.starts_with('/')).map(PathBuf::from),
            source: f.source,
            model: f.model,
            can_answer: f.answer != Answer::None,
            answer: f.answer,
            managed: None,
            limit: None,
            limit_plan: None,
            question: None,
        });
    }
    for s in out.iter_mut() {
        if let Some(sid) = &s.sid {
            s.pending = reqs.iter().filter(|r| &r.sid == sid).cloned().collect();
            s.auto = auto.contains(sid) || auto.contains(util::ALL);
        }
        if !s.pending.is_empty() {
            s.status = "needs you".into();
        }
        s.managed = managed::match_session(&runs, s.pid, &s.tool, &s.cwd);
        if s.status != "needs you" {
            if let Some(l) = crate::limits::of(s) {
                if l.resets_at.map(|r| r > util::now() - crate::limits::GRACE * 4.0).unwrap_or(true) {
                    s.limit_plan = Some(crate::limits::plan(s, &l));
                    s.status = "limit".into();
                    s.limit = Some(l);
                }
            }
            if s.status != "limit" {
                s.question = waiting_question(s);
            }
        }
    }
    // flags for sessions that have exited are just clutter
    let live: HashSet<String> = out.iter().filter_map(|s| s.sid.clone()).collect();
    for sid in auto.iter().filter(|s| *s != util::ALL && !live.contains(*s)) {
        util::set_auto(sid, false);
    }
    out
}

pub fn activity(s: &Session) -> Activity {
    if s.tool == "claude" {
        return s.transcript.as_ref().map(|p| claude::activity(p)).unwrap_or_default();
    }
    s.source.as_ref().map(|src| providers::activity(&s.tool, src)).unwrap_or_default()
}

/// Tools whose activity is cheap to re-read (cached file parses), used for conflict checks.
const FILE_BACKED: &[&str] = &["claude", "codex", "antigravity", "gemini", "qwen"];

/// Files this session edited recently that another live session edited too: {path: [names]}.
pub fn conflicts(s: &Session, everyone: &[Session]) -> BTreeMap<String, Vec<String>> {
    let window = 6.0 * 3600.0;
    let recent = |a: &Activity| -> HashMap<String, f64> { a.edited.iter().filter(|(_, t)| now() - **t < window).map(|(f, t)| (f.clone(), *t)).collect() };
    let mine = recent(&activity(s));
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if mine.is_empty() {
        return out;
    }
    for o in everyone.iter().filter(|o| o.key != s.key && FILE_BACKED.contains(&o.tool.as_str())) {
        for f in recent(&activity(o)).keys() {
            if mine.contains_key(f) {
                out.entry(f.clone()).or_default().push(o.name.clone());
            }
        }
    }
    out
}

/// pid | background id | name | session-id prefix | folder; None = sessions in this folder.
pub fn resolve<'a>(target: Option<&str>, sess: &'a [Session], claude_only: bool) -> Vec<&'a Session> {
    let pool: Vec<&Session> = sess.iter().filter(|s| !claude_only || s.tool == "claude").collect();
    let real = |p: &str| std::fs::canonicalize(p).ok();
    let Some(t) = target else {
        let here = std::env::current_dir().ok().and_then(|p| p.canonicalize().ok());
        return pool.into_iter().filter(|s| !s.cwd.is_empty() && real(&s.cwd) == here && here.is_some()).collect();
    };
    let tl = t.to_lowercase();
    let exact: Vec<&Session> = pool
        .iter()
        .copied()
        .filter(|s| {
            s.pid.map(|p| p.to_string()) == Some(tl.clone())
                || s.short.as_deref().map(str::to_lowercase) == Some(tl.clone())
                || s.sid.as_deref().map(str::to_lowercase) == Some(tl.clone())
                || s.name.to_lowercase() == tl
        })
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let path = real(&util::expand(t));
    pool.into_iter()
        .filter(|s| {
            s.sid.as_deref().map(|x| x.to_lowercase().starts_with(&tl)).unwrap_or(false)
                || s.name.to_lowercase().contains(&tl)
                || Path::new(s.cwd.trim_end_matches('/')).file_name().map(|b| b.to_string_lossy().to_lowercase() == tl).unwrap_or(false)
                || (path.is_some() && real(&s.cwd) == path)
        })
        .collect()
}

// ---------- git context, fetched off the UI thread ----------

#[derive(Debug, Clone, Serialize, Default)]
pub struct Git {
    pub branch: Option<String>,
    pub changed: Option<usize>,
    pub ahead: String,
}

/// folder -> (when looked up, summary)
type GitCache = Option<HashMap<String, (f64, Option<Git>)>>;
static GIT: Mutex<GitCache> = Mutex::new(None);
static GIT_BUSY: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn fetch_git(cwd: &str) -> Option<Git> {
    let o = Command::new("git").args(["-C", cwd, "status", "--porcelain=v1", "-b", "-uno"]).output().ok()?;
    if !o.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&o.stdout);
    let lines: Vec<&str> = text.lines().collect();
    let head = lines.first().and_then(|l| l.strip_prefix("## ")).unwrap_or("?");
    let branch = head.split("...").next().unwrap_or("?").replace("No commits yet on ", "");
    let ahead = head.split_once('[').map(|(_, r)| r.trim_end_matches(']').to_string()).unwrap_or_default();
    Some(Git { branch: Some(branch), changed: Some(lines.len().saturating_sub(1)), ahead })
}

/// Cached git summary for a folder (None until the first lookup lands, or not a repo).
pub fn git(cwd: &str) -> Option<Git> {
    if cwd.is_empty() {
        return None;
    }
    let hit = GIT.lock().unwrap().as_ref().and_then(|m| m.get(cwd).cloned());
    let stale = hit.as_ref().map(|(t, _)| now() - t > 15.0).unwrap_or(true);
    if stale {
        let mut busy = GIT_BUSY.lock().unwrap();
        let busy = busy.get_or_insert_with(HashSet::new);
        if busy.insert(cwd.to_string()) {
            let c = cwd.to_string();
            std::thread::spawn(move || {
                let g = fetch_git(&c);
                GIT.lock().unwrap().get_or_insert_with(HashMap::new).insert(c.clone(), (now(), g));
                GIT_BUSY.lock().unwrap().get_or_insert_with(HashSet::new).remove(&c);
            });
        }
    }
    hit.and_then(|(_, g)| g)
}
