//! Wire representations of background jobs and progress.

use serde::{Deserialize, Serialize};

pub use fleet_core::ids::JobId;

/// Kind of durable daemon background work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// Clone a pristine repository.
    Clone,
    /// Build a prepared-copy pool slot.
    PoolBuild,
    /// Refresh a prepared-copy pool slot.
    PoolRefresh,
    /// Create and publish a worktree.
    CreateWorktree,
    /// Delete a worktree.
    DeleteWorktree,
    /// Inspect and prune worktrees.
    Prune,
    /// Inspect worktrees.
    Inspect,
    /// Run post-create hooks.
    PostCreateHooks,
    /// Fetch pull requests.
    PrFetch,
    /// Fetch a pristine repository.
    RepoFetch,
    /// Discover repositories from GitHub.
    RepoDiscovery,
    /// Update Fleet itself.
    Update,
    /// Extension point for daemon-specific jobs.
    Custom(String),
}

/// Lifecycle of a daemon background job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// Waiting for scheduling or a resource lock.
    Queued,
    /// Currently executing.
    Running,
    /// Completed successfully.
    Succeeded,
    /// Completed unsuccessfully.
    Failed {
        /// Human-readable failure detail.
        error: String,
    },
    /// Explicitly cancelled.
    Cancelled,
}

/// Complete client-visible state of a daemon job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    /// Stable job identifier.
    pub id: JobId,
    /// Work category.
    pub kind: JobKind,
    /// Stable target key used for grouping and duplicate suppression.
    pub target: String,
    /// Human-readable title.
    pub title: String,
    /// Current lifecycle state.
    pub status: JobStatus,
    /// Most recent progress line.
    pub progress: Option<String>,
    /// Persistent raw log path.
    pub log_path: String,
    /// ISO-8601 execution start time.
    pub started_at: String,
    /// ISO-8601 completion time.
    pub finished_at: Option<String>,
    /// Whether explicit cancellation is currently supported.
    pub cancellable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_enums_round_trip() {
        for kind in [JobKind::Clone, JobKind::Custom("other".to_owned())] {
            let json = serde_json::to_string(&kind).unwrap_or_else(|error| panic!("{error}"));
            let decoded: JobKind =
                serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(decoded, kind);
        }
        let status = JobStatus::Failed {
            error: "boom".to_owned(),
        };
        let json = serde_json::to_string(&status).unwrap_or_else(|error| panic!("{error}"));
        let decoded: JobStatus =
            serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, status);
    }
}
