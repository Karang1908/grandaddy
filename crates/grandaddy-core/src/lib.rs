//! grandaddy-core: everything the `grandaddy` CLI and the desktop app share — finding coding
//! agent sessions across tools, reading what they're doing, answering what they're waiting on,
//! prompting them, goals, loops, git, staying awake, the phone page and the MCP server.
//! State lives in ~/.grandaddy.
pub mod awake;
pub mod claude;
pub mod commands;
pub mod config;
pub mod git;
pub mod goals;
pub mod hook;
pub mod install;
pub mod launch;
pub mod limits;
pub mod loops;
pub mod managed;
pub mod mcp;
pub mod phone;
pub mod prompt;
pub mod providers;
pub mod rules;
pub mod sessions;
pub mod util;
pub mod web;

pub use claude::{Activity, Decision, Event, Request};
pub use sessions::Session;
