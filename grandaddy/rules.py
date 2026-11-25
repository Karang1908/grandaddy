"""auto-yes rules: finer than all-or-nothing. One rule per line in ~/.grandaddy/rules:

    yes  Bash(npm test*)      approve automatically, in every session
    ask  Bash(git push*)      always ask you, even in an auto-yes session
    no   Bash(rm -rf /*)      refuse automatically

The pattern is Claude Code's own `Tool(argument)` shape; the argument is a glob (* and ?)
matched against the command, file path or URL. `Tool` alone matches every call to it.
The first rule that matches wins. Lines starting with # are comments.
"""
import fnmatch, os, re
from . import claude, core

PATH = os.path.join(core.STATE, "rules")
VERBS = ("yes", "ask", "no")
_LINE = re.compile(r"^(yes|ask|no)\s+([A-Za-z0-9_*?.:-]+)(?:\((.*)\))?\s*$")


def parse(line):
    m = _LINE.match(line.strip())
    if not m:
        return None
    return {"verb": m.group(1), "tool": m.group(2), "arg": m.group(3), "text": line.strip()}


def load():
    try:
        with open(PATH) as f:
            lines = f.read().splitlines()
    except FileNotFoundError:
        return []
    return [r for r in (parse(l) for l in lines if l.strip() and not l.lstrip().startswith("#")) if r]


def match(tool, inp):
    """First matching rule for this call, or None."""
    for r in load():
        if not fnmatch.fnmatchcase(tool, r["tool"]):
            continue
        if r["arg"] is None:
            return r
        subj = claude.subject(tool, inp)
        full = os.path.expanduser(subj) if subj.startswith("~") else subj
        if fnmatch.fnmatchcase(subj, r["arg"]) or fnmatch.fnmatchcase(full, r["arg"]):
            return r
    return None


def add(line):
    r = parse(line)
    if not r:
        raise ValueError("a rule looks like:  yes Bash(npm test*)  |  ask Bash(git push*)  |  no Bash(rm -rf /*)")
    os.makedirs(core.STATE, exist_ok=True)
    with open(PATH, "a") as f:
        f.write(r["text"] + "\n")
    return r


def remove(n):
    """Remove the n-th rule (1-based, as `list` numbers them)."""
    with open(PATH) as f:
        lines = f.read().splitlines()
    idx = [i for i, l in enumerate(lines) if l.strip() and not l.lstrip().startswith("#") and parse(l)]
    if not 1 <= n <= len(idx):
        raise IndexError(f"there are {len(idx)} rules")
    gone = lines.pop(idx[n - 1])
    with open(PATH, "w") as f:
        f.write("\n".join(lines) + ("\n" if lines else ""))
    return gone
