//! Pull-request querying, caching, and assignment workflow contracts.

use std::sync::Arc;

use fleet_core::{
    github::PrTab,
    ids::{ContextId, RepoId},
};
use fleet_proto::response::PrSlice;

use crate::{
    DaemonError, DaemonResult,
    adapters::github::Github as GithubAdapter,
    jobs::JobManager,
    stores::{config::ConfigStore, state::StateStore},
};

/// Pull-request service with cache and global GitHub concurrency handles.
#[derive(Clone)]
pub struct Github {
    _config: Arc<ConfigStore>,
    _state: Arc<StateStore>,
    _jobs: Arc<JobManager>,
    _github: Arc<dyn GithubAdapter>,
}

impl Github {
    /// Creates the pull-request service.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        github: Arc<dyn GithubAdapter>,
    ) -> Self {
        Self {
            _config: config,
            _state: state,
            _jobs: jobs,
            _github: github,
        }
    }

    /// Loads cached PR slices and refreshes them with global concurrency four (inventory sections 2, 6, and 7).
    pub async fn list_pull_requests(
        &self,
        _repo: Option<RepoId>,
        _context: Option<ContextId>,
        _tab: PrTab,
        _force: bool,
    ) -> DaemonResult<Vec<PrSlice>> {
        Err(DaemonError::Unimplemented("github::list_pull_requests"))
    }
}
