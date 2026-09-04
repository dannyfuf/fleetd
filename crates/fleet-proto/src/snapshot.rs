//! Complete daemon state snapshots used to initialize client mirrors.

use fleet_core::{
    ids::{ContextId, HostId, RepoId},
    model::{CloneJob, Context, Repo, Worktree},
    sessions::{Session, WorktreeStatus},
};
use serde::{Deserialize, Serialize};

use crate::job::JobRecord;

/// Daemon identity and lifetime information included in every snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonInfo {
    /// Daemon build version.
    pub version: String,
    /// Operating-system process identifier.
    pub pid: u32,
    /// ISO-8601 daemon start time.
    pub started_at: String,
    /// Absolute Fleet home directory.
    pub home: String,
}

/// Prepared-copy pool availability for one repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolStatus {
    /// Repository whose pool is summarized.
    pub repo: RepoId,
    /// Number of ready prepared copies.
    pub ready: u32,
    /// Configured target pool size.
    pub size: u32,
    /// ISO-8601 time of the most recent successful refresh.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<String>,
}

/// Reachability observation for one configured remote host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    /// Configured remote host identifier.
    pub id: HostId,
    /// Whether the most recent probe succeeded.
    pub reachable: bool,
    /// ISO-8601 time of the most recent probe.
    pub checked_at: String,
    /// Probe failure detail when unreachable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Complete initial state sent to a newly connected client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// ISO-8601 time at which this snapshot was assembled.
    pub generated_at: String,
    /// Registered contexts.
    pub contexts: Vec<Context>,
    /// Registered repositories.
    pub repos: Vec<Repo>,
    /// Persisted clone jobs.
    pub clones: Vec<CloneJob>,
    /// Published worktrees.
    pub worktrees: Vec<Worktree>,
    /// Active context identifier.
    pub active_context: Option<ContextId>,
    /// Daemon-owned terminal sessions.
    pub sessions: Vec<Session>,
    /// Runtime worktree status summaries.
    pub statuses: Vec<WorktreeStatus>,
    /// Prepared-copy availability by repository.
    #[serde(default)]
    pub pools: Vec<PoolStatus>,
    /// Reachability of configured remote hosts.
    #[serde(default)]
    pub hosts: Vec<HostStatus>,
    /// Background jobs.
    pub jobs: Vec<JobRecord>,
    /// Daemon identity and lifetime facts.
    pub daemon: DaemonInfo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_status_types_round_trip() {
        let pool = PoolStatus {
            repo: RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
            ready: 1,
            size: 2,
            refreshed_at: Some("2026-09-04T12:00:00Z".to_owned()),
        };
        let encoded = serde_json::to_string(&pool).unwrap_or_else(|error| panic!("{error}"));
        let decoded: PoolStatus =
            serde_json::from_str(&encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, pool);

        let host = HostStatus {
            id: HostId::try_from("devbox").unwrap_or_else(|error| panic!("{error}")),
            reachable: false,
            checked_at: "2026-09-04T12:00:00Z".to_owned(),
            error: Some("ssh timed out".to_owned()),
        };
        let encoded = serde_json::to_string(&host).unwrap_or_else(|error| panic!("{error}"));
        let decoded: HostStatus =
            serde_json::from_str(&encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, host);
    }
}
