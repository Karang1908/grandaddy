"""Start, resume, open and jump to sessions. All starting goes through Claude Code's own
`claude --bg` (background sessions), so new sessions show up in the hub like any other."""
import os, shlex, subprocess
from . import core

# terminal apps we can find above a session's process, by process name -> app name for `open -a`
APPS = {"ghostty": "Ghostty", "iTerm2": "iTerm", "Terminal": "Terminal", "Code Helper": "Visual Studio Code",
        "Electron": "Visual Studio Code", "Cursor Helper": "Cursor", "WezTerm": "WezTerm", "wezterm-gui": "WezTerm",
        "kitty": "kitty", "alacritty": "Alacritty", "Warp": "Warp", "stable": "Warp"}


def _claude(args, cwd):
    try:
        r = subprocess.run(["claude", *args], cwd=cwd, capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as e:
        return False, str(e)
    out = (r.stdout + r.stderr).strip()
    return r.returncode == 0, out


def new(cwd, prompt, worktree=None, model=None):
    """Start a background session. Returns (ok, Claude's output, which names the session id)."""
    if not os.path.isdir(cwd):
        return False, f"no such folder: {cwd}"
    args = ["--bg"]
    if worktree is not None:
        args += ["--worktree"] + ([worktree] if worktree else [])
    if model:
        args += ["--model", model]
    return _claude(args + [prompt], cwd)


def resume(sid, cwd, prompt=None):
    """Continue a session in the background (Claude starts a copy if it's still running)."""
    return _claude(["--bg", "--resume", sid] + ([prompt] if prompt else []), cwd if cwd and os.path.isdir(cwd) else None)


def terminal_app():
    tp = os.environ.get("TERM_PROGRAM", "")
    return {"ghostty": "Ghostty", "iTerm.app": "iTerm", "Apple_Terminal": "Terminal"}.get(tp, "Terminal")


def _osa(script):
    r = subprocess.run(["osascript", "-e", script], capture_output=True, text=True, timeout=10)
    return r.returncode == 0, (r.stdout + r.stderr).strip()


def _q(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def open_terminal(cmd, cwd):
    """Run `cmd` in a new window of your terminal (the one the hub runs in)."""
    app = terminal_app()
    line = f"cd {shlex.quote(cwd or core.HOME)} && {cmd}"
    try:
        if app == "Ghostty":
            # Ghostty ignores --working-directory together with -e, hence the cd in the command
            r = subprocess.run(["open", "-na", "Ghostty.app", "--args", "-e", "/bin/zsh", "-lc", line],
                               capture_output=True, text=True, timeout=10)
            return r.returncode == 0, f"opened in a new Ghostty window: {cmd}"
        if app == "iTerm":
            ok, out = _osa(f'tell application "iTerm" to create window with default profile command {_q("/bin/zsh -lc " + shlex.quote(line))}')
            return ok, out or f"opened in a new iTerm window: {cmd}"
        ok, out = _osa(f'tell application "Terminal"\n do script {_q(line)}\n activate\nend tell')
        return ok, out or f"opened in a new Terminal window: {cmd}"
    except (OSError, subprocess.TimeoutExpired) as e:
        return False, str(e)


def _ps(pid, fields):
    try:
        return subprocess.run(["ps", "-o", fields, "-p", str(pid)], capture_output=True, text=True, timeout=3).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return ""


def owner_app(pid):
    """The terminal app a session runs in, found by walking up its parent processes."""
    seen = 0
    while pid and pid > 1 and seen < 30:
        line = _ps(pid, "ppid=,comm=")
        if not line:
            return None
        ppid, _, comm = line.partition(" ")
        name = os.path.basename(comm.strip())
        for proc, app in APPS.items():
            if name == proc or name.startswith(proc):
                return app
        pid, seen = int(ppid), seen + 1
    return None


def jump(pid):
    """Bring the session's terminal to the front; select its exact tab where the app allows it."""
    app = owner_app(pid)
    if not app:
        return False, "couldn't tell which app this session runs in"
    tty = _ps(pid, "tty=")
    dev = f"/dev/{tty}" if tty and tty not in ("??", "-") else None
    if app == "Terminal" and dev:
        ok, _ = _osa(f'tell application "Terminal"\n activate\n repeat with w in windows\n repeat with t in tabs of w\n'
                     f' if tty of t is {_q(dev)} then\n set selected of t to true\n set index of w to 1\n end if\n'
                     f' end repeat\n end repeat\nend tell')
        return ok, f"switched to its Terminal tab ({tty})"
    if app == "iTerm" and dev:
        ok, _ = _osa(f'tell application "iTerm"\n activate\n repeat with w in windows\n repeat with t in tabs of w\n'
                     f' repeat with s in sessions of t\n if tty of s is {_q(dev)} then\n select t\n select w\n end if\n'
                     f' end repeat\n end repeat\n end repeat\nend tell')
        return ok, f"switched to its iTerm tab ({tty})"
    r = subprocess.run(["open", "-a", app], capture_output=True, text=True, timeout=10)
    return r.returncode == 0, f"brought {app} to the front — it's the tab on {tty or 'its terminal'}"
