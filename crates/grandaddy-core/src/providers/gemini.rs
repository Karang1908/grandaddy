//! Gemini CLI and its fork Qwen Code: chats saved as JSON under ~/.gemini/tmp/<project>/chats/
//! (and ~/.qwen/tmp/…): {sessionId, startTime, lastUpdated, messages: [{type: user|gemini, …}]}.
use super::{name_from, procs, seen, status_from_mtime, Answer, Found, RECENT};
use crate::claude::{Activity, Event};
use crate::util::{self, now, one_line, parse_ts};
use serde_json::Value;
use std::fs;
use std::path::Path;

pub fn sessions(procs: &[procs::Proc]) -> Vec<Found> {
    let mut out = vec![];
    for (tool, dir) in [("gemini", ".gemini"), ("qwen", ".qwen")] {
        let Ok(projects) = fs::read_dir(util::home().join(dir).join("tmp")) else { continue };
        for p in projects.flatten() {
            let Ok(chats) = fs::read_dir(p.path().join("chats")) else { continue };
            for c in chats.flatten() {
                let path = c.path();
                let Some(mtime) = util::file_mtime(&path).filter(|m| now() - m < RECENT) else { continue };
                let Some(d) = util::read_json::<Value>(&path) else { continue };
                let sid = d.get("sessionId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let act = from_value(&d, mtime);
                let cwd = seen(tool, &sid).and_then(|k| k.get("cwd")?.as_str().map(String::from)).unwrap_or_default();
                let pid = procs::find(procs, tool, &cwd).map(|p| p.pid);
                if pid.is_none() {
                    continue; // the CLI has exited
                }
                out.push(Found {
                    key: format!("{tool}:{sid}"),
                    tool: if tool == "gemini" { "gemini" } else { "qwen" },
                    surface: "CLI",
                    name: act.events.iter().find(|e| e.k == "you").map(|e| name_from(&e.text)).unwrap_or_else(|| super::label(tool).into()),
                    sid: Some(sid),
                    pid,
                    cwd,
                    status: status_from_mtime(mtime),
                    since: mtime,
                    started: d.get("startTime").and_then(parse_ts).unwrap_or(mtime),
                    model: act.model.clone(),
                    source: Some(path.to_string_lossy().into_owned()),
                    answer: if tool == "gemini" { Answer::Permissions } else { Answer::None },
                });
            }
        }
    }
    out
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|x| x.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

fn from_value(d: &Value, mtime: f64) -> Activity {
    let mut act = Activity { mtime: Some(mtime), ..Default::default() };
    for m in d.get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let ts = m.get("timestamp").and_then(parse_ts);
        let t = text_of(m.get("content").unwrap_or(&Value::Null));
        match m.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "user" if !t.trim().is_empty() => act.events.push(Event { k: "you".into(), text: t.trim().into(), tool: None, desc: None, ts }),
            "gemini" | "model" | "qwen" => {
                if let Some(model) = m.get("model").and_then(|v| v.as_str()) {
                    act.model = Some(model.to_string());
                }
                for call in m.get("toolCalls").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                    let name = call.get("name").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
                    let args = call.get("args").cloned().unwrap_or(Value::Null);
                    let shown = ["command", "file_path", "absolute_path", "path", "pattern", "url", "query"]
                        .iter()
                        .find_map(|k| args.get(*k).and_then(|v| v.as_str()).map(String::from))
                        .unwrap_or_else(|| args.to_string());
                    act.events.push(Event { k: "tool".into(), text: one_line(&util::short(&shown), 300), tool: Some(name), desc: None, ts });
                }
                if !t.trim().is_empty() {
                    act.events.push(Event { k: "text".into(), text: t.trim().into(), tool: None, desc: None, ts });
                }
            }
            _ => {}
        }
    }
    let n = act.events.len();
    act.events.drain(..n.saturating_sub(150));
    act
}

pub fn activity(path: &Path) -> Activity {
    let mtime = util::file_mtime(path).unwrap_or(0.0);
    util::read_json::<Value>(path).map(|d| from_value(&d, mtime)).unwrap_or_default()
}
