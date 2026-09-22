//! Stable child instructions and caller delivery messages.
//!
//! Every string here is part of the delegation contract a child and a caller read, so each one is
//! a constant with a test asserting it verbatim rather than a format string at its call site.
use chrono::{DateTime, Utc};
use fleet_core::{
    agents::{AgentKind, Delegation, DelegationId, DelegationStatus},
    ids::BoardId,
};

/// The completion footer appended to the child's first message (design doc, verbatim).
///
/// `{fleet}` is the program the child must invoke. It is an absolute path whenever the daemon
/// resolved one, because the child reading this footer is the one surface where a bare name that
/// does not resolve costs the whole delegation.
pub(crate) const FOOTER_TEMPLATE: &str = "--- Fleet delegation {id} ---\n\
You are running as a subagent. No human is watching this session.\n\
The caller expects: {expectation}\n\
When the work is fully finished and verified, report it with exactly one command:\n  \
{fleet} subagent complete --result-file <path-to-your-report.md>\n\
Write the report first, then run the command. Do not run it before you are done.\n\
If you are blocked and cannot finish, run:\n  \
{fleet} subagent complete --blocked --result-file <path-with-what-you-need>\n\
Do not ask the user questions; state assumptions in the report instead.";

/// The completion footer appended to a *card* run's first message (contracts §3.3, verbatim).
///
/// A card run's child is told two things a thread run's child is not: which card and board it is
/// working for, and that it must not move that card — the report moves it, so a child that moved
/// it itself would race the outcome the board is about to write.
pub(crate) const CARD_FOOTER_TEMPLATE: &str = "--- Fleet run {id} for card {key} on board {board} ---\n\
You are running as a subagent. No human is watching this session.\n\
The card expects: {expectation}\n\
When the work is fully finished and verified, report it with exactly one command:\n  \
{fleet} subagent complete --result-file <path-to-your-report.md>\n\
Write the report first, then run the command. Do not run it before you are done.\n\
If you are blocked and cannot finish, run:\n  \
{fleet} subagent complete --blocked --result-file <path-with-what-you-need>\n\
Do not ask the user questions; state assumptions in the report instead.\n\
Do not move card {key}; its report moves it when you finish. You may comment on it and move other cards.";

/// The program named in the footer when the daemon could not resolve an absolute `fleet`.
pub(crate) const BARE_FLEET: &str = "fleet";

/// Sent to a child that settled a turn without reporting a result.
pub(crate) const NUDGE: &str = "You have not reported a result. If the work is done, run `fleet subagent complete --result-file <path>`. If not, continue.";
/// Sent to a child whose provider exited and was resumed.
pub(crate) const RESUME_NUDGE: &str =
    "The session was restarted. Continue, and report with `fleet subagent complete` when done.";
/// Returned beside `DelegationStarted` when the child inherits the caller's worktree by default.
///
/// Only the implicit default is warned about: a caller that passed `--worktree` naming its own
/// worktree chose that, which is what an orchestrator with disjoint file ownership does.
pub(crate) const SAME_WORKTREE_WARNING: &str = "no --worktree was passed, so the child edits the caller's worktree by default; end your turn before it works, or pass --worktree to isolate it";

/// Renders the completion footer with this delegation's identity, expectation and `fleet` program.
pub(crate) fn footer(id: DelegationId, expectation: &str, fleet: &str) -> String {
    // `{fleet}` first and `{expectation}` last: only the expectation is caller-supplied text, so
    // it can never introduce a placeholder that a later pass would substitute.
    FOOTER_TEMPLATE
        .replace("{fleet}", fleet)
        .replace("{id}", &id.to_string())
        .replace("{expectation}", expectation)
}

/// Builds the first child message from its brief and completion footer.
pub(crate) fn first_message(
    brief: &str,
    id: DelegationId,
    expectation: &str,
    fleet: &str,
) -> String {
    let brief = brief.trim_end_matches(['\r', '\n']);
    format!("{brief}\n\n{}", footer(id, expectation, fleet))
}

/// Printed in place of an empty expectation, so the card footer never names nothing.
///
/// A column may leave `expect` empty (contracts §1.1); the child still reads the line, and a bare
/// `The card expects:` would read as a truncated instruction rather than an absent one.
pub(crate) const NO_EXPECTATION: &str = "(the column names no expectation)";

/// Renders a card run's completion footer with its identity, card, board and `fleet` program.
///
/// Substitution order is `{fleet}` → `{id}` → `{key}` → `{board}` → `{expectation}`: the
/// expectation is the only column-authored text, so it is substituted last and can never
/// introduce a placeholder a later pass would fill in.
pub(crate) fn card_footer(
    id: DelegationId,
    key: &str,
    board: &BoardId,
    expectation: &str,
    fleet: &str,
) -> String {
    // Whitespace-only counts as empty: a column author who typed a space meant no expectation,
    // and the parenthetical says that where a blank line would only look truncated.
    let expectation = if expectation.trim().is_empty() {
        NO_EXPECTATION
    } else {
        expectation
    };
    CARD_FOOTER_TEMPLATE
        .replace("{fleet}", fleet)
        .replace("{id}", &id.to_string())
        .replace("{key}", key)
        .replace("{board}", board.as_str())
        .replace("{expectation}", expectation)
}

/// Builds a card run's first child message from its brief and its rendered footer.
///
/// The footer arrives rendered rather than as its parts because a card footer needs the card, the
/// board and the resolved `fleet` program, which is four arguments the brief has no opinion about.
pub(crate) fn card_first_message(brief: &str, footer: &str) -> String {
    let brief = brief.trim_end_matches(['\r', '\n']);
    format!("{brief}\n\n{footer}")
}

/// Builds a card run's child thread title from the card's key and title.
///
/// The same first-line, 48-character cut [`child_title`] makes, so a card thread and a delegated
/// thread sit the same width in every list that shows both.
pub(crate) fn card_child_title(key: &str, title: &str) -> String {
    let first_line: String = title
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(48)
        .collect();
    format!("↳ {key} — {first_line}")
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
        DelegationCaller, DelegationResult, DeliveryState, ItemId, ResultSource, ThreadId, TurnId,
    };

    use super::*;

    fn board() -> BoardId {
        BoardId::try_from("board-1").expect("valid test board id")
    }

    fn delegation() -> Delegation {
        let created = DateTime::from_timestamp(1_700_000_000, 0).expect("valid test timestamp");
        Delegation {
            id: DelegationId::new(),
            caller: DelegationCaller::Thread(ThreadId::new()),
            caller_turn: Some(TurnId::new()),
            caller_item: Some(ItemId::new()),
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
            usage: None,
        }
    }

    #[test]
    fn footer_matches_the_contract_text_with_a_bare_program() {
        let id = DelegationId::new();
        assert_eq!(
            footer(id, "tests pass", BARE_FLEET),
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
    fn footer_names_the_resolved_absolute_path_in_both_commands() {
        let id = DelegationId::new();
        assert_eq!(
            footer(id, "tests pass", "/opt/fleet/bin/fleet"),
            format!(
                "--- Fleet delegation {id} ---\n\
You are running as a subagent. No human is watching this session.\n\
The caller expects: tests pass\n\
When the work is fully finished and verified, report it with exactly one command:\n  \
/opt/fleet/bin/fleet subagent complete --result-file <path-to-your-report.md>\n\
Write the report first, then run the command. Do not run it before you are done.\n\
If you are blocked and cannot finish, run:\n  \
/opt/fleet/bin/fleet subagent complete --blocked --result-file <path-with-what-you-need>\n\
Do not ask the user questions; state assumptions in the report instead."
            )
        );
    }

    #[test]
    fn an_expectation_naming_a_placeholder_is_never_substituted_again() {
        let id = DelegationId::new();
        let rendered = footer(id, "keep {fleet} and {id} literal", "/opt/fleet/bin/fleet");
        assert!(rendered.contains("The caller expects: keep {fleet} and {id} literal"));
    }

    /// Contract text a child reads, so it is pinned line by line rather than as one blob: a
    /// wrapped or reordered line is the kind of edit a diff hides and a child obeys.
    #[test]
    fn the_card_footer_template_is_the_contract_text() {
        assert_eq!(
            CARD_FOOTER_TEMPLATE.lines().collect::<Vec<_>>(),
            vec![
                "--- Fleet run {id} for card {key} on board {board} ---",
                "You are running as a subagent. No human is watching this session.",
                "The card expects: {expectation}",
                "When the work is fully finished and verified, report it with exactly one command:",
                "  {fleet} subagent complete --result-file <path-to-your-report.md>",
                "Write the report first, then run the command. Do not run it before you are done.",
                "If you are blocked and cannot finish, run:",
                "  {fleet} subagent complete --blocked --result-file <path-with-what-you-need>",
                "Do not ask the user questions; state assumptions in the report instead.",
                "Do not move card {key}; its report moves it when you finish. You may comment on it and move other cards.",
            ]
        );
    }

    /// The rendered text, not the template: a substitution that dropped a placeholder or filled
    /// one in the wrong order would leave this test's expectation untouched.
    #[test]
    fn the_card_footer_renders_every_placeholder() {
        let id = DelegationId::new();
        let board = board();
        assert_eq!(
            card_footer(id, "FLT-7", &board, "tests pass", "/opt/fleet/bin/fleet"),
            format!(
                "--- Fleet run {id} for card FLT-7 on board board-1 ---\n\
You are running as a subagent. No human is watching this session.\n\
The card expects: tests pass\n\
When the work is fully finished and verified, report it with exactly one command:\n  \
/opt/fleet/bin/fleet subagent complete --result-file <path-to-your-report.md>\n\
Write the report first, then run the command. Do not run it before you are done.\n\
If you are blocked and cannot finish, run:\n  \
/opt/fleet/bin/fleet subagent complete --blocked --result-file <path-with-what-you-need>\n\
Do not ask the user questions; state assumptions in the report instead.\n\
Do not move card FLT-7; its report moves it when you finish. You may comment on it and move other cards."
            )
        );
    }

    #[test]
    fn a_card_footer_with_no_expectation_names_the_column_instead() {
        let rendered = card_footer(DelegationId::new(), "FLT-7", &board(), "   ", BARE_FLEET);
        assert!(
            rendered.contains("The card expects: (the column names no expectation)"),
            "{rendered}"
        );
    }

    /// The card expectation is column-authored text, so it gets the guarantee the thread
    /// expectation has: it is substituted last and is never re-scanned for placeholders.
    #[test]
    fn a_card_expectation_naming_a_placeholder_is_never_substituted_again() {
        let rendered = card_footer(
            DelegationId::new(),
            "FLT-7",
            &board(),
            "keep {key} and {board} literal",
            BARE_FLEET,
        );
        assert!(
            rendered.contains("The card expects: keep {key} and {board} literal"),
            "{rendered}"
        );
    }

    #[test]
    fn card_first_message_separates_the_brief_and_footer() {
        assert_eq!(
            card_first_message("Do the work\r\n", "--- footer ---"),
            "Do the work\n\n--- footer ---"
        );
    }

    #[test]
    fn card_child_title_uses_the_first_48_characters_of_the_first_line() {
        let title = "123456789012345678901234567890123456789012345678EXTRA\nignored";
        assert_eq!(
            card_child_title("FLT-7", title),
            "↳ FLT-7 — 123456789012345678901234567890123456789012345678"
        );
    }

    #[test]
    fn first_message_separates_the_brief_and_footer() {
        let id = DelegationId::new();
        assert_eq!(
            first_message("Do the work", id, "ship it", BARE_FLEET),
            format!("Do the work\n\n{}", footer(id, "ship it", BARE_FLEET))
        );
    }

    #[test]
    fn first_message_normalizes_a_newline_terminated_brief() {
        let id = DelegationId::new();
        assert_eq!(
            first_message("Do the work\r\n", id, "ship it", BARE_FLEET),
            format!("Do the work\n\n{}", footer(id, "ship it", BARE_FLEET))
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
