//! Claude Code's PermissionRequest hook: Claude runs `grandaddy hook` before it shows a
//! permission dialog or asks a question, and waits for what it prints.
//!
//! Decision order: a `no` rule refuses, a `yes` rule approves, auto-yes approves (unless an `ask`
//! rule matched). Otherwise the request is published to ~/.grandaddy/pending (and your phone)
//! and held for an answer from grandaddy, while Claude's own dialog stays up in its terminal;
//! whichever is answered first wins. Questions are never answered by rules or auto-yes.
use crate::claude::{self, Decision, Request};
use crate::util::{self, now};
use crate::{phone, rules};
use serde_json::{json, Value};
use std::time::Duration;

const MAX_WAIT: f64 = 3500.0; // Claude Code gives the hook 3600s
const GRACE: f64 = 5.0; // the hook can start a moment before Claude marks the session "waiting"
const POLL: Duration = Duration::from_millis(250);
const PHONE_EVERY: f64 = 2.0;

/// One tool call from Claude Code.
#[derive(Debug, Clone)]
pub struct Call {
    pub sid: String,
    pub tool: String,
    pub input: Value,
    pub cwd: Option<String>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(String::from)
}

pub fn parse(stdin: &str) -> Option<Call> {
    let d: Value = serde_json::from_str(stdin).ok()?;
    let call = Call { sid: s(&d, "session_id")?, tool: s(&d, "tool_name")?, input: d.get("tool_input").cloned().unwrap_or(json!({})), cwd: s(&d, "cwd") };
    (!call.sid.is_empty()).then_some(call)
}

/// What to print for Claude. `None` = no opinion: Claude's own dialog decides.
pub fn reply(req_input: &Value, d: Option<&Decision>) -> Option<String> {
    let decision = match d? {
        Decision::Allow => json!({"behavior": "allow"}),
        Decision::Deny { message } => json!({"behavior": "deny", "message": message.clone().unwrap_or_else(|| "Denied from grandaddy.".into())}),
        Decision::Answers { answers } => {
            let mut input = req_input.clone();
            if let Value::Object(m) = &mut input {
                m.insert("answers".into(), Value::Object(answers.clone()));
            }
            json!({"behavior": "allow", "updatedInput": input})
        }
    };
    Some(json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": decision}}).to_string())
}

/// Run the hook on one payload; returns what to print.
pub fn run(stdin: &str) -> Option<String> {
    let call = parse(stdin)?;
    let short: String = call.sid.chars().take(8).collect();
    let what = claude::tool_summary(&call.tool, &call.input);
    let decided = |d: Decision, why: &str| {
        util::log(&format!("{short}  {why}  {what}"));
        reply(&call.input, Some(&d))
    };
    if call.tool != "AskUserQuestion" {
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
    wait_for_answer(&call, &short, &what)
}

fn wait_for_answer(call: &Call, short: &str, what: &str) -> Option<String> {
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
        agent: "claude".into(),
    };
    let ppath = util::pending_dir().join(format!("{rid}.json"));
    let apath = util::answers_dir().join(format!("{rid}.json"));
    let _ = util::write_json(&ppath, &req);
    let result = (|| {
        let ph = phone::config();
        if ph.is_some() {
            phone::notify_request(&req, &claude::session_name(&call.sid));
        }
        let (mut seen_waiting, mut next_phone) = (false, now() + PHONE_EVERY);
        while now() - created < MAX_WAIT {
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
                return reply(&call.input, Some(&dec));
            }
            // Claude's own dialog is up too; stop once it's been answered there
            match claude::raw_status(&call.sid).as_deref() {
                Some("waiting") => seen_waiting = true,
                _ if seen_waiting || now() - created > GRACE => return None,
                _ => {}
            }
            std::thread::sleep(POLL);
        }
        None
    })();
    util::remove(&ppath);
    util::remove(&apath);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_replies() {
        let c = parse(r#"{"session_id":"s","tool_name":"Bash","tool_input":{"command":"ls"},"cwd":"/x"}"#).unwrap();
        assert_eq!((c.sid.as_str(), c.tool.as_str(), c.cwd.as_deref()), ("s", "Bash", Some("/x")));
        assert!(parse("not json").is_none());
        assert!(parse(r#"{"tool_name":"Bash"}"#).is_none());
        let i = json!({"command": "ls"});
        assert_eq!(reply(&i, None), None);
        assert!(reply(&i, Some(&Decision::Allow)).unwrap().contains(r#""behavior":"allow""#));
        assert!(reply(&i, Some(&Decision::Deny { message: Some("no".into()) })).unwrap().contains(r#""message":"no""#));
        let answers = Decision::Answers { answers: serde_json::Map::from_iter([("Q".to_string(), json!("A"))]) };
        assert!(reply(&i, Some(&answers)).unwrap().contains("updatedInput"));
    }
}
