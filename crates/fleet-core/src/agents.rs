//! Shared recognition of coding-agent commands.

/// Agent executable names recognized by Fleet.
pub const RECOGNIZED_AGENTS: [&str; 3] = ["claude", "opencode", "codex"];

const SUPPORTED_INTERPRETERS: [&str; 4] = ["node", "nodejs", "bun", "deno"];

/// Returns the recognized agent executable present in a process command line.
#[must_use]
pub fn recognized_agent(command: &str) -> Option<&'static str> {
    let mut parts = command.split_whitespace();
    let executable = executable_name(parts.next()?)?;
    find_agent(executable).or_else(|| {
        SUPPORTED_INTERPRETERS
            .iter()
            .any(|interpreter| executable.eq_ignore_ascii_case(interpreter))
            .then(|| parts.next().and_then(executable_name).and_then(find_agent))
            .flatten()
    })
}

fn executable_name(part: &str) -> Option<&str> {
    part.trim_matches(|character: char| matches!(character, '\'' | '"'))
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
}

fn find_agent(name: &str) -> Option<&'static str> {
    RECOGNIZED_AGENTS
        .into_iter()
        .find(|agent| name.eq_ignore_ascii_case(agent))
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

    #[test]
    fn arbitrary_argument_is_not_agent() {
        assert_eq!(recognized_agent("sleep 10 codex"), None);
        assert_eq!(recognized_agent("echo claude"), None);
        assert_eq!(recognized_agent("cargo run -- opencode"), None);
    }
}
