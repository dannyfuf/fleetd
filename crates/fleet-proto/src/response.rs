//! Daemon-to-client request responses.

use fleet_core::{
    config::Config,
    github::{PullRequest, RemoteRepo},
    ids::{JobId, WorktreeId},
    inspection::WorktreeInspection,
    model::{Context, Repo, Worktree},
    sessions::{Session, Terminal},
};
use serde::{Deserialize, Serialize};

use crate::{error::ProtoError, job::JobRecord, snapshot::Snapshot};

/// A correlated daemon response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// Correlation identifier copied from the request.
    pub id: u64,
    /// Successful response payload or stable protocol error.
    pub result: Result<ResponseBody, ProtoError>,
}

/// Result of one worktree in a multi-delete request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeDeleteResult {
    /// Requested worktree.
    pub worktree_id: WorktreeId,
    /// Whether it was deleted.
    pub ok: bool,
    /// Failure reason when deletion failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A worktree skipped by safe prune.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneSkipped {
    /// Skipped worktree.
    pub worktree_id: WorktreeId,
    /// Safety reason.
    pub reason: String,
    /// Derived merged state.
    pub merged: bool,
    /// Whether uncommitted changes exist.
    pub dirty: bool,
    /// Unique commits when calculable.
    pub unique_commits: Option<u64>,
    /// Keep-alive labels.
    pub running: Vec<String>,
}

/// Complete safe-prune outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneResult {
    /// Whether no deletions were performed.
    pub dry_run: bool,
    /// Deleted or would-delete worktree identifiers.
    pub deleted: Vec<WorktreeId>,
    /// Worktrees rejected by safety checks or failed deletion.
    pub skipped: Vec<PruneSkipped>,
}

/// One terminal kept while sleeping a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepKept {
    /// Terminal name.
    pub window: String,
    /// Comma-separated preservation reason.
    pub reason: String,
}

/// Session sleep outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepResult {
    /// Terminals preserved by policy.
    pub kept: Vec<SleepKept>,
    /// Closed terminal names.
    pub closed: Vec<String>,
    /// Whether no terminal remained and the session was removed.
    pub session_killed: bool,
}

/// One environment diagnostic result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheck {
    /// Stable check name.
    pub check: String,
    /// Whether the check passed.
    pub ok: bool,
    /// Human-readable observed detail.
    pub detail: String,
}

/// Every successful daemon result payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ResponseBody {
    /// Successful protocol negotiation.
    Hello {
        /// Negotiated protocol version.
        protocol: u32,
        /// Daemon build version.
        server: String,
    },
    /// Complete daemon snapshot.
    Snapshot(Snapshot),
    /// Operation succeeded without a richer payload.
    Ack,
    /// Created or updated context.
    Context(Context),
    /// Registered repository.
    Repo(Repo),
    /// Repository clone was accepted as a background job.
    CloneStarted(JobRecord),
    /// GitHub repository discovery results.
    RemoteRepos(Vec<RemoteRepo>),
    /// Created or idempotently returned worktree.
    Worktree {
        /// Whether this request created the worktree.
        created: bool,
        /// Resulting worktree.
        worktree: Worktree,
    },
    /// Multi-worktree deletion results.
    WorktreesDeleted(Vec<WorktreeDeleteResult>),
    /// Worktree inspection results.
    Inspections(Vec<WorktreeInspection>),
    /// Safe-prune outcome.
    Pruned(PruneResult),
    /// Session sleep outcome.
    Slept(SleepResult),
    /// Resolved local worktree path.
    Path(String),
    /// Pull-request query results.
    PullRequests(Vec<PullRequest>),
    /// Created or changed session.
    Session(Session),
    /// Current daemon-owned sessions.
    Sessions(Vec<Session>),
    /// Created or changed terminal.
    Terminal(Terminal),
    /// Current and recently completed jobs.
    Jobs(Vec<JobRecord>),
    /// One accepted or changed job.
    Job(JobRecord),
    /// A job was cancelled.
    JobCancelled(JobId),
    /// Trailing job log lines.
    JobLog(Vec<String>),
    /// Effective merged configuration.
    Config(Config),
    /// Environment diagnostic results.
    Doctor(Vec<DoctorCheck>),
    /// Daemon liveness response.
    Pong,
    /// Daemon version information.
    Version {
        /// Daemon build version.
        version: String,
        /// Supported wire protocol version.
        protocol: u32,
    },
    /// Daemon accepted an explicit shutdown request.
    ShuttingDown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_body_and_result_round_trip() {
        let response = Response {
            id: 9,
            result: Ok(ResponseBody::Version {
                version: "fleet 0.1.0".to_owned(),
                protocol: 1,
            }),
        };
        let json = serde_json::to_string(&response).unwrap_or_else(|error| panic!("{error}"));
        let decoded: Response =
            serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, response);

        let response = Response {
            id: 10,
            result: Err(ProtoError {
                kind: crate::error::ErrorKind::Validation,
                message: "bad input".to_owned(),
            }),
        };
        let json = serde_json::to_string(&response).unwrap_or_else(|error| panic!("{error}"));
        let decoded: Response =
            serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, response);
    }
}
