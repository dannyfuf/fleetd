//! Stable child instructions and caller delivery messages.
//!
//! Every string here is part of the delegation contract a child and a caller read, so each one is
//! a constant with a test asserting it verbatim rather than a format string at its call site.
// The callers — `run.rs`, `complete.rs` and `worker.rs` — land later in phase 3; the allowance
// comes off with them.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use fleet_core::agents::{AgentKind, Delegation, DelegationId, DelegationStatus};

/// The completion footer appended to the child's first message (design doc, verbatim).
pub(crate) const FOOTER_TEMPLATE: &str = "--- Fleet delegation {id} ---\n\
You are running as a subagent. No human is watching this session.\n\
The caller expects: {expectation}\n\
When the work is fully finished and verified, report it with exactly one command:\n  \
fleet subagent complete --result-file <path-to-your-report.md>\n\
Write the report first, then run the command. Do not run it before you are done.\n\
If you are blocked and cannot finish, run:\n  \
fleet subagent complete --blocked --result-file <path-with-what-you-need>\n\
Do not ask the user questions; state assumptions in the report instead.";

/// Sent to a child that settled a turn without reporting a result.
pub(crate) const NUDGE: &str = "You have not reported a result. If the work is done, run `fleet subagent complete --result-file <path>`. If not, continue.";
/// Sent to a child whose provider exited and was resumed.
pub(crate) const RESUME_NUDGE: &str =
    "The session was restarted. Continue, and report with `fleet subagent complete` when done.";
/// Returned beside `DelegationStarted` when the child shares the caller's worktree.
pub(crate) const SAME_WORKTREE_WARNING: &str =
    "the child edits the caller's worktree; end your turn before it works, or pass --worktree";

/// Renders the completion footer with this delegation's identity and expectation.
pub(crate) fn footer(id: DelegationId, expectation: &str) -> String {
    FOOTER_TEMPLATE
        .replace("{id}", &id.to_string())
        .replace("{expectation}", expectation)
}

/// Builds the first child message from its brief and completion footer.
pub(crate) fn first_message(brief: &str, id: DelegationId, expectation: &str) -> String {
    format!("{brief}\n\n{}", footer(id, expectation))
}

/// Formats a terminal delegation result for delivery into the caller's thread.
pub(crate) fn delivered_message(delegation: &Delegation, now: DateTime<Utc>) -> String {
    let elapsed_seconds = delegation.elapsed(now).num_seconds().max(0);
    let minutes = elapsed_seconds / 60;
    let seconds = elapsed_seconds % 60;
    let (text, files_changed, elided) = delegation.result.as_ref().map_or_else(
        || ("", 0, false),
        |result| {
            (
                result.text.as_str(),
                result.files_changed.len(),
                result.elided,
            )
        },
    );
    let mut message = format!(
        "[fleet subagent {} finished: {}]\nprovider: {}, thread: {}, duration: {minutes}m {seconds:02}s, files changed: {files_changed}\n\n{text}",
        delegation.id,
        terminal_status_word(delegation.status),
        provider_word(delegation.provider),
        delegation.child,
    );
    if elided {
        message.push_str(&format!("\n\n(report elided at {} bytes)", text.len()));
    }
    message
}

/// Builds the default child thread title from the provider and first brief line.
pub(crate) fn child_title(provider: AgentKind, brief: &str) -> String {
    let first_line: String = brief
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(48)
        .collect();
    format!("↳ {} — {first_line}", provider_word(provider))
}

const fn provider_word(provider: AgentKind) -> &'static str {
    match provider {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
    }
}

const fn terminal_status_word(status: DelegationStatus) -> &'static str {
    match status {
        DelegationStatus::Succeeded => "succeeded",
        DelegationStatus::Incomplete => "incomplete",
        DelegationStatus::Failed => "failed",
        DelegationStatus::Cancelled => "cancelled",
        DelegationStatus::Starting => "starting",
        DelegationStatus::Running => "running",
        DelegationStatus::Blocked => "blocked",
        DelegationStatus::Settling => "settling",
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use fleet_core::agents::{
        DelegationResult, DeliveryState, ItemId, ResultSource, ThreadId, TurnId,
    };

    use super::*;

    fn delegation() -> Delegation {
        let created = DateTime::from_timestamp(1_700_000_000, 0).expect("valid test timestamp");
        Delegation {
            id: DelegationId::new(),
            caller: ThreadId::new(),
            caller_turn: TurnId::new(),
            caller_item: ItemId::new(),
            child: ThreadId::new(),
            provider: AgentKind::Codex,
            depth: 1,
            brief: "Implement the transition rules".into(),
            expectation: "All focused tests pass".into(),
            eager: false,
            status: DelegationStatus::Succeeded,
            status_payload: None,
            result: Some(DelegationResult {
                text: "Implemented and verified.".into(),
                files_changed: vec!["src/one.rs".into(), "src/two.rs".into()],
                source: ResultSource::Reported,
                elided: false,
            }),
            nudges: 0,
            recoveries: 0,
            delivery: DeliveryState::Pending,
            created,
            finished: Some(created + Duration::seconds(842)),
            headline: None,
        }
    }

    #[test]
    fn footer_matches_the_contract_text() {
        let id = DelegationId::new();
        assert_eq!(
            footer(id, "tests pass"),
            format!(
                "--- Fleet delegation {id} ---\n\
You are running as a subagent. No human is watching this session.\n\
The caller expects: tests pass\n\
When the work is fully finished and verified, report it with exactly one command:\n  \
fleet subagent complete --result-file <path-to-your-report.md>\n\
Write the report first, then run the command. Do not run it before you are done.\n\
If you are blocked and cannot finish, run:\n  \
fleet subagent complete --blocked --result-file <path-with-what-you-need>\n\
Do not ask the user questions; state assumptions in the report instead."
            )
        );
    }

    #[test]
    fn first_message_separates_the_brief_and_footer() {
        let id = DelegationId::new();
        assert_eq!(
            first_message("Do the work", id, "ship it"),
            format!("Do the work\n\n{}", footer(id, "ship it"))
        );
    }

    #[test]
    fn delivered_message_has_the_contract_format() {
        let delegation = delegation();
        assert_eq!(
            delivered_message(&delegation, delegation.created),
            format!(
                "[fleet subagent {} finished: succeeded]\nprovider: codex, thread: {}, duration: 14m 02s, files changed: 2\n\nImplemented and verified.",
                delegation.id, delegation.child
            )
        );
    }

    #[test]
    fn delivered_message_appends_the_elided_byte_count() {
        let mut delegation = delegation();
        let result = delegation
            .result
            .as_mut()
            .expect("test delegation has a result");
        result.text = "é report".into();
        result.elided = true;
        assert!(
            delivered_message(&delegation, delegation.created)
                .ends_with("é report\n\n(report elided at 9 bytes)")
        );
    }

    #[test]
    fn child_title_uses_the_first_48_characters_of_the_first_line() {
        let brief = "123456789012345678901234567890123456789012345678EXTRA\nignored";
        assert_eq!(
            child_title(AgentKind::Claude, brief),
            "↳ claude — 123456789012345678901234567890123456789012345678"
        );
    }
}
