//! Daemon-to-client request responses.

use fleet_core::{
    cache::RepoCache,
    config::Config,
    github::{PrTab, PullRequest},
    ids::{JobId, WorktreeId},
    inspection::WorktreeInspection,
    model::{Context, Repo, Worktree},
    sessions::{Session, Terminal, WorktreeStatus},
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

/// Pull requests and fetch state for one authored/review tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrSlice {
    /// Tab represented by this slice.
    pub tab: PrTab,
    /// ISO-8601 time of the most recent successful fetch.
    pub fetched_at: String,
    /// Whether a replacement fetch is currently running.
    pub loading: bool,
    /// Most recent refresh error, while stale data remains usable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Total result count before the client-visible cap.
    pub total: usize,
    /// Client-visible pull requests, capped by the service.
    pub prs: Vec<PullRequest>,
}

/// Candidate base refs and their refresh state for the Create dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseRefs {
    /// Ordered `origin/*` ref candidates.
    pub refs: Vec<String>,
    /// Whether a fetch is currently updating the candidates.
    pub fetching: bool,
    /// ISO-8601 time at which candidates were last refreshed.
    pub fetched_at: String,
}

/// Current process-match count or validation error for one keep-alive rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepAliveRuleMatch {
    /// Stable keep-alive rule identifier.
    pub rule_id: String,
    /// Number of live processes matching the rule.
    pub count: u64,
    /// Pattern or process-observation error for a skipped rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    RemoteRepos(RepoCache),
    /// Candidate base refs for worktree creation.
    BaseRefs(BaseRefs),
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
    PullRequests(Vec<PrSlice>),
    /// Refreshed runtime worktree statuses.
    Statuses(Vec<WorktreeStatus>),
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
    /// Live match counts for configured sleep rules.
    KeepAliveRuleMatches(Vec<KeepAliveRuleMatch>),
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

    #[test]
    fn new_response_types_round_trip() {
        let responses = vec![
            ResponseBody::RemoteRepos(RepoCache {
                fetched_at: "2026-09-04T12:00:00Z".to_owned(),
                repos: Vec::new(),
            }),
            ResponseBody::BaseRefs(BaseRefs {
                refs: vec!["origin/main".to_owned()],
                fetching: false,
                fetched_at: "2026-09-04T12:00:00Z".to_owned(),
            }),
            ResponseBody::PullRequests(vec![PrSlice {
                tab: PrTab::Review,
                fetched_at: "2026-09-04T12:00:00Z".to_owned(),
                loading: true,
                error: Some("temporary failure".to_owned()),
                total: 101,
                prs: Vec::new(),
            }]),
            ResponseBody::Statuses(Vec::new()),
            ResponseBody::KeepAliveRuleMatches(vec![KeepAliveRuleMatch {
                rule_id: "claude".to_owned(),
                count: 2,
                error: None,
            }]),
        ];
        for response in responses {
            let json = serde_json::to_string(&response).unwrap_or_else(|error| panic!("{error}"));
            let decoded: ResponseBody =
                serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(decoded, response);
        }
    }
}
