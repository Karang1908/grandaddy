//! Antigravity: the agent app, the IDE and the `agy` CLI each keep conversations under
//! ~/.gemini/<app dir>/brain/<conversationId>/.system_generated/logs/transcript.jsonl — one JSON
//! step per line (USER_INPUT, PLANNER_RESPONSE with tool_calls, GENERIC results, …).
use super::{name_from, procs, seen, status_from_mtime, Answer, Found, RECENT};
use crate::claude::{Activity, Event};
use crate::util::{self, now, one_line, parse_ts};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const DIRS: &[(&str, &str)] = &[("antigravity", "app"), ("antigravity-ide", "IDE"), ("antigravity-cli", "CLI")];

pub fn sessions(procs: &[procs::Proc]) -> Vec<Found> {
    let mut out = vec![];
    for (dir, surface) in DIRS {
        let Ok(rd) = fs::read_dir(util::home().join(".gemini").join(dir).join("brain")) else { continue };
        for e in rd.flatten() {
            let path = e.path().join(".system_generated/logs/transcript.jsonl");
            let Some(mtime) = util::file_mtime(&path).filter(|m| now() - m < RECENT) else { continue };
            let id = e.file_name().to_string_lossy().into_owned();
            let (act, waiting) = parse(&path);
            let known = seen("antigravity", &id);
            let cwd = known.as_ref().and_then(|k| k.get("cwd")?.as_str().map(String::from)).unwrap_or_default();
            let status = if waiting { "needs you".to_string() } else { status_from_mtime(mtime) };
            let pid = if *surface == "CLI" { procs::find(procs, "antigravity", &cwd).map(|p| p.pid) } else { None };
            if *surface == "CLI" && pid.is_none() {
                continue; // `agy` has exited
            }
            out.push(Found {
                key: format!("antigravity:{id}"),
                tool: "antigravity",
                surface,
                name: act.events.iter().find(|e| e.k == "you").map(|e| name_from(&e.text)).unwrap_or_else(|| "Antigravity".into()),
                sid: Some(id),
                pid,
                cwd,
                status,
                since: mtime,
                started: act.events.first().and_then(|e| e.ts).unwrap_or(mtime),
                model: act.model.clone(),
                source: Some(path.to_string_lossy().into_owned()),
                answer: Answer::Permissions,
            });
        }
    }
    out
}

fn strip_tags(s: &str) -> String {
    let s = s.trim();
    let inner = s.strip_prefix("<USER_REQUEST>").map(|r| r.split("</USER_REQUEST>").next().unwrap_or(r)).unwrap_or(s);
    inner.trim().to_string()
}

/// The argument worth showing for an Antigravity tool call.
fn subject(args: &Value) -> String {
    for k in ["CommandLine", "AbsolutePath", "TargetFile", "FilePath", "Url", "query", "Query", "SearchPath", "DirectoryPath", "Pattern"] {
        if let Some(v) = args.get(k).and_then(|v| v.as_str()) {
            return if k.contains("Path") || k.contains("File") { util::short(v) } else { one_line(v, 300) };
        }
    }
    one_line(&args.to_string(), 200)
}

/// transcript -> ((mtime, size) it was parsed at, (activity, waiting on you))
type ParseCache = Option<HashMap<PathBuf, ((f64, u64), (Activity, bool))>>;
static PARSED: Mutex<ParseCache> = Mutex::new(None);

/// (activity, waiting on the user?)
pub fn parse(path: &Path) -> (Activity, bool) {
    let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let key = (util::file_mtime(path).unwrap_or(0.0), size);
    if let Some((k, v)) = PARSED.lock().unwrap().as_ref().and_then(|m| m.get(path)) {
        if *k == key {
            return v.clone();
        }
    }
    let mut act = Activity { mtime: Some(key.0), ..Default::default() };
    let mut waiting = false;
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    for raw in &lines[lines.len().saturating_sub(3000)..] {
        let Ok(d) = serde_json::from_str::<Value>(raw) else { continue };
        let ts = d.get("created_at").and_then(parse_ts);
        let status = d.get("status").and_then(|v| v.as_str()).unwrap_or("");
        waiting = status.contains("WAIT") || status.contains("PENDING_USER");
        match d.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "USER_INPUT" => {
                let t = strip_tags(d.get("content").and_then(|v| v.as_str()).unwrap_or(""));
                if !t.is_empty() {
                    act.events.push(Event { k: "you".into(), text: t, tool: None, desc: None, ts });
                }
            }
            "PLANNER_RESPONSE" => {
                if let Some(m) = d.get("model").and_then(|v| v.as_str()) {
                    act.model = Some(m.to_string());
                }
                if let Some(t) = d.get("response").or_else(|| d.get("content")).and_then(|v| v.as_str()).filter(|t| !t.trim().is_empty()) {
                    act.events.push(Event { k: "text".into(), text: t.trim().to_string(), tool: None, desc: None, ts });
                }
                for call in d.get("tool_calls").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                    let name = call.get("name").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
                    let args = call.get("args").cloned().unwrap_or(Value::Null);
                    if matches!(name.as_str(), "write_to_file" | "replace_file_content" | "multi_replace_file_content") {
                        if let Some(f) = args.get("TargetFile").and_then(|v| v.as_str()) {
                            act.edited.insert(f.to_string(), ts.unwrap_or(0.0));
                        }
                    }
                    act.events.push(Event { k: "tool".into(), text: subject(&args), tool: Some(name), desc: None, ts });
                }
            }
            _ => {}
        }
    }
    let n = act.events.len();
    act.events.drain(..n.saturating_sub(150));
    let v = (act, waiting);
    PARSED.lock().unwrap().get_or_insert_with(HashMap::new).insert(path.to_path_buf(), (key, v.clone()));
    v
}

pub fn activity(path: &Path) -> Activity {
    parse(path).0
}
