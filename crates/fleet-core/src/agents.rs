//! Shared recognition of coding-agent commands.

/// Agent executable names recognized by Fleet.
pub const RECOGNIZED_AGENTS: [&str; 3] = ["claude", "opencode", "codex"];

/// Returns the recognized agent executable present in a process command line.
#[must_use]
pub fn recognized_agent(command: &str) -> Option<&'static str> {
    RECOGNIZED_AGENTS.into_iter().find(|agent| {
        command.split_whitespace().any(|part| {
            part.trim_matches(|character: char| matches!(character, '\'' | '"'))
                .rsplit('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case(agent))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_supported_agent_executables() {
        assert_eq!(recognized_agent("claude --resume"), Some("claude"));
        assert_eq!(recognized_agent("/opt/CLAUDE --resume"), Some("claude"));
        assert_eq!(
            recognized_agent("/usr/local/bin/opencode"),
            Some("opencode")
        );
        assert_eq!(
            recognized_agent("node /opt/tools/codex exec"),
            Some("codex")
        );
    }

    #[test]
    fn rejects_partial_and_unrelated_names() {
        assert_eq!(recognized_agent("claude-helper"), None);
        assert_eq!(recognized_agent("echo codexical"), None);
        assert_eq!(recognized_agent("cargo test"), None);
    }
}
