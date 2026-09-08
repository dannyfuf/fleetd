//! Fleet application services orchestrating domain rules, stores, adapters, jobs, and runtime snapshots.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use fleet_core::{
    config::{Config, default_config},
    ids::RepoId,
    paths::slot_path,
};
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
    machines::Machines,
    server::BroadcastBus,
    stores::{config::ConfigStore, state::StateStore},
};

mod agent_activity;
pub mod agents;
pub mod boards;
pub mod bootstrap;
pub mod contexts;
pub mod doctor;
pub mod github;
pub mod hosts;
pub mod import;
pub mod inspect;
pub mod mirror;
pub mod pool;
pub mod prune;
pub mod repos;
pub mod router;
pub mod sessions;
pub mod sleep;
pub mod update;
mod watch_discovery;
pub mod watches;
pub mod worktrees;

use agents::AgentService;
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
use update::Update;
use worktrees::Worktrees;

mod awaited;
mod cache;
mod composition;
mod dispatch;
pub use dispatch::RequestContext;
mod hooks;
mod maintenance;
mod snapshots;
pub use maintenance::PeriodicTasks;

/// Fully wired facade used by socket connection actors.
#[derive(Clone)]
pub struct Services {
    /// Backend-independent board service.
    pub boards: Arc<boards::Boards>,
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
    /// Configured machine providers and their lazy remote endpoints.
    pub machines: Arc<Machines>,
    /// Cached authoritative remote snapshot fragments.
    pub mirror: Arc<mirror::Mirror>,
    /// Request target classifier and forwarding facade.
    pub router: Arc<router::Router>,
    /// Remote daemon bootstrap job service.
    pub bootstrap: Arc<bootstrap::Bootstrap>,
    pub sessions: Sessions,
    /// Native structured coding-agent threads.
    pub agents: AgentService,
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
    daemon_id: String,
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

    /// Returns the stable identity persisted for this Fleet home.
    #[must_use]
    pub fn daemon_id(&self) -> &str {
        &self.daemon_id
    }
}

fn load_or_create_daemon_id(home: &Path) -> String {
    let path = home.join("daemon-id");
    if let Ok(value) = std::fs::read_to_string(&path) {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_owned();
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    if std::fs::create_dir_all(home).is_ok() {
        let _ = std::fs::write(path, format!("{id}\n"));
    }
    id
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
