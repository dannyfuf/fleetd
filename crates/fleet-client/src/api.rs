//! Typed high-level request and response operations.

mod agents;
mod daemon;
mod jobs;
mod repositories;
mod sessions;
mod terminals;
mod worktrees;

use fleet_core::model::Worktree;
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    job::JobRecord,
    response::ResponseBody,
};
use serde::{Deserialize, Serialize};

/// The result type returned by typed Fleet client operations.
pub type Result<T> = std::result::Result<T, ProtoError>;

/// Details returned by protocol negotiation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloResult {
    /// Negotiated protocol version.
    pub protocol: u32,
    /// Daemon build identifier.
    pub server: String,
}

/// Result of creating or idempotently finding a worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorktreeResult {
    /// Whether the request created a new worktree.
    pub created: bool,
    /// The resulting worktree.
    pub worktree: Worktree,
    /// Post-create hook job, when hooks were scheduled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_create_job: Option<JobRecord>,
}

/// Daemon version information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonVersion {
    /// Daemon build version.
    pub version: String,
    /// Supported wire protocol version.
    pub protocol: u32,
}

pub use agents::{AgentMirror, AgentSnapshot, MirrorOutcome};

pub(crate) fn expect_ack(operation: &str, response: ResponseBody) -> Result<()> {
    match response {
        ResponseBody::Ack => Ok(()),
        response => Err(unexpected(operation, response)),
    }
}

pub(crate) fn unexpected(operation: &str, response: ResponseBody) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unknown,
        message: format!("unexpected response to {operation}: {response:?}"),
    }
}
