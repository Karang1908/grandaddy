//! Claude Code: live sessions, what they're doing, token usage, past sessions, and the
//! requests our hook holds open.
use crate::util::{self, now, one_line, parse_ts, read_json, write_json};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

pub const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Claude Code's per-session status (interactive) or state (background) -> ours.
pub fn status_word(raw: &str) -> String {
    match raw {
        "busy" | "shell" | "working" | "running" => "working",
        "waiting" => "needs you",
        "idle" | "done" | "stopped" => "idle",
        // background sessions that can't run (login required, network blocked, API error)
        "blocked" => "blocked",
        "failed" => "failed",
        "" => "?",
        other => return other.to_string(),
    }
    .to_string()
}

fn sessions_dir() -> PathBuf {
    util::claude_dir().join("sessions")
}
pub fn projects_dir() -> PathBuf {
    util::claude_dir().join("projects")
}

// ---------- live sessions ----------

/// Interactive sessions from ~/.claude/sessions/<pid>.json whose process is alive.
pub fn from_files() -> Vec<Map<String, Value>> {
    let mut out = vec![];
    let Ok(rd) = fs::read_dir(sessions_dir()) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(Value::Object(s)) = read_json::<Value>(&p) else { continue };
        let ok = s.get("sessionId").and_then(|v| v.as_str()).is_some()
            && s.get("pid").and_then(|v| v.as_i64()).map(util::alive).unwrap_or(false);
        if ok {
            out.push(s);
        }
    }
    out
}

/// (when fetched, `claude agents --json` result)
type AgentsCache = Option<(f64, Option<Vec<Map<String, Value>>>)>;
static AGENTS: Mutex<AgentsCache> = Mutex::new(None);

/// Running sessions, oldest first. `claude agents --json` is the official list (it also has
/// background sessions); the session files fill in what it lacks (statusUpdatedAt).
pub fn live() -> Vec<Map<String, Value>> {
    let mut files: HashMap<String, Map<String, Value>> = from_files()
        .into_iter()
        .filter_map(|s| Some((s.get("sessionId")?.as_str()?.to_string(), s)))
        .collect();
    let agents = {
        let mut g = AGENTS.lock().unwrap();
        let stale = g.as_ref().map(|(t, _)| now() - t > 1.5).unwrap_or(true);
        if stale {
            let data = Command::new("claude")
                .args(["agents", "--json"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| serde_json::from_slice::<Vec<Map<String, Value>>>(&o.stdout).ok());
            *g = Some((now(), data));
        }
        g.as_ref().and_then(|(_, d)| d.clone()).unwrap_or_default()
    };
    let mut out = vec![];
    for a in agents {
        let Some(sid) = a.get("sessionId").and_then(|v| v.as_str()).map(String::from) else { continue };
        if let Some(pid) = a.get("pid").and_then(|v| v.as_i64()) {
            if !util::alive(pid) {
                continue;
            }
        }
        let mut merged = files.remove(&sid).unwrap_or_default();
        for (k, v) in a {
            merged.insert(k, v);
        }
        out.push(merged);
    }
    out.extend(files.into_values());
    out.sort_by(|a, b| {
        let f = |m: &Map<String, Value>| m.get("startedAt").and_then(|v| v.as_f64()).unwrap_or(0.0);
        f(a).partial_cmp(&f(b)).unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Fast status lookup for the hook: session files only, no subprocess.
pub fn raw_status(sid: &str) -> Option<String> {
    from_files()
        .into_iter()
        .find(|s| s.get("sessionId").and_then(|v| v.as_str()) == Some(sid))
        .and_then(|s| s.get("status").and_then(|v| v.as_str()).map(String::from))
}

pub fn session_name(sid: &str) -> String {
    from_files()
        .into_iter()
        .find(|s| s.get("sessionId").and_then(|v| v.as_str()) == Some(sid))
        .and_then(|s| s.get("name").and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_else(|| sid.chars().take(8).collect())
}

// ---------- transcripts ----------

static PATHS: Mutex<Option<HashMap<String, PathBuf>>> = Mutex::new(None);

pub fn transcript(sid: &str) -> Option<PathBuf> {
    {
        let g = PATHS.lock().unwrap();
        if let Some(p) = g.as_ref().and_then(|m| m.get(sid)) {
            if p.exists() {
                return Some(p.clone());
            }
        }
    }
    let name = format!("{sid}.jsonl");
    let best = fs::read_dir(projects_dir())
        .ok()?
        .flatten()
        .map(|d| d.path().join(&name))
        .filter(|p| p.exists())
        .max_by(|a, b| util::file_mtime(a).partial_cmp(&util::file_mtime(b)).unwrap_or(std::cmp::Ordering::Equal))?;
    PATHS.lock().unwrap().get_or_insert_with(HashMap::new).insert(sid.to_string(), best.clone());
    Some(best)
}

fn tail_lines(path: &Path, nbytes: u64) -> Vec<String> {
    let Ok(mut f) = fs::File::open(path) else { return vec![] };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = size.saturating_sub(nbytes);
    let _ = f.seek(SeekFrom::Start(start));
    let mut data = vec![];
    let _ = f.read_to_end(&mut data);
    let text = String::from_utf8_lossy(&data);
    let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // cut in half
    }
    lines
}

/// The one argument worth showing for a tool call.
pub fn subject(name: &str, inp: &Value) -> String {
    for k in ["command", "file_path", "notebook_path", "pattern", "url", "query", "description", "skill", "prompt", "path"] {
        if let Some(v) = inp.get(k).and_then(|v| v.as_str()) {
            if !v.trim().is_empty() {
                return if matches!(k, "file_path" | "notebook_path" | "path") { util::short(v) } else { one_line(v, 300) };
            }
        }
    }
    if name == "AskUserQuestion" {
        if let Some(q) = inp.pointer("/questions/0/question").and_then(|v| v.as_str()) {
            return one_line(q, 300);
        }
    }
    String::new()
}

pub fn tool_summary(name: &str, inp: &Value) -> String {
    let s = subject(name, inp);
    if s.is_empty() { name.to_string() } else { format!("{name}  {s}") }
}

fn user_text(text: &str) -> Option<String> {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("<command-name>") {
        return Some(rest.split("</command-name>").next().unwrap_or("").trim().to_string());
    }
    if t.is_empty() || t.starts_with('<') {
        return None; // system-injected context, not something you typed
    }
    Some(t.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    /// you | text | tool | error | mark
    pub k: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
    pub ts: Option<f64>,
}

impl Event {
    fn new(k: &str, text: String, ts: Option<f64>) -> Self {
        Event { k: k.into(), text, tool: None, desc: None, ts }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Activity {
    pub events: Vec<Event>,
    pub model: Option<String>,
    pub context: Option<u64>,
    pub mtime: Option<f64>,
    /// file path -> when it was last edited
    pub edited: BTreeMap<String, f64>,
    /// set when the newest thing in the session is a usage-limit stop
    pub limit: Option<Limit>,
}

/// A session stopped by a plan's usage limit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Limit {
    /// five_hour | seven_day | session | usage
    pub kind: String,
    /// when it lifts (epoch seconds), if known
    pub resets_at: Option<f64>,
    /// the agent's own words, e.g. "You've hit your 5-hour limit · resets 4:30am (Asia/Dubai)"
    pub text: String,
    /// when the session hit it
    pub at: f64,
}

/// transcript -> ((mtime, size) it was parsed at, result)
type ParseCache = Option<HashMap<PathBuf, ((f64, u64), Activity)>>;
static PARSED: Mutex<ParseCache> = Mutex::new(None);

/// Recent activity from the transcript tail (cached until the file changes).
pub fn activity(path: &Path) -> Activity {
    let Ok(meta) = fs::metadata(path) else { return Activity::default() };
    let key = (util::file_mtime(path).unwrap_or(0.0), meta.len());
    if let Some((k, a)) = PARSED.lock().unwrap().as_ref().and_then(|m| m.get(path)) {
        if *k == key {
            return a.clone();
        }
    }
    let mut act = Activity { mtime: Some(key.0), ..Default::default() };
    let mut events = vec![];
    for raw in tail_lines(path, 800_000) {
        let Ok(d) = serde_json::from_str::<Value>(&raw) else { continue };
        if d.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let t = d.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ts = d.get("timestamp").and_then(parse_ts);
        let msg = d.get("message").cloned().unwrap_or(Value::Null);
        let content = msg.get("content").cloned().unwrap_or(Value::Null);
        if d.get("isCompactSummary").and_then(|v| v.as_bool()) == Some(true) {
            events.push(Event::new("mark", "conversation compacted".into(), ts));
        } else if d.get("isMeta").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        } else if t == "user" {
            let items: Vec<Value> = match &content {
                Value::String(s) => vec![serde_json::json!({"type": "text", "text": s})],
                Value::Array(a) => a.clone(),
                _ => vec![],
            };
            for c in items {
                match c.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        if let Some(u) = user_text(c.get("text").and_then(|v| v.as_str()).unwrap_or("")) {
                            act.limit = None; // you've written since; a new limit hit would set it again
                            events.push(Event::new("you", u, ts));
                        }
                    }
                    Some("tool_result") if c.get("is_error").and_then(|v| v.as_bool()) == Some(true) => {
                        let body = match c.get("content") {
                            Some(Value::String(s)) => s.clone(),
                            Some(Value::Array(a)) => a.iter().filter_map(|x| x.get("text").and_then(|v| v.as_str())).collect::<Vec<_>>().join(" "),
                            _ => "error".into(),
                        };
                        events.push(Event::new("error", one_line(&body, 300), ts));
                    }
                    _ => {}
                }
            }
        } else if t == "assistant" && d.get("error").and_then(|v| v.as_str()) == Some("rate_limit") {
            let text = match &content {
                Value::Array(a) => a.iter().filter_map(|x| x.get("text").and_then(|v| v.as_str())).collect::<Vec<_>>().join(" "),
                Value::String(s) => s.clone(),
                _ => String::new(),
            };
            let q = d.get("quotaLimits").or_else(|| d.pointer("/apiErrorParams/rate_limit_info"));
            let at = ts.unwrap_or_else(now);
            let resets_at = q.and_then(|q| q.get("resetsAt")).and_then(|v| v.as_f64()).or_else(|| crate::limits::parse_reset(&text, at));
            let kind = q.and_then(|q| q.get("rateLimitType")).and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| crate::limits::kind_from_text(&text));
            events.push(Event::new("limit", text.clone(), ts));
            act.limit = Some(Limit { kind, resets_at, text, at });
        } else if t == "assistant" {
            let Value::Array(items) = content else { continue };
            act.limit = None; // it's working again
            if let Some(m) = msg.get("model").and_then(|v| v.as_str()) {
                act.model = Some(m.to_string());
            }
            if let Some(u) = msg.get("usage").filter(|u| u.is_object()) {
                let g = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                act.context = Some(g("input_tokens") + g("cache_read_input_tokens") + g("cache_creation_input_tokens"));
            }
            for c in items {
                match c.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        let txt = c.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
                        if !txt.is_empty() {
                            events.push(Event::new("text", txt.to_string(), ts));
                        }
                    }
                    Some("tool_use") => {
                        let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string();
                        let inp = c.get("input").cloned().unwrap_or(Value::Null);
                        let desc = if name == "Bash" { inp.get("description").and_then(|v| v.as_str()).map(String::from) } else { None };
                        if EDIT_TOOLS.contains(&name.as_str()) {
                            if let Some(fp) = inp.get("file_path").or_else(|| inp.get("notebook_path")).and_then(|v| v.as_str()) {
                                act.edited.insert(fp.to_string(), ts.unwrap_or(0.0));
                            }
                        }
                        events.push(Event { k: "tool".into(), text: subject(&name, &inp), tool: Some(name), desc, ts });
                    }
                    _ => {}
                }
            }
        }
    }
    let n = events.len();
    act.events = events.into_iter().skip(n.saturating_sub(150)).collect();
    PARSED.lock().unwrap().get_or_insert_with(HashMap::new).insert(path.to_path_buf(), (key, act.clone()));
    act
}

/// Hints that a session might need a look: gone quiet while working, or repeating itself.
pub fn signals(act: &Activity, status: &str) -> Vec<String> {
    let mut out = vec![];
    if status == "working" {
        if let Some(m) = act.mtime {
            if now() - m > 300.0 {
                out.push(format!("quiet {}", util::ago(now() - m)));
            }
        }
    }
    let tools: Vec<(&Option<String>, &String)> = act.events.iter().filter(|e| e.k == "tool").map(|e| (&e.tool, &e.text)).collect();
    if tools.len() >= 4 {
        let last = &tools[tools.len() - 4..];
        if last.iter().all(|t| *t == last[0]) {
            out.push("looping?".into());
        }
    }
    out
}

// ---------- token usage (incremental, cached on disk) ----------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct Tokens {
    #[serde(rename = "in")]
    pub input: u64,
    pub out: u64,
    pub cache: u64,
}
impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.out + self.cache
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct UsageEntry {
    offset: u64,
    days: BTreeMap<String, [u64; 3]>,
    project: Option<String>,
}

fn usage_cache_path() -> PathBuf {
    util::state().join("usage-cache.json")
}
static USAGE_LOCK: Mutex<()> = Mutex::new(());

fn all_transcripts() -> Vec<PathBuf> {
    let mut out = vec![];
    if let Ok(rd) = fs::read_dir(projects_dir()) {
        for d in rd.flatten() {
            if let Ok(files) = fs::read_dir(d.path()) {
                out.extend(files.flatten().map(|f| f.path()).filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl")));
            }
        }
    }
    out
}

/// Tokens per day per project for transcripts touched in the last `days` days.
/// {day: {project: Tokens}}. Reads only bytes added since the last call.
pub fn usage(days: u32) -> BTreeMap<String, BTreeMap<String, Tokens>> {
    let _g = USAGE_LOCK.lock().unwrap();
    let mut cache: HashMap<String, UsageEntry> = read_json(&usage_cache_path()).unwrap_or_default();
    let cutoff = now() - days as f64 * 86400.0;
    let mut out: BTreeMap<String, BTreeMap<String, Tokens>> = BTreeMap::new();
    for path in all_transcripts() {
        let Some(mtime) = util::file_mtime(&path) else { continue };
        if mtime < cutoff {
            continue;
        }
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let key = path.to_string_lossy().into_owned();
        let mut ent = cache.get(&key).cloned().filter(|e| e.offset <= size).unwrap_or_default();
        if ent.offset < size {
            if let Ok(mut f) = fs::File::open(&path) {
                let _ = f.seek(SeekFrom::Start(ent.offset));
                let mut data = vec![];
                let _ = f.read_to_end(&mut data);
                let end = data.iter().rposition(|&b| b == b'\n').map(|i| i + 1).unwrap_or(0);
                for raw in data[..end].split(|&b| b == b'\n') {
                    let has = |needle: &[u8]| raw.windows(needle.len()).any(|w| w == needle);
                    if !has(b"\"usage\"") {
                        if ent.project.is_none() && has(b"\"cwd\"") {
                            if let Ok(d) = serde_json::from_slice::<Value>(raw) {
                                ent.project = d.get("cwd").and_then(|v| v.as_str()).map(String::from);
                            }
                        }
                        continue;
                    }
                    let Ok(d) = serde_json::from_slice::<Value>(raw) else { continue };
                    if ent.project.is_none() {
                        ent.project = d.get("cwd").and_then(|v| v.as_str()).map(String::from);
                    }
                    let u = d.pointer("/message/usage");
                    if d.get("type").and_then(|v| v.as_str()) != Some("assistant") || u.is_none() {
                        continue;
                    }
                    let u = u.unwrap();
                    let g = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                    let day = d.get("timestamp").and_then(parse_ts).map(day_of).unwrap_or_else(|| "?".into());
                    let e = ent.days.entry(day).or_insert([0, 0, 0]);
                    e[0] += g("input_tokens");
                    e[1] += g("output_tokens");
                    e[2] += g("cache_read_input_tokens") + g("cache_creation_input_tokens");
                }
                ent.offset += end as u64;
            }
            cache.insert(key.clone(), ent.clone());
        }
        let proj = ent.project.clone().unwrap_or_else(|| "?".into());
        for (day, [i, o, c]) in &ent.days {
            let t = out.entry(day.clone()).or_default().entry(proj.clone()).or_default();
            t.input += i;
            t.out += o;
            t.cache += c;
        }
    }
    let _ = write_json(&usage_cache_path(), &cache);
    out
}

fn day_of(ts: f64) -> String {
    use chrono::{Local, TimeZone};
    Local.timestamp_opt(ts as i64, 0).single().map(|t| t.format("%Y-%m-%d").to_string()).unwrap_or_else(|| "?".into())
}

/// Total tokens for one transcript, from the usage cache (None until usage() has seen it).
pub fn session_tokens(path: &Path) -> Option<Tokens> {
    let cache: HashMap<String, UsageEntry> = read_json(&usage_cache_path())?;
    let ent = cache.get(path.to_string_lossy().as_ref())?;
    let mut t = Tokens::default();
    for [i, o, c] in ent.days.values() {
        t.input += i;
        t.out += o;
        t.cache += c;
    }
    Some(t)
}

// ---------- past sessions ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Past {
    pub sid: String,
    pub path: String,
    pub cwd: Option<String>,
    pub prompt: Option<String>,
    pub mtime: f64,
    pub entry: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct HistEntry {
    size: u64,
    mtime: f64,
    cwd: Option<String>,
    prompt: Option<String>,
    entry: Option<String>,
}

/// cwd, the first thing you typed, and how the session was started.
fn first_prompt(path: &Path) -> (Option<String>, Option<String>, Option<String>) {
    let (mut cwd, mut prompt, mut entry, mut command) = (None, None, None, None);
    let Ok(f) = fs::File::open(path) else { return (cwd, prompt, entry) };
    let mut budget: i64 = 2_000_000; // session-start context can push the first prompt far down
    for raw in std::io::BufReader::new(f).split(b'\n').map_while(Result::ok) {
        budget -= raw.len() as i64;
        if budget < 0 {
            break;
        }
        if matches!(&entry, Some(e) if e != "cli") {
            break; // started by a tool: hidden from history anyway
        }
        let has_user = raw.windows(6).any(|w| w == b"\"user\"");
        if !has_user && cwd.is_some() && entry.is_some() {
            continue;
        }
        let Ok(d) = serde_json::from_slice::<Value>(&raw) else { continue };
        if cwd.is_none() {
            cwd = d.get("cwd").and_then(|v| v.as_str()).map(String::from);
        }
        if entry.is_none() {
            entry = d.get("entrypoint").and_then(|v| v.as_str()).map(String::from);
        }
        let is = |k: &str| d.get(k).and_then(|v| v.as_bool()) == Some(true);
        if prompt.is_none() && d.get("type").and_then(|v| v.as_str()) == Some("user") && !is("isMeta") && !is("isCompactSummary") {
            let texts: Vec<String> = match d.pointer("/message/content") {
                Some(Value::String(s)) => vec![s.clone()],
                Some(Value::Array(a)) => a
                    .iter()
                    .filter(|x| x.get("type").and_then(|v| v.as_str()) == Some("text"))
                    .filter_map(|x| x.get("text").and_then(|v| v.as_str()).map(String::from))
                    .collect(),
                _ => vec![],
            };
            for t in texts {
                match user_text(&t) {
                    Some(u) if u.starts_with('/') => {
                        command.get_or_insert(u); // /resume, /model…: only if nothing better follows
                    }
                    Some(u) => {
                        prompt = Some(one_line(&u, 300));
                        break;
                    }
                    None => {}
                }
            }
        }
        if cwd.is_some() && prompt.is_some() && entry.is_some() {
            break;
        }
    }
    (cwd, prompt.or(command), entry)
}

/// Past sessions you started yourself, newest first. Tool-started ones (entrypoint sdk-*) are
/// left out. Cached by size + mtime.
pub fn history() -> Vec<Past> {
    let cpath = util::state().join("history-cache.json");
    let mut cache: HashMap<String, HistEntry> = read_json(&cpath).unwrap_or_default();
    let mut changed = false;
    let mut out = vec![];
    for path in all_transcripts() {
        let Ok(meta) = fs::metadata(&path) else { continue };
        let mtime = util::file_mtime(&path).unwrap_or(0.0);
        let key = path.to_string_lossy().into_owned();
        let fresh = cache.get(&key).map(|e| e.size == meta.len() && e.mtime == mtime).unwrap_or(false);
        if !fresh {
            let (cwd, prompt, entry) = first_prompt(&path);
            cache.insert(key.clone(), HistEntry { size: meta.len(), mtime, cwd, prompt, entry });
            changed = true;
        }
        let e = &cache[&key];
        if e.prompt.is_none() || matches!(&e.entry, Some(x) if x != "cli") {
            continue;
        }
        let sid = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        out.push(Past { sid, path: key, cwd: e.cwd.clone(), prompt: e.prompt.clone(), mtime: e.mtime, entry: e.entry.clone() });
    }
    if changed {
        let _ = write_json(&cpath, &cache);
    }
    out.sort_by(|a, b| b.mtime.partial_cmp(&a.mtime).unwrap_or(std::cmp::Ordering::Equal));
    out
}

// ---------- requests held open by our hook ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub sid: String,
    pub tool: String,
    pub input: Value,
    #[serde(default)]
    pub cwd: Option<String>,
    pub created: f64,
    pub hook_pid: i64,
    /// which agent asked: claude, codex, antigravity, cursor, gemini
    #[serde(default = "claude_agent")]
    pub agent: String,
}

fn claude_agent() -> String {
    "claude".into()
}

impl Request {
    pub fn questions(&self) -> Vec<Value> {
        self.input.get("questions").and_then(|v| v.as_array()).cloned().unwrap_or_default()
    }
    pub fn is_question(&self) -> bool {
        self.tool == "AskUserQuestion"
    }
}

/// Open requests, oldest first; drops ones whose hook process has gone.
pub fn pending() -> Vec<Request> {
    let mut out: Vec<Request> = vec![];
    let Ok(rd) = fs::read_dir(util::pending_dir()) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(r) = read_json::<Request>(&p) else { continue };
        if !util::alive(r.hook_pid) {
            util::remove(&p);
            continue;
        }
        out.push(r);
    }
    out.sort_by(|a, b| a.created.partial_cmp(&b.created).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// What the hub/CLI/app/phone decided, in the answer file the hook polls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Answers { answers: Map<String, Value> },
}

pub fn answer(req: &Request, d: &Decision) -> std::io::Result<()> {
    write_json(&util::answers_dir().join(format!("{}.json", req.id)), d)
}

pub fn describe_request(req: &Request) -> String {
    if req.is_question() {
        let q = req.input.pointer("/questions/0/question").and_then(|v| v.as_str()).unwrap_or("a question");
        format!("asks: {}", one_line(q, 200))
    } else {
        format!("wants: {}", tool_summary(&req.tool, &req.input))
    }
}
