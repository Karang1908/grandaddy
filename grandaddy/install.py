"""Register / remove the hook in ~/.claude/settings.json."""
import json, os, shutil
from . import core

SETTINGS = os.path.join(core.HOME, ".claude", "settings.json")
HOOK_CMD = f'"{core.BIN}" hook'
TIMEOUT = 3600  # the hook may hold a dialog open for the hub this long
# earlier names of this tool, whose hook entries an install replaces
LEGACY = ('/auto-yes" hook', '/bin/granddaddy" hook')


def _load():
    try:
        with open(SETTINGS) as f:
            return json.load(f)
    except FileNotFoundError:
        return {}


def _save(d):
    if os.path.exists(SETTINGS):
        shutil.copy2(SETTINGS, SETTINGS + ".grandaddy.bak")
    tmp = SETTINGS + ".grandaddy.tmp"
    with open(tmp, "w") as f:
        json.dump(d, f, indent=2)
        f.write("\n")
    os.replace(tmp, SETTINGS)


def _ours(group):
    return any(h.get("command") == HOOK_CMD or str(h.get("command", "")).endswith(LEGACY)
               for h in group.get("hooks", []))


def _current(group):
    return any(h.get("command") == HOOK_CMD and h.get("timeout") == TIMEOUT for h in group.get("hooks", []))


def installed():
    return any(_current(g) for g in _load().get("hooks", {}).get("PermissionRequest", []))


def install():
    """Returns "installed", "updated" (replaced an older entry) or "already"."""
    d = _load()
    groups = d.setdefault("hooks", {}).setdefault("PermissionRequest", [])
    if any(_current(g) for g in groups) and sum(_ours(g) for g in groups) == 1:
        return "already"
    had_old = any(_ours(g) for g in groups)
    groups[:] = [g for g in groups if not _ours(g)]
    groups.append({"hooks": [{"type": "command", "command": HOOK_CMD, "timeout": TIMEOUT}]})
    _save(d)
    return "updated" if had_old else "installed"


def uninstall():
    d = _load()
    groups = d.get("hooks", {}).get("PermissionRequest", [])
    if not any(_ours(g) for g in groups):
        return False
    rest = [g for g in groups if not _ours(g)]
    if rest:
        d["hooks"]["PermissionRequest"] = rest
    else:
        del d["hooks"]["PermissionRequest"]
        if not d["hooks"]:
            del d["hooks"]
    _save(d)
    shutil.rmtree(core.AUTO, ignore_errors=True)
    return True
