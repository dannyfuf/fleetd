//! Stable protocol-versioned JSON output envelopes.

use fleet_core::{
    inspection::WorktreeInspection,
    model::{Repo, Worktree},
    sessions::WorktreeStatus,
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

/// A refreshed status result compatible with swarm protocol one.
#[derive(Debug, Serialize)]
pub struct StatusEnvelope<'a> {
    pub protocol: u32,
    pub statuses: &'a [WorktreeStatus],
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

/// Collapses arbitrary diagnostic text into the envelope's required single line.
#[must_use]
pub fn single_line(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
