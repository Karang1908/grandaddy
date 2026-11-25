"""Claude Code: live sessions, what they're doing, token usage, and past sessions."""
import datetime, glob, json, os, subprocess, threading, time
from . import core

CLAUDE_DIR = os.path.join(core.HOME, ".claude")
SESSIONS = os.path.join(CLAUDE_DIR, "sessions")
PROJECTS = os.path.join(CLAUDE_DIR, "projects")

# Claude Code's per-session status (interactive) and state (background) -> ours
STATUS = {"busy": "working", "shell": "working", "waiting": "needs you", "idle": "idle",
          "working": "working", "running": "working", "done": "idle", "stopped": "idle",
          # background sessions that can't run (login required, network blocked, API error)
          "blocked": "blocked", "failed": "failed"}
EDIT_TOOLS = ("Edit", "Write", "MultiEdit", "NotebookEdit")


# ---------- live sessions ----------

def _from_files():
    out = []
    try:
        names = os.listdir(SESSIONS)
    except FileNotFoundError:
        return out
    for fn in names:
        if fn.endswith(".json"):
            s = core.read_json(os.path.join(SESSIONS, fn))
            if s and s.get("sessionId") and isinstance(s.get("pid"), int) and core.alive(s["pid"]):
                out.append(s)
    return out


_agents = {"t": 0, "data": None}


def live():
    """Running sessions, oldest first. `claude agents --json` is the official list and also has
    background sessions; the session files are the fallback (and add statusUpdatedAt)."""
    files = {s["sessionId"]: s for s in _from_files()}
    if time.time() - _agents["t"] > 1.5:
        _agents["t"] = time.time()
        try:
            r = subprocess.run(["claude", "agents", "--json"], capture_output=True, text=True, timeout=5)
            _agents["data"] = json.loads(r.stdout) if r.returncode == 0 else None
        except (OSError, ValueError, subprocess.TimeoutExpired):
            _agents["data"] = None
    out = []
    for a in _agents["data"] or []:
        sid = a.get("sessionId")
        if not sid:
            continue
        pid = a.get("pid")
        if isinstance(pid, int) and not core.alive(pid):
            continue
        out.append({**files.pop(sid, {}), **a})
    out += files.values()  # anything the agents list didn't have
    return sorted(out, key=lambda s: s.get("startedAt", 0))


def raw_status(sid):
    """Fast status lookup for the hook: session files only, no subprocess."""
    for s in _from_files():
        if s["sessionId"] == sid:
            return s.get("status")
    return None


# ---------- transcripts ----------

_paths = {}


def transcript(sid):
    p = _paths.get(sid)
    if p and os.path.exists(p):
        return p
    hits = glob.glob(os.path.join(PROJECTS, "*", f"{sid}.jsonl"))
    if hits:
        _paths[sid] = p = max(hits, key=os.path.getmtime)
        return p
    return None


def _tail_lines(path, nbytes=800_000):
    with open(path, "rb") as f:
        f.seek(0, 2)
        size = f.tell()
        f.seek(max(0, size - nbytes))
        data = f.read()
    lines = data.split(b"\n")
    return lines[1:] if size > nbytes else lines


def subject(name, inp):
    """The one argument worth showing for a tool call."""
    inp = inp if isinstance(inp, dict) else {}
    for k in ("command", "file_path", "notebook_path", "pattern", "url", "query",
              "description", "skill", "prompt", "path"):
        v = inp.get(k)
        if isinstance(v, str) and v.strip():
            return core.short(v) if k in ("file_path", "notebook_path", "path") else core.one_line(v, 300)
    if name == "AskUserQuestion":
        qs = inp.get("questions") or []
        return core.one_line(qs[0].get("question", ""), 300) if qs else ""
    return ""


def tool_summary(name, inp):
    s = subject(name, inp)
    return f"{name}  {s}" if s else name


def _user_text(text):
    text = text.strip()
    if text.startswith("<command-name>"):
        return text.split("<command-name>", 1)[1].split("</command-name>", 1)[0].strip()
    if text.startswith("<") or not text:
        return None  # system-injected context, not something you typed
    return text


def parse_ts(ts):
    try:
        return datetime.datetime.fromisoformat(str(ts).replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


_parsed = {}


def activity(path, limit=150):
    """Recent activity from the transcript tail.
    events: dicts {k: you|text|tool|error|mark, text, tool?, ts}; plus model, context, edited files."""
    empty = {"events": [], "model": None, "context": None, "mtime": None, "edited": {}}
    try:
        st = os.stat(path)
    except (OSError, TypeError):
        return empty
    hit = _parsed.get(path)
    if hit and hit[0] == (st.st_mtime, st.st_size):
        return hit[1]
    events, model, context, edited = [], None, None, {}
    for raw in _tail_lines(path):
        try:
            d = json.loads(raw)
        except ValueError:
            continue
        if d.get("isSidechain"):
            continue
        t, msg, ts = d.get("type"), d.get("message") or {}, parse_ts(d.get("timestamp"))
        content = msg.get("content")
        if d.get("isCompactSummary"):
            events.append({"k": "mark", "text": "conversation compacted", "ts": ts})
        elif d.get("isMeta"):
            continue
        elif t == "user":
            items = [{"type": "text", "text": content}] if isinstance(content, str) else content or []
            for c in items:
                if not isinstance(c, dict):
                    continue
                if c.get("type") == "text":
                    u = _user_text(c.get("text", ""))
                    if u:
                        events.append({"k": "you", "text": u, "ts": ts})
                elif c.get("type") == "tool_result" and c.get("is_error"):
                    body = c.get("content")
                    if isinstance(body, list):
                        body = " ".join(x.get("text", "") for x in body if isinstance(x, dict))
                    events.append({"k": "error", "text": core.one_line(body or "error", 300), "ts": ts})
        elif t == "assistant" and isinstance(content, list):
            model = msg.get("model") or model
            u = msg.get("usage") or {}
            if u:
                context = sum(u.get(k) or 0 for k in
                              ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"))
            for c in content:
                if not isinstance(c, dict):
                    continue
                if c.get("type") == "text" and c.get("text", "").strip():
                    events.append({"k": "text", "text": c["text"].strip(), "ts": ts})
                elif c.get("type") == "tool_use":
                    name, inp = c.get("name", "?"), c.get("input") or {}
                    desc = inp.get("description") if name == "Bash" and isinstance(inp.get("description"), str) else None
                    events.append({"k": "tool", "tool": name, "text": subject(name, inp), "desc": desc, "ts": ts})
                    if name in EDIT_TOOLS:
                        fp = inp.get("file_path") or inp.get("notebook_path")
                        if fp:
                            edited[fp] = ts or 0
    res = {"events": events[-limit:], "model": model, "context": context, "mtime": st.st_mtime, "edited": edited}
    _parsed[path] = ((st.st_mtime, st.st_size), res)
    return res


def signals(act, status, now=None):
    """Hints that a session might need a look: gone quiet while working, or repeating itself."""
    now = now or time.time()
    out = []
    if status == "working" and act.get("mtime") and now - act["mtime"] > 300:
        out.append(f"quiet {core.ago(now - act['mtime'])}")
    tools = [(e["tool"], e["text"]) for e in act.get("events", []) if e["k"] == "tool"][-4:]
    if len(tools) == 4 and len(set(tools)) == 1:
        out.append("looping?")
    return out


# ---------- token usage (incremental, cached on disk) ----------

USAGE_CACHE = os.path.join(core.STATE, "usage-cache.json")
_usage_lock = threading.Lock()


def _day(ts):
    return datetime.date.fromtimestamp(ts).isoformat() if ts else "?"


def usage(days=7):
    """Tokens per day per project for transcripts touched in the last `days` days.
    Returns {day: {project: {"in": n, "out": n, "cache": n}}}. Reads only bytes added since last time."""
    with _usage_lock:
        cache = core.read_json(USAGE_CACHE) or {}
        cutoff = time.time() - days * 86400
        out = {}
        for path in glob.glob(os.path.join(PROJECTS, "*", "*.jsonl")):
            try:
                st = os.stat(path)
            except OSError:
                continue
            if st.st_mtime < cutoff:
                continue
            ent = cache.get(path)
            if not ent or ent["offset"] > st.st_size:
                ent = {"offset": 0, "days": {}, "project": None}
            if ent["offset"] < st.st_size:
                with open(path, "rb") as f:
                    f.seek(ent["offset"])
                    data = f.read()
                end = data.rfind(b"\n") + 1  # leave a half-written last line for next time
                for raw in data[:end].split(b"\n"):
                    if b'"usage"' not in raw:
                        if ent["project"] is None and b'"cwd"' in raw:
                            try:
                                ent["project"] = json.loads(raw).get("cwd")
                            except ValueError:
                                pass
                        continue
                    try:
                        d = json.loads(raw)
                    except ValueError:
                        continue
                    ent["project"] = ent["project"] or d.get("cwd")
                    u = (d.get("message") or {}).get("usage") or {}
                    if d.get("type") != "assistant" or not u:
                        continue
                    day = ent["days"].setdefault(_day(parse_ts(d.get("timestamp"))), [0, 0, 0])
                    day[0] += u.get("input_tokens") or 0
                    day[1] += u.get("output_tokens") or 0
                    day[2] += (u.get("cache_read_input_tokens") or 0) + (u.get("cache_creation_input_tokens") or 0)
                ent["offset"] += end
                cache[path] = ent
            proj = ent["project"] or "?"
            for day, (i, o, c) in ent["days"].items():
                p = out.setdefault(day, {}).setdefault(proj, {"in": 0, "out": 0, "cache": 0})
                p["in"] += i; p["out"] += o; p["cache"] += c
        core.write_json(USAGE_CACHE, cache)
        return out


def session_tokens(path):
    """Total tokens for one transcript (uses the same cache)."""
    if not path:
        return None
    cache = core.read_json(USAGE_CACHE) or {}
    ent = cache.get(path)
    if not ent:
        return None
    i = sum(v[0] for v in ent["days"].values())
    o = sum(v[1] for v in ent["days"].values())
    c = sum(v[2] for v in ent["days"].values())
    return {"in": i, "out": o, "cache": c}


# ---------- past sessions ----------

HISTORY_CACHE = os.path.join(core.STATE, "history-cache.json")


def _first_prompt(path):
    """cwd, first thing you typed, and how the session was started, from the head of a transcript."""
    cwd, prompt, entry, command = None, None, None, None
    try:
        f = open(path, "rb")
    except OSError:
        return cwd, prompt, entry
    budget = 2_000_000  # session-start context can push the first prompt well past the head
    for raw in f:
        budget -= len(raw)
        if budget < 0:
            break
        if entry not in (None, "cli") or (b'"user"' not in raw and cwd and entry):
            if entry not in (None, "cli"):
                break  # started by a tool: hidden from history anyway, don't read further
            continue
        try:
            d = json.loads(raw)
        except ValueError:
            continue
        cwd = cwd or d.get("cwd")
        entry = entry or d.get("entrypoint")
        if prompt is None and d.get("type") == "user" and not d.get("isMeta") and not d.get("isCompactSummary"):
            c = (d.get("message") or {}).get("content")
            texts = [c] if isinstance(c, str) else [x.get("text", "") for x in c or [] if isinstance(x, dict) and x.get("type") == "text"]
            for t in texts:
                u = _user_text(t or "")
                if u and u.startswith("/"):
                    command = command or u  # /resume, /model…: only if nothing better follows
                elif u:
                    prompt = core.one_line(u, 300)
                    break
        if cwd and prompt and entry:
            break
    prompt = prompt or command
    f.close()
    return cwd, prompt, entry


def history():
    """Past sessions you started yourself, newest first: {sid, path, cwd, prompt, mtime, entry}.
    Sessions started by tools (entrypoint sdk-*) are left out. Cached by size+mtime."""
    cache = core.read_json(HISTORY_CACHE) or {}
    out, changed = [], False
    for path in glob.glob(os.path.join(PROJECTS, "*", "*.jsonl")):
        try:
            st = os.stat(path)
        except OSError:
            continue
        ent = cache.get(path)
        if not ent or ent["size"] != st.st_size or ent["mtime"] != st.st_mtime:
            cwd, prompt, entry = _first_prompt(path)
            ent = {"size": st.st_size, "mtime": st.st_mtime, "cwd": cwd, "prompt": prompt, "entry": entry}
            cache[path] = ent
            changed = True
        if not ent["prompt"]:
            continue
        if ent["entry"] not in (None, "cli"):
            continue
        out.append({"sid": os.path.basename(path)[:-6], "path": path, **ent})
    if changed:
        core.write_json(HISTORY_CACHE, cache)
    return sorted(out, key=lambda h: h["mtime"], reverse=True)


# ---------- requests held open by our hook ----------

def pending():
    """Open requests, oldest first; drops ones whose hook process has gone."""
    out = []
    try:
        names = os.listdir(core.PENDING)
    except FileNotFoundError:
        return out
    for fn in names:
        if not fn.endswith(".json"):
            continue
        p = core.read_json(os.path.join(core.PENDING, fn))
        if not p:
            continue
        if not core.alive(p.get("hook_pid", 0)):
            core.remove(os.path.join(core.PENDING, fn))
            continue
        out.append(p)
    return sorted(out, key=lambda p: p.get("created", 0))


def answer(req, decision):
    """decision: {"kind": "allow"} | {"kind": "deny"} | {"kind": "answers", "answers": {question: text}}"""
    core.write_json(os.path.join(core.ANSWERS, req["id"] + ".json"), decision)


def describe_request(req):
    if req.get("tool") == "AskUserQuestion":
        qs = (req.get("input") or {}).get("questions") or []
        return "asks: " + (core.one_line(qs[0].get("question", ""), 200) if qs else "a question")
    return "wants: " + tool_summary(req.get("tool", "?"), req.get("input"))
