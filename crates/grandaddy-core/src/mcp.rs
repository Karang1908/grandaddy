//! `grandaddy mcp` — an MCP server (stdio) so sessions can work with each other across agents.
//! Experimental, and off until you turn it on in Settings (or `grandaddy mesh on`).
//!
//! S2S: a session lists, reads and prompts other sessions, whatever agent they run in.
//! A2A: a session starts helpers in any agent and model, waits for them, and reads the result —
//! so a planner on one model can hand work to sessions on others.
use crate::sessions::{self, Session};
use crate::util::{self, now};
use crate::{config, launch, prompt, providers};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

/// Versions Claude Code accepts, newest first.
const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

fn tools() -> Value {
    json!([
        {"name": "list_sessions", "description": "Coding-agent sessions running on this Mac (Claude Code, Codex, Antigravity, Cursor, Gemini, OpenCode, Kilo…): key, agent, status, folder, model, and whether it accepts prompts.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "read_session", "description": "Recent activity of a session: prompts, replies, tool calls and errors.",
         "inputSchema": {"type": "object", "properties": {"session": {"type": "string", "description": "session key from list_sessions"}, "limit": {"type": "integer", "default": 30}}, "required": ["session"]}},
        {"name": "send_prompt", "description": "Type a prompt into another session (only sessions that accept prompts, e.g. ones started with `grandaddy run`). The message is prefixed with who sent it.",
         "inputSchema": {"type": "object", "properties": {"session": {"type": "string"}, "text": {"type": "string"}}, "required": ["session", "text"]}},
        {"name": "start_session", "description": "Start a new session in any supported agent and model to take on a piece of work. Claude Code runs in the background; other agents open in a terminal window.",
         "inputSchema": {"type": "object", "properties": {
             "agent": {"type": "string", "enum": ["claude", "codex", "antigravity", "gemini", "opencode", "kilo"], "default": "claude"},
             "prompt": {"type": "string"}, "folder": {"type": "string", "description": "working folder; defaults to the caller's"},
             "model": {"type": "string", "description": "optional model id for that agent"}}, "required": ["prompt"]}},
        {"name": "wait_for_session", "description": "Wait until a session stops working (or the timeout passes) and return its latest reply.",
         "inputSchema": {"type": "object", "properties": {"session": {"type": "string"}, "timeout_seconds": {"type": "integer", "default": 300, "maximum": 1800}}, "required": ["session"]}}
    ])
}

/// The session this server was started by, when the agent exposes it.
fn caller() -> Option<Session> {
    let sid = std::env::var("CLAUDE_CODE_SESSION_ID").ok()?;
    sessions::collect().into_iter().find(|s| s.sid.as_deref() == Some(sid.as_str()))
}

fn find(key: &str) -> Result<Session, String> {
    sessions::collect().into_iter().find(|s| s.key == key || s.sid.as_deref() == Some(key)).ok_or_else(|| format!("no running session {key} — call list_sessions"))
}

fn summary(s: &Session) -> Value {
    json!({"key": s.key, "name": s.name, "agent": providers::label(&s.tool), "surface": s.surface, "status": s.status,
           "folder": s.cwd, "model": s.model, "accepts_prompts": prompt::typable(s), "started": s.started})
}

fn call(name: &str, args: &Value) -> Result<String, String> {
    if !config::load().experimental_mesh {
        return Err("Session-to-session is off. Turn it on in Grandaddy → Settings → Experimental, or run `grandaddy mesh on`.".into());
    }
    let arg = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    match name {
        "list_sessions" => {
            let me = caller().map(|c| c.key);
            let list: Vec<Value> = sessions::collect().iter().filter(|s| Some(&s.key) != me.as_ref()).map(summary).collect();
            Ok(serde_json::to_string_pretty(&list).unwrap_or_default())
        }
        "read_session" => {
            let s = find(&arg("session"))?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(30) as usize;
            let events = sessions::activity(&s).events;
            let lines: Vec<String> = events[events.len().saturating_sub(limit)..]
                .iter()
                .map(|e| match e.k.as_str() {
                    "you" => format!("PROMPT: {}", e.text),
                    "text" => format!("REPLY: {}", e.text),
                    "tool" => format!("TOOL {}: {}", e.tool.as_deref().unwrap_or("?"), e.desc.as_deref().unwrap_or(&e.text)),
                    "error" => format!("ERROR: {}", e.text),
                    _ => format!("-- {} --", e.text),
                })
                .collect();
            Ok(format!("{} ({}, {}):\n{}", s.name, providers::label(&s.tool), s.status, lines.join("\n")))
        }
        "send_prompt" => {
            let s = find(&arg("session"))?;
            let me = caller();
            if me.as_ref().map(|m| m.key == s.key).unwrap_or(false) {
                return Err("that's this session".into());
            }
            if !prompt::typable(&s) {
                return Err(format!("{} doesn't accept prompts from other sessions — it has to be started with `grandaddy run`", s.name));
            }
            let from = me.map(|m| format!("From {} (another session): ", m.name)).unwrap_or_else(|| "From another session: ".into());
            prompt::send(&s, &format!("{from}{}", arg("text")), false).map(|d| d.message)
        }
        "start_session" => {
            let agent = if arg("agent").is_empty() { "claude".to_string() } else { arg("agent") };
            let folder = if arg("folder").is_empty() { caller().map(|c| c.cwd).unwrap_or_else(|| std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()) } else { util::expand(&arg("folder")) };
            let model = Some(arg("model")).filter(|m| !m.is_empty());
            let text = arg("prompt");
            if text.trim().is_empty() {
                return Err("give the new session a prompt".into());
            }
            if agent == "claude" {
                let (ok, out) = launch::new(&folder, &text, None, model.as_deref());
                return if ok { Ok(format!("{out}\nIt will appear in list_sessions within a few seconds.")) } else { Err(out) };
            }
            let g = crate::goals::start(&text, &folder, &agent, model.as_deref(), true).map_err(|e| e.to_string())?;
            Ok(format!("Opened {} in a terminal window (goal {}). It will appear in list_sessions shortly.", providers::label(&agent), g.id))
        }
        "wait_for_session" => {
            let key = arg("session");
            let timeout = args.get("timeout_seconds").and_then(|v| v.as_u64()).unwrap_or(300).min(1800) as f64;
            let start = now();
            loop {
                let s = find(&key)?;
                if s.status != "working" || now() - start > timeout {
                    let reply = sessions::activity(&s).events.iter().rev().find(|e| e.k == "text").map(|e| e.text.clone()).unwrap_or_default();
                    let state = if s.status == "working" { "still working (timed out)" } else { &s.status };
                    return Ok(format!("{} is {state}.\nLatest reply:\n{reply}", s.name));
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

fn respond(id: &Value, result: Result<Value, (i64, String)>) -> String {
    match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, message)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
    }
    .to_string()
}

/// Handle one JSON-RPC message; None for notifications.
pub fn handle(line: &str) -> Option<String> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(respond(&Value::Null, Err((-32700, format!("parse error: {e}"))))),
    };
    let id = msg.get("id")?.clone();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
            let version = if VERSIONS.contains(&asked) { asked } else { VERSIONS[1] };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "grandaddy", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Work with other coding-agent sessions on this Mac: list and read them, prompt the ones that accept prompts, start helpers in any agent and model, and wait for their results."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Ok(match call(name, &args) {
                Ok(text) => json!({"content": [{"type": "text", "text": text}]}),
                Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
            })
        }
        other => Err((-32601, format!("method not found: {other}"))),
    };
    Some(respond(&id, result))
}

pub fn serve() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(out) = handle(&line) {
            let _ = writeln!(stdout, "{out}");
            let _ = stdout.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol() {
        let r: Value = serde_json::from_str(&handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#).unwrap()).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "grandaddy");
        let r: Value = serde_json::from_str(&handle(r#"{"jsonrpc":"2.0","id":"x","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#).unwrap()).unwrap();
        assert_eq!(r["result"]["protocolVersion"], VERSIONS[1]);
        assert!(handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let r: Value = serde_json::from_str(&handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap()).unwrap();
        assert_eq!(r["result"]["tools"].as_array().unwrap().len(), 5);
        let r: Value = serde_json::from_str(&handle(r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#).unwrap()).unwrap();
        assert_eq!(r["error"]["code"], -32601);
        assert!(handle("garbage").unwrap().contains("-32700"));
    }
}
