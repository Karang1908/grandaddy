//! Grandaddy desktop app: a thin Tauri layer over grandaddy-core (the same code the CLI runs).
use grandaddy_core::claude::{self, Decision, Past, Tokens};
use grandaddy_core::sessions::{self, Session};
use grandaddy_core::util::{self, now};
use grandaddy_core::{install, launch, phone, rules};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

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

#[tauri::command]
fn list_sessions() -> Vec<SessionView> {
    sessions::collect().into_iter().map(view).collect()
}

#[tauri::command]
fn session_detail(key: String) -> Res<Value> {
    let all = sessions::collect();
    let s = all.iter().find(|s| s.key == key).ok_or("that session has ended")?;
    let act = sessions::activity(s);
    let signals = claude::signals(&act, &s.status);
    let tokens = s.transcript.as_ref().filter(|_| s.tool == "claude").and_then(|p| claude::session_tokens(p));
    let twins: Vec<&str> = all.iter().filter(|o| o.key != s.key && !o.cwd.is_empty() && o.cwd == s.cwd).map(|o| o.name.as_str()).collect();
    let conflicts = if s.tool == "claude" { sessions::conflicts(s, &all) } else { BTreeMap::new() };
    Ok(json!({
        "session": view(s.clone()),
        "events": act.events,
        "model": act.model.map(|m| m.replace("claude-", "")),
        "context": act.context,
        "tokens": tokens.map(|t| t.total()),
        "git": sessions::git(&s.cwd),
        "signals": signals,
        "twins": twins,
        "conflicts": conflicts,
    }))
}

fn find_request(rid: &str) -> Res<claude::Request> {
    claude::pending().into_iter().find(|r| r.id == rid).ok_or_else(|| "that request was already answered".to_string())
}

#[tauri::command]
fn decide(rid: String, allow: bool) -> Res<String> {
    let req = find_request(&rid)?;
    let d = if allow { Decision::Allow } else { Decision::Deny { message: None } };
    claude::answer(&req, &d).map_err(|e| e.to_string())?;
    Ok(format!("{} {}", if allow { "Allowed" } else { "Denied" }, claude::tool_summary(&req.tool, &req.input)))
}

#[tauri::command]
fn answer(rid: String, answers: Map<String, Value>) -> Res<String> {
    let req = find_request(&rid)?;
    claude::answer(&req, &Decision::Answers { answers: answers.clone() }).map_err(|e| e.to_string())?;
    Ok(format!("Answered: {}", answers.values().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("; ")))
}

#[tauri::command]
fn set_auto(sid: String, on: bool) -> Res<()> {
    if on && !install::installed() {
        return Err("Install the hook first (Settings → Claude Code hook)".into());
    }
    util::set_auto(&sid, on);
    Ok(())
}

#[tauri::command]
fn set_auto_all(on: bool) -> Res<()> {
    if on && !install::installed() {
        return Err("Install the hook first (Settings → Claude Code hook)".into());
    }
    util::set_auto(util::ALL, on);
    Ok(())
}

fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> impl std::future::Future<Output = Res<T>> {
    async move { tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string()) }
}

fn outcome((ok, out): (bool, String)) -> Res<String> {
    let last = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(if ok { "Done" } else { "Failed" }).to_string();
    if ok { Ok(last) } else { Err(last) }
}

#[tauri::command]
async fn start_session(cwd: String, prompt: String, worktree: Option<String>, model: Option<String>) -> Res<String> {
    let cwd = util::expand(cwd.trim());
    outcome(blocking(move || launch::new(&cwd, prompt.trim(), worktree.as_deref(), model.as_deref())).await?)
}

#[tauri::command]
async fn resume_session(sid: String, cwd: Option<String>, prompt: Option<String>) -> Res<String> {
    outcome(blocking(move || launch::resume(&sid, cwd.as_deref(), prompt.as_deref())).await?)
}

#[tauri::command]
async fn stop_session(short: String) -> Res<String> {
    outcome(blocking(move || launch::stop(&short)).await?)
}

#[tauri::command]
async fn open_session(key: String) -> Res<String> {
    let s = sessions::collect().into_iter().find(|s| s.key == key).ok_or("that session has ended")?;
    outcome(blocking(move || match (s.bg, s.short.clone(), s.pid) {
        (true, Some(id), _) => launch::open_terminal(&format!("claude attach {id}"), Some(&s.cwd)),
        (_, _, Some(pid)) => launch::jump(pid),
        _ => (false, "Nothing to open".into()),
    })
    .await?)
}

#[tauri::command]
async fn open_past(sid: String, cwd: Option<String>) -> Res<String> {
    outcome(blocking(move || launch::open_terminal(&format!("claude --resume {sid}"), cwd.as_deref())).await?)
}

#[derive(Serialize)]
struct PastView {
    #[serde(flatten)]
    p: Past,
    folder: String,
    ago: String,
}

#[tauri::command]
async fn history() -> Res<Vec<PastView>> {
    blocking(|| {
        claude::history()
            .into_iter()
            .map(|p| PastView { folder: util::short(p.cwd.as_deref().unwrap_or("?")), ago: util::ago(now() - p.mtime), p })
            .collect()
    })
    .await
}

#[tauri::command]
fn past_activity(path: String) -> Vec<claude::Event> {
    claude::activity(Path::new(&path)).events
}

#[tauri::command]
async fn usage(days: Option<u32>) -> Res<Value> {
    blocking(move || {
        let u = claude::usage(days.unwrap_or(7));
        let mut by_day = vec![];
        for (day, projects) in u.iter().rev().take(days.unwrap_or(7) as usize) {
            let mut t = Tokens::default();
            for p in projects.values() {
                t.input += p.input;
                t.out += p.out;
                t.cache += p.cache;
            }
            by_day.push(json!({"day": day, "in": t.input, "out": t.out, "cache": t.cache, "total": t.total()}));
        }
        let today = util::today();
        let mut projects: Vec<Value> = u
            .get(&today)
            .map(|ps| ps.iter().map(|(p, t)| json!({"project": util::short(p), "total": t.total()})).collect())
            .unwrap_or_default();
        projects.sort_by_key(|v| std::cmp::Reverse(v["total"].as_u64().unwrap_or(0)));
        json!({"days": by_day, "today": projects})
    })
    .await
}

#[tauri::command]
fn rules_list() -> Vec<rules::Rule> {
    rules::load()
}
#[tauri::command]
fn rules_add(rule: String) -> Res<rules::Rule> {
    rules::add(&rule)
}
#[tauri::command]
fn rules_remove(n: usize) -> Res<String> {
    rules::remove(n)
}

#[tauri::command]
fn settings() -> Value {
    json!({
        "hookInstalled": install::installed(),
        "hookCommand": install::hook_command(),
        "autoAll": util::auto_ids().contains(util::ALL),
        "phone": phone::config(),
        "log": util::log_tail(40),
    })
}

#[tauri::command]
fn install_hook() -> Res<String> {
    install::install().map(|r| r.to_string()).map_err(|e| e.to_string())
}

#[tauri::command]
fn phone_setup(server: Option<String>) -> phone::Config {
    phone::setup(server.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("https://ntfy.sh"))
}
#[tauri::command]
async fn phone_test() -> Res<String> {
    blocking(phone::test).await?
}
#[tauri::command]
fn phone_off() {
    phone::off()
}

/// Page errors (and, with GRANDADDY_DEBUG=1, a startup self-check) land in the app's stderr.
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

/// Desktop notifications from the backend, so they arrive with the window closed too.
fn watch_for_notifications() {
    std::thread::spawn(|| {
        let mut prev: HashMap<String, (String, f64)> = HashMap::new();
        let mut first = true;
        loop {
            let t = now();
            for s in sessions::collect() {
                if let (Some((pst, since)), false) = (prev.get(&s.key), first) {
                    if s.status == "needs you" && pst != "needs you" {
                        let body = s.pending.first().map(claude::describe_request).unwrap_or_else(|| "is waiting for you".into());
                        launch::notify(&s.name, &body);
                    } else if s.status == "idle" && pst == "working" && t - since >= 15.0 {
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
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|_app| {
            if std::env::var("GRANDADDY_NOTIFY").as_deref() != Ok("0") {
                watch_for_notifications();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions, session_detail, decide, answer, set_auto, set_auto_all, start_session, resume_session,
            stop_session, open_session, open_past, history, past_activity, usage, rules_list, rules_add, rules_remove,
            settings, install_hook, phone_setup, phone_test, phone_off, ui_log, debug
        ])
        .run(tauri::generate_context!())
        .expect("error while running grandaddy");
}
