//! Loops: a prompt sent on a schedule — to a session you pick, or to a new background session
//! each time. A loop waits while its session is still working, and can stop itself when the
//! reply contains a phrase you choose or after a number of runs.
//!
//! Stored in ~/.grandaddy/loops.json; run by the app, or by `grandaddy daemon` without it.
use crate::sessions::{self, Session};
use crate::util::{self, now};
use crate::{launch, prompt};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Loop {
    pub id: String,
    pub prompt: String,
    pub every: u64,
    /// send to this session (by key); when it has ended, Claude sessions are resumed by sid
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub sid: Option<String>,
    /// no session: start a new Claude Code background session here each run
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub max_runs: Option<u32>,
    #[serde(default)]
    pub runs: u32,
    #[serde(default)]
    pub next_run: f64,
    #[serde(default)]
    pub last_run: Option<f64>,
    #[serde(default)]
    pub last_result: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub created: f64,
}

fn yes() -> bool {
    true
}

fn path() -> std::path::PathBuf {
    util::state().join("loops.json")
}

pub fn list() -> Vec<Loop> {
    util::read_json(&path()).unwrap_or_default()
}

fn save(v: &[Loop]) {
    let _ = util::write_json(&path(), &v);
}

/// "30s", "5m", "2h", "1d" or plain seconds.
pub fn parse_every(s: &str) -> Option<u64> {
    let s = s.trim();
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = n.parse().ok()?;
    let secs = match unit.trim() {
        "" | "s" => n,
        "m" | "min" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        _ => return None,
    };
    (secs >= 30).then_some(secs)
}

pub fn every_label(secs: u64) -> String {
    match secs {
        s if s % 86400 == 0 => format!("every {}d", s / 86400),
        s if s % 3600 == 0 => format!("every {}h", s / 3600),
        s if s % 60 == 0 => format!("every {}m", s / 60),
        s => format!("every {s}s"),
    }
}

pub fn add(prompt: &str, every: u64, session: Option<&Session>, cwd: Option<&str>, until: Option<&str>, max_runs: Option<u32>) -> Result<Loop, String> {
    if prompt.trim().is_empty() {
        return Err("a loop needs a prompt".into());
    }
    if every < 30 {
        return Err("loops run at most every 30 seconds".into());
    }
    if session.is_none() && cwd.map(|c| !std::path::Path::new(&util::expand(c)).is_dir()).unwrap_or(true) {
        return Err("pick a session, or a folder to start new sessions in".into());
    }
    let l = Loop {
        id: format!("{:x}", (now() * 1000.0) as u64),
        prompt: prompt.trim().into(),
        every,
        session: session.map(|s| s.key.clone()),
        sid: session.and_then(|s| s.sid.clone()),
        cwd: session.map(|s| s.cwd.clone()).or_else(|| cwd.map(util::expand)),
        until: until.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
        max_runs,
        runs: 0,
        next_run: now(),
        last_run: None,
        last_result: None,
        enabled: true,
        created: now(),
    };
    let mut all = list();
    all.push(l.clone());
    save(&all);
    Ok(l)
}

pub fn remove(id: &str) -> bool {
    let mut all = list();
    let n = all.len();
    all.retain(|l| l.id != id);
    save(&all);
    all.len() != n
}

pub fn set_enabled(id: &str, on: bool) -> bool {
    let mut all = list();
    let mut found = false;
    for l in all.iter_mut().filter(|l| l.id == id) {
        l.enabled = on;
        if on {
            l.next_run = now();
        }
        found = true;
    }
    save(&all);
    found
}

/// Run whatever is due. Returns one line per loop that ran or stopped.
pub fn tick(live: &[Session]) -> Vec<String> {
    let mut all = list();
    let mut notes = vec![];
    let mut changed = false;
    for l in all.iter_mut().filter(|l| l.enabled && now() >= l.next_run) {
        let target = l.session.as_ref().and_then(|k| live.iter().find(|s| &s.key == k));
        if let Some(s) = target {
            if s.status == "working" || s.status == "needs you" {
                continue; // try again on the next tick
            }
            if let Some(stop) = &l.until {
                let last = sessions::activity(s).events.iter().rev().find(|e| e.k == "text").map(|e| e.text.clone()).unwrap_or_default();
                if l.runs > 0 && last.contains(stop.as_str()) {
                    l.enabled = false;
                    l.last_result = Some(format!("stopped: the reply contained “{stop}”"));
                    notes.push(format!("loop stopped in {}: reply contained “{stop}”", s.name));
                    changed = true;
                    continue;
                }
            }
        }
        let result = match (target, &l.sid) {
            (Some(s), _) if prompt::typable(s) => prompt::send(s, &l.prompt, false).map(|d| d.message),
            (_, Some(sid)) => {
                let (ok, out) = launch::resume(sid, l.cwd.as_deref(), Some(&l.prompt));
                if ok { Ok(format!("resumed {}", &sid[..8.min(sid.len())])) } else { Err(out) }
            }
            (None, None) => {
                let (ok, out) = launch::new(l.cwd.as_deref().unwrap_or("."), &l.prompt, None, None);
                if ok { Ok(out.lines().next().unwrap_or("started").to_string()) } else { Err(out) }
            }
            (Some(s), None) => Err(format!("can't type into {} — start it with `grandaddy run`", s.name)),
        };
        l.runs += 1;
        l.last_run = Some(now());
        l.next_run = now() + l.every as f64;
        l.last_result = Some(match &result {
            Ok(m) => m.clone(),
            Err(e) => format!("failed: {e}"),
        });
        notes.push(format!("loop ran: {}", util::one_line(&l.prompt, 60)));
        if l.max_runs.map(|m| l.runs >= m).unwrap_or(false) {
            l.enabled = false;
        }
        changed = true;
    }
    if changed {
        save(&all);
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn intervals() {
        assert_eq!(parse_every("30m"), Some(1800));
        assert_eq!(parse_every("2h"), Some(7200));
        assert_eq!(parse_every("45"), Some(45));
        assert_eq!(parse_every("10s"), None);
        assert_eq!(parse_every("soon"), None);
        assert_eq!(every_label(1800), "every 30m");
    }
}
