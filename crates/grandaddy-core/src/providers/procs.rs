//! Interactive agent CLIs found in the process table (only processes attached to a terminal).
use crate::util;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

/// command name -> tool id
const COMMANDS: &[(&str, &str)] = &[
    ("codex", "codex"), ("gemini", "gemini"), ("qwen", "qwen"), ("opencode", "opencode"), ("kilo", "kilo"),
    ("kilocode", "kilo"), ("agy", "antigravity"), ("cursor-agent", "cursor"), ("aider", "aider"), ("amp", "amp"),
    ("goose", "goose"), ("crush", "crush"), ("droid", "droid"), ("copilot", "copilot"), ("claude", "claude"),
];

#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: i64,
    pub ppid: i64,
    pub tool: &'static str,
    pub tty: String,
    pub started: f64,
    pub cwd: String,
}

/// ps etime: [[dd-]hh:]mm:ss
pub fn etime_seconds(e: &str) -> Option<f64> {
    let (days, rest) = match e.split_once('-') {
        Some((d, r)) => (d.parse::<f64>().ok()?, r),
        None => (0.0, e),
    };
    let mut parts: Vec<f64> = rest.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    while parts.len() < 3 {
        parts.insert(0, 0.0);
    }
    Some(days * 86400.0 + parts[0] * 3600.0 + parts[1] * 60.0 + parts[2])
}

pub fn tool_of(args: &str) -> Option<&'static str> {
    args.split_whitespace().take(2).find_map(|tok| {
        let b = Path::new(tok).file_name()?.to_str()?;
        COMMANDS.iter().find(|(c, _)| *c == b).map(|(_, t)| *t)
    })
}

static CWD: Mutex<Option<HashMap<i64, String>>> = Mutex::new(None);

pub fn cwd_of(pid: i64) -> String {
    if let Some(c) = CWD.lock().unwrap().as_ref().and_then(|m| m.get(&pid)) {
        return c.clone();
    }
    let cwd = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().find_map(|l| l.strip_prefix('n').map(String::from)))
        .unwrap_or_default();
    CWD.lock().unwrap().get_or_insert_with(HashMap::new).insert(pid, cwd.clone());
    cwd
}

/// Agent CLIs running in a terminal, wrapper processes collapsed into the session they start.
pub fn processes() -> Vec<Proc> {
    let Ok(o) = Command::new("ps").args(["-axo", "pid=,ppid=,tty=,etime=,args="]).output() else { return vec![] };
    let now = util::now();
    let mut found: HashMap<i64, Proc> = HashMap::new();
    for line in String::from_utf8_lossy(&o.stdout).lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        let args = parts[4..].join(" ");
        let Some(tool) = tool_of(&args) else { continue };
        let tty = parts[2].to_string();
        if tty == "??" || tty == "-" {
            continue;
        }
        let (Ok(pid), Ok(ppid), Some(age)) = (parts[0].parse(), parts[1].parse(), etime_seconds(parts[3])) else { continue };
        found.insert(pid, Proc { pid, ppid, tool, tty, started: now - age, cwd: String::new() });
    }
    // a node wrapper and the native binary it spawns are one session: keep the outer one
    let mut procs: Vec<Proc> = found.values().filter(|p| !found.get(&p.ppid).map(|q| q.tool == p.tool).unwrap_or(false)).cloned().collect();
    let live: HashSet<i64> = found.keys().copied().collect();
    if let Some(m) = CWD.lock().unwrap().as_mut() {
        m.retain(|pid, _| live.contains(pid));
    }
    for p in procs.iter_mut() {
        p.cwd = cwd_of(p.pid);
    }
    procs.sort_by(|a, b| a.started.partial_cmp(&b.started).unwrap_or(std::cmp::Ordering::Equal));
    procs
}

/// The live process of `tool` running in `cwd`. With the folder unknown, the one process of that
/// tool if there's exactly one. A terminal session with no process here has ended, whatever its
/// log last said.
pub fn find<'a>(procs: &'a [Proc], tool: &str, cwd: &str) -> Option<&'a Proc> {
    if cwd.is_empty() {
        let mut of_tool = procs.iter().filter(|p| p.tool == tool);
        return match (of_tool.next(), of_tool.next()) {
            (Some(p), None) => Some(p),
            _ => None,
        };
    }
    procs.iter().find(|p| p.tool == tool && p.cwd == cwd)
}
