//! One-time import of compatible swarm configuration and state.

use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use fleet_core::{
    config::merge_config_with_user_home,
    state::{State, default_state},
};
use fleet_proto::job::{JobKind, JobRecord};

use crate::{
    DaemonError, DaemonResult,
    adapters::files::Files,
    jobs::JobManager,
    stores::{config::ConfigStore, state::StateStore},
};

/// Notification seam invoked after imported state has committed.
#[async_trait]
pub trait ImportNotifier: Send + Sync {
    /// Publishes the newly assembled authoritative snapshot.
    async fn snapshot_changed(&self) -> DaemonResult<()>;
}

#[derive(Debug)]
struct NoopNotifier;

#[async_trait]
impl ImportNotifier for NoopNotifier {
    async fn snapshot_changed(&self) -> DaemonResult<()> {
        Ok(())
    }
}

/// Imports a version-one swarm home into an empty Fleet home.
#[derive(Clone)]
pub struct Import {
    fleet_home: PathBuf,
    swarm_home: PathBuf,
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    jobs: Arc<JobManager>,
    files: Arc<dyn Files>,
    notifier: Arc<dyn ImportNotifier>,
}

impl Import {
    /// Creates an importer for explicit source and destination homes.
    #[must_use]
    pub fn new(
        fleet_home: impl Into<PathBuf>,
        swarm_home: impl Into<PathBuf>,
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        files: Arc<dyn Files>,
    ) -> Self {
        Self {
            fleet_home: fleet_home.into(),
            swarm_home: swarm_home.into(),
            config,
            state,
            jobs,
            files,
            notifier: Arc::new(NoopNotifier),
        }
    }

    /// Adds the snapshot notification boundary used by the daemon facade.
    #[must_use]
    pub fn with_notifier(mut self, notifier: Arc<dyn ImportNotifier>) -> Self {
        self.notifier = notifier;
        self
    }

    /// Starts a cancellable import job and returns its initial record.
    pub async fn start(&self) -> DaemonResult<JobRecord> {
        let destination_state = self.fleet_home.join("state.json");
        if self.files.exists(&destination_state) {
            return Err(DaemonError::Conflict(format!(
                "Fleet state already exists at {}",
                destination_state.display()
            )));
        }
        let fleet_home = self.fleet_home.clone();
        let swarm_home = self.swarm_home.clone();
        let config_store = Arc::clone(&self.config);
        let state_store = Arc::clone(&self.state);
        let files = Arc::clone(&self.files);
        let notifier = Arc::clone(&self.notifier);
        let id = self.jobs.submit(
            JobKind::Import,
            swarm_home.display().to_string(),
            "Import from swarm",
            true,
            false,
            move |context| async move {
                // Keep the in-job check as a race guard: another client may create state after
                // the synchronous preflight above but before this queued operation starts.
                let destination_state = fleet_home.join("state.json");
                if files.exists(&destination_state) {
                    return Err(DaemonError::Conflict(format!(
                        "Fleet state already exists at {}",
                        destination_state.display()
                    )));
                }
                if context.cancel.is_cancelled() {
                    return Err(DaemonError::Cancelled);
                }

                context.progress("reading swarm configuration and state")?;
                let source_config = swarm_home.join("config.json");
                let source_state = swarm_home.join("state.json");
                let config_patch: serde_json::Value =
                    serde_json::from_str(&files.read_text(&source_config)?)?;
                let imported_state: State = serde_json::from_str(&files.read_text(&source_state)?)?;
                imported_state
                    .validate()
                    .map_err(|error| DaemonError::Validation(error.to_string()))?;
                let user_home = swarm_home.parent().unwrap_or(&swarm_home);
                let mut imported_config =
                    merge_config_with_user_home(&swarm_home, user_home, config_patch)
                        .map_err(|error| DaemonError::Validation(error.to_string()))?;
                // Import-only: a swarm `lazygit` window means "the git UI lives in this tab",
                // and Fleet has its own. Writing `lazygit` into Fleet's own config.json is
                // left alone, which is the opt-out back to the binary in a PTY.
                fleet_core::config::normalize_imported_windows(&mut imported_config.windows);
                if context.cancel.is_cancelled() {
                    return Err(DaemonError::Cancelled);
                }

                context.progress(format!(
                    "validated {} contexts, {} repositories, and {} worktrees",
                    imported_state.contexts.len(),
                    imported_state.repos.len(),
                    imported_state.worktrees.len()
                ))?;
                config_store.save(imported_config).await?;
                state_store
                    .transaction(move |state| {
                        if state != &default_state() {
                            return Err(DaemonError::Conflict(
                                "Fleet state is no longer empty".to_owned(),
                            ));
                        }
                        *state = imported_state;
                        Ok(())
                    })
                    .await?;
                notifier.snapshot_changed().await?;
                context.progress("swarm import complete")?;
                Ok(())
            },
        );
        self.jobs
            .list()
            .into_iter()
            .find(|record| record.id == id)
            .ok_or_else(|| DaemonError::NotFound(format!("job {id}")))
    }
}
