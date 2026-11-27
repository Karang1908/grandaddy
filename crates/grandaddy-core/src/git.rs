//! Git and GitHub for a session's folder: status, changes, diffs, history, branches and pull
//! requests (via `gh`). Reads are cheap; every write is an explicit action the caller asks for,
//! and nothing here ever force-pushes or force-deletes.
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn git(cwd: &str, args: &[&str]) -> Option<Output> {
    Command::new("git").arg("-C").arg(cwd).args(args).env("GIT_TERMINAL_PROMPT", "0").output().ok()
}

fn out(o: &Option<Output>) -> String {
    o.as_ref().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

/// Ok(stdout) or Err(the tool's own message).
fn run(cmd: &str, cwd: &str, args: &[&str]) -> Result<String, String> {
    let o = Command::new(cmd).args(args).current_dir(cwd).env("GIT_TERMINAL_PROMPT", "0").output().map_err(|e| format!("couldn't run {cmd}: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)).trim().to_string();
    if o.status.success() { Ok(text) } else { Err(if text.is_empty() { format!("{cmd} failed") } else { text }) }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Repo {
    pub root: String,
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<Change>,
    /// "owner/repo" when the remote is on GitHub and `gh` is available
    pub github: Option<String>,
    /// the branch pull requests go into (origin's HEAD), e.g. "main"
    pub default_branch: Option<String>,
    pub detached: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Change {
    pub path: String,
    /// modified, added, deleted, renamed, untracked, conflicted
    pub kind: String,
    pub staged: bool,
}

/// Repository state for a folder, or None if it isn't in a git repository.
pub fn repo(cwd: &str) -> Option<Repo> {
    if cwd.is_empty() || !Path::new(cwd).is_dir() {
        return None;
    }
    let root = out(&git(cwd, &["rev-parse", "--show-toplevel"])).trim().to_string();
    if root.is_empty() {
        return None;
    }
    let mut r = Repo { root: root.clone(), ..Default::default() };
    for line in out(&git(cwd, &["status", "--porcelain=v2", "--branch", "-z"])).split('\0') {
        if let Some(h) = line.strip_prefix("# branch.head ") {
            r.detached = h == "(detached)";
            r.branch = h.to_string();
        } else if let Some(u) = line.strip_prefix("# branch.upstream ") {
            r.upstream = Some(u.to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split_whitespace();
            r.ahead = it.next().and_then(|a| a.trim_start_matches('+').parse().ok()).unwrap_or(0);
            r.behind = it.next().and_then(|b| b.trim_start_matches('-').parse().ok()).unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("? ") {
            r.changes.push(Change { path: rest.to_string(), kind: "untracked".into(), staged: false });
        } else if line.starts_with("1 ") || line.starts_with("2 ") || line.starts_with("u ") {
            // field counts: ordinary 9, renamed 10, unmerged 11 — the path is always last
            let fields = if line.starts_with('2') { 10 } else if line.starts_with('u') { 11 } else { 9 };
            let parts: Vec<&str> = line.splitn(fields, ' ').collect();
            let xy = parts.get(1).copied().unwrap_or("..");
            let path = parts.last().copied().unwrap_or("").to_string();
            let (x, y) = (xy.chars().next().unwrap_or('.'), xy.chars().nth(1).unwrap_or('.'));
            let code = if x != '.' { x } else { y };
            let kind = if line.starts_with("u ") {
                "conflicted"
            } else {
                match code {
                    'A' => "added",
                    'D' => "deleted",
                    'R' | 'C' => "renamed",
                    _ => "modified",
                }
            };
            r.changes.push(Change { path, kind: kind.into(), staged: x != '.' });
        }
    }
    // origin's HEAD when it's recorded; repositories pushed from a local `git init` often lack it,
    // so fall back to whichever of main / master origin has
    r.default_branch = Some(out(&git(cwd, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])).trim().trim_start_matches("origin/").to_string())
        .filter(|b| !b.is_empty())
        .or_else(|| ["main", "master"].iter().find(|b| git(cwd, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{b}")]).map(|o| o.status.success()).unwrap_or(false)).map(|b| b.to_string()));
    let remote = out(&git(cwd, &["remote", "get-url", "origin"])).trim().to_string();
    r.github = github_slug(&remote).filter(|_| gh_available());
    Some(r)
}

fn github_slug(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/").or_else(|| url.strip_prefix("git@github.com:")).or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    Some(rest.trim_end_matches(".git").trim_end_matches('/').to_string())
}

fn gh_available() -> bool {
    Command::new("gh").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[derive(Debug, Clone, Serialize)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub author: String,
    pub when: f64,
}

pub fn log(cwd: &str, n: usize) -> Vec<Commit> {
    out(&git(cwd, &["log", &format!("-n{n}"), "--format=%h%x1f%s%x1f%an%x1f%ct"]))
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\u{1f}').collect();
            (p.len() == 4).then(|| Commit { hash: p[0].into(), subject: p[1].into(), author: p[2].into(), when: p[3].parse().unwrap_or(0.0) })
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub upstream: Option<String>,
    pub when: f64,
    pub subject: String,
}

/// Local branches, most recently committed first.
pub fn branches(cwd: &str) -> Vec<Branch> {
    out(&git(cwd, &["for-each-ref", "--sort=-committerdate", "refs/heads", "--format=%(HEAD)%1f%(refname:short)%1f%(upstream:short)%1f%(committerdate:unix)%1f%(contents:subject)"]))
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\u{1f}').collect();
            (p.len() == 5).then(|| Branch {
                current: p[0] == "*",
                name: p[1].into(),
                upstream: (!p[2].is_empty()).then(|| p[2].to_string()),
                when: p[3].parse().unwrap_or(0.0),
                subject: p[4].into(),
            })
        })
        .collect()
}

/// The diff for one file (staged and unstaged together), or the whole tree when `path` is None.
pub fn diff(cwd: &str, path: Option<&str>) -> String {
    let mut args = vec!["diff", "HEAD", "--no-color", "--no-ext-diff"];
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    let d = out(&git(cwd, &args));
    if !d.is_empty() {
        return d;
    }
    // untracked files have no diff against HEAD: show them as all-new
    match path {
        Some(p) => {
            let full = Path::new(cwd).join(p);
            std::fs::read_to_string(&full).map(|t| t.lines().map(|l| format!("+{l}\n")).collect()).unwrap_or_default()
        }
        None => d,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub branch: String,
    pub base: String,
    pub state: String,
    pub draft: bool,
    /// APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED or empty
    pub review: String,
    /// passing, failing, pending or none
    pub checks: String,
    pub mergeable: String,
    pub url: String,
    pub author: String,
    pub additions: u64,
    pub deletions: u64,
}

const PR_FIELDS: &str = "number,title,headRefName,baseRefName,state,isDraft,reviewDecision,statusCheckRollup,mergeable,url,author,additions,deletions";

fn pr_from(v: &Value) -> Pr {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let checks = v.get("statusCheckRollup").and_then(|c| c.as_array()).map(|a| {
        let state = |c: &Value| c.get("conclusion").or_else(|| c.get("state")).and_then(|x| x.as_str()).unwrap_or("").to_uppercase();
        if a.is_empty() {
            "none"
        } else if a.iter().any(|c| matches!(state(c).as_str(), "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT")) {
            "failing"
        } else if a.iter().all(|c| matches!(state(c).as_str(), "SUCCESS" | "NEUTRAL" | "SKIPPED")) {
            "passing"
        } else {
            "pending"
        }
    });
    Pr {
        number: v.get("number").and_then(|x| x.as_u64()).unwrap_or(0),
        title: s("title"),
        branch: s("headRefName"),
        base: s("baseRefName"),
        state: s("state"),
        draft: v.get("isDraft").and_then(|x| x.as_bool()).unwrap_or(false),
        review: s("reviewDecision"),
        checks: checks.unwrap_or("none").to_string(),
        mergeable: s("mergeable"),
        url: s("url"),
        author: v.pointer("/author/login").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        additions: v.get("additions").and_then(|x| x.as_u64()).unwrap_or(0),
        deletions: v.get("deletions").and_then(|x| x.as_u64()).unwrap_or(0),
    }
}

/// Open pull requests for the folder's GitHub repository.
pub fn prs(cwd: &str) -> Result<Vec<Pr>, String> {
    let text = run("gh", cwd, &["pr", "list", "--state", "open", "--limit", "30", "--json", PR_FIELDS])?;
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(v.as_array().map(|a| a.iter().map(pr_from).collect()).unwrap_or_default())
}

/// The pull request for the current branch, if there is one.
pub fn current_pr(cwd: &str) -> Option<Pr> {
    let text = run("gh", cwd, &["pr", "view", "--json", PR_FIELDS]).ok()?;
    serde_json::from_str::<Value>(&text).ok().map(|v| pr_from(&v))
}

// ---------- actions ----------

fn valid_branch(name: &str) -> Result<(), String> {
    let ok = !name.is_empty() && !name.starts_with('-') && Command::new("git").args(["check-ref-format", "--branch", name]).output().map(|o| o.status.success()).unwrap_or(false);
    if ok { Ok(()) } else { Err(format!("“{name}” isn't a valid branch name")) }
}

pub fn switch(cwd: &str, branch: &str) -> Result<String, String> {
    valid_branch(branch)?;
    run("git", cwd, &["switch", branch]).map(|_| format!("Switched to {branch}"))
}

pub fn create_branch(cwd: &str, name: &str) -> Result<String, String> {
    valid_branch(name)?;
    run("git", cwd, &["switch", "-c", name]).map(|_| format!("Created and switched to {name}"))
}

/// Stage everything and commit.
pub fn commit_all(cwd: &str, message: &str) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("write a commit message first".into());
    }
    run("git", cwd, &["add", "-A"])?;
    let out = run("git", cwd, &["commit", "-m", message.trim()])?;
    Ok(out.lines().next().unwrap_or("Committed").to_string())
}

pub fn pull(cwd: &str) -> Result<String, String> {
    run("git", cwd, &["pull", "--ff-only"]).map(|o| o.lines().last().unwrap_or("Up to date").to_string())
}

/// Push the current branch, setting its upstream the first time. Never forces.
pub fn push(cwd: &str) -> Result<String, String> {
    let r = repo(cwd).ok_or("not a git repository")?;
    if r.detached {
        return Err("you're not on a branch".into());
    }
    let args: Vec<&str> = if r.upstream.is_some() { vec!["push"] } else { vec!["push", "-u", "origin", &r.branch] };
    run("git", cwd, &args).map(|_| format!("Pushed {}", r.branch))
}

/// Open a pull request for the current branch (pushing it first if needed).
pub fn create_pr(cwd: &str, title: &str, body: &str, draft: bool) -> Result<String, String> {
    let r = repo(cwd).ok_or("not a git repository")?;
    if r.default_branch.as_deref() == Some(r.branch.as_str()) {
        return Err(format!("You're on {}, the branch pull requests go into. Create a branch for this work first.", r.branch));
    }
    if r.upstream.is_none() || r.ahead > 0 {
        push(cwd)?;
    }
    let mut args = vec!["pr", "create", "--head", &r.branch];
    if title.trim().is_empty() {
        args.push("--fill");
    } else {
        args.extend(["--title", title.trim(), "--body", body]);
    }
    if draft {
        args.push("--draft");
    }
    let out = run("gh", cwd, &args)?;
    Ok(out.lines().rev().find(|l| l.contains("github.com")).unwrap_or("Pull request opened").to_string())
}

/// method: squash | merge | rebase
pub fn merge_pr(cwd: &str, number: u64, method: &str, delete_branch: bool) -> Result<String, String> {
    let flag = match method {
        "merge" => "--merge",
        "rebase" => "--rebase",
        _ => "--squash",
    };
    let n = number.to_string();
    let mut args = vec!["pr", "merge", &n, flag];
    if delete_branch {
        args.push("--delete-branch");
    }
    run("gh", cwd, &args).map(|_| format!("Merged #{number}"))
}

/// Delete a local branch only if it's fully merged (`git branch -d`, never `-D`).
pub fn delete_branch(cwd: &str, name: &str) -> Result<String, String> {
    valid_branch(name)?;
    run("git", cwd, &["branch", "-d", name]).map(|_| format!("Deleted {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_remotes() {
        assert_eq!(github_slug("https://github.com/a/b.git").as_deref(), Some("a/b"));
        assert_eq!(github_slug("git@github.com:a/b.git").as_deref(), Some("a/b"));
        assert_eq!(github_slug("https://gitlab.com/a/b"), None);
    }
}
