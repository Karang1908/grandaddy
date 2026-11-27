//! OpenCode and its fork Kilo CLI keep everything in SQLite (~/.local/share/{opencode,kilo}):
//! `session` (directory, title, model, tokens, times), `message` and `part` rows holding JSON.
//! Read with the system sqlite3 in read-only mode.
use super::{db_mtime, procs, sqlite_json, Answer, Found, ACTIVE, RECENT};
use crate::claude::{Activity, Event};
use crate::util::{self, now, one_line};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;

fn db(tool: &str) -> String {
    let file = if tool == "kilo" { "kilo/kilo.db" } else { "opencode/opencode.db" };
    util::home().join(".local/share").join(file).to_string_lossy().into_owned()
}

/// tool -> (database mtime, sessions read at that mtime)
type Cache = Option<HashMap<String, (f64, Vec<Found>)>>;
static CACHE: Mutex<Cache> = Mutex::new(None);

pub fn sessions(procs: &[procs::Proc]) -> Vec<Found> {
    let mut out = vec![];
    for tool in ["opencode", "kilo"] {
        let path = db(tool);
        if !std::path::Path::new(&path).exists() {
            continue;
        }
        let mtime = db_mtime(&path);
        if let Some((m, v)) = CACHE.lock().unwrap().as_ref().and_then(|c| c.get(tool)) {
            if *m == mtime && now() - mtime > ACTIVE {
                out.extend(v.iter().cloned().filter_map(|mut f| {
                    f.pid = procs::find(procs, f.tool, &f.cwd).map(|p| p.pid);
                    f.pid.map(|_| f) // only while the CLI is running
                }));
                continue;
            }
        }
        let since_ms = ((now() - RECENT) * 1000.0) as i64;
        let rows = sqlite_json(
            &path,
            &format!(
                "select s.id, s.directory, s.title, s.model, s.time_created, s.time_updated,
                   (select data from message m where m.session_id = s.id order by m.time_created desc limit 1) as last
                 from session s where s.parent_id is null and s.time_archived is null and s.time_updated > {since_ms}
                 order by s.time_updated desc limit 30"
            ),
        );
        let mut found = vec![];
        for r in rows {
            let g = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let updated = r.get("time_updated").and_then(|v| v.as_f64()).unwrap_or(0.0) / 1000.0;
            let last: Value = serde_json::from_str(&g("last")).unwrap_or(Value::Null);
            let busy = last.get("role").and_then(|v| v.as_str()) == Some("assistant") && last.pointer("/time/completed").is_none();
            // an assistant reply still being written, or a write in the last few seconds
            let status = if (busy && now() - updated < 600.0) || now() - updated < ACTIVE { "working" } else { "idle" };
            let model: Value = serde_json::from_str(&g("model")).unwrap_or(Value::Null);
            let id = g("id");
            let cwd = g("directory");
            found.push(Found {
                key: format!("{tool}:{id}"),
                tool: if tool == "kilo" { "kilo" } else { "opencode" },
                surface: "CLI",
                name: if g("title").is_empty() { super::label(tool).to_string() } else { g("title") },
                source: Some(format!("{path}|{id}")),
                sid: Some(id),
                pid: procs::find(procs, tool, &cwd).map(|p| p.pid),
                cwd,
                status: status.into(),
                since: updated,
                started: r.get("time_created").and_then(|v| v.as_f64()).unwrap_or(0.0) / 1000.0,
                model: model.get("id").and_then(|v| v.as_str()).map(String::from),
                answer: Answer::None,
            });
        }
        CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(tool.to_string(), (mtime, found.clone()));
        out.extend(found.into_iter().filter(|f| f.pid.is_some())); // only while the CLI is running
    }
    out
}

/// source = "<db>|<session id>"
pub fn activity(source: &str) -> Activity {
    let Some((path, id)) = source.split_once('|') else { return Activity::default() };
    if id.contains('\'') {
        return Activity::default();
    }
    let mut act = Activity { mtime: Some(db_mtime(path)), ..Default::default() };
    let rows = sqlite_json(
        path,
        &format!(
            "select p.data as part, m.data as msg, p.time_created as t from part p join message m on m.id = p.message_id
             where p.session_id = '{id}' order by p.time_created desc limit 400"
        ),
    );
    for r in rows.into_iter().rev() {
        let part: Value = serde_json::from_str(r.get("part").and_then(|v| v.as_str()).unwrap_or("")).unwrap_or(Value::Null);
        let msg: Value = serde_json::from_str(r.get("msg").and_then(|v| v.as_str()).unwrap_or("")).unwrap_or(Value::Null);
        let ts = r.get("t").and_then(|v| v.as_f64()).map(|t| t / 1000.0);
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(m) = msg.get("modelID").and_then(|v| v.as_str()) {
            act.model = Some(m.to_string());
        }
        match part.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "text" => {
                let t = part.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if !t.is_empty() && part.get("synthetic").and_then(|v| v.as_bool()) != Some(true) {
                    act.events.push(Event { k: if role == "user" { "you" } else { "text" }.into(), text: t, tool: None, desc: None, ts });
                }
            }
            "tool" => {
                let name = part.get("tool").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
                let input = part.pointer("/state/input").cloned().unwrap_or(Value::Null);
                let shown = ["command", "filePath", "file_path", "path", "pattern", "url", "query", "description"]
                    .iter()
                    .find_map(|k| input.get(*k).and_then(|v| v.as_str()).map(String::from))
                    .unwrap_or_else(|| input.to_string());
                if matches!(name.as_str(), "edit" | "write" | "patch") {
                    if let Some(f) = input.get("filePath").and_then(|v| v.as_str()) {
                        act.edited.insert(f.to_string(), ts.unwrap_or(0.0));
                    }
                }
                act.events.push(Event { k: "tool".into(), text: one_line(&util::short(&shown), 300), tool: Some(name), desc: None, ts });
                if part.pointer("/state/status").and_then(|v| v.as_str()) == Some("error") {
                    let err = part.pointer("/state/error").and_then(|v| v.as_str()).unwrap_or("error");
                    act.events.push(Event { k: "error".into(), text: one_line(err, 300), tool: None, desc: None, ts });
                }
            }
            _ => {}
        }
    }
    let n = act.events.len();
    act.events.drain(..n.saturating_sub(150));
    act
}
