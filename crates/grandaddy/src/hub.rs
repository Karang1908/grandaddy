//! The live hub: every session, what it's doing, and answering whatever it's waiting on.
use grandaddy_core::claude::{self, Decision, Event, Past, Tokens};
use grandaddy_core::sessions::{self, Session};
use grandaddy_core::util::{self, now};
use grandaddy_core::{install, launch, phone, rules};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DOT: &str = "●";
const FILE_TOOLS: &[&str] = &["Read", "Edit", "Write", "MultiEdit", "NotebookEdit", "Grep", "Glob"];
const FACE: [&str; 5] = ["    _.---._", " ~ /       \\ ~", "~ | (◠)-(◠) | ~", "   \\  ⌒⌒⌒  /", "    `-...-'"];
const FACE_W: u16 = 15;
const MINI: &str = "(◠‿◠)";
const TAGLINE: &str = "Manage your vibe coding sessions. All in one place.";

type Usage = BTreeMap<String, BTreeMap<String, Tokens>>;

#[derive(PartialEq, Clone, Copy)]
enum View {
    Sessions,
    History,
    Usage,
}

enum Purpose {
    Search,
    Continue(Past),
    StartFolder,
    StartPrompt(String),
    StartWorktree(String, String),
    Answer(String),
}

struct Ask {
    label: String,
    buf: String,
    purpose: Purpose,
}

#[derive(Default, Clone)]
struct Draft {
    qi: usize,
    answers: Map<String, Value>,
    multi: BTreeSet<usize>,
}

struct Hub {
    view: View,
    help: bool,
    sess: Vec<Session>,
    sel_key: Option<String>,
    sel_i: usize,
    hist: Vec<Past>,
    hist_i: usize,
    hist_q: String,
    msg: Arc<Mutex<(String, f64, bool)>>,
    mute: bool,
    prev: HashMap<String, (String, f64)>,
    first: bool,
    drafts: HashMap<String, Draft>,
    confirm: Option<(String, f64)>,
    usage: Arc<Mutex<Option<Usage>>>,
    usage_t: f64,
    usage_busy: Arc<Mutex<bool>>,
    ask: Option<Ask>,
    installed: bool,
}

// ---------- small helpers ----------

fn st(fg: Color) -> Style {
    Style::default().fg(fg)
}
fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}
fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}
fn rev() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}
fn hot() -> Style {
    Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
}
fn status_style(s: &str) -> Style {
    match s {
        "working" => st(Color::Green),
        "needs you" => st(Color::Yellow).add_modifier(Modifier::BOLD),
        "idle" => st(Color::Blue),
        "running" => st(Color::Magenta),
        "blocked" | "failed" => st(Color::Red),
        _ => dim(),
    }
}
fn chars(s: &str) -> usize {
    s.chars().count()
}
fn take(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Word-wrap keeping paragraph breaks; hard-break words longer than the width.
fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(4);
    let mut lines: Vec<String> = vec![];
    for para in text.lines() {
        let mut cur = String::new();
        for w in para.split_whitespace() {
            let mut w = w.to_string();
            while chars(&w) > width {
                if !cur.is_empty() {
                    lines.push(std::mem::take(&mut cur));
                }
                lines.push(take(&w, width));
                w = w.chars().skip(width).collect();
            }
            if !cur.is_empty() && chars(&cur) + 1 + chars(&w) > width {
                lines.push(std::mem::replace(&mut cur, w));
            } else if cur.is_empty() {
                cur = w;
            } else {
                cur = format!("{cur} {w}");
            }
        }
        if !cur.trim().is_empty() {
            lines.push(cur);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        let last = lines.last_mut().unwrap();
        *last = format!("{}…", take(last, width - 1).trim_end());
    }
    lines
}

struct Canvas<'a> {
    buf: &'a mut Buffer,
    w: u16,
    h: u16,
}

impl Canvas<'_> {
    /// Write text at (y, x), clipped to the screen (and `width` columns if given). Returns columns used.
    fn put(&mut self, y: u16, x: u16, text: &str, style: Style, width: Option<usize>) -> u16 {
        if y >= self.h || x + 1 >= self.w {
            return 0;
        }
        let room = (self.w - x - 1) as usize;
        let room = width.map(|w| w.min(room)).unwrap_or(room);
        if room == 0 {
            return 0;
        }
        let (end, _) = self.buf.set_stringn(x, y, text, room, style);
        end - x
    }
    fn p(&mut self, y: u16, x: u16, text: &str, style: Style) -> u16 {
        self.put(y, x, text, style, None)
    }
    fn hline(&mut self, y: u16) {
        let line = "─".repeat(self.w as usize);
        self.put(y, 0, &line, dim(), None);
    }
}

// ---------- hub ----------

impl Hub {
    fn new() -> Self {
        Hub {
            view: View::Sessions,
            help: false,
            sess: vec![],
            sel_key: None,
            sel_i: 0,
            hist: vec![],
            hist_i: 0,
            hist_q: String::new(),
            msg: Arc::new(Mutex::new((String::new(), 0.0, false))),
            mute: std::env::var("GRANDADDY_NOTIFY").as_deref() == Ok("0"),
            prev: HashMap::new(),
            first: true,
            drafts: HashMap::new(),
            confirm: None,
            usage: Arc::new(Mutex::new(None)),
            usage_t: 0.0,
            usage_busy: Arc::new(Mutex::new(false)),
            ask: None,
            installed: install::installed(),
        }
    }

    fn say(&self, m: impl Into<String>) {
        *self.msg.lock().unwrap() = (m.into(), now(), false);
    }

    fn background<F: FnOnce() -> (bool, String) + Send + 'static>(&self, label: &str, f: F) {
        self.say(format!("{label}…"));
        let msg = self.msg.clone();
        std::thread::spawn(move || {
            let (ok, out) = f();
            let last = out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(if ok { "done" } else { "failed" }).to_string();
            *msg.lock().unwrap() = (last, now(), !ok);
        });
    }

    fn refresh(&mut self) {
        self.sess = sessions::collect();
        self.installed = install::installed();
        self.track();
        if !*self.usage_busy.lock().unwrap() && now() - self.usage_t > 60.0 {
            *self.usage_busy.lock().unwrap() = true;
            self.usage_t = now();
            let (u, busy) = (self.usage.clone(), self.usage_busy.clone());
            std::thread::spawn(move || {
                let r = claude::usage(7);
                *u.lock().unwrap() = Some(r);
                *busy.lock().unwrap() = false;
            });
        }
    }

    /// Notify on: started needing you, or finished a stretch of work.
    fn track(&mut self) {
        let t = now();
        for s in &self.sess {
            let prev = self.prev.get(&s.key).cloned();
            if let (Some((pst, since)), false, false) = (&prev, self.first, self.mute) {
                if s.status == "needs you" && pst != "needs you" {
                    let body = s.pending.first().map(claude::describe_request).unwrap_or_else(|| "is waiting for you".into());
                    launch::notify(&s.name, &body);
                } else if s.status == "idle" && pst == "working" && t - since >= 15.0 {
                    launch::notify(&s.name, &format!("finished after {}", util::ago(t - since)));
                }
            }
            let since = match &prev {
                Some((p, since)) if p == "working" && s.status == "working" => *since,
                _ => t,
            };
            self.prev.insert(s.key.clone(), (s.status.clone(), since));
        }
        self.first = false;
        let live: BTreeSet<String> = self.sess.iter().flat_map(|s| s.pending.iter().map(|r| r.id.clone())).collect();
        self.drafts.retain(|rid, _| live.contains(rid));
    }

    fn selected(&mut self) -> Option<Session> {
        if self.sess.is_empty() {
            return None;
        }
        if let Some(i) = self.sel_key.as_ref().and_then(|k| self.sess.iter().position(|s| &s.key == k)) {
            self.sel_i = i;
        }
        self.sel_i = self.sel_i.min(self.sess.len() - 1);
        self.sel_key = Some(self.sess[self.sel_i].key.clone());
        Some(self.sess[self.sel_i].clone())
    }

    fn history_rows(&self) -> Vec<Past> {
        let q = self.hist_q.to_lowercase();
        self.hist
            .iter()
            .filter(|h| q.is_empty() || h.prompt.as_deref().unwrap_or("").to_lowercase().contains(&q) || h.cwd.as_deref().unwrap_or("").to_lowercase().contains(&q))
            .cloned()
            .collect()
    }

    // ---------- drawing ----------

    fn draw(&mut self, buf: &mut Buffer) {
        let area = buf.area;
        let mut c = Canvas { buf, w: area.width, h: area.height };
        let (h, w) = (c.h, c.w);
        if h < 4 || w < 20 {
            c.p(0, 0, "grandaddy: window too small", dim());
            return;
        }
        let y = self.header(&mut c);
        let bottom = h.saturating_sub(3);
        match self.view {
            View::History => self.history_view(&mut c, y, bottom),
            View::Usage => self.usage_view(&mut c, y, bottom),
            View::Sessions => {
                let cur = self.selected();
                if w >= 100 {
                    let lw = (w * 2 / 5).clamp(40, 56);
                    self.session_list(&mut c, y, 0, lw, bottom.saturating_sub(y) + 1);
                    for yy in y..=bottom {
                        c.p(yy, lw, "│", dim());
                    }
                    self.detail(&mut c, cur.as_ref(), y, lw + 2, w.saturating_sub(lw + 3), bottom + 1);
                } else {
                    let rows = ((self.sess.len().max(1) + 1) as u16).min((bottom.saturating_sub(y) / 3).max(4));
                    self.session_list(&mut c, y, 0, w, rows);
                    c.hline(y + rows);
                    self.detail(&mut c, cur.as_ref(), y + rows + 1, 1, w.saturating_sub(2), bottom + 1);
                }
            }
        }
        self.footer(&mut c);
        if self.help {
            self.help_overlay(&mut c);
        }
    }

    fn header(&self, c: &mut Canvas) -> u16 {
        let (h, w) = (c.h, c.w);
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for s in &self.sess {
            *counts.entry(s.status.as_str()).or_default() += 1;
        }
        let today = util::today();
        let today_tokens = self.usage.lock().unwrap().as_ref().map(|u| u.get(&today).map(|d| d.values().map(|t| t.total()).sum::<u64>()).unwrap_or(0));
        let (mut x, cy, y);
        if h >= 28 && w >= 72 {
            for (i, line) in FACE.iter().enumerate() {
                c.p(i as u16, 1, line, Style::default());
            }
            x = FACE_W + 4;
            c.p(1, x, "grandaddy", bold());
            c.p(2, x, TAGLINE, dim());
            cy = 4;
            y = FACE.len() as u16;
        } else {
            let n = c.p(0, 1, MINI, bold());
            let n2 = c.p(0, 3 + n, "grandaddy", bold());
            x = 3 + n + n2 + 3;
            cy = 0;
            y = 1;
        }
        if self.view == View::Sessions {
            if self.sess.is_empty() {
                x += c.p(cy, x, "no sessions running   ", dim());
            }
            for s in ["needs you", "working", "idle", "running", "blocked", "failed"] {
                if let Some(n) = counts.get(s) {
                    c.p(cy, x, DOT, status_style(s));
                    let label = format!(" {n} {s}   ");
                    x += 1 + c.p(cy, x + 1, &label, if s == "needs you" { status_style(s) } else { dim() });
                }
            }
        } else {
            let title = if self.view == View::History { "past sessions   " } else { "token usage   " };
            x += c.p(cy, x, title, bold());
        }
        if let Some(t) = today_tokens {
            x += c.p(cy, x, &format!("today {} tokens   ", util::tokens(t)), dim());
        }
        if phone::config().is_some() {
            x += c.p(cy, x, "phone on   ", dim());
        }
        if self.mute {
            x += c.p(cy, x, "muted   ", dim());
        }
        if !self.installed {
            c.p(cy, x, "hook not installed — press i", st(Color::Red).add_modifier(Modifier::BOLD));
        }
        c.hline(y);
        y + 1
    }

    fn footer(&mut self, c: &mut Canvas) {
        let (h, w) = (c.h, c.w);
        c.hline(h - 2);
        if let Some(a) = &self.ask {
            c.put(h - 2, 0, &" ".repeat(w as usize), Style::default(), None);
            let n = c.p(h - 2, 1, &a.label, st(Color::Yellow).add_modifier(Modifier::BOLD));
            let room = (w as usize).saturating_sub(chars(&a.label) + 4);
            let shown: String = a.buf.chars().rev().take(room).collect::<Vec<_>>().into_iter().rev().collect();
            c.p(h - 2, 2 + n, &shown, Style::default());
            c.p(h - 2, 2 + n + chars(&shown) as u16, "▏", st(Color::Yellow));
        } else {
            let (m, t, err) = self.msg.lock().unwrap().clone();
            if !m.is_empty() && now() - t < 7.0 {
                c.p(h - 2, 2, &format!(" {m} "), st(if err { Color::Red } else { Color::Yellow }).add_modifier(Modifier::BOLD));
            }
        }
        let hints = self.key_hints();
        let mut used = 0u16;
        for (key, label, is_hot) in hints {
            let need = (chars(&key) + 2 + 1 + chars(&label) + 2) as u16;
            if 1 + used + need > w - 1 {
                break;
            }
            c.p(h - 1, 1 + used, &format!(" {key} "), if is_hot { hot() } else { rev() });
            c.p(h - 1, 1 + used + chars(&key) as u16 + 3, &label, if is_hot { bold() } else { Style::default() });
            used += need;
        }
    }

    fn key_hints(&mut self) -> Vec<(String, String, bool)> {
        let k = |a: &str, b: &str, hot: bool| (a.to_string(), b.to_string(), hot);
        if self.ask.is_some() {
            return vec![k("enter", "ok", true), k("esc", "cancel", false), k("ctrl-u", "clear", false)];
        }
        match self.view {
            View::History => {
                return vec![k("o", "open", true), k("p", "continue with a prompt", true), k("/", "search", false), k("↑↓", "move", false), k("esc", "back", false), k("?", "help", false)]
            }
            View::Usage => return vec![k("esc", "back", false), k("?", "help", false), k("q", "quit", false)],
            View::Sessions => {}
        }
        let s = self.selected();
        let mut items = vec![];
        if let Some(req) = s.as_ref().and_then(|s| s.pending.first()) {
            if req.is_question() {
                let qs = req.questions();
                let d = self.drafts.get(&req.id).cloned().unwrap_or_default();
                let q = qs.get(d.qi.min(qs.len().saturating_sub(1))).cloned().unwrap_or(Value::Null);
                let n = q.get("options").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                let multi = q.get("multiSelect").and_then(|v| v.as_bool()) == Some(true);
                items.push(k(&if n > 1 { format!("1-{n}") } else { "1".into() }, if multi { "toggle" } else { "answer" }, true));
                items.push(k("t", "type", true));
                if multi {
                    items.push(k("enter", "send", true));
                }
            } else {
                items.push(k("y", "allow", true));
                items.push(k("n", "deny", true));
            }
        }
        if self.sess.iter().any(|o| o.status == "needs you") {
            let waiting_here = s.as_ref().map(|s| !s.pending.is_empty()).unwrap_or(false);
            items.push(k("tab", "next waiting", !waiting_here));
        }
        if let Some(s) = &s {
            items.push(k("o", if s.bg { "attach" } else { "jump to it" }, false));
            if s.tool == "claude" {
                items.push(k("a", if s.auto { "auto-yes off" } else { "auto-yes" }, false));
            }
            if s.bg {
                items.push(k("x", "stop", false));
            }
        }
        for (a, b) in [("s", "start new"), ("h", "history"), ("u", "usage"), ("?", "help"), ("q", "quit")] {
            items.push(k(a, b, false));
        }
        items
    }

    fn session_list(&self, c: &mut Canvas, y: u16, x: u16, width: u16, rows: u16) {
        let title = if self.sess.is_empty() { "SESSIONS".to_string() } else { format!("SESSIONS  {}", self.sess.len()) };
        c.p(y, x + 1, &title, dim().add_modifier(Modifier::BOLD));
        let (y, rows) = (y + 1, rows.saturating_sub(1));
        if self.sess.is_empty() {
            c.p(y + 1, x + 2, "Nothing running yet.", dim());
            c.p(y + 2, x + 2, "Press s to start one, or run claude / codex", dim());
            c.p(y + 3, x + 2, "/ gemini in any terminal.", dim());
            return;
        }
        let start = (self.sel_i + 1).saturating_sub(rows as usize);
        for (i, s) in self.sess.iter().enumerate().skip(start).take(rows as usize) {
            let yy = y + (i - start) as u16;
            let sel = i == self.sel_i;
            let tail = format!("{:<9} {:>4}", s.status, util::ago(now() - s.since));
            let name_w = (width as usize).saturating_sub(chars(&tail) + 12).max(6);
            c.p(yy, x, if sel { "›" } else { " " }, bold());
            c.p(yy, x + 2, DOT, status_style(&s.status));
            let name = format!("{:<w$}", take(&s.name, name_w), w = name_w);
            c.put(yy, x + 4, &name, if sel { bold().add_modifier(Modifier::REVERSED) } else { Style::default() }, Some(name_w));
            let tx = x + 5 + name_w as u16; // fixed 5-column tag area: "bg ◆ "
            if s.bg {
                c.p(yy, tx, "bg", dim());
            }
            if s.auto {
                c.p(yy, tx + 3, "◆", st(Color::Green));
            }
            let loud = matches!(s.status.as_str(), "needs you" | "blocked" | "failed");
            c.p(yy, tx + 5, &tail, if loud { status_style(&s.status) } else { dim() });
        }
        if self.sess.len() > rows as usize && rows > 0 {
            c.p(y + rows - 1, x + width.saturating_sub(9), &format!("+{} more", self.sess.len() - rows as usize), dim());
        }
    }

    fn detail(&self, c: &mut Canvas, s: Option<&Session>, y: u16, x: u16, width: u16, end: u16) {
        let Some(s) = s else { return };
        if end <= y + 3 || width < 24 {
            return;
        }
        let act = sessions::activity(s);
        c.p(y, x, &s.name, bold());
        let model = act.model.clone().unwrap_or_default().replace("claude-", "");
        let ident = if s.bg { s.short.clone().map(|i| format!("bg {i}")) } else { s.pid.map(|p| format!("pid {p}")) };
        let line1: Vec<String> = [Some(s.tool.clone()), (!model.is_empty()).then_some(model), ident].into_iter().flatten().collect();
        c.p(y + 1, x, &line1.join("  ·  "), dim());
        let mut where_ = vec![if s.cwd.is_empty() { "?".into() } else { util::short(&s.cwd) }];
        if let Some(g) = sessions::git(&s.cwd) {
            if let Some(b) = g.branch {
                where_.push(b);
                if let Some(n) = g.changed.filter(|n| *n > 0) {
                    where_.push(format!("{n} changed"));
                }
                if !g.ahead.is_empty() {
                    where_.push(g.ahead);
                }
            }
        }
        c.p(y + 2, x, &where_.join("  ·  "), dim());
        let mut bits = vec![format!("{} for {}", s.status, util::ago(now() - s.since))];
        if let Some(ctx) = act.context {
            bits.push(format!("context {}", util::tokens(ctx)));
        }
        if let Some(t) = s.transcript.as_ref().filter(|_| s.tool == "claude").and_then(|p| claude::session_tokens(p)).filter(|t| t.total() > 0) {
            bits.push(format!("{} tokens", util::tokens(t.total())));
        }
        if s.tool == "claude" {
            bits.push(if s.auto { "auto-yes on".into() } else { "auto-yes off".into() });
        }
        c.p(y + 3, x, DOT, status_style(&s.status));
        c.p(y + 3, x + 2, &bits.join("  ·  "), Style::default());
        let mut yy = y + 4;
        let mut warns = claude::signals(&act, &s.status);
        let twins: Vec<String> = self.sess.iter().filter(|o| o.key != s.key && !o.cwd.is_empty() && o.cwd == s.cwd).map(|o| o.name.clone()).collect();
        if !twins.is_empty() {
            warns.push(format!("also running here: {}", twins.join(", ")));
        }
        if s.tool == "claude" {
            let clash = sessions::conflicts(s, &self.sess);
            if !clash.is_empty() {
                let names: BTreeSet<String> = clash.values().flatten().cloned().collect();
                let files: Vec<String> = clash.keys().take(3).map(|f| std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect();
                let more = if clash.len() > 3 { " …" } else { "" };
                warns.push(format!("{} also edited {}{more}", names.into_iter().collect::<Vec<_>>().join(" & "), files.join(", ")));
            }
        }
        for wtxt in warns {
            for l in wrap(&format!("! {wtxt}"), width as usize, 2) {
                c.p(yy, x, &l, st(Color::Yellow));
                yy += 1;
            }
        }
        yy += 1;
        let yy = self.needs_you(c, s, yy, x, width);
        self.feed(c, s, &act, yy, x, width, end);
    }

    fn needs_you(&self, c: &mut Canvas, s: &Session, mut y: u16, x: u16, width: u16) -> u16 {
        let bar = |c: &mut Canvas, yy: u16| {
            c.p(yy, x, "┃", st(Color::Yellow));
        };
        if let Some(req) = s.pending.first() {
            if req.is_question() {
                y = self.question(c, req, y, x, width);
            } else {
                bar(c, y);
                c.p(y, x + 2, "NEEDS YOU", hot());
                c.p(y, x + 12, "permission", st(Color::Yellow));
                y += 1;
                bar(c, y);
                c.p(y, x + 2, &req.tool, bold());
                let subj = claude::subject(&req.tool, &req.input);
                let subj = if subj.is_empty() { "(no arguments)".to_string() } else { subj };
                for l in wrap(&subj, (width as usize).saturating_sub(4 + chars(&req.tool)), 5) {
                    bar(c, y);
                    c.p(y, x + 3 + chars(&req.tool) as u16, &l, Style::default());
                    y += 1;
                }
                bar(c, y);
                c.p(y, x + 2, "allow with y, deny with n — or answer in its terminal", dim());
                y += 1;
            }
            if s.pending.len() > 1 {
                bar(c, y);
                c.p(y, x + 2, &format!("+{} more waiting after this", s.pending.len() - 1), dim());
                y += 1;
            }
            return y + 1;
        }
        if s.status == "blocked" || s.status == "failed" {
            bar(c, y);
            let why = s.raw.clone().unwrap_or_default();
            c.p(y, x + 2, &format!("This background session can't run ({why}) — press o to attach and see why, or x x to stop it"), st(Color::Red));
            return y + 2;
        }
        if s.status == "needs you" {
            let note = if !s.can_answer {
                format!("Waiting on you in its own terminal ({}) — press o to jump there", s.tool)
            } else if s.bg {
                "Waiting on you — press o to attach".to_string()
            } else {
                "Waiting on you in its terminal — press o to jump there".to_string()
            };
            bar(c, y);
            c.p(y, x + 2, &note, st(Color::Yellow));
            return y + 2;
        }
        y
    }

    fn question(&self, c: &mut Canvas, req: &claude::Request, mut y: u16, x: u16, width: u16) -> u16 {
        let bar = |c: &mut Canvas, yy: u16| {
            c.p(yy, x, "┃", st(Color::Yellow));
        };
        let qs = req.questions();
        let d = self.drafts.get(&req.id).cloned().unwrap_or_default();
        if qs.is_empty() || d.qi >= qs.len() {
            bar(c, y);
            c.p(y, x + 2, "answer sent…", dim());
            return y + 1;
        }
        let q = &qs[d.qi];
        bar(c, y);
        c.p(y, x + 2, "NEEDS YOU", hot());
        let mut info = "question".to_string();
        if qs.len() > 1 {
            info += &format!(" {} of {}", d.qi + 1, qs.len());
        }
        if let Some(hdr) = q.get("header").and_then(|v| v.as_str()) {
            info += &format!("  ·  {hdr}");
        }
        c.p(y, x + 12, &info, st(Color::Yellow));
        y += 1;
        for l in wrap(q.get("question").and_then(|v| v.as_str()).unwrap_or(""), (width as usize).saturating_sub(3), 3) {
            bar(c, y);
            c.p(y, x + 2, &l, bold());
            y += 1;
        }
        let multi = q.get("multiSelect").and_then(|v| v.as_bool()) == Some(true);
        let opts = q.get("options").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let label = |o: &Value| o.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let lw = (opts.iter().map(|o| chars(&label(o))).max().unwrap_or(4).max(4) + if multi { 4 } else { 0 }).min(28);
        for (i, o) in opts.iter().enumerate() {
            bar(c, y);
            c.p(y, x + 2, &format!(" {} ", i + 1), rev());
            let picked = d.multi.contains(&i);
            let boxed = if multi { if picked { "[x] " } else { "[ ] " } } else { "" };
            c.put(y, x + 6, &format!("{boxed}{}", label(o)), if picked { bold() } else { Style::default() }, Some(lw));
            if let Some(desc) = o.get("description").and_then(|v| v.as_str()) {
                c.put(y, x + 8 + lw as u16, &util::one_line(desc, 300), dim(), Some((width as usize).saturating_sub(lw + 9)));
            }
            y += 1;
        }
        bar(c, y);
        c.p(y, x + 2, " t ", rev());
        c.p(y, x + 6, "type your own answer", dim());
        y += 1;
        bar(c, y);
        let hint = if multi { "numbers toggle, enter sends" } else { "press a number to answer" };
        c.p(y, x + 2, &format!("{hint} — or answer in its terminal"), dim());
        y + 1
    }

    /// The activity timeline as drawable lines: [(dx, text, style)] per line, oldest first.
    fn feed_lines(events: &[Event], width: u16) -> Vec<Vec<(u16, String, Style)>> {
        let tw = (width as usize).saturating_sub(8).max(10);
        // runs of the same file tool collapse into one line: "Read ×3  a.py, b.py"
        let mut groups: Vec<(Event, Vec<String>)> = vec![];
        for e in events {
            if e.k == "tool" {
                if let Some((last, items)) = groups.last_mut() {
                    if last.k == "tool" && last.tool == e.tool && FILE_TOOLS.contains(&e.tool.as_deref().unwrap_or("")) {
                        items.push(e.text.clone());
                        continue;
                    }
                }
            }
            groups.push((e.clone(), vec![e.text.clone()]));
        }
        let mut out: Vec<Vec<(u16, String, Style)>> = vec![];
        if groups.first().map(|(e, _)| e.k != "you").unwrap_or(false) {
            out.push(vec![(8, "…".into(), dim())]);
        }
        for (g, items) in &groups {
            match g.k.as_str() {
                "you" => {
                    if !out.is_empty() {
                        out.push(vec![]);
                    }
                    for (i, l) in wrap(&g.text, tw.saturating_sub(2), 3).into_iter().enumerate() {
                        out.push(if i == 0 {
                            vec![(0, util::clock(g.ts), dim()), (6, "›".into(), st(Color::Cyan).add_modifier(Modifier::BOLD)), (8, l, bold())]
                        } else {
                            vec![(8, l, bold())]
                        });
                    }
                }
                "text" => {
                    for l in wrap(&g.text, tw, 4) {
                        out.push(vec![(8, l, Style::default())]);
                    }
                }
                "tool" => {
                    let name = take(&g.tool.clone().unwrap_or_default().replace("mcp__", ""), 14);
                    let n = items.len();
                    let subj = if n == 1 {
                        g.desc.clone().unwrap_or_else(|| items[0].clone())
                    } else {
                        let mut names: Vec<String> = vec![];
                        for it in items {
                            let b = std::path::Path::new(it.trim_end_matches('/')).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| it.clone());
                            if !names.contains(&b) {
                                names.push(b);
                            }
                        }
                        names[names.len().saturating_sub(5)..].join(", ")
                    };
                    let label = if n > 1 { format!("{name} ×{n}") } else { name };
                    let col = chars(&label).max(9);
                    out.push(vec![(8, "▸".into(), dim()), (10, label, st(Color::Cyan)), (11 + col as u16, take(&subj, tw.saturating_sub(col + 3)), dim())]);
                }
                "error" => out.push(vec![(8, "✗".into(), st(Color::Red)), (10, take(&g.text, tw.saturating_sub(2)), st(Color::Red))]),
                _ => out.push(vec![(8, format!("── {} ──", g.text), dim())]),
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn feed(&self, c: &mut Canvas, s: &Session, act: &claude::Activity, mut y: u16, x: u16, width: u16, end: u16) {
        if end <= y + 1 {
            return;
        }
        c.p(y, x, "ACTIVITY", dim().add_modifier(Modifier::BOLD));
        c.p(y, x + 9, &"─".repeat((width as usize).saturating_sub(9)), dim());
        y += 1;
        if act.events.is_empty() {
            let msg = if s.transcript.is_some() || s.tool == "claude" { "no activity yet".to_string() } else { format!("{} doesn't share its activity yet", s.tool) };
            c.p(y, x + 1, &msg, dim());
            return;
        }
        let lines = Self::feed_lines(&act.events, width);
        let room = (end - y) as usize;
        for line in &lines[lines.len().saturating_sub(room)..] {
            for (dx, text, style) in line {
                c.put(y, x + dx, text, *style, Some((width.saturating_sub(*dx)) as usize));
            }
            y += 1;
        }
    }

    fn history_view(&self, c: &mut Canvas, y: u16, bottom: u16) {
        let w = c.w;
        let rows = self.history_rows();
        let title = if self.hist_q.is_empty() { format!("PAST SESSIONS  {}   / to search", rows.len()) } else { format!("PAST SESSIONS  {}   matching “{}”", rows.len(), self.hist_q) };
        c.p(y, 1, &title, bold().add_modifier(Modifier::DIM));
        let y = y + 1;
        let hi = self.hist_i.min(rows.len().saturating_sub(1));
        let lw = if w >= 110 { (w as usize * 11 / 20).min(70) } else { w as usize };
        let cw = (lw / 3).min(26);
        let n = (bottom + 1).saturating_sub(y) as usize;
        let start = (hi + 1).saturating_sub(n);
        for (i, h) in rows.iter().enumerate().skip(start).take(n) {
            let yy = y + (i - start) as u16;
            let sel = i == hi;
            c.p(yy, 0, if sel { "›" } else { " " }, bold());
            c.p(yy, 2, &format!("{:>3}", util::ago(now() - h.mtime)), dim());
            let folder = util::short(h.cwd.as_deref().unwrap_or("?"));
            let folder = if chars(&folder) <= cw { folder } else { format!("…{}", folder.chars().rev().take(cw - 1).collect::<Vec<_>>().into_iter().rev().collect::<String>()) };
            c.p(yy, 7, &format!("{folder:<cw$}"), st(Color::Cyan));
            c.put(yy, 8 + cw as u16, h.prompt.as_deref().unwrap_or(""), if sel { rev() } else { Style::default() }, Some(lw.saturating_sub(cw + 10)));
        }
        if rows.is_empty() {
            c.p(y + 1, 2, if self.hist_q.is_empty() { "no past sessions found" } else { "nothing matches" }, dim());
        }
        if w >= 110 && !rows.is_empty() {
            let h = &rows[hi];
            let x = lw as u16 + 2;
            for yy in y - 1..=bottom {
                c.p(yy, lw as u16, "│", dim());
            }
            c.p(y - 1, x, &util::short(h.cwd.as_deref().unwrap_or("?")), bold());
            c.p(y, x, &format!("{}  ·  last active {} ago", take(&h.sid, 8), util::ago(now() - h.mtime)), dim());
            let act = claude::activity(std::path::Path::new(&h.path));
            let lines = Self::feed_lines(&act.events, w - x - 1);
            let room = bottom.saturating_sub(y + 1) as usize;
            for (i, line) in lines[lines.len().saturating_sub(room)..].iter().enumerate() {
                for (dx, text, style) in line {
                    c.put(y + 2 + i as u16, x + dx, text, *style, Some((w - x - dx - 1) as usize));
                }
            }
        }
    }

    fn usage_view(&self, c: &mut Canvas, y: u16, bottom: u16) {
        let w = c.w;
        let usage = self.usage.lock().unwrap().clone();
        let Some(u) = usage else {
            c.p(y + 1, 2, "counting tokens across your transcripts…", dim());
            return;
        };
        c.p(y, 1, "TOKENS BY DAY  (last 7 days)", bold().add_modifier(Modifier::DIM));
        c.p(y + 1, 2, &format!("{:<12}{:>10}{:>10}{:>10}{:>10}", "day", "input", "output", "cache", "total"), dim());
        let mut yy = y + 2;
        let days: Vec<(&String, Tokens)> = u
            .iter()
            .rev()
            .take(7)
            .map(|(d, ps)| {
                let mut t = Tokens::default();
                for p in ps.values() {
                    t.input += p.input;
                    t.out += p.out;
                    t.cache += p.cache;
                }
                (d, t)
            })
            .collect();
        let peak = days.iter().map(|(_, t)| t.total()).max().unwrap_or(1).max(1);
        for (d, t) in &days {
            c.p(yy, 2, &format!("{:<12}{:>10}{:>10}{:>10}{:>10}", d, util::tokens(t.input), util::tokens(t.out), util::tokens(t.cache), util::tokens(t.total())), Style::default());
            if w > 70 {
                let bar = ((w as u64 - 58) * t.total() / peak) as usize;
                c.p(yy, 56, &"█".repeat(bar), st(Color::Cyan));
            }
            yy += 1;
        }
        let today = util::today();
        yy += 1;
        c.p(yy, 1, "TODAY BY PROJECT", bold().add_modifier(Modifier::DIM));
        yy += 1;
        if let Some(ps) = u.get(&today) {
            let mut v: Vec<_> = ps.iter().collect();
            v.sort_by_key(|(_, t)| std::cmp::Reverse(t.total()));
            for (p, t) in v.into_iter().take(bottom.saturating_sub(yy) as usize) {
                c.p(yy, 2, &format!("{:>8}  {}", util::tokens(t.total()), util::short(p)), Style::default());
                yy += 1;
            }
        }
        c.p(bottom, 2, "Counts come from your Claude Code transcripts (input + output + cache reads/writes).", dim());
    }

    fn help_overlay(&self, c: &mut Canvas) {
        let (h, w) = (c.h, c.w);
        let mut lines: Vec<(String, Option<String>)> = vec![];
        let help: &[(&str, Option<&str>)] = &[
            ("SESSIONS", None),
            ("↑ ↓  j k", Some("move between sessions")),
            ("tab", Some("jump to the next session waiting on you")),
            ("y / n", Some("allow / deny the permission it's asking for")),
            ("1 – 9", Some("answer its question (toggle on multi-select)")),
            ("enter", Some("send a multi-select answer")),
            ("t", Some("type your own answer")),
            ("o", Some("jump to its terminal (or attach, for background sessions)")),
            ("a / A", Some("auto-yes for this session / for every session")),
            ("x x", Some("stop a background session")),
            ("s", Some("start a new background session, optionally in a new worktree")),
            ("VIEWS", None),
            ("h", Some("past sessions: search, open, or continue with a prompt")),
            ("u", Some("token usage by day and project")),
            ("m", Some("mute desktop notifications")),
            ("i", Some("install the Claude Code hook")),
            ("esc", Some("back to sessions")),
            ("q", Some("quit")),
        ];
        for (k, v) in help {
            lines.push((k.to_string(), v.map(String::from)));
        }
        lines.push(("RULES".into(), None));
        let rl = rules::load();
        if rl.is_empty() {
            lines.push((String::new(), Some("none yet — grandaddy rules add yes \"Bash(npm test*)\"".into())));
        }
        for r in rl.iter().take(6) {
            lines.push((r.verb.clone(), Some(r.text[r.verb.len()..].trim().to_string())));
        }
        lines.push(("PHONE".into(), None));
        lines.push((String::new(), Some(if phone::config().is_some() { "on — approvals go to your ntfy topic".into() } else { "off — grandaddy phone setup".into() })));
        let bw = w.saturating_sub(4).min(78);
        let bh = h.saturating_sub(2).min(lines.len() as u16 + 4);
        let (bx, by) = ((w - bw) / 2, (h.saturating_sub(bh)) / 2);
        for yy in by..by + bh {
            c.put(yy, bx, &" ".repeat(bw as usize), Style::default(), None);
        }
        c.p(by, bx, &format!("┌{}┐", "─".repeat(bw as usize - 2)), dim());
        c.p(by + bh - 1, bx, &format!("└{}┘", "─".repeat(bw as usize - 2)), dim());
        for yy in by + 1..by + bh - 1 {
            c.p(yy, bx, "│", dim());
            c.p(yy, bx + bw - 1, "│", dim());
        }
        c.p(by, bx + 2, &format!(" {MINI} grandaddy — keys "), bold());
        let kw = lines.iter().filter(|(_, v)| v.is_some()).map(|(k, _)| chars(k)).max().unwrap_or(4) as u16 + 4;
        let mut yy = by + 1;
        for (k, v) in lines.iter().take(bh.saturating_sub(3) as usize) {
            match v {
                None => {
                    c.p(yy, bx + 3, k, bold().add_modifier(Modifier::DIM));
                }
                Some(v) => {
                    if !k.is_empty() {
                        c.p(yy, bx + 3, &format!(" {k} "), rev());
                    }
                    c.put(yy, bx + 3 + kw, v, Style::default(), Some((bw - kw - 5) as usize));
                }
            }
            yy += 1;
        }
        c.p(by + bh - 2, bx + 3, "any key to close", dim());
    }

    // ---------- keys ----------

    /// Returns true to quit.
    fn handle(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') && self.ask.is_none() {
            return true;
        }
        if self.ask.is_some() {
            self.handle_ask(key);
            return false;
        }
        if self.help {
            self.help = false;
            return false;
        }
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('m') => {
                self.mute = !self.mute;
                self.say(if self.mute { "notifications off" } else { "notifications on" });
            }
            KeyCode::Char('i') => {
                let r = install::install().map(|r| match r {
                    "installed" => "hook installed",
                    "updated" => "hook updated",
                    _ => "hook already installed",
                });
                self.installed = install::installed();
                self.say(r.unwrap_or("couldn't write ~/.claude/settings.json"));
            }
            KeyCode::Char('u') => {
                self.view = if self.view == View::Usage { View::Sessions } else { View::Usage };
            }
            KeyCode::Char('h') if self.view != View::History => {
                self.view = View::History;
                self.hist_q.clear();
                self.hist_i = 0;
                self.hist = claude::history();
                self.say(format!("{} past sessions", self.hist.len()));
            }
            KeyCode::Esc | KeyCode::Char('h') => {
                if self.view == View::History && !self.hist_q.is_empty() {
                    self.hist_q.clear();
                } else {
                    self.view = View::Sessions;
                }
            }
            _ if self.view == View::History => self.handle_history(key),
            _ if self.view == View::Usage => {}
            _ => self.handle_sessions(key),
        }
        false
    }

    fn start_ask(&mut self, label: &str, default: &str, purpose: Purpose) {
        self.ask = Some(Ask { label: label.into(), buf: default.into(), purpose });
    }

    fn handle_ask(&mut self, key: KeyEvent) {
        let Some(ask) = self.ask.as_mut() else { return };
        match key.code {
            KeyCode::Esc => {
                self.ask = None;
                return;
            }
            KeyCode::Backspace => {
                ask.buf.pop();
                return;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                ask.buf.clear();
                return;
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                ask.buf.push(ch);
                return;
            }
            KeyCode::Enter => {}
            _ => return,
        }
        let Ask { buf, purpose, .. } = self.ask.take().unwrap();
        let text = buf.trim().to_string();
        match purpose {
            Purpose::Search => {
                self.hist_q = text;
                self.hist_i = 0;
            }
            Purpose::Continue(h) => {
                if !text.is_empty() {
                    let (sid, cwd) = (h.sid.clone(), h.cwd.clone());
                    self.background("resuming in the background", move || launch::resume(&sid, cwd.as_deref(), Some(&text)));
                    self.view = View::Sessions;
                }
            }
            Purpose::StartFolder => {
                if !text.is_empty() {
                    self.start_ask("first prompt: ", "", Purpose::StartPrompt(util::expand(&text)));
                }
            }
            Purpose::StartPrompt(folder) => {
                if text.is_empty() {
                    self.say("cancelled — a new session needs a first prompt");
                } else {
                    self.start_ask("new git worktree? (enter = no, y = yes, or type its name): ", "", Purpose::StartWorktree(folder, text));
                }
            }
            Purpose::StartWorktree(folder, prompt) => {
                let wt = match text.to_lowercase().as_str() {
                    "" | "n" | "no" => None,
                    "y" | "yes" => Some(String::new()),
                    _ => Some(text),
                };
                self.background("starting a background session", move || launch::new(&folder, &prompt, wt.as_deref(), None));
            }
            Purpose::Answer(rid) => {
                if !text.is_empty() {
                    if let Some(req) = self.sess.iter().flat_map(|s| s.pending.iter()).find(|r| r.id == rid).cloned() {
                        let qs = req.questions();
                        let mut d = self.drafts.get(&rid).cloned().unwrap_or_default();
                        if let Some(q) = qs.get(d.qi).cloned() {
                            self.record(&req, &mut d, &qs, &q, text);
                            self.drafts.insert(rid, d);
                        }
                    }
                }
            }
        }
    }

    fn handle_history(&mut self, key: KeyEvent) {
        let rows = self.history_rows();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.hist_i = self.hist_i.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.hist_i = (self.hist_i + 1).min(rows.len().saturating_sub(1)),
            KeyCode::Char('/') => {
                let q = self.hist_q.clone();
                self.start_ask("search: ", &q, Purpose::Search);
            }
            KeyCode::Char('o') | KeyCode::Enter if !rows.is_empty() => {
                let h = rows[self.hist_i.min(rows.len() - 1)].clone();
                self.background("opening", move || launch::open_terminal(&format!("claude --resume {}", h.sid), h.cwd.as_deref()));
            }
            KeyCode::Char('p') if !rows.is_empty() => {
                let h = rows[self.hist_i.min(rows.len() - 1)].clone();
                self.start_ask("continue with: ", "", Purpose::Continue(h));
            }
            _ => {}
        }
    }

    fn handle_sessions(&mut self, key: KeyEvent) {
        let s = self.selected();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.sel_i = self.sel_i.saturating_sub(1);
                self.sel_key = None;
                return;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.sel_i += 1;
                self.sel_key = None;
                return;
            }
            KeyCode::Tab => {
                let waiting: Vec<usize> = self.sess.iter().enumerate().filter(|(_, o)| o.status == "needs you").map(|(i, _)| i).collect();
                if waiting.is_empty() {
                    self.say("nothing is waiting on you");
                } else {
                    let nxt = waiting.iter().copied().find(|i| *i > self.sel_i).unwrap_or(waiting[0]);
                    self.sel_i = nxt;
                    self.sel_key = Some(self.sess[nxt].key.clone());
                }
                return;
            }
            KeyCode::Char('A') => {
                let on = !util::auto_ids().contains(util::ALL);
                util::set_auto(util::ALL, on);
                self.say(if on { "auto-yes on for ALL sessions" } else { "auto-yes for all: off" });
                self.refresh();
                return;
            }
            KeyCode::Char('s') => {
                let def = s.as_ref().map(|s| util::short(&s.cwd)).filter(|c| !c.is_empty()).unwrap_or_else(|| util::short(&std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()));
                self.start_ask("start in folder: ", &def, Purpose::StartFolder);
                return;
            }
            _ => {}
        }
        let Some(s) = s else { return };
        if let Some(req) = s.pending.first().cloned() {
            if self.answer_key(key, &req) {
                return;
            }
        }
        match key.code {
            KeyCode::Char('a') => {
                if s.tool != "claude" {
                    self.say("auto-yes works for Claude Code sessions only");
                } else {
                    let sid = s.sid.clone().unwrap_or_default();
                    let ids = util::auto_ids();
                    let on = !s.auto || (ids.contains(util::ALL) && !ids.contains(&sid));
                    util::set_auto(&sid, on);
                    let note = if !on && ids.contains(util::ALL) { " (ALL is still on)" } else { "" };
                    self.say(format!("{}{}{note}", if on { "auto-yes on: " } else { "auto-yes off: " }, s.name));
                    self.refresh();
                }
            }
            KeyCode::Char('o') => {
                if let (true, Some(id)) = (s.bg, s.short.clone()) {
                    let cwd = s.cwd.clone();
                    self.background("opening", move || launch::open_terminal(&format!("claude attach {id}"), Some(&cwd)));
                } else if let Some(pid) = s.pid {
                    self.background("finding its terminal", move || launch::jump(pid));
                }
            }
            KeyCode::Char('x') => match (s.bg, s.short.clone()) {
                (true, Some(id)) => {
                    if self.confirm.as_ref().map(|(k, t)| k == &s.key && now() < *t).unwrap_or(false) {
                        self.confirm = None;
                        self.background(&format!("stopping {}", s.name), move || launch::stop(&id));
                    } else {
                        self.confirm = Some((s.key.clone(), now() + 4.0));
                        self.say(format!("press x again to stop {}", s.name));
                    }
                }
                _ => self.say("only background sessions can be stopped from here"),
            },
            _ => {}
        }
    }

    /// True if the key was used for the pending request.
    fn answer_key(&mut self, key: KeyEvent, req: &claude::Request) -> bool {
        if !req.is_question() {
            let d = match key.code {
                KeyCode::Char('y') => Decision::Allow,
                KeyCode::Char('n') => Decision::Deny { message: None },
                _ => return false,
            };
            let ok = claude::answer(req, &d).is_ok();
            let verb = if d == Decision::Allow { "allowed: " } else { "denied: " };
            self.say(if ok { format!("{verb}{}", claude::tool_summary(&req.tool, &req.input)) } else { "couldn't write the answer".into() });
            return true;
        }
        let qs = req.questions();
        let mut d = self.drafts.get(&req.id).cloned().unwrap_or_default();
        let Some(q) = qs.get(d.qi).cloned() else { return false };
        let opts = q.get("options").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let multi = q.get("multiSelect").and_then(|v| v.as_bool()) == Some(true);
        let label = |i: usize| opts[i].get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let used = match key.code {
            KeyCode::Char(ch @ '1'..='9') if (ch as usize - '1' as usize) < opts.len() => {
                let i = ch as usize - '1' as usize;
                if multi {
                    if !d.multi.remove(&i) {
                        d.multi.insert(i);
                    }
                } else {
                    self.record(req, &mut d, &qs, &q, label(i));
                }
                true
            }
            KeyCode::Enter if multi && !d.multi.is_empty() => {
                let text = d.multi.iter().map(|i| label(*i)).collect::<Vec<_>>().join(", ");
                self.record(req, &mut d, &qs, &q, text);
                true
            }
            KeyCode::Char('t') => {
                self.drafts.insert(req.id.clone(), d);
                self.start_ask("your answer: ", "", Purpose::Answer(req.id.clone()));
                return true;
            }
            _ => false,
        };
        self.drafts.insert(req.id.clone(), d);
        used
    }

    fn record(&mut self, req: &claude::Request, d: &mut Draft, qs: &[Value], q: &Value, text: String) {
        d.answers.insert(q.get("question").and_then(|v| v.as_str()).unwrap_or("").to_string(), Value::String(text));
        d.qi += 1;
        d.multi.clear();
        if d.qi >= qs.len() {
            let ok = claude::answer(req, &Decision::Answers { answers: d.answers.clone() }).is_ok();
            let shown: Vec<String> = d.answers.values().filter_map(|v| v.as_str().map(String::from)).collect();
            self.say(if ok { format!("answered: {}", shown.join("; ")) } else { "couldn't write the answer".into() });
        }
    }
}

pub fn run() -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let mut hub = Hub::new();
    hub.refresh();
    let mut last = now();
    let result = (|| -> std::io::Result<()> {
        loop {
            terminal.draw(|f| hub.draw(f.buffer_mut()))?;
            // wait for input until the next refresh is due, then handle every key that has
            // arrived before drawing again (refreshing per key made typing crawl)
            let wait = Duration::from_secs_f64((1.0 - (now() - last)).clamp(0.0, 1.0));
            let mut acted = false;
            if event::poll(wait)? {
                loop {
                    if let TermEvent::Key(k) = event::read()? {
                        if k.kind == KeyEventKind::Press {
                            if hub.handle(k) {
                                return Ok(());
                            }
                            acted = hub.ask.is_none();
                        }
                    }
                    if !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
            if now() - last >= 1.0 || acted {
                hub.refresh();
                last = now();
            }
        }
    })();
    ratatui::restore();
    result
}
