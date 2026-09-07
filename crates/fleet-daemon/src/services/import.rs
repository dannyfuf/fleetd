//! One-time import of compatible swarm configuration and state.

use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use fleet_core::{
    config::Config,
    config::merge_config_with_user_home,
    state::{State, default_state},
};
use fleet_proto::job::{JobKind, JobRecord};
use serde::{Deserialize, Serialize};

use crate::{
    DaemonError, DaemonResult,
    adapters::files::Files,
    jobs::JobManager,
    stores::{config::ConfigStore, state::StateStore},
};

const IMPORT_JOURNAL: &str = "import-transaction.json";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportJournal {
    original_config: Option<String>,
    imported_config: Config,
    imported_state: State,
    #[serde(default)]
    state_committed: bool,
}

/// Notification seam invoked after imported state has committed.
#[async_trait]
pub trait ImportNotifier: Send + Sync {
    /// Publishes the newly assembled authoritative snapshot.
    async fn snapshot_changed(&self) -> DaemonResult<()>;
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
    notifier: Option<Arc<dyn ImportNotifier>>,
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
            notifier: None,
        }
    }

    /// Adds the snapshot notification boundary used by the daemon facade.
    #[must_use]
    pub fn with_notifier(mut self, notifier: Arc<dyn ImportNotifier>) -> Self {
        self.notifier = Some(notifier);
        self
    }

    pub(crate) async fn recover(&self) -> DaemonResult<()> {
        if let Some(config) = recover_import(
            self.files.as_ref(),
            &self.config,
            &self.state,
            &self.fleet_home.join(IMPORT_JOURNAL),
        )
        .await?
        {
            apply_runtime_config(&self.config, &self.jobs, self.files.as_ref(), &config);
        }
        Ok(())
    }

    /// Starts a cancellable import job and returns its initial record.
    pub async fn start(&self) -> DaemonResult<JobRecord> {
        self.recover().await?;
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
        let runtime_jobs = Arc::clone(&self.jobs);
        let files = Arc::clone(&self.files);
        let notifier = self.notifier.clone();
        let id = self.jobs.submit(
            JobKind::Import,
            swarm_home.display().to_string(),
            "Import from swarm",
            true,
            false,
            move |context| async move {
                let journal_path = fleet_home.join(IMPORT_JOURNAL);
                if let Some(config) =
                    recover_import(files.as_ref(), &config_store, &state_store, &journal_path)
                        .await?
                {
                    apply_runtime_config(&config_store, &runtime_jobs, files.as_ref(), &config);
                }
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
                let mut journal = ImportJournal {
                    original_config: files
                        .exists(config_store.path())
                        .then(|| files.read_text(config_store.path()))
                        .transpose()?,
                    imported_config: imported_config.clone(),
                    imported_state: imported_state.clone(),
                    state_committed: false,
                };
                write_journal(files.as_ref(), &journal_path, &journal)?;
                config_store.save(imported_config.clone()).await?;
                let state_commit = state_store
                    .transaction(move |state| {
                        if state != &default_state() {
                            return Err(DaemonError::Conflict(
                                "Fleet state is no longer empty".to_owned(),
                            ));
                        }
                        *state = imported_state;
                        Ok(())
                    })
                    .await;
                if let Err(error) = state_commit {
                    rollback_import(files.as_ref(), config_store.path(), &journal)?;
                    files.remove_file(&journal_path)?;
                    return Err(error);
                }
                journal.state_committed = true;
                write_journal(files.as_ref(), &journal_path, &journal)?;
                apply_runtime_config(
                    &config_store,
                    &runtime_jobs,
                    files.as_ref(),
                    &imported_config,
                );
                files.remove_file(&journal_path)?;
                if let Some(notifier) = notifier {
                    notifier.snapshot_changed().await?;
                }
                context.progress("swarm import complete")?;
                Ok(())
            },
        );
        self.jobs
            .record(&id)
            .ok_or_else(|| DaemonError::NotFound(format!("job {id}")))
    }
}

fn write_journal(
    files: &dyn Files,
    path: &std::path::Path,
    journal: &ImportJournal,
) -> DaemonResult<()> {
    let mut text = serde_json::to_string_pretty(journal)?;
    text.push('\n');
    files.atomic_write_text(path, &text)
}

fn rollback_import(
    files: &dyn Files,
    config_path: &std::path::Path,
    journal: &ImportJournal,
) -> DaemonResult<()> {
    if let Some(original) = &journal.original_config {
        files.atomic_write_text(config_path, original)
    } else {
        files.remove_file(config_path)
    }
}

async fn recover_import(
    files: &dyn Files,
    config: &ConfigStore,
    state: &StateStore,
    journal_path: &std::path::Path,
) -> DaemonResult<Option<Config>> {
    if !files.exists(journal_path) {
        return Ok(None);
    }
    let journal: ImportJournal = serde_json::from_str(&files.read_text(journal_path)?)?;
    let effective = if journal.state_committed || state.load().await? == journal.imported_state {
        config.save(journal.imported_config.clone()).await?;
        journal.imported_config
    } else {
        rollback_import(files, config.path(), &journal)?;
        config.load().await?
    };
    files.remove_file(journal_path)?;
    Ok(Some(effective))
}

fn apply_runtime_config(
    store: &ConfigStore,
    jobs: &JobManager,
    files: &dyn Files,
    config: &Config,
) {
    jobs.set_retention(std::time::Duration::from_millis(
        config.jobs.keep_finished_for,
    ));
    files.set_removable_roots(vec![
        PathBuf::from(&config.repos_dir),
        PathBuf::from(&config.worktrees_dir),
    ]);
    super::maintenance::publish_runtime_config(store, config);
}
