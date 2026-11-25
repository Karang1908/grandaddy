"""The live hub: every session, what it's doing, and answering whatever it's waiting on."""
import curses, os, subprocess, threading, time
from . import brand, claude, core, install, launch, phone, rules, sessions

DOT = "●"
FILE_TOOLS = ("Read", "Edit", "Write", "MultiEdit", "NotebookEdit", "Grep", "Glob")


def cursor(visible):
    try:
        curses.curs_set(1 if visible else 0)
    except curses.error:
        pass  # terminals like vt100 can't hide the cursor


def notify(title, body):
    q = lambda s: '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'
    try:
        subprocess.Popen(["osascript", "-e", f"display notification {q(body)} with title {q(brand.NAME)} subtitle {q(title)}"],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except OSError:
        pass


def wrap(text, width, max_lines):
    """Word-wrap keeping paragraph breaks; hard-break words longer than the width."""
    width = max(4, width)
    lines = []
    for para in str(text).splitlines() or [""]:
        cur = ""
        for wd in para.split():
            while len(wd) > width:
                if cur:
                    lines.append(cur); cur = ""
                lines.append(wd[:width]); wd = wd[width:]
            if cur and len(cur) + 1 + len(wd) > width:
                lines.append(cur); cur = wd
            else:
                cur = f"{cur} {wd}" if cur else wd
        if cur:
            lines.append(cur)
    lines = [l for l in lines if l.strip()] or [""]
    if len(lines) > max_lines:
        lines = lines[:max_lines]
        lines[-1] = lines[-1][:width - 1].rstrip() + "…"
    return lines


def clock(ts):
    if not ts:
        return "     "
    lt = time.localtime(ts)
    return time.strftime("%H:%M", lt) if time.strftime("%Y%m%d", lt) == time.strftime("%Y%m%d") else time.strftime("%d %b", lt)[:5]


def tokens(n):
    if n is None:
        return "?"
    return f"{n / 1e6:.1f}M" if n >= 1e6 else f"{n / 1e3:.0f}k" if n >= 1e3 else str(n)


class Hub:
    def __init__(self, scr):
        self.scr = scr
        self.view = "sessions"          # sessions | history | usage
        self.overlay = None             # None | "help"
        self.sel_key, self.sel_i = None, 0
        self.hist, self.hist_i, self.hist_q = [], 0, ""
        self.msg, self.msg_t, self.msg_kind = "", 0, "info"
        self.mute = os.environ.get("GRANDADDY_NOTIFY", "1") == "0"
        self.prev, self.drafts, self.first = {}, {}, True
        self.confirm = None             # (key, deadline) for two-press actions
        self.usage, self.usage_t, self.usage_busy = None, 0, False
        cursor(False)
        try:
            curses.set_escdelay(25)  # esc acts at once instead of after ncurses' 1s default
        except AttributeError:
            pass
        if curses.has_colors():
            curses.use_default_colors()
            for i, c in enumerate((curses.COLOR_GREEN, curses.COLOR_YELLOW, curses.COLOR_BLUE,
                                   curses.COLOR_MAGENTA, curses.COLOR_RED, curses.COLOR_CYAN), 1):
                curses.init_pair(i, c, -1)
            curses.init_pair(7, curses.COLOR_BLACK, curses.COLOR_YELLOW)
            cp = curses.color_pair
        else:  # monochrome terminal: bold/reverse only
            cp = lambda i: curses.A_REVERSE if i == 7 else curses.A_BOLD if i in (2, 5) else 0
        self.C = {"working": cp(1), "needs you": cp(2) | curses.A_BOLD, "idle": cp(3), "running": cp(4),
                  "blocked": cp(5), "failed": cp(5),
                  "red": cp(5), "green": cp(1), "yellow": cp(2), "cyan": cp(6), "hot": cp(7) | curses.A_BOLD,
                  "dim": curses.A_DIM, "bold": curses.A_BOLD}
        scr.timeout(1000)

    # ---------- drawing primitives ----------

    def put(self, y, x, text, attr=0, width=None):
        h, w = self.scr.getmaxyx()
        if not (0 <= y < h) or x >= w - 1 or x < 0:
            return 0
        room = w - x - 1 if width is None else min(width, w - x - 1)
        if room <= 0:
            return 0
        text = str(text)[:room]
        try:
            self.scr.addstr(y, x, text, attr)
        except curses.error:
            pass
        return len(text)

    def say(self, msg, kind="info"):
        self.msg, self.msg_t, self.msg_kind = msg, time.time(), kind

    def chips(self, y, x, items, width):
        """Key chips: [(key, label, hot)], dropped from the end when they don't fit."""
        used = 0
        for key, label, hot in items:
            need = len(key) + 2 + 1 + len(label) + 2
            if used + need > width:
                break
            self.put(y, x + used, f" {key} ", self.C["hot"] if hot else curses.A_REVERSE)
            self.put(y, x + used + len(key) + 3, label, curses.A_BOLD if hot else 0)
            used += need
        return used

    # ---------- main loop ----------

    def run(self):
        while True:
            sess = sessions.collect() if self.view == "sessions" else getattr(self, "_last", [])
            self._last = sess
            self.track(sess)
            self.refresh_usage()
            self.draw(sess)
            k = self.scr.getch()
            if k in (-1, curses.KEY_RESIZE):
                continue
            k = self.decode(k)
            if self.overlay:
                self.overlay = None
                continue
            if self.handle(k, sess) == "quit":
                return

    def decode(self, k):
        if k == 27:  # arrows ncurses didn't decode (ESC [ A / ESC O A); a bare ESC is "back"
            self.scr.nodelay(True)
            first = self.scr.getch()
            second = self.scr.getch() if first in (ord("["), ord("O")) else -1
            self.scr.timeout(1000)
            if first in (ord("["), ord("O")):
                return {ord("A"): curses.KEY_UP, ord("B"): curses.KEY_DOWN}.get(second, -1)
            if first != -1:
                curses.ungetch(first)  # a key typed right after esc: keep it
            return "esc"
        return k

    def track(self, sess):
        """Notify on: started needing you, or finished a stretch of work."""
        now = time.time()
        for s in sess:
            prev = self.prev.get(s["key"])
            if prev and not self.first and not self.mute:
                if s["status"] == "needs you" and prev[0] != "needs you":
                    notify(s["name"], claude.describe_request(s["pending"][0]) if s["pending"] else "is waiting for you")
                elif s["status"] == "idle" and prev[0] == "working" and now - prev[1] >= 15:
                    notify(s["name"], f"finished after {core.ago(now - prev[1])}")
            since = prev[1] if prev and prev[0] == "working" and s["status"] == "working" else now
            self.prev[s["key"]] = (s["status"], since)
        self.first = False
        live = {r["id"] for s in sess for r in s["pending"]}
        for rid in list(self.drafts):
            if rid not in live:
                del self.drafts[rid]

    def refresh_usage(self):
        if self.usage_busy or time.time() - self.usage_t < 60:
            return
        self.usage_busy = True

        def work():
            try:
                self.usage = claude.usage(7)
            finally:
                self.usage_t, self.usage_busy = time.time(), False
        threading.Thread(target=work, daemon=True).start()

    def selected(self, sess):
        if not sess:
            return None
        keys = [s["key"] for s in sess]
        if self.sel_key in keys:
            self.sel_i = keys.index(self.sel_key)
        self.sel_i = max(0, min(self.sel_i, len(sess) - 1))
        self.sel_key = keys[self.sel_i]
        return sess[self.sel_i]

    # ---------- layout ----------

    def draw(self, sess):
        state = (self.view, self.sel_key, self.overlay, self.hist_i, self.hist_q)
        if state != getattr(self, "_drawn", None):
            # the picture changed wholesale: repaint every line instead of letting ncurses
            # reuse old lines by scrolling, which some terminals get wrong
            self.scr.redrawwin()
            self._drawn = state
        self.scr.erase()
        h, w = self.scr.getmaxyx()
        y = self.header(sess, h, w)
        bottom = h - 3
        if self.view == "history":
            self.history_view(y, bottom, w)
        elif self.view == "usage":
            self.usage_view(y, bottom, w)
        else:
            cur = self.selected(sess)
            if w >= 100:
                lw = min(56, max(40, w * 2 // 5))
                self.session_list(sess, y, 0, lw, bottom - y + 1)
                for yy in range(y, bottom + 1):
                    self.put(yy, lw, "│", curses.A_DIM)
                self.detail(cur, sess, y, lw + 2, w - lw - 3, bottom + 1)
            else:
                rows = min(max(len(sess), 1) + 1, max(4, (bottom - y) // 3))
                self.session_list(sess, y, 0, w, rows)
                self.put(y + rows, 0, "─" * w, curses.A_DIM)
                self.detail(cur, sess, y + rows + 1, 1, w - 2, bottom + 1)
        self.footer(sess, h, w)
        if self.overlay == "help":
            self.help_overlay(h, w)
        self.scr.refresh()

    def header(self, sess, h, w):
        counts = {}
        for s in sess:
            counts[s["status"]] = counts.get(s["status"], 0) + 1
        warn = None if install.installed() else "hook not installed — press i"
        today = None
        if self.usage:
            d = self.usage.get(time.strftime("%Y-%m-%d"), {})
            today = sum(p["in"] + p["out"] + p["cache"] for p in d.values())
        tall = h >= 28 and w >= 72
        if tall:
            for i, line in enumerate(brand.FACE):
                self.put(i, 1, line)
            x = brand.FACE_WIDTH + 4
            self.put(1, x, brand.NAME, curses.A_BOLD)
            self.put(2, x, brand.TAGLINE, curses.A_DIM)
            cy, y = 4, len(brand.FACE)
        else:
            n = self.put(0, 1, brand.MINI, curses.A_BOLD)
            n += self.put(0, 3 + n, brand.NAME, curses.A_BOLD)
            x, cy, y = n + 6, 0, 1
        if self.view == "sessions":
            if not sess:
                x += self.put(cy, x, "no sessions running   ", curses.A_DIM)
            for st in ("needs you", "working", "idle", "running", "blocked", "failed"):
                if counts.get(st):
                    self.put(cy, x, DOT, self.C[st])
                    x += 1 + self.put(cy, x + 1, f" {counts[st]} {st}   ", self.C[st] if st == "needs you" else curses.A_DIM)
        else:
            x += self.put(cy, x, {"history": "past sessions", "usage": "token usage"}[self.view] + "   ", curses.A_BOLD)
        if today is not None:
            x += self.put(cy, x, f"today {tokens(today)} tokens   ", curses.A_DIM)
        if phone.config():
            x += self.put(cy, x, "phone on   ", curses.A_DIM)
        if self.mute:
            x += self.put(cy, x, "muted   ", curses.A_DIM)
        if warn:
            self.put(cy, x, warn, self.C["red"] | curses.A_BOLD)
        self.put(y, 0, "─" * w, curses.A_DIM)
        return y + 1

    def footer(self, sess, h, w):
        self.put(h - 2, 0, "─" * w, curses.A_DIM)
        if self.msg and time.time() - self.msg_t < 7:
            attr = self.C["red"] if self.msg_kind == "error" else self.C["yellow"]
            self.put(h - 2, 2, f" {self.msg} ", attr | curses.A_BOLD)
        self.chips(h - 1, 1, self.key_hints(sess), w - 2)

    def key_hints(self, sess):
        if self.view == "history":
            return [("o", "open", True), ("p", "continue with a prompt", True), ("/", "search", False),
                    ("↑↓", "move", False), ("esc", "back", False), ("?", "help", False)]
        if self.view == "usage":
            return [("esc", "back", False), ("?", "help", False), ("q", "quit", False)]
        s = self.selected(sess)
        items = []
        if s and s["pending"]:
            req = s["pending"][0]
            if req["tool"] == "AskUserQuestion":
                qs = (req["input"] or {}).get("questions") or []
                d = self.drafts.get(req["id"], {"qi": 0, "multi": set()})
                q = qs[min(d["qi"], len(qs) - 1)] if qs else {}
                n = len(q.get("options") or [])
                items += [(f"1-{n}" if n > 1 else "1", "toggle" if q.get("multiSelect") else "answer", True), ("t", "type", True)]
                if q.get("multiSelect"):
                    items.append(("enter", "send", True))
            else:
                items += [("y", "allow", True), ("n", "deny", True)]
        if any(o["status"] == "needs you" for o in sess):
            items.append(("tab", "next waiting", not (s and s["pending"])))
        if s:
            items.append(("o", "attach" if s["bg"] else "jump to it", False))
            if s["tool"] == "claude":
                items.append(("a", "auto-yes off" if s["auto"] else "auto-yes", False))
            if s["bg"]:
                items.append(("x", "stop", False))
        items += [("s", "start new", False), ("h", "history", False), ("u", "usage", False),
                  ("?", "help", False), ("q", "quit", False)]
        return items

    # ---------- sessions view ----------

    def session_list(self, sess, y, x, width, rows):
        self.put(y, x + 1, f"SESSIONS  {len(sess)}" if sess else "SESSIONS", curses.A_DIM | curses.A_BOLD)
        y, rows = y + 1, rows - 1
        if not sess:
            self.put(y + 1, x + 2, "Nothing running yet.", curses.A_DIM)
            self.put(y + 2, x + 2, "Press s to start one, or run claude / codex", curses.A_DIM)
            self.put(y + 3, x + 2, "/ gemini in any terminal.", curses.A_DIM)
            return
        start = max(0, self.sel_i - rows + 1)
        now = time.time()
        for i, s in enumerate(sess[start:start + rows], start):
            yy = y + i - start
            sel = i == self.sel_i
            st = s["status"]
            tail = f"{st:<9} {core.ago(now - s['since']):>4}"
            name_w = max(6, width - len(tail) - 12)
            self.put(yy, x, "›" if sel else " ", curses.A_BOLD)
            self.put(yy, x + 2, DOT, self.C.get(st, curses.A_DIM))
            self.put(yy, x + 4, s["name"][:name_w].ljust(name_w), (curses.A_BOLD | curses.A_REVERSE) if sel else 0)
            tx = x + 5 + name_w                      # fixed 5-column tag area: "bg ◆ "
            if s["bg"]:
                self.put(yy, tx, "bg", curses.A_DIM)
            if s["auto"]:
                self.put(yy, tx + 3, "◆", self.C["green"])
            self.put(yy, tx + 5, tail, self.C.get(st, curses.A_DIM) if st in ("needs you", "blocked", "failed") else curses.A_DIM)
        if len(sess) > rows:
            self.put(y + rows - 1, x + width - 9, f"+{len(sess) - rows} more", curses.A_DIM)

    def detail(self, s, sess, y, x, width, end):
        if not s or end - y < 3 or width < 24:
            return
        act = sessions.activity(s)
        now = time.time()
        self.put(y, x, s["name"], curses.A_BOLD)
        model = (act.get("model") or "").replace("claude-", "")
        ident = f"bg {s['short']}" if s["bg"] and s["short"] else f"pid {s['pid']}" if s["pid"] else ""
        self.put(y + 1, x, "  ·  ".join(p for p in (s["tool"], model, ident) if p), curses.A_DIM)
        g = sessions.git(s["cwd"])
        where = [core.short(s["cwd"]) or "?"]
        if g and g.get("branch"):
            where.append(g["branch"])
            if g["changed"]:
                where.append(f"{g['changed']} changed")
            if g["ahead"]:
                where.append(g["ahead"])
        self.put(y + 2, x, "  ·  ".join(where), curses.A_DIM)
        st = s["status"]
        bits = [f"{st} for {core.ago(now - s['since'])}"]
        if act.get("context"):
            bits.append(f"context {tokens(act['context'])}")
        tk = claude.session_tokens(s["transcript"]) if s["tool"] == "claude" else None
        if tk and tk["in"] + tk["out"] + tk["cache"]:
            bits.append(f"{tokens(tk['in'] + tk['out'] + tk['cache'])} tokens")
        if s["tool"] == "claude":
            bits.append("auto-yes on" if s["auto"] else "auto-yes off")
        self.put(y + 3, x, DOT, self.C.get(st, curses.A_DIM))
        self.put(y + 3, x + 2, "  ·  ".join(bits))
        yy = y + 4
        warns = claude.signals(act, st, now)
        twins = [o for o in sess if o is not s and o["cwd"] and o["cwd"] == s["cwd"]]
        if twins:
            warns.append("also running here: " + ", ".join(o["name"] for o in twins))
        if s["tool"] == "claude":
            clash = sessions.conflicts(s, sess)
            if clash:
                names = sorted({n for v in clash.values() for n in v})
                files = ", ".join(os.path.basename(f) for f in list(clash)[:3]) + (" …" if len(clash) > 3 else "")
                warns.append(f"{' & '.join(names)} also edited {files}")
        for wtxt in warns:
            for l in wrap("! " + wtxt, width, 2):
                self.put(yy, x, l, self.C["yellow"]); yy += 1
        yy += 1
        yy = self.needs_you(s, yy, x, width, end)
        self.feed(s, act, yy, x, width, end)

    def needs_you(self, s, y, x, width, end):
        bar = lambda yy: self.put(yy, x, "┃", self.C["yellow"])
        if s["pending"]:
            req = s["pending"][0]
            more = len(s["pending"]) - 1
            if req["tool"] == "AskUserQuestion":
                y = self.question(req, y, x, width, bar)
            else:
                bar(y); self.put(y, x + 2, "NEEDS YOU", self.C["hot"]); self.put(y, x + 12, "permission", self.C["yellow"])
                y += 1
                bar(y); self.put(y, x + 2, req["tool"], curses.A_BOLD)
                for l in wrap(claude.subject(req["tool"], req["input"]) or "(no arguments)", width - 4 - len(req["tool"]), 5):
                    bar(y); self.put(y, x + 3 + len(req["tool"]), l); y += 1
                bar(y); self.put(y, x + 2, "allow with y, deny with n — or answer in its terminal", curses.A_DIM); y += 1
            if more:
                bar(y); self.put(y, x + 2, f"+{more} more waiting after this", curses.A_DIM); y += 1
            return y + 1
        if s["status"] in ("blocked", "failed"):
            bar(y); self.put(y, x + 2, f"This background session can't run ({s['raw']}) — press o to attach and see why,"
                     " or x x to stop it", self.C["red"])
            return y + 2
        if s["status"] == "needs you":
            note = ("Waiting on you" + (" — press o to attach" if s["bg"] else " in its terminal — press o to jump there")
                    if s["can_answer"] else f"Waiting on you in its own terminal ({s['tool']}) — press o to jump there")
            bar(y); self.put(y, x + 2, note, self.C["yellow"])
            return y + 2
        return y

    def question(self, req, y, x, width, bar):
        qs = (req["input"] or {}).get("questions") or []
        d = self.drafts.setdefault(req["id"], {"qi": 0, "answers": {}, "multi": set()})
        if not qs or d["qi"] >= len(qs):
            bar(y); self.put(y, x + 2, "answer sent…", curses.A_DIM)
            return y + 1
        q = qs[d["qi"]]
        bar(y); self.put(y, x + 2, "NEEDS YOU", self.C["hot"])
        info = "question" + (f" {d['qi'] + 1} of {len(qs)}" if len(qs) > 1 else "") + (f"  ·  {q['header']}" if q.get("header") else "")
        self.put(y, x + 12, info, self.C["yellow"]); y += 1
        for l in wrap(q.get("question", ""), width - 3, 3):
            bar(y); self.put(y, x + 2, l, curses.A_BOLD); y += 1
        multi = q.get("multiSelect")
        opts = q.get("options") or []
        lw = min(28, max([len(o.get("label", "")) for o in opts] + [4]) + (4 if multi else 0))
        for i, o in enumerate(opts, 1):
            bar(y)
            self.put(y, x + 2, f" {i} ", curses.A_REVERSE)
            box = ("[x] " if i - 1 in d["multi"] else "[ ] ") if multi else ""
            self.put(y, x + 6, (box + o.get("label", ""))[:lw], curses.A_BOLD if i - 1 in d["multi"] else 0)
            if o.get("description"):
                self.put(y, x + 8 + lw, core.one_line(o["description"]), curses.A_DIM, width=width - lw - 9)
            y += 1
        bar(y); self.put(y, x + 2, " t ", curses.A_REVERSE); self.put(y, x + 6, "type your own answer", curses.A_DIM); y += 1
        hint = "numbers toggle, enter sends" if multi else "press a number to answer"
        bar(y); self.put(y, x + 2, hint + " — or answer in its terminal", curses.A_DIM)
        return y + 1

    def feed_lines(self, events, width, who):
        """The activity timeline as drawable lines: [(x, text, attr), ...] per line, oldest first."""
        tw = max(10, width - 8)
        out = []
        groups = []  # runs of the same file tool collapse into one line: "Read ×3  a.py, b.py"
        for e in events:
            if (e["k"] == "tool" and e["tool"] in FILE_TOOLS and groups and groups[-1]["k"] == "tool"
                    and groups[-1]["tool"] == e["tool"]):
                groups[-1]["items"].append(e["text"])
                continue
            groups.append({**e, "items": [e["text"]]} if e["k"] == "tool" else e)
        if groups and groups[0]["k"] != "you":
            out.append([(8, "…", curses.A_DIM)])
        for g in groups:
            k = g["k"]
            if k == "you":
                if out:
                    out.append([])
                for i, l in enumerate(wrap(g["text"], tw - 2, 3)):
                    out.append([(0, clock(g.get("ts")), curses.A_DIM), (6, "›", self.C["cyan"] | curses.A_BOLD), (8, l, curses.A_BOLD)]
                               if i == 0 else [(8, l, curses.A_BOLD)])
            elif k == "text":
                for l in wrap(g["text"], tw, 4):
                    out.append([(8, l, 0)])
            elif k == "tool":
                name = g["tool"].replace("mcp__", "")[:14]
                n = len(g["items"])
                if n == 1:
                    subj = g.get("desc") or g["items"][0]
                else:
                    names = []
                    for it in g["items"]:
                        b = os.path.basename(it.rstrip("/")) or it
                        if b not in names:
                            names.append(b)
                    subj = ", ".join(names[-5:])
                label = name + (f" ×{n}" if n > 1 else "")
                out.append([(8, "▸", curses.A_DIM), (10, label, self.C["cyan"]),
                            (11 + max(9, len(label)), subj[:max(0, tw - max(9, len(label)) - 3)], curses.A_DIM)])
            elif k == "error":
                out.append([(8, "✗", self.C["red"]), (10, g["text"][:tw - 2], self.C["red"])])
            elif k == "mark":
                out.append([(8, f"── {g['text']} ──", curses.A_DIM)])
        return out

    def feed(self, s, act, y, x, width, end):
        if end - y < 2:
            return
        self.put(y, x, "ACTIVITY", curses.A_DIM | curses.A_BOLD)
        self.put(y, x + 9, "─" * max(0, width - 9), curses.A_DIM)
        y += 1
        events = act.get("events") or []
        if not events:
            msg = ("no activity yet" if s.get("transcript") or s["tool"] == "claude"
                   else f"{s['tool']} doesn't share its activity yet")
            self.put(y, x + 1, msg, curses.A_DIM)
            return
        lines = self.feed_lines(events, width, s["tool"])
        for line in lines[-(end - y):]:
            for dx, text, attr in line:
                self.put(y, x + dx, text, attr, width=width - dx)
            y += 1

    # ---------- history view ----------

    def history_rows(self):
        q = self.hist_q.lower()
        return [h for h in self.hist if not q or q in (h["prompt"] or "").lower() or q in (h["cwd"] or "").lower()]

    def history_view(self, y, bottom, w):
        rows = self.history_rows()
        title = f"PAST SESSIONS  {len(rows)}" + (f"   matching “{self.hist_q}”" if self.hist_q else "   / to search")
        self.put(y, 1, title, curses.A_BOLD | curses.A_DIM)
        y += 1
        self.hist_i = max(0, min(self.hist_i, len(rows) - 1))
        lw = min(70, w * 11 // 20) if w >= 110 else w
        cw = min(26, lw // 3)
        n = bottom - y + 1
        start = max(0, self.hist_i - n + 1)
        now = time.time()
        for i, hrec in enumerate(rows[start:start + n], start):
            yy = y + i - start
            sel = i == self.hist_i
            self.put(yy, 0, "›" if sel else " ", curses.A_BOLD)
            self.put(yy, 2, f"{core.ago(now - hrec['mtime']):>3}", curses.A_DIM)
            folder = core.short(hrec["cwd"] or "?")
            folder = folder if len(folder) <= cw else "…" + folder[-(cw - 1):]
            self.put(yy, 7, folder.ljust(cw), self.C["cyan"])
            self.put(yy, 8 + cw, (hrec["prompt"] or "")[:lw - cw - 10], curses.A_REVERSE if sel else 0)
        if not rows:
            self.put(y + 1, 2, "nothing matches" if self.hist_q else "no past sessions found", curses.A_DIM)
        if w >= 110 and rows:
            hrec = rows[self.hist_i]
            x = lw + 2
            for yy in range(y - 1, bottom + 1):
                self.put(yy, lw, "│", curses.A_DIM)
            self.put(y - 1, x, core.short(hrec["cwd"] or "?"), curses.A_BOLD)
            self.put(y, x, f"{hrec['sid'][:8]}  ·  last active {core.ago(now - hrec['mtime'])} ago", curses.A_DIM)
            act = claude.activity(hrec["path"])
            lines = self.feed_lines(act["events"], w - x - 1, "claude")
            for i, line in enumerate(lines[-(bottom - y - 1):]):
                for dx, text, attr in line:
                    self.put(y + 2 + i, x + dx, text, attr, width=w - x - dx - 1)

    # ---------- usage view ----------

    def usage_view(self, y, bottom, w):
        if self.usage is None:
            self.put(y + 1, 2, "counting tokens across your transcripts…", curses.A_DIM)
            return
        self.put(y, 1, "TOKENS BY DAY  (last 7 days)", curses.A_BOLD | curses.A_DIM)
        hdr = f"{'day':<12}{'input':>10}{'output':>10}{'cache':>10}{'total':>10}"
        self.put(y + 1, 2, hdr, curses.A_DIM)
        yy = y + 2
        days = sorted(self.usage, reverse=True)[:7]
        peak = max([sum(p["in"] + p["out"] + p["cache"] for p in self.usage[d].values()) for d in days] + [1])
        for d in days:
            t = {k: sum(p[k] for p in self.usage[d].values()) for k in ("in", "out", "cache")}
            total = t["in"] + t["out"] + t["cache"]
            self.put(yy, 2, f"{d:<12}{tokens(t['in']):>10}{tokens(t['out']):>10}{tokens(t['cache']):>10}{tokens(total):>10}")
            bar = int((w - 58) * total / peak) if w > 70 else 0
            self.put(yy, 56, "█" * bar, self.C["cyan"])
            yy += 1
        today = self.usage.get(time.strftime("%Y-%m-%d"), {})
        yy += 1
        self.put(yy, 1, "TODAY BY PROJECT", curses.A_BOLD | curses.A_DIM)
        yy += 1
        for proj, t in sorted(today.items(), key=lambda kv: -(kv[1]["in"] + kv[1]["out"] + kv[1]["cache"]))[:max(0, bottom - yy)]:
            self.put(yy, 2, f"{tokens(t['in'] + t['out'] + t['cache']):>8}  {core.short(proj)}")
            yy += 1
        self.put(bottom, 2, "Counts come from your Claude Code transcripts (input + output + cache reads/writes).", curses.A_DIM)

    # ---------- help ----------

    HELP = [
        ("SESSIONS", None),
        ("↑ ↓  j k", "move between sessions"),
        ("tab", "jump to the next session waiting on you"),
        ("y / n", "allow / deny the permission it's asking for"),
        ("1 – 9", "answer its question (toggle on multi-select)"),
        ("enter", "send a multi-select answer"),
        ("t", "type your own answer"),
        ("o", "jump to its terminal (or attach, for background sessions)"),
        ("a / A", "auto-yes for this session / for every session"),
        ("x x", "stop a background session"),
        ("s", "start a new background session, optionally in a new worktree"),
        ("VIEWS", None),
        ("h", "past sessions: search, open, or continue with a prompt"),
        ("u", "token usage by day and project"),
        ("m", "mute desktop notifications"),
        ("i", "install the Claude Code hook"),
        ("esc", "back to sessions"),
        ("q", "quit"),
    ]

    def help_overlay(self, h, w):
        rl = rules.load()
        lines = []
        for k, v in self.HELP:
            lines.append((k, v))
        lines.append(("RULES", None))
        if rl:
            for r in rl[:6]:
                lines.append((r["verb"], r["text"][len(r["verb"]):].strip()))
        else:
            lines.append(("", "none yet — grandaddy rules add yes \"Bash(npm test*)\""))
        lines.append(("PHONE", None))
        lines.append(("", "on — approvals go to your ntfy topic" if phone.config() else "off — grandaddy phone setup"))
        bw = min(w - 4, 78)
        bh = min(h - 2, len(lines) + 4)
        bx, by = (w - bw) // 2, max(0, (h - bh) // 2)
        for yy in range(by, by + bh):
            self.put(yy, bx, " " * bw)
        self.put(by, bx, "┌" + "─" * (bw - 2) + "┐", curses.A_DIM)
        self.put(by + bh - 1, bx, "└" + "─" * (bw - 2) + "┘", curses.A_DIM)
        for yy in range(by + 1, by + bh - 1):
            self.put(yy, bx, "│", curses.A_DIM); self.put(yy, bx + bw - 1, "│", curses.A_DIM)
        self.put(by, bx + 2, f" {brand.MINI} {brand.NAME} — keys ", curses.A_BOLD)
        yy = by + 1
        kw = max(len(k) for k, v in lines if v is not None) + 4
        for k, v in lines[:bh - 3]:
            if v is None:
                self.put(yy, bx + 3, k, curses.A_BOLD | curses.A_DIM)
            else:
                if k:
                    self.put(yy, bx + 3, f" {k} ", curses.A_REVERSE)
                self.put(yy, bx + 3 + kw, v[:bw - kw - 5])
            yy += 1
        self.put(by + bh - 2, bx + 3, "any key to close", curses.A_DIM)

    # ---------- input ----------

    def ask(self, label, default=""):
        """One-line editor on the message row. Returns the text, or None if you pressed esc."""
        h, w = self.scr.getmaxyx()
        buf = list(default)
        cursor(True)
        self.scr.timeout(-1)
        try:
            while True:
                self.put(h - 2, 0, " " * (w - 1))
                self.put(h - 2, 1, label, self.C["yellow"] | curses.A_BOLD)
                text = "".join(buf)
                room = w - len(label) - 4
                shown = text[-room:] if room > 0 else ""
                self.put(h - 2, 2 + len(label), shown)
                try:
                    self.scr.move(h - 2, min(w - 2, 2 + len(label) + len(shown)))
                except curses.error:
                    pass
                self.scr.refresh()
                ch = self.scr.get_wch()
                if ch in ("\n", "\r", curses.KEY_ENTER):
                    return "".join(buf).strip()
                if ch == "\x1b":
                    self.scr.nodelay(True)
                    nxt = self.scr.getch()
                    if nxt != -1:          # an arrow or other sequence: swallow it
                        while self.scr.getch() != -1:
                            pass
                        self.scr.timeout(-1)
                        continue
                    self.scr.timeout(-1)
                    return None
                if ch in ("\x7f", "\b", curses.KEY_BACKSPACE):
                    if buf:
                        buf.pop()
                elif ch == "\x15":          # ctrl-u
                    buf = []
                elif isinstance(ch, str) and ch.isprintable():
                    buf.append(ch)
        finally:
            cursor(False)
            self.scr.timeout(1000)

    def background(self, label, fn, *args):
        """Run a slow action off the UI thread; report its result as a message."""
        self.say(label + "…")

        def work():
            try:
                ok, out = fn(*args)
            except Exception as e:  # report, don't take the hub down
                ok, out = False, str(e)
            last = next((l for l in reversed((out or "").splitlines()) if l.strip()), "")
            self.say(last or ("done" if ok else "failed"), "info" if ok else "error")
        threading.Thread(target=work, daemon=True).start()

    # ---------- keys ----------

    def handle(self, k, sess):
        if k == ord("q"):
            return "quit"
        if k == ord("?"):
            self.overlay = "help"
            return
        if k == ord("m"):
            self.mute = not self.mute
            self.say("notifications off" if self.mute else "notifications on")
            return
        if k == ord("i"):
            r = install.install()
            self.say({"installed": "hook installed", "updated": "hook updated", "already": "hook already installed"}[r])
            return
        if k == ord("u"):
            self.view = "sessions" if self.view == "usage" else "usage"
            self.usage_t = 0 if self.usage is None else self.usage_t
            return
        if k == ord("h") and self.view != "history":
            self.view, self.hist_q, self.hist_i = "history", "", 0
            self.say("loading past sessions…")
            self.hist = claude.history()
            self.say(f"{len(self.hist)} past sessions")
            return
        if k == "esc" or (k == ord("h") and self.view == "history"):
            if self.view == "history" and self.hist_q:
                self.hist_q = ""
            else:
                self.view = "sessions"
            return
        if self.view == "history":
            return self.handle_history(k)
        if self.view == "usage":
            return
        return self.handle_sessions(k, sess)

    def handle_history(self, k):
        rows = self.history_rows()
        if k in (curses.KEY_UP, ord("k")):
            self.hist_i -= 1
        elif k in (curses.KEY_DOWN, ord("j")):
            self.hist_i += 1
        elif k == ord("/"):
            q = self.ask("search: ", self.hist_q)
            if q is not None:
                self.hist_q, self.hist_i = q, 0
        elif rows and k in (ord("o"), 10, 13, curses.KEY_ENTER):
            hrec = rows[self.hist_i]
            self.background("opening", launch.open_terminal, f"claude --resume {hrec['sid']}", hrec["cwd"])
        elif rows and k == ord("p"):
            hrec = rows[self.hist_i]
            prompt = self.ask("continue with: ")
            if prompt:
                self.background("resuming in the background", launch.resume, hrec["sid"], hrec["cwd"], prompt)
                self.view = "sessions"

    def handle_sessions(self, k, sess):
        s = self.selected(sess)
        if k in (curses.KEY_UP, ord("k")):
            self.sel_i -= 1; self.sel_key = None
        elif k in (curses.KEY_DOWN, ord("j")):
            self.sel_i += 1; self.sel_key = None
        elif k == 9:  # tab
            waiting = [i for i, o in enumerate(sess) if o["status"] == "needs you"]
            if waiting:
                nxt = next((i for i in waiting if i > self.sel_i), waiting[0])
                self.sel_i, self.sel_key = nxt, sess[nxt]["key"]
            else:
                self.say("nothing is waiting on you")
        elif k == ord("A"):
            on = core.ALL not in core.auto_ids()
            core.set_auto(core.ALL, on)
            self.say("auto-yes on for ALL sessions" if on else "auto-yes for all: off")
        elif k == ord("s"):
            self.start_session(s)
        elif not s:
            return
        elif s["pending"] and self.answer_key(k, s, s["pending"][0]):
            return
        elif k == ord("a"):
            if s["tool"] != "claude":
                self.say("auto-yes works for Claude Code sessions only")
            else:
                on = not s["auto"] or (core.ALL in core.auto_ids() and s["sid"] not in core.auto_ids())
                core.set_auto(s["sid"], on)
                note = " (ALL is still on)" if not on and core.ALL in core.auto_ids() else ""
                self.say(("auto-yes on: " if on else "auto-yes off: ") + s["name"] + note)
        elif k == ord("o"):
            if s["bg"] and s["short"]:
                self.background("opening", launch.open_terminal, f"claude attach {s['short']}", s["cwd"])
            elif s["pid"]:
                self.background("finding its terminal", launch.jump, s["pid"])
        elif k == ord("x"):
            if not (s["bg"] and s["short"]):
                self.say("only background sessions can be stopped from here")
            elif self.confirm and self.confirm[0] == s["key"] and time.time() < self.confirm[1]:
                self.confirm = None
                self.background(f"stopping {s['name']}", launch._claude, ["stop", s["short"]], None)
            else:
                self.confirm = (s["key"], time.time() + 4)
                self.say(f"press x again to stop {s['name']}")

    def start_session(self, s):
        folder = self.ask("start in folder: ", core.short((s or {}).get("cwd") or os.getcwd()))
        if not folder:
            return
        folder = os.path.expanduser(folder)
        prompt = self.ask("first prompt: ")
        if not prompt:
            self.say("cancelled — a new session needs a first prompt")
            return
        wt = self.ask("new git worktree? (enter = no, y = yes, or type its name): ")
        if wt is None:
            return
        worktree = None if wt.lower() in ("", "n", "no") else ("" if wt.lower() in ("y", "yes") else wt)
        self.background("starting a background session", launch.new, folder, prompt, worktree)

    def answer_key(self, k, s, req):
        """True if the key was used for the pending request."""
        if req["tool"] != "AskUserQuestion":
            if k in (ord("y"), ord("n")):
                claude.answer(req, {"kind": "allow" if k == ord("y") else "deny"})
                self.say(("allowed: " if k == ord("y") else "denied: ") + claude.tool_summary(req["tool"], req["input"]))
                return True
            return False
        qs = (req["input"] or {}).get("questions") or []
        d = self.drafts.setdefault(req["id"], {"qi": 0, "answers": {}, "multi": set()})
        if d["qi"] >= len(qs):
            return False
        q, opts = qs[d["qi"]], qs[d["qi"]].get("options") or []
        if isinstance(k, int) and ord("1") <= k <= ord("9") and k - ord("1") < len(opts):
            i = k - ord("1")
            if q.get("multiSelect"):
                d["multi"] ^= {i}
            else:
                self.record(req, d, qs, q, opts[i]["label"])
            return True
        if k in (10, 13, curses.KEY_ENTER) and q.get("multiSelect") and d["multi"]:
            self.record(req, d, qs, q, ", ".join(opts[i]["label"] for i in sorted(d["multi"])))
            return True
        if k == ord("t"):
            text = self.ask("your answer: ")
            if text:
                self.record(req, d, qs, q, text)
            return True
        return False

    def record(self, req, d, qs, q, text):
        d["answers"][q.get("question", "")] = text
        d["qi"] += 1
        d["multi"] = set()
        if d["qi"] >= len(qs):
            claude.answer(req, {"kind": "answers", "answers": d["answers"]})
            self.say("answered: " + "; ".join(d["answers"].values()))


def run():
    try:
        curses.wrapper(lambda scr: Hub(scr).run())
    except KeyboardInterrupt:
        pass
