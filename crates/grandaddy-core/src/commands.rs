//! The slash commands a Claude Code session understands: the built-in ones worth sending from
//! Grandaddy (names and descriptions as Claude Code 2.1 defines them), plus your own skills and
//! commands from ~/.claude and the session's project.
use crate::util;
use serde::Serialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Command {
    /// without the slash
    pub name: String,
    pub description: String,
    /// built-in | skill | command (yours) | project
    pub source: &'static str,
    /// values it takes, offered as choices (e.g. /model sonnet)
    pub choices: Vec<String>,
    /// takes free text after it (e.g. /btw <question>)
    pub takes_text: bool,
    /// opens a menu in the session, so it's best used in the session's own window
    pub opens_menu: bool,
}

fn builtin(name: &str, description: &str, choices: &[&str], takes_text: bool, opens_menu: bool) -> Command {
    Command { name: name.into(), description: description.into(), source: "built-in", choices: choices.iter().map(|s| s.to_string()).collect(), takes_text, opens_menu }
}

pub fn builtins() -> Vec<Command> {
    vec![
        builtin("compact", "Free up context by summarizing the conversation so far", &[], true, false),
        builtin("clear", "Start a new session with empty context", &[], false, false),
        builtin("context", "Show what fills the context", &[], false, false),
        builtin("usage", "Show session cost, plan usage, and activity stats", &[], false, false),
        builtin("model", "Switch the model", &["fable", "opus", "sonnet", "haiku"], false, false),
        builtin("effort", "Set the model's effort level", &["low", "medium", "high", "xhigh", "max"], false, false),
        builtin("plan", "Turn on plan mode or view the plan", &[], true, false),
        builtin("btw", "Ask a side question without interrupting Claude", &[], true, false),
        builtin("goal", "Set a goal Claude checks before stopping", &[], true, false),
        builtin("recap", "Generate a one-line session recap now", &[], false, false),
        builtin("init", "Initialize a CLAUDE.md file with codebase documentation", &[], false, false),
        builtin("review", "Review a pull request", &[], true, false),
        builtin("subtask", "Send a subagent off with your full context; its result comes back here", &[], true, false),
        builtin("fork", "Start a background agent with the full conversation", &[], true, false),
        builtin("branch", "Create a branch of the current conversation at this point", &[], false, false),
        builtin("add-dir", "Add a working directory", &[], true, false),
        builtin("cd", "Move this session to a new working directory", &[], true, false),
        builtin("copy", "Copy the last response to clipboard", &[], false, false),
        builtin("status", "Show version, account, model, MCP servers, and setup warnings", &[], false, false),
        builtin("skills", "List available skills", &[], false, false),
        builtin("reload-skills", "Pick up skills added or changed on disk during this session", &[], false, false),
        builtin("rewind", "Restore earlier code, conversation, or both", &[], false, true),
        builtin("memory", "Edit CLAUDE.md files and memory settings", &[], false, true),
        builtin("mcp", "Manage MCP servers", &[], false, true),
        builtin("export", "Export this session to a file or clipboard", &[], false, true),
        builtin("hooks", "View hook configurations for tool events", &[], false, true),
        builtin("config", "Open settings", &[], false, true),
        builtin("help", "Show help and available commands", &[], false, true),
        builtin("exit", "End the session", &[], false, false),
    ]
}

/// "description:" from a Markdown file's front matter (first line of it).
fn front(text: &str, key: &str) -> Option<String> {
    let body = text.strip_prefix("---")?;
    let end = body.find("\n---")?;
    body[..end].lines().find_map(|l| {
        let v = l.trim_start().strip_prefix(key)?.trim_start().strip_prefix(':')?.trim();
        let v = v.trim_matches(|c| c == '"' || c == '\'');
        (!v.is_empty() && v != ">" && v != "|").then(|| v.to_string())
    })
}

fn short(d: &str) -> String {
    let first = d.split(". ").next().unwrap_or(d);
    util::one_line(first.trim_end_matches('.'), 140)
}

/// Skills (<dir>/skills/<name>/SKILL.md) and commands (<dir>/commands/**/*.md).
fn from_dir(dir: &Path, skill_source: &'static str, command_source: &'static str, out: &mut Vec<Command>) {
    if let Ok(rd) = fs::read_dir(dir.join("skills")) {
        for e in rd.flatten() {
            let Ok(text) = fs::read_to_string(e.path().join("SKILL.md")) else { continue };
            let name = front(&text, "name").unwrap_or_else(|| e.file_name().to_string_lossy().into_owned());
            out.push(Command { name, description: front(&text, "description").map(|d| short(&d)).unwrap_or_default(), source: skill_source, choices: vec![], takes_text: true, opens_menu: false });
        }
    }
    fn walk(base: &Path, dir: &Path, source: &'static str, out: &mut Vec<Command>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, source, out);
            } else if p.extension().and_then(|x| x.to_str()) == Some("md") {
                // nested folders become namespaces: commands/git/pr.md → /git:pr
                let rel = p.strip_prefix(base).unwrap_or(&p).with_extension("");
                let name = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join(":");
                let text = fs::read_to_string(&p).unwrap_or_default();
                let description = front(&text, "description").unwrap_or_else(|| text.lines().find(|l| !l.trim().is_empty() && !l.starts_with("---")).unwrap_or("").to_string());
                out.push(Command { name, description: short(&description), source, choices: vec![], takes_text: true, opens_menu: false });
            }
        }
    }
    walk(&dir.join("commands"), &dir.join("commands"), command_source, out);
}

/// Every command for a session in `cwd`: built-ins, then the project's, then yours.
pub fn for_folder(cwd: &str) -> Vec<Command> {
    let mut out = builtins();
    let mut extra = vec![];
    if !cwd.is_empty() {
        from_dir(&Path::new(cwd).join(".claude"), "project", "project", &mut extra);
    }
    from_dir(&util::claude_dir(), "skill", "command", &mut extra);
    extra.sort_by(|a, b| a.name.cmp(&b.name));
    for c in extra {
        if !out.iter().any(|o| o.name == c.name) {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn front_matter() {
        let t = "---\nname: animate\ndescription: Build an animation from scratch. More words.\n---\n# x";
        assert_eq!(front(t, "name").as_deref(), Some("animate"));
        assert_eq!(short(&front(t, "description").unwrap()), "Build an animation from scratch");
        assert_eq!(front("no front matter", "name"), None);
    }
}
