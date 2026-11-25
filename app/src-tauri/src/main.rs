// No console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Read;

fn main() {
    // The app binary can also be the Claude Code hook (`… hook`), so installing the hook from
    // the app works without the CLI. Handled before any window/runtime code starts.
    if std::env::args().nth(1).as_deref() == Some("hook") {
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        if let Some(out) = grandaddy_core::hook::run(&input) {
            println!("{out}");
        }
        return;
    }
    grandaddy_app_lib::run()
}
