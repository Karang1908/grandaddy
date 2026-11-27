use grandaddy_core::claude::{self, Decision, Request};
use grandaddy_core::sessions::{self, Session};
use grandaddy_core::util::{self, now};
use grandaddy_core::{awake, config, git, goals, hook, install, launch, limits, loops, managed, mcp, phone, prompt, providers, rules, web};
use serde_json::{Map, Value};
use std::io::{IsTerminal, Read};

pub const USAGE: &str = "grandaddy — every coding agent on your Mac, in one place.

See and answer
  grandaddy                       the hub (? shows the keys)
  grandaddy ls                    sessions across Claude Code, Codex, Antigravity, Cursor, Gemini, OpenCode, Kilo…
  grandaddy watch [session]       follow a session as it works
  grandaddy pending               what's waiting on you
  grandaddy approve | deny [session]
  grandaddy answer [session] 2 1,3 \"my own words\"     one answer per question

Talk to sessions
  grandaddy run <agent> [args…]   start any agent so grandaddy can type into it later
  grandaddy send [session] <prompt>                 add --open to bring its window forward
  grandaddy open [session]        bring its window forward (or attach to a background session)
  grandaddy start <prompt> [--in DIR] [--worktree [NAME]] [--model M]   a background Claude session
  grandaddy stop [session]        stop a background session

Goals and loops
  grandaddy goal <what you want> [--in DIR] [--agent A] [--model M] [--terminal]
  grandaddy goal ls | rm <id>
  grandaddy loop add <prompt> --every 30m [--session S | --in DIR] [--until TEXT] [--max N]
  grandaddy loop ls | pause <id> | resume <id> | rm <id>

Git
  grandaddy git [session]         branch, changes and pull request for a session's folder
  grandaddy git log | branches | prs [session]
  grandaddy git commit <message> | push | pull | branch <name> | switch <name> [--in DIR]
  grandaddy git pr <title> [--draft] | merge <number> [--squash|--merge|--rebase] [--delete-branch]

Approvals
  grandaddy auto on|off [session|--all]             approve every permission prompt in a session
  grandaddy rules [add <rule> | rm <n>]             yes Bash(npm test*) · ask Bash(git push*) · no Bash(rm -rf*)
  grandaddy remote on|off <agent>                   hold another agent's prompts for an answer from here
  grandaddy phone setup [server] | test | off       answer on your phone with ntfy
  grandaddy web on|off|url        a page for your phone's browser on this Wi-Fi

Agents and the Mac
  grandaddy tools                 which agents are connected
  grandaddy install [agent|all]   connect the approval hook (default: claude)
  grandaddy uninstall [agent]
  grandaddy awake on|off | lid off|full|agents     stay awake while sessions work; what a closed lid does
  grandaddy awake preview         which apps agents-only mode would pause right now
  grandaddy continue on|off [\"message\"]           continue sessions after their usage limit resets
  grandaddy mesh on|off           experimental: let sessions talk to each other (S2S / A2A)
  grandaddy mcp install <agent|all> | mcp           the MCP server sessions use for that
  grandaddy daemon                loops, auto-continue, staying awake and the phone page, without the app
  grandaddy history [search] · resume <id> [prompt] · usage [days] · log [n]

A session is a pid, name, background id, session-id prefix or folder. Leave it out for the
session in this folder.";

struct Fail(String);
type R = Result<(), Fail>;
fn fail<T>(msg: impl Into<String>) -> Result<T, Fail> {
    Err(Fail(msg.into()))
}
fn io(e: impl std::fmt::Display) -> Fail {
    Fail(e.to_string())
}

pub fn main(args: Vec<String>) -> i32 {
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r.to_vec()),
        None => {
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                return match crate::hub::run() {
                    Ok(()) => 0,
                    Err(e) => {
                        eprintln!("grandaddy: {e}");
                        1
                    }
                };
            }
            ("ls", vec![])
        }
    };
    let result = match cmd {
        "hook" => {
            let mut input = String::new();
            let _ = std::io::stdin().read_to_string(&mut input);
            let agent = rest.iter().position(|a| a == "--tool").and_then(|i| rest.get(i + 1)).map(String::as_str);
            if let Some(out) = hook::run(&input, agent) {
                println!("{out}");
            }
            Ok(())
        }
        "mcp" if rest.is_empty() => {
            mcp::serve();
            Ok(())
        }
        "mcp" => cmd_mcp(&rest),
        "run" => {
            return match managed::run(&rest) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
        "ls" | "list" => cmd_ls(),
        "watch" => cmd_watch(&rest),
        "pending" => cmd_pending(),
        "approve" => cmd_decide(&rest, true),
        "deny" => cmd_decide(&rest, false),
        "answer" => cmd_answer(&rest),
        "send" => cmd_send(&rest),
        "open" => cmd_open(&rest),
        "auto" => cmd_auto(&rest),
        "attach" => cmd_auto(&[vec!["on".to_string()], rest].concat()),
        "detach" => cmd_auto(&[vec!["off".to_string()], rest].concat()),
        "rules" => cmd_rules(&rest),
        "start" => cmd_start(&rest),
        "stop" => cmd_stop(&rest),
        "goal" | "goals" => cmd_goal(&rest),
        "loop" | "loops" => cmd_loop(&rest),
        "git" => cmd_git(&rest),
        "history" => cmd_history(&rest),
        "resume" => cmd_resume(&rest),
        "usage" => cmd_usage(&rest),
        "phone" => cmd_phone(&rest),
        "web" => cmd_web(&rest),
        "remote" => cmd_remote(&rest),
        "awake" => cmd_awake(&rest),
        "mesh" => cmd_mesh(&rest),
        "continue" => cmd_continue(&rest),
        "tools" => cmd_tools(),
        "daemon" => cmd_daemon(),
        "log" => {
            let n = rest.first().and_then(|s| s.parse().ok()).unwrap_or(20);
            let lines = util::log_tail(n);
            println!("{}", if lines.is_empty() { "Nothing logged yet.".into() } else { lines.join("\n") });
            Ok(())
        }
        "install" => cmd_install(&rest),
        "uninstall" => cmd_uninstall(&rest),
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        "version" | "-V" | "--version" => {
            println!("grandaddy {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => {
            println!("{USAGE}");
            return 1;
        }
    };
    match result {
        Ok(()) => 0,
        Err(Fail(m)) => {
            if !m.is_empty() {
                eprintln!("{m}");
            }
            1
        }
    }
}

fn ident(s: &Session) -> String {
    if s.bg {
        format!("bg {}", s.short.clone().unwrap_or_default())
    } else if let Some(p) = s.pid {
        format!("pid {p}")
    } else {
        s.sid.as_deref().map(|x| x.chars().take(8).collect()).unwrap_or_default()
    }
}

fn row(s: &Session) -> String {
    let mut flags = String::new();
    if s.auto {
        flags.push_str("  auto-yes");
    }
    if s.managed.is_some() {
        flags.push_str("  takes prompts");
    }
    let name: String = s.name.chars().take(34).collect();
    let agent = if s.surface == "CLI" { s.tool_label.clone() } else { format!("{} {}", s.tool_label, s.surface) };
    format!("  {:<34} {:<9} {:>4}  {:<16} {:<11} {}{}", name, s.status, util::ago(now() - s.since), agent, ident(s), util::short(&s.cwd), flags)
}

fn pick(args: &[String], claude_only: bool, need_pending: bool) -> Result<Session, Fail> {
    let sess = sessions::collect();
    let target = args.first().map(String::as_str);
    let m = sessions::resolve(target, &sess, claude_only);
    if need_pending && target.is_none() && m.len() != 1 {
        let waiting: Vec<&Session> = sess.iter().filter(|s| !s.pending.is_empty()).collect();
        if waiting.len() == 1 {
            return Ok(waiting[0].clone()); // only one thing is waiting on you anyway
        }
    }
    if m.len() == 1 {
        return Ok(m[0].clone());
    }
    let here = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let place = target.map(|t| format!("'{t}'")).unwrap_or_else(|| format!("this folder ({here})"));
    eprintln!("{}{}:", if m.is_empty() { "No session matches ".to_string() } else { format!("{} sessions match ", m.len()) }, place);
    let shown: Vec<&Session> = if m.is_empty() { sess.iter().filter(|s| !claude_only || s.tool == "claude").collect() } else { m };
    for s in shown {
        eprintln!("{}", row(s));
    }
    fail("Pass a pid, name or session-id prefix to choose.")
}

fn cmd_ls() -> R {
    let sess = sessions::collect();
    if !install::installed() {
        println!("The Claude Code hook isn't connected yet — run: grandaddy install");
    }
    if util::auto_ids().contains(util::ALL) {
        println!("Auto-yes is on for every session.");
    }
    if sess.is_empty() {
        println!("No sessions running.");
    }
    for s in &sess {
        println!("{}", row(s));
        for r in &s.pending {
            println!("      ↳ {}", claude::describe_request(r));
        }
    }
    Ok(())
}

fn cmd_pending() -> R {
    let sess: Vec<Session> = sessions::collect().into_iter().filter(|s| !s.pending.is_empty()).collect();
    if sess.is_empty() {
        println!("Nothing is waiting on you.");
    }
    for s in &sess {
        for r in &s.pending {
            println!("{} ({}, {}) {}", s.name, s.tool_label, ident(s), claude::describe_request(r));
            if r.is_question() {
                for (qi, q) in r.questions().iter().enumerate() {
                    let multi = if q.get("multiSelect").and_then(|v| v.as_bool()) == Some(true) { "  (pick several)" } else { "" };
                    println!("   Q{}. {}{}", qi + 1, q.get("question").and_then(|v| v.as_str()).unwrap_or(""), multi);
                    for (i, o) in q.get("options").and_then(|v| v.as_array()).cloned().unwrap_or_default().iter().enumerate() {
                        println!("       {}  {}", i + 1, o.get("label").and_then(|v| v.as_str()).unwrap_or(""));
                    }
                }
            } else {
                println!("   grandaddy approve / deny");
            }
        }
    }
    Ok(())
}

fn first_pending(s: &Session) -> Result<Request, Fail> {
    s.pending.first().cloned().ok_or_else(|| Fail(format!("{} isn't waiting on anything grandaddy can answer.", s.name)))
}

fn cmd_decide(args: &[String], allow: bool) -> R {
    let s = pick(args, false, true)?;
    let req = first_pending(&s)?;
    if req.is_question() {
        return fail(format!("{} asked a question — answer it with: grandaddy answer <choice>", s.name));
    }
    let d = if allow { Decision::Allow } else { Decision::Deny { message: None } };
    claude::answer(&req, &d).map_err(io)?;
    println!("{}{}", if allow { "Allowed: " } else { "Denied: " }, claude::tool_summary(&req.tool, &req.input));
    Ok(())
}

/// One answer per question: option numbers ("2", or "1,3" on a multi-select) or your own words.
pub fn build_answers(questions: &[Value], choices: &[String]) -> Result<Map<String, Value>, String> {
    if choices.len() != questions.len() {
        return Err(format!("This asks {} question(s); give one answer each.", questions.len()));
    }
    let mut out = Map::new();
    for (q, c) in questions.iter().zip(choices) {
        let opts = q.get("options").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let multi = q.get("multiSelect").and_then(|v| v.as_bool()) == Some(true);
        let picks: Vec<&str> = if multi { c.split(',').collect() } else { vec![c.as_str()] };
        let nums: Option<Vec<usize>> = picks.iter().map(|p| p.trim().parse::<usize>().ok().filter(|n| (1..=opts.len()).contains(n))).collect();
        let text = match nums {
            Some(ns) => ns.iter().map(|n| opts[n - 1].get("label").and_then(|v| v.as_str()).unwrap_or("").to_string()).collect::<Vec<_>>().join(", "),
            None => c.clone(),
        };
        out.insert(q.get("question").and_then(|v| v.as_str()).unwrap_or("").to_string(), Value::String(text));
    }
    Ok(out)
}

fn cmd_answer(args: &[String]) -> R {
    // the session is optional, so anything that isn't a live session is treated as an answer
    let sess = sessions::collect();
    let target = (args.len() > 1 && sessions::resolve(Some(&args[0]), &sess, true).len() == 1).then(|| args[0].clone());
    let s = pick(&target.clone().into_iter().collect::<Vec<_>>(), true, true)?;
    let choices = if target.is_some() { &args[1..] } else { args };
    let req = first_pending(&s)?;
    if !req.is_question() {
        return fail(format!("{} is asking for permission, not an answer — use: grandaddy approve / deny", s.name));
    }
    if choices.is_empty() {
        return fail("Give an answer, e.g.  grandaddy answer 2");
    }
    let answers = build_answers(&req.questions(), choices).map_err(Fail)?;
    claude::answer(&req, &Decision::Answers { answers: answers.clone() }).map_err(io)?;
    let shown: Vec<String> = answers.iter().map(|(q, a)| format!("{q} → {}", a.as_str().unwrap_or(""))).collect();
    println!("Answered: {}", shown.join("; "));
    Ok(())
}

/// Pull `--name [value]` out of args. Some("") = flag without a value.
fn opt(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.remove(i);
    if i < args.len() && !args[i].starts_with("--") {
        Some(args.remove(i))
    } else {
        Some(String::new())
    }
}

fn cmd_send(args: &[String]) -> R {
    let mut a = args.to_vec();
    let focus = opt(&mut a, "--open").is_some();
    let sess = sessions::collect();
    let target = (a.len() > 1 && sessions::resolve(Some(&a[0]), &sess, false).len() == 1).then(|| a[0].clone());
    let s = pick(&target.clone().into_iter().collect::<Vec<_>>(), false, false)?;
    let text = if target.is_some() { a[1..].join(" ") } else { a.join(" ") };
    let d = prompt::send(&s, &text, focus).map_err(Fail)?;
    println!("{}", d.message);
    Ok(())
}

pub fn fmt_event(e: &claude::Event) -> String {
    match e.k.as_str() {
        "you" => format!("\n{} › {}", util::clock(e.ts), e.text),
        "text" => format!("      {}", e.text.replace('\n', "\n      ")),
        "tool" => format!("      ▸ {:<10} {}", e.tool.as_deref().unwrap_or("?"), e.desc.as_deref().unwrap_or(&e.text)),
        "error" => format!("      ✗ {}", e.text),
        _ => format!("      ── {} ──", e.text),
    }
}

fn cmd_watch(args: &[String]) -> R {
    let s = pick(args, false, false)?;
    println!("Watching {} ({}, {}) — ctrl-c to stop", s.name, s.tool_label, ident(&s));
    let mut seen = 0usize;
    loop {
        let events = sessions::activity(&s).events;
        if seen == 0 || events.len() < seen {
            seen = events.len().saturating_sub(15); // recent context; resync if the tail moved
        }
        for e in &events[seen..] {
            println!("{}", fmt_event(e));
        }
        seen = events.len();
        if let Some(pid) = s.pid {
            if !util::alive(pid) {
                println!("— the session ended");
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn cmd_auto(args: &[String]) -> R {
    let on = match args.first().map(String::as_str) {
        Some("on") => true,
        Some("off") => false,
        _ => return fail("usage: grandaddy auto on|off [session|--all]"),
    };
    if on && !install::installed() {
        return fail("Connect the hook first: grandaddy install");
    }
    let rest = &args[1..];
    if rest.first().map(String::as_str) == Some("--all") {
        if on {
            util::set_auto(util::ALL, true);
            println!("Auto-yes is on for every session — every permission prompt will be approved.");
        } else {
            let _ = std::fs::remove_dir_all(util::auto_dir());
            println!("Auto-yes is off everywhere.");
        }
        return Ok(());
    }
    let s = pick(rest, false, false)?;
    let Some(sid) = s.sid.clone() else { return fail(format!("{} can't use auto-yes — grandaddy has no hook into it.", s.name)) };
    util::set_auto(&sid, on);
    println!("Auto-yes {}: {} ({})", if on { "on" } else { "off" }, s.name, ident(&s));
    if !on && util::auto_ids().contains(util::ALL) {
        println!("Note: it's still on for every session — grandaddy auto off --all");
    }
    Ok(())
}

fn cmd_rules(args: &[String]) -> R {
    match args.first().map(String::as_str) {
        Some("add") if args.len() > 1 => {
            let r = rules::add(&args[1..].join(" ")).map_err(Fail)?;
            println!("added: {}", r.text);
            if !install::installed() {
                println!("Rules apply once the hook is connected: grandaddy install");
            }
            Ok(())
        }
        Some("rm") if args.len() == 2 => {
            let n: usize = args[1].parse().map_err(|_| Fail("usage: grandaddy rules rm <n>".into()))?;
            println!("removed: {}", rules::remove(n).map_err(Fail)?);
            Ok(())
        }
        Some(_) => fail("usage: grandaddy rules [add <rule> | rm <n>]"),
        None => {
            let rl = rules::load();
            if rl.is_empty() {
                println!("no rules yet. Examples:");
                println!("  grandaddy rules add yes \"Bash(npm test*)\"     approve automatically, every session");
                println!("  grandaddy rules add ask \"Bash(git push*)\"     always ask, even with auto-yes on");
                println!("  grandaddy rules add no  \"Bash(rm -rf /*)\"     refuse automatically");
            }
            for (i, r) in rl.iter().enumerate() {
                println!("  {}. {}", i + 1, r.text);
            }
            println!("\nRules cover every connected agent; the first match wins. ({})", rules::path().display());
            Ok(())
        }
    }
}

fn cmd_start(args: &[String]) -> R {
    let mut a = args.to_vec();
    let folder = opt(&mut a, "--in").filter(|s| !s.is_empty());
    let worktree = opt(&mut a, "--worktree");
    let model = opt(&mut a, "--model");
    let prompt = a.join(" ");
    if prompt.trim().is_empty() {
        return fail("usage: grandaddy start <prompt> [--in DIR] [--worktree [NAME]] [--model M]");
    }
    let dir = util::expand(&folder.unwrap_or_else(|| std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()));
    let (ok, out) = launch::new(&dir, prompt.trim(), worktree.as_deref(), model.as_deref());
    println!("{out}");
    if ok { Ok(()) } else { fail("") }
}

fn cmd_open(args: &[String]) -> R {
    let s = pick(args, false, false)?;
    println!("{}", prompt::open(&s).map_err(Fail)?);
    Ok(())
}

fn cmd_stop(args: &[String]) -> R {
    let s = pick(args, true, false)?;
    let Some(id) = s.short.as_ref().filter(|_| s.bg) else {
        return fail(format!("{} isn't a background session — stop it in its own window.", s.name));
    };
    let (ok, out) = launch::stop(id);
    println!("{}", if out.is_empty() { if ok { "Stopped.".into() } else { "Failed.".into() } } else { out });
    if ok { Ok(()) } else { fail("") }
}

fn cmd_goal(args: &[String]) -> R {
    match args.first().map(String::as_str) {
        None | Some("ls") => {
            let gl = goals::list(&sessions::collect());
            if gl.is_empty() {
                println!("No goals yet. Start one:  grandaddy goal \"add dark mode to the settings page\"");
            }
            for g in gl {
                println!("  {}  {:<10} {:<12} {}", g.id, g.status, providers::label(&g.tool), util::one_line(&g.text, 70));
                if let Some(s) = g.summary {
                    println!("        {}", util::one_line(&s, 120));
                }
            }
            Ok(())
        }
        Some("rm") if args.len() == 2 => {
            if goals::remove(&args[1]) { println!("Removed."); Ok(()) } else { fail("No goal with that id.") }
        }
        _ => {
            let mut a = args.to_vec();
            let folder = opt(&mut a, "--in").filter(|s| !s.is_empty()).unwrap_or_else(|| std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default());
            let agent = opt(&mut a, "--agent").filter(|s| !s.is_empty()).unwrap_or_else(|| "claude".into());
            let model = opt(&mut a, "--model").filter(|s| !s.is_empty());
            let terminal = opt(&mut a, "--terminal").is_some();
            let g = goals::start(&a.join(" "), &folder, &agent, model.as_deref(), terminal).map_err(Fail)?;
            let where_ = if g.runner == "background" { "in the background — its questions will show up in grandaddy and on your phone" } else { "in a new terminal window" };
            println!("Goal {} started {where_}.", g.id);
            Ok(())
        }
    }
}

fn cmd_loop(args: &[String]) -> R {
    match args.first().map(String::as_str) {
        None | Some("ls") => {
            let ls = loops::list();
            if ls.is_empty() {
                println!("No loops. Add one:  grandaddy loop add \"check CI and fix what's red\" --every 30m");
            }
            for l in ls {
                let target = l.session.clone().unwrap_or_else(|| format!("new session in {}", util::short(l.cwd.as_deref().unwrap_or("?"))));
                let state = if l.enabled { format!("next in {}", util::ago(l.next_run - now())) } else { "paused".into() };
                println!("  {}  {:<10} {:<14} {}  → {}", l.id, loops::every_label(l.every), state, util::one_line(&l.prompt, 50), target);
                if let Some(r) = l.last_result {
                    println!("        last: {r}");
                }
            }
            Ok(())
        }
        Some("rm") if args.len() == 2 => if loops::remove(&args[1]) { println!("Removed."); Ok(()) } else { fail("No loop with that id.") },
        Some(v @ ("pause" | "resume")) if args.len() == 2 => {
            if loops::set_enabled(&args[1], v == "resume") { println!("{}.", if v == "resume" { "Resumed" } else { "Paused" }); Ok(()) } else { fail("No loop with that id.") }
        }
        Some("add") => {
            let mut a = args[1..].to_vec();
            let every = opt(&mut a, "--every").and_then(|e| loops::parse_every(&e)).ok_or_else(|| Fail("Give an interval of at least 30s, e.g. --every 30m".into()))?;
            let session = opt(&mut a, "--session").filter(|s| !s.is_empty());
            let folder = opt(&mut a, "--in").filter(|s| !s.is_empty());
            let until = opt(&mut a, "--until").filter(|s| !s.is_empty());
            let max = opt(&mut a, "--max").and_then(|m| m.parse().ok());
            let target = match &session {
                Some(t) => Some(pick(std::slice::from_ref(t), false, false)?),
                None => None,
            };
            let l = loops::add(&a.join(" "), every, target.as_ref(), folder.as_deref(), until.as_deref(), max).map_err(Fail)?;
            println!("Loop {} added, {}. It runs while the Grandaddy app or `grandaddy daemon` is running.", l.id, loops::every_label(l.every));
            Ok(())
        }
        _ => fail("usage: grandaddy loop add <prompt> --every 30m [--session S | --in DIR] [--until TEXT] [--max N]"),
    }
}

fn git_dir(args: &mut Vec<String>) -> Result<String, Fail> {
    if let Some(d) = opt(args, "--in").filter(|s| !s.is_empty()) {
        return Ok(util::expand(&d));
    }
    Ok(std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default())
}

fn cmd_git(args: &[String]) -> R {
    let mut a = args.to_vec();
    let sub = a.first().cloned().unwrap_or_default();
    let session_dir = |rest: &[String]| -> Result<String, Fail> {
        if rest.is_empty() {
            return Ok(std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default());
        }
        Ok(pick(rest, false, false)?.cwd)
    };
    let done = |r: Result<String, String>| -> R {
        println!("{}", r.map_err(Fail)?);
        Ok(())
    };
    match sub.as_str() {
        "log" => {
            let d = session_dir(&a[1..])?;
            for c in git::log(&d, 15) {
                println!("  {}  {:>4}  {}  ({})", c.hash, util::ago(now() - c.when), c.subject, c.author);
            }
            Ok(())
        }
        "branches" => {
            let d = session_dir(&a[1..])?;
            for b in git::branches(&d) {
                println!("  {} {:<30} {:>4}  {}", if b.current { "●" } else { " " }, b.name, util::ago(now() - b.when), util::one_line(&b.subject, 60));
            }
            Ok(())
        }
        "prs" => {
            let d = session_dir(&a[1..])?;
            let prs = git::prs(&d).map_err(Fail)?;
            if prs.is_empty() {
                println!("No open pull requests.");
            }
            for p in prs {
                println!("  #{:<5} {:<50} {}  checks {}{}", p.number, util::one_line(&p.title, 50), p.branch, p.checks, if p.draft { "  draft" } else { "" });
            }
            Ok(())
        }
        "commit" => {
            a.remove(0);
            let d = git_dir(&mut a)?;
            done(git::commit_all(&d, &a.join(" ")))
        }
        "push" | "pull" => {
            a.remove(0);
            let d = git_dir(&mut a)?;
            done(if sub == "push" { git::push(&d) } else { git::pull(&d) })
        }
        "branch" | "switch" => {
            a.remove(0);
            let d = git_dir(&mut a)?;
            let name = a.first().cloned().unwrap_or_default();
            done(if sub == "branch" { git::create_branch(&d, &name) } else { git::switch(&d, &name) })
        }
        "pr" => {
            a.remove(0);
            let d = git_dir(&mut a)?;
            let draft = opt(&mut a, "--draft").is_some();
            done(git::create_pr(&d, &a.join(" "), "", draft))
        }
        "merge" => {
            a.remove(0);
            let d = git_dir(&mut a)?;
            let del = opt(&mut a, "--delete-branch").is_some();
            let method = if opt(&mut a, "--merge").is_some() {
                "merge"
            } else if opt(&mut a, "--rebase").is_some() {
                "rebase"
            } else {
                opt(&mut a, "--squash");
                "squash"
            };
            let n: u64 = a.first().and_then(|x| x.trim_start_matches('#').parse().ok()).ok_or_else(|| Fail("usage: grandaddy git merge <number>".into()))?;
            done(git::merge_pr(&d, n, method, del))
        }
        _ => {
            let d = session_dir(&a)?;
            let r = git::repo(&d).ok_or_else(|| Fail(format!("{} isn't in a git repository.", util::short(&d))))?;
            let sync = match (&r.upstream, r.ahead, r.behind) {
                (None, _, _) => "not pushed yet".to_string(),
                (Some(_), 0, 0) => "up to date".into(),
                (Some(_), a, 0) => format!("{a} to push"),
                (Some(_), 0, b) => format!("{b} to pull"),
                (Some(_), a, b) => format!("{a} to push, {b} to pull"),
            };
            println!("{} on {} — {sync}", util::short(&r.root), r.branch);
            if r.changes.is_empty() {
                println!("No changes.");
            }
            for c in r.changes.iter().take(25) {
                println!("  {:<10} {}{}", c.kind, c.path, if c.staged { "  (staged)" } else { "" });
            }
            if r.changes.len() > 25 {
                println!("  … and {} more", r.changes.len() - 25);
            }
            if r.github.is_some() {
                if let Some(p) = git::current_pr(&d) {
                    println!("Pull request #{}: {} — checks {}, {}", p.number, p.title, p.checks, if p.review.is_empty() { "no review yet".into() } else { p.review.to_lowercase().replace('_', " ") });
                }
            }
            Ok(())
        }
    }
}

fn cmd_history(args: &[String]) -> R {
    let q = args.join(" ").to_lowercase();
    let rows: Vec<claude::Past> = claude::history()
        .into_iter()
        .filter(|h| q.is_empty() || h.prompt.as_deref().unwrap_or("").to_lowercase().contains(&q) || h.cwd.as_deref().unwrap_or("").to_lowercase().contains(&q))
        .collect();
    if rows.is_empty() {
        println!("{}", if q.is_empty() { "No past sessions found." } else { "Nothing matches." });
    }
    for h in rows.iter().take(40) {
        let folder = util::short(h.cwd.as_deref().unwrap_or("?"));
        let tail: String = folder.chars().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect();
        let prompt: String = h.prompt.as_deref().unwrap_or("").chars().take(70).collect();
        println!("  {}  {:>4}  {:<30}  {}", &h.sid[..8.min(h.sid.len())], util::ago(now() - h.mtime), tail, prompt);
    }
    if rows.len() > 40 {
        println!("  … {} more — narrow it down: grandaddy history <search>", rows.len() - 40);
    }
    if !rows.is_empty() {
        println!("\nContinue one:  grandaddy resume <id> [prompt]");
    }
    Ok(())
}

fn cmd_resume(args: &[String]) -> R {
    let Some(id) = args.first() else { return fail("usage: grandaddy resume <session-id or prefix> [prompt]") };
    let hits: Vec<claude::Past> = claude::history().into_iter().filter(|h| h.sid.starts_with(id.as_str())).collect();
    if hits.len() != 1 {
        return fail(if hits.is_empty() { "No past session with that id." } else { "That prefix matches several sessions — give more of the id." });
    }
    let p = args[1..].join(" ");
    let (ok, out) = launch::resume(&hits[0].sid, hits[0].cwd.as_deref(), (!p.is_empty()).then_some(p.as_str()));
    println!("{out}");
    if ok { Ok(()) } else { fail("") }
}

fn cmd_usage(args: &[String]) -> R {
    let days: u32 = args.first().and_then(|s| s.parse().ok()).unwrap_or(7);
    let u = claude::usage(days);
    println!("{:<12}{:>9}{:>9}{:>9}{:>9}", "day", "input", "output", "cache", "total");
    for (d, projects) in u.iter().rev().take(days as usize) {
        let mut t = claude::Tokens::default();
        for p in projects.values() {
            t.input += p.input;
            t.out += p.out;
            t.cache += p.cache;
        }
        println!("{:<12}{:>9}{:>9}{:>9}{:>9}", d, util::tokens(t.input), util::tokens(t.out), util::tokens(t.cache), util::tokens(t.total()));
    }
    if let Some(projects) = u.get(&util::today()) {
        println!("\nToday by project:");
        let mut v: Vec<_> = projects.iter().collect();
        v.sort_by_key(|(_, t)| std::cmp::Reverse(t.total()));
        for (p, t) in v.into_iter().take(10) {
            println!("  {:>8}  {}", util::tokens(t.total()), util::short(p));
        }
    }
    Ok(())
}

fn cmd_phone(args: &[String]) -> R {
    match args.first().map(String::as_str).unwrap_or("status") {
        "setup" => {
            let c = phone::setup(args.get(1).map(String::as_str).unwrap_or("https://ntfy.sh"));
            println!("Phone approvals are on.");
            println!("1. Install the free ntfy app (iOS or Android).");
            println!("2. Subscribe to this topic on {}:\n\n     {}\n", c.server, c.topic);
            println!("3. Check it works:  grandaddy phone test");
            println!("\nThe topic name works like a password: anyone who has it can see and answer your prompts.");
            println!("Prompts pass through that ntfy server. For anything sensitive, use your own:");
            println!("  grandaddy phone setup https://ntfy.example.com");
            Ok(())
        }
        "test" => {
            println!("{}", phone::test().map_err(Fail)?);
            Ok(())
        }
        "off" => {
            phone::off();
            println!("Phone approvals are off.");
            Ok(())
        }
        _ => {
            match phone::config() {
                Some(c) => println!("On: {}  topic {}", c.server, c.topic),
                None => println!("Off — grandaddy phone setup"),
            }
            Ok(())
        }
    }
}

fn print_qr(data: &str) {
    use qrcode::render::unicode::Dense1x2;
    if let Ok(q) = qrcode::QrCode::new(data.as_bytes()) {
        println!("{}", q.render::<Dense1x2>().dark_color(Dense1x2::Light).light_color(Dense1x2::Dark).quiet_zone(true).build());
    }
}

fn cmd_web(args: &[String]) -> R {
    match args.first().map(String::as_str).unwrap_or("url") {
        "on" => {
            web::ensure_token();
            config::update(|c| c.web.enabled = true);
            println!("The phone page is on. It's served by the Grandaddy app or `grandaddy daemon`.");
            cmd_web(&["url".into()])
        }
        "off" => {
            config::update(|c| c.web.enabled = false);
            println!("The phone page is off.");
            Ok(())
        }
        _ => {
            let url = web::url().ok_or_else(|| Fail("This Mac isn't on a network right now.".into()))?;
            if !config::load().web.enabled {
                println!("The phone page is off — grandaddy web on");
            }
            println!("Open this on your phone (same Wi-Fi):\n\n  {url}\n");
            print_qr(&url);
            Ok(())
        }
    }
}

fn cmd_remote(args: &[String]) -> R {
    let (Some(state), Some(agent)) = (args.first(), args.get(1)) else {
        let c = config::load();
        println!("Holding prompts for an answer from grandaddy:");
        println!("  Claude Code   always (its own dialog stays up too)");
        for t in ["codex", "antigravity", "cursor", "gemini"] {
            println!("  {:<13} {}", providers::label(t), if c.remote.get(t).copied().unwrap_or(false) { "on" } else { "off" });
        }
        println!("\nOther agents' hooks run before their own prompt appears, so while one waits here its window");
        println!("shows nothing. After {}s it falls back to its own prompt.  grandaddy remote on|off <agent>", c.remote_timeout);
        return Ok(());
    };
    let on = state == "on";
    if !["codex", "antigravity", "cursor", "gemini"].contains(&agent.as_str()) {
        return fail("Remote answering can be turned on for: codex, antigravity, cursor, gemini");
    }
    config::update(|c| {
        c.remote.insert(agent.clone(), on);
    });
    println!("{}'s prompts {} wait for an answer from grandaddy.", providers::label(agent), if on { "will now" } else { "won't" });
    Ok(())
}

fn cmd_awake(args: &[String]) -> R {
    match (args.first().map(String::as_str), args.get(1).map(String::as_str)) {
        (Some("on"), _) => {
            config::update(|c| c.awake.enabled = true);
            println!("Your Mac will stay awake while sessions are working (the display still sleeps).");
        }
        (Some("off"), _) => {
            config::update(|c| c.awake.enabled = false);
            println!("Staying awake is off.");
        }
        (Some("lid"), Some(mode @ ("full" | "agents"))) => {
            let need_agents = mode == "agents" && !awake::agents_helper_installed();
            if !awake::lid_helper_installed() || need_agents {
                println!("macOS asks for your password once, to allow exactly `pmset -a disablesleep 0|1` and `pmset -b lowpowermode 0|1`.");
                awake::install_lid_helper().map_err(Fail)?;
            }
            config::update(|c| {
                c.awake.enabled = true;
                c.awake.lid = mode.into();
            });
            if mode == "full" {
                println!("With the lid closed the whole Mac keeps running while sessions work.");
            } else {
                println!("With the lid closed only your agents keep running: the display goes off, other apps pause,");
                println!("sound mutes and Low Power Mode turns on. Opening the lid puts it all back.");
            }
            println!("Keep it somewhere it can breathe, not in a bag.");
        }
        (Some("lid"), Some("off" | "sleep")) => {
            config::update(|c| {
                c.awake.lid = "sleep".into();
                c.awake.lid_closed = false;
            });
            awake::recover();
            println!("Closing the lid puts the Mac to sleep again.");
        }
        (Some("lid"), _) => return fail("usage: grandaddy awake lid off|full|agents"),
        (Some("preview"), _) => {
            let (pause, stay) = awake::preview(&sessions::collect());
            println!("With the lid closed in “agents only”, these would pause:");
            println!("  {}", if pause.is_empty() { "nothing".to_string() } else { pause.join(", ") });
            println!("\nThese keep running:");
            for (name, why) in stay {
                println!("  {name:<28} {why}");
            }
        }
        _ => {
            let c = config::load().awake;
            let lid = match c.lid_mode() {
                "full" => "everything keeps running",
                "agents" => "only the agents keep running",
                _ => "the Mac sleeps",
            };
            println!("Stay awake while sessions work: {}", if c.enabled { "on" } else { "off" });
            println!("With the lid closed: {lid}{}", if c.lid_mode() != "sleep" && !awake::lid_helper_installed() { " (needs setup: grandaddy awake lid " .to_string() + c.lid_mode() + ")" } else { String::new() });
            if c.lid_mode() == "agents" {
                println!("Kept running besides the agents: {}", c.keep.join(", "));
            }
            println!("This runs inside the Grandaddy app or `grandaddy daemon`.");
        }
    }
    Ok(())
}

fn cmd_continue(args: &[String]) -> R {
    match args.first().map(String::as_str) {
        Some(v @ ("on" | "off")) => {
            let msg = args[1..].join(" ");
            let c = config::update(|c| {
                c.auto_continue.enabled = v == "on";
                if !msg.trim().is_empty() {
                    c.auto_continue.message = msg.trim().to_string();
                }
            });
            if v == "on" {
                println!("When a session stops on a usage limit, grandaddy sends it “{}” once the limit resets.", c.auto_continue.message);
                println!("It types into sessions started with `grandaddy run` (or in iTerm, Terminal, tmux) and resumes");
                println!("background Claude sessions; for any other session you get a notification instead.");
            } else {
                println!("Auto-continue is off.");
            }
            Ok(())
        }
        Some("skip") | Some("unskip") => {
            let s = pick(&args[1..], false, false)?;
            limits::set_skip(&s.key, args[0] == "skip");
            println!("{} {} be continued after limits.", s.name, if args[0] == "skip" { "won't" } else { "will" });
            Ok(())
        }
        _ => {
            let c = config::load().auto_continue;
            println!("Auto-continue after usage limits: {} (message: “{}”)", if c.enabled { "on" } else { "off" }, c.message);
            for s in sessions::collect().iter().filter(|s| s.limit.is_some()) {
                let l = s.limit.as_ref().unwrap();
                let when = l.resets_at.map(|r| util::clock(Some(r))).unwrap_or_else(|| "unknown".into());
                println!("  {}  {} · resets {when} · {}", s.name, limits::label(&l.kind), s.limit_plan.clone().unwrap_or_default());
            }
            Ok(())
        }
    }
}

fn cmd_mesh(args: &[String]) -> R {
    match args.first().map(String::as_str) {
        Some(v @ ("on" | "off")) => {
            config::update(|c| c.experimental_mesh = v == "on");
            println!("Sessions talking to each other: {v}.{}", if v == "on" { " Connect agents with: grandaddy mcp install all" } else { "" });
            Ok(())
        }
        _ => {
            println!("Experimental. Sessions talking to each other: {}", if config::load().experimental_mesh { "on" } else { "off" });
            Ok(())
        }
    }
}

fn cmd_mcp(args: &[String]) -> R {
    let (Some(verb), Some(agent)) = (args.first(), args.get(1)) else { return fail("usage: grandaddy mcp install|remove <agent|all>") };
    let targets: Vec<&str> = if agent == "all" { install::MCP_TOOLS.iter().copied().filter(|t| install::detected(t)).collect() } else { vec![agent.as_str()] };
    for t in targets {
        let r = if verb == "remove" { install::uninstall_mcp(t).map(|_| format!("removed from {}", providers::label(t))) } else { install::install_mcp(t) };
        match r {
            Ok(m) => println!("{m}"),
            Err(e) => println!("{}: {e}", providers::label(t)),
        }
    }
    Ok(())
}

fn cmd_tools() -> R {
    println!("{:<14} {:<10} {:<16} MCP (S2S)", "agent", "on Mac", "approvals");
    for t in install::MCP_TOOLS {
        let hook = if install::HOOK_TOOLS.contains(t) {
            if install::hook_installed(t) { "connected" } else { "not connected" }
        } else {
            "—"
        };
        println!("{:<14} {:<10} {:<16} {}", providers::label(t), if install::detected(t) { "yes" } else { "no" }, hook, if install::mcp_installed(t) { "added" } else { "—" });
    }
    println!("\nConnect approvals:  grandaddy install <agent|all>    Add the MCP server:  grandaddy mcp install <agent|all>");
    if install::hook_installed("codex") {
        println!("Codex runs new hooks only after you trust them: open Codex and use /hooks.");
    }
    Ok(())
}

fn cmd_install(args: &[String]) -> R {
    let which = args.first().map(String::as_str).unwrap_or("claude");
    let targets: Vec<&str> = if which == "all" { install::HOOK_TOOLS.iter().copied().filter(|t| install::detected(t)).collect() } else { vec![which] };
    for t in targets {
        match install::install_hook(t) {
            Ok("already") => println!("{}: already connected", providers::label(t)),
            Ok(r) => {
                println!("{}: {r}", providers::label(t));
                if let Some(n) = install::hook_status(t).note {
                    println!("  {n}");
                }
            }
            Err(e) => println!("{}: {e}", providers::label(t)),
        }
    }
    Ok(())
}

fn cmd_uninstall(args: &[String]) -> R {
    let which = args.first().map(String::as_str).unwrap_or("claude");
    match install::uninstall_hook(which) {
        Ok(true) => println!("Disconnected {}.", providers::label(which)),
        Ok(false) => println!("{} wasn't connected.", providers::label(which)),
        Err(e) => return fail(e),
    }
    Ok(())
}

/// Loops, staying awake and the phone page, for people not running the app.
fn cmd_daemon() -> R {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    let lock = util::state().join("daemon.pid");
    if let Some(pid) = std::fs::read_to_string(&lock).ok().and_then(|s| s.trim().parse::<i64>().ok()) {
        if util::alive(pid) && pid != std::process::id() as i64 {
            return fail(format!("Already running (pid {pid}) — in the Grandaddy app or another `grandaddy daemon`."));
        }
    }
    let _ = std::fs::create_dir_all(util::state());
    std::fs::write(&lock, std::process::id().to_string()).map_err(io)?;
    println!("Running loops, stay-awake and the phone page. ctrl-c to stop.");
    let mut keeper = awake::Keeper::new();
    let stop = Arc::new(AtomicBool::new(false));
    let mut web_thread: Option<std::thread::JoinHandle<()>> = None;
    loop {
        let live = sessions::collect();
        keeper.update(&live);
        for note in loops::tick(&live).into_iter().chain(limits::tick(&live)) {
            println!("{} {note}", util::clock(Some(now())));
        }
        if config::load().web.enabled && web_thread.as_ref().map(|t| t.is_finished()).unwrap_or(true) {
            let s = stop.clone();
            web_thread = Some(std::thread::spawn(move || {
                if let Err(e) = web::serve(s) {
                    eprintln!("{e}");
                }
            }));
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
}
