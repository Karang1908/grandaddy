use grandaddy_core::claude::{self, Decision, Request};
use grandaddy_core::sessions::{self, Session};
use grandaddy_core::util::{self, now};
use grandaddy_core::{hook, install, launch, phone, rules};
use serde_json::{Map, Value};
use std::io::{IsTerminal, Read};

pub const USAGE: &str = "grandaddy — manage your vibe coding sessions. All in one place.

  grandaddy                     the live hub (↑↓ pick a session, ? for keys, q quits)
  grandaddy ls                  every session and what state it's in
  grandaddy watch [target]      follow one session's activity as it happens
  grandaddy pending             everything waiting on you
  grandaddy approve [target]    say yes to a session's permission prompt
  grandaddy deny [target]       ...or no
  grandaddy answer [target] <choice> [<choice> ...]
                                answer its question: an option number or your own text,
                                one per question; \"1,3\" picks several on a multi-select
  grandaddy auto on|off [target|--all]
                                auto-yes: approve every prompt in that session
  grandaddy rules [add <rule> | rm <n>]
                                finer auto-yes:  yes Bash(npm test*) · ask Bash(git push*) · no Bash(rm -rf*)
  grandaddy start <prompt> [--in DIR] [--worktree [NAME]] [--model M]
                                start a background Claude session (shows up in the hub)
  grandaddy open [target]       jump to its terminal, or attach to a background session
  grandaddy stop [target]       stop a background session
  grandaddy history [search]    past sessions you started
  grandaddy resume <id> [prompt]
                                continue a past session in the background
  grandaddy usage [days]        tokens by day and project
  grandaddy phone setup [server] | test | off
                                approve from your phone with the free ntfy app
  grandaddy log [n]             what was approved, denied or answered
  grandaddy install | uninstall the Claude Code hook (once)

target = pid, name, background id, session-id prefix or folder; left out = the session in this folder.";

struct Fail(String);
type R = Result<(), Fail>;
fn fail<T>(msg: impl Into<String>) -> Result<T, Fail> {
    Err(Fail(msg.into()))
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
            if let Some(out) = hook::run(&input) {
                println!("{out}");
            }
            Ok(())
        }
        "ls" | "list" => cmd_ls(),
        "watch" => cmd_watch(&rest),
        "pending" => cmd_pending(),
        "approve" => cmd_decide(&rest, true),
        "deny" => cmd_decide(&rest, false),
        "answer" => cmd_answer(&rest),
        "auto" => cmd_auto(&rest),
        "attach" => cmd_auto(&[vec!["on".to_string()], rest].concat()),
        "detach" => cmd_auto(&[vec!["off".to_string()], rest].concat()),
        "rules" => cmd_rules(&rest),
        "start" => cmd_start(&rest),
        "open" => cmd_open(&rest),
        "stop" => cmd_stop(&rest),
        "history" => cmd_history(&rest),
        "resume" => cmd_resume(&rest),
        "usage" => cmd_usage(&rest),
        "phone" => cmd_phone(&rest),
        "log" => {
            let n = rest.first().and_then(|s| s.parse().ok()).unwrap_or(20);
            let lines = util::log_tail(n);
            println!("{}", if lines.is_empty() { "nothing logged yet".into() } else { lines.join("\n") });
            Ok(())
        }
        "install" => cmd_install(),
        "uninstall" => {
            match install::uninstall() {
                Ok(true) => println!("uninstalled; auto-yes is off everywhere"),
                Ok(false) => println!("not installed"),
                Err(e) => return fail_code(&e.to_string()),
            }
            Ok(())
        }
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
        Err(Fail(m)) => fail_code(&m),
    }
}

fn fail_code(msg: &str) -> i32 {
    if !msg.is_empty() {
        eprintln!("{msg}");
    }
    1
}

fn row(s: &Session) -> String {
    let flag = if s.auto { " ◆auto" } else { "" };
    let ident = if s.bg { format!("bg {}", s.short.clone().unwrap_or_default()) } else { format!("pid {}", s.pid.unwrap_or(0)) };
    let name: String = s.name.chars().take(34).collect();
    format!("  ● {:<34} {:<9} {:>4}  {:<8} {:<11} {}{}", name, s.status, util::ago(now() - s.since), s.tool, ident, util::short(&s.cwd), flag)
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
    eprintln!("{}{}:", if m.is_empty() { "no session matches ".to_string() } else { format!("{} sessions match ", m.len()) }, place);
    let shown: Vec<&Session> = if m.is_empty() { sess.iter().filter(|s| !claude_only || s.tool == "claude").collect() } else { m };
    for s in shown {
        eprintln!("{}", row(s));
    }
    fail("pass a pid or session-id prefix to choose")
}

fn cmd_ls() -> R {
    let sess = sessions::collect();
    if !install::installed() {
        println!("! hook not installed — run: grandaddy install");
    }
    if util::auto_ids().contains(util::ALL) {
        println!("auto-yes is on for ALL sessions");
    }
    if sess.is_empty() {
        println!("no sessions running");
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
        println!("nothing is waiting on you");
    }
    for s in &sess {
        for r in &s.pending {
            let ident = s.pid.map(|p| format!("pid {p}")).or(s.short.as_ref().map(|x| format!("bg {x}"))).unwrap_or_default();
            println!("{} ({}) {}", s.name, ident, claude::describe_request(r));
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
    s.pending.first().cloned().ok_or_else(|| Fail(format!("{} isn't waiting on anything grandaddy can answer", s.name)))
}

fn cmd_decide(args: &[String], allow: bool) -> R {
    let s = pick(args, true, true)?;
    let req = first_pending(&s)?;
    if req.is_question() {
        return fail(format!("{} asked a question — use: grandaddy answer <choice>", s.name));
    }
    let d = if allow { Decision::Allow } else { Decision::Deny { message: None } };
    claude::answer(&req, &d).map_err(|e| Fail(e.to_string()))?;
    println!("{}{}", if allow { "allowed: " } else { "denied: " }, claude::tool_summary(&req.tool, &req.input));
    Ok(())
}

/// One answer per question: option numbers ("2", or "1,3" on a multi-select) or your own words.
pub fn build_answers(questions: &[Value], choices: &[String]) -> Result<Map<String, Value>, String> {
    if choices.len() != questions.len() {
        return Err(format!("this asks {} question(s); give one answer each", questions.len()));
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
    // the target is optional, so anything that isn't a live session is treated as an answer
    let sess = sessions::collect();
    let target = (args.len() > 1 && sessions::resolve(Some(&args[0]), &sess, true).len() == 1).then(|| args[0].clone());
    let s = pick(&target.clone().into_iter().collect::<Vec<_>>(), true, true)?;
    let choices = if target.is_some() { &args[1..] } else { args };
    let req = first_pending(&s)?;
    if !req.is_question() {
        return fail(format!("{} wants permission, not an answer — use: grandaddy approve / deny", s.name));
    }
    if choices.is_empty() {
        return fail("give an answer, e.g.  grandaddy answer 2");
    }
    let answers = build_answers(&req.questions(), choices).map_err(Fail)?;
    claude::answer(&req, &Decision::Answers { answers: answers.clone() }).map_err(|e| Fail(e.to_string()))?;
    let shown: Vec<String> = answers.iter().map(|(q, a)| format!("{q} → {}", a.as_str().unwrap_or(""))).collect();
    println!("answered: {}", shown.join("; "));
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
    let ident = s.pid.map(|p| format!("pid {p}")).or(s.short.as_ref().map(|x| format!("bg {x}"))).unwrap_or_default();
    println!("(◠‿◠) watching {} ({}, {}) — ctrl-c to stop", s.name, s.tool, ident);
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
                println!("— session ended");
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
        _ => return fail("usage: grandaddy auto on|off [target|--all]"),
    };
    if on && !install::installed() {
        return fail("hook not installed yet — run: grandaddy install");
    }
    let rest = &args[1..];
    if rest.first().map(String::as_str) == Some("--all") {
        if on {
            util::set_auto(util::ALL, true);
            println!("auto-yes on for ALL sessions — every permission prompt will be approved");
        } else {
            let _ = std::fs::remove_dir_all(util::auto_dir());
            println!("auto-yes off everywhere");
        }
        return Ok(());
    }
    let s = pick(rest, true, false)?;
    util::set_auto(s.sid.as_deref().unwrap_or(""), on);
    let ident = s.pid.map(|p| format!("pid {p}")).or(s.short.as_ref().map(|x| format!("bg {x}"))).unwrap_or_default();
    println!("auto-yes {}: {} ({})", if on { "on" } else { "off" }, s.name, ident);
    if !on && util::auto_ids().contains(util::ALL) {
        println!("note: ALL is still on — grandaddy auto off --all");
    }
    Ok(())
}

fn cmd_rules(args: &[String]) -> R {
    match args.first().map(String::as_str) {
        Some("add") if args.len() > 1 => {
            let r = rules::add(&args[1..].join(" ")).map_err(Fail)?;
            println!("added: {}", r.text);
            if !install::installed() {
                println!("note: rules only apply once the hook is installed — grandaddy install");
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
            println!("\n({}; first match wins)", rules::path().display());
            Ok(())
        }
    }
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
    let (ok, out) = match (&s.short, s.bg, s.pid) {
        (Some(id), true, _) => launch::open_terminal(&format!("claude attach {id}"), Some(&s.cwd)),
        (_, _, Some(pid)) => launch::jump(pid),
        _ => (false, "nothing to open".into()),
    };
    println!("{out}");
    if ok { Ok(()) } else { fail("") }
}

fn cmd_stop(args: &[String]) -> R {
    let s = pick(args, true, false)?;
    let Some(id) = s.short.as_ref().filter(|_| s.bg) else {
        return fail(format!("{} isn't a background session — stop it in its own terminal", s.name));
    };
    let (ok, out) = launch::stop(id);
    println!("{}", if out.is_empty() { if ok { "stopped".into() } else { "failed".into() } } else { out });
    if ok { Ok(()) } else { fail("") }
}

fn cmd_history(args: &[String]) -> R {
    let q = args.join(" ").to_lowercase();
    let rows: Vec<claude::Past> = claude::history()
        .into_iter()
        .filter(|h| q.is_empty() || h.prompt.as_deref().unwrap_or("").to_lowercase().contains(&q) || h.cwd.as_deref().unwrap_or("").to_lowercase().contains(&q))
        .collect();
    if rows.is_empty() {
        println!("{}", if q.is_empty() { "no past sessions found" } else { "nothing matches" });
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
        println!("\ncontinue one:  grandaddy resume <id> [prompt]");
    }
    Ok(())
}

fn cmd_resume(args: &[String]) -> R {
    let Some(id) = args.first() else { return fail("usage: grandaddy resume <session-id or prefix> [prompt]") };
    let hits: Vec<claude::Past> = claude::history().into_iter().filter(|h| h.sid.starts_with(id.as_str())).collect();
    if hits.len() != 1 {
        return fail(if hits.is_empty() { "no past session with that id" } else { "that prefix matches several sessions — give more of the id" });
    }
    let prompt = args[1..].join(" ");
    let (ok, out) = launch::resume(&hits[0].sid, hits[0].cwd.as_deref(), (!prompt.is_empty()).then_some(prompt.as_str()));
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
    let today = util::today();
    if let Some(projects) = u.get(&today) {
        println!("\ntoday by project:");
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
            println!("1. Install the free ntfy app (iOS / Android).");
            println!("2. Subscribe to this topic on server {}:\n\n     {}\n", c.server, c.topic);
            println!("3. Check it works:  grandaddy phone test");
            println!("\nThe topic name is the password: anyone who has it can see and answer your prompts.");
            println!("Your prompts pass through that ntfy server. Use your own server for anything sensitive:");
            println!("  grandaddy phone setup https://ntfy.example.com");
            Ok(())
        }
        "test" => {
            let msg = phone::test().map_err(Fail)?;
            println!("{msg}");
            Ok(())
        }
        "off" => {
            phone::off();
            println!("phone approvals off");
            Ok(())
        }
        _ => {
            match phone::config() {
                Some(c) => println!("on: {}  topic {}", c.server, c.topic),
                None => println!("off — grandaddy phone setup"),
            }
            Ok(())
        }
    }
}

fn cmd_install() -> R {
    let r = install::install().map_err(|e| Fail(e.to_string()))?;
    let p = install::settings_path();
    match r {
        "installed" => println!("installed the hook in {}", p.display()),
        "updated" => println!("updated the hook in {} (replaced the old one)", p.display()),
        _ => println!("already installed"),
    }
    if r != "already" {
        println!("running sessions pick it up straight away; nothing is auto-approved until you ask");
    }
    Ok(())
}
