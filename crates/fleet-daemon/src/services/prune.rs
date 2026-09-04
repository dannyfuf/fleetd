//! Safe stale-worktree pruning workflow contracts.

use std::sync::Arc;

use fleet_core::ids::RepoId;
use fleet_proto::response::PruneResult;

use crate::{DaemonError, DaemonResult, jobs::JobManager, stores::state::StateStore};

/// Safe-prune service using one inspection pass followed by sequential deletions.
#[derive(Clone)]
pub struct Prune {
    _state: Arc<StateStore>,
    _jobs: Arc<JobManager>,
}

impl Prune {
    /// Creates the prune service.
    #[must_use]
    pub fn new(state: Arc<StateStore>, jobs: Arc<JobManager>) -> Self {
        Self {
            _state: state,
            _jobs: jobs,
        }
    }

    /// Prunes only merged, clean, inactive worktrees and reports every skip (inventory sections 3 and 6).
    pub async fn worktrees(
        &self,
        _dry_run: bool,
        _fetch: bool,
        _kill_sessions: bool,
        _repo: Option<RepoId>,
    ) -> DaemonResult<PruneResult> {
        Err(DaemonError::Unimplemented("prune::worktrees"))
    }
}
