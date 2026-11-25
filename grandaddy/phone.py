"""Approve from your phone with ntfy (https://ntfy.sh): free, no account, open source app.

When a session needs you, the hook posts a notification with buttons to your private topic.
Tapping a button posts a reply to <topic>-reply, which the hook polls. Nothing has to reach
your Mac from outside. The topic name is the secret: anyone who knows it can read your
prompts and answer them, so it's long and random, and you can point it at your own server.
"""
import json, os, secrets, time, urllib.parse, urllib.request
from . import claude, core

CONFIG = os.path.join(core.STATE, "phone.json")
TIMEOUT = 5


def config():
    c = core.read_json(CONFIG)
    return c if c and c.get("server") and c.get("topic") else None


def setup(server="https://ntfy.sh"):
    c = {"server": server.rstrip("/"), "topic": "grandaddy-" + secrets.token_urlsafe(18)}
    core.write_json(CONFIG, c)
    return c


def off():
    core.remove(CONFIG)


def _post(url, data, json_body=True):
    body = json.dumps(data).encode() if json_body else data.encode()
    req = urllib.request.Request(url, data=body, method="POST",
                                 headers={"Content-Type": "application/json"} if json_body else {})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        return r.status


def publish(c, title, message, buttons=()):
    """buttons: [(label, reply_body)] — at most 3 (ntfy's limit)."""
    actions = [{"action": "http", "label": label, "url": f"{c['server']}/{c['topic']}-reply",
                "method": "POST", "body": body, "clear": True} for label, body in list(buttons)[:3]]
    return _post(c["server"] + "/", {"topic": c["topic"], "title": title[:200], "message": message[:3500],
                                     "priority": 4, "tags": ["older_man"], "actions": actions})


def request_buttons(req):
    """Buttons for a pending request, or () when it needs the hub (multi-part questions)."""
    rid = req["id"]
    if req["tool"] != "AskUserQuestion":
        return [("Allow", f"{rid} allow"), ("Deny", f"{rid} deny")]
    qs = (req["input"] or {}).get("questions") or []
    if len(qs) == 1 and not qs[0].get("multiSelect") and len(qs[0].get("options") or []) <= 3:
        return [(o.get("label", str(i))[:40], f"{rid} {i}") for i, o in enumerate(qs[0]["options"], 1)]
    return ()


def notify_request(req, name):
    c = config()
    if not c:
        return False
    buttons = request_buttons(req)
    msg = claude.describe_request(req)
    if req["tool"] == "AskUserQuestion":
        qs = (req["input"] or {}).get("questions") or []
        msg = qs[0].get("question", msg) if qs else msg
        if not buttons:
            msg += "\n(answer this one in grandaddy)"
    try:
        publish(c, f"{name} needs you", msg, buttons)
        return True
    except OSError:
        return False


def replies(c, rid, since):
    """Reply bodies for this request posted since `since` (unix seconds)."""
    url = f"{c['server']}/{urllib.parse.quote(c['topic'])}-reply/json?poll=1&since={int(since)}"
    try:
        with urllib.request.urlopen(url, timeout=TIMEOUT) as r:
            lines = r.read().decode("utf-8", "ignore").splitlines()
    except OSError:
        return []
    out = []
    for l in lines:
        try:
            m = json.loads(l)
        except ValueError:
            continue
        if m.get("event") == "message" and str(m.get("message", "")).startswith(rid + " "):
            out.append(m["message"][len(rid) + 1:].strip())
    return out


def decision_from_reply(req, reply):
    """'allow' | 'deny' | option number -> the same decision shape the hub writes."""
    if reply in ("allow", "deny") and req["tool"] != "AskUserQuestion":
        return {"kind": reply}
    if reply.isdigit() and req["tool"] == "AskUserQuestion":
        qs = (req["input"] or {}).get("questions") or []
        opts = qs[0].get("options") or [] if qs else []
        if 1 <= int(reply) <= len(opts):
            return {"kind": "answers", "answers": {qs[0].get("question", ""): opts[int(reply) - 1].get("label", "")}}
    return None


def test():
    c = config()
    if not c:
        return False, "phone isn't set up — run: grandaddy phone setup"
    try:
        publish(c, "grandaddy", f"Test from grandaddy at {time.strftime('%H:%M:%S')}. If you see this, you're set.")
        return True, "sent a test notification"
    except OSError as e:
        return False, f"couldn't reach {c['server']}: {e}"
