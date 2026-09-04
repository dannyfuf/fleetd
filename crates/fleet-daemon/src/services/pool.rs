//! Prepared-copy pool scheduling and claiming contracts.

use std::sync::Arc;

use fleet_core::ids::RepoId;

use crate::{
    DaemonError, DaemonResult,
    jobs::JobManager,
    stores::{config::ConfigStore, state::StateStore},
};

/// Prepared-copy pool service serialized by per-repository job-manager locks.
#[derive(Clone)]
pub struct Pool {
    _config: Arc<ConfigStore>,
    _state: Arc<StateStore>,
    _jobs: Arc<JobManager>,
}

impl Pool {
    /// Creates the prepared-copy pool service.
    #[must_use]
    pub fn new(config: Arc<ConfigStore>, state: Arc<StateStore>, jobs: Arc<JobManager>) -> Self {
        Self {
            _config: config,
            _state: state,
            _jobs: jobs,
        }
    }

    /// Ensures configured slots exist with at most two repositories preparing concurrently (inventory sections 2 and 6).
    pub async fn prepare(&self, _repo: RepoId, _force: bool) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("pool::prepare"))
    }
}
