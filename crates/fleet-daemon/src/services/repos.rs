//! Repository registration, discovery, caching, and clone orchestration contracts.

use std::sync::Arc;

use fleet_core::{
    cache::RepoCache,
    ids::{ContextId, RepoId},
    model::{Repo, RepoHooks},
};
use fleet_proto::{job::JobRecord, response::BaseRefs};

use crate::{
    DaemonError, DaemonResult,
    adapters::{files::Files, git::Git, github::Github},
    jobs::JobManager,
    stores::{config::ConfigStore, state::StateStore},
};

/// Repository domain service with durable state, configuration, and job scheduling handles.
#[derive(Clone)]
pub struct Repos {
    _config: Arc<ConfigStore>,
    _state: Arc<StateStore>,
    _jobs: Arc<JobManager>,
    _git: Arc<dyn Git>,
    _github: Arc<dyn Github>,
    _files: Arc<dyn Files>,
}

impl Repos {
    /// Creates the repository service.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        git: Arc<dyn Git>,
        github: Arc<dyn Github>,
        files: Arc<dyn Files>,
    ) -> Self {
        Self {
            _config: config,
            _state: state,
            _jobs: jobs,
            _git: git,
            _github: github,
            _files: files,
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
    pub async fn search_remote(&self, _owner: String, _query: String) -> DaemonResult<RepoCache> {
        Err(DaemonError::Unimplemented("repos::search_remote"))
    }

    /// Loads or refreshes the owner repository cache through exact `gh` commands (inventory sections 2, 6, and 7).
    pub async fn list_remote(&self, _owner: String, _force: bool) -> DaemonResult<RepoCache> {
        Err(DaemonError::Unimplemented("repos::list_remote"))
    }

    /// Lists remote-tracking base refs, optionally fetching first.
    pub async fn list_base_refs(&self, _repo: RepoId, _force: bool) -> DaemonResult<BaseRefs> {
        Err(DaemonError::Unimplemented("repos::list_base_refs"))
    }

    /// Replaces one repository's prepare and post-create hooks.
    pub async fn set_hooks(&self, _repo: RepoId, _hooks: RepoHooks) -> DaemonResult<Repo> {
        Err(DaemonError::Unimplemented("repos::set_hooks"))
    }

    /// Dismisses a retained failed clone record.
    pub async fn dismiss_clone(&self, _repo: RepoId) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("repos::dismiss_clone"))
    }

    /// Starts a non-destructive import from the default swarm home.
    pub async fn import_from_swarm(&self) -> DaemonResult<JobRecord> {
        Err(DaemonError::Unimplemented("repos::import_from_swarm"))
    }
}
