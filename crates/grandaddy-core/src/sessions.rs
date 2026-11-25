//! One list of sessions across tools, in the single shape the CLI, hub and app all use.
use crate::claude::{self, Activity, Request};
use crate::others;
use crate::util::{self, now};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub key: String,
    pub tool: String,
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
    pub can_answer: bool,
}

/// Every live session: Claude Code first (full detail), then other CLIs.
pub fn collect() -> Vec<Session> {
    let auto = util::auto_ids();
    let reqs = claude::pending();
    let mut out = vec![];
    for s in claude::live() {
        let g = |k: &str| s.get(k).and_then(|v| v.as_str()).map(String::from);
        let f = |k: &str| s.get(k).and_then(|v| v.as_f64());
        let Some(sid) = g("sessionId") else { continue };
        let mine: Vec<Request> = reqs.iter().filter(|r| r.sid == sid).cloned().collect();
        let raw = g("status").or_else(|| g("state"));
        let kind = g("kind").unwrap_or_else(|| "interactive".into());
        let status = if !mine.is_empty() { "needs you".to_string() } else { claude::status_word(raw.as_deref().unwrap_or("")) };
        out.push(Session {
            key: format!("claude:{sid}"),
            tool: "claude".into(),
            pid: s.get("pid").and_then(|v| v.as_i64()),
            short: g("id"),
            name: g("name").unwrap_or_else(|| sid.chars().take(8).collect()),
            cwd: g("cwd").unwrap_or_default(),
            bg: kind == "background",
            kind,
            raw,
            status,
            since: f("statusUpdatedAt").or_else(|| f("startedAt")).unwrap_or(0.0) / 1000.0,
            started: f("startedAt").unwrap_or(0.0) / 1000.0,
            auto: auto.contains(&sid) || auto.contains(util::ALL),
            pending: mine,
            transcript: claude::transcript(&sid),
            can_answer: true,
            sid: Some(sid),
        });
    }
    for p in others::processes() {
        let (mut state, mut rollout) = (None, None);
        if p.tool == "codex" {
            rollout = others::codex_rollout(&p.cwd, p.started);
            if let Some(r) = &rollout {
                state = others::codex_activity(r).1;
            }
        }
        let base = Path::new(p.cwd.trim_end_matches('/')).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "?".into());
        out.push(Session {
            key: format!("{}:{}", p.tool, p.pid),
            name: format!("{} · {}", p.tool, base),
            tool: p.tool,
            pid: Some(p.pid),
            sid: None,
            short: None,
            cwd: p.cwd,
            kind: "interactive".into(),
            bg: false,
            status: state.clone().unwrap_or_else(|| "running".into()),
            raw: state,
            since: p.started,
            started: p.started,
            auto: false,
            pending: vec![],
            transcript: rollout,
            can_answer: false,
        });
    }
    // flags for sessions that have exited are just clutter
    let live: HashSet<String> = out.iter().filter_map(|s| s.sid.clone()).collect();
    for sid in auto.iter().filter(|s| *s != util::ALL && !live.contains(*s)) {
        util::set_auto(sid, false);
    }
    out
}

pub fn activity(s: &Session) -> Activity {
    match (&s.transcript, s.tool.as_str()) {
        (Some(p), "claude") => claude::activity(p),
        (Some(p), _) => others::codex_activity(p).0,
        _ => Activity::default(),
    }
}

/// Files this session edited recently that another live session edited too: {path: [names]}.
pub fn conflicts(s: &Session, everyone: &[Session]) -> BTreeMap<String, Vec<String>> {
    let window = 6.0 * 3600.0;
    let recent = |a: &Activity| -> HashMap<String, f64> { a.edited.iter().filter(|(_, t)| now() - **t < window).map(|(f, t)| (f.clone(), *t)).collect() };
    let mine = recent(&activity(s));
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if mine.is_empty() {
        return out;
    }
    for o in everyone.iter().filter(|o| o.key != s.key && o.tool == "claude") {
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
