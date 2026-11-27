//! Everything sleeps except the coding sessions.
//!
//! While any session is working, grandaddy holds `caffeinate -i -s -w <our pid>`: no idle system
//! sleep, while the display still sleeps and locks. The hold ends by itself if grandaddy exits.
//!
//! With the lid closed, two modes (both only while sessions work):
//!  - **full**: `pmset disablesleep 1`, so the whole Mac keeps running.
//!  - **agents**: the same, plus, while the lid is shut: the display goes off, every app that isn't
//!    running a session (or on the keep list) is paused with SIGSTOP, sound is muted and Low Power
//!    Mode turns on. Opening the lid undoes each of these. (After Insomnia's lid actions.)
//!
//! Both need root for pmset, granted once by a sudoers rule that allows exactly
//! `pmset -a disablesleep 0|1` and `pmset -b lowpowermode 0|1`. Every change is recorded in
//! ~/.grandaddy/awake/ before it's made, so a crash can't leave apps frozen or sleep disabled: the
//! next start puts everything back, and a watchdog does it as soon as grandaddy dies.
use crate::config;
use crate::sessions::Session;
use crate::util::{self, now};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

const SUDOERS: &str = "/etc/sudoers.d/grandaddy-pmset";
/// After the last reason to stay awake goes away, hold on this long before letting the Mac sleep.
const IDLE_GRACE: f64 = 600.0;

/// Why the Mac should stay awake right now: anything an agent is doing or waiting on that needs
/// the Mac running to carry on. Empty means it can sleep.
pub fn reasons(live: &[Session]) -> Vec<String> {
    let mut out = vec![];
    let n = |k: usize, one: &str, many: &str| if k == 1 { one.to_string() } else { format!("{k} {many}") };
    let working = live.iter().filter(|s| s.status == "working").count();
    if working > 0 {
        out.push(n(working, "a session is working", "sessions working"));
    }
    // answers can still arrive from your phone, and the hook has to be awake to pass them on
    let waiting = live.iter().filter(|s| !s.pending.is_empty()).count();
    if waiting > 0 {
        out.push(n(waiting, "a session is waiting on you", "sessions waiting on you"));
    }
    if config::load().auto_continue.enabled {
        let limited = live.iter().filter(|s| s.limit.as_ref().and_then(|l| l.resets_at).is_some() && !crate::limits::skipped(&s.key)).count();
        if limited > 0 {
            out.push(n(limited, "a session continues when its limit resets", "sessions continue when their limits reset"));
        }
    }
    let due = crate::loops::list().into_iter().filter(|l| l.enabled && l.next_run - now() < 3600.0).count();
    if due > 0 {
        out.push(n(due, "a loop runs within the hour", "loops run within the hour"));
    }
    out
}

fn dir() -> PathBuf {
    util::state().join("awake")
}
fn mark(name: &str) -> PathBuf {
    dir().join(name)
}

fn sudo_pmset(args: &[&str]) -> bool {
    Command::new("sudo").arg("-n").arg("/usr/bin/pmset").args(args).stdin(Stdio::null()).output().map(|o| o.status.success()).unwrap_or(false)
}
/// Can we run this pmset command without a password? (`sudo -l` checks without running it.)
fn allowed(args: &[&str]) -> bool {
    Command::new("sudo").args(["-n", "-l", "/usr/bin/pmset"]).args(args).stdin(Stdio::null()).output().map(|o| o.status.success()).unwrap_or(false)
}
pub fn lid_helper_installed() -> bool {
    std::path::Path::new(SUDOERS).exists() && allowed(&["-a", "disablesleep", "1"])
}
pub fn agents_helper_installed() -> bool {
    lid_helper_installed() && allowed(&["-b", "lowpowermode", "1"])
}

/// Is the lid closed right now? (False on desktops and when it can't tell.)
pub fn lid_closed() -> bool {
    Command::new("ioreg")
        .args(["-r", "-k", "AppleClamshellState", "-d", "1"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("\"AppleClamshellState\" = Yes"))
        .unwrap_or(false)
}

fn muted() -> Option<bool> {
    let o = Command::new("osascript").args(["-e", "output muted of (get volume settings)"]).output().ok()?;
    match String::from_utf8_lossy(&o.stdout).trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}
fn set_muted(on: bool) -> bool {
    Command::new("osascript").args(["-e", &format!("set volume output muted {on}")]).status().map(|s| s.success()).unwrap_or(false)
}
fn low_power_on() -> bool {
    Command::new("pmset").arg("-g").output().map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim_start().starts_with("lowpowermode") && l.trim_end().ends_with('1'))).unwrap_or(false)
}

// ---------- which apps to pause ----------

#[derive(Debug, Clone)]
struct AppProcs {
    name: String,
    pids: Vec<i64>,
}

/// Your apps, by bundle: /Applications/Slack.app -> every process inside it (helpers included).
fn user_apps() -> (HashMap<String, AppProcs>, HashMap<i64, i64>) {
    let uid = unsafe { libc::getuid() };
    let mut apps: HashMap<String, AppProcs> = HashMap::new();
    let mut parent = HashMap::new();
    let Ok(o) = Command::new("ps").args(["-U", &uid.to_string(), "-o", "pid=,ppid=,comm="]).output() else { return (apps, parent) };
    for line in String::from_utf8_lossy(&o.stdout).lines() {
        let line = line.trim_start();
        let mut it = line.splitn(3, char::is_whitespace);
        let (Some(pid), Some(rest)) = (it.next().and_then(|p| p.parse::<i64>().ok()), it.next()) else { continue };
        let rest = format!("{rest} {}", it.next().unwrap_or(""));
        let mut r2 = rest.trim_start().splitn(2, char::is_whitespace);
        let (Some(ppid), Some(exe)) = (r2.next().and_then(|p| p.parse::<i64>().ok()), r2.next()) else { continue };
        parent.insert(pid, ppid);
        let exe = exe.trim();
        let Some(i) = exe.find(".app/") else { continue };
        let bundle = &exe[..i + 4];
        if bundle.starts_with("/System/") || bundle.starts_with("/Library/Apple") || bundle.starts_with("/usr/") {
            continue;
        }
        let name = std::path::Path::new(bundle).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        apps.entry(bundle.to_string()).or_insert_with(|| AppProcs { name, pids: vec![] }).pids.push(pid);
    }
    (apps, parent)
}

/// Bundles of apps a session runs in: the app owning any ancestor of its process, plus the app
/// itself for IDE / desktop-app sessions.
fn hosts(live: &[Session], apps: &HashMap<String, AppProcs>, parent: &HashMap<i64, i64>) -> BTreeSet<String> {
    let by_pid: HashMap<i64, &String> = apps.iter().flat_map(|(b, a)| a.pids.iter().map(move |p| (*p, b))).collect();
    let mut keep = BTreeSet::new();
    let runs = crate::managed::list();
    for s in live {
        let mut pids: Vec<i64> = s.pid.into_iter().collect();
        if let Some(r) = s.managed.as_ref().and_then(|id| runs.iter().find(|r| &r.id == id)) {
            pids.push(r.host_pid);
        }
        for mut p in pids {
            for _ in 0..40 {
                if let Some(b) = by_pid.get(&p) {
                    keep.insert((*b).clone());
                }
                match parent.get(&p) {
                    Some(&pp) if pp > 1 && pp != p => p = pp,
                    _ => break,
                }
            }
        }
        if s.surface != "CLI" {
            let name = match s.tool.as_str() {
                "cursor" => "Cursor",
                "antigravity" => "Antigravity",
                "claude" => "Claude",
                "codex" => if s.surface == "IDE" { "Visual Studio Code" } else { "Codex" },
                _ => "",
            };
            for (b, a) in apps {
                if !name.is_empty() && a.name.starts_with(name) {
                    keep.insert(b.clone());
                }
            }
        }
    }
    keep
}

/// Which apps "agents only" would pause right now, and which keep running and why. Read-only.
pub fn preview(live: &[Session]) -> (Vec<String>, Vec<(String, &'static str)>) {
    let (apps, parent) = user_apps();
    let hosting = hosts(live, &apps, &parent);
    let keep: BTreeSet<String> = config::load().awake.keep.iter().map(|k| k.to_lowercase()).collect();
    let me = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok()).map(|p| p.display().to_string()).unwrap_or_default();
    let (mut pause, mut stay) = (BTreeSet::new(), BTreeSet::new());
    for (bundle, app) in &apps {
        let why = if hosting.contains(bundle) {
            Some("a session runs in it")
        } else if keep.contains(&app.name.to_lowercase()) {
            Some("on the keep list")
        } else if me.starts_with(bundle.as_str()) || app.name.starts_with("Grandaddy") {
            Some("Grandaddy")
        } else {
            None
        };
        match why {
            Some(w) => {
                stay.insert((app.name.clone(), w));
            }
            None => {
                pause.insert(app.name.clone());
            }
        }
    }
    (pause.into_iter().collect(), stay.into_iter().collect())
}

// ---------- the keeper ----------

#[derive(Default)]
pub struct Keeper {
    reasons: Vec<String>,
    last_lid: Option<bool>,
    caffeinate: Option<Child>,
    lid_hold: bool,
    lid_actions: bool,
    idle_since: Option<f64>,
    last_display_off: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub enabled: bool,
    pub holding: bool,
    /// why it's holding, in words (empty: nothing needs the Mac)
    pub reasons: Vec<String>,
    /// the lid is shut right now
    pub lid_closed: bool,
    /// sleep | full | agents
    pub lid_mode: String,
    pub lid_helper: bool,
    pub agents_helper: bool,
    /// sleep is disabled right now (lid modes)
    pub lid_active: bool,
    /// the lid is shut and agents-only actions are applied
    pub agents_only_active: bool,
    pub frozen: Vec<String>,
}

fn frozen_list() -> Vec<(i64, String)> {
    std::fs::read_to_string(mark("frozen"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (p, n) = l.split_once(' ')?;
            Some((p.parse().ok()?, n.to_string()))
        })
        .collect()
}

/// Undo whatever a previous run left behind (crash, force quit).
pub fn recover() {
    thaw();
    if mark("muted").exists() {
        set_muted(false);
        util::remove(&mark("muted"));
    }
    if mark("lowpower").exists() && sudo_pmset(&["-b", "lowpowermode", "0"]) {
        util::remove(&mark("lowpower"));
    }
    if mark("sleep-disabled").exists() && sudo_pmset(&["-a", "disablesleep", "0"]) {
        util::remove(&mark("sleep-disabled"));
    }
}

fn thaw() {
    for (pid, _) in frozen_list() {
        unsafe { libc::kill(pid as i32, libc::SIGCONT) };
    }
    util::remove(&mark("frozen"));
}

/// While the shell is alive it waits for `pid` to die, then undoes everything still recorded.
fn spawn_watchdog() {
    let pid = std::process::id();
    let d = dir();
    let script = format!(
        r#"while kill -0 {pid} 2>/dev/null; do sleep 3; done
cd '{d}' 2>/dev/null || exit 0
[ -f frozen ] && cut -d' ' -f1 frozen | xargs kill -CONT 2>/dev/null; rm -f frozen
[ -f muted ] && osascript -e 'set volume output muted false' && rm -f muted
[ -f lowpower ] && sudo -n /usr/bin/pmset -b lowpowermode 0 && rm -f lowpower
[ -f sleep-disabled ] && sudo -n /usr/bin/pmset -a disablesleep 0 && rm -f sleep-disabled
exit 0"#,
        d = d.display()
    );
    let _ = Command::new("/bin/sh").args(["-c", &script]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

impl Keeper {
    pub fn new() -> Keeper {
        let _ = std::fs::create_dir_all(dir());
        recover();
        spawn_watchdog();
        Keeper::default()
    }

    /// Call every few seconds with the live sessions.
    pub fn update(&mut self, live: &[Session]) {
        let cfg = config::load().awake;
        let why = reasons(live);
        let working = !why.is_empty();
        self.reasons = why;
        if let Some(c) = self.caffeinate.as_mut() {
            if c.try_wait().ok().flatten().is_some() {
                self.caffeinate = None;
            }
        }
        if working {
            self.idle_since = None;
        } else if self.idle_since.is_none() && (self.caffeinate.is_some() || self.lid_hold) {
            self.idle_since = Some(now());
        }
        let want = cfg.enabled && (working || self.idle_since.map(|t| now() - t < IDLE_GRACE).unwrap_or(false));

        if want && self.caffeinate.is_none() {
            self.caffeinate = Command::new("caffeinate").args(["-i", "-s", "-w", &std::process::id().to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok();
        } else if !want {
            if let Some(mut c) = self.caffeinate.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }

        let mode = cfg.lid_mode().to_string();
        let want_lid = want && mode != "sleep" && lid_helper_installed();
        if want_lid && !self.lid_hold {
            let _ = std::fs::write(mark("sleep-disabled"), "1");
            self.lid_hold = sudo_pmset(&["-a", "disablesleep", "1"]);
            if !self.lid_hold {
                util::remove(&mark("sleep-disabled"));
            }
        }

        let shut = lid_closed();
        if self.last_lid.is_some() && self.last_lid != Some(shut) {
            util::log(&if shut {
                if self.lid_hold { format!("lid closed: kept awake ({mode}) because {}", self.reasons.join(", ")) } else { "lid closed: nothing needs the Mac, so it sleeps".to_string() }
            } else {
                "lid opened".to_string()
            });
        }
        self.last_lid = Some(shut);
        let closed = self.lid_hold && shut;
        let want_actions = closed && mode == "agents";
        if want_actions && !self.lid_actions {
            self.apply_agents_only(live, &cfg.keep);
        } else if !want_actions && self.lid_actions {
            self.undo_agents_only();
        } else if want_actions {
            self.pause_new_apps(live, &cfg.keep); // apps opened since the lid closed
        }
        // with sleep disabled, macOS leaves the display lit under a closed lid
        if closed && now() - self.last_display_off > 60.0 {
            let _ = Command::new("pmset").arg("displaysleepnow").status();
            self.last_display_off = now();
        }
        if !closed {
            self.last_display_off = 0.0;
        }

        if !want_lid && self.lid_hold {
            if self.lid_actions {
                self.undo_agents_only();
            }
            if sudo_pmset(&["-a", "disablesleep", "0"]) {
                util::remove(&mark("sleep-disabled"));
                self.lid_hold = false;
                util::log("nothing needs the Mac any more: closing the lid sleeps it again");
            }
        }
    }

    fn apply_agents_only(&mut self, live: &[Session], keep: &[String]) {
        self.lid_actions = true;
        if muted() == Some(false) {
            let _ = std::fs::write(mark("muted"), "1");
            if !set_muted(true) {
                util::remove(&mark("muted"));
            }
        }
        if agents_helper_installed() && !low_power_on() {
            let _ = std::fs::write(mark("lowpower"), "1");
            if !sudo_pmset(&["-b", "lowpowermode", "1"]) {
                util::remove(&mark("lowpower"));
            }
        }
        self.pause_new_apps(live, keep);
        util::log(&format!("lid closed: agents only, {} apps paused", frozen_list().iter().map(|(_, n)| n).collect::<BTreeSet<_>>().len()));
    }

    fn pause_new_apps(&mut self, live: &[Session], keep: &[String]) {
        let (apps, parent) = user_apps();
        let hosting = hosts(live, &apps, &parent);
        let me = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok()).map(|p| p.display().to_string()).unwrap_or_default();
        let keep: BTreeSet<String> = keep.iter().map(|k| k.to_lowercase()).collect();
        let already: BTreeSet<i64> = frozen_list().into_iter().map(|(p, _)| p).collect();
        let mut lines = std::fs::read_to_string(mark("frozen")).unwrap_or_default();
        for (bundle, app) in &apps {
            if hosting.contains(bundle) || keep.contains(&app.name.to_lowercase()) || me.starts_with(bundle.as_str()) || app.name.starts_with("Grandaddy") {
                continue;
            }
            for pid in &app.pids {
                if already.contains(pid) {
                    continue;
                }
                // record first, so a crash in between still gets it resumed
                lines.push_str(&format!("{pid} {}\n", app.name));
                let _ = std::fs::write(mark("frozen"), &lines);
                unsafe { libc::kill(*pid as i32, libc::SIGSTOP) };
            }
        }
    }

    fn undo_agents_only(&mut self) {
        let n = frozen_list().iter().map(|(_, n)| n.clone()).collect::<BTreeSet<_>>().len();
        thaw();
        if mark("muted").exists() {
            set_muted(false);
            util::remove(&mark("muted"));
        }
        if mark("lowpower").exists() && sudo_pmset(&["-b", "lowpowermode", "0"]) {
            util::remove(&mark("lowpower"));
        }
        self.lid_actions = false;
        util::log(&format!("lid open: {n} apps resumed"));
    }

    pub fn status(&self) -> Status {
        let cfg = config::load().awake;
        Status {
            enabled: cfg.enabled,
            holding: self.caffeinate.is_some(),
            reasons: self.reasons.clone(),
            lid_closed: lid_closed(),
            lid_mode: cfg.lid_mode().to_string(),
            lid_helper: lid_helper_installed(),
            agents_helper: agents_helper_installed(),
            lid_active: self.lid_hold,
            agents_only_active: self.lid_actions,
            frozen: frozen_list().into_iter().map(|(_, n)| n).collect::<BTreeSet<_>>().into_iter().collect(),
        }
    }
}

impl Drop for Keeper {
    fn drop(&mut self) {
        if let Some(mut c) = self.caffeinate.take() {
            let _ = c.kill();
        }
        if self.lid_actions {
            self.undo_agents_only();
        }
        if self.lid_hold && sudo_pmset(&["-a", "disablesleep", "0"]) {
            util::remove(&mark("sleep-disabled"));
        }
    }
}

/// One-time admin approval for exactly these four pmset commands.
pub fn install_lid_helper() -> Result<(), String> {
    let user = std::env::var("USER").map_err(|_| "couldn't tell which user you are")?;
    if !user.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
        return Err("unexpected user name".into());
    }
    let rule = format!(
        "{user} ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 0, /usr/bin/pmset -a disablesleep 1, /usr/bin/pmset -b lowpowermode 0, /usr/bin/pmset -b lowpowermode 1"
    );
    let tmp = format!("/tmp/grandaddy-sudoers-{}", std::process::id());
    std::fs::write(&tmp, format!("{rule}\n")).map_err(|e| e.to_string())?;
    let script = format!(
        "do shell script \"/usr/sbin/visudo -cf {tmp} && /usr/bin/install -m 0440 -o root -g wheel {tmp} {SUDOERS}\" with administrator privileges with prompt \"Grandaddy needs your password once, so agents can keep working with the lid closed.\""
    );
    let o = Command::new("osascript").args(["-e", &script]).output().map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tmp);
    if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
}

pub fn remove_lid_helper() -> Result<(), String> {
    recover();
    let script = format!("do shell script \"/bin/rm -f {SUDOERS}\" with administrator privileges");
    let o = Command::new("osascript").args(["-e", &script]).output().map_err(|e| e.to_string())?;
    if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn frozen_file_round_trip() {
        let line = "123 Slack\n456 Spotify Helper\n";
        let parsed: Vec<(i64, String)> = line.lines().filter_map(|l| {
            let (p, n) = l.split_once(' ')?;
            Some((p.parse().ok()?, n.to_string()))
        }).collect();
        assert_eq!(parsed, vec![(123, "Slack".into()), (456, "Spotify Helper".into())]);
    }
}
