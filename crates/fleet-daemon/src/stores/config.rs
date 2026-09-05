//! Configuration loading, default merging, and atomic persistence.

use std::{path::PathBuf, sync::Arc};

use fleet_core::{
    config::{Config, merge_config, validate_config},
    paths::FleetHome,
};
use serde_json::Value;

use crate::{DaemonError, DaemonResult, adapters::files::Files};

/// Durable effective-configuration store.
#[derive(Clone)]
pub struct ConfigStore {
    home: PathBuf,
    path: PathBuf,
    files: Arc<dyn Files>,
    gate: Arc<tokio::sync::Mutex<()>>,
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
        }
    }

    /// Loads the effective configuration, deep-merging defaults and creating a missing file.
    pub async fn load(&self) -> DaemonResult<Config> {
        let _guard = self.gate.lock().await;
        let home = self.home.clone();
        let path = self.path.clone();
        let files = Arc::clone(&self.files);
        tokio::task::spawn_blocking(move || load_sync(&home, &path, files.as_ref()))
            .await
            .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    /// Validates and atomically saves a complete configuration.
    pub async fn save(&self, config: Config) -> DaemonResult<()> {
        let _guard = self.gate.lock().await;
        let path = self.path.clone();
        let files = Arc::clone(&self.files);
        tokio::task::spawn_blocking(move || save_sync(&path, files.as_ref(), &config))
            .await
            .map_err(|error| DaemonError::Join(error.to_string()))?
    }

    /// Deep-merges and persists a partial JSON configuration patch.
    pub async fn update(&self, patch: Value) -> DaemonResult<Config> {
        let _guard = self.gate.lock().await;
        let home = self.home.clone();
        let path = self.path.clone();
        let files = Arc::clone(&self.files);
        tokio::task::spawn_blocking(move || {
            let current = if files.exists(&path) {
                serde_json::from_str(&files.read_text(&path)?)?
            } else {
                Value::Object(serde_json::Map::new())
            };
            let mut merged_patch = current;
            fleet_core::config::deep_merge_json(&mut merged_patch, patch);
            let config = merge_config(&home, merged_patch)
                .map_err(|error| DaemonError::Validation(error.to_string()))?;
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
    use std::sync::Arc;

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
}
