//! Pull-request querying, caching, and assignment workflow contracts.

use std::sync::Arc;

use fleet_core::{
    github::{PrTab, PullRequest},
    ids::{ContextId, RepoId},
};

use crate::{
    DaemonError, DaemonResult,
    jobs::JobManager,
    stores::{config::ConfigStore, state::StateStore},
};

/// Pull-request service with cache and global GitHub concurrency handles.
#[derive(Clone)]
pub struct Github {
    _config: Arc<ConfigStore>,
    _state: Arc<StateStore>,
    _jobs: Arc<JobManager>,
}

impl Github {
    /// Creates the pull-request service.
    #[must_use]
    pub fn new(config: Arc<ConfigStore>, state: Arc<StateStore>, jobs: Arc<JobManager>) -> Self {
        Self {
            _config: config,
            _state: state,
            _jobs: jobs,
        }
    }

    /// Loads cached PR slices and refreshes them with global concurrency four (inventory sections 2, 6, and 7).
    pub async fn list_pull_requests(
        &self,
        _repo: Option<RepoId>,
        _context: Option<ContextId>,
        _tab: PrTab,
        _force: bool,
    ) -> DaemonResult<Vec<PullRequest>> {
        Err(DaemonError::Unimplemented("github::list_pull_requests"))
    }
}
