//! auto-yes rules: finer than all-or-nothing. One rule per line in ~/.grandaddy/rules:
//!
//! ```text
//! yes  Bash(npm test*)      approve automatically, in every session
//! ask  Bash(git push*)      always ask you, even in an auto-yes session
//! no   Bash(rm -rf /*)      refuse automatically
//! ```
//!
//! The pattern is Claude Code's own `Tool(argument)` shape; the argument is a glob (`*`, `?`)
//! matched against the command, file path or URL. `Tool` alone matches every call to it.
//! The first rule that matches wins. Lines starting with # are comments.
use crate::claude;
use crate::util;
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

pub const USAGE: &str = "a rule looks like:  yes Bash(npm test*)  |  ask Bash(git push*)  |  no Bash(rm -rf /*)";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Rule {
    pub verb: String,
    pub tool: String,
    pub arg: Option<String>,
    pub text: String,
}

pub fn path() -> PathBuf {
    util::state().join("rules")
}

pub fn parse(line: &str) -> Option<Rule> {
    let line = line.trim();
    let (verb, rest) = line.split_once(char::is_whitespace)?;
    if !["yes", "ask", "no"].contains(&verb) {
        return None;
    }
    let rest = rest.trim();
    let (tool, arg) = match rest.find('(') {
        Some(i) if rest.ends_with(')') => (&rest[..i], Some(rest[i + 1..rest.len() - 1].to_string())),
        Some(_) => return None,
        None => (rest, None),
    };
    let ok = !tool.is_empty() && tool.chars().all(|c| c.is_ascii_alphanumeric() || "_*?.:-".contains(c));
    ok.then(|| Rule { verb: verb.into(), tool: tool.into(), arg, text: line.into() })
}

fn rule_lines() -> Vec<String> {
    fs::read_to_string(path()).unwrap_or_default().lines().map(String::from).collect()
}

fn is_rule_line(l: &str) -> bool {
    !l.trim().is_empty() && !l.trim_start().starts_with('#') && parse(l).is_some()
}

pub fn load() -> Vec<Rule> {
    rule_lines().iter().filter(|l| is_rule_line(l)).filter_map(|l| parse(l)).collect()
}

/// Glob match with `*` (any run) and `?` (one char), like Python's fnmatchcase without [sets].
pub fn glob(pat: &str, s: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// First matching rule for this call, or None.
pub fn matching(tool: &str, inp: &Value) -> Option<Rule> {
    load().into_iter().find(|r| {
        if !glob(&r.tool, tool) {
            return false;
        }
        let Some(arg) = &r.arg else { return true };
        let subj = claude::subject(tool, inp);
        glob(arg, &subj) || glob(arg, &util::expand(&subj)) || glob(&util::expand(arg), &util::expand(&subj))
    })
}

pub fn add(line: &str) -> Result<Rule, String> {
    let r = parse(line).ok_or_else(|| USAGE.to_string())?;
    let _ = fs::create_dir_all(util::state());
    let mut lines = rule_lines();
    lines.push(r.text.clone());
    fs::write(path(), lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    Ok(r)
}

/// Remove the n-th rule (1-based, as `load` orders them). Returns the removed line.
pub fn remove(n: usize) -> Result<String, String> {
    let mut lines = rule_lines();
    let idx: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| is_rule_line(l)).map(|(i, _)| i).collect();
    if n == 0 || n > idx.len() {
        return Err(format!("there are {} rules", idx.len()));
    }
    let gone = lines.remove(idx[n - 1]);
    let body = if lines.is_empty() { String::new() } else { lines.join("\n") + "\n" };
    fs::write(path(), body).map_err(|e| e.to_string())?;
    Ok(gone.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn globbing() {
        assert!(glob("npm test*", "npm test -- --watch=false"));
        assert!(glob("rm -rf*", "rm -rf build"));
        assert!(!glob("git push*", "git status"));
        assert!(glob("mcp__*", "mcp__github__create"));
        assert!(glob("a?c", "abc") && !glob("a?c", "abbc"));
        assert!(glob("*", ""));
    }
    #[test]
    fn parsing() {
        assert_eq!(parse("yes Bash(npm test*)").unwrap().arg.as_deref(), Some("npm test*"));
        assert_eq!(parse("no  Read").unwrap().tool, "Read");
        assert!(parse("maybe Bash(x)").is_none());
        assert!(parse("yes Bash(unclosed").is_none());
    }
}
