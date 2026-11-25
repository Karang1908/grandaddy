"""grandaddy — manage your vibe coding sessions. All in one place.

  grandaddy                     the live hub (↑↓ pick a session, q quits)
  grandaddy ls                  every session and what state it's in
  grandaddy watch [target]      follow one session's activity as it happens
  grandaddy pending             everything waiting on you
  grandaddy approve [target]    say yes to a session's permission prompt
  grandaddy deny [target]       ...or no
  grandaddy answer [target] <choice> [<choice> ...]
                                 answer its question: an option number or your own text,
                                 one per question; "1,3" picks several on a multi-select
  grandaddy auto on|off [target|--all]
                                 auto-yes: approve every prompt in that session
  grandaddy rules [add <rule> | rm <n>]
                                 finer auto-yes:  yes Bash(npm test*) · ask Bash(git push*) · no Bash(rm -rf*)
  grandaddy start <prompt> [--in DIR] [--worktree [NAME]] [--model M]
                                 start a background Claude session (shows up in the hub)
  grandaddy open [target]        jump to its terminal, or attach to a background session
  grandaddy stop [target]        stop a background session
  grandaddy history [search]     past sessions you started
  grandaddy resume <id> [prompt] continue a past session in the background
  grandaddy usage [days]         tokens by day and project
  grandaddy phone setup [server] | test | off
                                 approve from your phone with the free ntfy app
  grandaddy log [n]             what was approved, denied or answered
  grandaddy install | uninstall the Claude Code hook (once)

target = pid, name, background id, session-id prefix or folder; left out = the session in this folder.
"""
import os, sys, time
from . import brand, claude, core, hook, install, launch, phone, rules, sessions


def die(msg):
    print(msg, file=sys.stderr)
    sys.exit(1)


def row(s, now):
    flag = " ◆auto" if s["auto"] else ""
    ident = f"bg {s['short']}" if s["bg"] else f"pid {s['pid']}"
    return (f"  ● {s['name'][:34]:<34} {s['status']:<9} {core.ago(now - s['since']):>4}  "
            f"{s['tool']:<8} {ident:<11} {core.short(s['cwd'])}{flag}")


def pick(args, claude_only=False, need_pending=False):
    sess = sessions.collect()
    target = args[0] if args else None
    m = sessions.resolve(target, sess, claude_only)
    if need_pending and target is None and len(m) != 1:
        waiting = [s for s in sess if s["pending"]]
        if len(waiting) == 1:
            return waiting[0]  # only one thing is waiting on you anyway
    if len(m) == 1:
        return m[0]
    where = f"'{target}'" if target else f"this folder ({os.getcwd()})"
    print(("no session matches " if not m else f"{len(m)} sessions match ") + where + ":", file=sys.stderr)
    now = time.time()
    for s in (m or [s for s in sess if s["tool"] == "claude" or not claude_only]):
        print(row(s, now), file=sys.stderr)
    die("pass a pid or session-id prefix to choose")


def cmd_ls(args):
    sess, now = sessions.collect(), time.time()
    if not install.installed():
        print("! hook not installed — run: grandaddy install")
    if core.ALL in core.auto_ids():
        print("auto-yes is on for ALL sessions")
    if not sess:
        print("no sessions running")
    for s in sess:
        print(row(s, now))
        for r in s["pending"]:
            print(f"      ↳ {claude.describe_request(r)}")


def cmd_pending(args):
    sess = [s for s in sessions.collect() if s["pending"]]
    if not sess:
        print("nothing is waiting on you")
    for s in sess:
        for r in s["pending"]:
            print(f"{s['name']} (pid {s['pid']}) {claude.describe_request(r)}")
            if r["tool"] == "AskUserQuestion":
                for qi, q in enumerate(r["input"].get("questions") or [], 1):
                    multi = "  (pick several)" if q.get("multiSelect") else ""
                    print(f"   Q{qi}. {q.get('question', '')}{multi}")
                    for i, o in enumerate(q.get("options") or [], 1):
                        print(f"       {i}  {o.get('label', '')}")
            else:
                print("   grandaddy approve / deny")


def first_pending(s):
    if not s["pending"]:
        die(f"{s['name']} isn't waiting on anything grandaddy can answer")
    return s["pending"][0]


def cmd_decide(args, allow):
    s = pick(args, claude_only=True, need_pending=True)
    req = first_pending(s)
    if req["tool"] == "AskUserQuestion":
        die(f"{s['name']} asked a question — use: grandaddy answer <choice>")
    claude.answer(req, {"kind": "allow" if allow else "deny"})
    print(("allowed: " if allow else "denied: ") + claude.tool_summary(req["tool"], req["input"]))


def build_answers(questions, choices):
    if len(choices) != len(questions):
        die(f"this asks {len(questions)} question(s); give one answer each")
    out = {}
    for q, c in zip(questions, choices):
        opts = q.get("options") or []
        picks = c.split(",") if q.get("multiSelect") else [c]
        if all(p.strip().isdigit() and 1 <= int(p) <= len(opts) for p in picks):
            out[q.get("question", "")] = ", ".join(opts[int(p) - 1]["label"] for p in picks)
        else:
            out[q.get("question", "")] = c  # your own words
    return out


def cmd_answer(args):
    # the target is optional, so anything that isn't a live session is treated as an answer
    sess = sessions.collect()
    target = args[0] if args and len(sessions.resolve(args[0], sess, True)) == 1 and len(args) > 1 else None
    s = pick([target] if target else [], claude_only=True, need_pending=True)
    choices = args[1:] if target else args
    req = first_pending(s)
    if req["tool"] != "AskUserQuestion":
        die(f"{s['name']} wants permission, not an answer — use: grandaddy approve / deny")
    if not choices:
        die("give an answer, e.g.  grandaddy answer 2")
    answers = build_answers(req["input"].get("questions") or [], choices)
    claude.answer(req, {"kind": "answers", "answers": answers})
    print("answered: " + "; ".join(f"{q} → {a}" for q, a in answers.items()))


def fmt_event(e, who):
    k = e["k"]
    if k == "you":
        when = time.strftime("%H:%M", time.localtime(e["ts"])) if e.get("ts") else "     "
        return f"\n{when} › {e['text']}"
    if k == "text":
        return "      " + e["text"].replace("\n", "\n      ")
    if k == "tool":
        return f"      ▸ {e['tool']:<10} {e['text']}"
    if k == "error":
        return f"      ✗ {e['text']}"
    return f"      ── {e['text']} ──"


def cmd_watch(args):
    s = pick(args)
    print(f"{brand.MINI} watching {s['name']} ({s['tool']}, pid {s['pid']}) — ctrl-c to stop")
    seen = 0
    try:
        while True:
            events = sessions.activity(s)["events"]
            if seen == 0 or len(events) < seen:
                seen = max(0, len(events) - 15)  # start with recent context; resync if the tail moved
            for e in events[seen:]:
                print(fmt_event(e, s["tool"]))
            seen = len(events)
            if s["pid"] and not core.alive(s["pid"]):
                print("— session ended")
                return
            time.sleep(1)
    except KeyboardInterrupt:
        pass


def cmd_auto(args):
    if not args or args[0] not in ("on", "off"):
        die("usage: grandaddy auto on|off [target|--all]")
    on, rest = args[0] == "on", args[1:]
    if on and not install.installed():
        die("hook not installed yet — run: grandaddy install")
    if rest[:1] == ["--all"]:
        if on:
            core.set_auto(core.ALL, True)
            print("auto-yes on for ALL sessions — every permission prompt will be approved")
        else:
            import shutil
            shutil.rmtree(core.AUTO, ignore_errors=True)
            print("auto-yes off everywhere")
        return
    s = pick(rest, claude_only=True)
    core.set_auto(s["sid"], on)
    print(f"auto-yes {'on' if on else 'off'}: {s['name']} (pid {s['pid']})")
    if not on and core.ALL in core.auto_ids():
        print("note: ALL is still on — grandaddy auto off --all")


def cmd_log(args):
    lines = core.log_tail(int(args[0]) if args else 20)
    print("\n".join(lines) if lines else "nothing logged yet")


def cmd_install(args):
    r = install.install()
    print({"installed": f"installed the hook in {install.SETTINGS}",
           "updated": f"updated the hook in {install.SETTINGS} (replaced the old one)",
           "already": "already installed"}[r])
    if r != "already":
        print("running sessions pick it up straight away; nothing is auto-approved until you ask")


def cmd_uninstall(args):
    print("uninstalled; auto-yes is off everywhere" if install.uninstall() else "not installed")


def cmd_rules(args):
    if args[:1] == ["add"] and len(args) > 1:
        try:
            r = rules.add(" ".join(args[1:]))
        except ValueError as e:
            die(str(e))
        print(f"added: {r['text']}")
        if not install.installed():
            print("note: rules only apply once the hook is installed — grandaddy install")
        return
    if args[:1] == ["rm"] and len(args) == 2 and args[1].isdigit():
        try:
            print("removed: " + rules.remove(int(args[1])))
        except (IndexError, FileNotFoundError) as e:
            die(str(e) or "no rules yet")
        return
    if args:
        die("usage: grandaddy rules [add <rule> | rm <n>]")
    rl = rules.load()
    if not rl:
        print("no rules yet. Examples:")
        print('  grandaddy rules add yes "Bash(npm test*)"     approve automatically, every session')
        print('  grandaddy rules add ask "Bash(git push*)"     always ask, even with auto-yes on')
        print('  grandaddy rules add no  "Bash(rm -rf /*)"     refuse automatically')
    for i, r in enumerate(rl, 1):
        print(f"  {i}. {r['text']}")
    print(f"\n({rules.PATH}; first match wins)")


def opt(args, name, takes_value=True):
    """Pull --name [value] out of args."""
    if name not in args:
        return None, args
    i = args.index(name)
    if not takes_value:
        return True, args[:i] + args[i + 1:]
    val = args[i + 1] if i + 1 < len(args) and not args[i + 1].startswith("--") else ""
    return val, args[:i] + args[i + 1 + (1 if val else 0):]


def cmd_start(args):
    folder, args = opt(args, "--in")
    worktree, args = opt(args, "--worktree")
    model, args = opt(args, "--model")
    prompt = " ".join(args).strip()
    if not prompt:
        die("usage: grandaddy start <prompt> [--in DIR] [--worktree [NAME]] [--model M]")
    ok, out = launch.new(os.path.expanduser(folder or os.getcwd()), prompt, worktree, model or None)
    print(out)
    sys.exit(0 if ok else 1)


def cmd_open(args):
    s = pick(args)
    if s["bg"] and s["short"]:
        ok, out = launch.open_terminal(f"claude attach {s['short']}", s["cwd"])
    elif s["pid"]:
        ok, out = launch.jump(s["pid"])
    else:
        ok, out = False, "nothing to open"
    print(out)
    sys.exit(0 if ok else 1)


def cmd_stop(args):
    s = pick(args, claude_only=True)
    if not (s["bg"] and s["short"]):
        die(f"{s['name']} isn't a background session — stop it in its own terminal")
    ok, out = launch._claude(["stop", s["short"]], None)
    print(out or ("stopped" if ok else "failed"))
    sys.exit(0 if ok else 1)


def cmd_history(args):
    q = " ".join(args).lower()
    now = time.time()
    rows = [h for h in claude.history() if not q or q in (h["prompt"] or "").lower() or q in (h["cwd"] or "").lower()]
    if not rows:
        print("nothing matches" if q else "no past sessions found")
    for h in rows[:40]:
        print(f"  {h['sid'][:8]}  {core.ago(now - h['mtime']):>4}  {core.short(h['cwd'] or '?')[-30:]:<30}  {h['prompt'][:70]}")
    if len(rows) > 40:
        print(f"  … {len(rows) - 40} more — narrow it down: grandaddy history <search>")
    if rows:
        print("\ncontinue one:  grandaddy resume <id> [prompt]")


def cmd_resume(args):
    if not args:
        die("usage: grandaddy resume <session-id or prefix> [prompt]")
    hits = [h for h in claude.history() if h["sid"].startswith(args[0])]
    if len(hits) != 1:
        die("no past session with that id" if not hits else "that prefix matches several sessions — give more of the id")
    h = hits[0]
    ok, out = launch.resume(h["sid"], h["cwd"], " ".join(args[1:]) or None)
    print(out)
    sys.exit(0 if ok else 1)


def cmd_usage(args):
    days = int(args[0]) if args and args[0].isdigit() else 7
    u = claude.usage(days)
    fmt = lambda n: f"{n / 1e6:.1f}M" if n >= 1e6 else f"{n / 1e3:.0f}k" if n >= 1e3 else str(n)
    print(f"{'day':<12}{'input':>9}{'output':>9}{'cache':>9}{'total':>9}")
    for d in sorted(u, reverse=True)[:days]:
        t = {k: sum(p[k] for p in u[d].values()) for k in ("in", "out", "cache")}
        print(f"{d:<12}{fmt(t['in']):>9}{fmt(t['out']):>9}{fmt(t['cache']):>9}{fmt(sum(t.values())):>9}")
    today = u.get(time.strftime("%Y-%m-%d"), {})
    if today:
        print("\ntoday by project:")
        for proj, t in sorted(today.items(), key=lambda kv: -sum(kv[1].values()))[:10]:
            print(f"  {fmt(sum(t.values())):>8}  {core.short(proj)}")


def cmd_phone(args):
    sub = args[0] if args else "status"
    if sub == "setup":
        c = phone.setup(args[1] if len(args) > 1 else "https://ntfy.sh")
        print("Phone approvals are on.")
        print(f"1. Install the free ntfy app (iOS / Android).")
        print(f"2. Subscribe to this topic on server {c['server']}:")
        print(f"\n     {c['topic']}\n")
        print("3. Check it works:  grandaddy phone test")
        print("\nThe topic name is the password: anyone who has it can see and answer your prompts.")
        print("Your prompts pass through that ntfy server. Use your own server for anything sensitive:")
        print("  grandaddy phone setup https://ntfy.example.com")
    elif sub == "test":
        ok, out = phone.test()
        print(out)
        sys.exit(0 if ok else 1)
    elif sub == "off":
        phone.off()
        print("phone approvals off")
    else:
        c = phone.config()
        print(f"on: {c['server']}  topic {c['topic']}" if c else "off — grandaddy phone setup")


def main():
    core.migrate_state()
    a = sys.argv[1:]
    if not a:
        if sys.stdin.isatty() and sys.stdout.isatty():
            from . import hub
            return hub.run()
        return cmd_ls([])
    cmd, rest = a[0], a[1:]
    table = {
        "hook": lambda r: hook.run(), "ls": cmd_ls, "list": cmd_ls, "watch": cmd_watch,
        "pending": cmd_pending, "approve": lambda r: cmd_decide(r, True), "deny": lambda r: cmd_decide(r, False),
        "answer": cmd_answer, "auto": cmd_auto, "log": cmd_log,
        "install": cmd_install, "uninstall": cmd_uninstall, "rules": cmd_rules, "start": cmd_start,
        "open": cmd_open, "stop": cmd_stop, "history": cmd_history, "resume": cmd_resume,
        "usage": cmd_usage, "phone": cmd_phone,
        # from the auto-yes days
        "attach": lambda r: cmd_auto(["on"] + r), "detach": lambda r: cmd_auto(["off"] + r),
    }
    if cmd not in table:
        print(__doc__.strip())
        sys.exit(0 if cmd in ("help", "-h", "--help") else 1)
    table[cmd](rest)
