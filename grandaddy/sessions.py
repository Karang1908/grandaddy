"""One list of sessions across tools, in a single shape the hub and CLI both use."""
import os, subprocess, threading, time
from . import claude, core, others


def collect():
    """Every live session: Claude Code first (full detail), then other CLIs."""
    auto, reqs = core.auto_ids(), claude.pending()
    out = []
    for s in claude.live():
        sid = s["sessionId"]
        mine = [r for r in reqs if r["sid"] == sid]
        raw = s.get("status") or s.get("state")
        bg = s.get("kind") == "background"
        out.append({
            "key": "claude:" + sid, "tool": "claude", "pid": s.get("pid"), "sid": sid, "short": s.get("id"),
            "name": s.get("name") or sid[:8], "cwd": s.get("cwd", ""), "kind": s.get("kind", "interactive"),
            "bg": bg, "raw": raw,
            "status": "needs you" if mine else claude.STATUS.get(raw, raw or "?"),
            "since": (s.get("statusUpdatedAt") or s.get("startedAt") or 0) / 1000,
            "started": (s.get("startedAt") or 0) / 1000,
            "auto": sid in auto or core.ALL in auto,
            "pending": mine, "transcript": claude.transcript(sid), "can_answer": True,
        })
    for p in others.processes():
        state, rollout = None, None
        if p["tool"] == "codex":
            rollout = others.codex_rollout(p["cwd"], p["started"])
            if rollout:
                state = others.codex_activity(rollout)["state"]
        out.append({
            "key": f"{p['tool']}:{p['pid']}", "tool": p["tool"], "pid": p["pid"], "sid": None, "short": None,
            "name": f"{p['tool']} · {os.path.basename(p['cwd'].rstrip('/')) or '?'}", "cwd": p["cwd"],
            "kind": "interactive", "bg": False, "raw": state, "status": state or "running",
            "since": p["started"], "started": p["started"], "auto": False, "pending": [],
            "transcript": rollout, "can_answer": False,
        })
    live = {s["sid"] for s in out if s["sid"]}
    for sid in auto - live - {core.ALL}:  # flags for sessions that have exited are just clutter
        core.set_auto(sid, False)
    return out


def activity(sess):
    if not sess.get("transcript"):
        return {"events": [], "model": None, "context": None, "mtime": None, "edited": {}}
    if sess["tool"] == "claude":
        return claude.activity(sess["transcript"])
    return {"context": None, "edited": {}, **others.codex_activity(sess["transcript"])}


def conflicts(sess, everyone, window=6 * 3600):
    """Files this session edited recently that another live session edited too: {path: [names]}."""
    now = time.time()
    mine = {f for f, ts in activity(sess)["edited"].items() if now - (ts or 0) < window}
    if not mine:
        return {}
    out = {}
    for o in everyone:
        if o is sess or o["tool"] != "claude":
            continue
        for f, ts in activity(o)["edited"].items():
            if f in mine and now - (ts or 0) < window:
                out.setdefault(f, []).append(o["name"])
    return out


def resolve(target, sess, claude_only=False):
    """pid | short id | name | session-id prefix | folder; no target = sessions in this folder."""
    pool = [s for s in sess if s["tool"] == "claude"] if claude_only else sess
    if target is None:
        here = os.path.realpath(os.getcwd())
        return [s for s in pool if s["cwd"] and os.path.realpath(s["cwd"]) == here]
    t = target.lower()
    exact = [s for s in pool if t in (str(s["pid"]), (s["short"] or "").lower(), (s["sid"] or "").lower(), s["name"].lower())]
    if exact:
        return exact
    path = os.path.realpath(os.path.expanduser(target))
    return [s for s in pool
            if (s["sid"] or "").lower().startswith(t) or t in s["name"].lower()
            or os.path.basename(s["cwd"].rstrip("/")).lower() == t
            or (s["cwd"] and os.path.realpath(s["cwd"]) == path)]


# ---------- git context, fetched off the UI thread ----------

_git, _busy, _lock = {}, set(), threading.Lock()


def _fetch_git(cwd):
    info = None
    try:
        r = subprocess.run(["git", "-C", cwd, "status", "--porcelain=v1", "-b", "-uno"],
                           capture_output=True, text=True, timeout=3)
        if r.returncode == 0:
            lines = r.stdout.splitlines()
            head = lines[0][3:] if lines and lines[0].startswith("## ") else "?"
            branch = head.split("...")[0].replace("No commits yet on ", "")
            ahead = head.split("[", 1)[1].rstrip("]") if "[" in head else ""
            info = {"branch": branch, "changed": len(lines) - 1, "ahead": ahead}
    except (OSError, subprocess.TimeoutExpired):
        info = {"branch": None, "changed": None, "ahead": "", "slow": True}
    with _lock:
        _git[cwd] = (time.time(), info)
        _busy.discard(cwd)


def git(cwd):
    """Cached git summary for a folder (None until the first lookup lands, or not a repo)."""
    if not cwd:
        return None
    with _lock:
        hit = _git.get(cwd)
        if (not hit or time.time() - hit[0] > 15) and cwd not in _busy:
            _busy.add(cwd)
            threading.Thread(target=_fetch_git, args=(cwd,), daemon=True).start()
    return hit[1] if hit else None
