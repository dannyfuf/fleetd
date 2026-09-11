//! Wire payloads for Fleet-owned turn checkpoints and the revert that restores one.
//!
//! A checkpoint is a hidden Git ref the daemon writes **before** a turn runs, and before the
//! first edit to a file inside one (`docs/NATIVE-AGENTS.md` §5). It is Fleet's, not the harness's:
//! reverting files and rewinding a model's context are different operations, and Codex's own
//! `thread/rollback` is both deprecated upstream and documented to leave the working tree alone.
//! So nothing here touches a conversation — [`CheckpointId`] addresses a tree, and a revert
//! answers with the files it put back.
//!
//! Both requests are additive on protocol 7 and gated on
//! [`AGENT_CHECKPOINTS_CAPABILITY`](crate::AGENT_CHECKPOINTS_CAPABILITY): a peer that never
//! advertised it is never sent them, and a daemon without the checkpoint service refuses them
//! with [`checkpoints_capability_error`] instead of answering an empty list, because an empty
//! list reads as "this thread has no checkpoints" — which is what the app draws `[u]` from.

use chrono::{DateTime, Utc};
use fleet_core::agents::{ThreadId, TurnId};
use serde::{Deserialize, Serialize};

use crate::error::{ErrorKind, ProtoError};

/// Digits an ordinal is padded to inside a [`CheckpointId`].
///
/// Padding exists so the daemon's ref namespace sorts the same way `for-each-ref` prints it;
/// a thread past `99999` checkpoints keeps working and simply stops being lexicographically
/// ordered, which nothing depends on — every reader sorts on [`TurnCheckpoint::ordinal`].
const ORDINAL_DIGITS: usize = 5;

/// Most paths one [`AgentRevertReport`] names before it stops listing and only counts.
///
/// A revert of a turn that rewrote a generated directory can touch thousands of files, and the
/// report is rendered as one line of prose. The counts stay exact; the sample is what a human
/// reads.
pub const REVERT_PATH_SAMPLE: usize = 50;

/// What one checkpoint covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointScope {
    /// The whole worktree, captured before a turn was submitted.
    Turn,
    /// The files one edit was about to touch, captured before it ran.
    File,
}

impl CheckpointScope {
    /// The token this scope occupies inside a [`CheckpointId`].
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::File => "file",
        }
    }

    /// Parses the token [`CheckpointScope::as_token`] writes.
    fn from_token(token: &str) -> Option<Self> {
        match token {
            "turn" => Some(Self::Turn),
            "file" => Some(Self::File),
            _ => None,
        }
    }
}

/// A checkpoint's identity inside its thread: `<ordinal>-<scope>-<turn>`.
///
/// Opaque to every client — it is built by the daemon and echoed back verbatim in
/// [`RequestBody::AgentRevert`](crate::request::RequestBody::AgentRevert) — but it is *not* opaque
/// to the daemon, which re-parses it with [`CheckpointId::parse`] before it reaches Git. That is
/// deliberate: the id becomes the leaf of a `refs/fleet/checkpoints/<thread>/` ref name, so an
/// unvalidated one would be a ref-injection primitive. `Deserialize` is transparent and does
/// **not** validate, exactly as the rest of this wire treats ids: a malformed value must produce
/// a typed `Validation` refusal from the service, not a frame the peer cannot decode.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CheckpointId(String);

impl CheckpointId {
    /// Builds the canonical id for a checkpoint the daemon just took.
    #[must_use]
    pub fn from_parts(ordinal: u32, scope: CheckpointScope, turn: TurnId) -> Self {
        Self(format!(
            "{ordinal:0width$}-{scope}-{turn}",
            width = ORDINAL_DIGITS,
            scope = scope.as_token()
        ))
    }

    /// Parses and validates an id received from a peer.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidCheckpointId`] when the value is not exactly the shape
    /// [`CheckpointId::from_parts`] writes: an all-digit ordinal of at least [`ORDINAL_DIGITS`]
    /// digits, a known scope token, and a UUID turn identifier.
    pub fn parse(value: &str) -> Result<ParsedCheckpointId, InvalidCheckpointId> {
        let invalid = || InvalidCheckpointId {
            value: value.to_owned(),
        };
        let (ordinal, rest) = value.split_once('-').ok_or_else(invalid)?;
        let (scope, turn) = rest.split_once('-').ok_or_else(invalid)?;
        if ordinal.len() < ORDINAL_DIGITS || !ordinal.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        let ordinal = ordinal.parse::<u32>().map_err(|_| invalid())?;
        let scope = CheckpointScope::from_token(scope).ok_or_else(invalid)?;
        let turn = turn.parse::<TurnId>().map_err(|_| invalid())?;
        Ok(ParsedCheckpointId {
            id: Self::from_parts(ordinal, scope, turn),
            ordinal,
            scope,
            turn,
        })
    }

    /// The wire representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CheckpointId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// The three fields a validated [`CheckpointId`] carries, plus the canonical id itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCheckpointId {
    /// The id, re-rendered canonically, so a peer's padding can never reach a ref name.
    pub id: CheckpointId,
    /// Capture order within the thread, one-based.
    pub ordinal: u32,
    /// What the checkpoint covers.
    pub scope: CheckpointScope,
    /// The turn it was captured for.
    pub turn: TurnId,
}

/// A [`CheckpointId`] that is not the shape the daemon writes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{value}` is not a Fleet checkpoint identifier")]
pub struct InvalidCheckpointId {
    /// The rejected value, quoted back so a log line names it.
    pub value: String,
}

/// One checkpoint a thread can be reverted to.
///
/// Deliberately without the captured paths: listing is one `git for-each-ref`, and a file
/// checkpoint's paths are read from the checkpoint itself at revert time. Adding them here would
/// make a listing O(checkpoints) subprocesses for information the `[u]` affordance does not need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCheckpoint {
    /// Identity inside the thread.
    pub id: CheckpointId,
    /// What it covers.
    pub scope: CheckpointScope,
    /// The turn it was captured for.
    pub turn: TurnId,
    /// Capture order within the thread, one-based.
    pub ordinal: u32,
    /// When it was captured.
    pub at: DateTime<Utc>,
}

/// What a revert did to the working tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRevertReport {
    /// Thread whose worktree was restored.
    pub thread: ThreadId,
    /// Checkpoint the tree was restored to.
    pub checkpoint: CheckpointId,
    /// Files written back from the checkpoint.
    pub restored: u32,
    /// Files removed because the checkpoint did not have them.
    pub deleted: u32,
    /// Up to [`REVERT_PATH_SAMPLE`] of the affected paths, worktree-relative, sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

impl AgentRevertReport {
    /// Whether the working tree already matched the checkpoint.
    #[must_use]
    pub const fn is_noop(&self) -> bool {
        self.restored == 0 && self.deleted == 0
    }
}

/// The refusal for a checkpoint request sent to a daemon without the checkpoint service.
///
/// [`ErrorKind::Unsupported`] rather than an empty list: `[u]` is drawn only where a checkpoint
/// exists, and "none exist" and "this daemon cannot tell you" must not render the same
/// (`DESIGN-SYSTEM.md` §7).
#[must_use]
pub fn checkpoints_capability_error() -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unsupported,
        message: format!(
            "this daemon does not implement Fleet turn checkpoints (capability `{}`)",
            crate::AGENT_CHECKPOINTS_CAPABILITY
        ),
    }
}

/// The refusal for a revert on a thread that has no such checkpoint.
///
/// [`ErrorKind::NotFound`] is load-bearing for the same reason `gate_closed_error` is a conflict:
/// the surface must be able to tell "there was nothing to revert to" — a checkpoint garbage
/// collected, or a `[u]` drawn from a stale listing — from a Git failure it should retry.
#[must_use]
pub fn checkpoint_missing_error(thread: ThreadId, checkpoint: &CheckpointId) -> ProtoError {
    ProtoError {
        kind: ErrorKind::NotFound,
        message: format!("checkpoint {checkpoint} of agent thread {thread}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identifier_round_trips_through_its_wire_form() {
        let turn = TurnId::new();
        let id = CheckpointId::from_parts(7, CheckpointScope::Turn, turn);

        assert_eq!(id.as_str(), format!("00007-turn-{turn}"));
        let parsed = CheckpointId::parse(id.as_str()).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(parsed.id, id);
        assert_eq!(parsed.ordinal, 7);
        assert_eq!(parsed.scope, CheckpointScope::Turn);
        assert_eq!(parsed.turn, turn);
    }

    #[test]
    fn a_peer_supplied_identifier_can_never_escape_its_ref_namespace() {
        let turn = TurnId::new();
        for hostile in [
            "",
            "00001",
            "00001-turn",
            "0001-turn-not-a-uuid",
            &format!("../../heads/main-turn-{turn}"),
            &format!("00001-plan-{turn}"),
            &format!("0001-turn-{turn}"),
            &format!("00001-turn-{turn}/../../../heads/main"),
            &format!("0000a-turn-{turn}"),
        ] {
            assert!(
                CheckpointId::parse(hostile).is_err(),
                "`{hostile}` must not parse as a checkpoint identifier"
            );
        }
    }

    #[test]
    fn a_padded_identifier_is_re_rendered_canonically() {
        let turn = TurnId::new();
        let parsed = CheckpointId::parse(&format!("000000012-file-{turn}"))
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(parsed.id.as_str(), format!("00012-file-{turn}"));
        assert_eq!(parsed.scope, CheckpointScope::File);
    }

    #[test]
    fn every_refusal_carries_a_kind_the_surface_can_branch_on() {
        assert_eq!(checkpoints_capability_error().kind, ErrorKind::Unsupported);
        assert!(
            checkpoints_capability_error()
                .message
                .contains(crate::AGENT_CHECKPOINTS_CAPABILITY)
        );
        let thread = ThreadId::new();
        let id = CheckpointId::from_parts(1, CheckpointScope::Turn, TurnId::new());
        let missing = checkpoint_missing_error(thread, &id);
        assert_eq!(missing.kind, ErrorKind::NotFound);
        assert!(missing.message.contains(id.as_str()));
    }

    #[test]
    fn a_report_that_changed_nothing_says_so() {
        let report = AgentRevertReport {
            thread: ThreadId::new(),
            checkpoint: CheckpointId::from_parts(1, CheckpointScope::Turn, TurnId::new()),
            restored: 0,
            deleted: 0,
            paths: Vec::new(),
        };

        assert!(report.is_noop());
        assert!(
            !AgentRevertReport {
                restored: 1,
                ..report
            }
            .is_noop()
        );
    }
}
