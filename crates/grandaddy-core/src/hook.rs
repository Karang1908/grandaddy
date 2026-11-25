//! The PermissionRequest hook. Claude Code runs it whenever it is about to show a dialog.
//!
//! 1. A `no` rule refuses and a `yes` rule approves, in any session.
//! 2. auto-yes is on for the session and no `ask` rule matched: approve.
//! 3. Otherwise publish the request to ~/.grandaddy/pending (and to your phone, if set up) and
//!    wait. The dialog is already on screen in the terminal, so you can answer there, in the
//!    hub/app, or on your phone; whichever comes first wins (Claude ignores a hook answer that
//!    arrives after you answered locally). The hook gives up once the session stops waiting.
//!
//! Questions (AskUserQuestion) are never answered by rules or auto-yes: they always reach you.
use crate::claude::{self, Decision, Request};
use crate::util::{self, now};
use crate::{phone, rules};
use serde_json::{json, Value};
use std::time::Duration;

const MAX_WAIT: f64 = 3500.0; // settings.json gives the hook 3600s
const GRACE: f64 = 5.0; // the hook can start a moment before Claude marks the session "waiting"
const POLL: Duration = Duration::from_millis(250);
const PHONE_EVERY: f64 = 2.0;

/// The JSON Claude Code expects back, for a decision.
pub fn output(req_input: &Value, d: &Decision) -> Value {
    let decision = match d {
        Decision::Allow => json!({"behavior": "allow"}),
        Decision::Deny { message } => {
            json!({"behavior": "deny", "message": message.clone().unwrap_or_else(|| "Denied from grandaddy.".into())})
        }
        Decision::Answers { answers } => {
            let mut input = req_input.clone();
            if let Value::Object(m) = &mut input {
                m.insert("answers".into(), Value::Object(answers.clone()));
            }
            json!({"behavior": "allow", "updatedInput": input})
        }
    };
    json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": decision}})
}

/// Run the hook on one payload; returns what to print (None = no decision, show the dialog).
pub fn run(stdin: &str) -> Option<String> {
    let d: Value = serde_json::from_str(stdin).ok()?;
    let sid = d.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    let tool = d.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    if sid.is_empty() || tool.is_empty() {
        return None;
    }
    let inp = d.get("tool_input").cloned().unwrap_or_else(|| json!({}));
    let what = claude::tool_summary(tool, &inp);
    let short: String = sid.chars().take(8).collect();
    if tool != "AskUserQuestion" {
        let rule = rules::matching(tool, &inp);
        match rule.as_ref().map(|r| r.verb.as_str()) {
            Some("no") => {
                let r = rule.as_ref().unwrap();
                util::log(&format!("{short}  denied by rule `{}`  {what}", r.text));
                let msg = format!("Blocked by grandaddy rule: {}", r.text);
                return Some(output(&inp, &Decision::Deny { message: Some(msg) }).to_string());
            }
            Some("yes") => {
                util::log(&format!("{short}  allowed by rule `{}`  {what}", rule.unwrap().text));
                return Some(output(&inp, &Decision::Allow).to_string());
            }
            Some("ask") => {}
            _ => {
                if util::is_auto(sid) {
                    util::log(&format!("{short}  auto-approved  {what}"));
                    return Some(output(&inp, &Decision::Allow).to_string());
                }
            }
        }
    }

    let created = now();
    let rid = format!("{}-{}", (created * 1000.0) as u64, std::process::id());
    let req = Request {
        id: rid.clone(),
        sid: sid.into(),
        tool: tool.into(),
        input: inp.clone(),
        cwd: d.get("cwd").and_then(|v| v.as_str()).map(String::from),
        created,
        hook_pid: std::process::id() as i64,
    };
    let ppath = util::pending_dir().join(format!("{rid}.json"));
    let apath = util::answers_dir().join(format!("{rid}.json"));
    let _ = util::write_json(&ppath, &req);
    let result = (|| {
        let ph = phone::config();
        if ph.is_some() {
            phone::notify_request(&req, &claude::session_name(sid));
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
                return Some(output(&inp, &dec).to_string());
            }
            match claude::raw_status(sid).as_deref() {
                Some("waiting") => seen_waiting = true,
                _ if seen_waiting || now() - created > GRACE => return None, // answered elsewhere, or gone
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
