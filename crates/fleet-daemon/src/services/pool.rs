//! Prepared-copy pool scheduling and claiming contracts.

use std::sync::Arc;

use fleet_core::{ids::RepoId, model::Repo};
use fleet_proto::snapshot::PoolStatus;

use crate::{
    DaemonError, DaemonResult,
    adapters::{files::Files, git::Git},
    jobs::JobManager,
    stores::{config::ConfigStore, state::StateStore},
};

/// Prepared-copy pool service serialized by per-repository job-manager locks.
#[derive(Clone)]
pub struct Pool {
    _config: Arc<ConfigStore>,
    _state: Arc<StateStore>,
    _jobs: Arc<JobManager>,
    _git: Arc<dyn Git>,
    _files: Arc<dyn Files>,
}

impl Pool {
    /// Creates the prepared-copy pool service.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        git: Arc<dyn Git>,
        files: Arc<dyn Files>,
    ) -> Self {
        Self {
            _config: config,
            _state: state,
            _jobs: jobs,
            _git: git,
            _files: files,
        }
    }

    /// Ensures configured slots exist with at most two repositories preparing concurrently (inventory sections 2 and 6).
    pub async fn prepare(&self, _repo: RepoId, _force: bool) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("pool::prepare"))
    }

    /// Claims one ready prepared copy for worktree publication.
    pub async fn claim(&self, _repo: RepoId) -> DaemonResult<Option<std::path::PathBuf>> {
        Err(DaemonError::Unimplemented("pool::claim"))
    }

    /// Summarizes every repository's pool until the persistent slot scanner is implemented.
    #[must_use]
    pub fn snapshot_statuses(repos: &[Repo], configured_size: u64) -> Vec<PoolStatus> {
        let size = u32::try_from(configured_size).unwrap_or(u32::MAX);
        repos
            .iter()
            .map(|repo| PoolStatus {
                repo: repo.id.clone(),
                ready: 0,
                size,
                refreshed_at: None,
            })
            .collect()
    }
}
