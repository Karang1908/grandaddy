//! The approval hook, shared by every agent that has one. Each tool calls it in its own format;
//! it's normalised into one call, decided the same way, and answered in the tool's format.
//!
//! | tool        | event                                   | sends                                  |
//! |-------------|-----------------------------------------|----------------------------------------|
//! | claude      | PermissionRequest (+ AskUserQuestion)   | session_id, tool_name, tool_input, cwd |
//! | codex       | PermissionRequest                       | session_id, tool_name, tool_input, cwd |
//! | antigravity | PreToolUse                              | conversationId, toolCall{name,args}    |
//! | cursor      | beforeShellExecution / beforeMCPExecution | conversation_id, command / tool_name |
//! | gemini      | BeforeTool                              | session_id, tool_name, tool_input      |
//!
//! Decision order: a `no` rule refuses, a `yes` rule approves, auto-yes approves (unless an `ask`
//! rule matched). Otherwise the request is published to ~/.grandaddy/pending (and your phone)
//! and held for an answer — always for Claude Code, whose own dialog stays up meanwhile; for
//! other tools only when remote answering is on for them, and then only for `remote_timeout`.
//! Questions are never answered by rules or auto-yes.
use crate::claude::{self, Decision, Request};
use crate::util::{self, now};
use crate::{config, phone, providers, rules};
use serde_json::{json, Value};
use std::time::Duration;

const MAX_WAIT: f64 = 3500.0; // Claude Code gives the hook 3600s
const GRACE: f64 = 5.0; // the hook can start a moment before Claude marks the session "waiting"
const POLL: Duration = Duration::from_millis(250);
const PHONE_EVERY: f64 = 2.0;

/// One tool call, whatever agent it came from.
#[derive(Debug, Clone)]
pub struct Call {
    pub agent: String,
    pub sid: String,
    /// the agent's own tool name, mapped to Claude Code's names where they mean the same
    /// (run_command / shell → Bash), so one set of rules covers every agent
    pub tool: String,
    pub input: Value,
    pub cwd: Option<String>,
    pub model: Option<String>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(String::from)
}

/// Map shell-like tools onto Claude Code's `Bash` with a `command` argument.
fn canonical(tool: &str, input: Value) -> (String, Value) {
    let cmd = ["command", "CommandLine", "cmd"].iter().find_map(|k| input.get(*k).and_then(|v| v.as_str()).map(String::from)).or_else(|| {
        input.get("command").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" "))
    });
    match (tool, cmd) {
        ("run_command" | "run_shell_command" | "shell" | "exec_command" | "local_shell" | "bash", Some(c)) => {
            let mut v = input.clone();
            if let Value::Object(m) = &mut v {
                m.insert("command".into(), Value::String(c));
            }
            ("Bash".into(), v)
        }
        _ => (tool.to_string(), input),
    }
}

/// Parse a hook payload. `agent` comes from `--tool`; without it the shape decides.
pub fn parse(stdin: &str, agent: Option<&str>) -> Option<Call> {
    let d: Value = serde_json::from_str(stdin).ok()?;
    let event = s(&d, "hook_event_name").unwrap_or_default();
    let agent = agent.map(String::from).unwrap_or_else(|| {
        if d.get("toolCall").is_some() {
            "antigravity".into()
        } else if event.starts_with("before") {
            "cursor".into()
        } else if event == "BeforeTool" {
            "gemini".into()
        } else if d.get("turn_id").is_some() {
            "codex".into()
        } else {
            "claude".into()
        }
    });
    let call = match agent.as_str() {
        "antigravity" => {
            let tc = d.get("toolCall")?;
            let (tool, input) = canonical(&s(tc, "name")?, tc.get("args").cloned().unwrap_or(json!({})));
            let cwd = d.get("workspacePaths").and_then(|v| v.as_array()).and_then(|a| a.first()).and_then(|v| v.as_str()).map(String::from);
            Call { agent, sid: s(&d, "conversationId")?, tool, input, cwd, model: s(&d, "modelName") }
        }
        "cursor" => {
            let cwd = s(&d, "cwd").or_else(|| d.get("workspace_roots").and_then(|v| v.as_array()).and_then(|a| a.first()).and_then(|v| v.as_str()).map(String::from));
            let sid = s(&d, "conversation_id")?;
            let (tool, input) = if event == "beforeMCPExecution" {
                let server = s(&d, "mcp_server_name").unwrap_or_else(|| "mcp".into());
                (format!("mcp__{server}__{}", s(&d, "tool_name").unwrap_or_default()), d.get("tool_input").cloned().unwrap_or(json!({})))
            } else {
                ("Bash".into(), json!({"command": s(&d, "command").unwrap_or_default()}))
            };
            Call { agent, sid, tool, input, cwd, model: s(&d, "model") }
        }
        _ => {
            let (tool, input) = canonical(&s(&d, "tool_name")?, d.get("tool_input").cloned().unwrap_or(json!({})));
            Call { agent, sid: s(&d, "session_id")?, tool, input, cwd: s(&d, "cwd"), model: s(&d, "model") }
        }
    };
    (!call.sid.is_empty()).then_some(call)
}

/// The reply, in the agent's own format. `None` decision = "no opinion": the agent's usual
/// approval flow continues.
pub fn reply(agent: &str, req_input: &Value, d: Option<&Decision>) -> Option<String> {
    let deny_msg = |m: &Option<String>| m.clone().unwrap_or_else(|| "Denied from grandaddy.".into());
    let out = match agent {
        "antigravity" => match d {
            // "allow" defers to the next hook or the default policy; "auto_approve" skips the prompt
            None => json!({"decision": "allow"}),
            Some(Decision::Deny { message }) => json!({"decision": "deny", "reason": deny_msg(message)}),
            Some(_) => json!({"decision": "auto_approve", "reason": "Approved in grandaddy"}),
        },
        "cursor" => match d {
            // Cursor treats "allow" as run-without-asking, so "no opinion" has to be "ask"
            None => json!({"permission": "ask"}),
            Some(Decision::Deny { message }) => json!({"permission": "deny", "user_message": deny_msg(message), "agent_message": deny_msg(message)}),
            Some(_) => json!({"permission": "allow"}),
        },
        "gemini" => match d {
            None => json!({}),
            Some(Decision::Deny { message }) => json!({"decision": "deny", "reason": deny_msg(message)}),
            Some(_) => json!({"decision": "allow"}),
        },
        _ => {
            // claude + codex: PermissionRequest; Codex fails closed on updatedInput, so only Claude gets answers
            let decision = match d? {
                Decision::Allow => json!({"behavior": "allow"}),
                Decision::Deny { message } => json!({"behavior": "deny", "message": deny_msg(message)}),
                Decision::Answers { answers } if agent == "claude" => {
                    let mut input = req_input.clone();
                    if let Value::Object(m) = &mut input {
                        m.insert("answers".into(), Value::Object(answers.clone()));
                    }
                    json!({"behavior": "allow", "updatedInput": input})
                }
                Decision::Answers { .. } => return None,
            };
            json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": decision}})
        }
    };
    Some(out.to_string())
}

/// Run the hook on one payload; returns what to print.
pub fn run(stdin: &str, agent: Option<&str>) -> Option<String> {
    let call = parse(stdin, agent)?;
    if call.agent != "claude" {
        providers::remember(&call.agent, &call.sid, call.cwd.as_deref(), call.model.as_deref());
    }
    let short: String = call.sid.chars().take(8).collect();
    let what = format!("{}{}", if call.agent == "claude" { String::new() } else { format!("[{}] ", call.agent) }, claude::tool_summary(&call.tool, &call.input));
    let decided = |d: Decision, why: &str| {
        util::log(&format!("{short}  {why}  {what}"));
        reply(&call.agent, &call.input, Some(&d))
    };
    let question = call.tool == "AskUserQuestion";
    if !question {
        let rule = rules::matching(&call.tool, &call.input);
        match rule.as_ref().map(|r| r.verb.as_str()) {
            Some("no") => {
                let r = rule.unwrap();
                return decided(Decision::Deny { message: Some(format!("Blocked by grandaddy rule: {}", r.text)) }, &format!("denied by rule `{}`", r.text));
            }
            Some("yes") => return decided(Decision::Allow, &format!("allowed by rule `{}`", rule.unwrap().text)),
            Some("ask") => {}
            _ if util::is_auto(&call.sid) => return decided(Decision::Allow, "auto-approved"),
            _ => {}
        }
    }
    if !config::remote_for(&call.agent) || (question && call.agent != "claude") {
        return reply(&call.agent, &call.input, None);
    }
    let max_wait = if call.agent == "claude" { MAX_WAIT } else { config::load().remote_timeout as f64 };
    wait_for_answer(&call, &short, &what, max_wait)
}

fn wait_for_answer(call: &Call, short: &str, what: &str, max_wait: f64) -> Option<String> {
    let created = now();
    let rid = format!("{}-{}", (created * 1000.0) as u64, std::process::id());
    let req = Request {
        id: rid.clone(),
        sid: call.sid.clone(),
        tool: call.tool.clone(),
        input: call.input.clone(),
        cwd: call.cwd.clone(),
        created,
        hook_pid: std::process::id() as i64,
        agent: call.agent.clone(),
    };
    let ppath = util::pending_dir().join(format!("{rid}.json"));
    let apath = util::answers_dir().join(format!("{rid}.json"));
    let _ = util::write_json(&ppath, &req);
    let result = (|| {
        let ph = phone::config();
        if ph.is_some() {
            let name = if call.agent == "claude" { claude::session_name(&call.sid) } else { providers::label(&call.agent).to_string() };
            phone::notify_request(&req, &name);
        }
        let (mut seen_waiting, mut next_phone) = (false, now() + PHONE_EVERY);
        while now() - created < max_wait {
            let mut via = "grandaddy";
            let mut dec = util::read_json::<Decision>(&apath);
            if dec.is_none() {
                if let Some(c) = ph.as_ref().filter(|_| now() >= next_phone) {
                    next_phone = now() + PHONE_EVERY;
                    dec = phone::replies(c, &rid, created - 5.0).iter().find_map(|r| phone::decision_from_reply(&req, r));
                    via = "phone";
                }
            }
            if let Some(dec) = dec {
                let verb = match (&dec, req.is_question()) {
                    (_, true) => "answered",
                    (Decision::Deny { .. }, _) => "denied",
                    _ => "allowed",
                };
                util::log(&format!("{short}  {verb} from {via}  {what}"));
                return reply(&call.agent, &call.input, Some(&dec));
            }
            if call.agent == "claude" {
                // Claude's own dialog is up too; stop once it's been answered there
                match claude::raw_status(&call.sid).as_deref() {
                    Some("waiting") => seen_waiting = true,
                    _ if seen_waiting || now() - created > GRACE => return None,
                    _ => {}
                }
            }
            std::thread::sleep(POLL);
        }
        reply(&call.agent, &call.input, None)
    })();
    util::remove(&ppath);
    util::remove(&apath);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_agent() {
        let c = parse(r#"{"session_id":"s","tool_name":"Bash","tool_input":{"command":"ls"},"cwd":"/x"}"#, None).unwrap();
        assert_eq!((c.agent.as_str(), c.tool.as_str()), ("claude", "Bash"));
        let c = parse(r#"{"session_id":"t","turn_id":"1","hook_event_name":"PermissionRequest","tool_name":"exec_command","tool_input":{"cmd":"git push"},"cwd":"/x","model":"gpt"}"#, Some("codex")).unwrap();
        assert_eq!((c.agent.as_str(), c.tool.as_str(), c.input["command"].as_str()), ("codex", "Bash", Some("git push")));
        let c = parse(r#"{"conversationId":"a","toolCall":{"name":"run_command","args":{"CommandLine":"npm test","Cwd":"/w"}},"workspacePaths":["/w"],"modelName":"g"}"#, None).unwrap();
        assert_eq!((c.agent.as_str(), c.tool.as_str(), c.input["command"].as_str(), c.cwd.as_deref()), ("antigravity", "Bash", Some("npm test"), Some("/w")));
        let c = parse(r#"{"conversation_id":"c","hook_event_name":"beforeShellExecution","command":"rm -rf x","cwd":"/p"}"#, None).unwrap();
        assert_eq!((c.agent.as_str(), c.tool.as_str(), c.input["command"].as_str()), ("cursor", "Bash", Some("rm -rf x")));
        let c = parse(r#"{"conversation_id":"c","hook_event_name":"beforeMCPExecution","mcp_server_name":"gh","tool_name":"create_pr","tool_input":{}}"#, None).unwrap();
        assert_eq!(c.tool, "mcp__gh__create_pr");
        let c = parse(r#"{"session_id":"g","hook_event_name":"BeforeTool","tool_name":"run_shell_command","tool_input":{"command":"ls"}}"#, None).unwrap();
        assert_eq!((c.agent.as_str(), c.tool.as_str()), ("gemini", "Bash"));
        assert!(parse("not json", None).is_none());
        assert!(parse(r#"{"tool_name":"Bash"}"#, None).is_none());
    }

    #[test]
    fn replies_in_each_format() {
        let i = json!({"command": "ls"});
        let deny = Decision::Deny { message: Some("no".into()) };
        assert_eq!(reply("claude", &i, None), None);
        assert_eq!(reply("codex", &i, None), None);
        assert!(reply("claude", &i, Some(&Decision::Allow)).unwrap().contains(r#""behavior":"allow""#));
        assert!(reply("codex", &i, Some(&deny)).unwrap().contains(r#""message":"no""#));
        assert_eq!(reply("antigravity", &i, None).unwrap(), r#"{"decision":"allow"}"#);
        assert!(reply("antigravity", &i, Some(&Decision::Allow)).unwrap().contains("auto_approve"));
        assert!(reply("antigravity", &i, Some(&deny)).unwrap().contains(r#""decision":"deny""#));
        assert_eq!(reply("cursor", &i, None).unwrap(), r#"{"permission":"ask"}"#);
        assert!(reply("cursor", &i, Some(&Decision::Allow)).unwrap().contains(r#""permission":"allow""#));
        assert_eq!(reply("gemini", &i, None).unwrap(), "{}");
        let answers = Decision::Answers { answers: serde_json::Map::from_iter([("Q".to_string(), json!("A"))]) };
        assert!(reply("claude", &i, Some(&answers)).unwrap().contains("updatedInput"));
        assert_eq!(reply("codex", &i, Some(&answers)), None);
    }
}
