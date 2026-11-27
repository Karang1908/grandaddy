//! Settings in ~/.grandaddy/config.json. Every field has a default, so a missing or partial file
//! is fine; unknown fields are kept when saving.
use crate::util;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub awake: Awake,
    pub web: Web,
    /// S2S / A2A: lets sessions talk to each other through the grandaddy MCP server.
    pub experimental_mesh: bool,
    pub auto_continue: AutoContinue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoContinue {
    /// When a session stops on a usage limit, send it `message` once the limit lifts.
    pub enabled: bool,
    pub message: String,
}

impl Default for AutoContinue {
    fn default() -> Self {
        AutoContinue { enabled: false, message: "continue".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Awake {
    /// Keep the Mac from idle-sleeping while any session is working (the display still sleeps).
    pub enabled: bool,
    /// What a closed lid does while sessions work:
    ///   "sleep"  — the Mac sleeps as usual
    ///   "full"   — everything keeps running
    ///   "agents" — only the agents keep running: display off, other apps paused, sound muted,
    ///              Low Power Mode on; all undone when the lid opens
    /// Both lid modes need a one-time admin approval.
    pub lid: String,
    /// Older configs: true meant "full".
    #[serde(skip_serializing)]
    pub lid_closed: bool,
    /// Apps that keep running in "agents" mode, besides the ones running sessions.
    pub keep: Vec<String>,
}

impl Awake {
    pub fn lid_mode(&self) -> &str {
        match self.lid.as_str() {
            "full" | "agents" => &self.lid,
            _ if self.lid_closed => "full",
            _ => "sleep",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Web {
    /// Serve the phone page on your local network.
    pub enabled: bool,
    pub port: u16,
    /// Required in every request; part of the URL you open on your phone.
    pub token: String,
}

impl Default for Awake {
    fn default() -> Self {
        Awake { enabled: true, lid: String::new(), lid_closed: false, keep: DEFAULT_KEEP.iter().map(|s| s.to_string()).collect() }
    }
}
/// Kept running in "agents" mode: browsers (agents drive them), terminals, and anything that
/// records or is a call.
pub const DEFAULT_KEEP: &[&str] = &[
    "Google Chrome", "Safari", "Arc", "Firefox", "Brave Browser", "Microsoft Edge", "Dia", "Chromium",
    "Terminal", "iTerm2", "Ghostty", "Warp", "Alacritty", "kitty", "WezTerm", "Docker Desktop", "Docker",
    "zoom.us", "Microsoft Teams", "Webex", "FaceTime", "OBS", "Loom", "Granola", "Otter", "Wispr Flow",
];

impl Default for Web {
    fn default() -> Self {
        Web { enabled: false, port: 7341, token: String::new() }
    }
}

fn path() -> std::path::PathBuf {
    util::state().join("config.json")
}

pub fn load() -> Config {
    let mut c: Config = util::read_json(&path()).unwrap_or_default();
    if c.awake.lid.is_empty() && c.awake.lid_closed {
        c.awake.lid = "full".into(); // older configs; saved in the new form from here on
    }
    c
}

pub fn save(c: &Config) -> std::io::Result<()> {
    util::write_json(&path(), c)
}

/// Apply a change and save.
pub fn update(f: impl FnOnce(&mut Config)) -> Config {
    let mut c = load();
    f(&mut c);
    let _ = save(&c);
    c
}

