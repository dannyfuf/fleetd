//! Complete daemon state snapshots used to initialize client mirrors.

use fleet_core::{
    ids::ContextId,
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

/// Complete initial state sent to a newly connected client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
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
    /// Background jobs.
    pub jobs: Vec<JobRecord>,
    /// Daemon identity and lifetime facts.
    pub daemon: DaemonInfo,
}
