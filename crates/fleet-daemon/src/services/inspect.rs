//! Worktree activity, process, and port inspection orchestration contracts.

use std::sync::Arc;

use fleet_core::{
    ids::{RepoId, WorktreeId},
    inspection::WorktreeInspection,
};

use crate::{
    DaemonError, DaemonResult,
    jobs::JobManager,
    stores::{config::ConfigStore, state::StateStore},
};

/// Worktree inspection service coordinating Git, GitHub, process, and remote facts.
#[derive(Clone)]
pub struct Inspect {
    _config: Arc<ConfigStore>,
    _state: Arc<StateStore>,
    _jobs: Arc<JobManager>,
}

impl Inspect {
    /// Creates the inspection service.
    #[must_use]
    pub fn new(config: Arc<ConfigStore>, state: Arc<StateStore>, jobs: Arc<JobManager>) -> Self {
        Self {
            _config: config,
            _state: state,
            _jobs: jobs,
        }
    }

    /// Inspects selected worktrees, preserving fail-closed warning semantics (inventory sections 1, 6, and 7).
    pub async fn worktrees(
        &self,
        _ids: Vec<WorktreeId>,
        _repo: Option<RepoId>,
        _fetch: bool,
    ) -> DaemonResult<Vec<WorktreeInspection>> {
        Err(DaemonError::Unimplemented("inspect::worktrees"))
    }
}
