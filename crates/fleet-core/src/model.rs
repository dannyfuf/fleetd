//! Domain models for contexts, repositories, worktrees, and clone jobs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{ContextId, HostId, RepoId, WorktreeId};

/// A user-defined grouping of repositories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    /// Stable context identifier.
    pub id: ContextId,
    /// Display name.
    pub name: String,
    /// GitHub owners included in the context.
    pub owners: Vec<String>,
    /// ISO-8601 creation time.
    pub created_at: String,
}

/// Commands run while preparing and publishing repository copies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoHooks {
    /// Commands run while preparing a copy.
    #[serde(default)]
    pub prepare: Vec<String>,
    /// Commands run after a worktree is published.
    #[serde(default)]
    pub post_create: Vec<String>,
}

/// A pristine repository clone registered with Fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    /// Stable `owner/name` identifier.
    pub id: RepoId,
    /// GitHub owner.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Selected clone URL.
    pub url: String,
    /// Owning context.
    pub context_id: ContextId,
    /// Default branch name.
    pub default_branch: String,
    /// Absolute pristine-clone path.
    pub path: String,
    /// ISO-8601 clone completion time.
    pub cloned_at: String,
    /// Prepare and post-create hooks.
    #[serde(default)]
    pub hooks: RepoHooks,
}

/// Current phase of a persisted clone job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloneStatus {
    /// The detached clone process is being launched.
    Starting,
    /// The repository clone is running.
    Cloning,
    /// The clone terminated unsuccessfully.
    Failed,
}

/// A repository clone that is in progress or awaiting reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneJob {
    /// Repository identifier used as the clone identifier.
    pub id: RepoId,
    /// GitHub owner.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Selected clone URL.
    pub url: String,
    /// Target context.
    pub context_id: ContextId,
    /// Expected default branch.
    pub default_branch: String,
    /// Final repository path.
    pub path: String,
    /// Private staging path.
    pub staging_path: String,
    /// Clone log path.
    pub log_path: String,
    /// Detached child PID, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// ISO-8601 start time.
    pub started_at: String,
    /// Current clone phase.
    pub status: CloneStatus,
    /// Failure detail, when failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Persisted failure metadata for a worktree that completed with degraded setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Degraded {
    /// Stable failure category, such as `post_create_hooks`.
    pub kind: String,
    /// Human-readable or command-index step that failed.
    pub step: String,
    /// Child exit code when one was available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// ISO-8601 failure time.
    pub at: String,
    /// Absolute path to the detailed operation log.
    pub log_path: String,
}

/// A published independent repository copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    /// Globally unique worktree identifier.
    pub id: WorktreeId,
    /// Parent repository identifier.
    pub repo_id: RepoId,
    /// Canonical worktree slug.
    pub slug: String,
    /// Checked-out local branch.
    pub branch: String,
    /// Ref used to create the branch.
    pub base_ref: String,
    /// Authoritative local or remote path.
    pub path: String,
    /// Authoritative daemon session name.
    pub session: String,
    /// Remote host identifier; absent means local.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<HostId>,
    /// ISO-8601 creation time.
    pub created_at: String,
    /// Most recent ISO-8601 open time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_opened_at: Option<String>,
    /// Setup failure retained after the worktree was otherwise published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded: Option<Degraded>,
}

/// Remote-host connection settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostConfigEntry {
    /// SSH destination accepted by the `ssh` command.
    pub ssh: String,
    /// Remote command prefix.
    #[serde(default = "default_swarm_command")]
    pub swarm_command: String,
}

fn default_swarm_command() -> String {
    "swarm".to_owned()
}

/// The ordered remote host map stored in configuration.
pub type Hosts = BTreeMap<HostId, HostConfigEntry>;
