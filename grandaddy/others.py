"""Other coding CLIs. Found from the process table; Codex also gets status + activity from its rollout log."""
import datetime, glob, json, os, subprocess, time
from . import core

TOOLS = ("codex", "gemini", "opencode", "aider", "cursor-agent", "amp", "goose", "qwen")
CODEX_SESSIONS = os.path.join(core.HOME, ".codex", "sessions")
_cwd = {}


def _etime_seconds(e):
    """ps etime: [[dd-]hh:]mm:ss"""
    days, _, rest = e.rpartition("-")
    parts = [int(x) for x in rest.split(":")]
    while len(parts) < 3:
        parts.insert(0, 0)
    h, m, s = parts
    return (int(days) if days else 0) * 86400 + h * 3600 + m * 60 + s


def _tool_of(args):
    for tok in args.split()[:2]:
        b = os.path.basename(tok)
        if b in TOOLS:
            return b
    return None


def cwd_of(pid):
    if pid in _cwd:
        return _cwd[pid]
    try:
        out = subprocess.run(["lsof", "-a", "-p", str(pid), "-d", "cwd", "-Fn"],
                             capture_output=True, text=True, timeout=2).stdout
        cwd = next((l[1:] for l in out.splitlines() if l.startswith("n")), "")
    except (OSError, subprocess.TimeoutExpired):
        cwd = ""
    _cwd[pid] = cwd
    return cwd


def processes():
    """Interactive (has a terminal) processes of known coding CLIs, wrapper processes collapsed."""
    try:
        out = subprocess.run(["ps", "-axo", "pid=,ppid=,tty=,etime=,args="],
                             capture_output=True, text=True, timeout=3).stdout
    except (OSError, subprocess.TimeoutExpired):
        return []
    found = {}
    for line in out.splitlines():
        parts = line.split(None, 4)
        if len(parts) < 5:
            continue
        pid, ppid, tty, etime, args = parts
        tool = _tool_of(args)
        if not tool or tty in ("??", "-"):
            continue
        try:
            found[int(pid)] = {"pid": int(pid), "ppid": int(ppid), "tool": tool, "tty": tty,
                               "started": time.time() - _etime_seconds(etime), "args": args}
        except ValueError:
            continue
    # a node wrapper and the native binary it spawns are one session: keep the outer one
    procs = [p for p in found.values() if not (p["ppid"] in found and found[p["ppid"]]["tool"] == p["tool"])]
    live = set(found)
    for pid in list(_cwd):
        if pid not in live:
            del _cwd[pid]
    for p in procs:
        p["cwd"] = cwd_of(p["pid"])
    return sorted(procs, key=lambda p: p["started"])


# ---------- Codex rollout logs ----------

def codex_rollout(cwd, started):
    """Most recently written rollout file for this folder since the process started."""
    if not cwd or not os.path.isdir(CODEX_SESSIONS):
        return None
    day = datetime.date.fromtimestamp(started - 86400)
    files = []
    while day <= datetime.date.today():
        files += glob.glob(os.path.join(CODEX_SESSIONS, f"{day:%Y/%m/%d}", "rollout-*.jsonl"))
        day += datetime.timedelta(days=1)
    for f in sorted(files, key=os.path.getmtime, reverse=True):
        if os.path.getmtime(f) < started - 120:
            break
        try:
            with open(f) as fh:
                meta = json.loads(fh.readline())
        except (OSError, ValueError):
            continue
        if (meta.get("payload") or {}).get("cwd") == cwd:
            return f
    return None


def _text(content):
    if isinstance(content, list):
        return " ".join(c.get("text", "") for c in content if isinstance(c, dict))
    return str(content or "")


def _ts(ts):
    try:
        return datetime.datetime.fromisoformat(str(ts).replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def codex_activity(path, limit=150):
    events, state = [], None
    try:
        lines = open(path, errors="ignore").read().splitlines()[-3000:]
    except OSError:
        return {"events": [], "state": None, "mtime": None, "model": None}
    model = None
    for raw in lines:
        try:
            d = json.loads(raw)
        except ValueError:
            continue
        typ, p, ts = d.get("type"), d.get("payload") or {}, d.get("timestamp")
        pt = p.get("type")
        if typ == "event_msg" and pt == "task_started":
            state = "working"
        elif typ == "event_msg" and pt == "task_complete":
            state = "idle"
        elif typ == "turn_context":
            model = p.get("model") or model
        elif typ == "response_item" and pt == "message":
            text = _text(p.get("content")).strip()
            if not text or text.startswith("<"):
                continue  # injected environment/instructions
            events.append({"k": "you" if p.get("role") == "user" else "text", "text": text, "ts": _ts(ts)})
        elif typ == "response_item" and pt in ("function_call", "custom_tool_call", "local_shell_call"):
            name = p.get("name") or "shell"
            arg = p.get("arguments") or p.get("input") or p.get("action") or ""
            if isinstance(arg, str):
                try:
                    arg = json.loads(arg)
                except ValueError:
                    pass
            if isinstance(arg, dict):
                cmd = arg.get("command") or arg.get("cmd")
                arg = " ".join(cmd) if isinstance(cmd, list) else (cmd or json.dumps(arg))
            events.append({"k": "tool", "tool": name, "text": core.one_line(arg, 300), "ts": _ts(ts)})
    return {"events": events[-limit:], "state": state, "mtime": os.path.getmtime(path), "model": model}
