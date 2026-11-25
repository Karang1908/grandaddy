"""The PermissionRequest hook. Claude Code runs it whenever it is about to show a dialog.

1. A `no` rule refuses and a `yes` rule approves, in any session (see rules.py).
2. auto-yes is on for the session and no `ask` rule matched: approve.
3. Otherwise publish the request to ~/.grandaddy/pending (and to your phone, if set up) and
   wait. The dialog is already on screen in the terminal, so you can answer there, in the hub,
   or on your phone; whichever comes first wins (Claude ignores a hook answer that arrives after
   you answered locally). The hook gives up as soon as the session stops waiting.
Questions (AskUserQuestion) are never answered by rules or auto-yes: they always reach you.
"""
import json, os, sys, time
from . import claude, core, phone, rules

MAX_WAIT = 3500   # settings.json gives the hook 3600s
GRACE = 5         # the hook can start a moment before Claude marks the session "waiting"
POLL = 0.25
PHONE_EVERY = 2.0


def decision_for(req, ans):
    kind = (ans or {}).get("kind")
    if kind == "allow":
        return {"behavior": "allow"}
    if kind == "deny":
        return {"behavior": "deny", "message": ans.get("message") or "Denied from grandaddy."}
    if kind == "answers" and isinstance(ans.get("answers"), dict):
        return {"behavior": "allow", "updatedInput": {**req["input"], "answers": ans["answers"]}}
    return None


def emit(decision):
    print(json.dumps({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": decision}}))


def session_name(sid):
    for s in claude._from_files():
        if s["sessionId"] == sid:
            return s.get("name") or sid[:8]
    return sid[:8]


def run():
    try:
        d = json.load(sys.stdin)
    except ValueError:
        return
    sid, tool = d.get("session_id") or "", d.get("tool_name") or ""
    if not sid or not tool:
        return
    inp = d.get("tool_input") or {}
    what = claude.tool_summary(tool, inp)
    if tool != "AskUserQuestion":
        rule = rules.match(tool, inp)
        if rule and rule["verb"] == "no":
            core.log(f"{sid[:8]}  denied by rule `{rule['text']}`  {what}")
            emit({"behavior": "deny", "message": f"Blocked by grandaddy rule: {rule['text']}"})
            return
        if rule and rule["verb"] == "yes":
            core.log(f"{sid[:8]}  allowed by rule `{rule['text']}`  {what}")
            emit({"behavior": "allow"})
            return
        if core.is_auto(sid) and not (rule and rule["verb"] == "ask"):
            core.log(f"{sid[:8]}  auto-approved  {what}")
            emit({"behavior": "allow"})
            return

    rid = f"{int(time.time() * 1000)}-{os.getpid()}"
    req = {"id": rid, "sid": sid, "tool": tool, "input": inp, "cwd": d.get("cwd"),
           "created": time.time(), "hook_pid": os.getpid()}
    ppath = os.path.join(core.PENDING, rid + ".json")
    apath = os.path.join(core.ANSWERS, rid + ".json")
    core.write_json(ppath, req)
    try:
        ph = phone.config()
        if ph:
            phone.notify_request(req, session_name(sid))
        t0, seen_waiting, next_phone = time.time(), False, time.time() + PHONE_EVERY
        while time.time() - t0 < MAX_WAIT:
            ans, via = core.read_json(apath), "grandaddy"
            if not ans and ph and time.time() >= next_phone:
                next_phone = time.time() + PHONE_EVERY
                for reply in phone.replies(ph, rid, req["created"] - 5):
                    ans, via = phone.decision_from_reply(req, reply), "phone"
                    if ans:
                        break
            dec = decision_for(req, ans)
            if dec:
                emit(dec)
                verb = "answered" if tool == "AskUserQuestion" else {"allow": "allowed", "deny": "denied"}[dec["behavior"]]
                core.log(f"{sid[:8]}  {verb} from {via}  {what}")
                return
            st = claude.raw_status(sid)
            if st == "waiting":
                seen_waiting = True
            elif seen_waiting or time.time() - t0 > GRACE:
                return  # answered in the terminal, interrupted, or the session exited
            time.sleep(POLL)
    finally:
        core.remove(ppath)
        core.remove(apath)
