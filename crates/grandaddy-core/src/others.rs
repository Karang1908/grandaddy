//! Other coding CLIs. Found from the process table; Codex also gets status + activity from its
//! rollout log.
use crate::claude::{Activity, Event};
use crate::util::{self, one_line, parse_ts};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

pub const TOOLS: &[&str] = &["codex", "gemini", "opencode", "aider", "cursor-agent", "amp", "goose", "qwen"];

#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: i64,
    pub ppid: i64,
    pub tool: String,
    pub tty: String,
    pub started: f64,
    pub cwd: String,
}

/// ps etime: [[dd-]hh:]mm:ss
fn etime_seconds(e: &str) -> Option<f64> {
    let (days, rest) = match e.split_once('-') {
        Some((d, r)) => (d.parse::<f64>().ok()?, r),
        None => (0.0, e),
    };
    let mut parts: Vec<f64> = rest.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    while parts.len() < 3 {
        parts.insert(0, 0.0);
    }
    Some(days * 86400.0 + parts[0] * 3600.0 + parts[1] * 60.0 + parts[2])
}

fn tool_of(args: &str) -> Option<String> {
    args.split_whitespace().take(2).find_map(|tok| {
        let b = Path::new(tok).file_name()?.to_str()?;
        TOOLS.contains(&b).then(|| b.to_string())
    })
}

static CWD: Mutex<Option<HashMap<i64, String>>> = Mutex::new(None);

pub fn cwd_of(pid: i64) -> String {
    if let Some(c) = CWD.lock().unwrap().as_ref().and_then(|m| m.get(&pid)) {
        return c.clone();
    }
    let cwd = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().find_map(|l| l.strip_prefix('n').map(String::from)))
        .unwrap_or_default();
    CWD.lock().unwrap().get_or_insert_with(HashMap::new).insert(pid, cwd.clone());
    cwd
}

/// Interactive (has a terminal) processes of known coding CLIs, wrapper processes collapsed.
pub fn processes() -> Vec<Proc> {
    let Ok(o) = Command::new("ps").args(["-axo", "pid=,ppid=,tty=,etime=,args="]).output() else { return vec![] };
    let now = util::now();
    let mut found: HashMap<i64, Proc> = HashMap::new();
    for line in String::from_utf8_lossy(&o.stdout).lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        let args = parts[4..].join(" ");
        let Some(tool) = tool_of(&args) else { continue };
        let tty = parts[2].to_string();
        if tty == "??" || tty == "-" {
            continue;
        }
        let (Ok(pid), Ok(ppid), Some(age)) = (parts[0].parse(), parts[1].parse(), etime_seconds(parts[3])) else { continue };
        found.insert(pid, Proc { pid, ppid, tool, tty, started: now - age, cwd: String::new() });
    }
    // a node wrapper and the native binary it spawns are one session: keep the outer one
    let mut procs: Vec<Proc> = found
        .values()
        .filter(|p| !found.get(&p.ppid).map(|parent| parent.tool == p.tool).unwrap_or(false))
        .cloned()
        .collect();
    let live: HashSet<i64> = found.keys().copied().collect();
    if let Some(m) = CWD.lock().unwrap().as_mut() {
        m.retain(|pid, _| live.contains(pid));
    }
    for p in procs.iter_mut() {
        p.cwd = cwd_of(p.pid);
    }
    procs.sort_by(|a, b| a.started.partial_cmp(&b.started).unwrap_or(std::cmp::Ordering::Equal));
    procs
}

// ---------- Codex rollout logs ----------

fn codex_sessions() -> PathBuf {
    util::home().join(".codex").join("sessions")
}

/// Most recently written rollout file for this folder since the process started.
pub fn codex_rollout(cwd: &str, started: f64) -> Option<PathBuf> {
    use chrono::{Duration, Local, TimeZone};
    if cwd.is_empty() {
        return None;
    }
    let base = codex_sessions();
    let mut day = Local.timestamp_opt((started - 86400.0) as i64, 0).single()?.date_naive();
    let today = Local::now().date_naive();
    let mut files: Vec<PathBuf> = vec![];
    while day <= today {
        let dir = base.join(day.format("%Y/%m/%d").to_string());
        if let Ok(rd) = fs::read_dir(dir) {
            files.extend(rd.flatten().map(|e| e.path()).filter(|p| {
                p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with("rollout-") && n.ends_with(".jsonl")).unwrap_or(false)
            }));
        }
        day += Duration::days(1);
    }
    files.sort_by(|a, b| util::file_mtime(b).partial_cmp(&util::file_mtime(a)).unwrap_or(std::cmp::Ordering::Equal));
    for f in files {
        if util::file_mtime(&f).unwrap_or(0.0) < started - 120.0 {
            break;
        }
        let first = fs::read_to_string(&f).ok().and_then(|s| s.lines().next().map(String::from)).unwrap_or_default();
        if let Ok(meta) = serde_json::from_str::<Value>(&first) {
            if meta.pointer("/payload/cwd").and_then(|v| v.as_str()) == Some(cwd) {
                return Some(f);
            }
        }
    }
    None
}

fn text_of(content: &Value) -> String {
    match content {
        Value::Array(a) => a.iter().filter_map(|c| c.get("text").and_then(|v| v.as_str())).collect::<Vec<_>>().join(" "),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// (activity, state) where state is working/idle from task_started/task_complete.
pub fn codex_activity(path: &Path) -> (Activity, Option<String>) {
    let mut act = Activity { mtime: util::file_mtime(path), ..Default::default() };
    let mut state = None;
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    for raw in &lines[lines.len().saturating_sub(3000)..] {
        let Ok(d) = serde_json::from_str::<Value>(raw) else { continue };
        let typ = d.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let p = d.get("payload").cloned().unwrap_or(Value::Null);
        let pt = p.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ts = d.get("timestamp").and_then(parse_ts);
        match (typ, pt) {
            ("event_msg", "task_started") => state = Some("working".to_string()),
            ("event_msg", "task_complete") => state = Some("idle".to_string()),
            ("turn_context", _) => {
                if let Some(m) = p.get("model").and_then(|v| v.as_str()) {
                    act.model = Some(m.to_string());
                }
            }
            ("response_item", "message") => {
                let t = text_of(p.get("content").unwrap_or(&Value::Null)).trim().to_string();
                if t.is_empty() || t.starts_with('<') {
                    continue; // injected environment / instructions
                }
                let k = if p.get("role").and_then(|v| v.as_str()) == Some("user") { "you" } else { "text" };
                act.events.push(Event { k: k.into(), text: t, tool: None, desc: None, ts });
            }
            ("response_item", "function_call" | "custom_tool_call" | "local_shell_call") => {
                let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("shell").to_string();
                let mut arg = p.get("arguments").or_else(|| p.get("input")).or_else(|| p.get("action")).cloned().unwrap_or(Value::Null);
                if let Value::String(s) = &arg {
                    if let Ok(v) = serde_json::from_str::<Value>(s) {
                        arg = v;
                    }
                }
                let shown = match &arg {
                    Value::Object(o) => match o.get("command").or_else(|| o.get("cmd")) {
                        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" "),
                        Some(Value::String(s)) => s.clone(),
                        _ => arg.to_string(),
                    },
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                act.events.push(Event { k: "tool".into(), text: one_line(&shown, 300), tool: Some(name), desc: None, ts });
            }
            _ => {}
        }
    }
    let n = act.events.len();
    act.events.drain(..n.saturating_sub(150));
    (act, state)
}
