//! Approve from your phone with ntfy (https://ntfy.sh): free, no account, open-source app.
//!
//! When a session needs you, the hook posts a notification with buttons to your private topic.
//! Tapping a button posts a reply to `<topic>-reply`, which the hook polls. Nothing has to reach
//! your Mac from outside. The topic name is the secret: anyone who knows it can read and answer
//! your prompts, so it's long and random, and you can point it at your own server.
use crate::claude::{describe_request, Decision, Request};
use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub server: String,
    pub topic: String,
}

fn path() -> PathBuf {
    util::state().join("phone.json")
}

pub fn config() -> Option<Config> {
    util::read_json::<Config>(&path()).filter(|c| !c.server.is_empty() && !c.topic.is_empty())
}

pub fn setup(server: &str) -> Config {
    let c = Config { server: server.trim_end_matches('/').to_string(), topic: format!("grandaddy-{}", util::random_token()) };
    let _ = util::write_json(&path(), &c);
    c
}

pub fn off() {
    util::remove(&path());
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(5))).build().into()
}

/// buttons: (label, reply body), at most 3 (ntfy's limit).
pub fn publish(c: &Config, title: &str, message: &str, buttons: &[(String, String)]) -> Result<(), String> {
    let actions: Vec<Value> = buttons
        .iter()
        .take(3)
        .map(|(label, body)| {
            json!({"action": "http", "label": label, "url": format!("{}/{}-reply", c.server, c.topic),
                   "method": "POST", "body": body, "clear": true})
        })
        .collect();
    let body = json!({"topic": c.topic, "title": title.chars().take(200).collect::<String>(),
                      "message": message.chars().take(3500).collect::<String>(),
                      "priority": 4, "tags": ["older_man"], "actions": actions});
    agent()
        .post(&format!("{}/", c.server))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Buttons for a request, or none when it needs the hub (multi-part questions).
pub fn request_buttons(req: &Request) -> Vec<(String, String)> {
    if !req.is_question() {
        return vec![("Allow".into(), format!("{} allow", req.id)), ("Deny".into(), format!("{} deny", req.id))];
    }
    let qs = req.questions();
    let single = qs.len() == 1 && qs[0].get("multiSelect").and_then(|v| v.as_bool()) != Some(true);
    let opts = qs.first().and_then(|q| q.get("options")).and_then(|v| v.as_array()).cloned().unwrap_or_default();
    if single && opts.len() <= 3 {
        opts.iter()
            .enumerate()
            .map(|(i, o)| {
                let label: String = o.get("label").and_then(|v| v.as_str()).unwrap_or("?").chars().take(40).collect();
                (label, format!("{} {}", req.id, i + 1))
            })
            .collect()
    } else {
        vec![]
    }
}

pub fn notify_request(req: &Request, name: &str) -> bool {
    let Some(c) = config() else { return false };
    let buttons = request_buttons(req);
    let mut msg = describe_request(req);
    if req.is_question() {
        if let Some(q) = req.input.pointer("/questions/0/question").and_then(|v| v.as_str()) {
            msg = q.to_string();
        }
        if buttons.is_empty() {
            msg.push_str("\n(answer this one in grandaddy)");
        }
    }
    publish(&c, &format!("{name} needs you"), &msg, &buttons).is_ok()
}

/// Reply bodies for this request posted since `since` (unix seconds).
pub fn replies(c: &Config, rid: &str, since: f64) -> Vec<String> {
    let url = format!("{}/{}-reply/json?poll=1&since={}", c.server, c.topic, since as i64);
    let Ok(mut resp) = agent().get(&url).call() else { return vec![] };
    let Ok(body) = resp.body_mut().read_to_string() else { return vec![] };
    let prefix = format!("{rid} ");
    body.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|m| m.get("event").and_then(|v| v.as_str()) == Some("message"))
        .filter_map(|m| m.get("message").and_then(|v| v.as_str()).and_then(|s| s.strip_prefix(&prefix)).map(|s| s.trim().to_string()))
        .collect()
}

/// "allow" | "deny" | option number -> the same decision the hub would write.
pub fn decision_from_reply(req: &Request, reply: &str) -> Option<Decision> {
    if !req.is_question() {
        return match reply {
            "allow" => Some(Decision::Allow),
            "deny" => Some(Decision::Deny { message: None }),
            _ => None,
        };
    }
    let n: usize = reply.parse().ok()?;
    let q = req.questions().into_iter().next()?;
    let opt = q.get("options")?.as_array()?.get(n.checked_sub(1)?)?.clone();
    let mut answers = Map::new();
    answers.insert(q.get("question")?.as_str()?.to_string(), opt.get("label")?.clone());
    Some(Decision::Answers { answers })
}

pub fn test() -> Result<String, String> {
    let c = config().ok_or("phone isn't set up — run: grandaddy phone setup")?;
    let stamp = chrono::Local::now().format("%H:%M:%S");
    publish(&c, "grandaddy", &format!("Test from grandaddy at {stamp}. If you see this, you're set."), &[])
        .map(|_| "sent a test notification".to_string())
        .map_err(|e| format!("couldn't reach {}: {e}", c.server))
}
