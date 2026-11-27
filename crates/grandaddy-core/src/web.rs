//! The phone page: answer what's waiting, glance at sessions and goals, and send a prompt —
//! from your phone's browser on the same Wi-Fi. Off by default. Every request must carry the
//! random token that's part of the URL (scan the QR code in the app). Plain HTTP, so keep it to
//! networks you trust; ntfy covers being away from home.
use crate::claude::{self, Decision};
use crate::sessions;
use crate::util;
use crate::{config, goals, prompt};
use serde_json::{json, Value};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tiny_http::{Header, Method, Response, Server};

pub fn ensure_token() -> String {
    let c = config::load();
    if !c.web.token.is_empty() {
        return c.web.token;
    }
    config::update(|c| c.web.token = util::random_token()).web.token
}

pub fn lan_ip() -> Option<String> {
    for iface in ["en0", "en1", "en2"] {
        if let Ok(o) = Command::new("ipconfig").args(["getifaddr", iface]).output() {
            let ip = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if o.status.success() && !ip.is_empty() {
                return Some(ip);
            }
        }
    }
    None
}

pub fn url() -> Option<String> {
    let c = config::load();
    Some(format!("http://{}:{}/?t={}", lan_ip()?, c.web.port, ensure_token()))
}

pub fn qr_svg(data: &str) -> String {
    qrcode::QrCode::new(data.as_bytes())
        .map(|q| q.render::<qrcode::render::svg::Color>().min_dimensions(220, 220).quiet_zone(true).build())
        .unwrap_or_default()
}

fn token_ok(url: &str, headers: &[Header], token: &str) -> bool {
    let from_query = url.split_once('?').map(|(_, q)| q.split('&').any(|kv| kv == format!("t={token}"))).unwrap_or(false);
    let from_header = headers.iter().any(|h| h.field.equiv("X-Grandaddy-Token") && h.value.as_str() == token);
    !token.is_empty() && (from_query || from_header)
}

fn json_response(v: Value, code: u16) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(v.to_string())
        .with_status_code(code)
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
        .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap())
}

pub fn state() -> Value {
    let live = sessions::collect();
    let waiting: Vec<Value> = live
        .iter()
        .flat_map(|s| {
            s.pending.iter().map(move |r| {
                json!({"id": r.id, "session": s.name, "tool": s.tool_label, "agent": r.agent, "kind": if r.is_question() { "question" } else { "permission" },
                       "what": claude::tool_summary(&r.tool, &r.input), "questions": r.questions()})
            })
        })
        .collect();
    let list: Vec<Value> = live
        .iter()
        .map(|s| json!({"key": s.key, "name": s.name, "tool": s.tool_label, "status": s.status, "folder": util::short(&s.cwd), "typable": prompt::typable(s)}))
        .collect();
    let gl: Vec<Value> = goals::list(&live).into_iter().take(10).map(|g| json!({"text": g.text, "status": g.status, "summary": g.summary})).collect();
    json!({"waiting": waiting, "sessions": list, "goals": gl})
}

fn handle(mut req: tiny_http::Request, token: &str) {
    let url = req.url().to_string();
    if !token_ok(&url, req.headers(), token) {
        let _ = req.respond(Response::from_string("Open the link from the Grandaddy app.").with_status_code(403));
        return;
    }
    let path = url.split('?').next().unwrap_or("/").to_string();
    let mut body = String::new();
    let _ = req.as_reader().read_to_string(&mut body);
    let input: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let resp = match (req.method(), path.as_str()) {
        (Method::Get, "/") => Response::from_string(PAGE.replace("__TOKEN__", token)).with_header(Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap()),
        (Method::Get, "/api/state") => json_response(state(), 200),
        (Method::Post, "/api/answer") => {
            let rid = input.get("id").and_then(|v| v.as_str()).unwrap_or("");
            match claude::pending().into_iter().find(|r| r.id == rid) {
                None => json_response(json!({"error": "already answered"}), 409),
                Some(r) => {
                    let d = match input.get("kind").and_then(|v| v.as_str()) {
                        Some("allow") => Decision::Allow,
                        Some("deny") => Decision::Deny { message: None },
                        _ => Decision::Answers { answers: input.get("answers").and_then(|v| v.as_object()).cloned().unwrap_or_default() },
                    };
                    match claude::answer(&r, &d) {
                        Ok(()) => json_response(json!({"ok": true}), 200),
                        Err(e) => json_response(json!({"error": e.to_string()}), 500),
                    }
                }
            }
        }
        (Method::Post, "/api/prompt") => {
            let key = input.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let text = input.get("text").and_then(|v| v.as_str()).unwrap_or("");
            match sessions::collect().into_iter().find(|s| s.key == key) {
                None => json_response(json!({"error": "that session has ended"}), 404),
                Some(s) if !prompt::typable(&s) => json_response(json!({"error": format!("{} can't take prompts from here — start it with grandaddy run", s.name)}), 409),
                Some(s) => match prompt::send(&s, text, false) {
                    Ok(d) => json_response(json!({"ok": true, "message": d.message}), 200),
                    Err(e) => json_response(json!({"error": e}), 400),
                },
            }
        }
        _ => json_response(json!({"error": "not found"}), 404),
    };
    let _ = req.respond(resp);
}

/// Serve until `stop` is set or the page is turned off in settings.
pub fn serve(stop: Arc<AtomicBool>) -> Result<(), String> {
    let c = config::load();
    let token = ensure_token();
    let server = Server::http(("0.0.0.0", c.web.port)).map_err(|e| format!("couldn't listen on port {}: {e}", c.web.port))?;
    while !stop.load(Ordering::Relaxed) && config::load().web.enabled {
        if let Ok(Some(req)) = server.recv_timeout(Duration::from_millis(500)) {
            let t = token.clone();
            std::thread::spawn(move || handle(req, &t));
        }
    }
    Ok(())
}

const PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="apple-mobile-web-app-capable" content="yes">
<title>Grandaddy</title>
<style>
:root{color-scheme:light dark;--bg:#f2f2f7;--card:#fff;--text:#1c1c1e;--sub:#6e6e73;--line:rgba(60,60,67,.18);--accent:#0a84ff;--warn:#d97706;--red:#d70015}
@media(prefers-color-scheme:dark){:root{--bg:#000;--card:#1c1c1e;--text:#f2f2f7;--sub:#98989f;--line:rgba(84,84,88,.6);--warn:#ff9f0a;--red:#ff453a}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:17px/1.4 -apple-system,system-ui,sans-serif;-webkit-font-smoothing:antialiased;padding:env(safe-area-inset-top) 16px 40px}
h1{font-size:34px;font-weight:700;letter-spacing:.01em;margin:18px 4px 4px}.lead{color:var(--sub);margin:0 4px 22px}
h2{font-size:20px;font-weight:600;margin:26px 4px 8px}
.card{background:var(--card);border-radius:14px;padding:14px 16px;margin-bottom:10px}
.who{font-size:15px;color:var(--sub);margin-bottom:4px}.q{font-weight:600;margin-bottom:12px}
pre{white-space:pre-wrap;word-break:break-all;font:15px ui-monospace,Menlo,monospace;background:var(--bg);border-radius:10px;padding:10px 12px;margin:0 0 12px}
button{font:inherit;border:0;border-radius:12px;padding:12px 14px;width:100%;margin-top:8px;background:var(--bg);color:var(--accent);font-weight:600}
button.primary{background:var(--accent);color:#fff}button.deny{color:var(--red)}
.row{display:flex;gap:8px}.row button{flex:1}
.opt{text-align:left;color:var(--text);font-weight:500}.opt small{display:block;color:var(--sub);font-weight:400;font-size:14px}
.opt.on{outline:2px solid var(--accent)}
.list .card{display:flex;justify-content:space-between;align-items:center;gap:10px}
.name{font-weight:600}.meta{font-size:15px;color:var(--sub)}
.dot{width:10px;height:10px;border-radius:50%;flex:none;background:#8e8e93}.working{background:#34c759}.needs{background:var(--warn)}.idle{background:var(--accent)}
textarea{width:100%;font:inherit;border:1px solid var(--line);border-radius:12px;padding:10px 12px;background:var(--card);color:var(--text);margin-top:6px}
select{font:inherit;width:100%;padding:10px;border-radius:12px;border:1px solid var(--line);background:var(--card);color:var(--text)}
.empty{color:var(--sub);margin:0 4px}.toast{position:fixed;left:16px;right:16px;bottom:20px;background:var(--text);color:var(--bg);padding:12px 16px;border-radius:14px;opacity:0;transition:opacity .2s}.toast.on{opacity:.95}
</style></head><body>
<h1>Grandaddy</h1><p class="lead" id="lead">Loading…</p>
<div id="waiting"></div>
<h2>Sessions</h2><div class="list" id="sessions"></div>
<h2>Send a prompt</h2><div class="card"><select id="to"></select><textarea id="text" rows="3" placeholder="What should it do next?"></textarea><button class="primary" id="send">Send</button></div>
<h2>Goals</h2><div id="goals"></div>
<div class="toast" id="toast"></div>
<script>
const T="__TOKEN__";const H={"X-Grandaddy-Token":T,"Content-Type":"application/json"};const picks={};
const esc=s=>String(s??"").replace(/[&<>"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]));
function toast(m){const t=document.getElementById("toast");t.textContent=m;t.className="toast on";setTimeout(()=>t.className="toast",2200)}
async function post(p,b){const r=await fetch(p,{method:"POST",headers:H,body:JSON.stringify(b)});const j=await r.json().catch(()=>({}));if(!r.ok)throw j.error||"Failed";return j}
async function answer(id,kind,answers){try{await post("/api/answer",{id,kind,answers});toast(kind==="deny"?"Denied":"Done");load()}catch(e){toast(e)}}
function render(s){
 const n=s.waiting.length;document.getElementById("lead").textContent=n?`${n} waiting on you`:`${s.sessions.length} session${s.sessions.length==1?"":"s"} running. Nothing needs you.`;
 document.getElementById("waiting").innerHTML=s.waiting.map(w=>{
  if(w.kind==="permission")return `<div class="card"><div class="who">${esc(w.session)} · ${esc(w.tool)}</div><div class="q">Wants to run</div><pre>${esc(w.what)}</pre><div class="row"><button class="deny" onclick="answer('${w.id}','deny')">Deny</button><button class="primary" onclick="answer('${w.id}','allow')">Allow</button></div></div>`;
  return w.questions.map((q,qi)=>`<div class="card"><div class="who">${esc(w.session)} asks</div><div class="q">${esc(q.question)}</div>${(q.options||[]).map((o,i)=>`<button class="opt" data-w="${w.id}" data-q="${qi}" data-i="${i}">${esc(o.label)}${o.description?`<small>${esc(o.description)}</small>`:""}</button>`).join("")}</div>`).join("")+`<button class="primary" onclick="sendAnswers('${w.id}')">Send answer${w.questions.length>1?"s":""}</button>`;
 }).join("");
 document.getElementById("sessions").innerHTML=s.sessions.map(x=>`<div class="card"><div><div class="name">${esc(x.name)}</div><div class="meta">${esc(x.tool)} · ${esc(x.status)} · ${esc(x.folder)}</div></div><span class="dot ${x.status==="needs you"?"needs":esc(x.status)}"></span></div>`).join("")||`<p class="empty">No sessions running.</p>`;
 const to=document.getElementById("to"),cur=to.value;to.innerHTML=s.sessions.filter(x=>x.typable).map(x=>`<option value="${esc(x.key)}">${esc(x.name)}</option>`).join("")||`<option value="">No session takes prompts — start one with grandaddy run</option>`;if(cur)to.value=cur;
 document.getElementById("goals").innerHTML=s.goals.map(g=>`<div class="card"><div class="name">${esc(g.text)}</div><div class="meta">${esc(g.status)}${g.summary?` — ${esc(g.summary)}`:""}</div></div>`).join("")||`<p class="empty">No goals yet.</p>`;
 for(const k in picks){const [w,q]=k.split(":");const b=document.querySelector(`.opt[data-w="${w}"][data-q="${q}"][data-i="${picks[k]}"]`);if(b)b.classList.add("on")}
 window.__state=s;
}
document.addEventListener("click",e=>{const b=e.target.closest(".opt");if(!b)return;const k=b.dataset.w+":"+b.dataset.q;picks[k]=+b.dataset.i;document.querySelectorAll(`.opt[data-w="${b.dataset.w}"][data-q="${b.dataset.q}"]`).forEach(o=>o.classList.toggle("on",o===b))});
function sendAnswers(id){const w=window.__state.waiting.find(x=>x.id===id);const a={};for(const [qi,q] of w.questions.entries()){const i=picks[id+":"+qi];if(i===undefined)return toast("Pick an answer for each question");a[q.question]=q.options[i].label}answer(id,"answers",a)}
document.getElementById("send").onclick=async()=>{const key=document.getElementById("to").value,text=document.getElementById("text").value.trim();if(!key||!text)return;try{const r=await post("/api/prompt",{key,text});document.getElementById("text").value="";toast(r.message||"Sent")}catch(e){toast(e)}};
async function load(){try{const r=await fetch("/api/state",{headers:H});if(r.ok)render(await r.json())}catch{}}
load();setInterval(load,2500);
</script></body></html>"#;
