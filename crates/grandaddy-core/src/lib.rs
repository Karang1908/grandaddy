//! grandaddy-core: everything the `grandaddy` CLI and the desktop app share — finding coding
//! agent sessions, reading what they're doing, answering what they're waiting on (via the
//! Claude Code PermissionRequest hook), auto-yes rules, phone approvals, starting/resuming
//! sessions, history and token usage. State lives in ~/.grandaddy.
pub mod claude;
pub mod hook;
pub mod install;
pub mod launch;
pub mod others;
pub mod phone;
pub mod rules;
pub mod sessions;
pub mod util;

pub use claude::{Activity, Decision, Event, Request};
pub use sessions::Session;
