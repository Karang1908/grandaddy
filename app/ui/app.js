// Grandaddy app: renders what grandaddy-core reports, and sends your answers back.
"use strict";

const invoke = (cmd, args = {}) => window.__TAURI__.core.invoke(cmd, args);
const $ = (s) => document.querySelector(s);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const fmtTokens = (n) => (n == null ? "–" : n >= 1e6 ? (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? Math.round(n / 1e3) + "k" : String(n));
const STATUS = { working: "working", "needs you": "waiting", idle: "idle", running: "running", blocked: "blocked", failed: "failed" };
const GROUPS = [["needs you", "Needs you"], ["working", "Working"], ["idle", "Idle"], ["running", "Other tools"], ["blocked", "Can't run"], ["failed", "Failed"]];
const FILE_TOOLS = new Set(["Read", "Edit", "Write", "MultiEdit", "NotebookEdit", "Grep", "Glob"]);

const S = {
  sessions: [], sel: null, view: "sessions", detail: null, filter: "",
  drafts: {}, hist: null, histQ: "", histSel: null, histAct: [], usage: null, rules: [], settings: null,
  lastPane: "", busy: false,
};

// ---------- feedback ----------
let toastTimer;
function toast(msg, err = false) {
  const t = $("#toast");
  t.textContent = msg;
  t.className = "toast show" + (err ? " err" : "");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.className = "toast" + (err ? " err" : "")), 3200);
}
async function act(promise, okMsg) {
  try {
    const r = await promise;
    toast(okMsg || (typeof r === "string" ? r : "Done"));
    return r;
  } catch (e) {
    toast(String(e), true);
    return undefined;
  }
}

// ---------- icons ----------
const ICON = {
  term: '<svg viewBox="0 0 16 16"><path d="m3 5 3 3-3 3M8 11h5"/></svg>',
  doc: '<svg viewBox="0 0 16 16"><path d="M4 2h5l3 3v9H4z"/><path d="M9 2v3h3"/></svg>',
  pen: '<svg viewBox="0 0 16 16"><path d="m10.5 3 2.5 2.5L6 12.5H3.5V10z"/></svg>',
  find: '<svg viewBox="0 0 16 16"><circle cx="7" cy="7" r="4.5"/><path d="m10.5 10.5 3 3"/></svg>',
  globe: '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6"/><path d="M2 8h12M8 2c2 2 2 10 0 12M8 2c-2 2-2 10 0 12"/></svg>',
  agent: '<svg viewBox="0 0 16 16"><circle cx="8" cy="5.5" r="2.5"/><path d="M3.5 13.5c.8-2.5 2.5-3.5 4.5-3.5s3.7 1 4.5 3.5"/></svg>',
  ask: '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6"/><path d="M6.3 6.2a1.8 1.8 0 1 1 2.5 1.7c-.5.2-.8.6-.8 1.1M8 11.3v.1"/></svg>',
  dot: '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="2"/></svg>',
  x: '<svg viewBox="0 0 16 16"><path d="m5 5 6 6M11 5l-6 6"/></svg>',
  warn: '<svg viewBox="0 0 16 16"><path d="M8 2.5 14 13H2z"/><path d="M8 6.5v3M8 11.3v.1"/></svg>',
  branch: '<svg viewBox="0 0 16 16"><circle cx="5" cy="4" r="1.5"/><circle cx="5" cy="12" r="1.5"/><circle cx="11" cy="6" r="1.5"/><path d="M5 5.5v5M11 7.5c0 2-3 2-5.5 3.5"/></svg>',
  ctx: '<svg viewBox="0 0 16 16"><rect x="2.5" y="3" width="11" height="10" rx="2"/><path d="M5 6h6M5 9h4"/></svg>',
  bolt: '<svg viewBox="0 0 16 16"><path d="M9 2 4 9h4l-1 5 5-7H8z"/></svg>',
  folder: '<svg viewBox="0 0 16 16"><path d="M2 4.5h4l1.5 1.5H14v6.5H2z"/></svg>',
};
const toolIcon = (t = "") =>
  t === "Bash" || t === "shell" ? ICON.term : t === "Read" ? ICON.doc : ["Edit", "Write", "MultiEdit", "NotebookEdit"].includes(t) ? ICON.pen
  : ["Grep", "Glob", "ToolSearch"].includes(t) ? ICON.find : t.startsWith("Web") ? ICON.globe : ["Task", "Agent"].includes(t) ? ICON.agent
  : t === "AskUserQuestion" ? ICON.ask : ICON.dot;

// ---------- data ----------
async function refreshSessions() {
  try {
    S.sessions = await invoke("list_sessions");
  } catch (e) {
    S.sessions = [];
  }
  if (S.sel && !S.sessions.some((s) => s.key === S.sel)) S.sel = null;
  if (!S.sel && S.sessions.length) {
    // open on what matters: something waiting on you, then something working, then anything idle
    const by = (st) => S.sessions.find((s) => s.status === st);
    S.sel = (by("needs you") || by("working") || by("idle") || S.sessions[0]).key;
  }
  renderSidebar();
  renderToolbar();
}
async function refreshDetail() {
  if (S.view !== "sessions" || !S.sel) {
    S.detail = null;
    return;
  }
  try {
    S.detail = await invoke("session_detail", { key: S.sel });
  } catch {
    S.detail = null;
  }
}
async function tick() {
  if (S.busy) return;
  S.busy = true;
  try {
    await refreshSessions();
    await refreshDetail();
    renderPane();
  } finally {
    S.busy = false;
  }
}

// ---------- sidebar ----------
function renderSidebar() {
  const q = S.filter.toLowerCase();
  const list = S.sessions.filter((s) => !q || s.name.toLowerCase().includes(q) || (s.folder || "").toLowerCase().includes(q));
  const waiting = S.sessions.filter((s) => s.status === "needs you").length;
  $("#brand-sub").textContent = waiting ? `${waiting} waiting on you` : S.sessions.length ? `${S.sessions.length} session${S.sessions.length > 1 ? "s" : ""} running` : "Nothing running";
  let html = "";
  for (const [st, title] of GROUPS) {
    const rows = list.filter((s) => s.status === st);
    if (!rows.length) continue;
    html += `<div class="group-title ${st === "needs you" ? "hot" : ""}"><span>${title}</span><span class="count">${rows.length}</span></div>`;
    for (const s of rows) {
      const badges = (s.bg ? '<span class="badge">bg</span>' : "") + (s.auto ? '<span class="badge auto">auto</span>' : "");
      const meta = s.tool === "claude" ? s.folder : `${s.tool} · ${s.folder}`;
      html += `<button class="srow ${S.view === "sessions" && s.key === S.sel ? "sel" : ""}" data-key="${esc(s.key)}">
        <span class="dot ${STATUS[s.status] || ""}"></span>
        <span class="name">${esc(s.name)}${badges}</span>
        <span class="when">${esc(s.ago)}</span>
        <span class="meta">${esc(meta)}</span></button>`;
    }
  }
  const other = list.filter((s) => !GROUPS.some(([g]) => g === s.status));
  if (other.length) html += other.map((s) => `<button class="srow" data-key="${esc(s.key)}"><span class="dot"></span><span class="name">${esc(s.name)}</span><span class="when">${esc(s.ago)}</span><span class="meta">${esc(s.status)}</span></button>`).join("");
  if (!html) html = `<div class="empty-side">${q ? "No sessions match." : "No sessions running."}</div>`;
  $("#groups").innerHTML = html;
  document.querySelectorAll(".view-btn").forEach((b) => b.classList.toggle("on", b.dataset.view === S.view));
}

function selected() {
  return S.sessions.find((s) => s.key === S.sel);
}

function renderToolbar() {
  const s = S.view === "sessions" ? selected() : null;
  const titles = { history: "History", usage: "Usage", rules: "Rules", settings: "Settings" };
  $("#toolbar-title").textContent = s ? "" : titles[S.view] || "";
  $("#session-actions").hidden = !s;
  if (!s) return;
  $("#btn-open-label").textContent = s.bg ? "Attach" : "Open";
  $("#btn-auto").hidden = s.tool !== "claude";
  $("#btn-auto").classList.toggle("on", !!s.auto);
  $("#btn-stop").hidden = !(s.bg && s.short);
}

// ---------- panes ----------
function renderPane() {
  const pane = $("#pane");
  // don't yank the page out from under someone typing an answer or a search
  if (pane.contains(document.activeElement) && document.activeElement.matches("input, textarea")) return;
  const html = S.view === "sessions" ? sessionPane() : S.view === "history" ? historyPane() : S.view === "usage" ? usagePane()
    : S.view === "rules" ? rulesPane() : settingsPane();
  if (html === S.lastPane) return;
  S.lastPane = html;
  const keepScroll = pane.scrollTop;
  pane.innerHTML = html;
  pane.scrollTop = keepScroll;
}

function sessionPane() {
  const s = selected();
  if (!s) {
    return `<div class="empty"><div><img src="assets/watching.png" alt="">
      <h2>${S.sessions.length ? "Pick a session" : "Nothing running yet"}</h2>
      <p>Start Claude Code, Codex or Gemini in any terminal and it shows up here — or start one in the background.</p>
      <button class="btn primary" data-act="new">New Session</button></div></div>`;
  }
  const d = S.detail && S.detail.session.key === s.key ? S.detail : null;
  const st = STATUS[s.status] || "";
  const ident = s.bg && s.short ? `bg ${s.short}` : s.pid ? `pid ${s.pid}` : "";
  const sub = [s.tool, d && d.model, ident].filter(Boolean).map(esc).join(" · ");
  let h = `<div class="wrap"><div class="hero"><div style="flex:1;min-width:0"><h1 class="selectable">${esc(s.name)}</h1>
    <div class="sub">${sub}</div></div><span class="status-pill ${st}"><span class="dot ${st}"></span>${esc(s.status)} · ${esc(s.ago)}</span></div>`;
  const chips = [`<span class="chip">${ICON.folder}<span class="selectable">${esc(s.folder)}</span></span>`];
  if (d && d.git && d.git.branch) chips.push(`<span class="chip">${ICON.branch}<b>${esc(d.git.branch)}</b>${d.git.changed ? ` · ${d.git.changed} changed` : ""}${d.git.ahead ? ` · ${esc(d.git.ahead)}` : ""}</span>`);
  if (d && d.context) chips.push(`<span class="chip">${ICON.ctx}context <b>${fmtTokens(d.context)}</b></span>`);
  if (d && d.tokens) chips.push(`<span class="chip">${ICON.bolt}<b>${fmtTokens(d.tokens)}</b> tokens</span>`);
  if (s.tool === "claude") chips.push(`<span class="chip">auto-yes <b>${s.auto ? "on" : "off"}</b></span>`);
  h += `<div class="chips">${chips.join("")}</div>`;
  const notes = [];
  if (d) {
    d.signals.forEach((x) => notes.push(["", x.startsWith("quiet") ? `No new output for ${x.slice(6)} while working` : "It's repeating the same tool call — it may be stuck"]));
    if (d.twins.length) notes.push(["", `Also running in this folder: ${d.twins.join(", ")}`]);
    const files = Object.keys(d.conflicts || {});
    if (files.length) {
      const who = [...new Set(Object.values(d.conflicts).flat())].join(" & ");
      notes.push(["", `${who} also edited ${files.slice(0, 3).map((f) => f.split("/").pop()).join(", ")}${files.length > 3 ? " …" : ""}`]);
    }
  }
  if (s.status === "blocked" || s.status === "failed") notes.push(["bad", `This background session can't run (${s.raw}). Attach to see why, or stop it.`]);
  if (notes.length) h += `<div class="notes">${notes.map(([c, t]) => `<div class="note ${c}">${ICON.warn}<span>${esc(t)}</span></div>`).join("")}</div>`;
  h += needsCard(s);
  h += `<div class="section-title">Activity</div>`;
  h += d ? feed(d.events, s.tool) : `<div class="more-above">Loading…</div>`;
  return h + "</div>";
}

function draft(rid) {
  return (S.drafts[rid] ||= { qi: 0, answers: {}, multi: new Set() });
}

function needsCard(s) {
  const req = s.pending && s.pending[0];
  const more = s.pending && s.pending.length > 1 ? `<div class="hint" style="margin-top:10px">+${s.pending.length - 1} more waiting after this</div>` : "";
  if (!req) {
    if (s.status !== "needs you") return "";
    const where = !s.can_answer ? `in its own terminal (${esc(s.tool)})` : s.bg ? "— attach to answer it" : "in its terminal";
    return `<div class="needs"><div class="needs-head">${ICON.warn}Needs you</div><h3>Waiting on you ${where}</h3>
      <div class="actions"><span class="hint">Grandaddy can't see this prompt.</span><button class="btn" data-act="open">${s.bg ? "Attach" : "Open terminal"}</button></div></div>`;
  }
  if (req.tool !== "AskUserQuestion") {
    const subj = req.input.command || req.input.file_path || req.input.url || req.input.pattern || JSON.stringify(req.input, null, 1);
    return `<div class="needs" data-rid="${esc(req.id)}"><div class="needs-head">${ICON.warn}Needs you<span class="tag">permission</span></div>
      <h3>${esc(s.name)} wants to use ${esc(req.tool)}</h3>
      <pre class="cmd selectable"><span class="tool">${esc(req.tool)}</span>${esc(subj)}</pre>
      <div class="actions"><span class="hint">or answer in its terminal · <kbd>Y</kbd> <kbd>N</kbd></span>
        <button class="btn danger" data-act="deny">Deny</button><button class="btn primary" data-act="allow">Allow</button></div>${more}</div>`;
  }
  const qs = req.input.questions || [];
  const dr = draft(req.id);
  if (dr.qi >= qs.length) return `<div class="needs"><div class="needs-head">Answer sent…</div></div>`;
  const q = qs[dr.qi];
  const multi = !!q.multiSelect;
  const tag = [qs.length > 1 ? `Question ${dr.qi + 1} of ${qs.length}` : "", q.header || ""].filter(Boolean).join(" · ");
  const opts = (q.options || []).map((o, i) => `<button class="opt ${dr.multi.has(i) ? "picked" : ""}" data-act="opt" data-i="${i}">
      <span class="key">${multi && dr.multi.has(i) ? "✓" : i + 1}</span><span><div class="label">${esc(o.label)}</div>${o.description ? `<div class="desc">${esc(o.description)}</div>` : ""}</span></button>`).join("");
  return `<div class="needs" data-rid="${esc(req.id)}"><div class="needs-head">${ICON.ask}Question<span class="tag">${esc(tag)}</span></div>
    <h3 class="selectable">${esc(q.question)}</h3><div class="options">${opts}</div>
    <form class="own" data-act="own"><input placeholder="Or type your own answer…" autocomplete="off"><button class="btn" type="submit">Send</button>
    ${multi ? `<button class="btn primary" type="button" data-act="send-multi" ${dr.multi.size ? "" : "disabled"}>Send selected</button>` : ""}</form>
    <div class="actions" style="margin-top:10px"><span class="hint">${multi ? "Pick any — " : ""}press <kbd>1</kbd>–<kbd>${(q.options || []).length}</kbd>${multi ? " then <kbd>↩</kbd>" : ""} · or answer in its terminal</span></div>${more}</div>`;
}

function feed(events, who) {
  if (!events || !events.length) return `<div class="more-above">${who === "claude" ? "No activity yet." : `${esc(who)} doesn't share its activity yet.`}</div>`;
  // group: turns start at each prompt; runs of the same file tool collapse into one row
  const groups = [];
  for (const e of events) {
    const last = groups[groups.length - 1];
    if (e.k === "tool" && last && last.k === "tool" && last.tool === e.tool && FILE_TOOLS.has(e.tool)) {
      last.items.push(e.text);
      continue;
    }
    groups.push({ ...e, items: [e.text] });
  }
  const turns = [];
  for (const g of groups) {
    if (g.k === "you" || !turns.length) turns.push({ head: g.k === "you" ? g : null, body: [] });
    if (g.k !== "you") turns[turns.length - 1].body.push(g);
  }
  const time = (ts) => (ts ? new Date(ts * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false }) : "");
  let h = turns[0] && !turns[0].head ? `<div class="more-above">Earlier activity…</div>` : "";
  h += '<div class="feed">';
  for (const t of turns.slice(-30)) {
    const rows = t.body.map((g) => {
      if (g.k === "text") return `<div class="say clamped selectable">${esc(g.text)}</div>`;
      if (g.k === "error") return `<div class="tool-row err"><span class="ic">${ICON.x}</span><span class="tname">Error</span><span class="targ" title="${esc(g.text)}">${esc(g.text)}</span></div>`;
      if (g.k === "mark") return `<div class="mark">${esc(g.text)}</div>`;
      const n = g.items.length;
      const subj = n === 1 ? g.desc || g.items[0] : [...new Set(g.items.map((x) => x.split("/").pop()))].slice(-5).join(", ");
      const name = (g.tool || "?").replace("mcp__", "");
      return `<div class="tool-row"><span class="ic">${toolIcon(g.tool)}</span><span class="tname">${esc(name)}${n > 1 ? `<span class="x">×${n}</span>` : ""}</span><span class="targ" title="${esc(g.items.join("\n"))}">${esc(subj)}</span></div>`;
    }).join("");
    h += `<div class="turn"><div class="time">${t.head ? time(t.head.ts) : ""}</div><div class="body">${t.head ? `<div class="prompt selectable">${esc(t.head.text)}</div>` : ""}${rows}</div></div>`;
  }
  return h + "</div>";
}

function historyPane() {
  if (!S.hist) return `<div class="wrap"><div class="view-head"><h1>History</h1><p>Loading your past sessions…</p></div></div>`;
  const q = S.histQ.toLowerCase();
  const rows = S.hist.filter((h) => !q || (h.prompt || "").toLowerCase().includes(q) || (h.folder || "").toLowerCase().includes(q));
  const cur = rows.find((h) => h.sid === S.histSel) || rows[0];
  const list = rows.slice(0, 200).map((h) => `<div class="li click ${cur && h.sid === cur.sid ? "sel" : ""}" data-act="hist" data-sid="${esc(h.sid)}">
      <div class="t">${esc(h.prompt)}</div><div class="s">${esc(h.folder)}</div><div class="r">${esc(h.ago)} ago</div></div>`).join("");
  const right = cur ? `<div class="card"><h3 class="selectable">${esc(cur.folder)}</h3><p class="mono">${esc(cur.sid)}</p>
      <div class="actions" style="margin-bottom:10px"><button class="btn" data-act="hist-open">Open in terminal</button></div>
      <form class="own" data-act="hist-continue"><input placeholder="Continue it in the background with…"><button class="btn primary" type="submit">Continue</button></form></div>
      ${feed(S.histAct, "claude")}` : "";
  return `<div class="wrap" style="max-width:1100px"><div class="view-head"><h1>History</h1><p>${S.hist.length} sessions you started. Reopen one, or continue it in the background with a new prompt.</p></div>
    <div class="searchbar"><input id="hist-q" placeholder="Search prompts and folders" value="${esc(S.histQ)}"></div>
    <div class="split"><div class="list">${list || '<div class="li"><div class="s">Nothing matches.</div></div>'}</div><div>${right}</div></div></div>`;
}

function usagePane() {
  if (!S.usage) return `<div class="wrap"><div class="view-head"><h1>Usage</h1><p>Counting tokens across your transcripts…</p></div></div>`;
  const peak = Math.max(1, ...S.usage.days.map((d) => d.total));
  const days = S.usage.days.map((d) => `<div class="bar-row"><span class="d">${esc(new Date(d.day + "T12:00").toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" }))}</span>
      <div class="track"><div class="fill" style="width:${Math.max(1, (100 * d.total) / peak)}%"></div></div><span class="n">${fmtTokens(d.total)}</span></div>`).join("");
  const projects = S.usage.today.slice(0, 12).map((p) => `<div class="li"><div class="t">${esc(p.project)}</div><div class="r"><b>${fmtTokens(p.total)}</b></div></div>`).join("");
  return `<div class="wrap"><div class="view-head"><h1>Usage</h1><p>Tokens from your Claude Code transcripts — input, output and cache reads/writes.</p></div>
    <div class="card"><h3>Last 7 days</h3><p></p><div class="bars">${days || "No usage yet."}</div></div>
    <div class="section-title">Today by project</div><div class="list">${projects || '<div class="li"><div class="s">Nothing today yet.</div></div>'}</div></div>`;
}

function rulesPane() {
  const rows = S.rules.map((r, i) => `<div class="li"><div class="t"><span class="verb ${esc(r.verb)}">${esc(r.verb)}</span>&nbsp; <span class="mono">${esc(r.text.slice(r.verb.length).trim())}</span></div>
      <div class="r"><button class="btn" data-act="rule-rm" data-n="${i + 1}">Remove</button></div></div>`).join("");
  return `<div class="wrap"><div class="view-head"><h1>Rules</h1><p>Finer than auto-yes. The first rule that matches a permission prompt wins, in every session. Questions always come to you.</p></div>
    <div class="card"><h3>Add a rule</h3><p><span class="verb yes">yes</span> approve automatically · <span class="verb ask">ask</span> always ask, even with auto-yes on · <span class="verb no">no</span> refuse automatically</p>
      <form class="own" data-act="rule-add"><input id="rule-in" class="mono" placeholder='yes Bash(npm test*)' spellcheck="false"><button class="btn primary" type="submit">Add</button></form></div>
    <div class="list">${rows || '<div class="li"><div class="s">No rules yet. Try <span class="mono">ask Bash(git push*)</span> so pushes always come to you.</div></div>'}</div></div>`;
}

function settingsPane() {
  const st = S.settings;
  if (!st) return `<div class="wrap"><div class="view-head"><h1>Settings</h1></div></div>`;
  const phone = st.phone
    ? `<p>On. Your ntfy topic (keep it secret — it's the password):</p><pre class="cmd selectable">${esc(st.phone.server)}/${esc(st.phone.topic)}</pre>
       <div class="actions"><span class="hint"></span><button class="btn" data-act="phone-test">Send a test</button><button class="btn danger" data-act="phone-off">Turn off</button></div>`
    : `<p>Get a notification with Allow / Deny buttons on your phone, using the free ntfy app. Prompts pass through the ntfy server, so use your own for anything sensitive.</p>
       <form class="own" data-act="phone-on"><input placeholder="https://ntfy.sh" spellcheck="false"><button class="btn primary" type="submit">Turn on</button></form>`;
  return `<div class="wrap"><div class="view-head"><h1>Settings</h1></div>
    <div class="card"><h3>Claude Code hook</h3>
      <p>${st.hookInstalled ? "Installed. Running sessions pick it up straight away." : "Not installed — grandaddy can't answer prompts until it is."}</p>
      <dl class="kv"><dt>Command</dt><dd class="mono selectable">${esc(st.hookCommand)}</dd></dl>
      ${st.hookInstalled ? "" : '<div class="actions" style="margin-top:12px"><span class="hint"></span><button class="btn primary" data-act="install">Install hook</button></div>'}</div>
    <div class="card"><h3>Auto-yes everywhere</h3><p>Approve every permission prompt in every session. Questions still come to you, and <span class="verb ask">ask</span> rules still apply.</p>
      <label class="toggle"><input type="checkbox" data-act="auto-all" ${st.autoAll ? "checked" : ""}><span class="switch"></span>${st.autoAll ? "On" : "Off"}</label></div>
    <div class="card"><h3>Phone approvals</h3>${phone}</div>
    <div class="card"><h3>Recent decisions</h3><pre class="log selectable">${esc(st.log.slice().reverse().join("\n")) || "Nothing yet."}</pre></div></div>`;
}

// ---------- actions ----------
async function setView(v) {
  S.view = v;
  S.lastPane = "";
  $("#pane").scrollTop = 0;
  renderSidebar();
  renderToolbar();
  if (v === "history" && !S.hist) {
    renderPane();
    S.hist = await invoke("history").catch(() => []);
    if (S.hist[0]) await pickHistory(S.hist[0].sid);
  }
  if (v === "usage") invoke("usage", { days: 7 }).then((u) => { S.usage = u; renderPane(); }).catch((e) => toast(String(e), true));
  if (v === "rules") S.rules = await invoke("rules_list");
  if (v === "settings") S.settings = await invoke("settings");
  if (v === "sessions") await refreshDetail();
  renderPane();
}
async function pickHistory(sid) {
  S.histSel = sid;
  const h = S.hist.find((x) => x.sid === sid);
  S.histAct = h ? await invoke("past_activity", { path: h.path }).catch(() => []) : [];
  S.lastPane = "";
  renderPane();
}
function selectSession(key) {
  S.sel = key;
  S.lastPane = "";
  if (S.view !== "sessions") return setView("sessions");
  renderSidebar();
  renderToolbar();
  refreshDetail().then(renderPane);
  renderPane();
}
function move(delta) {
  const keys = [...document.querySelectorAll(".srow")].map((b) => b.dataset.key);
  if (!keys.length) return;
  const i = Math.max(0, Math.min(keys.length - 1, keys.indexOf(S.sel) + delta));
  selectSession(keys[i]);
}
function nextWaiting() {
  const w = S.sessions.filter((s) => s.status === "needs you").map((s) => s.key);
  if (!w.length) return toast("Nothing is waiting on you");
  selectSession(w[(w.indexOf(S.sel) + 1) % w.length]);
}

async function decide(allow) {
  const s = selected(), req = s && s.pending[0];
  if (!req || req.tool === "AskUserQuestion") return;
  await act(invoke("decide", { rid: req.id, allow }));
  tick();
}
async function record(req, q, text) {
  const qs = req.input.questions || [];
  const dr = draft(req.id);
  dr.answers[q.question] = text;
  dr.qi += 1;
  dr.multi = new Set();
  S.lastPane = "";
  if (dr.qi >= qs.length) await act(invoke("answer", { rid: req.id, answers: dr.answers }));
  renderPane();
  tick();
}
function currentQuestion() {
  const s = selected(), req = s && s.pending[0];
  if (!req || req.tool !== "AskUserQuestion") return null;
  const qs = req.input.questions || [];
  const dr = draft(req.id);
  return dr.qi < qs.length ? { req, q: qs[dr.qi], dr } : null;
}
function pickOption(i) {
  const c = currentQuestion();
  if (!c || !c.q.options || i >= c.q.options.length) return;
  if (c.q.multiSelect) {
    c.dr.multi.has(i) ? c.dr.multi.delete(i) : c.dr.multi.add(i);
    S.lastPane = "";
    renderPane();
  } else record(c.req, c.q, c.q.options[i].label);
}
function sendMulti() {
  const c = currentQuestion();
  if (c && c.q.multiSelect && c.dr.multi.size) record(c.req, c.q, [...c.dr.multi].sort().map((i) => c.q.options[i].label).join(", "));
}

// ---------- new session sheet ----------
function openSheet() {
  const s = selected();
  $("#new-cwd").value = (s && s.folder) || "~";
  $("#new-prompt").value = "";
  $("#new-wt").checked = false;
  $("#new-model").value = "";
  $("#scrim").hidden = $("#sheet").hidden = false;
  setTimeout(() => $("#new-prompt").focus(), 30);
}
function closeSheet() {
  $("#scrim").hidden = $("#sheet").hidden = true;
}
$("#sheet").addEventListener("submit", async (e) => {
  e.preventDefault();
  const go = $("#new-go");
  go.disabled = true;
  go.textContent = "Starting…";
  const r = await act(invoke("start_session", { cwd: $("#new-cwd").value, prompt: $("#new-prompt").value,
    worktree: $("#new-wt").checked ? "" : null, model: $("#new-model").value || null }));
  go.disabled = false;
  go.textContent = "Start";
  if (r !== undefined) {
    closeSheet();
    setTimeout(tick, 1200);
  }
});
$("#new-cancel").onclick = closeSheet;
$("#scrim").onclick = closeSheet;

// ---------- events ----------
$("#groups").addEventListener("click", (e) => {
  const b = e.target.closest(".srow");
  if (b) selectSession(b.dataset.key);
});
document.querySelectorAll(".view-btn").forEach((b) => (b.onclick = () => setView(S.view === b.dataset.view ? "sessions" : b.dataset.view)));
$("#filter").addEventListener("input", (e) => { S.filter = e.target.value; renderSidebar(); });
$("#btn-new").onclick = openSheet;
$("#btn-open").onclick = () => S.sel && act(invoke("open_session", { key: S.sel }));
$("#btn-auto").onclick = async () => {
  const s = selected();
  if (!s) return;
  await act(invoke("set_auto", { sid: s.sid, on: !s.auto }), `Auto-yes ${s.auto ? "off" : "on"} for ${s.name}`);
  tick();
};
let stopArmed = 0;
$("#btn-stop").onclick = async () => {
  const s = selected();
  if (!s || !s.short) return;
  if (Date.now() - stopArmed > 4000) {
    stopArmed = Date.now();
    return toast(`Click Stop again to stop ${s.name}`);
  }
  stopArmed = 0;
  await act(invoke("stop_session", { short: s.short }));
  tick();
};

$("#pane").addEventListener("click", async (e) => {
  const el = e.target.closest("[data-act]");
  if (!el) {
    const say = e.target.closest(".say");
    if (say) say.classList.toggle("clamped");
    return;
  }
  const a = el.dataset.act;
  const h = S.hist && S.hist.find((x) => x.sid === S.histSel);
  if (a === "allow") decide(true);
  else if (a === "deny") decide(false);
  else if (a === "opt") pickOption(+el.dataset.i);
  else if (a === "send-multi") sendMulti();
  else if (a === "new") openSheet();
  else if (a === "open") $("#btn-open").click();
  else if (a === "hist") pickHistory(el.dataset.sid);
  else if (a === "hist-open" && h) act(invoke("open_past", { sid: h.sid, cwd: h.cwd }));
  else if (a === "rule-rm") { await act(invoke("rules_remove", { n: +el.dataset.n }), "Rule removed"); S.rules = await invoke("rules_list"); S.lastPane = ""; renderPane(); }
  else if (a === "install") { await act(invoke("install_hook"), "Hook installed"); S.settings = await invoke("settings"); S.lastPane = ""; renderPane(); }
  else if (a === "phone-test") act(invoke("phone_test"));
  else if (a === "phone-off") { await invoke("phone_off"); S.settings = await invoke("settings"); S.lastPane = ""; renderPane(); toast("Phone approvals off"); }
});
$("#pane").addEventListener("change", async (e) => {
  if (e.target.dataset.act === "auto-all") {
    await act(invoke("set_auto_all", { on: e.target.checked }), `Auto-yes everywhere ${e.target.checked ? "on" : "off"}`);
    S.settings = await invoke("settings");
    S.lastPane = "";
    renderPane();
  }
});
$("#pane").addEventListener("submit", async (e) => {
  e.preventDefault();
  const f = e.target, a = f.dataset.act, input = f.querySelector("input"), v = input ? input.value.trim() : "";
  input && input.blur();
  if (a === "own" && v) { const c = currentQuestion(); if (c) record(c.req, c.q, v); }
  else if (a === "hist-continue" && v) {
    const h = S.hist.find((x) => x.sid === S.histSel);
    if (h && (await act(invoke("resume_session", { sid: h.sid, cwd: h.cwd, prompt: v }))) !== undefined) { setView("sessions"); setTimeout(tick, 1200); }
  } else if (a === "rule-add" && v) {
    if ((await act(invoke("rules_add", { rule: v }), "Rule added")) !== undefined) { S.rules = await invoke("rules_list"); S.lastPane = ""; renderPane(); }
  } else if (a === "phone-on") {
    await invoke("phone_setup", { server: v || null });
    S.settings = await invoke("settings");
    S.lastPane = "";
    renderPane();
    toast("Phone approvals on — subscribe to the topic in the ntfy app");
  }
});
$("#pane").addEventListener("input", (e) => {
  if (e.target.id === "hist-q") {
    S.histQ = e.target.value;
    const pos = e.target.selectionStart;
    S.lastPane = "";
    e.target.blur();
    renderPane();
    const q = $("#hist-q");
    q.focus();
    q.setSelectionRange(pos, pos);
  }
});

document.addEventListener("keydown", (e) => {
  if (!$("#sheet").hidden) {
    if (e.key === "Escape") closeSheet();
    return;
  }
  if (e.metaKey && e.key.toLowerCase() === "n") { e.preventDefault(); return openSheet(); }
  if (e.metaKey && e.key.toLowerCase() === "f") { e.preventDefault(); return $("#filter").focus(); }
  const typing = document.activeElement && document.activeElement.matches("input, textarea, select");
  if (e.key === "Escape") {
    if (typing) return document.activeElement.blur();
    if (S.view !== "sessions") return setView("sessions");
  }
  if (typing || e.metaKey || e.ctrlKey || e.altKey) return;
  if (e.key === "Tab") { e.preventDefault(); return nextWaiting(); }
  if (S.view === "sessions") {
    if (e.key === "ArrowDown" || e.key === "j") { e.preventDefault(); move(1); }
    else if (e.key === "ArrowUp" || e.key === "k") { e.preventDefault(); move(-1); }
    else if (e.key === "y") decide(true);
    else if (e.key === "n") decide(false);
    else if (/^[1-9]$/.test(e.key)) pickOption(+e.key - 1);
    else if (e.key === "Enter") sendMulti();
  } else if (S.view === "history" && S.hist) {
    const rows = [...document.querySelectorAll('[data-act="hist"]')].map((r) => r.dataset.sid);
    const i = rows.indexOf(S.histSel);
    if (e.key === "ArrowDown") { e.preventDefault(); pickHistory(rows[Math.min(rows.length - 1, i + 1)]); }
    if (e.key === "ArrowUp") { e.preventDefault(); pickHistory(rows[Math.max(0, i - 1)]); }
  }
});

// errors go to the app's console; GRANDADDY_DEBUG=1 adds a self-check of every read command
const log = (level, msg) => invoke("ui_log", { level, msg: String(msg) }).catch(() => {});
window.addEventListener("error", (e) => log("error", `${e.message} @ ${e.filename}:${e.lineno}`));
window.addEventListener("unhandledrejection", (e) => log("error", `unhandled: ${e.reason}`));
async function selfCheck() {
  if (!(await invoke("debug").catch(() => false))) return;
  const sum = async (name, fn) => { try { log("info", `${name}: ${await fn()}`); } catch (e) { log("error", `${name} failed: ${e}`); } };
  await sum("list_sessions", async () => { const s = await invoke("list_sessions"); return `${s.length} sessions [${s.map((x) => `${x.name}=${x.status}`).join(", ")}]`; });
  await sum("session_detail", async () => { if (!S.sel) return "no session to open"; const d = await invoke("session_detail", { key: S.sel });
    return `${d.session.name}: ${d.events.length} events, model ${d.model}, context ${d.context}, branch ${d.git && d.git.branch}`; });
  await sum("settings", async () => { const st = await invoke("settings"); return `hook installed=${st.hookInstalled}, autoAll=${st.autoAll}, phone=${!!st.phone}, ${st.log.length} log lines`; });
  await sum("history", async () => { const h = await invoke("history"); return `${h.length} past sessions, newest "${h[0] && h[0].prompt.slice(0, 40)}"`; });
  await sum("past_activity", async () => { const h = await invoke("history"); return h[0] ? `${(await invoke("past_activity", { path: h[0].path })).length} events` : "none"; });
  await sum("usage", async () => { const u = await invoke("usage", { days: 7 }); return `${u.days.length} days, today ${u.today.length} projects`; });
  await sum("rules_list", async () => `${(await invoke("rules_list")).length} rules`);
  await sum("rendered", async () => `sidebar rows=${document.querySelectorAll(".srow").length}, pane="${$("#pane").innerText.replace(/\s+/g, " ").slice(0, 90)}"`);
}

tick().then(selfCheck);
setInterval(tick, 1500);
