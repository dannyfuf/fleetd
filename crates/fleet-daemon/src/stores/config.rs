//! Configuration loading, default merging, and atomic persistence.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use fleet_core::{
    config::{Config, merge_config, validate_config},
    paths::FleetHome,
    sleep::CompiledSleepPolicy,
};
use serde_json::Value;

use crate::{
    DaemonError, DaemonResult,
    adapters::files::{FileRevision, Files},
};

/// A parse of `config.json` and the file revision it was read from.
#[derive(Clone)]
struct CachedConfig {
    revision: FileRevision,
    config: Config,
}

/// Durable effective-configuration store.
#[derive(Clone)]
pub struct ConfigStore {
    home: PathBuf,
    path: PathBuf,
    files: Arc<dyn Files>,
    gate: Arc<tokio::sync::Mutex<()>>,
    cache: Arc<Mutex<Option<CachedConfig>>>,
    sleep_policy: Arc<Mutex<Arc<CompiledSleepPolicy>>>,
}

impl ConfigStore {
    /// Creates a configuration store rooted at a Fleet home.
    #[must_use]
    pub fn new(home: impl Into<PathBuf>, files: Arc<dyn Files>) -> Self {
        let home = home.into();
        let path = FleetHome::new(home.clone()).config_path();
        Self {
            home,
            path,
            files,
            gate: Arc::new(tokio::sync::Mutex::new(())),
            cache: Arc::new(Mutex::new(None)),
            sleep_policy: Arc::new(Mutex::new(Arc::new(CompiledSleepPolicy::new(&[])))),
        }
    }

    /// Loads the effective configuration, deep-merging defaults and creating a missing file.
    ///
    /// Every dispatched request loads the configuration, so an unchanged file answers from a
    /// cached parse validated by one `stat`, without taking the write gate.
    pub async fn load(&self) -> DaemonResult<Config> {
        if let Some(cached) = self.cached().await {
            return Ok(cached);
        }
        let guard = Arc::clone(&self.gate).lock_owned().await;
        let home = self.home.clone();
        let path = self.path.clone();
        let files = Arc::clone(&self.files);
        let cache = Arc::clone(&self.cache);
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let config = load_sync(&home, &path, files.as_ref())?;
            // The gate serializes this against every write, so the stored pair cannot mix a
            // configuration with a revision written after it.
            if let Ok(revision) = files.revision(&path) {
                *lock(&cache) = Some(CachedConfig {
                    revision,
                    config: config.clone(),
                });
            }
            Ok(config)
        })
        .await
        .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    /// Returns the cached configuration when the file has not changed since it was parsed.
    async fn cached(&self) -> Option<Config> {
        let cached = lock(&self.cache).clone()?;
        let files = Arc::clone(&self.files);
        let path = self.path.clone();
        let revision = tokio::task::spawn_blocking(move || files.revision(&path))
            .await
            .ok()?
            .ok()?;
        (revision == cached.revision).then_some(cached.config)
    }

    /// Loads rules and retains their compiled matcher across sleep requests and polls.
    /// Each caller keeps an immutable snapshot while process observation is in flight.
    pub(crate) async fn load_with_sleep_policy(
        &self,
    ) -> DaemonResult<(Config, Arc<CompiledSleepPolicy>)> {
        let config = self.load().await?;
        let mut policy = self
            .sleep_policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::make_mut(&mut policy).update(&config.sleep.keep_alive);
        Ok((config, Arc::clone(&policy)))
    }

    /// Validates and atomically saves a complete configuration.
    pub async fn save(&self, config: Config) -> DaemonResult<()> {
        let guard = Arc::clone(&self.gate).lock_owned().await;
        let path = self.path.clone();
        let files = Arc::clone(&self.files);
        let cache = Arc::clone(&self.cache);
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            *lock(&cache) = None;
            save_sync(&path, files.as_ref(), &config)
        })
        .await
        .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    /// Deep-merges and persists a partial JSON configuration patch.
    pub async fn update(&self, patch: Value) -> DaemonResult<Config> {
        let guard = Arc::clone(&self.gate).lock_owned().await;
        let home = self.home.clone();
        let path = self.path.clone();
        let files = Arc::clone(&self.files);
        let cache = Arc::clone(&self.cache);
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let current = if files.exists(&path) {
                serde_json::from_str(&files.read_text(&path)?)?
            } else {
                Value::Object(serde_json::Map::new())
            };
            let mut merged_patch = current;
            fleet_core::config::deep_merge_json(&mut merged_patch, patch);
            let config = merge_config(&home, merged_patch)
                .map_err(|error| DaemonError::Validation(error.to_string()))?;
            *lock(&cache) = None;
            save_sync(&path, files.as_ref(), &config)?;
            Ok(config)
        })
        .await
        .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    /// Returns the backing `config.json` path.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Returns the Fleet home this store is rooted at.
    #[must_use]
    pub fn home(&self) -> &std::path::Path {
        &self.home
    }
}

/// Locks a store mutex, recovering the guard after a previous holder panicked.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn load_sync(
    home: &std::path::Path,
    path: &std::path::Path,
    files: &dyn Files,
) -> DaemonResult<Config> {
    let missing = !files.exists(path);
    let patch = if missing {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(&files.read_text(path)?)?
    };
    let config =
        merge_config(home, patch).map_err(|error| DaemonError::Validation(error.to_string()))?;
    if missing {
        save_sync(path, files, &config)?;
    }
    Ok(config)
}

fn save_sync(path: &std::path::Path, files: &dyn Files, config: &Config) -> DaemonResult<()> {
    validate_config(config).map_err(|error| DaemonError::Validation(error.to_string()))?;
    let mut text = serde_json::to_string_pretty(config)?;
    text.push('\n');
    files.atomic_write_text(path, &text)
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::{Arc, Condvar},
    };

    use crate::adapters::files::RealFiles;

    use super::*;

    fn store(temp: &tempfile::TempDir) -> ConfigStore {
        let home = temp.path().join(".fleet");
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ));
        ConfigStore::new(home, files)
    }

    struct CountingFiles {
        inner: RealFiles,
        reads: std::sync::atomic::AtomicUsize,
    }

    impl CountingFiles {
        fn reads(&self) -> usize {
            self.reads.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Files for CountingFiles {
        fn read_text(&self, path: &Path) -> DaemonResult<String> {
            if path.file_name().is_some_and(|name| name == "config.json") {
                self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            self.inner.read_text(path)
        }

        fn create_dir_all(&self, path: &Path) -> DaemonResult<()> {
            self.inner.create_dir_all(path)
        }

        fn clone_dir(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
            self.inner.clone_dir(source, destination)
        }

        fn atomic_write_text(&self, path: &Path, text: &str) -> DaemonResult<()> {
            self.inner.atomic_write_text(path, text)
        }

        fn rename(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
            self.inner.rename(source, destination)
        }

        fn trash(&self, path: &Path) -> DaemonResult<PathBuf> {
            self.inner.trash(path)
        }

        fn remove_detached(&self, path: &Path) -> DaemonResult<()> {
            self.inner.remove_detached(path)
        }

        fn remove_file(&self, path: &Path) -> DaemonResult<()> {
            self.inner.remove_file(path)
        }

        fn revision(&self, path: &Path) -> DaemonResult<FileRevision> {
            self.inner.revision(path)
        }

        fn metadata(&self, path: &Path) -> DaemonResult<crate::adapters::files::FileMetadata> {
            self.inner.metadata(path)
        }

        fn exists(&self, path: &Path) -> bool {
            self.inner.exists(path)
        }

        fn list(&self, path: &Path) -> DaemonResult<Vec<PathBuf>> {
            self.inner.list(path)
        }

        fn guard_strict_descendant(&self, path: &Path) -> DaemonResult<()> {
            self.inner.guard_strict_descendant(path)
        }

        fn set_removable_roots(&self, roots: Vec<PathBuf>) {
            self.inner.set_removable_roots(roots);
        }
    }

    #[tokio::test]
    async fn repeated_loads_reuse_the_parsed_config_until_the_file_changes() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = temp.path().join(".fleet");
        let files = Arc::new(CountingFiles {
            inner: RealFiles::new(
                home.join("trash"),
                [home.join("repos"), home.join("worktrees")],
            ),
            reads: std::sync::atomic::AtomicUsize::new(0),
        });
        let store = ConfigStore::new(&home, files.clone());
        store.load().await.unwrap_or_else(|error| panic!("{error}"));
        let after_first = files.reads();

        for _ in 0..50 {
            store.load().await.unwrap_or_else(|error| panic!("{error}"));
        }
        assert_eq!(
            files.reads(),
            after_first,
            "every dispatched request re-read config.json"
        );

        // A hand edit that truncates the file in place must still be observed.
        std::fs::write(store.path(), r#"{"hotPoolSize":4}"#)
            .unwrap_or_else(|error| panic!("{error}"));
        let config = store.load().await.unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.hot_pool_size, 4);
    }

    #[tokio::test]
    async fn missing_config_writes_defaults() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let store = store(&temp);
        let config = store.load().await.unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.hot_pool_size, 1);
        assert!(store.path().exists());
    }

    #[tokio::test]
    async fn partial_config_deep_merges_nested_defaults() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let store = store(&temp);
        std::fs::create_dir_all(store.path().parent().unwrap_or(temp.path()))
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(store.path(), r#"{"github":{"prTtlSeconds":12}}"#)
            .unwrap_or_else(|error| panic!("{error}"));
        let config = store.load().await.unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(config.github.pr_ttl_seconds, 12);
        assert_eq!(config.github.cache_ttl_seconds, 3_600);
    }

    struct BlockingWriteFiles {
        inner: RealFiles,
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: (Mutex<bool>, Condvar),
    }

    impl BlockingWriteFiles {
        fn new(inner: RealFiles) -> Self {
            Self {
                inner,
                started: Mutex::new(None),
                release: (Mutex::new(false), Condvar::new()),
            }
        }

        fn block_next_write(&self) -> tokio::sync::oneshot::Receiver<()> {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            *self
                .started
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sender);
            receiver
        }

        fn release_write(&self) {
            *self
                .release
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            self.release.1.notify_all();
        }
    }

    impl Files for BlockingWriteFiles {
        fn read_text(&self, path: &Path) -> DaemonResult<String> {
            self.inner.read_text(path)
        }

        fn create_dir_all(&self, path: &Path) -> DaemonResult<()> {
            self.inner.create_dir_all(path)
        }

        fn clone_dir(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
            self.inner.clone_dir(source, destination)
        }

        fn atomic_write_text(&self, path: &Path, text: &str) -> DaemonResult<()> {
            let sender = self
                .started
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(sender) = sender {
                let _ignored = sender.send(());
                let (released, ready) = &self.release;
                let released = released
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                drop(
                    ready
                        .wait_while(released, |released| !*released)
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
            }
            self.inner.atomic_write_text(path, text)
        }

        fn rename(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
            self.inner.rename(source, destination)
        }

        fn trash(&self, path: &Path) -> DaemonResult<PathBuf> {
            self.inner.trash(path)
        }

        fn remove_detached(&self, path: &Path) -> DaemonResult<()> {
            self.inner.remove_detached(path)
        }

        fn remove_file(&self, path: &Path) -> DaemonResult<()> {
            self.inner.remove_file(path)
        }

        fn exists(&self, path: &Path) -> bool {
            self.inner.exists(path)
        }

        fn list(&self, path: &Path) -> DaemonResult<Vec<PathBuf>> {
            self.inner.list(path)
        }

        fn guard_strict_descendant(&self, path: &Path) -> DaemonResult<()> {
            self.inner.guard_strict_descendant(path)
        }

        fn set_removable_roots(&self, roots: Vec<PathBuf>) {
            self.inner.set_removable_roots(roots);
        }
    }

    #[tokio::test]
    async fn cancelled_write_retains_serialization() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let home = temp.path().join(".fleet");
        let real = RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        );
        let files = Arc::new(BlockingWriteFiles::new(real));
        let store = ConfigStore::new(&home, files.clone());
        let config = store.load().await.unwrap_or_else(|error| panic!("{error}"));
        let started = files.block_next_write();
        let writer = store.clone();
        let write = tokio::spawn(async move { writer.save(config).await });
        started
            .await
            .unwrap_or_else(|error| panic!("write did not start: {error}"));
        write.abort();
        let _cancelled = write.await;

        assert!(store.gate.try_lock().is_err());
        files.release_write();
        store.load().await.unwrap_or_else(|error| panic!("{error}"));
        assert!(store.gate.try_lock().is_ok());
    }

    #[tokio::test]
    async fn sleep_policy_reuses_rules_and_preserves_in_flight_snapshots() {
        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let store = store(&temp);
        let (_, initial) = store.load_with_sleep_policy().await.unwrap();
        let initial_address = Arc::as_ptr(&initial);
        drop(initial);
        let (_, initial) = store.clone().load_with_sleep_policy().await.unwrap();
        assert_eq!(initial_address, Arc::as_ptr(&initial));

        store
            .update(serde_json::json!({"sleep": {"keepAlive": []}}))
            .await
            .unwrap();
        let (_, updated) = store.load_with_sleep_policy().await.unwrap();
        let commands = vec!["claude".to_owned()];
        assert!(updated.match_keep_alive(&commands, &[]).is_empty());
        assert_eq!(initial.match_keep_alive(&commands, &[]), ["claude"]);

        // Reloads also pick up edits made outside ConfigStore.
        std::fs::write(store.path(), r#"{"sleep":{"keepAlive":[{"id":"broken","label":"broken","kind":"process","pattern":"\\q"}]}}"#).unwrap();
        let (_, invalid) = store.load_with_sleep_policy().await.unwrap();
        assert_eq!(invalid.diagnostics().len(), 1);
        assert!(updated.diagnostics().is_empty());
    }
}
