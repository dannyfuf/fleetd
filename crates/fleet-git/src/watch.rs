//! Debounced repository filesystem watcher.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{ChangeEvent, GitError, RepoPaths, Result};

const DEBOUNCE: Duration = Duration::from_millis(150);

/// Keeps native filesystem watches and the debounce task alive.
#[derive(Debug)]
pub struct RepoWatcher {
    _watcher: RecommendedWatcher,
    task: tokio::task::JoinHandle<()>,
}

impl RepoWatcher {
    /// Watches worktree content and relevant per-worktree/shared Git metadata.
    pub fn new(paths: &RepoPaths) -> Result<(Self, async_channel::Receiver<ChangeEvent>)> {
        let (raw_tx, raw_rx) = async_channel::unbounded::<Vec<PathBuf>>();
        let (event_tx, event_rx) = async_channel::bounded(32);
        let worktree = paths.worktree_root.clone();
        let git_dir = paths.git_dir.clone();
        let common_dir = paths.common_dir.clone();
        let callback_tx = raw_tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    let paths: Vec<PathBuf> = event
                        .paths
                        .into_iter()
                        .filter(|path| relevant(path, &worktree, &git_dir, &common_dir))
                        .collect();
                    if !paths.is_empty() {
                        let _ = callback_tx.send_blocking(paths);
                    }
                }
            },
            Config::default(),
        )
        .map_err(|error| GitError::parse("repository watcher", error.to_string()))?;
        watcher
            .watch(&paths.worktree_root, RecursiveMode::Recursive)
            .map_err(|error| GitError::parse("repository watcher", error.to_string()))?;
        if !paths.git_dir.starts_with(&paths.worktree_root) {
            watcher
                .watch(&paths.git_dir, RecursiveMode::Recursive)
                .map_err(|error| GitError::parse("repository watcher", error.to_string()))?;
        }
        if paths.common_dir != paths.git_dir && !paths.common_dir.starts_with(&paths.worktree_root)
        {
            watcher
                .watch(&paths.common_dir, RecursiveMode::Recursive)
                .map_err(|error| GitError::parse("repository watcher", error.to_string()))?;
        }
        let task = tokio::spawn(async move {
            while let Ok(first) = raw_rx.recv().await {
                let mut paths = first;
                while let Ok(Ok(more)) = tokio::time::timeout(DEBOUNCE, raw_rx.recv()).await {
                    paths.extend(more);
                }
                paths.sort();
                paths.dedup();
                if event_tx
                    .send(ChangeEvent {
                        paths,
                        debounce: DEBOUNCE,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        Ok((
            Self {
                _watcher: watcher,
                task,
            },
            event_rx,
        ))
    }
}

impl Drop for RepoWatcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn relevant(path: &Path, worktree: &Path, git_dir: &Path, common_dir: &Path) -> bool {
    let normalized = canonical(path);
    let path = normalized.as_path();
    for directory in [git_dir, common_dir] {
        if let Ok(relative) = path.strip_prefix(directory) {
            if relative.starts_with("objects") || relative == Path::new("index.lock") {
                return false;
            }
            return relative == Path::new("index")
                || relative == Path::new("HEAD")
                || relative.starts_with("refs")
                || relative == Path::new("packed-refs")
                || relative == Path::new("logs/HEAD")
                || relative == Path::new("config")
                || relative.starts_with("rebase-merge")
                || relative.starts_with("rebase-apply")
                || matches!(
                    relative.to_str(),
                    Some("MERGE_HEAD" | "CHERRY_PICK_HEAD" | "REVERT_HEAD" | "BISECT_START")
                );
        }
    }
    path.starts_with(worktree)
}

/// Canonicalizes `path`, falling back to its parent for entries that the event
/// reports after they were already deleted.
fn canonical(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => match std::fs::canonicalize(parent) {
            Ok(resolved) => resolved.join(name),
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}
