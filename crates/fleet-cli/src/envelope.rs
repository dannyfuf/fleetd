//! Stable protocol-versioned JSON output envelopes.

use fleet_core::{
    agents::AttentionKind,
    board::{BackendDescriptor, BackendSchema, Board, BoardSummary, Card},
    inspection::WorktreeInspection,
    model::{Repo, Worktree},
    sessions::{AgentActivity, WorktreeStatus},
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    response::{PruneSkipped, SleepKept, WorktreeDeleteResult},
};
use serde::Serialize;

/// The stable CLI JSON protocol version.
pub const PROTOCOL: u32 = 1;

/// A create result compatible with swarm protocol one.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEnvelope<'a> {
    pub protocol: u32,
    pub created: bool,
    pub worktree: &'a Worktree,
}

/// A list result compatible with swarm protocol one.
#[derive(Debug, Serialize)]
pub struct ListEnvelope<'a> {
    pub protocol: u32,
    pub version: &'a str,
    pub repos: &'a [Repo],
    pub worktrees: &'a [Worktree],
}

/// A session's watch metadata in a protocol-one envelope.
#[derive(Debug, Serialize)]
pub struct WatchesEnvelope<'a> {
    pub protocol: u32,
    pub watches: &'a [fleet_core::watches::Watch],
}

/// A multi-delete result compatible with swarm protocol one.
#[derive(Debug, Serialize)]
pub struct DeleteEnvelope<'a> {
    pub protocol: u32,
    pub ok: bool,
    pub results: &'a [WorktreeDeleteResult],
}

/// An inspection result compatible with swarm protocol one.
#[derive(Debug, Serialize)]
pub struct InspectEnvelope<'a> {
    pub protocol: u32,
    pub worktrees: &'a [WorktreeInspection],
}

/// A prune result compatible with swarm protocol one.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneEnvelope<'a> {
    pub protocol: u32,
    pub dry_run: bool,
    pub deleted: &'a [fleet_core::ids::WorktreeId],
    pub skipped: &'a [PruneSkipped],
}

/// A successful boolean operation compatible with swarm protocol one.
#[derive(Debug, Serialize)]
pub struct OkEnvelope {
    pub protocol: u32,
    pub ok: bool,
}

/// A successful quarantined-state reset.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetStateEnvelope<'a> {
    pub protocol: u32,
    pub archived_path: &'a str,
}

/// A refreshed status result compatible with swarm protocol one.
#[derive(Debug, Serialize)]
pub struct StatusEnvelope<'a> {
    pub protocol: u32,
    pub statuses: &'a [WorktreeStatus],
}

/// A successful explicit agent-activity signal.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusEnvelope<'a> {
    pub protocol: u32,
    pub ok: bool,
    pub session: &'a fleet_core::ids::SessionId,
    pub terminal_id: fleet_core::ids::TerminalId,
    pub activity: AgentActivity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention: Option<AttentionKind>,
}

/// A sleep result compatible with swarm protocol one.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepEnvelope<'a> {
    pub protocol: u32,
    pub kept: &'a [SleepKept],
    pub closed: &'a [String],
    pub session_killed: bool,
}

/// A complete board and its cards in a protocol-one envelope.
#[derive(Debug, Serialize)]
pub struct BoardEnvelope<'a> {
    pub protocol: u32,
    pub board: &'a Board,
    pub cards: &'a [Card],
}

/// Board summaries in a protocol-one envelope.
#[derive(Debug, Serialize)]
pub struct BoardListEnvelope<'a> {
    pub protocol: u32,
    pub boards: &'a [BoardSummary],
}

/// One created, inspected, or updated card in a protocol-one envelope.
#[derive(Debug, Serialize)]
pub struct BoardCardEnvelope<'a> {
    pub protocol: u32,
    pub card: &'a Card,
}

/// A worktree created from a card, with the card it links, in a protocol-one envelope.
#[derive(Debug, Serialize)]
pub struct BoardWorktreeEnvelope<'a> {
    pub protocol: u32,
    pub created: bool,
    pub card: &'a Card,
    pub worktree: &'a Worktree,
}

/// A submitted board synchronization job and its completed result, in a protocol-one envelope.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSyncEnvelope<'a> {
    pub protocol: u32,
    pub job_id: &'a fleet_core::ids::JobId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job: Option<&'a fleet_proto::job::JobRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<&'a BoardSummary>,
}

/// The registered board backend kinds in a protocol-one envelope.
#[derive(Debug, Serialize)]
pub struct BoardBackendsEnvelope<'a> {
    pub protocol: u32,
    pub backends: &'a [BackendDescriptor],
}

/// What one board's backend reports about itself, in a protocol-one envelope.
#[derive(Debug, Serialize)]
pub struct BoardBackendSchemaEnvelope<'a> {
    pub protocol: u32,
    pub schema: &'a BackendSchema,
}

/// A protocol-compatible error result.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope<'a> {
    pub protocol: u32,
    pub error: ErrorBody<'a>,
}

/// The stable error payload nested inside [`ErrorEnvelope`].
#[derive(Debug, Serialize)]
pub struct ErrorBody<'a> {
    pub kind: ErrorKind,
    pub message: &'a str,
}

/// Serializes an envelope to one compact JSON line.
pub fn to_json(value: &impl Serialize) -> Result<String, ProtoError> {
    serde_json::to_string(value).map_err(|error| ProtoError {
        kind: ErrorKind::Unknown,
        message: single_line(&format!("could not serialize CLI output: {error}")),
    })
}

/// Serializes a protocol error to one compact JSON line.
pub fn error_json(error: &ProtoError) -> String {
    let message = single_line(&error.message);
    let envelope = ErrorEnvelope {
        protocol: PROTOCOL,
        error: ErrorBody {
            kind: error.kind,
            message: &message,
        },
    };
    serde_json::to_string(&envelope).unwrap_or_else(|_| {
        r#"{"protocol":1,"error":{"kind":"unknown","message":"could not serialize CLI error"}}"#
            .to_owned()
    })
}

/// Collapses arbitrary text into the envelope's required single line, control bytes removed.
///
/// Whitespace collapses to one space; every other control character is dropped. Card titles,
/// labels and assignees are printed through here, and an escape sequence in one of them would
/// otherwise repaint the terminal, set its window title, or hide the rest of the line.
#[must_use]
pub fn single_line(message: &str) -> String {
    message
        .split_whitespace()
        .map(|word| word.chars().filter(|c| !c.is_control()).collect::<String>())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Text a report prints across several lines, with the control bytes a terminal would obey
/// removed: line breaks and tabs survive, escapes and carriage returns do not.
#[must_use]
pub fn safe_block(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_block_keeps_layout_and_drops_escapes() {
        assert_eq!(
            safe_block("one\n\ttwo \u{1b}[2Jthree\rfour"),
            "one\n\ttwo [2Jthreefour"
        );
    }

    #[test]
    fn error_envelope_has_exact_protocol_shape_and_kind_name() {
        let json = error_json(&ProtoError {
            kind: ErrorKind::NotFound,
            message: "missing\nworktree".to_owned(),
        });
        assert_eq!(
            json,
            r#"{"protocol":1,"error":{"kind":"not-found","message":"missing worktree"}}"#
        );
    }

    #[test]
    fn normalizes_diagnostics_to_a_single_line() {
        assert_eq!(single_line("first\n second\tthird"), "first second third");
    }

    #[test]
    fn strips_the_control_bytes_a_terminal_would_obey() {
        assert_eq!(
            single_line("Evil \u{1b}[31mRED\u{1b}[0m \u{1b}]0;pwned\u{7}title"),
            "Evil [31mRED[0m ]0;pwnedtitle"
        );
        // C1 controls are just as executable as C0 ones, and a word of nothing else is dropped.
        assert_eq!(single_line("a \u{9b}31m b"), "a 31m b");
        assert_eq!(single_line("a \u{7} b"), "a b");
    }
}
