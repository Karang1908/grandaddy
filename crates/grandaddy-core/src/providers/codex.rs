//! Codex: every client (CLI, IDE extension, desktop app) writes a rollout log per thread under
//! ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl. The first line is session_meta (id, cwd,
//! originator, source); task_started / task_complete events give the state.
use super::{name_from, procs, status_from_mtime, Answer, Found, RECENT};
use crate::claude::{Activity, Event};
use crate::util::{self, now, one_line, parse_ts};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn root() -> PathBuf {
    util::home().join(".codex").join("sessions")
}

struct Meta {
    id: String,
    cwd: String,
    surface: &'static str,
    started: f64,
}

/// rollout -> (id, cwd, surface, started), or None if its first line isn't session_meta
type MetaCache = Option<HashMap<PathBuf, Option<(String, String, &'static str, f64)>>>;
static META: Mutex<MetaCache> = Mutex::new(None);

fn meta(path: &Path) -> Option<Meta> {
    if let Some(hit) = META.lock().unwrap().as_ref().and_then(|m| m.get(path)) {
        return hit.clone().map(|(id, cwd, surface, started)| Meta { id, cwd, surface, started });
    }
    let first = fs::read_to_string(path).ok().and_then(|s| s.lines().next().map(String::from)).unwrap_or_default();
    let parsed = serde_json::from_str::<Value>(&first).ok().and_then(|d| {
        let p = d.get("payload")?;
        // the id is in session_meta; failing that, a rollout's file name ends in it
        // (rollout-<time>-<uuid>.jsonl), and any other name is still a stable id
        let from_name = || {
            let stem = path.file_stem()?.to_str()?;
            Some(stem.get(stem.len().saturating_sub(36)..).filter(|_| stem.len() > 36).unwrap_or(stem).to_string())
        };
        let id = p.get("id").or_else(|| p.get("session_id")).and_then(|v| v.as_str()).map(String::from).or_else(from_name)?;
        let cwd = p.get("cwd").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let origin = p.get("originator").and_then(|v| v.as_str()).unwrap_or("");
        let source = p.get("source").and_then(|v| v.as_str()).unwrap_or("");
        let surface = if source == "vscode" || origin.contains("vscode") {
            "IDE"
        } else if origin.contains("desktop") || origin.contains("app") || source == "app" {
            "app"
        } else {
            "CLI"
        };
        let started = p.get("timestamp").and_then(parse_ts).or_else(|| d.get("timestamp").and_then(parse_ts)).unwrap_or(0.0);
        Some((id, cwd, surface, started))
    });
    META.lock().unwrap().get_or_insert_with(HashMap::new).insert(path.to_path_buf(), parsed.clone());
    parsed.map(|(id, cwd, surface, started)| Meta { id, cwd, surface, started })
}

/// Rollouts written in the last few hours (they live in the folder of the day they started,
/// so look back a week of folders).
fn recent_rollouts() -> Vec<(PathBuf, f64)> {
    use chrono::{Duration, Local};
    let mut out = vec![];
    let today = Local::now().date_naive();
    for back in 0..7 {
        let day = today - Duration::days(back);
        let Ok(rd) = fs::read_dir(root().join(day.format("%Y/%m/%d").to_string())) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                if let Some(m) = util::file_mtime(&p).filter(|m| now() - m < RECENT) {
                    out.push((p, m));
                }
            }
        }
    }
    out
}

pub fn sessions(procs: &[procs::Proc]) -> Vec<Found> {
    let mut out = vec![];
    for (path, mtime) in recent_rollouts() {
        let Some(m) = meta(&path) else { continue };
        let (act, state) = parse(&path);
        let mut status = state.unwrap_or_else(|| status_from_mtime(mtime));
        if status == "working" && now() - mtime > 600.0 {
            status = "idle".into(); // a task_started with nothing since: the client went away
        }
        let name = act.events.iter().find(|e| e.k == "you").map(|e| name_from(&e.text)).unwrap_or_else(|| "Codex".into());
        let pid = procs::find(procs, "codex", &m.cwd).map(|p| p.pid);
        if m.surface == "CLI" && pid.is_none() {
            continue; // the terminal session has exited, even if the log never said it finished
        }
        out.push(Found {
            key: format!("codex:{}", m.id),
            tool: "codex",
            surface: m.surface,
            sid: Some(m.id),
            pid,
            name,
            cwd: m.cwd,
            status,
            since: mtime,
            started: m.started,
            model: act.model,
            source: Some(path.to_string_lossy().into_owned()),
            answer: Answer::Permissions,
        });
    }
    out
}

fn text_of(content: &Value) -> String {
    match content {
        Value::Array(a) => a.iter().filter_map(|c| c.get("text").and_then(|v| v.as_str())).collect::<Vec<_>>().join(" "),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// rollout -> ((mtime, size) it was parsed at, (activity, working/idle))
type ParseCache = Option<HashMap<PathBuf, ((f64, u64), (Activity, Option<String>))>>;
static PARSED: Mutex<ParseCache> = Mutex::new(None);

/// (activity, state) where state is working/idle from task_started/task_complete.
pub fn parse(path: &Path) -> (Activity, Option<String>) {
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let key = (util::file_mtime(path).unwrap_or(0.0), size);
    if let Some((k, v)) = PARSED.lock().unwrap().as_ref().and_then(|m| m.get(path)) {
        if *k == key {
            return v.clone();
        }
    }
    let mut act = Activity { mtime: Some(key.0), ..Default::default() };
    let mut state = None;
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    for raw in &lines[lines.len().saturating_sub(4000)..] {
        let Ok(d) = serde_json::from_str::<Value>(raw) else { continue };
        let typ = d.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let p = d.get("payload").cloned().unwrap_or(Value::Null);
        let pt = p.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ts = d.get("timestamp").and_then(parse_ts);
        match (typ, pt) {
            ("event_msg", "task_started") => {
                state = Some("working".to_string());
                act.limit = None;
            }
            ("event_msg", "error") => {
                let msg = p.get("message").and_then(|v| v.as_str()).unwrap_or("error").to_string();
                let lower = msg.to_lowercase();
                if lower.contains("usage limit") || lower.contains("hit your") {
                    let at = ts.unwrap_or_else(now);
                    act.limit = Some(crate::claude::Limit { kind: crate::limits::kind_from_text(&msg), resets_at: crate::limits::parse_reset(&msg, at), text: msg.clone(), at });
                    act.events.push(Event { k: "limit".into(), text: msg, tool: None, desc: None, ts });
                } else {
                    act.events.push(Event { k: "error".into(), text: one_line(&msg, 300), tool: None, desc: None, ts });
                }
            }
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
                        _ => o.get("path").or_else(|| o.get("file_path")).and_then(|v| v.as_str()).map(util::short).unwrap_or_else(|| arg.to_string()),
                    },
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if name == "apply_patch" {
                    for line in shown.lines().filter(|l| l.starts_with("*** Update File:") || l.starts_with("*** Add File:")) {
                        let f = line.split_once(':').map(|(_, f)| f.trim()).unwrap_or("");
                        act.edited.insert(f.to_string(), ts.unwrap_or(0.0));
                    }
                }
                act.events.push(Event { k: "tool".into(), text: one_line(&shown, 300), tool: Some(name), desc: None, ts });
            }
            _ => {}
        }
    }
    let n = act.events.len();
    act.events.drain(..n.saturating_sub(150));
    let v = (act, state);
    PARSED.lock().unwrap().get_or_insert_with(HashMap::new).insert(path.to_path_buf(), (key, v.clone()));
    v
}

pub fn activity(path: &Path) -> Activity {
    parse(path).0
}
