//! Connect grandaddy to Claude Code: the approval hook (~/.claude/settings.json), the MCP server
//! (`claude mcp add -s user`), and terminal control (a `claude` function in your shell's rc file
//! that runs it through `grandaddy run`). Every edit keeps the rest of the file, backs it up
//! first, and can be undone.
use crate::util;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Claude Code may hold a dialog open for the hub this long.
pub const TIMEOUT: u64 = 3600;

/// The binary hooks and the shell should run: this one, or, from inside the app bundle, the
/// `grandaddy` CLI that ships next to it.
pub fn exe() -> String {
    let me = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok()).unwrap_or_default();
    if me.file_name().and_then(|n| n.to_str()) == Some("grandaddy-app") {
        let cli = me.with_file_name("grandaddy");
        if cli.exists() {
            return cli.display().to_string();
        }
    }
    me.display().to_string()
}

pub fn hook_command() -> String {
    format!("\"{}\" hook", exe())
}

/// The binary a hook command runs, if it's one of ours (any name this tool has had, CLI or app).
fn our_binary(cmd: &str) -> Option<PathBuf> {
    let rest = cmd.strip_prefix('"')?;
    let (path, after) = rest.split_once('"')?;
    if !after.trim_start().starts_with("hook") {
        return None;
    }
    let name = Path::new(path).file_name()?.to_str()?;
    ["auto-yes", "granddaddy", "grandaddy", "grandaddy-app", "Grandaddy"].contains(&name).then(|| PathBuf::from(path))
}

pub fn settings_path() -> PathBuf {
    util::claude_dir().join("settings.json")
}

fn load(p: &Path) -> Map<String, Value> {
    fs::read_to_string(p).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(p: &Path, d: &Map<String, Value>) -> std::io::Result<()> {
    if p.exists() {
        fs::copy(p, p.with_extension("json.grandaddy.bak"))?;
    }
    fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("grandaddy.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&Value::Object(d.clone()))? + "\n")?;
    fs::rename(tmp, p)
}

fn group_cmds(g: &Value) -> Vec<String> {
    g.get("hooks").and_then(|h| h.as_array()).map(|a| a.iter().filter_map(|h| h.get("command")?.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// A working entry: one of our binaries, still on disk, with the long timeout the hub needs.
fn current(g: &Value) -> bool {
    g.get("hooks")
        .and_then(|h| h.as_array())
        .map(|a| {
            a.iter().any(|h| {
                let cmd = h.get("command").and_then(|v| v.as_str()).unwrap_or("");
                our_binary(cmd).map(|p| p.exists()).unwrap_or(false) && h.get("timeout").and_then(|v| v.as_u64()) == Some(TIMEOUT)
            })
        })
        .unwrap_or(false)
}

pub fn installed() -> bool {
    load(&settings_path()).get("hooks").and_then(|h| h.get("PermissionRequest")).and_then(|v| v.as_array()).map(|a| a.iter().any(current)).unwrap_or(false)
}

/// "installed", "updated" (replaced an older entry) or "already".
pub fn install() -> std::io::Result<&'static str> {
    let mut d = load(&settings_path());
    let hooks = d.entry("hooks").or_insert_with(|| json!({}));
    let groups = hooks.as_object_mut().unwrap().entry("PermissionRequest").or_insert_with(|| json!([]));
    let arr = groups.as_array_mut().unwrap();
    let ours = |g: &Value| group_cmds(g).iter().any(|c| our_binary(c).is_some());
    if arr.iter().any(current) && arr.iter().filter(|g| ours(g)).count() == 1 {
        return Ok("already");
    }
    let had_old = arr.iter().any(ours);
    arr.retain(|g| !ours(g));
    arr.push(json!({"hooks": [{"type": "command", "command": hook_command(), "timeout": TIMEOUT}]}));
    save(&settings_path(), &d)?;
    Ok(if had_old { "updated" } else { "installed" })
}

pub fn uninstall() -> std::io::Result<bool> {
    let mut d = load(&settings_path());
    let Some(hooks) = d.get_mut("hooks").and_then(|h| h.as_object_mut()) else { return Ok(false) };
    let Some(arr) = hooks.get_mut("PermissionRequest").and_then(|v| v.as_array_mut()) else { return Ok(false) };
    let before = arr.len();
    arr.retain(|g| !group_cmds(g).iter().any(|c| our_binary(c).is_some()));
    if arr.len() == before {
        return Ok(false);
    }
    if arr.is_empty() {
        hooks.remove("PermissionRequest");
        if hooks.is_empty() {
            d.remove("hooks");
        }
    }
    save(&settings_path(), &d)?;
    let _ = fs::remove_dir_all(util::auto_dir());
    Ok(true)
}

// ---------- MCP (experimental) ----------

pub fn mcp_installed() -> bool {
    load(&util::home().join(".claude.json")).get("mcpServers").and_then(|m| m.get("grandaddy")).is_some()
}

fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    let o = Command::new(cmd).args(args).output().map_err(|e| format!("couldn't run {cmd}: {e}"))?;
    if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
}

pub fn install_mcp() -> Result<String, String> {
    if !mcp_installed() {
        run("claude", &["mcp", "add", "-s", "user", "grandaddy", "--", &exe(), "mcp"])?;
    }
    Ok("grandaddy MCP server added to Claude Code".into())
}

pub fn uninstall_mcp() -> Result<(), String> {
    run("claude", &["mcp", "remove", "-s", "user", "grandaddy"])
}

// ---------- terminal control ----------

const BEGIN: &str = "# >>> grandaddy: claude sessions you can control from Grandaddy >>>";
const END: &str = "# <<< grandaddy <<<";

/// The shell files we touch: ~/.zshrc always, ~/.bashrc if you have one.
fn rc_files() -> Vec<PathBuf> {
    let mut out = vec![util::home().join(".zshrc")];
    let bash = util::home().join(".bashrc");
    if bash.exists() {
        out.push(bash);
    }
    out
}

fn shell_block() -> String {
    format!(
        "{BEGIN}\n# `claude` runs inside Grandaddy's terminal so the app can send it messages, slash commands and\n# mode changes. Non-interactive uses (claude -p, pipes, subcommands) run claude directly.\n# Remove with: grandaddy terminal off\nclaude() {{ \"{}\" run claude \"$@\"; }}\n{END}\n",
        exe()
    )
}

fn strip_block(text: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for line in text.split_inclusive('\n') {
        if line.trim_end() == BEGIN {
            skip = true;
            continue;
        }
        if skip {
            if line.trim_end() == END {
                skip = false;
            }
            continue;
        }
        out.push_str(line);
    }
    out
}

pub fn terminal_installed() -> bool {
    fs::read_to_string(util::home().join(".zshrc")).map(|t| t.contains(BEGIN)).unwrap_or(false)
}

/// Add (or refresh) the block in each rc file. Backs each file up first.
pub fn install_terminal() -> Result<String, String> {
    for p in rc_files() {
        let old = fs::read_to_string(&p).unwrap_or_default();
        if p.exists() {
            fs::copy(&p, p.with_file_name(format!("{}.grandaddy.bak", p.file_name().unwrap().to_string_lossy()))).map_err(|e| e.to_string())?;
        }
        let mut new = strip_block(&old);
        if !new.is_empty() && !new.ends_with('\n') {
            new.push('\n');
        }
        new.push_str(&shell_block());
        fs::write(&p, new).map_err(|e| e.to_string())?;
    }
    Ok("New Claude sessions you start with `claude` can be controlled from Grandaddy. Open a new terminal tab to pick it up.".into())
}

pub fn uninstall_terminal() -> Result<String, String> {
    for p in rc_files() {
        if let Ok(old) = fs::read_to_string(&p) {
            if old.contains(BEGIN) {
                fs::write(&p, strip_block(&old)).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok("`claude` runs on its own again in new terminal tabs.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognises_our_commands() {
        assert!(our_binary("\"/Users/x/.cargo/bin/grandaddy\" hook").is_some());
        assert!(our_binary("\"/Applications/Grandaddy.app/Contents/MacOS/grandaddy\" hook").is_some());
        assert!(our_binary("\"/usr/bin/python3\" hook.py").is_none());
    }
    #[test]
    fn shell_block_round_trip() {
        let original = "export PATH=$HOME/bin:$PATH\nalias ll='ls -l'\n";
        let with = format!("{original}{}", shell_block());
        assert!(with.contains("claude() {"));
        assert_eq!(strip_block(&with), original);
        assert_eq!(strip_block(&format!("{}{}", strip_block(&with), shell_block())).trim_end(), original.trim_end());
    }
}
