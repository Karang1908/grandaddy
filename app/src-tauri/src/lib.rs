//! Grandaddy for macOS: a thin Tauri layer over grandaddy-core (the same code the CLI runs),
//! plus the menu bar icon, the quick panel, and the background work (staying awake, loops,
//! the phone page, notifications).
use grandaddy_core::claude::{self, Decision, Past, Tokens};
use grandaddy_core::sessions::{self, Session};
use grandaddy_core::util::{self, now};
use grandaddy_core::{awake, config, git, goals, install, launch, limits, loops, phone, prompt, providers, rules, web};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, RunEvent, WindowEvent};

type Res<T> = Result<T, String>;

#[derive(Serialize)]
struct SessionView {
    #[serde(flatten)]
    s: Session,
    folder: String,
    ago: String,
}

fn view(s: Session) -> SessionView {
    SessionView { folder: util::short(&s.cwd), ago: util::ago(now() - s.since), s }
}

fn find(key: &str) -> Res<Session> {
    sessions::collect().into_iter().find(|s| s.key == key).ok_or_else(|| "That session has ended.".to_string())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

fn outcome((ok, out): (bool, String)) -> Res<String> {
    let last = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(if ok { "Done" } else { "Failed" }).to_string();
    if ok { Ok(last) } else { Err(last) }
}

// ---------- sessions ----------

async fn list_sessions() -> Res<Vec<SessionView>> {
    blocking(|| sessions::collect().into_iter().map(view).collect()).await
}

async fn session_detail(key: String) -> Res<Value> {
    blocking(move || {
        let all = sessions::collect();
        let s = all.iter().find(|s| s.key == key).ok_or("That session has ended.")?;
        let act = sessions::activity(s);
        let signals = claude::signals(&act, &s.status);
        let tokens = s.transcript.as_ref().filter(|_| s.tool == "claude").and_then(|p| claude::session_tokens(p));
        let twins: Vec<&str> = all.iter().filter(|o| o.key != s.key && !o.cwd.is_empty() && o.cwd == s.cwd).map(|o| o.name.as_str()).collect();
        let conflicts = sessions::conflicts(s, &all);
        Ok(json!({
            "session": view(s.clone()),
            "events": act.events,
            "model": act.model.map(|m| m.replace("claude-", "")).or(s.model.clone()),
            "context": act.context,
            "tokens": tokens.map(|t| t.total()),
            "repo": git::repo(&s.cwd),
            "signals": signals,
            "twins": twins,
            "conflicts": conflicts,
            "typable": prompt::typable(s),
        }))
    })
    .await?
}

fn find_request(rid: &str) -> Res<claude::Request> {
    claude::pending().into_iter().find(|r| r.id == rid).ok_or_else(|| "That was already answered.".to_string())
}

fn decide(rid: String, allow: bool) -> Res<String> {
    let req = find_request(&rid)?;
    let d = if allow { Decision::Allow } else { Decision::Deny { message: None } };
    claude::answer(&req, &d).map_err(|e| e.to_string())?;
    Ok(format!("{} {}", if allow { "Allowed" } else { "Denied" }, claude::tool_summary(&req.tool, &req.input)))
}

fn answer(rid: String, answers: Map<String, Value>) -> Res<String> {
    let req = find_request(&rid)?;
    claude::answer(&req, &Decision::Answers { answers: answers.clone() }).map_err(|e| e.to_string())?;
    Ok(format!("Answered: {}", answers.values().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("; ")))
}

async fn send_prompt(key: String, text: String, focus: bool) -> Res<Value> {
    blocking(move || {
        let s = find(&key)?;
        prompt::send(&s, &text, focus).map(|d| json!(d))
    })
    .await?
}

async fn open_session(key: String) -> Res<String> {
    blocking(move || prompt::open(&find(&key)?)).await?
}

fn set_auto(sid: String, on: bool) -> Res<()> {
    if on && !install::installed() {
        return Err("Connect Claude Code first, in Agents.".into());
    }
    util::set_auto(&sid, on);
    Ok(())
}

fn set_auto_all(on: bool) -> Res<()> {
    if on && !install::installed() {
        return Err("Connect Claude Code first, in Agents.".into());
    }
    util::set_auto(util::ALL, on);
    Ok(())
}

async fn start_session(cwd: String, prompt: String, worktree: Option<String>, model: Option<String>) -> Res<String> {
    let cwd = util::expand(cwd.trim());
    outcome(blocking(move || launch::new(&cwd, prompt.trim(), worktree.as_deref(), model.as_deref().filter(|m| !m.is_empty()))).await?)
}

async fn resume_session(sid: String, cwd: Option<String>, prompt: Option<String>) -> Res<String> {
    outcome(blocking(move || launch::resume(&sid, cwd.as_deref(), prompt.as_deref())).await?)
}

async fn stop_session(short: String) -> Res<String> {
    outcome(blocking(move || launch::stop(&short)).await?)
}

async fn open_past(sid: String, cwd: Option<String>) -> Res<String> {
    outcome(blocking(move || launch::open_terminal(&format!("claude --resume {sid}"), cwd.as_deref())).await?)
}

// ---------- git ----------

async fn git_overview(cwd: String) -> Res<Value> {
    blocking(move || {
        let repo = git::repo(&cwd).ok_or("This folder isn't in a git repository.")?;
        let pr = if repo.github.is_some() { git::current_pr(&cwd) } else { None };
        Ok(json!({"repo": repo, "log": git::log(&cwd, 12), "branches": git::branches(&cwd), "pr": pr}))
    })
    .await?
}

async fn git_diff(cwd: String, path: Option<String>) -> Res<String> {
    blocking(move || {
        let d = git::diff(&cwd, path.as_deref());
        if d.len() > 400_000 {
            let cut = (0..=400_000).rev().find(|i| d.is_char_boundary(*i)).unwrap_or(0);
            format!("{}\n… (diff truncated)", &d[..cut])
        } else {
            d
        }
    })
    .await
}

async fn git_prs(cwd: String) -> Res<Vec<git::Pr>> {
    blocking(move || git::prs(&cwd)).await?
}

/// One git action. Merging and deleting are confirmed in the UI before this is called.
async fn git_action(cwd: String, action: String, arg: Option<String>, extra: Option<Value>) -> Res<String> {
    blocking(move || {
        let a = arg.unwrap_or_default();
        let get = |k: &str| extra.as_ref().and_then(|e| e.get(k)).cloned().unwrap_or(Value::Null);
        match action.as_str() {
            "commit" => git::commit_all(&cwd, &a),
            "push" => git::push(&cwd),
            "pull" => git::pull(&cwd),
            "branch" => git::create_branch(&cwd, &a),
            "switch" => git::switch(&cwd, &a),
            "delete-branch" => git::delete_branch(&cwd, &a),
            "pr" => git::create_pr(&cwd, &a, get("body").as_str().unwrap_or(""), get("draft").as_bool().unwrap_or(false)),
            "merge" => git::merge_pr(
                &cwd,
                a.parse().map_err(|_| "Which pull request?".to_string())?,
                get("method").as_str().unwrap_or("squash"),
                get("deleteBranch").as_bool().unwrap_or(false),
            ),
            _ => Err(format!("unknown git action {action}")),
        }
    })
    .await?
}

// ---------- goals & loops ----------

async fn goals_list() -> Res<Vec<goals::Goal>> {
    blocking(|| goals::list(&sessions::collect())).await
}

async fn goal_start(text: String, cwd: String, agent: String, model: Option<String>, terminal: bool) -> Res<goals::Goal> {
    blocking(move || goals::start(&text, &cwd, &agent, model.as_deref().filter(|m| !m.is_empty()), terminal)).await?
}

fn goal_remove(id: String) -> bool {
    goals::remove(&id)
}

fn loops_list() -> Vec<loops::Loop> {
    loops::list()
}

async fn loop_add(prompt: String, every: String, session: Option<String>, cwd: Option<String>, until: Option<String>, max_runs: Option<u32>) -> Res<loops::Loop> {
    blocking(move || {
        let secs = loops::parse_every(&every).ok_or("Use an interval like 15m, 2h or 1d (at least 30s).")?;
        let target = match session.filter(|s| !s.is_empty()) {
            Some(k) => Some(find(&k)?),
            None => None,
        };
        loops::add(&prompt, secs, target.as_ref(), cwd.as_deref().filter(|c| !c.is_empty()), until.as_deref(), max_runs)
    })
    .await?
}

fn loop_toggle(id: String, on: bool) -> bool {
    loops::set_enabled(&id, on)
}

fn loop_remove(id: String) -> bool {
    loops::remove(&id)
}

// ---------- history & usage ----------

#[derive(Serialize)]
struct PastView {
    #[serde(flatten)]
    p: Past,
    folder: String,
    ago: String,
}

async fn history() -> Res<Vec<PastView>> {
    blocking(|| {
        claude::history()
            .into_iter()
            .map(|p| PastView { folder: util::short(p.cwd.as_deref().unwrap_or("?")), ago: util::ago(now() - p.mtime), p })
            .collect()
    })
    .await
}

fn past_activity(path: String) -> Vec<claude::Event> {
    claude::activity(Path::new(&path)).events
}

async fn usage(days: Option<u32>) -> Res<Value> {
    let days = days.unwrap_or(7);
    blocking(move || {
        let u = claude::usage(days);
        let mut by_day = vec![];
        for (day, projects) in u.iter().rev().take(days as usize) {
            let mut t = Tokens::default();
            for p in projects.values() {
                t.input += p.input;
                t.out += p.out;
                t.cache += p.cache;
            }
            by_day.push(json!({"day": day, "in": t.input, "out": t.out, "cache": t.cache, "total": t.total()}));
        }
        let mut projects: Vec<Value> = u
            .get(&util::today())
            .map(|ps| ps.iter().map(|(p, t)| json!({"project": util::short(p), "total": t.total()})).collect())
            .unwrap_or_default();
        projects.sort_by_key(|v| std::cmp::Reverse(v["total"].as_u64().unwrap_or(0)));
        json!({"days": by_day, "today": projects})
    })
    .await
}

// ---------- rules, agents, settings ----------

fn rules_list() -> Vec<rules::Rule> {
    rules::load()
}
fn rules_add(rule: String) -> Res<rules::Rule> {
    rules::add(&rule)
}
fn rules_remove(n: usize) -> Res<String> {
    rules::remove(n)
}

async fn agents() -> Res<Vec<Value>> {
    blocking(|| {
        let cfg = config::load();
        install::MCP_TOOLS
            .iter()
            .map(|t| {
                let hookable = install::HOOK_TOOLS.contains(t);
                json!({
                    "tool": t, "label": providers::label(t), "detected": install::detected(t),
                    "hookable": hookable, "hook": hookable && install::hook_installed(t),
                    "note": install::hook_status(t).note,
                    "remote": *t == "claude" || cfg.remote.get(*t).copied().unwrap_or(false),
                    "mcp": install::mcp_installed(t), "mcpSnippet": install::mcp_snippet(t),
                })
            })
            .collect()
    })
    .await
}

async fn agent_hook(tool: String, on: bool) -> Res<String> {
    blocking(move || if on { install::install_hook(&tool).map(|r| r.to_string()) } else { install::uninstall_hook(&tool).map(|_| "removed".into()) }).await?
}

fn agent_remote(tool: String, on: bool) {
    config::update(|c| {
        c.remote.insert(tool, on);
    });
}

async fn agent_mcp(tool: String, on: bool) -> Res<String> {
    blocking(move || if on { install::install_mcp(&tool) } else { install::uninstall_mcp(&tool).map(|_| "removed".into()) }).await?
}

fn login_agent_path() -> std::path::PathBuf {
    util::home().join("Library/LaunchAgents/dev.grandaddy.desktop.plist")
}

fn bundled_cli() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok().map(|p| p.with_file_name("grandaddy")).filter(|p| p.exists())
}

async fn settings() -> Res<Value> {
    let awake_now = AWAKE.lock().unwrap().clone();
    blocking(move || {
        let cfg = config::load();
        let url = if cfg.web.enabled { web::url() } else { None };
        json!({
            "hookInstalled": install::installed(),
            "autoAll": util::auto_ids().contains(util::ALL),
            "phone": phone::config(),
            "web": {"enabled": cfg.web.enabled, "url": url, "qr": url.as_ref().map(|u| web::qr_svg(u))},
            "awake": awake_now,
            "lidMode": cfg.awake.lid_mode(),
            "keep": cfg.awake.keep,
            "autoContinue": {"enabled": cfg.auto_continue.enabled, "message": cfg.auto_continue.message},
            "mesh": cfg.experimental_mesh,
            "remoteTimeout": cfg.remote_timeout,
            "login": login_agent_path().exists(),
            "cli": {"bundled": bundled_cli().is_some(), "linked": util::home().join(".local/bin/grandaddy").symlink_metadata().is_ok()},
            "log": util::log_tail(60),
            "version": env!("CARGO_PKG_VERSION"),
        })
    })
    .await
}

/// enabled: stay awake while sessions work · lid: sleep | full | agents
async fn set_awake(enabled: bool, lid: String) -> Res<()> {
    blocking(move || {
        let lid = if ["full", "agents"].contains(&lid.as_str()) { lid } else { "sleep".to_string() };
        let need = match lid.as_str() {
            "full" => !awake::lid_helper_installed(),
            "agents" => !awake::agents_helper_installed(),
            _ => false,
        };
        if need {
            awake::install_lid_helper()?;
        }
        config::update(|c| {
            c.awake.enabled = enabled;
            c.awake.lid = lid.clone();
            c.awake.lid_closed = false;
        });
        if lid == "sleep" {
            awake::recover();
        }
        Ok(())
    })
    .await?
}

fn set_keep(apps: Vec<String>) {
    config::update(|c| c.awake.keep = apps.into_iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect());
}

fn set_auto_continue(enabled: bool, message: String) {
    config::update(|c| {
        c.auto_continue.enabled = enabled;
        if !message.trim().is_empty() {
            c.auto_continue.message = message.trim().to_string();
        }
    });
}

fn set_continue_skip(key: String, skip: bool) {
    limits::set_skip(&key, skip);
}

fn set_web(on: bool) {
    if on {
        web::ensure_token();
    }
    config::update(|c| c.web.enabled = on);
}

fn set_mesh(on: bool) {
    config::update(|c| c.experimental_mesh = on);
}

fn phone_setup(server: Option<String>) -> phone::Config {
    phone::setup(server.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("https://ntfy.sh"))
}
async fn phone_test() -> Res<String> {
    blocking(phone::test).await?
}
fn phone_off() {
    phone::off()
}

/// Start at login, hidden, so the menu bar icon and the background work are always there.
fn set_login(on: bool) -> Res<()> {
    let p = login_agent_path();
    if !on {
        let _ = std::fs::remove_file(&p);
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>dev.grandaddy.desktop</string>
  <key>ProgramArguments</key><array><string>{}</string><string>--background</string></array>
  <key>RunAtLoad</key><true/>
</dict></plist>
"#,
        exe.display()
    );
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&p, plist).map_err(|e| e.to_string())
}

/// Put the bundled `grandaddy` command on your PATH (~/.local/bin).
fn link_cli() -> Res<String> {
    let cli = bundled_cli().ok_or("This build doesn't include the command-line tool.")?;
    let dir = util::home().join(".local/bin");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let link = dir.join("grandaddy");
    if link.symlink_metadata().is_ok() {
        if std::fs::read_link(&link).map(|t| t == cli).unwrap_or(false) {
            return Ok("Already linked.".into());
        }
        return Err(format!("{} already exists. Remove it first if you want this one.", link.display()));
    }
    std::os::unix::fs::symlink(&cli, &link).map_err(|e| e.to_string())?;
    Ok(format!("Linked {}. Make sure ~/.local/bin is on your PATH.", link.display()))
}

#[tauri::command]
fn show_main(app: AppHandle, key: Option<String>) {
    if let Some(q) = app.get_webview_window("quick") {
        let _ = q.hide();
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        if let Some(k) = key {
            let _ = w.eval(format!("window.grandaddy && window.grandaddy.select({})", serde_json::to_string(&k).unwrap_or_default()));
        }
    }
}

#[tauri::command]
fn hide_quick(app: AppHandle) {
    if let Some(q) = app.get_webview_window("quick") {
        let _ = q.hide();
    }
}

/// Open a web page (pull requests) in the default browser. https only.
fn open_url(url: String) -> Res<()> {
    if !url.starts_with("https://") {
        return Err("Only https links open from here.".into());
    }
    std::process::Command::new("open").arg(&url).status().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn ui_log(level: String, msg: String) {
    if level == "error" || debug() {
        eprintln!("[ui {level}] {msg}");
    }
}

#[tauri::command]
fn debug() -> bool {
    std::env::var("GRANDADDY_DEBUG").as_deref() == Ok("1")
}

// ---------- one entry point ----------

/// Turn an argument from the page into the type a command takes. Missing keys read as null,
/// so `Option` arguments may be left out.
fn arg<T: serde::de::DeserializeOwned>(a: &Value, key: &str) -> Res<T> {
    serde_json::from_value(a.get(key).cloned().unwrap_or(Value::Null)).map_err(|e| format!("{key}: {e}"))
}

/// Commands return either a plain value or a Result; both come out as Res<Value>.
trait IntoRes {
    fn into_res(self) -> Res<Value>;
}
impl<T: Serialize> IntoRes for Res<T> {
    fn into_res(self) -> Res<Value> {
        serde_json::to_value(self?).map_err(|e| e.to_string())
    }
}
macro_rules! plain {
    ($($t:ty),*) => {$(
        impl IntoRes for $t {
            fn into_res(self) -> Res<Value> {
                serde_json::to_value(self).map_err(|e| e.to_string())
            }
        }
    )*};
}
plain!((), bool, Value, phone::Config, Vec<rules::Rule>, Vec<loops::Loop>, Vec<claude::Event>);
fn out(r: impl IntoRes) -> Res<Value> {
    r.into_res()
}

/// Every command the page can call, by name. The app calls it through `call`; the debug bridge
/// (`--ui-bridge`) calls it over HTTP, so both run exactly the same code.
pub async fn dispatch(cmd: &str, a: Value) -> Res<Value> {
    match cmd {
        "list_sessions" => out(list_sessions().await),
        "session_detail" => out(session_detail(arg::<String>(&a, "key")?).await),
        "decide" => out(decide(arg::<String>(&a, "rid")?, arg::<bool>(&a, "allow")?)),
        "answer" => out(answer(arg::<String>(&a, "rid")?, arg::<Map<String, Value>>(&a, "answers")?)),
        "send_prompt" => out(send_prompt(arg::<String>(&a, "key")?, arg::<String>(&a, "text")?, arg::<bool>(&a, "focus")?).await),
        "open_session" => out(open_session(arg::<String>(&a, "key")?).await),
        "set_auto" => out(set_auto(arg::<String>(&a, "sid")?, arg::<bool>(&a, "on")?)),
        "set_auto_all" => out(set_auto_all(arg::<bool>(&a, "on")?)),
        "start_session" => out(start_session(arg::<String>(&a, "cwd")?, arg::<String>(&a, "prompt")?, arg::<Option<String>>(&a, "worktree")?, arg::<Option<String>>(&a, "model")?).await),
        "resume_session" => out(resume_session(arg::<String>(&a, "sid")?, arg::<Option<String>>(&a, "cwd")?, arg::<Option<String>>(&a, "prompt")?).await),
        "stop_session" => out(stop_session(arg::<String>(&a, "short")?).await),
        "open_past" => out(open_past(arg::<String>(&a, "sid")?, arg::<Option<String>>(&a, "cwd")?).await),
        "git_overview" => out(git_overview(arg::<String>(&a, "cwd")?).await),
        "git_diff" => out(git_diff(arg::<String>(&a, "cwd")?, arg::<Option<String>>(&a, "path")?).await),
        "git_prs" => out(git_prs(arg::<String>(&a, "cwd")?).await),
        "git_action" => out(git_action(arg::<String>(&a, "cwd")?, arg::<String>(&a, "action")?, arg::<Option<String>>(&a, "arg")?, arg::<Option<Value>>(&a, "extra")?).await),
        "goals_list" => out(goals_list().await),
        "goal_start" => out(goal_start(arg::<String>(&a, "text")?, arg::<String>(&a, "cwd")?, arg::<String>(&a, "agent")?, arg::<Option<String>>(&a, "model")?, arg::<bool>(&a, "terminal")?).await),
        "goal_remove" => out(goal_remove(arg::<String>(&a, "id")?)),
        "loops_list" => out(loops_list()),
        "loop_add" => out(loop_add(arg::<String>(&a, "prompt")?, arg::<String>(&a, "every")?, arg::<Option<String>>(&a, "session")?, arg::<Option<String>>(&a, "cwd")?, arg::<Option<String>>(&a, "until")?, arg::<Option<u32>>(&a, "maxRuns")?).await),
        "loop_toggle" => out(loop_toggle(arg::<String>(&a, "id")?, arg::<bool>(&a, "on")?)),
        "loop_remove" => out(loop_remove(arg::<String>(&a, "id")?)),
        "history" => out(history().await),
        "past_activity" => out(past_activity(arg::<String>(&a, "path")?)),
        "usage" => out(usage(arg::<Option<u32>>(&a, "days")?).await),
        "rules_list" => out(rules_list()),
        "rules_add" => out(rules_add(arg::<String>(&a, "rule")?)),
        "rules_remove" => out(rules_remove(arg::<usize>(&a, "n")?)),
        "agents" => out(agents().await),
        "agent_hook" => out(agent_hook(arg::<String>(&a, "tool")?, arg::<bool>(&a, "on")?).await),
        "agent_remote" => {
            agent_remote(arg::<String>(&a, "tool")?, arg::<bool>(&a, "on")?);
            out(())
        }
        "agent_mcp" => out(agent_mcp(arg::<String>(&a, "tool")?, arg::<bool>(&a, "on")?).await),
        "settings" => out(settings().await),
        "set_awake" => out(set_awake(arg::<bool>(&a, "enabled")?, arg::<String>(&a, "lid")?).await),
        "set_keep" => {
            set_keep(arg::<Vec<String>>(&a, "apps")?);
            out(())
        }
        "set_auto_continue" => {
            set_auto_continue(arg::<bool>(&a, "enabled")?, arg::<String>(&a, "message")?);
            out(())
        }
        "set_continue_skip" => {
            set_continue_skip(arg::<String>(&a, "key")?, arg::<bool>(&a, "skip")?);
            out(())
        }
        "set_web" => {
            set_web(arg::<bool>(&a, "on")?);
            out(())
        }
        "set_mesh" => {
            set_mesh(arg::<bool>(&a, "on")?);
            out(())
        }
        "phone_setup" => out(phone_setup(arg::<Option<String>>(&a, "server")?)),
        "phone_test" => out(phone_test().await),
        "phone_off" => {
            phone_off();
            out(())
        }
        "set_login" => out(set_login(arg::<bool>(&a, "on")?)),
        "link_cli" => out(link_cli()),
        "open_url" => out(open_url(arg::<String>(&a, "url")?)),
        _ => Err(format!("unknown command {cmd}")),
    }
}

#[tauri::command]
async fn call(cmd: String, args: Value) -> Res<Value> {
    dispatch(&cmd, args).await
}

// ---------- background ----------

/// The power keeper's latest status, for Settings and the menu bar panel.
static AWAKE: Mutex<Option<awake::Status>> = Mutex::new(None);

/// Staying awake, loops, the phone page, notifications and the menu bar count.
fn runtime(app: AppHandle) {
    std::thread::spawn(move || {
        let lock = util::state().join("daemon.pid");
        let mut keeper: Option<awake::Keeper> = None;
        let stop = Arc::new(AtomicBool::new(false));
        let mut web_thread: Option<std::thread::JoinHandle<()>> = None;
        let mut prev: HashMap<String, (String, f64)> = HashMap::new();
        let mut first = true;
        let notify = std::env::var("GRANDADDY_NOTIFY").as_deref() != Ok("0");
        loop {
            let live = sessions::collect();

            // the background work runs in one place: here, unless a `grandaddy daemon` has it
            let other = std::fs::read_to_string(&lock).ok().and_then(|s| s.trim().parse::<i64>().ok()).filter(|p| *p != std::process::id() as i64 && util::alive(*p));
            if other.is_none() {
                let _ = std::fs::write(&lock, std::process::id().to_string());
                let keeper = keeper.get_or_insert_with(awake::Keeper::new);
                keeper.update(&live);
                *AWAKE.lock().unwrap() = Some(keeper.status());
                loops::tick(&live);
                limits::tick(&live);
                if config::load().web.enabled && web_thread.as_ref().map(|t| t.is_finished()).unwrap_or(true) {
                    let s = stop.clone();
                    web_thread = Some(std::thread::spawn(move || {
                        if let Err(e) = web::serve(s) {
                            eprintln!("phone page: {e}");
                        }
                    }));
                }
            }

            let waiting = live.iter().filter(|s| s.status == "needs you").count();
            if let Some(tray) = app.tray_by_id("main") {
                let _ = tray.set_title(if waiting > 0 { Some(waiting.to_string()) } else { None });
            }
            let t = now();
            for s in &live {
                if let (Some((p, since)), false, true) = (prev.get(&s.key), first, notify) {
                    if s.status == "needs you" && p != "needs you" {
                        let body = s.pending.first().map(claude::describe_request).unwrap_or_else(|| "is waiting for you".into());
                        launch::notify(&s.name, &body);
                    } else if s.status == "idle" && p == "working" && t - since >= 15.0 {
                        launch::notify(&s.name, &format!("finished after {}", util::ago(t - since)));
                    }
                }
                let since = match prev.get(&s.key) {
                    Some((p, since)) if p == "working" && s.status == "working" => *since,
                    _ => t,
                };
                prev.insert(s.key.clone(), (s.status.clone(), since));
            }
            first = false;
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    });
}

/// Show the quick panel under the menu bar icon, or hide it if it's open.
fn toggle_quick(app: &AppHandle, rect: Option<tauri::Rect>) {
    let Some(q) = app.get_webview_window("quick") else { return };
    if q.is_visible().unwrap_or(false) {
        let _ = q.hide();
        return;
    }
    if let Some(r) = rect {
        let scale = q.scale_factor().unwrap_or(2.0);
        let pos: PhysicalPosition<f64> = r.position.to_physical(scale);
        let size: tauri::PhysicalSize<f64> = r.size.to_physical(scale);
        let width = q.outer_size().map(|s| s.width as f64).unwrap_or(380.0 * scale);
        let x = (pos.x + size.width / 2.0 - width / 2.0).max(8.0 * scale);
        let y = pos.y + size.height + 4.0 * scale;
        let _ = q.set_position(PhysicalPosition::new(x, y));
    }
    let _ = q.show();
    let _ = q.set_focus();
    let _ = q.eval("window.quick && window.quick.refresh()");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let background = std::env::args().any(|a| a == "--background");
    let app = tauri::Builder::default()
        .setup(move |app| {
            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray@2x.png"))?;
            TrayIconBuilder::with_id("main")
                .icon(icon)
                .icon_as_template(true)
                .tooltip("Grandaddy")
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } = event {
                        toggle_quick(tray.app_handle(), Some(rect));
                    }
                })
                .build(app)?;
            if let Some(q) = app.get_webview_window("quick") {
                let qc = q.clone();
                q.on_window_event(move |e| {
                    if let WindowEvent::Focused(false) = e {
                        let _ = qc.hide();
                    }
                });
            }
            if background {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            runtime(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // closing the main window keeps Grandaddy running in the menu bar
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![call, show_main, hide_quick, quit, ui_log, debug])
        .build(tauri::generate_context!())
        .expect("error while building grandaddy");
    app.run(|app, event| {
        if let RunEvent::Reopen { .. } = event {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }
    });
}

/// Debug only: serve the page on 127.0.0.1 with every command answered by `dispatch`, so the
/// real UI can be driven against the real backend from a browser (`grandaddy-app --ui-bridge
/// 7400 path/to/ui`). Window commands do nothing here.
pub fn serve_bridge(port: u16, ui_dir: &str) -> Result<(), String> {
    use tiny_http::{Header, Method, Response, Server};
    let server = Server::http(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    eprintln!("ui bridge on http://127.0.0.1:{port}/");
    const SHIM: &str = r#"<script>window.__TAURI__={core:{invoke:async(c,a={})=>{const [cmd,args]=c==="call"?[a.cmd,a.args]:[c,a];const r=await fetch("/call",{method:"POST",body:JSON.stringify({cmd,args})});const j=await r.json();if(!r.ok)throw j.error;return j.value}}};</script>"#;
    for mut req in server.incoming_requests() {
        let url = req.url().split('?').next().unwrap_or("/").to_string();
        if *req.method() == Method::Post && url == "/call" {
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(req.as_reader(), &mut body);
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let cmd = v.get("cmd").and_then(|c| c.as_str()).unwrap_or("").to_string();
            let args = v.get("args").cloned().unwrap_or(json!({}));
            let res = if ["show_main", "hide_quick", "quit", "ui_log", "debug"].contains(&cmd.as_str()) { Ok(Value::Null) } else { tauri::async_runtime::block_on(dispatch(&cmd, args)) };
            let (code, body) = match res {
                Ok(v) => (200, json!({ "value": v })),
                Err(e) => (400, json!({ "error": e })),
            };
            let _ = req.respond(Response::from_string(body.to_string()).with_status_code(code).with_header(Header::from_bytes("Content-Type", "application/json").unwrap()));
            continue;
        }
        let rel = if url == "/" { "index.html".to_string() } else { url.trim_start_matches('/').to_string() };
        if rel.contains("..") {
            let _ = req.respond(Response::from_string("no").with_status_code(403));
            continue;
        }
        let path = std::path::Path::new(ui_dir).join(&rel);
        match std::fs::read(&path) {
            Ok(mut bytes) => {
                let ct = match path.extension().and_then(|e| e.to_str()) {
                    Some("html") => "text/html; charset=utf-8",
                    Some("css") => "text/css",
                    Some("js") => "text/javascript",
                    Some("png") => "image/png",
                    Some("svg") => "image/svg+xml",
                    _ => "application/octet-stream",
                };
                if ct.starts_with("text/html") {
                    bytes = String::from_utf8_lossy(&bytes).replacen("<head>", &format!("<head>{SHIM}"), 1).into_bytes();
                }
                let _ = req.respond(Response::from_data(bytes).with_header(Header::from_bytes("Content-Type", ct).unwrap()));
            }
            Err(_) => {
                let _ = req.respond(Response::from_string("not found").with_status_code(404));
            }
        }
    }
    Ok(())
}
