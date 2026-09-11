//! Fleet application services orchestrating domain rules, stores, adapters, jobs, and runtime snapshots.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use fleet_core::{
    config::{Config, default_config},
    ids::{HostId, RepoId},
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
    jobs::{JobCtx, JobManager},
    machines::Machines,
    server::BroadcastBus,
    stores::{config::ConfigStore, state::StateStore},
};

mod agent_activity;
pub mod agents;
pub mod boards;
pub mod bootstrap;
pub mod checkpoints;
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
    /// Fleet-owned turn checkpoints and the revert that restores one.
    pub checkpoints: checkpoints::Checkpoints,
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

/// Loads the identity persisted for this Fleet home, minting one when there is none to load.
///
/// The value is the host identity remote peers key on, so neither failure here is silent: a file
/// that is not a valid host id is moved aside to `daemon-id.invalid` and reported rather than
/// aborting the daemon's startup, and a write that cannot persist a fresh identity says so.
fn load_or_create_daemon_id(home: &Path) -> HostId {
    let path = home.join("daemon-id");
    let rejected = match std::fs::read_to_string(&path) {
        Ok(value) => {
            let value = value.trim().to_owned();
            match HostId::try_from(value.as_str()) {
                Ok(id) => return id,
                // An absent identity and an empty one are the same situation: mint a fresh one.
                Err(_) if value.is_empty() => None,
                Err(error) => Some((value, error.to_string())),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "could not read the persisted daemon identity; minting a new one"
            );
            None
        }
    };
    let id = HostId::try_from(uuid::Uuid::new_v4().to_string().as_str())
        .expect("a generated uuid is a valid host id");
    if let Some((value, reason)) = rejected {
        quarantine_daemon_id(&path, &value, &reason, &id);
    }
    match std::fs::create_dir_all(home) {
        Ok(()) => {
            if let Err(error) = std::fs::write(&path, format!("{id}\n")) {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "could not persist the daemon identity; it will change on the next restart"
                );
            }
        }
        Err(error) => tracing::warn!(
            path = %home.display(),
            %error,
            "could not create the Fleet home; the daemon identity will change on the next restart"
        ),
    }
    id
}

/// Moves a rejected identity file aside so the replacement is traceable to the value it replaced.
fn quarantine_daemon_id(path: &Path, rejected: &str, reason: &str, minted: &HostId) {
    let aside = path.with_extension("invalid");
    if let Err(error) = std::fs::rename(path, &aside) {
        tracing::warn!(
            path = %path.display(),
            %error,
            "could not move the rejected daemon identity aside"
        );
    }
    tracing::warn!(
        path = %path.display(),
        rejected,
        aside = %aside.display(),
        reason,
        %minted,
        "the persisted daemon identity is not a valid host id; minting a new one"
    );
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
