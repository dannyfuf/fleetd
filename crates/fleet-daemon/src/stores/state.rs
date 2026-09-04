//! Validated state transactions, persistence, and broken-state quarantine.

use std::{path::PathBuf, sync::Arc};

use fleet_core::{
    paths::FleetHome,
    state::{State, default_state, validate_state},
};

use crate::{
    DaemonError, DaemonResult,
    adapters::{clock::Clock, files::Files},
    stores::lock::StateLock,
};

/// Validated durable state store with cross-process transactions.
#[derive(Clone)]
pub struct StateStore {
    path: PathBuf,
    lock_path: PathBuf,
    files: Arc<dyn Files>,
    clock: Arc<dyn Clock>,
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl StateStore {
    /// Creates a state store rooted at a Fleet home.
    #[must_use]
    pub fn new(home: impl Into<PathBuf>, files: Arc<dyn Files>, clock: Arc<dyn Clock>) -> Self {
        let layout = FleetHome::new(home.into());
        Self {
            path: layout.state_path(),
            lock_path: layout.lock_path(),
            files,
            clock,
            gate: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Loads and validates state, quarantining malformed state before returning an error.
    pub async fn load(&self) -> DaemonResult<State> {
        let _guard = self.gate.lock().await;
        let path = self.path.clone();
        let files = Arc::clone(&self.files);
        let epoch = self.clock.epoch_millis();
        tokio::task::spawn_blocking(move || load_sync(&path, files.as_ref(), epoch))
            .await
            .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    /// Validates and atomically saves complete state under the cross-process lock.
    pub async fn save(&self, state: State) -> DaemonResult<()> {
        let _guard = self.gate.lock().await;
        let path = self.path.clone();
        let lock_path = self.lock_path.clone();
        let files = Arc::clone(&self.files);
        tokio::task::spawn_blocking(move || {
            let _lock = StateLock::acquire(lock_path)?;
            save_sync(&path, files.as_ref(), &state)
        })
        .await
        .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    /// Runs one load-modify-validate-save transaction while holding `state.json.lock`.
    pub async fn transaction<F, R>(&self, operation: F) -> DaemonResult<R>
    where
        F: FnOnce(&mut State) -> DaemonResult<R> + Send + 'static,
        R: Send + 'static,
    {
        let _guard = self.gate.lock().await;
        let path = self.path.clone();
        let lock_path = self.lock_path.clone();
        let files = Arc::clone(&self.files);
        let epoch = self.clock.epoch_millis();
        tokio::task::spawn_blocking(move || {
            let _lock = StateLock::acquire(lock_path)?;
            let mut state = load_sync(&path, files.as_ref(), epoch)?;
            let result = operation(&mut state)?;
            save_sync(&path, files.as_ref(), &state)?;
            Ok(result)
        })
        .await
        .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    /// Returns the backing `state.json` path.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

fn load_sync(path: &std::path::Path, files: &dyn Files, epoch: i64) -> DaemonResult<State> {
    if !files.exists(path) {
        return Ok(default_state());
    }
    let text = files.read_text(path)?;
    let parsed = serde_json::from_str::<State>(&text);
    let state = match parsed {
        Ok(state) => state,
        Err(error) => {
            quarantine(path, files, epoch)?;
            return Err(DaemonError::Validation(format!(
                "invalid state JSON: {error}"
            )));
        }
    };
    if let Err(error) = validate_state(&state) {
        quarantine(path, files, epoch)?;
        return Err(DaemonError::Validation(error.to_string()));
    }
    Ok(state)
}

fn save_sync(path: &std::path::Path, files: &dyn Files, state: &State) -> DaemonResult<()> {
    validate_state(state).map_err(|error| DaemonError::Validation(error.to_string()))?;
    let mut text = serde_json::to_string_pretty(state)?;
    text.push('\n');
    files.atomic_write_text(path, &text)
}

fn quarantine(path: &std::path::Path, files: &dyn Files, epoch: i64) -> DaemonResult<()> {
    let broken = path.with_file_name(format!("state.json.broken-{epoch}"));
    files.rename(path, &broken)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use crate::{adapters::files::RealFiles, testing::fakes::FixedClock};

    use super::*;

    fn store(temp: &tempfile::TempDir) -> StateStore {
        let home = temp.path().join(".fleet");
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ));
        let now = Utc
            .timestamp_millis_opt(1_700_000_000_123)
            .single()
            .unwrap_or_else(|| panic!("valid timestamp"));
        StateStore::new(home, files, Arc::new(FixedClock::new(now)))
    }

    #[tokio::test]
    async fn broken_state_is_quarantined() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let store = store(&temp);
        std::fs::create_dir_all(store.path().parent().unwrap_or(temp.path()))
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(store.path(), "not json").unwrap_or_else(|error| panic!("{error}"));
        assert!(store.load().await.is_err());
        assert!(!store.path().exists());
        assert!(
            store
                .path()
                .with_file_name("state.json.broken-1700000000123")
                .exists()
        );
    }

    #[tokio::test]
    async fn transaction_persists_valid_changes_and_releases_lock() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let store = store(&temp);
        store
            .transaction(|state| {
                state.version = 1;
                Ok(())
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(store.path().exists());
        assert!(!store.path.with_file_name("state.json.lock").exists());
    }
}
