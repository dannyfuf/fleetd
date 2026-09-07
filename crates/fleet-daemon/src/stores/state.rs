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
        let lock_path = self.lock_path.clone();
        let files = Arc::clone(&self.files);
        let epoch = self.clock.epoch_millis();
        tokio::task::spawn_blocking(move || {
            load_for_read_sync(&path, &lock_path, files.as_ref(), epoch)
        })
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

    /// Replaces a quarantined state with the validated default while retaining the broken file.
    pub async fn reset_quarantined(&self) -> DaemonResult<PathBuf> {
        let _guard = self.gate.lock().await;
        let path = self.path.clone();
        let lock_path = self.lock_path.clone();
        let files = Arc::clone(&self.files);
        tokio::task::spawn_blocking(move || {
            let _lock = StateLock::acquire(lock_path)?;
            let broken = broken_state_path(&path, files.as_ref())?.ok_or_else(|| {
                DaemonError::Conflict(format!("state is not quarantined: {}", path.display()))
            })?;
            if files.exists(&path) {
                return Err(DaemonError::Conflict(format!(
                    "state already exists: {}",
                    path.display()
                )));
            }
            save_sync(&path, files.as_ref(), &default_state())?;
            Ok(broken)
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
    match inspect_state(path, files)? {
        StateObservation::Missing => {
            if let Some(broken) = broken_state_path(path, files)? {
                return Err(DaemonError::Validation(format!(
                    "state is quarantined at {}; an explicit state reset is required",
                    broken.display()
                )));
            }
            Ok(default_state())
        }
        StateObservation::Valid(state) => Ok(state),
        StateObservation::Invalid(error) => {
            quarantine(path, files, epoch)?;
            Err(DaemonError::Validation(error))
        }
    }
}

fn load_for_read_sync(
    path: &std::path::Path,
    lock_path: &std::path::Path,
    files: &dyn Files,
    epoch: i64,
) -> DaemonResult<State> {
    load_for_read_sync_after_observation(path, lock_path, files, epoch, || {})
}

fn load_for_read_sync_after_observation<F>(
    path: &std::path::Path,
    lock_path: &std::path::Path,
    files: &dyn Files,
    epoch: i64,
    before_lock: F,
) -> DaemonResult<State>
where
    F: FnOnce(),
{
    if let StateObservation::Valid(state) = inspect_state(path, files)? {
        return Ok(state);
    }
    before_lock();
    let _lock = StateLock::acquire(lock_path)?;
    load_sync(path, files, epoch)
}

enum StateObservation {
    Missing,
    Valid(State),
    Invalid(String),
}

fn inspect_state(path: &std::path::Path, files: &dyn Files) -> DaemonResult<StateObservation> {
    if !files.exists(path) {
        return Ok(StateObservation::Missing);
    }
    let text = files.read_text(path)?;
    let state = match serde_json::from_str::<State>(&text) {
        Ok(state) => state,
        Err(error) => {
            return Ok(StateObservation::Invalid(format!(
                "invalid state JSON: {error}"
            )));
        }
    };
    match validate_state(&state) {
        Ok(()) => Ok(StateObservation::Valid(state)),
        Err(error) => Ok(StateObservation::Invalid(error.to_string())),
    }
}

fn broken_state_path(path: &std::path::Path, files: &dyn Files) -> DaemonResult<Option<PathBuf>> {
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    if !files.exists(parent) {
        return Ok(None);
    }
    let prefix = format!(
        "{}.broken-",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("state.json")
    );
    Ok(files.list(parent)?.into_iter().find(|candidate| {
        candidate
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(&prefix))
    }))
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
    async fn quarantined_state_never_falls_back() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let store = store(&temp);
        std::fs::create_dir_all(store.path().parent().unwrap_or(temp.path()))
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(store.path(), "not json").unwrap_or_else(|error| panic!("{error}"));

        assert!(store.load().await.is_err());
        let second = store.load().await;
        assert!(
            matches!(second, Err(DaemonError::Validation(message)) if message.contains("quarantined"))
        );
    }

    #[tokio::test]
    async fn explicit_reset_preserves_quarantine_and_writes_default_state() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let store = store(&temp);
        std::fs::create_dir_all(store.path().parent().unwrap_or(temp.path()))
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(store.path(), "not json").unwrap_or_else(|error| panic!("{error}"));
        assert!(store.load().await.is_err());

        let broken = store
            .reset_quarantined()
            .await
            .unwrap_or_else(|error| panic!("{error}"));

        assert!(broken.exists());
        assert_eq!(
            store.load().await.unwrap_or_else(|error| panic!("{error}")),
            default_state()
        );
    }

    #[test]
    fn load_does_not_quarantine_replacement() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let store = store(&temp);
        std::fs::create_dir_all(store.path().parent().unwrap_or(temp.path()))
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(store.path(), "not json").unwrap_or_else(|error| panic!("{error}"));
        let replacement =
            serde_json::to_string(&default_state()).unwrap_or_else(|error| panic!("{error}"));

        let loaded = load_for_read_sync_after_observation(
            &store.path,
            &store.lock_path,
            store.files.as_ref(),
            1_700_000_000_123,
            || {
                std::fs::write(store.path(), replacement).unwrap_or_else(|error| panic!("{error}"));
            },
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(loaded, default_state());
        assert!(store.path().exists());
        assert!(
            !store
                .path()
                .with_file_name("state.json.broken-1700000000123")
                .exists()
        );
    }

    fn context(name: &str) -> fleet_core::model::Context {
        fleet_core::model::Context {
            id: fleet_core::ids::ContextId::try_from(name).expect("valid context ID"),
            name: name.to_owned(),
            owners: Vec::new(),
            created_at: "2026-09-06T00:00:00Z".to_owned(),
        }
    }

    #[tokio::test]
    async fn transaction_persists_changes_and_returns_the_operation_result() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = store(&temp);
        let selected = store
            .transaction(|state| {
                let context = context("team");
                let id = context.id.clone();
                state.contexts.push(context);
                state.active_context_id = Some(id.clone());
                Ok(id)
            })
            .await
            .expect("commit context");
        let loaded = store.load().await.expect("reload state");
        assert_eq!(loaded.contexts, [context("team")]);
        assert_eq!(loaded.active_context_id.as_ref(), Some(&selected));
        assert!(!store.lock_path.exists());
    }

    #[tokio::test]
    async fn failed_operations_and_invalid_state_leave_persisted_data_unchanged() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = store(&temp);
        store
            .transaction(|state| {
                state.contexts.push(context("team"));
                Ok(())
            })
            .await
            .expect("initial state");
        let before = std::fs::read(store.path()).expect("read before");
        let failed: DaemonResult<()> = store
            .transaction(|state| {
                state.contexts.clear();
                Err(DaemonError::Conflict("operation failed".to_owned()))
            })
            .await;
        assert!(matches!(failed, Err(DaemonError::Conflict(_))));
        assert_eq!(
            std::fs::read(store.path()).expect("read after error"),
            before
        );
        let invalid = store
            .transaction(|state| {
                state.contexts.push(context("team"));
                Ok(())
            })
            .await;
        assert!(matches!(invalid, Err(DaemonError::Validation(_))));
        assert_eq!(
            std::fs::read(store.path()).expect("read after validation"),
            before
        );
        assert!(!store.lock_path.exists());
    }

    #[tokio::test]
    async fn independent_stores_preserve_both_concurrent_transactions() {
        let temp = tempfile::tempdir().expect("temp dir");
        let first = store(&temp);
        let second = store(&temp);
        let (one, two) = tokio::join!(
            first.transaction(|state| {
                state.contexts.push(context("one"));
                Ok(())
            }),
            second.transaction(|state| {
                state.contexts.push(context("two"));
                Ok(())
            }),
        );
        one.expect("first transaction");
        two.expect("second transaction");
        let mut names = first
            .load()
            .await
            .expect("reload")
            .contexts
            .into_iter()
            .map(|context| context.name)
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, ["one", "two"]);
        assert!(!first.lock_path.exists());
    }
}
