//! Prepared worktree creation, recovery, publication, and deletion contracts.

use std::sync::Arc;

use fleet_core::{
    ids::{HostId, RepoId, WorktreeId},
    model::{RepoHooks, Worktree},
};
use fleet_proto::response::WorktreeDeleteResult;

use crate::{
    DaemonError, DaemonResult,
    jobs::JobManager,
    stores::{config::ConfigStore, state::StateStore},
};

/// Worktree service owning prepared-copy claim, publication, and deletion orchestration.
#[derive(Clone)]
pub struct Worktrees {
    _config: Arc<ConfigStore>,
    _state: Arc<StateStore>,
    _jobs: Arc<JobManager>,
}

impl Worktrees {
    /// Creates the worktree service.
    #[must_use]
    pub fn new(config: Arc<ConfigStore>, state: Arc<StateStore>, jobs: Arc<JobManager>) -> Self {
        Self {
            _config: config,
            _state: state,
            _jobs: jobs,
        }
    }

    /// Claims or copies a prepared slot and atomically publishes a worktree (inventory section 2).
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        _repo: RepoId,
        _slug: String,
        _branch: Option<String>,
        _base: Option<String>,
        _host: Option<HostId>,
        _hooks: RepoHooks,
    ) -> DaemonResult<(bool, Worktree)> {
        Err(DaemonError::Unimplemented("worktrees::create"))
    }

    /// Creates a worktree from the exact pull-request head ref (inventory sections 1, 2, and 7).
    pub async fn create_from_pr(
        &self,
        _repo: RepoId,
        _number: u64,
    ) -> DaemonResult<(bool, Worktree)> {
        Err(DaemonError::Unimplemented("worktrees::create_from_pr"))
    }

    /// Deletes worktrees independently using transactional trash rollback (inventory sections 2 and 6).
    pub async fn delete(&self, _ids: Vec<WorktreeId>) -> DaemonResult<Vec<WorktreeDeleteResult>> {
        Err(DaemonError::Unimplemented("worktrees::delete"))
    }

    /// Hard-kills the session associated with a worktree (inventory sections 4 and 6).
    pub async fn kill(&self, _id: WorktreeId) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("worktrees::kill"))
    }

    /// Updates `lastOpenedAt` transactionally without changing worktree identity (inventory section 1).
    pub async fn touch_opened(&self, _id: WorktreeId) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("worktrees::touch_opened"))
    }

    /// Resolves the absolute path of a local worktree and rejects remote mirrors (inventory sections 1 and 2).
    pub async fn path(&self, _id: WorktreeId) -> DaemonResult<String> {
        Err(DaemonError::Unimplemented("worktrees::path"))
    }
}
