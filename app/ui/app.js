// Grandaddy: renders what grandaddy-core reports, and sends your answers back.
"use strict";

// ---------- backend ----------
// Every command goes through one entry point (`call`); only window-level ones are separate.
const APP_ONLY = new Set(["show_main", "hide_quick", "quit", "ui_log", "debug"]);
let inflight = 0, progressTimer = null;
function trackProgress(delta) {
  inflight = Math.max(0, inflight + delta);
  clearTimeout(progressTimer);
  // only show the bar for things that take a moment, so quick refreshes don't flicker it
  if (inflight > 0) progressTimer = setTimeout(() => $("#progress").classList.add("on"), 280);
  else $("#progress").classList.remove("on");
}
const raw = (cmd, args = {}) => (APP_ONLY.has(cmd) ? window.__TAURI__.core.invoke(cmd, args) : window.__TAURI__.core.invoke("call", { cmd, args }));
async function invoke(cmd, args = {}, { quiet = false } = {}) {
  if (!quiet) trackProgress(1);
  try {
    return await raw(cmd, args);
  } finally {
    if (!quiet) trackProgress(-1);
  }
}
const poll = (cmd, args = {}) => invoke(cmd, args, { quiet: true }); // background refreshes: no bar

const $ = (s) => document.querySelector(s);
const $$ = (s) => [...document.querySelectorAll(s)];
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const md = (s) => window.markdown(s);
const fmtTokens = (n) => (n == null ? "" : n >= 1e6 ? (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? Math.round(n / 1e3) + "k" : String(n));
const ago = (secs) => {
  const s = Math.max(0, secs);
  return s < 45 ? "now" : s < 3600 ? `${Math.round(s / 60)}m` : s < 86400 ? `${Math.round(s / 3600)}h` : `${Math.round(s / 86400)}d`;
};
const now = () => Date.now() / 1000;
const clock = (ts) => (ts ? new Date(ts * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) : "");
const when = (ts) => {
  if (!ts) return "";
  const d = new Date(ts * 1000), today = new Date();
  return d.toDateString() === today.toDateString() ? clock(ts) : d.toLocaleDateString([], { weekday: "short" }) + " " + clock(ts);
};
const countdown = (secs) => {
  const s = Math.max(0, Math.round(secs));
  if (s < 60) return "under a minute";
  const h = Math.floor(s / 3600), m = Math.round((s % 3600) / 60);
  return h ? `${h}h ${m}m` : `${m}m`;
};
const limitLabel = (k) => ({ five_hour: "5-hour limit", seven_day: "weekly limit", session: "session limit" }[k] || "usage limit");

const GROUPS = [["needs you", "Needs You"], ["working", "Working"], ["limit", "Paused by Limits"], ["idle", "Idle"], ["running", "Running"], ["blocked", "Can't Run"], ["failed", "Failed"]];
const AGENTS = [["claude", "Claude Code"], ["codex", "Codex"], ["antigravity", "Antigravity"], ["gemini", "Gemini CLI"], ["opencode", "OpenCode"], ["kilo", "Kilo"]];

const S = {
  sessions: [], sel: null, view: "sessions", tab: "activity", detail: null, filter: "",
  drafts: {}, form: {}, ans: {}, open: {},
  git: null, gitCwd: null, diff: null, diffPath: null,
  goals: null, loops: null, hist: null, histSel: null, histAct: [], usage: null, agents: null, rules: null, settings: null,
  lastPane: "", busy: false, ticks: 0, booted: false,
};

// ---------- glyphs ----------
const G = {
  spinner: '<svg viewBox="0 0 16 16">' + [...Array(8)].map((_, i) => `<path d="M8 1.6v2.6" transform="rotate(${i * 45} 8 8)" style="opacity:${(0.2 + (i / 7) * 0.8).toFixed(2)}" stroke-width="1.7"/>`).join("") + "</svg>",
  needs: '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="7" fill="currentColor" stroke="none"/><path d="M8 4.4v4.2" stroke="#fff" stroke-width="1.8"/><circle cx="8" cy="11.3" r="1" fill="#fff" stroke="none"/></svg>',
  hourglass: '<svg viewBox="0 0 16 16"><path d="M4 1.8h8M4 14.2h8M5 1.8v1.6c0 1.6 3 3 3 4.6 0-1.6 3-3 3-4.6V1.8M5 14.2v-1.6c0-1.6 3-3 3-4.6 0 1.6 3 3 3 4.6v1.6"/></svg>',
  warn: '<svg viewBox="0 0 16 16"><path d="M8 2.2 14.2 13H1.8z" fill="currentColor" stroke="none"/><path d="M8 6.2v3.2" stroke="#fff" stroke-width="1.6"/><circle cx="8" cy="11.2" r=".85" fill="#fff" stroke="none"/></svg>',
  ring: '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="4.5" stroke-dasharray="2 2.2"/></svg>',
  term: '<svg viewBox="0 0 16 16"><path d="m3 5 3 3-3 3M8 11h5"/></svg>',
  doc: '<svg viewBox="0 0 16 16"><path d="M4 2h5l3 3v9H4z"/><path d="M9 2v3h3"/></svg>',
  pen: '<svg viewBox="0 0 16 16"><path d="m10.5 3 2.5 2.5L6 12.5H3.5V10z"/></svg>',
  find: '<svg viewBox="0 0 16 16"><circle cx="7" cy="7" r="4.5"/><path d="m10.5 10.5 3 3"/></svg>',
  globe: '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6"/><path d="M2 8h12M8 2c2 2 2 10 0 12M8 2c-2 2-2 10 0 12"/></svg>',
  agent: '<svg viewBox="0 0 16 16"><circle cx="8" cy="5.5" r="2.5"/><path d="M3.5 13.5c.8-2.5 2.5-3.5 4.5-3.5s3.7 1 4.5 3.5"/></svg>',
  dot: '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="1.6"/></svg>',
  alert: '<svg viewBox="0 0 16 16"><path d="M8 2.5 14 13H2z"/><path d="M8 6.5v3M8 11.3v.1"/></svg>',
  x: '<svg viewBox="0 0 16 16"><path d="m5 5 6 6M11 5l-6 6"/></svg>',
  check: '<svg viewBox="0 0 16 16"><path d="m3.5 8.5 3 3 6-7"/></svg>',
  folder: '<svg viewBox="0 0 16 16"><path d="M2 4.5h4l1.5 1.5H14v6.5H2z"/></svg>',
  branch: '<svg viewBox="0 0 16 16"><circle cx="5" cy="4" r="1.5"/><circle cx="5" cy="12" r="1.5"/><circle cx="11" cy="6" r="1.5"/><path d="M5 5.5v5M11 7.5c0 2-3 2-5.5 3.5"/></svg>',
  chip: '<svg viewBox="0 0 16 16"><rect x="4" y="4" width="8" height="8" rx="1.5"/><path d="M6 1.5v2.5M10 1.5v2.5M6 12v2.5M10 12v2.5M1.5 6h2.5M1.5 10h2.5M12 6h2.5M12 10h2.5"/></svg>',
  gauge: '<svg viewBox="0 0 16 16"><path d="M2.5 11a5.5 5.5 0 1 1 11 0"/><path d="m8 11 2.5-3.5"/></svg>',
};
/** The one indicator a session shows: a spinner while it works, a badge when it needs you,
 *  an hourglass when a limit paused it, a warning when it can't run, and nothing when idle. */
function statusGlyph(st) {
  if (st === "working") return `<span class="st working" title="Working">${G.spinner}</span>`;
  if (st === "needs you" || st === "asking") return `<span class="st needs" title="Needs you">${G.needs}</span>`;
  if (st === "limit") return `<span class="st limit" title="Paused by a usage limit">${G.hourglass}</span>`;
  if (st === "blocked" || st === "failed") return `<span class="st bad" title="${st === "blocked" ? "Can't run" : "Failed"}">${G.warn}</span>`;
  if (st === "running") return `<span class="st quiet" title="Running">${G.ring}</span>`;
  return `<span class="st"></span>`;
}
const toolIcon = (t = "") => {
  const n = t.toLowerCase();
  if (["bash", "shell", "run_command", "exec_command", "run_shell_command", "local_shell"].includes(n)) return G.term;
  if (["read", "view_file", "read_file"].includes(n)) return G.doc;
  if (/edit|write|patch|replace/.test(n)) return G.pen;
  if (/grep|glob|search|find|list/.test(n) && !n.startsWith("web")) return G.find;
  if (n.startsWith("web") || n.includes("url") || n.includes("browser")) return G.globe;
  if (["task", "agent"].includes(n)) return G.agent;
  return G.dot;
};

// ---------- feedback ----------
let toastTimer;
function toast(msg, err = false) {
  const t = $("#toast");
  t.textContent = msg;
  t.className = "toast glass show" + (err ? " err" : "");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.className = "toast glass" + (err ? " err" : "")), err ? 5000 : 2600);
}
/** Run an action, showing a spinner in the button that started it. */
async function act(promise, okMsg, btn) {
  if (btn) btn.classList.add("busy");
  try {
    const r = await promise;
    const m = okMsg || (typeof r === "string" ? r : r && r.message) || "Done";
    toast(m);
    return r ?? true;
  } catch (e) {
    toast(String(e), true);
    return undefined;
  } finally {
    if (btn) btn.classList.remove("busy");
  }
}
function confirmAlert(title, body, okLabel, destructive = false) {
  return new Promise((resolve) => {
    $("#alert-title").textContent = title;
    $("#alert-body").textContent = body;
    const ok = $("#alert-ok");
    ok.textContent = okLabel;
    ok.className = "btn default" + (destructive ? " destructive" : "");
    $("#alert").hidden = false;
    $("#scrim").hidden = false;
    const done = (v) => {
      $("#alert").hidden = true;
      $("#scrim").hidden = true;
      ok.onclick = $("#alert-cancel").onclick = null;
      document.removeEventListener("keydown", key, true);
      resolve(v);
    };
    const key = (e) => {
      if (e.key === "Escape") { e.preventDefault(); done(false); }
      if (e.key === "Enter") { e.preventDefault(); done(true); }
    };
    ok.onclick = () => done(true);
    $("#alert-cancel").onclick = () => done(false);
    document.addEventListener("keydown", key, true);
    ok.focus();
  });
}

// ---------- data ----------
async function refreshSessions() {
  try {
    S.sessions = await poll("list_sessions");
  } catch (e) {
    S.sessions = [];
  }
  if (S.sel && !S.sessions.some((s) => s.key === S.sel)) S.sel = null;
  if (!S.sel && S.sessions.length) {
    const by = (st) => S.sessions.find((s) => s.status === st);
    S.sel = (by("needs you") || by("working") || by("limit") || by("idle") || S.sessions[0]).key;
  }
}
async function refreshDetail() {
  if (S.view !== "sessions" || !S.sel) return (S.detail = null);
  try {
    S.detail = await poll("session_detail", { key: S.sel });
  } catch {
    S.detail = null;
  }
}
async function refreshView(force = false) {
  const v = S.view;
  const every = (n) => force || S.ticks % n === 0;
  const get = force ? invoke : poll;
  try {
    if (v === "goals" && every(2)) S.goals = await get("goals_list");
    if (v === "loops" && every(2)) S.loops = await get("loops_list");
    if (v === "history" && (force || !S.hist)) S.hist = await get("history");
    if (v === "usage" && (force || !S.usage)) S.usage = await get("usage", { days: 7 });
    if (v === "agents" && (force || !S.agents)) { S.agents = await get("agents"); S.settings = await get("settings"); }
    if (v === "rules" && (force || !S.rules)) S.rules = await get("rules_list");
    if ((v === "settings" && every(3)) || (v === "sessions" && every(10))) S.settings = await get("settings");
    if (v === "sessions" && S.tab === "git" && S.detail && (force || S.gitCwd !== S.detail.session.cwd)) await loadGit();
  } catch (e) {
    toast(String(e), true);
  }
}
async function loadGit() {
  const s = selected();
  if (!s || !s.cwd) return;
  if (S.gitCwd !== s.cwd) { S.diff = null; S.diffPath = null; S.git = null; }
  S.gitCwd = s.cwd;
  try {
    S.git = await invoke("git_overview", { cwd: s.cwd });
  } catch (e) {
    S.git = { error: String(e) };
  }
}
async function tick() {
  if (S.busy || !S.booted) return;
  S.busy = true;
  try {
    await refreshSessions();
    await refreshDetail();
    await refreshView();
    render();
  } finally {
    S.busy = false;
    S.ticks++;
  }
}
function render() {
  renderSidebar();
  renderToolbar();
  renderPane();
  renderComposer();
}

// ---------- launch ----------
async function boot() {
  const steps = [
    ["Finding your sessions…", async () => { await refreshSessions(); await refreshDetail(); }],
    ["Reading settings…", async () => { S.settings = await poll("settings").catch(() => null); }],
    ["Checking your agents…", async () => { S.agents = await poll("agents").catch(() => null); }],
  ];
  const bar = $("#splash-bar");
  for (let i = 0; i < steps.length; i++) {
    $("#splash-step").textContent = steps[i][0];
    bar.style.width = `${Math.round(((i + 0.4) / steps.length) * 100)}%`;
    await steps[i][1]();
  }
  bar.style.width = "100%";
  $("#splash-step").textContent = S.sessions.length ? `${S.sessions.length} session${S.sessions.length === 1 ? "" : "s"}` : "Ready";
  S.booted = true;
  render();
  // GRANDADDY_DEBUG=1: report what loaded, so a build can be checked without looking at it
  raw("debug").then((on) => on && raw("ui_log", { level: "info", msg: `main window: ${S.sessions.length} sessions, settings ${S.settings ? "ok" : "MISSING"}, agents ${S.agents ? S.agents.length : "MISSING"}, selected ${S.sel || "none"}, markdown ${typeof window.markdown}` })).catch(() => {});
  setTimeout(() => $("#splash").classList.add("done"), 260);
  setTimeout(() => $("#splash")?.remove(), 900);
}

// ---------- sidebar ----------
function renderSidebar() {
  const q = S.filter.toLowerCase();
  const list = S.sessions.filter((s) => !q || `${s.name} ${s.folder} ${s.tool_label}`.toLowerCase().includes(q));
  let html = "";
  for (const [st, title] of GROUPS) {
    const rows = list.filter((s) => s.status === st);
    if (!rows.length) continue;
    html += `<div class="section-title"><span>${title}</span><span class="count">${rows.length}</span></div>` + rows.map(srow).join("");
  }
  const other = list.filter((s) => !GROUPS.some(([g]) => g === s.status));
  if (other.length) html += `<div class="section-title"><span>Other</span></div>` + other.map(srow).join("");
  if (!html) html = `<div class="empty-side">${q ? "No sessions match." : "No sessions running."}</div>`;
  if ($("#groups").innerHTML !== html) $("#groups").innerHTML = html;
  $$(".row-btn").forEach((b) => b.classList.toggle("sel", b.dataset.view === S.view));
}
function srow(s) {
  const agent = s.tool === "claude" && s.surface === "CLI" ? "" : s.surface === "CLI" ? s.tool_label : `${s.tool_label} ${s.surface}`;
  let meta = [agent, s.folder].filter(Boolean).join(" · ");
  if (s.status === "limit" && s.limit && s.limit.resets_at) meta = `Resets ${when(s.limit.resets_at)}${agent ? " · " + agent : ""}`;
  return `<button class="srow ${S.view === "sessions" && s.key === S.sel ? "sel" : ""}" data-key="${esc(s.key)}">
    ${statusGlyph(s.status)}<span class="name">${esc(s.name)}</span>
    <span class="when">${esc(s.ago)}</span><span class="meta">${esc(meta)}</span></button>`;
}
const selected = () => S.sessions.find((s) => s.key === S.sel);

function renderToolbar() {
  const s = S.view === "sessions" ? selected() : null;
  let title = "", sub = "";
  if (s) {
    const st = { "needs you": "needs you", limit: "paused by a usage limit" }[s.status] || s.status;
    title = s.name;
    sub = [s.tool_label + (s.surface !== "CLI" ? ` ${s.surface}` : ""), st].join(" · ");
  } else if (S.view === "sessions") title = "Grandaddy";
  $("#title").textContent = title;
  $("#subtitle").textContent = sub;
  $("#tabs").hidden = !s;
  $$("#tabs button").forEach((b) => b.classList.toggle("on", b.dataset.tab === S.tab));
  placeLens();
  $("#session-tools").hidden = !s;
  if (!s) return;
  $("#btn-open").dataset.tip = s.bg ? "Attach in a terminal" : "Show its window";
  $("#btn-auto").hidden = !s.sid || s.answer === "none";
  $("#btn-auto").classList.toggle("on", !!s.auto);
  $("#btn-auto").dataset.tip = s.auto ? "Auto-yes is on" : "Auto-yes";
  $("#btn-stop").hidden = !(s.bg && s.short);
}
/** Slide the segmented control's lens under the selected segment. */
function placeLens() {
  const seg = $("#tabs");
  if (seg.hidden) return;
  const on = seg.querySelector("button.on");
  const lens = seg.querySelector(".lens");
  if (!on || !lens) return;
  lens.style.width = on.offsetWidth + "px";
  lens.style.transform = `translateX(${on.offsetLeft - 3}px)`;
}

// ---------- pane ----------
function renderPane() {
  const pane = $("#pane");
  const a = document.activeElement;
  // never redraw under someone typing (an answer, a commit message, a goal)
  if (pane.contains(a) && a.matches("input:not([type=checkbox]), textarea, select")) return;
  const views = { sessions: sessionPane, goals: goalsPane, loops: loopsPane, history: historyPane, usage: usagePane, agents: agentsPane, rules: rulesPane, settings: settingsPane };
  const html = (views[S.view] || sessionPane)();
  if (html === S.lastPane) return;
  const viewKey = `${S.view}:${S.view === "sessions" ? S.sel + S.tab : ""}`;
  const fresh = viewKey !== S.lastViewKey;
  S.lastPane = html;
  S.lastViewKey = viewKey;
  const keep = pane.scrollTop;
  const atBottom = pane.scrollHeight - pane.scrollTop - pane.clientHeight < 60;
  pane.innerHTML = html;
  if (fresh) pane.querySelector(".page")?.classList.add("enter");
  const follow = S.view === "sessions" && S.tab === "activity";
  pane.scrollTop = follow && (atBottom || S.jumpToEnd) ? pane.scrollHeight : fresh ? 0 : keep;
  S.jumpToEnd = false;
}
function repaint() {
  S.lastPane = "";
  renderPane();
}
const field = (name, attrs = "") => `data-f="${name}" ${attrs}`;
const fval = (name, dflt = "") => esc(S.form[name] ?? dflt);
const skeleton = (n = 3) => `<div class="page"><div class="skel skel-block"></div>${[...Array(n)].map((_, i) => `<div class="skel skel-line" style="width:${[92, 74, 86, 60, 80][i % 5]}%"></div>`).join("")}<div class="skel skel-block"></div></div>`;

// ---------- session: activity ----------
function sessionPane() {
  const s = selected();
  if (!s) {
    return `<div class="empty"><div><img src="assets/watching.png" alt="">
      <h2>${S.sessions.length ? "Choose a session" : "Nothing running"}</h2>
      <p>Sessions from Claude Code, Codex, Antigravity, Cursor, Gemini, OpenCode and Kilo appear here as soon as they start.</p>
      <button class="btn default large" data-act="new">New Session</button></div></div>`;
  }
  return S.tab === "git" ? gitPane(s) : activityPane(s);
}

function activityPane(s) {
  const d = S.detail && S.detail.session.key === s.key ? S.detail : null;
  if (!d) return skeleton(5);
  const repo = d.repo;
  const facts = [
    d.model && [G.chip, d.model],
    [G.folder, s.folder],
    repo && repo.branch && [G.branch, `${repo.branch}${repo.changes.length ? ` · ${repo.changes.length} changed` : ""}`],
    d.context && [G.gauge, `${fmtTokens(d.context)} context`],
  ].filter(Boolean);
  let h = `<div class="page"><div class="facts selectable">${facts.map(([i, t]) => `<span class="fact">${i}${esc(t)}</span>`).join("")}${s.auto ? '<span class="tag green">Auto-yes on</span>' : ""}</div>`;

  if (s.limit) h += limitCard(s);
  const notices = [];
  for (const sig of d.signals || []) notices.push(sig === "looping?" ? "The last few steps were identical. It may be stuck in a loop." : `No output for ${sig.replace("quiet ", "")}.`);
  const files = Object.entries(d.conflicts || {});
  if (files.length) notices.push(`Also edited by another session: ${files.slice(0, 3).map(([f, who]) => `${f.split("/").pop()} (${who.join(", ")})`).join(", ")}${files.length > 3 ? "…" : ""}`);
  if ((d.twins || []).length) notices.push(`Same folder as ${d.twins.join(", ")}.`);
  if (s.answer === "none" && !["idle", "limit"].includes(s.status)) notices.push(`${s.tool_label} has no approval hook, so answer its prompts in its own window.`);
  h += notices.map((n) => `<div class="notice">${G.alert}<div class="grow">${esc(n)}</div></div>`).join("");

  h += (s.pending || []).map((r) => requestCard(s, r)).join("");
  h += transcript(d.events || [], s.key);
  return h + "</div>";
}

function limitCard(s) {
  const l = s.limit;
  const left = l.resets_at ? l.resets_at - now() : null;
  const on = !!(S.settings && S.settings.autoContinue && S.settings.autoContinue.enabled);
  const skipped = /Won't continue/.test(s.limit_plan || "");
  return `<div class="limit-card"><span class="hg">${G.hourglass}</span>
    <div class="grow"><div class="t">Hit the ${esc(limitLabel(l.kind))}</div><div class="s">${esc(s.limit_plan || "")}</div>
    <div style="margin-top:9px;display:flex;gap:6px">${on
      ? `<button class="btn small glass" data-act="continue-skip" data-key="${esc(s.key)}" data-skip="${skipped ? "0" : "1"}">${skipped ? "Continue this one" : "Don't continue this one"}</button>`
      : `<button class="btn small glass" data-act="goto-settings">Turn on auto-continue</button>`}</div></div>
    ${l.resets_at ? `<div class="when">${esc(clock(l.resets_at))}<small>${left > 0 ? `in ${countdown(left)}` : "reset"}</small></div>` : ""}</div>`;
}

function reqText(r) {
  const i = r.input || {};
  if (r.tool === "Bash") return i.command || "";
  if (["Edit", "Write", "MultiEdit", "NotebookEdit", "Read"].includes(r.tool)) return i.file_path || i.notebook_path || "";
  if (r.tool === "WebFetch") return i.url || "";
  if (r.tool === "WebSearch") return i.query || "";
  if (r.tool === "ExitPlanMode") return i.plan || "";
  return JSON.stringify(i, null, 2);
}
function reqTitle(r) {
  if (r.tool === "Bash") return "Wants to run";
  if (["Edit", "Write", "MultiEdit", "NotebookEdit"].includes(r.tool)) return "Wants to change a file";
  if (r.tool === "WebFetch" || r.tool === "WebSearch") return "Wants to go online";
  if (r.tool === "ExitPlanMode") return "Has a plan for you";
  if (r.tool.startsWith("mcp__")) return `Wants to use ${r.tool.split("__").slice(1).join(" › ")}`;
  return `Wants to use ${r.tool}`;
}

function requestCard(s, r) {
  const ts = `<span class="ago">${ago(now() - r.created)}</span>`;
  const from = r.agent && r.agent !== "claude" ? ` · ${esc(s.tool_label)}` : "";
  if (r.tool !== "AskUserQuestion") {
    const body = r.tool === "ExitPlanMode" ? `<div class="say md selectable" style="margin:8px 16px 0">${md(reqText(r))}</div>` : `<pre class="selectable">${esc(reqText(r))}</pre>`;
    const why = r.input && r.input.description ? `<div class="why">${esc(r.input.description)}</div>` : "";
    return `<div class="request"><div class="request-head">${statusGlyph("needs you")}${esc(reqTitle(r))}${from}${ts}</div>${body}${why}
      <div class="buttons"><button class="btn large" data-act="deny" data-rid="${esc(r.id)}">Deny</button>
      <button class="btn default large" data-act="allow" data-rid="${esc(r.id)}">Allow</button></div></div>`;
  }
  const qs = (r.input && r.input.questions) || [];
  const a = S.ans[r.id] || (S.ans[r.id] = { pick: {}, own: {} });
  let h = `<div class="request"><div class="request-head">${statusGlyph("needs you")}${qs.length > 1 ? `${qs.length} questions` : "A question"}${from}${ts}</div>`;
  qs.forEach((q, qi) => {
    const multi = !!q.multiSelect;
    const picked = a.pick[qi] || [];
    h += `<div class="q"><div class="qtext selectable">${esc(q.question)}</div>`;
    (q.options || []).forEach((o, oi) => {
      h += `<button class="opt ${multi ? "multi" : ""} ${picked.includes(oi) ? "on" : ""}" data-act="pick" data-rid="${esc(r.id)}" data-q="${qi}" data-o="${oi}">
        <span class="radio"></span><span>${esc(o.label)}${o.description ? `<span class="od">${esc(o.description)}</span>` : ""}</span></button>`;
    });
    h += `<input class="own" placeholder="Or answer in your own words" data-own="${esc(r.id)}:${qi}" value="${esc(a.own[qi] || "")}"></div>`;
  });
  const ready = qs.every((_, qi) => (a.pick[qi] || []).length || (a.own[qi] || "").trim());
  h += `<div class="buttons"><button class="btn default large" data-act="answer" data-rid="${esc(r.id)}" ${ready ? "" : "disabled"}>${qs.length > 1 ? "Send Answers" : "Send Answer"}</button></div></div>`;
  return h;
}

/** Your prompts as bubbles, replies as rendered Markdown, the steps in between folded up. */
function transcript(events, key) {
  if (!events.length) return `<div class="empty" style="min-height:40vh"><div><p>Nothing to show yet.</p></div></div>`;
  const turns = [];
  let cur = null;
  for (const e of events) {
    if (e.k === "you" || !cur) turns.push((cur = { you: e.k === "you" ? e : null, items: [] }));
    if (e.k !== "you") cur.items.push(e);
  }
  let h = `<div class="transcript">`;
  if (events.length >= 150) h += `<div class="divider">Earlier activity is in the session itself</div>`;
  turns.slice(-30).forEach((t, ti) => {
    h += `<div class="turn">`;
    if (t.you) h += `<div class="you selectable">${esc(t.you.text)}</div><div class="you-meta">${esc(clock(t.you.ts))}</div>`;
    let run = [];
    const flush = (i) => {
      if (!run.length) return;
      const id = `${key}:${ti}:${i}`;
      const open = S.open[id];
      const shown = open || run.length <= 4 ? run : run.slice(-3);
      h += `<div class="steps">`;
      if (shown.length < run.length) h += `<button class="acts-more" data-act="more" data-id="${esc(id)}">${run.length - shown.length} more steps</button>`;
      h += shown.map(actLine).join("") + `</div>`;
      run = [];
    };
    t.items.forEach((e, i) => {
      if (e.k === "tool" || e.k === "error") run.push(e);
      else {
        flush(i);
        if (e.k === "text") h += `<div class="say md selectable">${md(e.text)}</div>`;
        else if (e.k === "limit") h += `<div class="limit-line">${G.hourglass}<span>${esc(e.text)}</span></div>`;
        else h += `<div class="divider">${esc(e.text)}</div>`;
      }
    });
    flush("end");
    h += `</div>`;
  });
  return h + `</div>`;
}
const actLine = (e) =>
  e.k === "error"
    ? `<div class="act err">${G.alert}<span class="ta selectable">${esc(e.text)}</span></div>`
    : `<div class="act">${toolIcon(e.tool)}<span class="tn">${esc(e.tool || "")}</span><span class="ta">${esc(e.desc || e.text)}</span></div>`;

// ---------- composer ----------
function renderComposer() {
  const s = S.view === "sessions" && S.tab === "activity" ? selected() : null;
  $("#composer").hidden = !s;
  if (!s) return;
  const ta = $("#composer-text");
  if (S.composerKey !== s.key) {
    S.composerKey = s.key;
    ta.value = S.drafts[s.key] || "";
    autosize();
  }
  ta.placeholder = `Message ${s.name}`;
  const d = S.detail && S.detail.session.key === s.key ? S.detail : null;
  let hint = "";
  if (d && !d.typable) {
    hint = s.tool === "claude" && s.bg
      ? "Background session · sending resumes it with your message"
      : "Sending copies your message and brings its window forward · start agents with grandaddy run to send straight in";
  }
  $("#composer-hint").textContent = hint;
  $("#composer-send").disabled = !ta.value.trim();
}
function autosize() {
  const ta = $("#composer-text");
  ta.style.height = "auto";
  ta.style.height = Math.min(ta.scrollHeight, 150) + "px";
}
$("#composer-text").addEventListener("input", (e) => {
  if (S.sel) S.drafts[S.sel] = e.target.value;
  $("#composer-send").disabled = !e.target.value.trim();
  autosize();
});
$("#composer-text").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    $("#composer").requestSubmit();
  }
});
$("#composer").addEventListener("submit", async (e) => {
  e.preventDefault();
  const s = selected();
  const text = $("#composer-text").value.trim();
  if (!s || !text) return;
  const btn = $("#composer-send");
  btn.disabled = true;
  btn.classList.add("busy");
  let r;
  if (s.bg && s.sid && !(S.detail && S.detail.typable)) r = await act(invoke("resume_session", { sid: s.sid, cwd: s.cwd, prompt: text }), `Sent to ${s.name}`);
  else r = await act(invoke("send_prompt", { key: s.key, text, focus: false }));
  btn.classList.remove("busy");
  if (r) {
    $("#composer-text").value = "";
    delete S.drafts[s.key];
    autosize();
    S.jumpToEnd = true;
    setTimeout(tick, 800);
  }
  btn.disabled = !$("#composer-text").value.trim();
});

// ---------- session: git ----------
function gitPane(s) {
  const g = S.git && S.gitCwd === s.cwd ? S.git : null;
  if (!g) return skeleton(6);
  if (g.error) return `<div class="empty"><div><h2>No repository here</h2><p>${esc(g.error)}</p></div></div>`;
  const r = g.repo;
  const sync = !r.upstream ? "Not on GitHub yet" : r.ahead && r.behind ? `${r.ahead} to push · ${r.behind} to pull` : r.ahead ? `${r.ahead} to push` : r.behind ? `${r.behind} to pull` : "Up to date";
  let h = `<div class="page"><div class="repo-line"><div><div class="branch selectable">${esc(r.branch || "detached")}</div><div class="sync">${esc(sync)}</div></div>
    <div class="btns"><button class="btn glass" data-act="git-refresh">Refresh</button>
    <button class="btn glass" data-act="git" data-op="pull" ${r.upstream ? "" : "disabled"}>Pull</button>
    <button class="btn default" data-act="git" data-op="push">${r.upstream ? "Push" : "Publish Branch"}</button></div></div>`;

  h += `<h2 class="group-title">Pull Request</h2>`;
  if (g.pr) {
    const p = g.pr;
    const review = { APPROVED: ["green", "Approved"], CHANGES_REQUESTED: ["red", "Changes requested"], REVIEW_REQUIRED: ["", "Review required"] }[p.review] || ["", "No reviews yet"];
    const checks = { passing: ["green", "Checks passing"], failing: ["red", "Checks failing"], pending: ["orange", "Checks running"], none: ["", "No checks"] }[p.checks];
    h += `<div class="group"><div class="cell"><div class="grow"><div class="t selectable">#${p.number} ${esc(p.title)}</div>
      <div class="s" style="display:flex;gap:6px;flex-wrap:wrap;margin-top:7px"><span class="tag">${esc(p.branch)} → ${esc(p.base)}</span><span class="tag ${checks[0]}">${checks[1]}</span><span class="tag ${review[0]}">${review[1]}</span>${p.draft ? '<span class="tag">Draft</span>' : ""}<span class="tag">+${p.additions} −${p.deletions}</span></div></div>
      <button class="btn glass" data-act="url" data-url="${esc(p.url)}">View on GitHub</button></div>
      <div class="inline-form"><span class="grow muted">Merge into ${esc(p.base)}</span>
      <select ${field("mergeMethod")}><option value="squash" ${!["merge", "rebase"].includes(S.form.mergeMethod) ? "selected" : ""}>Squash and merge</option>
      <option value="merge" ${S.form.mergeMethod === "merge" ? "selected" : ""}>Create a merge commit</option><option value="rebase" ${S.form.mergeMethod === "rebase" ? "selected" : ""}>Rebase and merge</option></select>
      <label class="check"><input type="checkbox" ${field("mergeDelete")} ${S.form.mergeDelete !== false ? "checked" : ""}> Delete branch</label>
      <button class="btn default" data-act="merge" data-n="${p.number}" ${p.draft ? "disabled" : ""}>Merge</button></div></div>`;
  } else if (r.github && r.default_branch && r.branch === r.default_branch) {
    h += `<p class="footnote">You're on ${esc(r.branch)}, the branch pull requests go into. Create a branch below for this work, then open a pull request from it.</p>`;
  } else if (r.github) {
    h += `<div class="group"><div class="inline-form"><input ${field("prTitle")} value="${fval("prTitle")}" placeholder="Title (leave empty to use the commits)">
      <label class="check"><input type="checkbox" ${field("prDraft")} ${S.form.prDraft ? "checked" : ""}> Draft</label>
      <button class="btn default" data-act="pr">Create Pull Request</button></div></div>
      <p class="footnote">Pushes ${esc(r.branch)} if needed, then opens the pull request on ${esc(r.github)}.</p>`;
  } else {
    h += `<p class="footnote">Pull requests need a GitHub remote and the gh command (brew install gh, then gh auth login).</p>`;
  }

  h += `<h2 class="group-title">Changes <span class="aside">${r.changes.length ? `${r.changes.length} file${r.changes.length > 1 ? "s" : ""}` : ""}</span></h2>`;
  if (!r.changes.length) h += `<p class="footnote">Nothing to commit.</p>`;
  else {
    const letter = { modified: "M", added: "A", untracked: "U", deleted: "D", renamed: "R", conflicted: "!" };
    h += `<div class="group">` + r.changes.slice(0, 200).map((c) =>
      `<div class="cell clickable ${S.diffPath === c.path ? "sel" : ""}" data-act="diff" data-path="${esc(c.path)}"><span class="kind ${c.kind}" title="${c.kind}">${letter[c.kind] || "?"}</span>
       <span class="path grow">${esc(c.path)}</span>${c.staged ? `<span class="tag">staged</span>` : ""}</div>`).join("") +
      `<div class="inline-form"><input ${field("commitMsg")} value="${fval("commitMsg")}" placeholder="Commit message"><button class="btn default" data-act="commit">Commit All</button></div></div>`;
    if (S.diffPath && S.diff != null) h += diffView(S.diff);
  }

  h += `<h2 class="group-title">Branches</h2><div class="group">` + (g.branches || []).slice(0, 15).map((b) =>
    `<div class="cell ${b.current ? "" : "clickable"}" ${b.current ? "" : `data-act="switch" data-branch="${esc(b.name)}"`}>
      <span style="width:16px;color:var(--accent)">${b.current ? G.check : ""}</span>
      <div class="grow"><div class="t">${esc(b.name)}</div><div class="s">${esc(b.subject)}</div></div><span class="trail">${ago(now() - b.when)}</span></div>`).join("") +
    `<div class="inline-form"><input ${field("newBranch")} value="${fval("newBranch")}" placeholder="new-branch-name"><button class="btn glass" data-act="branch">Create Branch</button></div></div>`;

  h += `<h2 class="group-title">Recent Commits</h2><div class="group selectable">` + (g.log || []).map((c) =>
    `<div class="cell"><code class="faint">${esc(c.hash)}</code><div class="grow">${esc(c.subject)}<div class="s">${esc(c.author)}</div></div><span class="trail">${ago(now() - c.when)}</span></div>`).join("") + `</div>`;
  return h + `</div>`;
}
function diffView(text) {
  if (!text.trim()) return `<p class="footnote">No textual changes.</p>`;
  const lines = text.split("\n").slice(0, 3000);
  return `<div class="diff selectable">${lines.map((l) => {
    const c = l.startsWith("@@") ? "hunk" : l.startsWith("+") && !l.startsWith("+++") ? "add" : l.startsWith("-") && !l.startsWith("---") ? "del" : "";
    if (/^(diff --git|index |\+\+\+|---)/.test(l)) return "";
    return `<div class="${c}">${esc(l) || " "}</div>`;
  }).join("")}</div>`;
}

// ---------- goals ----------
function goalsPane() {
  if (S.goals == null) return skeleton(4);
  const s = selected();
  const cwd = S.form.goalCwd ?? (s ? s.cwd : "");
  const agent = S.form.goalAgent || "claude";
  let h = `<div class="page"><h1 class="page-title">Goals</h1>
    <p class="lead">Describe the outcome you want. The agent asks what it needs to know first, then works on its own and checks with you before any major decision, here and on your phone.</p>
    <div class="group"><div class="form-cell">
      <label>Goal</label><textarea rows="3" ${field("goalText")} placeholder="Add CSV export to the reports page, with tests">${fval("goalText")}</textarea>
      <label>Folder</label><input ${field("goalCwd")} value="${esc(cwd)}" spellcheck="false" placeholder="~/code/project">
      <label>Agent</label><select ${field("goalAgent")}>${AGENTS.map(([v, l]) => `<option value="${v}" ${v === agent ? "selected" : ""}>${l}</option>`).join("")}</select>
      <label>Model</label><input ${field("goalModel")} value="${fval("goalModel")}" placeholder="Default" spellcheck="false">
      ${agent === "claude" ? `<span></span><label class="check"><input type="checkbox" ${field("goalTerminal")} ${S.form.goalTerminal ? "checked" : ""}> Open in a terminal window instead of the background</label>` : ""}
    </div><div class="actions"><button class="btn default large" data-act="goal-start">Start Goal</button></div></div>
    <p class="footnote">${agent === "claude" ? "Claude Code's questions come to Grandaddy and your phone." : "This agent asks its questions in its own window, which opens in your terminal."}</p>`;
  if (S.goals.length) {
    h += `<h2 class="group-title">Your Goals</h2><div class="group">` + S.goals.map((g) => {
      const live = g.session_key && S.sessions.some((x) => x.key === g.session_key);
      const tag = { asking: ["orange", "Asking questions"], working: ["", "Working"], "needs you": ["orange", "Needs you"], done: ["green", "Done"], stopped: ["", "Stopped"] }[g.status] || ["", g.status];
      return `<div class="cell ${live ? "clickable" : ""}" ${live ? `data-act="goto" data-key="${esc(g.session_key)}"` : ""}>
        ${statusGlyph(g.status === "asking" ? "needs you" : g.status)}<div class="grow"><div class="t">${esc(g.text)}</div>
        <div class="s">${esc(AGENTS.find(([v]) => v === g.tool)?.[1] || g.tool)} · ${esc(g.cwd.replace(/^\/Users\/[^/]+/, "~"))}</div>
        ${g.summary ? `<div class="s selectable">${esc(g.summary)}</div>` : ""}</div>
        <span class="tag ${tag[0]}">${tag[1]}</span>
        <button class="tool" data-act="goal-rm" data-id="${esc(g.id)}" title="Remove from the list">${G.x}</button></div>`;
    }).join("") + `</div>`;
  }
  return h + `</div>`;
}

// ---------- loops ----------
function loopsPane() {
  if (S.loops == null) return skeleton(4);
  const target = S.form.loopTarget ?? (S.sel || "");
  let h = `<div class="page"><h1 class="page-title">Loops</h1>
    <p class="lead">Send the same prompt on a schedule: check CI and fix what's red every half hour, or summarise the day's changes each evening. A loop waits while its session is busy.</p>
    <div class="group"><div class="form-cell">
      <label>Prompt</label><textarea rows="2" ${field("loopPrompt")} placeholder="Run the test suite and fix anything that fails">${fval("loopPrompt")}</textarea>
      <label>Every</label><input ${field("loopEvery")} value="${fval("loopEvery", "30m")}" style="width:100px" spellcheck="false">
      <label>Send to</label><select ${field("loopTarget")}><option value="" ${!target ? "selected" : ""}>A new Claude session each time</option>
        ${S.sessions.map((s) => `<option value="${esc(s.key)}" ${s.key === target ? "selected" : ""}>${esc(s.name)} · ${esc(s.tool_label)}</option>`).join("")}</select>
      ${!target ? `<label>Folder</label><input ${field("loopCwd")} value="${fval("loopCwd")}" placeholder="~/code/project" spellcheck="false">` : ""}
      <label>Stop when</label><input ${field("loopUntil")} value="${fval("loopUntil")}" placeholder="the reply contains… (optional)">
      <label>At most</label><input ${field("loopMax")} value="${fval("loopMax")}" placeholder="unlimited" style="width:100px">
    </div><div class="actions"><button class="btn default large" data-act="loop-add">Add Loop</button></div></div>
    <p class="footnote">Loops run while Grandaddy is open (it keeps running in the menu bar) or under grandaddy daemon.</p>`;
  if (S.loops.length) {
    h += `<h2 class="group-title">Scheduled</h2><div class="group">` + S.loops.map((l) => {
      const to = l.session ? (S.sessions.find((x) => x.key === l.session)?.name || "an ended session") : `new session in ${(l.cwd || "").replace(/^\/Users\/[^/]+/, "~")}`;
      const every = l.every % 86400 === 0 ? `${l.every / 86400}d` : l.every % 3600 === 0 ? `${l.every / 3600}h` : `${Math.round(l.every / 60)}m`;
      const next = l.enabled ? (l.next_run - now() > 30 ? `next in ${countdown(l.next_run - now())}` : "due now") : "paused";
      return `<div class="cell"><div class="grow"><div class="t">${esc(l.prompt)}</div>
        <div class="s">Every ${every} → ${esc(to)} · ${next} · ran ${l.runs}×${l.max_runs ? ` of ${l.max_runs}` : ""}</div>
        ${l.last_result ? `<div class="s">Last: ${esc(l.last_result)}</div>` : ""}</div>
        <label class="switch" title="${l.enabled ? "Pause" : "Resume"}"><input type="checkbox" data-act="loop-toggle" data-id="${esc(l.id)}" ${l.enabled ? "checked" : ""}><i></i></label>
        <button class="tool" data-act="loop-rm" data-id="${esc(l.id)}" title="Delete">${G.x}</button></div>`;
    }).join("") + `</div>`;
  }
  return h + `</div>`;
}

// ---------- history ----------
function historyPane() {
  if (!S.hist) return skeleton(8);
  const q = (S.form.histQ || "").toLowerCase();
  const rows = S.hist.filter((p) => !q || `${p.prompt || ""} ${p.folder}`.toLowerCase().includes(q)).slice(0, 80);
  let h = `<div class="page"><h1 class="page-title">History</h1><p class="lead">Every Claude Code session on this Mac. Pick one up where it left off.</p>
    <input type="search" ${field("histQ")} value="${fval("histQ")}" placeholder="Search prompts and folders" style="width:100%;margin-bottom:16px">`;
  if (!rows.length) return h + `<p class="muted">Nothing matches.</p></div>`;
  h += `<div class="group">` + rows.map((p) => {
    const open = S.histSel === p.sid;
    let row = `<div class="cell clickable ${open ? "sel" : ""}" data-act="hist" data-sid="${esc(p.sid)}"><div class="grow">
      <div class="t">${esc((p.prompt || "(no prompt)").slice(0, 140))}</div><div class="s">${esc(p.folder)}</div></div><span class="trail">${esc(p.ago)}</span></div>`;
    if (open) {
      row += `<div class="cell" style="display:block">${S.histAct === null ? '<div class="skel skel-line" style="width:70%"></div><div class="skel skel-line" style="width:50%"></div>' : transcript(S.histAct.slice(-40), "h:" + p.sid)}
        <div class="actions" style="padding:10px 0 4px"><button class="btn glass" data-act="hist-term" data-sid="${esc(p.sid)}">Open in Terminal</button>
        <button class="btn default" data-act="hist-resume" data-sid="${esc(p.sid)}">Resume in Background</button></div></div>`;
    }
    return row;
  }).join("") + `</div>`;
  return h + `</div>`;
}

// ---------- usage ----------
function usagePane() {
  const u = S.usage;
  if (!u) return skeleton(4);
  const days = [...u.days].reverse();
  const max = Math.max(1, ...days.map((d) => d.total));
  const total = days.reduce((a, d) => a + d.total, 0);
  const today = u.days[0];
  let h = `<div class="page"><h1 class="page-title">Usage</h1>
    <p class="lead">Claude Code used ${fmtTokens(total)} tokens this week, ${fmtTokens(today ? today.total : 0)} of them today. Most of it is usually cache reads, which cost far less than new input.</p>
    <div class="group"><div class="bars">${days.map((d, i) => `<div class="bar-col ${i === days.length - 1 ? "today" : ""}" title="${fmtTokens(d.in)} in · ${fmtTokens(d.out)} out · ${fmtTokens(d.cache)} cache">
      <b>${fmtTokens(d.total)}</b><i style="height:${Math.max(1.5, (d.total / max) * 100)}%;animation-delay:${i * 40}ms"></i>
      <span>${new Date(d.day + "T12:00").toLocaleDateString([], { weekday: "short" })}</span></div>`).join("")}</div><div style="height:14px"></div></div>`;
  if (u.today.length) h += `<h2 class="group-title">Today by Project</h2><div class="group">` + u.today.slice(0, 12).map((p) => `<div class="cell"><div class="grow">${esc(p.project)}</div><span class="trail">${fmtTokens(p.total)}</span></div>`).join("") + `</div>`;
  return h + `</div>`;
}

// ---------- agents ----------
const sw = (attrs, on) => `<label class="switch"><input type="checkbox" ${attrs} ${on ? "checked" : ""}><i></i></label>`;
function agentsPane() {
  if (!S.agents) return skeleton(6);
  const mesh = S.settings && S.settings.mesh;
  let h = `<div class="page"><h1 class="page-title">Agents</h1>
    <p class="lead">Grandaddy shows sessions from every agent below. Connect approvals so their permission prompts follow your rules and auto-yes, and can be answered here or on your phone.</p>
    <div class="group">` + S.agents.map((a) => {
      const status = !a.detected ? ["", "Not on this Mac"] : !a.hookable ? ["", "Sessions only"] : a.hook ? ["green", "Connected"] : ["", "Not connected"];
      let row = `<div class="cell"><div class="grow"><div class="t">${esc(a.label)}</div>${a.hook && a.note ? `<div class="s">${esc(a.note)}</div>` : !a.hookable && a.detected ? `<div class="s">It has no approval hook, so its prompts are answered in its own window.</div>` : ""}</div>
        <span class="tag ${status[0]}">${status[1]}</span>`;
      if (a.hookable) row += sw(`data-act="hook" data-tool="${a.tool}" ${a.detected ? "" : "disabled"}`, a.hook);
      row += `</div>`;
      if (a.hookable && a.hook && a.tool !== "claude") {
        row += `<div class="cell"><div class="grow"><div class="s">Hold ${esc(a.label)}'s prompts for an answer from Grandaddy or your phone. Its own prompt appears if you don't answer within ${S.settings ? S.settings.remoteTimeout : 120}s.</div></div>
          ${sw(`data-act="remote" data-tool="${a.tool}"`, a.remote)}</div>`;
      }
      return row;
    }).join("") + `</div>`;
  h += `<h2 class="group-title">Experimental</h2><div class="group"><div class="cell"><div class="grow"><div class="t">Sessions talk to each other</div>
    <div class="s">Adds a grandaddy MCP server, so a session can list, read and message other sessions, or start helpers in any agent and model and wait for their results.</div></div>
    ${sw('data-act="mesh"', mesh)}</div>`;
  if (mesh) {
    h += S.agents.filter((a) => a.detected).map((a) => `<div class="cell"><div class="grow">${esc(a.label)}${["opencode", "kilo"].includes(a.tool) && !a.mcp ? `<div class="s selectable"><code>${esc(a.mcpSnippet)}</code></div>` : ""}</div>
      ${["opencode", "kilo"].includes(a.tool) ? `<span class="tag">${a.mcp ? "Added" : "Add by hand"}</span>` : sw(`data-act="mcp" data-tool="${a.tool}"`, a.mcp)}</div>`).join("");
  }
  return h + `</div><p class="footnote">Sessions only accept messages from other sessions when they were started with grandaddy run.</p></div>`;
}

// ---------- rules ----------
function rulesPane() {
  if (S.rules == null) return skeleton(3);
  const examples = [["yes Bash(npm test*)", "approve test runs everywhere"], ["ask Bash(git push*)", "always ask, even with auto-yes on"], ["no Bash(rm -rf /*)", "refuse outright"]];
  let h = `<div class="page"><h1 class="page-title">Rules</h1>
    <p class="lead">Rules answer permission prompts for every connected agent before anything else does. The first match wins. Questions are never answered by a rule.</p>`;
  h += S.rules.length
    ? `<div class="group">` + S.rules.map((r, i) => `<div class="cell"><span class="tag ${r.verb === "no" ? "red" : r.verb === "yes" ? "green" : "orange"}">${esc(r.verb)}</span>
        <code class="grow selectable">${esc(r.text.replace(/^\w+\s+/, ""))}</code><button class="tool" data-act="rule-rm" data-n="${i + 1}" title="Remove">${G.x}</button></div>`).join("") + `</div>`
    : `<p class="footnote">No rules yet.</p>`;
  h += `<h2 class="group-title">Add a Rule</h2><div class="group"><div class="inline-form"><input ${field("rule")} value="${fval("rule")}" placeholder="yes Bash(npm test*)" spellcheck="false">
    <button class="btn default" data-act="rule-add">Add</button></div>` +
    examples.map(([r, d]) => `<div class="cell clickable" data-act="rule-example" data-rule="${esc(r)}"><code class="grow">${esc(r)}</code><span class="trail">${esc(d)}</span></div>`).join("") + `</div>`;
  return h + `</div>`;
}

// ---------- settings ----------
function settingsPane() {
  const st = S.settings;
  if (!st) return skeleton(6);
  const aw = st.awake || {};
  const mode = st.lidMode || "sleep";
  const ac = st.autoContinue || { enabled: false, message: "continue" };
  let h = `<div class="page"><h1 class="page-title">Settings</h1>`;

  h += `<h2 class="group-title">Power</h2><div class="group">
    <div class="cell"><div class="grow"><div class="t">Keep the Mac awake while sessions work</div><div class="s">${aw.holding ? "Holding it awake now. The display still sleeps." : "Everything else sleeps as usual, and the Mac sleeps once every session is idle."}</div></div>${sw('data-act="awake"', st.awake ? aw.enabled : true)}</div>
    <div class="cell" style="display:block;padding-bottom:0"><div class="t">When the lid closes</div><div class="s">Only while sessions are working. Keep the Mac somewhere it can breathe, not in a bag.</div></div>
    <div class="choices">
      ${[["sleep", "Sleep", "The Mac sleeps, as usual."], ["full", "Keep everything on", "The whole Mac keeps running."], ["agents", "Agents only", "Display off, other apps paused, sound muted, Low Power Mode. All undone when you open it."]]
        .map(([v, t, d]) => `<button class="choice ${mode === v ? "on" : ""}" data-act="lid" data-mode="${v}"><b>${t}</b><span>${d}</span></button>`).join("")}
    </div>`;
  if (mode !== "sleep" && aw.lid_active !== undefined) {
    const state = aw.agents_only_active ? `Lid closed · ${aw.frozen.length} app${aw.frozen.length === 1 ? "" : "s"} paused` : aw.lid_active ? "Ready: sessions are working" : "Waiting for a session to start working";
    h += `<div class="cell"><div class="grow"><div class="s">${esc(state)}${(mode === "full" ? aw.lid_helper : aw.agents_helper) ? "" : " · needs your password once, the first time it's chosen"}</div></div></div>`;
  }
  if (mode === "agents") {
    h += `<div class="cell" style="display:block;padding-bottom:8px"><div class="t">Keep running</div><div class="s">Besides the apps your sessions run in, these keep running with the lid closed. Everything else pauses.</div></div>
      <div class="chips">${(st.keep || []).map((k, i) => `<span class="chip">${esc(k)}<button data-act="keep-rm" data-i="${i}" title="Remove">${G.x}</button></span>`).join("")}</div>
      <div class="inline-form"><input ${field("keepAdd")} value="${fval("keepAdd")}" placeholder="App name, e.g. Spotify"><button class="btn glass" data-act="keep-add">Add</button></div>`;
  }
  h += `</div>`;

  h += `<h2 class="group-title">Usage Limits</h2><div class="group">
    <div class="cell"><div class="grow"><div class="t">Continue after a usage limit resets</div><div class="s">When a session stops on its 5-hour or weekly limit, Grandaddy sends it a message as soon as the limit lifts, so the work carries on without you.</div></div>${sw('data-act="auto-continue"', ac.enabled)}</div>
    <div class="inline-form"><span class="muted" style="width:70px">Message</span><input ${field("acMessage")} value="${fval("acMessage", ac.message)}" placeholder="continue"><button class="btn glass" data-act="ac-save">Save</button></div></div>
    <p class="footnote">It types into sessions started with grandaddy run, or running in iTerm, Terminal or tmux, and resumes background Claude sessions. For any other session you get a notification when its limit resets.</p>`;

  h += `<h2 class="group-title">Approvals</h2><div class="group">
    <div class="cell"><div class="grow"><div class="t">Auto-yes for every session</div><div class="s">Approves every permission prompt in every connected agent, except ones your rules say to ask about.</div></div>${sw('data-act="auto-all"', st.autoAll)}</div></div>`;

  h += `<h2 class="group-title">Phone</h2><div class="group">`;
  if (st.phone) {
    h += `<div class="cell"><div class="grow"><div class="t">Notifications with ntfy</div><div class="s selectable">Subscribed to <code>${esc(st.phone.topic)}</code> on ${esc(st.phone.server)}. The topic name works like a password.</div></div>
      <button class="btn glass" data-act="phone-test">Send Test</button><button class="btn glass" data-act="phone-off">Turn Off</button></div>`;
  } else {
    h += `<div class="cell"><div class="grow"><div class="t">Notifications with ntfy</div><div class="s">Get permission prompts and questions on your phone, and answer them there. Install the free ntfy app first.</div></div>
      <input ${field("ntfy")} value="${fval("ntfy")}" placeholder="https://ntfy.sh" style="width:150px" spellcheck="false"><button class="btn default" data-act="phone-setup">Set Up</button></div>`;
  }
  h += `<div class="cell"><div class="grow"><div class="t">Phone page on this Wi-Fi</div><div class="s">Answer prompts, follow goals and message sessions from your phone's browser. The link carries a private key.</div></div>${sw('data-act="web"', st.web.enabled)}</div>`;
  if (st.web.enabled && st.web.url) h += `<div class="cell"><div style="width:124px;background:#fff;border-radius:14px;padding:6px">${st.web.qr.replace(/width="\d+" height="\d+"/, 'width="112" height="112"')}</div>
    <div class="grow"><div class="s">Scan with your phone's camera, or open:</div><div class="selectable"><code>${esc(st.web.url)}</code></div></div></div>`;
  else if (st.web.enabled) h += `<div class="cell"><div class="s">This Mac isn't on a network right now.</div></div>`;
  h += `</div>`;

  h += `<h2 class="group-title">General</h2><div class="group">
    <div class="cell"><div class="grow"><div class="t">Open at login</div><div class="s">Starts in the menu bar, so loops, approvals and staying awake keep working.</div></div>${sw('data-act="login"', st.login)}</div>
    <div class="cell"><div class="grow"><div class="t">Command-line tool</div><div class="s">${st.cli.linked ? "grandaddy is on your PATH." : st.cli.bundled ? "Link the bundled grandaddy command into ~/.local/bin." : "Install with cargo install --path crates/grandaddy."}</div></div>
      ${st.cli.bundled && !st.cli.linked ? `<button class="btn glass" data-act="link-cli">Link</button>` : st.cli.linked ? '<span class="tag green">Linked</span>' : ""}</div></div>`;
  if (st.log && st.log.length) h += `<h2 class="group-title">Recent Decisions</h2><div class="group"><div class="cell selectable" style="display:block"><code style="white-space:pre-wrap;display:block;line-height:1.7">${esc(st.log.slice(-25).reverse().join("\n"))}</code></div></div>`;
  return h + `<p class="footnote" style="text-align:center;margin-top:26px">Grandaddy ${esc(st.version)}</p></div>`;
}

// ---------- events ----------
$("#groups").addEventListener("click", (e) => {
  const row = e.target.closest(".srow");
  if (row) select(row.dataset.key);
});
function select(key) {
  S.view = "sessions";
  S.sel = key;
  S.detail = null;
  S.git = null;
  S.gitCwd = null;
  S.jumpToEnd = true;
  render();
  refreshDetail().then(() => refreshView(true)).then(render);
}
window.grandaddy = { select };

async function openView(v) {
  S.view = v;
  S.lastPane = "";
  render();
  await refreshView(true);
  render();
}
$$(".row-btn").forEach((b) => (b.onclick = () => openView(b.dataset.view)));
$("#tabs").addEventListener("click", async (e) => {
  const b = e.target.closest("button");
  if (!b) return;
  S.tab = b.dataset.tab;
  S.lastPane = "";
  render();
  if (S.tab === "git") { await loadGit(); render(); }
});
window.addEventListener("resize", placeLens);
$("#filter").addEventListener("input", (e) => { S.filter = e.target.value; renderSidebar(); });

$("#btn-open").onclick = (e) => S.sel && act(invoke("open_session", { key: S.sel }), null, e.currentTarget);
$("#btn-auto").onclick = async () => {
  const s = selected();
  if (!s || !s.sid) return;
  if ((await act(invoke("set_auto", { sid: s.sid, on: !s.auto }), `Auto-yes ${s.auto ? "off" : "on"} for ${s.name}`)) !== undefined) tick();
};
$("#btn-stop").onclick = async () => {
  const s = selected();
  if (!s || !s.short) return;
  if (await confirmAlert(`Stop “${s.name}”?`, "The background session ends. You can resume it later from History.", "Stop", true)) {
    await act(invoke("stop_session", { short: s.short }));
    tick();
  }
};

$("#pane").addEventListener("input", (e) => {
  const f = e.target.dataset.f;
  if (f) S.form[f] = e.target.type === "checkbox" ? e.target.checked : e.target.value;
  const own = e.target.dataset.own;
  if (own) {
    const [rid, qi] = own.split(":");
    const a = S.ans[rid] || (S.ans[rid] = { pick: {}, own: {} });
    a.own[qi] = e.target.value;
    if (e.target.value.trim()) a.pick[qi] = [];
    const btn = $(`[data-act="answer"][data-rid="${CSS.escape(rid)}"]`);
    if (btn) btn.disabled = !answerReady(rid);
  }
  if (f === "histQ") { const pos = e.target.selectionStart; S.lastPane = ""; e.target.blur(); renderPane(); const i = $('[data-f="histQ"]'); i.focus(); i.setSelectionRange(pos, pos); }
});
$("#pane").addEventListener("change", (e) => {
  const f = e.target.dataset.f;
  if (f) {
    S.form[f] = e.target.type === "checkbox" ? e.target.checked : e.target.value;
    if (["goalAgent", "loopTarget"].includes(f)) { e.target.blur(); repaint(); }
  }
  const a = e.target.dataset.act;
  if (a && e.target.type === "checkbox") onToggle(a, e.target);
});
$("#pane").addEventListener("keydown", (e) => {
  if (e.key !== "Enter" || e.shiftKey || e.target.tagName === "TEXTAREA") return;
  const map = { commitMsg: "commit", newBranch: "branch", prTitle: "pr", rule: "rule-add", ntfy: "phone-setup", keepAdd: "keep-add", acMessage: "ac-save" };
  const target = map[e.target.dataset.f];
  if (target) { e.preventDefault(); $(`[data-act="${target}"]`)?.click(); }
});

function answerReady(rid) {
  const s = S.sessions.flatMap((x) => x.pending || []).find((r) => r.id === rid);
  if (!s) return false;
  const a = S.ans[rid];
  return (s.input.questions || []).every((_, qi) => (a.pick[qi] || []).length || (a.own[qi] || "").trim());
}

async function onToggle(a, el) {
  const on = el.checked;
  const tool = el.dataset.tool;
  let ok;
  if (a === "hook") ok = await act(invoke("agent_hook", { tool, on }), on ? "Connected" : "Disconnected");
  else if (a === "remote") ok = await act(invoke("agent_remote", { tool, on }), on ? "Prompts will wait for you here" : "Prompts go straight to the agent");
  else if (a === "mcp") ok = await act(invoke("agent_mcp", { tool, on }));
  else if (a === "mesh") ok = await act(invoke("set_mesh", { on }), on ? "Sessions can talk to each other" : "Turned off");
  else if (a === "login") ok = await act(invoke("set_login", { on }), on ? "Grandaddy opens at login" : "Won't open at login");
  else if (a === "auto-continue") ok = await act(invoke("set_auto_continue", { enabled: on, message: S.form.acMessage || (S.settings.autoContinue || {}).message || "continue" }), on ? "Sessions will continue after their limits reset" : "Auto-continue off");
  else if (a === "auto-all") {
    if (on && !(await confirmAlert("Approve everything?", "Every permission prompt in every connected session will be approved without asking, except ones your rules say to ask about.", "Turn On", true))) { el.checked = false; return; }
    ok = await act(invoke("set_auto_all", { on }), on ? "Auto-yes on everywhere" : "Auto-yes off");
  } else if (a === "awake") ok = await act(invoke("set_awake", { enabled: on, lid: on ? S.settings.lidMode || "sleep" : "sleep" }), "Saved");
  else if (a === "web") ok = await act(invoke("set_web", { on }), on ? "Phone page on" : "Phone page off");
  else if (a === "loop-toggle") ok = await act(invoke("loop_toggle", { id: el.dataset.id, on }), on ? "Resumed" : "Paused");
  if (ok === undefined) el.checked = !on;
  await refreshView(true);
  if (["hook", "remote", "mcp", "mesh"].includes(a)) S.agents = await invoke("agents");
  if (a === "auto-continue") S.settings = await invoke("settings");
  repaint();
}

$("#pane").addEventListener("click", async (e) => {
  const link = e.target.closest("a[data-url]");
  if (link) { e.preventDefault(); return act(invoke("open_url", { url: link.dataset.url }), "Opened in your browser"); }
  const el = e.target.closest("[data-act]");
  if (!el || el.type === "checkbox") return;
  const a = el.dataset.act;
  const s = selected();
  const F = S.form;
  const after = async (r) => { if (r !== undefined) { await refreshView(true); repaint(); } return r; };
  const run = (p, msg) => act(p, msg, el.matches("button") ? el : null);
  switch (a) {
    case "new": return openSheet();
    case "allow": case "deny": {
      el.disabled = true;
      await run(invoke("decide", { rid: el.dataset.rid, allow: a === "allow" }));
      return tick();
    }
    case "pick": {
      const { rid, q, o } = el.dataset;
      const req = (s && s.pending || []).find((r) => r.id === rid);
      const multi = req && req.input.questions[q].multiSelect;
      const st = S.ans[rid] || (S.ans[rid] = { pick: {}, own: {} });
      const cur = st.pick[q] || [];
      st.pick[q] = multi ? (cur.includes(+o) ? cur.filter((x) => x !== +o) : [...cur, +o]) : [+o];
      if (st.own[q]) st.own[q] = "";
      return repaint();
    }
    case "answer": {
      const req = (s && s.pending || []).find((r) => r.id === el.dataset.rid);
      if (!req) return;
      const st = S.ans[req.id];
      const answers = {};
      req.input.questions.forEach((q, qi) => {
        const own = (st.own[qi] || "").trim();
        answers[q.question] = own || (st.pick[qi] || []).map((i) => q.options[i].label).join(", ");
      });
      if ((await run(invoke("answer", { rid: req.id, answers }))) !== undefined) delete S.ans[req.id];
      return tick();
    }
    case "more": S.open[el.dataset.id] = true; return repaint();
    case "goto": return select(el.dataset.key);
    case "goto-settings": return openView("settings");
    case "url": return run(invoke("open_url", { url: el.dataset.url }), "Opened in your browser");
    case "continue-skip": {
      await run(invoke("set_continue_skip", { key: el.dataset.key, skip: el.dataset.skip === "1" }), el.dataset.skip === "1" ? "This session won't be continued" : "This session will be continued");
      return tick();
    }

    case "git-refresh": el.classList.add("busy"); await loadGit(); return repaint();
    case "git": {
      await run(invoke("git_action", { cwd: s.cwd, action: el.dataset.op }));
      await loadGit();
      return repaint();
    }
    case "diff": {
      if (S.diffPath === el.dataset.path) { S.diffPath = null; S.diff = null; return repaint(); }
      S.diffPath = el.dataset.path;
      S.diff = await invoke("git_diff", { cwd: s.cwd, path: el.dataset.path }).catch((err) => `couldn't read the diff: ${err}`);
      return repaint();
    }
    case "commit": {
      if (!(F.commitMsg || "").trim()) return toast("Write a commit message first.", true);
      if ((await run(invoke("git_action", { cwd: s.cwd, action: "commit", arg: F.commitMsg }))) !== undefined) { F.commitMsg = ""; S.diffPath = null; S.diff = null; }
      await loadGit();
      return repaint();
    }
    case "branch": {
      if ((await run(invoke("git_action", { cwd: s.cwd, action: "branch", arg: (F.newBranch || "").trim() }))) !== undefined) F.newBranch = "";
      await loadGit();
      return repaint();
    }
    case "switch": {
      await act(invoke("git_action", { cwd: s.cwd, action: "switch", arg: el.dataset.branch }));
      await loadGit();
      return repaint();
    }
    case "pr": {
      await run(invoke("git_action", { cwd: s.cwd, action: "pr", arg: F.prTitle || "", extra: { draft: !!F.prDraft, body: "" } }));
      F.prTitle = "";
      await loadGit();
      return repaint();
    }
    case "merge": {
      const method = F.mergeMethod || "squash";
      const del = F.mergeDelete !== false;
      const verb = { squash: "Squash and merge", merge: "Merge", rebase: "Rebase and merge" }[method];
      if (!(await confirmAlert(`${verb} #${el.dataset.n}?`, `This merges it into ${S.git.pr.base} on GitHub${del ? " and deletes the branch" : ""}. It can't be undone from here.`, "Merge"))) return;
      await run(invoke("git_action", { cwd: s.cwd, action: "merge", arg: el.dataset.n, extra: { method, deleteBranch: del } }));
      await loadGit();
      return repaint();
    }

    case "goal-start": {
      const r = await run(invoke("goal_start", { text: F.goalText || "", cwd: F.goalCwd ?? (s ? s.cwd : ""), agent: F.goalAgent || "claude", model: F.goalModel || null, terminal: !!F.goalTerminal }),
        (F.goalAgent || "claude") === "claude" && !F.goalTerminal ? "Goal started. Its first questions will appear here shortly." : "Goal opened in a terminal window.");
      if (r !== undefined) F.goalText = "";
      return after(r);
    }
    case "goal-rm": e.stopPropagation(); return after(await act(invoke("goal_remove", { id: el.dataset.id }), "Removed"));
    case "loop-add": {
      const target = F.loopTarget ?? (S.sel || "");
      const r = await run(invoke("loop_add", { prompt: F.loopPrompt || "", every: F.loopEvery || "30m", session: target || null, cwd: target ? null : F.loopCwd || null,
        until: F.loopUntil || null, maxRuns: F.loopMax ? parseInt(F.loopMax, 10) || null : null }), "Loop added. The first run happens now.");
      if (r !== undefined) { F.loopPrompt = ""; F.loopUntil = ""; F.loopMax = ""; }
      return after(r);
    }
    case "loop-rm": return after(await act(invoke("loop_remove", { id: el.dataset.id }), "Loop deleted"));

    case "hist": {
      if (S.histSel === el.dataset.sid) { S.histSel = null; return repaint(); }
      const p = S.hist.find((x) => x.sid === el.dataset.sid);
      S.histSel = p.sid;
      S.histAct = null;
      repaint();
      S.histAct = await invoke("past_activity", { path: p.path }).catch(() => []);
      return repaint();
    }
    case "hist-term": { const p = S.hist.find((x) => x.sid === el.dataset.sid); return run(invoke("open_past", { sid: p.sid, cwd: p.cwd })); }
    case "hist-resume": { const p = S.hist.find((x) => x.sid === el.dataset.sid); await run(invoke("resume_session", { sid: p.sid, cwd: p.cwd, prompt: null })); return tick(); }

    case "rule-add": { const r = await run(invoke("rules_add", { rule: F.rule || "" }), "Rule added"); if (r !== undefined) F.rule = ""; S.rules = await invoke("rules_list"); return repaint(); }
    case "rule-rm": await act(invoke("rules_remove", { n: +el.dataset.n }), "Rule removed"); S.rules = await invoke("rules_list"); return repaint();
    case "rule-example": F.rule = el.dataset.rule; repaint(); return $('[data-f="rule"]').focus();

    case "lid": {
      const mode = el.dataset.mode;
      if (mode === S.settings.lidMode) return;
      const msg = { sleep: "Closing the lid sleeps the Mac", full: "With the lid closed, everything keeps running while sessions work", agents: "With the lid closed, only your agents keep running" }[mode];
      el.classList.add("busy-choice");
      const r = await act(invoke("set_awake", { enabled: true, lid: mode }), msg);
      return after(r);
    }
    case "keep-add": {
      const name = (F.keepAdd || "").trim();
      if (!name) return;
      await run(invoke("set_keep", { apps: [...(S.settings.keep || []), name] }), `${name} keeps running`);
      F.keepAdd = "";
      return after(true);
    }
    case "keep-rm": {
      const keep = [...(S.settings.keep || [])];
      const [gone] = keep.splice(+el.dataset.i, 1);
      await act(invoke("set_keep", { apps: keep }), `${gone} will pause with the lid closed`);
      return after(true);
    }
    case "ac-save": {
      const msg = (F.acMessage || "").trim();
      if (!msg) return toast("Write the message to send.", true);
      await run(invoke("set_auto_continue", { enabled: !!(S.settings.autoContinue || {}).enabled, message: msg }), `Sessions will be sent “${msg}”`);
      return after(true);
    }
    case "phone-setup": await run(invoke("phone_setup", { server: F.ntfy || null }), "Subscribe to the topic shown, in the ntfy app"); return after(true);
    case "phone-test": return run(invoke("phone_test"));
    case "phone-off": await run(invoke("phone_off"), "Phone notifications off"); return after(true);
    case "link-cli": return after(await run(invoke("link_cli")));
  }
});

// ---------- new session sheet ----------
function openSheet() {
  const s = selected();
  $("#new-cwd").value = S.lastCwd || (s ? s.cwd : "");
  $("#scrim").hidden = false;
  $("#sheet").hidden = false;
  ($("#new-cwd").value ? $("#new-prompt") : $("#new-cwd")).focus();
}
function closeSheet() {
  $("#sheet").hidden = true;
  $("#scrim").hidden = true;
}
$("#btn-new").onclick = openSheet;
$("#new-cancel").onclick = closeSheet;
$("#scrim").onclick = () => { if ($("#alert").hidden) closeSheet(); };
$("#sheet").addEventListener("submit", async (e) => {
  e.preventDefault();
  const go = $("#new-go");
  const cwd = $("#new-cwd").value.trim();
  const r = await act(invoke("start_session", { cwd, prompt: $("#new-prompt").value, worktree: $("#new-wt").checked ? "" : null, model: $("#new-model").value || null }), null, go);
  if (r !== undefined) {
    S.lastCwd = cwd;
    $("#new-prompt").value = "";
    closeSheet();
    setTimeout(tick, 1500);
  }
});

// ---------- keyboard ----------
document.addEventListener("keydown", (e) => {
  const typing = e.target.matches("input, textarea, select");
  if (e.key === "Escape" && !$("#sheet").hidden) return closeSheet();
  if (e.metaKey && e.key === "n") { e.preventDefault(); return openSheet(); }
  if (e.metaKey && e.key === "f") { e.preventDefault(); return $("#filter").focus(); }
  if (e.metaKey && (e.key === "1" || e.key === "2") && selected()) { e.preventDefault(); return $(`#tabs [data-tab="${e.key === "1" ? "activity" : "git"}"]`).click(); }
  if (e.metaKey && e.key === "l" && selected()) { e.preventDefault(); return $("#composer-text").focus(); }
  if (typing || e.metaKey) return;
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    const rows = $$("#groups .srow").map((r) => r.dataset.key);
    if (!rows.length) return;
    e.preventDefault();
    const i = rows.indexOf(S.sel);
    select(rows[Math.max(0, Math.min(rows.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))]);
  }
});
window.addEventListener("blur", () => document.body.classList.add("inactive"));
window.addEventListener("focus", () => { document.body.classList.remove("inactive"); tick(); });

window.addEventListener("error", (e) => raw("ui_log", { level: "error", msg: `${e.message} @ ${e.filename}:${e.lineno}` }).catch(() => {}));
window.addEventListener("unhandledrejection", (e) => raw("ui_log", { level: "error", msg: String(e.reason) }).catch(() => {}));

boot();
setInterval(tick, 2000);
