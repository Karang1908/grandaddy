//! Goals: describe an outcome, and a session takes it from there. It asks you what it needs to
//! know first, works on its own, and stops to ask before any major decision. With Claude Code
//! those questions arrive in grandaddy and on your phone; other agents ask in their window.
//!
//! Stored in ~/.grandaddy/goals/<id>.json. A goal finds its session through a marker in the
//! first prompt, and its status follows that session's.
use crate::sessions::{self, Session};
use crate::util::{self, now};
use crate::{launch, providers};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub text: String,
    pub cwd: String,
    pub tool: String,
    #[serde(default)]
    pub model: Option<String>,
    /// background (Claude Code only) or terminal
    pub runner: String,
    pub created: f64,
    #[serde(default)]
    pub session_key: Option<String>,
    /// asking | working | needs you | done | stopped
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub summary: Option<String>,
}

fn dir() -> PathBuf {
    util::state().join("goals")
}

fn marker(id: &str) -> String {
    format!("(grandaddy goal {id})")
}

/// The instructions the session starts with.
pub fn prompt(tool: &str, id: &str, text: &str) -> String {
    let _ = tool;
    let ask = "using the AskUserQuestion tool";
    format!(
        "I want you to take on this goal:\n\n{text}\n\n\
         How to work on it:\n\
         1. Before building anything, make sure you understand it. Ask me what you need to know — scope, constraints, what done looks like — {ask}, a few questions at a time. If it's already clear, say so and begin.\n\
         2. Then work through it on your own. Make routine decisions yourself.\n\
         3. Stop and ask me ({ask}) before any major decision: changing the plan or scope, choosing between real architectural options, anything destructive or hard to undo (deleting data, migrations, force-pushes, anything in production), or anything that costs money. Give the options and say which you'd pick.\n\
         4. When it's done, finish with a line that starts with GOAL COMPLETE, followed by a short summary of what changed.\n\n{}",
        marker(id)
    )
}

/// Claude Code, started interactively with a first prompt.
fn command(model: Option<&str>, prompt: &str) -> Vec<String> {
    let mut cmd = vec!["claude".to_string()];
    if let Some(m) = model.filter(|m| !m.is_empty()) {
        cmd.push("--model".into());
        cmd.push(m.into());
    }
    cmd.push(prompt.to_string());
    cmd
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Start a goal. Claude Code goals can run in the background; everything else (and any goal
/// you'd like to watch) opens in a terminal window under `grandaddy run`.
pub fn start(text: &str, cwd: &str, tool: &str, model: Option<&str>, in_terminal: bool) -> Result<Goal, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("describe the goal first".into());
    }
    let cwd = util::expand(cwd.trim());
    if !std::path::Path::new(&cwd).is_dir() {
        return Err(format!("no such folder: {cwd}"));
    }
    let id = format!("{:x}", (now() * 1000.0) as u64);
    let p = prompt(tool, &id, text);
    let runner = if tool == "claude" && !in_terminal { "background" } else { "terminal" };
    if runner == "background" {
        let (ok, out) = launch::new(&cwd, &p, None, model);
        if !ok {
            return Err(out);
        }
    } else {
        let cli = crate::install::exe();
        let line = std::iter::once(cli).chain(std::iter::once("run".to_string())).chain(command(model, &p)).map(|a| shell_quote(&a)).collect::<Vec<_>>().join(" ");
        let (ok, out) = launch::open_terminal(&line, Some(&cwd));
        if !ok {
            return Err(out);
        }
    }
    let g = Goal { id: id.clone(), text: text.into(), cwd, tool: tool.into(), model: model.map(String::from), runner: runner.into(), created: now(), session_key: None, status: "asking".into(), summary: None };
    util::write_json(&dir().join(format!("{id}.json")), &g).map_err(|e| e.to_string())?;
    Ok(g)
}

/// All goals, newest first, with status refreshed from their sessions.
pub fn list(live: &[Session]) -> Vec<Goal> {
    let mut out: Vec<Goal> = std::fs::read_dir(dir())
        .map(|rd| rd.flatten().filter_map(|e| util::read_json::<Goal>(&e.path())).collect())
        .unwrap_or_default();
    for g in out.iter_mut() {
        if matches!(g.status.as_str(), "done" | "stopped") {
            continue;
        }
        let before = (g.session_key.clone(), g.status.clone(), g.summary.clone());
        let session = match &g.session_key {
            Some(k) => live.iter().find(|s| &s.key == k),
            None => live.iter().filter(|s| s.tool == g.tool && s.cwd == g.cwd && s.started >= g.created - 60.0).find(|s| {
                sessions::activity(s).events.iter().any(|e| e.k == "you" && e.text.contains(&marker(&g.id)))
            }),
        };
        if let Some(s) = session {
            g.session_key = Some(s.key.clone());
            let events = sessions::activity(s).events;
            let done = events.iter().rev().filter(|e| e.k == "text").take(3).find(|e| e.text.contains("GOAL COMPLETE"));
            g.status = if let Some(d) = done {
                g.summary = d.text.split("GOAL COMPLETE").nth(1).map(|t| util::one_line(t.trim_start_matches([':', ' ', '.', '—', '-']), 400));
                "done".into()
            } else if s.status == "needs you" {
                "needs you".into()
            } else if events.iter().filter(|e| e.k == "tool").count() > 0 {
                "working".into()
            } else {
                "asking".into()
            };
        } else if g.session_key.is_some() || now() - g.created > providers::RECENT {
            g.status = "stopped".into();
        }
        if before != (g.session_key.clone(), g.status.clone(), g.summary.clone()) {
            let _ = util::write_json(&dir().join(format!("{}.json", g.id)), g);
        }
    }
    out.sort_by(|a, b| b.created.partial_cmp(&a.created).unwrap_or(std::cmp::Ordering::Equal));
    out
}

pub fn remove(id: &str) -> bool {
    std::fs::remove_file(dir().join(format!("{id}.json"))).is_ok()
}
