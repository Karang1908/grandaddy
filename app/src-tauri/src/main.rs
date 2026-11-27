// No console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Read;

fn main() {
    // The app binary can also be the Claude Code hook (`… hook`), so installing the hook from
    // the app works without the CLI. Handled before any window/runtime code starts.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("hook") {
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        if let Some(out) = grandaddy_core::hook::run(&input) {
            println!("{out}");
        }
        return;
    }
    if args.get(1).map(String::as_str) == Some("--ui-bridge") {
        let port = args.get(2).and_then(|p| p.parse().ok()).unwrap_or(7400);
        let dir = args.get(3).cloned().unwrap_or_else(|| "app/ui".into());
        if let Err(e) = grandaddy_app_lib::serve_bridge(port, &dir) {
            eprintln!("{e}");
        }
        return;
    }
    grandaddy_app_lib::run()
}
