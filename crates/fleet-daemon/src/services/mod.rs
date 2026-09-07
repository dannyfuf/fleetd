//! Fleet application services orchestrating domain rules, stores, adapters, jobs, and runtime snapshots.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use fleet_core::{config::Config, ids::RepoId, paths::slot_path, sessions::WorktreeStatus};
use fleet_proto::{
    event::Event,
    request::RequestBody,
    response::ResponseBody,
    snapshot::{DaemonInfo, PoolStatus, Snapshot},
};
use tokio::{sync::RwLock, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    DaemonError, DaemonResult,
    adapters::{Adapters, files::Files},
    error::remote_unsupported,
    jobs::{JobCtx, JobManager},
    server::BroadcastBus,
    stores::{config::ConfigStore, state::StateStore},
};

mod agent_activity;
pub mod contexts;
pub mod doctor;
pub mod github;
pub mod hosts;
pub mod import;
pub mod inspect;
pub mod pool;
pub mod prune;
pub mod repos;
pub mod sessions;
pub mod sleep;
pub mod update;
mod watch_discovery;
pub mod watches;
pub mod worktrees;

use contexts::Contexts;
use doctor::Doctor;
use github::Github;
use hosts::Hosts;
use import::{Import, ImportNotifier};
use inspect::Inspect;
use pool::Pool;
use prune::Prune;
use repos::Repos;
use sessions::Sessions;
use sleep::Sleep;
use snapshots::merge_observed_statuses;
use update::Update;
use worktrees::Worktrees;

mod awaited;
mod cache;
mod composition;
mod dispatch;
mod hooks;
mod maintenance;
mod snapshots;
pub use maintenance::PeriodicTasks;

/// Fully wired facade used by socket connection actors.
#[derive(Clone)]
pub struct Services {
    pub config: Arc<ConfigStore>,
    pub state: Arc<StateStore>,
    pub jobs: Arc<JobManager>,
    /// External-system dependency bundle shared by services.
    pub adapters: Adapters,
    pub contexts: Contexts,
    pub repos: Repos,
    pub worktrees: Worktrees,
    pub pool: Pool,
    pub github: Github,
    pub hosts: Hosts,
    pub sessions: Sessions,
    /// Cooperative and discovered child output and lifecycle registry.
    pub watches: watches::Watches,
    watch_discovery: watch_discovery::WatchDiscovery,
    pub sleep: Sleep,
    pub inspect: Inspect,
    pub prune: Prune,
    pub doctor: Doctor,
    pub import: Import,
    pub update: Update,
    home: PathBuf,
    started_at: String,
    statuses: Arc<RwLock<Option<Vec<WorktreeStatus>>>>,
    inventory: Arc<tokio::sync::Mutex<snapshots::InventoryCache>>,
    pool_refreshed_at: Arc<RwLock<BTreeMap<RepoId, String>>>,
    /// Daemon-wide event bus shared by every service integration.
    pub events: BroadcastBus,
}

struct BusImportNotifier {
    events: BroadcastBus,
}

#[async_trait::async_trait]
impl ImportNotifier for BusImportNotifier {
    async fn snapshot_changed(&self) -> DaemonResult<()> {
        self.events.request_snapshot_current();
        Ok(())
    }
}

impl Services {
    /// Returns the daemon build version string.
    #[must_use]
    pub fn version() -> String {
        format!("fleetd {}", env!("CARGO_PKG_VERSION"))
    }
}

/// Directory holding every prepared copy and worktree of one repository.
fn repo_worktrees_dir(config: &Config, repo: &RepoId) -> PathBuf {
    Path::new(&config.worktrees_dir)
        .join(repo.owner())
        .join(repo.name())
}

/// Refspec that mirrors one upstream branch into `refs/remotes/origin`.
fn branch_refspec(branch: &str) -> String {
    format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")
}

/// Aborts a long service loop as soon as its job is cancelled.
fn check_cancelled(context: &JobCtx) -> DaemonResult<()> {
    if context.cancel.is_cancelled() {
        Err(DaemonError::Cancelled)
    } else {
        Ok(())
    }
}

/// Renames a tree aside under a unique name before deleting it, so a caller never
/// waits on a large removal and a crash never leaves a half-deleted worktree in place.
fn discard_path(files: &dyn Files, path: &Path) -> DaemonResult<()> {
    if !files.exists(path) {
        return Ok(());
    }
    let parent = path.parent().ok_or_else(|| {
        DaemonError::Validation(format!("path has no parent: {}", path.display()))
    })?;
    let discarded = parent.join(format!(".discard-{}", uuid::Uuid::new_v4()));
    files.rename(path, &discarded)?;
    files.remove_detached(&discarded)
}
