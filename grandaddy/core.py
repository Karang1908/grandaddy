"""Shared paths, state and small helpers."""
import json, os, time

HOME = os.path.expanduser("~")
STATE = os.path.join(HOME, ".grandaddy")
OLD_STATE = os.path.join(HOME, ".granddaddy")  # before the rename
AUTO = os.path.join(STATE, "auto")          # <session_id> or ALL -> auto-approve on
PENDING = os.path.join(STATE, "pending")    # requests a hook is holding open for the hub
ANSWERS = os.path.join(STATE, "answers")    # the hub's replies, picked up by that hook
LOG = os.path.join(STATE, "activity.log")
ALL = "ALL"
ROOT = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
BIN = os.path.join(ROOT, "bin", "grandaddy")


def migrate_state():
    """One-time merge of ~/.granddaddy (the old spelling) into ~/.grandaddy."""
    if not os.path.isdir(OLD_STATE):
        return
    os.makedirs(STATE, exist_ok=True)
    for name in os.listdir(OLD_STATE):
        src, dst = os.path.join(OLD_STATE, name), os.path.join(STATE, name)
        if os.path.isdir(src) and os.path.isdir(dst):
            for f in os.listdir(src):
                if not os.path.exists(os.path.join(dst, f)):
                    os.rename(os.path.join(src, f), os.path.join(dst, f))
        elif name == "activity.log" and os.path.exists(dst):
            with open(src) as a, open(dst) as b:
                merged = a.read() + b.read()
            with open(dst, "w") as out:
                out.write(merged)
        elif not os.path.exists(dst):
            os.rename(src, dst)
    import shutil
    shutil.rmtree(OLD_STATE, ignore_errors=True)


def read_json(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def write_json(path, data):
    """Atomic write, so a reader never sees half a file."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = f"{path}.{os.getpid()}.tmp"
    with open(tmp, "w") as f:
        json.dump(data, f)
    os.replace(tmp, path)


def remove(path):
    try:
        os.remove(path)
    except FileNotFoundError:
        pass


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def auto_ids():
    try:
        return set(os.listdir(AUTO))
    except FileNotFoundError:
        return set()


def set_auto(sid, on):
    if on:
        os.makedirs(AUTO, exist_ok=True)
        open(os.path.join(AUTO, sid), "w").close()
    else:
        remove(os.path.join(AUTO, sid))


def is_auto(sid):
    return os.path.exists(os.path.join(AUTO, sid)) or os.path.exists(os.path.join(AUTO, ALL))


def log(line):
    try:
        os.makedirs(STATE, exist_ok=True)
        with open(LOG, "a") as f:
            f.write(f"{time.strftime('%Y-%m-%d %H:%M:%S')}  {' '.join(line.split())[:300]}\n")
    except OSError:
        pass  # logging must never break a hook or the hub


def log_tail(n):
    try:
        with open(LOG) as f:
            return f.read().splitlines()[-n:]
    except FileNotFoundError:
        return []


def short(path):
    return path.replace(HOME, "~", 1) if path.startswith(HOME) else path


def ago(seconds):
    s = max(0, int(seconds))
    if s < 60:
        return f"{s}s"
    if s < 3600:
        return f"{s // 60}m"
    if s < 86400:
        return f"{s // 3600}h"
    return f"{s // 86400}d"


def one_line(text, n=400):
    return " ".join(str(text).split())[:n]
