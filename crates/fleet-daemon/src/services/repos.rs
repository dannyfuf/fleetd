//! Repository registration, discovery, caching, and clone orchestration contracts.

use std::sync::Arc;

use fleet_core::{
    github::RemoteRepo,
    ids::{ContextId, RepoId},
    model::Repo,
};
use fleet_proto::job::JobRecord;

use crate::{
    DaemonError, DaemonResult,
    jobs::JobManager,
    stores::{config::ConfigStore, state::StateStore},
};

/// Repository domain service with durable state, configuration, and job scheduling handles.
#[derive(Clone)]
pub struct Repos {
    _config: Arc<ConfigStore>,
    _state: Arc<StateStore>,
    _jobs: Arc<JobManager>,
}

impl Repos {
    /// Creates the repository service.
    #[must_use]
    pub fn new(config: Arc<ConfigStore>, state: Arc<StateStore>, jobs: Arc<JobManager>) -> Self {
        Self {
            _config: config,
            _state: state,
            _jobs: jobs,
        }
    }

    /// Starts detached `git clone --progress` and reconciliation (inventory sections 2, 6, and 7).
    pub async fn clone_repo(
        &self,
        _owner: String,
        _name: String,
        _url: String,
        _context: ContextId,
    ) -> DaemonResult<JobRecord> {
        Err(DaemonError::Unimplemented("repos::clone_repo"))
    }

    /// Cascades repository deletion through sessions, worktrees, pool slots, and trash (inventory sections 1, 2, and 6).
    pub async fn delete(&self, _repo: RepoId) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("repos::delete"))
    }

    /// Moves a repository to an existing context in one state transaction (inventory sections 1 and 6).
    pub async fn move_to_context(&self, _repo: RepoId, _context: ContextId) -> DaemonResult<Repo> {
        Err(DaemonError::Unimplemented("repos::move_to_context"))
    }

    /// Searches cached/discovered owner repositories using swarm matching semantics (inventory sections 1 and 6).
    pub async fn search_remote(
        &self,
        _owner: String,
        _query: String,
    ) -> DaemonResult<Vec<RemoteRepo>> {
        Err(DaemonError::Unimplemented("repos::search_remote"))
    }

    /// Loads or refreshes the owner repository cache through exact `gh` commands (inventory sections 2, 6, and 7).
    pub async fn list_remote(&self, _owner: String, _force: bool) -> DaemonResult<Vec<RemoteRepo>> {
        Err(DaemonError::Unimplemented("repos::list_remote"))
    }
}
