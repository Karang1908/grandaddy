//! Connect grandaddy to each agent: its approval hook and (optionally) the grandaddy MCP server.
//! Every edit keeps the rest of the file, writes a `.grandaddy.bak` copy first, and is idempotent.
//!
//! | tool        | hook file / event                                         | MCP                        |
//! |-------------|-----------------------------------------------------------|----------------------------|
//! | claude      | ~/.claude/settings.json  PermissionRequest                 | `claude mcp add -s user`   |
//! | codex       | ~/.codex/hooks.json      PermissionRequest (trust in /hooks)| `codex mcp add`            |
//! | antigravity | ~/.gemini/config/hooks.json  PreToolUse                    | ~/.gemini/config/mcp_config.json |
//! | cursor      | ~/.cursor/hooks.json     beforeShellExecution, beforeMCPExecution | ~/.cursor/mcp.json   |
//! | gemini      | ~/.gemini/settings.json  BeforeTool                        | ~/.gemini/settings.json    |
//! | opencode, kilo | — (no hook API)                                         | shown as a snippet         |
use crate::util;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Claude Code may hold a dialog open for the hub this long.
pub const TIMEOUT: u64 = 3600;
/// Other agents: long enough for remote answering (which itself stops at `remote_timeout`).
const OTHER_TIMEOUT: u64 = 900;

pub const HOOK_TOOLS: &[&str] = &["claude", "codex", "antigravity", "cursor", "gemini"];
pub const MCP_TOOLS: &[&str] = &["claude", "codex", "antigravity", "cursor", "gemini", "opencode", "kilo"];

/// The binary hooks and MCP entries should run: this one, or — from inside the app bundle —
/// the `grandaddy` CLI that ships next to it (smaller and faster to start).
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

/// The command an agent runs for the hook.
pub fn hook_command_for(tool: &str) -> String {
    if tool == "claude" { format!("\"{}\" hook", exe()) } else { format!("\"{}\" hook --tool {tool}", exe()) }
}
pub fn hook_command() -> String {
    hook_command_for("claude")
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

fn is_ours(cmd: &str) -> bool {
    our_binary(cmd).is_some()
}

fn home(rel: &str) -> PathBuf {
    util::home().join(rel)
}

pub fn settings_path() -> PathBuf {
    util::claude_dir().join("settings.json")
}

fn load(p: &Path) -> Map<String, Value> {
    fs::read_to_string(p).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(p: &Path, d: &Map<String, Value>) -> std::io::Result<()> {
    if p.exists() {
        fs::copy(p, p.with_extension(format!("{}.grandaddy.bak", p.extension().and_then(|e| e.to_str()).unwrap_or("json"))))?;
    }
    fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("grandaddy.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&Value::Object(d.clone()))? + "\n")?;
    fs::rename(tmp, p)
}

/// Is the agent on this Mac at all?
pub fn detected(tool: &str) -> bool {
    let on_path = |b: &str| Command::new("/bin/sh").args(["-lc", &format!("command -v {b}")]).output().map(|o| o.status.success()).unwrap_or(false);
    match tool {
        "claude" => util::claude_dir().exists(),
        "codex" => home(".codex").exists(),
        "antigravity" => home(".gemini/antigravity").exists() || home(".gemini/antigravity-cli").exists() || home(".gemini/antigravity-ide").exists(),
        "cursor" => home(".cursor").exists(),
        "gemini" => home(".gemini/tmp").exists() || on_path("gemini"),
        "opencode" => home(".local/share/opencode").exists() || home(".config/opencode").exists(),
        "kilo" => home(".local/share/kilo").exists() || home(".config/kilo").exists(),
        _ => false,
    }
}

// ---------- hooks ----------

/// (file, path to the event array inside it)
fn hook_location(tool: &str) -> Option<(PathBuf, Vec<&'static str>)> {
    Some(match tool {
        "claude" => (settings_path(), vec!["hooks", "PermissionRequest"]),
        "codex" => (home(".codex/hooks.json"), vec!["hooks", "PermissionRequest"]),
        "antigravity" => (home(".gemini/config/hooks.json"), vec!["grandaddy", "PreToolUse"]),
        "cursor" => (home(".cursor/hooks.json"), vec!["hooks", "beforeShellExecution"]),
        "gemini" => (home(".gemini/settings.json"), vec!["hooks", "BeforeTool"]),
        _ => return None,
    })
}

fn entry_cmds(e: &Value) -> Vec<String> {
    let mut out: Vec<String> = e.get("command").and_then(|v| v.as_str()).map(String::from).into_iter().collect();
    if let Some(a) = e.get("hooks").and_then(|h| h.as_array()) {
        out.extend(a.iter().filter_map(|h| h.get("command")?.as_str().map(String::from)));
    }
    out
}

fn get_path<'a>(d: &'a Map<String, Value>, path: &[&str]) -> Option<&'a Vec<Value>> {
    let mut v = d.get(path[0])?;
    for k in &path[1..] {
        v = v.get(*k)?;
    }
    v.as_array()
}

fn array_at<'a>(d: &'a mut Map<String, Value>, path: &[&str]) -> &'a mut Vec<Value> {
    let mut v = d.entry(path[0].to_string()).or_insert_with(|| json!({}));
    for (i, k) in path[1..].iter().enumerate() {
        let last = i == path.len() - 2;
        if !v.is_object() {
            *v = json!({});
        }
        v = v.as_object_mut().unwrap().entry(k.to_string()).or_insert_with(|| if last { json!([]) } else { json!({}) });
    }
    if !v.is_array() {
        *v = json!([]);
    }
    v.as_array_mut().unwrap()
}

#[derive(Debug, Clone, Serialize)]
pub struct HookStatus {
    pub tool: String,
    pub detected: bool,
    pub installed: bool,
    /// what's left for you to do after installing
    pub note: Option<String>,
}

/// A working entry: one of our binaries, still on disk (and for Claude, with the long timeout).
pub fn hook_installed(tool: &str) -> bool {
    let Some((file, path)) = hook_location(tool) else { return false };
    let d = load(&file);
    get_path(&d, &path)
        .map(|a| {
            a.iter().any(|e| {
                let timeout_ok = tool != "claude" || e.get("hooks").and_then(|h| h.as_array()).map(|hs| hs.iter().any(|h| h.get("timeout").and_then(|t| t.as_u64()) == Some(TIMEOUT))).unwrap_or(false);
                timeout_ok && entry_cmds(e).iter().any(|c| our_binary(c).map(|p| p.exists()).unwrap_or(false))
            })
        })
        .unwrap_or(false)
}

pub fn installed() -> bool {
    hook_installed("claude")
}

pub fn hook_status(tool: &str) -> HookStatus {
    let note = match tool {
        "codex" => Some("Codex runs new hooks only after you trust them: open Codex and use /hooks.".to_string()),
        "cursor" => Some("Cursor shows its own prompt for anything grandaddy doesn't decide.".to_string()),
        _ => None,
    };
    HookStatus { tool: tool.into(), detected: detected(tool), installed: hook_installed(tool), note }
}

fn hook_entry(tool: &str) -> Value {
    let cmd = hook_command_for(tool);
    match tool {
        "claude" => json!({"hooks": [{"type": "command", "command": cmd, "timeout": TIMEOUT}]}),
        "codex" => json!({"hooks": [{"type": "command", "command": cmd, "timeout": OTHER_TIMEOUT, "statusMessage": "Checking grandaddy"}]}),
        "antigravity" => json!({"matcher": ".*", "hooks": [{"type": "command", "command": cmd, "timeout": OTHER_TIMEOUT}]}),
        "cursor" => json!({"command": cmd, "timeout": OTHER_TIMEOUT}),
        "gemini" => json!({"matcher": ".*", "hooks": [{"name": "grandaddy", "type": "command", "command": cmd, "timeout": OTHER_TIMEOUT * 1000}]}),
        _ => Value::Null,
    }
}

/// "installed", "updated" (replaced an older entry) or "already".
pub fn install_hook(tool: &str) -> Result<&'static str, String> {
    let (file, path) = hook_location(tool).ok_or_else(|| format!("{tool} has no hook system grandaddy can use"))?;
    if hook_installed(tool) && get_path(&load(&file), &path).map(|a| a.iter().filter(|e| entry_cmds(e).iter().any(|c| is_ours(c))).count() == 1).unwrap_or(false) {
        return Ok("already");
    }
    let mut d = load(&file);
    let mut events = vec![path.clone()];
    if tool == "cursor" {
        events.push(vec!["hooks", "beforeMCPExecution"]);
        d.entry("version").or_insert(json!(1));
    }
    if tool == "antigravity" {
        if let Some(Value::Object(g)) = d.get_mut("grandaddy") {
            g.insert("enabled".into(), json!(true));
        } else {
            d.insert("grandaddy".into(), json!({"enabled": true}));
        }
    }
    let mut had_old = false;
    for ev in events {
        let arr = array_at(&mut d, &ev);
        had_old |= arr.iter().any(|e| entry_cmds(e).iter().any(|c| is_ours(c)));
        arr.retain(|e| !entry_cmds(e).iter().any(|c| is_ours(c)));
        arr.push(hook_entry(tool));
    }
    save(&file, &d).map_err(|e| e.to_string())?;
    Ok(if had_old { "updated" } else { "installed" })
}

pub fn install() -> std::io::Result<&'static str> {
    install_hook("claude").map_err(std::io::Error::other)
}

pub fn uninstall_hook(tool: &str) -> Result<bool, String> {
    let (file, path) = hook_location(tool).ok_or("no hook for that tool")?;
    let mut d = load(&file);
    let mut removed = false;
    let mut events = vec![path.clone()];
    if tool == "cursor" {
        events.push(vec!["hooks", "beforeMCPExecution"]);
    }
    for ev in &events {
        if get_path(&d, ev).is_some() {
            let arr = array_at(&mut d, ev);
            let before = arr.len();
            arr.retain(|e| !entry_cmds(e).iter().any(|c| is_ours(c)));
            removed |= arr.len() != before;
        }
    }
    if !removed {
        return Ok(false);
    }
    if tool == "antigravity" {
        d.remove("grandaddy");
    }
    if tool == "claude" {
        if let Some(h) = d.get_mut("hooks").and_then(|h| h.as_object_mut()) {
            if h.get("PermissionRequest").and_then(|v| v.as_array()).map(|a| a.is_empty()).unwrap_or(false) {
                h.remove("PermissionRequest");
            }
            if h.is_empty() {
                d.remove("hooks");
            }
        }
        let _ = fs::remove_dir_all(util::auto_dir());
    }
    // leave no empty lists or tables behind where our entries were
    for ev in &events {
        let (parent, key) = (ev[0], ev[1]);
        if let Some(Value::Object(p)) = d.get_mut(parent) {
            if p.get(key).and_then(|v| v.as_array()).map(|a| a.is_empty()).unwrap_or(false) {
                p.remove(key);
            }
            if p.is_empty() {
                d.remove(parent);
            }
        }
    }
    save(&file, &d).map_err(|e| e.to_string())?;
    Ok(true)
}

pub fn uninstall() -> std::io::Result<bool> {
    uninstall_hook("claude").map_err(std::io::Error::other)
}

// ---------- MCP (S2S / A2A, experimental) ----------

fn mcp_file(tool: &str) -> Option<PathBuf> {
    Some(match tool {
        "antigravity" => home(".gemini/config/mcp_config.json"),
        "cursor" => home(".cursor/mcp.json"),
        "gemini" => home(".gemini/settings.json"),
        _ => return None,
    })
}

pub fn mcp_installed(tool: &str) -> bool {
    match tool {
        "claude" => load(&util::home().join(".claude.json")).get("mcpServers").and_then(|m| m.get("grandaddy")).is_some(),
        "codex" => fs::read_to_string(home(".codex/config.toml")).map(|s| s.contains("[mcp_servers.grandaddy]")).unwrap_or(false),
        _ => mcp_file(tool).map(|f| load(&f).get("mcpServers").and_then(|m| m.get("grandaddy")).is_some()).unwrap_or(false),
    }
}

/// What to paste where grandaddy can't safely edit the config itself (OpenCode/Kilo use JSONC).
pub fn mcp_snippet(tool: &str) -> String {
    let exe = exe();
    match tool {
        "opencode" => format!("~/.config/opencode/opencode.jsonc → \"mcp\": {{ \"servers\": {{ \"grandaddy\": {{ \"type\": \"local\", \"command\": [\"{exe}\", \"mcp\"] }} }} }}"),
        "kilo" => format!("~/.config/kilo/kilo.jsonc → \"mcp\": {{ \"grandaddy\": {{ \"type\": \"local\", \"command\": [\"{exe}\", \"mcp\"] }} }}"),
        _ => format!("{{\"command\": \"{exe}\", \"args\": [\"mcp\"]}}"),
    }
}

fn run(cmd: &str, args: &[&str]) -> Result<(), String> {
    let o = Command::new(cmd).args(args).output().map_err(|e| format!("couldn't run {cmd}: {e}"))?;
    if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
}

pub fn install_mcp(tool: &str) -> Result<String, String> {
    let exe = exe();
    match tool {
        "claude" => {
            if !mcp_installed("claude") {
                run("claude", &["mcp", "add", "-s", "user", "grandaddy", "--", &exe, "mcp"])?;
            }
        }
        "codex" => {
            if !mcp_installed("codex") {
                run("codex", &["mcp", "add", "grandaddy", "--", &exe, "mcp"])?;
            }
        }
        "opencode" | "kilo" => return Err(format!("add this to its config by hand: {}", mcp_snippet(tool))),
        _ => {
            let f = mcp_file(tool).ok_or("unknown tool")?;
            let mut d = load(&f);
            let servers = d.entry("mcpServers").or_insert_with(|| json!({}));
            if let Value::Object(m) = servers {
                m.insert("grandaddy".into(), json!({"command": exe, "args": ["mcp"]}));
            }
            save(&f, &d).map_err(|e| e.to_string())?;
        }
    }
    Ok(format!("grandaddy MCP server added to {}", crate::providers::label(tool)))
}

pub fn uninstall_mcp(tool: &str) -> Result<(), String> {
    match tool {
        "claude" => run("claude", &["mcp", "remove", "-s", "user", "grandaddy"]),
        "codex" => run("codex", &["mcp", "remove", "grandaddy"]),
        _ => {
            let f = mcp_file(tool).ok_or("unknown tool")?;
            let mut d = load(&f);
            if let Some(Value::Object(m)) = d.get_mut("mcpServers") {
                m.remove("grandaddy");
            }
            save(&f, &d).map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognises_our_commands() {
        assert!(our_binary("\"/Users/x/.cargo/bin/grandaddy\" hook").is_some());
        assert!(our_binary("\"/Applications/Grandaddy.app/Contents/MacOS/grandaddy-app\" hook --tool codex").is_some());
        assert!(our_binary("\"/old/auto-yes\" hook").is_some());
        assert!(our_binary("\"/usr/bin/python3\" hook.py").is_none());
        assert!(our_binary("/x/grandaddy hook").is_none());
    }
}
