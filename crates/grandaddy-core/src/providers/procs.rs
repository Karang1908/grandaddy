//! Which program a command line runs.
use std::path::Path;

/// "claude" when the command (or the program it wraps) is Claude Code.
pub fn tool_of(args: &str) -> Option<&'static str> {
    args.split_whitespace().take(2).find_map(|tok| (Path::new(tok).file_name()?.to_str()? == "claude").then_some("claude"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn spots_claude() {
        assert_eq!(super::tool_of("/Users/me/.local/bin/claude --model haiku"), Some("claude"));
        assert_eq!(super::tool_of("node /x/claude"), Some("claude"));
        assert_eq!(super::tool_of("codex"), None);
    }
}
