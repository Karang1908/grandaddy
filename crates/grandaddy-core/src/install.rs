//! Register / remove the hook in ~/.claude/settings.json (key order preserved).
use crate::util;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::PathBuf;

pub const TIMEOUT: u64 = 3600; // the hook may hold a dialog open for the hub this long

pub fn settings_path() -> PathBuf {
    util::claude_dir().join("settings.json")
}

/// The command Claude runs: this binary, by its real path.
pub fn hook_command() -> String {
    let exe = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok()).unwrap_or_default();
    format!("\"{}\" hook", exe.display())
}

/// The binary a hook command runs, if it's one of ours (any name this tool has had, CLI or app).
fn our_binary(cmd: &str) -> Option<PathBuf> {
    let path = cmd.strip_suffix("\" hook")?.strip_prefix('"')?;
    let name = std::path::Path::new(path).file_name()?.to_str()?;
    ["auto-yes", "granddaddy", "grandaddy", "grandaddy-app", "Grandaddy"].contains(&name).then(|| PathBuf::from(path))
}

fn is_ours(cmd: &str) -> bool {
    cmd == hook_command() || our_binary(cmd).is_some()
}

fn load() -> Map<String, Value> {
    fs::read_to_string(settings_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(d: &Map<String, Value>) -> std::io::Result<()> {
    let p = settings_path();
    if p.exists() {
        fs::copy(&p, p.with_extension("json.grandaddy.bak"))?;
    }
    let tmp = p.with_extension("json.grandaddy.tmp");
    fs::create_dir_all(p.parent().unwrap())?;
    fs::write(&tmp, serde_json::to_string_pretty(&Value::Object(d.clone()))? + "\n")?;
    fs::rename(tmp, p)
}

fn group_cmds(g: &Value) -> Vec<String> {
    g.get("hooks").and_then(|h| h.as_array()).map(|a| a.iter().filter_map(|h| h.get("command")?.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// A working entry: one of our binaries, still on disk, with the long timeout the hub needs.
fn current(g: &Value) -> bool {
    g.get("hooks").and_then(|h| h.as_array()).map(|a| {
        a.iter().any(|h| {
            let cmd = h.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let exists = our_binary(cmd).map(|p| p.exists()).unwrap_or(false);
            exists && h.get("timeout").and_then(|v| v.as_u64()) == Some(TIMEOUT)
        })
    }).unwrap_or(false)
}

pub fn installed() -> bool {
    load().get("hooks").and_then(|h| h.get("PermissionRequest")).and_then(|v| v.as_array()).map(|a| a.iter().any(current)).unwrap_or(false)
}

/// "installed", "updated" (replaced an older entry) or "already".
pub fn install() -> std::io::Result<&'static str> {
    let mut d = load();
    let hooks = d.entry("hooks").or_insert_with(|| json!({}));
    let groups = hooks.as_object_mut().unwrap().entry("PermissionRequest").or_insert_with(|| json!([]));
    let arr = groups.as_array_mut().unwrap();
    let ours = |g: &Value| group_cmds(g).iter().any(|c| is_ours(c));
    if arr.iter().any(current) && arr.iter().filter(|g| ours(g)).count() == 1 {
        return Ok("already");
    }
    let had_old = arr.iter().any(ours);
    arr.retain(|g| !ours(g));
    arr.push(json!({"hooks": [{"type": "command", "command": hook_command(), "timeout": TIMEOUT}]}));
    save(&d)?;
    Ok(if had_old { "updated" } else { "installed" })
}

pub fn uninstall() -> std::io::Result<bool> {
    let mut d = load();
    let Some(hooks) = d.get_mut("hooks").and_then(|h| h.as_object_mut()) else { return Ok(false) };
    let Some(arr) = hooks.get_mut("PermissionRequest").and_then(|v| v.as_array_mut()) else { return Ok(false) };
    let before = arr.len();
    arr.retain(|g| !group_cmds(g).iter().any(|c| is_ours(c)));
    if arr.len() == before {
        return Ok(false);
    }
    if arr.is_empty() {
        hooks.remove("PermissionRequest");
        if hooks.is_empty() {
            d.remove("hooks");
        }
    }
    save(&d)?;
    let _ = fs::remove_dir_all(util::auto_dir());
    Ok(true)
}
