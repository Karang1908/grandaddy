//! Claude desktop app: its Code sessions are recorded under
//! ~/Library/Application Support/Claude/claude-code-sessions/<account>/<org>/local_<id>.json
//! (title, cwd, model, lastActivityAt, cliSessionId). The conversation itself is a normal
//! Claude Code transcript, found by cliSessionId.
use super::{status_from_mtime, Answer, Found, RECENT};
use crate::claude;
use crate::util::{self, now};
use serde_json::Value;
use std::fs;

pub fn sessions() -> Vec<Found> {
    let root = util::home().join("Library/Application Support/Claude/claude-code-sessions");
    let mut out = vec![];
    let Ok(accounts) = fs::read_dir(root) else { return out };
    for a in accounts.flatten() {
        let Ok(orgs) = fs::read_dir(a.path()) else { continue };
        for o in orgs.flatten() {
            let Ok(files) = fs::read_dir(o.path()) else { continue };
            for f in files.flatten() {
                let p = f.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !(name.starts_with("local_") && name.ends_with(".json")) {
                    continue;
                }
                let Some(d) = util::read_json::<Value>(&p) else { continue };
                let g = |k: &str| d.get(k).and_then(|v| v.as_str()).map(String::from);
                if d.get("isArchived").and_then(|v| v.as_bool()) == Some(true) {
                    continue;
                }
                let last = d.get("lastActivityAt").and_then(|v| v.as_f64()).unwrap_or(0.0) / 1000.0;
                let Some(cli) = g("cliSessionId") else { continue };
                let transcript = claude::transcript(&cli);
                let mtime = transcript.as_ref().and_then(|t| util::file_mtime(t)).unwrap_or(last).max(last);
                if now() - mtime > RECENT {
                    continue;
                }
                out.push(Found {
                    key: format!("claude:{cli}"),
                    tool: "claude",
                    surface: "app",
                    name: g("title").unwrap_or_else(|| "Claude".into()),
                    sid: Some(cli),
                    pid: None,
                    cwd: g("cwd").unwrap_or_default(),
                    status: status_from_mtime(mtime),
                    since: mtime,
                    started: d.get("createdAt").and_then(|v| v.as_f64()).unwrap_or(0.0) / 1000.0,
                    model: g("model").map(|m| m.replace("claude-", "")),
                    source: transcript.map(|t| t.to_string_lossy().into_owned()),
                    answer: Answer::Full,
                });
            }
        }
    }
    out
}
