//! Debounced repository filesystem watcher.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::time::Instant;

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{ChangeEvent, GitError, RepoPaths, Result};

const DEBOUNCE: Duration = Duration::from_millis(150);
const MAX_DEBOUNCE: Duration = Duration::from_secs(1);
const MAX_DIRTY_PATHS: usize = 1024;

/// Keeps native filesystem watches and the debounce task alive.
#[derive(Debug)]
pub struct RepoWatcher {
    _watcher: RecommendedWatcher,
    task: tokio::task::JoinHandle<()>,
}

impl RepoWatcher {
    /// Watches worktree content and relevant per-worktree/shared Git metadata.
    pub fn new(paths: &RepoPaths) -> Result<(Self, async_channel::Receiver<ChangeEvent>)> {
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let pending = Arc::new(Mutex::new(PendingChanges::new(paths)));
        let (event_tx, event_rx) = async_channel::bounded(32);
        let worktree = paths.worktree_root.clone();
        let git_dir = paths.git_dir.clone();
        let common_dir = paths.common_dir.clone();
        let callback_pending = Arc::clone(&pending);
        let mut watcher = RecommendedWatcher::new(
            move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    let mut changed = false;
                    for path in event.paths {
                        if relevant(&path, &worktree, &git_dir, &common_dir) {
                            callback_pending
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .insert(path, Instant::now());
                            changed = true;
                        }
                    }
                    if changed {
                        // A pending wake already covers every path in the shared buffer.
                        let _ = wake_tx.try_send(());
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
        let task = tokio::spawn(forward_changes(pending, wake_rx, event_tx));
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

#[derive(Debug)]
struct PendingChanges {
    roots: BTreeSet<PathBuf>,
    paths: BTreeSet<PathBuf>,
    full_refresh: bool,
    started_at: Option<Instant>,
    updated_at: Instant,
}

impl PendingChanges {
    fn new(paths: &RepoPaths) -> Self {
        Self {
            roots: [
                paths.worktree_root.clone(),
                paths.git_dir.clone(),
                paths.common_dir.clone(),
            ]
            .into_iter()
            .collect(),
            paths: BTreeSet::new(),
            full_refresh: false,
            started_at: None,
            updated_at: Instant::now(),
        }
    }

    fn insert(&mut self, path: PathBuf, now: Instant) {
        self.started_at.get_or_insert(now);
        self.updated_at = now;
        if !self.full_refresh {
            self.paths.insert(path);
            if self.paths.len() > MAX_DIRTY_PATHS {
                self.paths.clone_from(&self.roots);
                self.full_refresh = true;
            }
        }
    }

    fn deadline(&self) -> Option<Instant> {
        self.started_at
            .map(|started| (started + MAX_DEBOUNCE).min(self.updated_at + DEBOUNCE))
    }

    fn take_ready(&mut self, now: Instant) -> Option<Vec<PathBuf>> {
        if !self.deadline().is_some_and(|deadline| deadline <= now) {
            return None;
        }
        self.started_at = None;
        self.full_refresh = false;
        Some(std::mem::take(&mut self.paths).into_iter().collect())
    }
}

async fn forward_changes(
    pending: Arc<Mutex<PendingChanges>>,
    wake_rx: async_channel::Receiver<()>,
    event_tx: async_channel::Sender<ChangeEvent>,
) {
    while wake_rx.recv().await.is_ok() {
        loop {
            let (batch, deadline) = {
                let mut pending = pending.lock().unwrap_or_else(|error| error.into_inner());
                (pending.take_ready(Instant::now()), pending.deadline())
            };
            if let Some(paths) = batch {
                // While the consumer is scanning or backpressured, new invalidations
                // stay in the bounded shared buffer for the next refresh.
                if event_tx
                    .send(ChangeEvent {
                        paths,
                        debounce: DEBOUNCE,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                break;
            }
            let Some(deadline) = deadline else {
                break;
            };
            if matches!(
                tokio::time::timeout_at(deadline, wake_rx.recv()).await,
                Ok(Err(_))
            ) {
                return;
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> RepoPaths {
        RepoPaths {
            worktree_root: PathBuf::from("/worktree"),
            git_dir: PathBuf::from("/common/worktrees/linked"),
            common_dir: PathBuf::from("/common"),
        }
    }

    #[test]
    fn repeated_paths_coalesce_and_continuous_changes_have_a_deadline() {
        let mut pending = PendingChanges::new(&paths());
        let start = Instant::now();
        for tick in 0..100_000 {
            pending.insert(
                PathBuf::from("/worktree/file"),
                start + Duration::from_micros(tick * 9),
            );
        }
        assert!(
            pending
                .take_ready(start + MAX_DEBOUNCE - Duration::from_nanos(1))
                .is_none()
        );
        assert_eq!(
            pending.take_ready(start + MAX_DEBOUNCE),
            Some(vec![PathBuf::from("/worktree/file")])
        );
        assert!(pending.deadline().is_none());
    }

    #[test]
    fn path_overflow_invalidates_all_roots_then_starts_a_fresh_burst() {
        let paths = paths();
        let mut pending = PendingChanges::new(&paths);
        let now = Instant::now();
        for index in 0..MAX_DIRTY_PATHS * 10 {
            pending.insert(PathBuf::from(format!("/worktree/{index}")), now);
        }
        let roots = pending.take_ready(now + DEBOUNCE).unwrap();
        assert_eq!(roots.len(), 3);
        for root in [paths.worktree_root, paths.git_dir, paths.common_dir] {
            assert!(roots.contains(&root));
        }
        pending.insert(PathBuf::from("/worktree/next"), now + DEBOUNCE);
        assert_eq!(
            pending.take_ready(now + DEBOUNCE * 2),
            Some(vec![PathBuf::from("/worktree/next")])
        );
    }

    #[tokio::test]
    async fn slow_consumer_keeps_invalidations_arriving_during_delivery() {
        let pending = Arc::new(Mutex::new(PendingChanges::new(&paths())));
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let (event_tx, event_rx) = async_channel::bounded(1);
        let task = tokio::spawn(forward_changes(Arc::clone(&pending), wake_rx, event_tx));
        for name in ["first", "second", "third"] {
            pending
                .lock()
                .unwrap()
                .insert(PathBuf::from(name), Instant::now() - MAX_DEBOUNCE);
            let _ = wake_tx.try_send(());
            // Let the forwarder fill the output slot, then block on its next delivery.
            tokio::task::yield_now().await;
        }
        let mut received = BTreeSet::new();
        while received.len() < 3 {
            let event = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
                .await
                .unwrap()
                .unwrap();
            received.extend(event.paths);
        }
        assert_eq!(
            received,
            ["first", "second", "third"]
                .into_iter()
                .map(PathBuf::from)
                .collect()
        );
        drop(wake_tx);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn forwarder_delivers_during_a_continuous_burst() {
        let pending = Arc::new(Mutex::new(PendingChanges::new(&paths())));
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let (event_tx, event_rx) = async_channel::bounded(1);
        let task = tokio::spawn(forward_changes(Arc::clone(&pending), wake_rx, event_tx));
        let start = Instant::now();
        let producer = async {
            loop {
                pending
                    .lock()
                    .unwrap()
                    .insert(PathBuf::from("active"), Instant::now());
                let _ = wake_tx.try_send(());
                tokio::time::sleep(DEBOUNCE / 4).await;
            }
        };
        let event = tokio::select! {
            event = tokio::time::timeout(MAX_DEBOUNCE * 3, event_rx.recv()) => event.unwrap().unwrap(),
            _ = producer => unreachable!(),
        };
        assert!(start.elapsed() >= MAX_DEBOUNCE);
        assert_eq!(event.paths, vec![PathBuf::from("active")]);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    }
}
