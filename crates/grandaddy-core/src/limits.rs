//! Usage limits and auto-continue. When a session stops on its plan's usage limit ("You've hit
//! your 5-hour limit · resets 4:30am"), grandaddy can send it a message once the limit lifts, so
//! the work carries on without you.
//!
//! Claude Code records the exact reset time (`quotaLimits.resetsAt`); for anything else the
//! reset is read from the message text. Each limit is continued at most once, a little after it
//! lifts, and only in sessions grandaddy can reach: ones started with `grandaddy run`, in iTerm,
//! Terminal or tmux, or Claude Code background sessions (resumed). For any other session you get
//! a notification instead.
use crate::claude::Limit;
use crate::sessions::{self, Session};
use crate::util::{self, now};
use crate::{config, launch, prompt};
use chrono::{Datelike, Local, NaiveDate, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// After the reset, wait this long before continuing (the limit can lift a little late).
pub const GRACE: f64 = 90.0;

pub fn kind_from_text(text: &str) -> String {
    let t = text.to_lowercase();
    if t.contains("5-hour") {
        "five_hour"
    } else if t.contains("weekly") {
        "seven_day"
    } else if t.contains("session limit") {
        "session"
    } else {
        "usage"
    }
    .into()
}

pub fn label(kind: &str) -> &'static str {
    match kind {
        "five_hour" => "5-hour limit",
        "seven_day" => "weekly limit",
        "session" => "session limit",
        _ => "usage limit",
    }
}

/// "4:30am", "12pm", "3:05 PM" → (hour, minute)
fn clock(s: &str) -> Option<(u32, u32)> {
    let s = s.trim().to_lowercase().replace(' ', "");
    let (num, pm) = match s.strip_suffix("pm") {
        Some(n) => (n, true),
        None => (s.strip_suffix("am")?, false),
    };
    let (h, m) = match num.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (num.parse::<u32>().ok()?, 0),
    };
    if !(1..=12).contains(&h) || m > 59 {
        return None;
    }
    Some(((h % 12) + if pm { 12 } else { 0 }, m))
}

fn local_ts(date: NaiveDate, h: u32, m: u32) -> Option<f64> {
    Local.from_local_datetime(&date.and_time(NaiveTime::from_hms_opt(h, m, 0)?)).earliest().map(|d| d.timestamp() as f64)
}

/// The reset time from a limit message, relative to when it was hit. Times are the machine's
/// local time, which is what the agents print.
///  - "… resets 4:30am (Asia/Dubai)"            → the next 4:30am after `at`
///  - "… resets Nov 20 at 8am (Asia/Dubai)"     → that date
///  - "… try again at 3:05 PM" / "in 2 hours 13 minutes" (Codex)
pub fn parse_reset(text: &str, at: f64) -> Option<f64> {
    let lower = text.to_lowercase();
    let hit = Local.timestamp_opt(at as i64, 0).single()?;
    if let Some(rest) = lower.split("try again in ").nth(1) {
        let mut secs = 0.0;
        let words: Vec<&str> = rest.split_whitespace().collect();
        for w in words.windows(2) {
            if let Ok(n) = w[0].parse::<f64>() {
                let unit = w[1].trim_end_matches(['.', ',']);
                secs += n * match unit {
                    u if u.starts_with("day") => 86400.0,
                    u if u.starts_with("hour") => 3600.0,
                    u if u.starts_with("min") => 60.0,
                    u if u.starts_with("sec") => 1.0,
                    _ => 0.0,
                };
            }
        }
        return (secs > 0.0).then_some(at + secs);
    }
    let rest = lower.split("resets ").nth(1).or_else(|| lower.split("try again at ").nth(1))?;
    let rest = rest.split('(').next().unwrap_or(rest).trim().trim_end_matches('.');
    // "nov 20 at 8am"
    if let Some((date, time)) = rest.split_once(" at ") {
        let (h, m) = clock(time)?;
        let mut parts = date.split_whitespace();
        let word = parts.next()?;
        let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|mo| word.starts_with(mo))? as u32 + 1;
        let day: u32 = parts.next()?.trim_end_matches(',').parse().ok()?;
        let mut year = hit.year();
        let mut d = NaiveDate::from_ymd_opt(year, month, day)?;
        if d < hit.date_naive() {
            year += 1; // "Jan 2" seen on Dec 30
            d = NaiveDate::from_ymd_opt(year, month, day)?;
        }
        return local_ts(d, h, m);
    }
    // "4:30am"
    let (h, m) = clock(rest.split_whitespace().take(2).collect::<Vec<_>>().join("").as_str()).or_else(|| clock(rest.split_whitespace().next()?))?;
    let today = local_ts(hit.date_naive(), h, m)?;
    Some(if today > at { today } else { today + 86400.0 })
}

// ---------- auto-continue ----------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    /// session key -> the reset time already continued for
    #[serde(default)]
    done: BTreeMap<String, f64>,
    /// sessions you said not to continue
    #[serde(default)]
    skip: BTreeSet<String>,
}

fn path() -> std::path::PathBuf {
    util::state().join("auto-continue.json")
}

fn load() -> State {
    util::read_json(&path()).unwrap_or_default()
}

fn save(s: &State) {
    let _ = util::write_json(&path(), s);
}

pub fn skipped(key: &str) -> bool {
    load().skip.contains(key)
}

/// Turn auto-continue off (or back on) for one session.
pub fn set_skip(key: &str, skip: bool) {
    let mut s = load();
    if skip {
        s.skip.insert(key.to_string());
    } else {
        s.skip.remove(key);
    }
    save(&s);
}

/// The session's current limit stop, if it has one.
pub fn of(s: &Session) -> Option<Limit> {
    if !matches!(s.tool.as_str(), "claude" | "codex") || s.status == "working" {
        return None;
    }
    sessions::activity(s).limit
}

/// What happens when this limit lifts, in words for the UI.
pub fn plan(s: &Session, l: &Limit) -> String {
    let c = config::load().auto_continue;
    if !c.enabled {
        return "Auto-continue is off".into();
    }
    if skipped(&s.key) {
        return "Won't continue this session".into();
    }
    if l.resets_at.is_none() {
        return "Reset time unknown".into();
    }
    if prompt::typable(s) {
        format!("Will send “{}” when it resets", c.message)
    } else if s.tool == "claude" && s.sid.is_some() && s.bg {
        format!("Will resume it with “{}” when it resets", c.message)
    } else {
        "You'll get a notification when it resets".into()
    }
}

/// Continue whatever is due. Returns one line per action, for logs.
pub fn tick(live: &[Session]) -> Vec<String> {
    let c = config::load().auto_continue;
    if !c.enabled {
        return vec![];
    }
    let mut st = load();
    let mut notes = vec![];
    for s in live {
        let Some(l) = of(s) else { continue };
        let Some(resets) = l.resets_at else { continue };
        if now() < resets + GRACE || st.skip.contains(&s.key) || st.done.get(&s.key) == Some(&resets) {
            continue;
        }
        // a limit from long ago that nobody continued is history, not a task
        if now() - resets > 6.0 * 3600.0 {
            st.done.insert(s.key.clone(), resets);
            continue;
        }
        let result = if prompt::typable(s) {
            prompt::send(s, &c.message, false).map(|d| d.message)
        } else if s.tool == "claude" && s.bg {
            match &s.sid {
                Some(sid) => {
                    let (ok, out) = launch::resume(sid, Some(&s.cwd), Some(&c.message));
                    if ok { Ok(format!("Resumed {}", s.name)) } else { Err(out) }
                }
                None => Err("no session id".into()),
            }
        } else {
            launch::notify(&s.name, &format!("Its {} has reset. Continue it in its window.", label(&l.kind)));
            Ok("notified".into())
        };
        let line = match result {
            Ok(m) => format!("auto-continue {}: {m}", s.name),
            Err(e) => format!("auto-continue {} failed: {e}", s.name),
        };
        util::log(&line);
        notes.push(line);
        st.done.insert(s.key.clone(), resets);
    }
    // forget sessions that are gone
    let keys: BTreeSet<&String> = live.iter().map(|s| &s.key).collect();
    st.done.retain(|k, _| keys.contains(k));
    save(&st);
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> f64 {
        local_ts(NaiveDate::from_ymd_opt(y, mo, d).unwrap(), h, mi).unwrap()
    }
    #[test]
    fn reset_times() {
        let hit = at(2026, 11, 12, 14, 39);
        assert_eq!(parse_reset("You've hit your 5-hour limit · resets 4:30pm (Asia/Dubai)", hit), Some(at(2026, 11, 12, 16, 30)));
        assert_eq!(parse_reset("You've hit your 5-hour limit · resets 1:40am (Asia/Dubai)", hit), Some(at(2026, 11, 13, 1, 40)));
        assert_eq!(parse_reset("You've hit your 5-hour limit · resets 12am (Asia/Dubai)", hit), Some(at(2026, 11, 13, 0, 0)));
        assert_eq!(parse_reset("You've hit your 5-hour limit · resets 12pm (Asia/Dubai)", hit), Some(at(2026, 11, 13, 12, 0)));
        assert_eq!(parse_reset("You've hit your weekly limit · resets Nov 20 at 8am (Asia/Dubai)", hit), Some(at(2026, 11, 20, 8, 0)));
        assert_eq!(parse_reset("You've hit your weekly limit · resets Jan 2 at 8am (Asia/Dubai)", at(2026, 12, 30, 9, 0)), Some(at(2027, 1, 2, 8, 0)));
        assert_eq!(parse_reset("You've hit your usage limit. Try again in 2 hours 13 minutes.", hit), Some(hit + 2.0 * 3600.0 + 13.0 * 60.0));
        assert_eq!(parse_reset("You've hit your usage limit. Try again at 3:05 PM.", hit), Some(at(2026, 11, 12, 15, 5)));
        assert_eq!(parse_reset("something else entirely", hit), None);
        assert_eq!(kind_from_text("You've hit your session limit · resets 2:10pm"), "session");
    }
}
