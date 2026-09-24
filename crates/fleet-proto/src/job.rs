//! Wire representations of background jobs and progress.

use serde::{Deserialize, Serialize};

use fleet_core::ids::JobId;

/// Target prefix reserved for silent app-originated inspection sweeps.
///
/// The daemon still records these as jobs for scheduling, cancellation, and harness accounting;
/// app chrome uses the prefix to distinguish them from user-visible inspection jobs.
pub const BACKGROUND_INSPECTION_TARGET_PREFIX: &str = "background-inspect-";

/// Kind of durable daemon background work.
///
/// Every kind but [`JobKind::ScheduledTask`] is spelt as its snake_case name. That one travels
/// through the `Custom` extension point, as `{"custom":"scheduled_task"}`: jobs reach every peer
/// in the snapshot and in `JobUpdated`, and a peer built before schedules existed decodes a
/// custom kind but would drop the connection on an unknown unit variant (ADR 0025).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "WireJobKind", into = "WireJobKind")]
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
    /// Delete a repository and its descendants.
    DeleteRepo,
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
    /// Import compatible configuration and state from swarm.
    Import,
    /// Run one fire of a scheduled agent task; the target is the schedule id.
    ScheduledTask,
    /// Extension point for daemon-specific jobs.
    Custom(String),
}

/// The `Custom` name [`JobKind::ScheduledTask`] travels under.
const SCHEDULED_TASK_WIRE: &str = "scheduled_task";

/// [`JobKind`] as it is spelt on the wire.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireJobKind {
    Clone,
    PoolBuild,
    PoolRefresh,
    CreateWorktree,
    DeleteWorktree,
    DeleteRepo,
    Prune,
    Inspect,
    PostCreateHooks,
    PrFetch,
    RepoFetch,
    RepoDiscovery,
    Update,
    Import,
    /// Never written; read so a bare `"scheduled_task"` still decodes.
    ScheduledTask,
    Custom(String),
}

impl From<JobKind> for WireJobKind {
    fn from(kind: JobKind) -> Self {
        match kind {
            JobKind::Clone => Self::Clone,
            JobKind::PoolBuild => Self::PoolBuild,
            JobKind::PoolRefresh => Self::PoolRefresh,
            JobKind::CreateWorktree => Self::CreateWorktree,
            JobKind::DeleteWorktree => Self::DeleteWorktree,
            JobKind::DeleteRepo => Self::DeleteRepo,
            JobKind::Prune => Self::Prune,
            JobKind::Inspect => Self::Inspect,
            JobKind::PostCreateHooks => Self::PostCreateHooks,
            JobKind::PrFetch => Self::PrFetch,
            JobKind::RepoFetch => Self::RepoFetch,
            JobKind::RepoDiscovery => Self::RepoDiscovery,
            JobKind::Update => Self::Update,
            JobKind::Import => Self::Import,
            JobKind::ScheduledTask => Self::Custom(SCHEDULED_TASK_WIRE.to_owned()),
            JobKind::Custom(name) => Self::Custom(name),
        }
    }
}

impl From<WireJobKind> for JobKind {
    fn from(kind: WireJobKind) -> Self {
        match kind {
            WireJobKind::Clone => Self::Clone,
            WireJobKind::PoolBuild => Self::PoolBuild,
            WireJobKind::PoolRefresh => Self::PoolRefresh,
            WireJobKind::CreateWorktree => Self::CreateWorktree,
            WireJobKind::DeleteWorktree => Self::DeleteWorktree,
            WireJobKind::DeleteRepo => Self::DeleteRepo,
            WireJobKind::Prune => Self::Prune,
            WireJobKind::Inspect => Self::Inspect,
            WireJobKind::PostCreateHooks => Self::PostCreateHooks,
            WireJobKind::PrFetch => Self::PrFetch,
            WireJobKind::RepoFetch => Self::RepoFetch,
            WireJobKind::RepoDiscovery => Self::RepoDiscovery,
            WireJobKind::Update => Self::Update,
            WireJobKind::Import => Self::Import,
            WireJobKind::ScheduledTask => Self::ScheduledTask,
            WireJobKind::Custom(name) if name == SCHEDULED_TASK_WIRE => Self::ScheduledTask,
            WireJobKind::Custom(name) => Self::Custom(name),
        }
    }
}

/// Lifecycle of a daemon background job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// Waiting for scheduling or a resource lock.
    Queued,
    /// Currently executing.
    Running,
    /// Cancellation was requested and shutdown is still in progress.
    Cancelling,
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
    /// Whether a failed or cancelled job may be started again.
    #[serde(default)]
    pub retryable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_round_trip;

    #[test]
    fn job_kinds_and_statuses_round_trip() {
        for kind in [
            JobKind::Clone,
            JobKind::Import,
            JobKind::ScheduledTask,
            JobKind::Custom("other".to_owned()),
        ] {
            assert_round_trip(kind);
        }
        assert_round_trip(JobStatus::Failed {
            error: "boom".to_owned(),
        });
        assert_round_trip(JobStatus::Cancelling);
    }

    /// A schedule's job reaches an older peer as a custom kind it can decode, and a bare
    /// `"scheduled_task"` still reads back.
    #[test]
    fn a_scheduled_task_travels_as_a_custom_kind() {
        assert_eq!(
            serde_json::to_value(JobKind::ScheduledTask).unwrap(),
            serde_json::json!({"custom": "scheduled_task"})
        );
        assert_eq!(
            serde_json::from_value::<JobKind>(serde_json::json!("scheduled_task")).unwrap(),
            JobKind::ScheduledTask
        );
        assert_eq!(
            serde_json::to_value(JobKind::Clone).unwrap(),
            serde_json::json!("clone")
        );
    }

    #[test]
    fn job_record_round_trips_retryability() {
        assert_round_trip(JobRecord {
            id: JobId::try_from("job-round-trip").unwrap_or_else(|error| panic!("{error}")),
            kind: JobKind::Import,
            target: "~/.swarm".to_owned(),
            title: "Import from swarm".to_owned(),
            status: JobStatus::Queued,
            progress: None,
            log_path: "/tmp/import.log".to_owned(),
            started_at: "2026-09-04T12:00:00Z".to_owned(),
            finished_at: None,
            cancellable: true,
            retryable: true,
        });
    }
}
