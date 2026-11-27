//! Cursor: agent chats ("composers") live in the IDE's global SQLite store
//! (~/Library/Application Support/Cursor/User/globalStorage/state.vscdb, table cursorDiskKV):
//! `composerData:<id>` holds name/status/timestamps, `bubbleId:<id>:<bubble>` each message.
use super::{db_mtime, procs, seen, sqlite_json, Answer, Found, ACTIVE, RECENT};
use crate::claude::{Activity, Event};
use crate::util::{self, now, one_line};
use serde_json::Value;
use std::sync::Mutex;

fn db() -> String {
    util::home().join("Library/Application Support/Cursor/User/globalStorage/state.vscdb").to_string_lossy().into_owned()
}

/// Read-only query; falls back to immutable mode when the plain read-only open is refused
/// (immutable skips an uncheckpointed WAL, so very fresh writes can lag a few seconds).
fn query(sql: &str) -> Vec<serde_json::Map<String, Value>> {
    let rows = sqlite_json(&db(), sql);
    if !rows.is_empty() {
        return rows;
    }
    sqlite_json(&format!("file:{}?immutable=1", db()), sql)
}

static CACHE: Mutex<Option<(f64, f64, Vec<Found>)>> = Mutex::new(None);

pub fn sessions(procs: &[procs::Proc]) -> Vec<Found> {
    if !std::path::Path::new(&db()).exists() {
        return vec![];
    }
    let mtime = db_mtime(&db());
    if let Some((m, at, v)) = CACHE.lock().unwrap().as_ref() {
        if *m == mtime && now() - at < 15.0 {
            return v.clone();
        }
    }
    let since_ms = ((now() - RECENT) * 1000.0) as i64;
    let rows = query(&format!(
        "select substr(key, 14) id, json_extract(value,'$.name') name, json_extract(value,'$.status') status,
           json_extract(value,'$.createdAt') created, json_extract(value,'$.lastUpdatedAt') updated
         from cursorDiskKV where key like 'composerData:%' and json_extract(value,'$.lastUpdatedAt') > {since_ms}
         order by updated desc limit 20"
    ));
    let cli_running = procs.iter().any(|p| p.tool == "cursor");
    let out: Vec<Found> = rows
        .into_iter()
        .map(|r| {
            let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let updated = r.get("updated").and_then(|v| v.as_f64()).unwrap_or(0.0) / 1000.0;
            let raw = r.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let status = if raw == "generating" || now() - updated < ACTIVE { "working" } else { "idle" };
            let cwd = seen("cursor", &id).and_then(|k| k.get("cwd")?.as_str().map(String::from)).unwrap_or_default();
            Found {
                key: format!("cursor:{id}"),
                tool: "cursor",
                surface: if cli_running { "CLI" } else { "IDE" },
                name: r.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("Cursor chat").to_string(),
                source: Some(id.clone()),
                sid: Some(id),
                pid: None,
                cwd,
                status: status.into(),
                since: updated,
                started: r.get("created").and_then(|v| v.as_f64()).unwrap_or(0.0) / 1000.0,
                model: None,
                answer: Answer::Permissions,
            }
        })
        .collect();
    *CACHE.lock().unwrap() = Some((mtime, now(), out.clone()));
    out
}

/// source = composer id
pub fn activity(id: &str) -> Activity {
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Activity::default();
    }
    let mut act = Activity { mtime: Some(db_mtime(&db())), ..Default::default() };
    let rows = query(&format!(
        "select value from cursorDiskKV where key like 'bubbleId:{id}:%'
         order by json_extract(value,'$.createdAt') limit 400"
    ));
    for r in rows {
        let b: Value = serde_json::from_str(r.get("value").and_then(|v| v.as_str()).unwrap_or("")).unwrap_or(Value::Null);
        let ts = b.get("createdAt").and_then(|v| v.as_str()).and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d| d.timestamp() as f64);
        let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        match b.get("type").and_then(|v| v.as_i64()) {
            Some(1) if !text.is_empty() => act.events.push(Event { k: "you".into(), text, tool: None, desc: None, ts }),
            Some(2) => {
                if let Some(tf) = b.get("toolFormerData") {
                    let name = tf.get("name").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
                    let args: Value = tf.get("rawArgs").and_then(|v| v.as_str()).and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
                    let shown = ["command", "target_file", "file_path", "path", "query", "url"]
                        .iter()
                        .find_map(|k| args.get(*k).and_then(|v| v.as_str()).map(String::from))
                        .unwrap_or_default();
                    act.events.push(Event { k: "tool".into(), text: one_line(&util::short(&shown), 300), tool: Some(name), desc: None, ts });
                }
                if !text.is_empty() {
                    act.events.push(Event { k: "text".into(), text, tool: None, desc: None, ts });
                }
            }
            _ => {}
        }
    }
    let n = act.events.len();
    act.events.drain(..n.saturating_sub(150));
    act
}
