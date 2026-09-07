//! Bounded repository snapshots and operation detection.

use self::refs::order_by_checkout_recency;
use crate::{
    CommandKind, GitError, Head, ObjectId, OperationState, RepoSnapshot, Repository, Result,
    SnapshotOptions,
};
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::SystemTime,
};

mod conflict;
mod diff;
mod history;
mod refs;

const MAX_SNAPSHOT_ATTEMPTS: usize = 3;

impl Repository {
    /// Loads a complete, bounded snapshot; independent reads execute concurrently.
    pub async fn snapshot(&self, options: SnapshotOptions) -> Result<RepoSnapshot> {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let _guard = self.mutation_lock.lock().await;
        self.retry_invalidated(|| self.collect_snapshot(options, generation))
            .await
    }

    async fn retry_invalidated<T, F, Fut>(&self, mut collect: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Option<T>>>,
    {
        for attempt in 1..=MAX_SNAPSHOT_ATTEMPTS {
            let invalidation = self.snapshot_invalidation.load(Ordering::Acquire);
            let value = collect().await?;
            if !self.snapshot_invalidated(invalidation)
                && let Some(value) = value
            {
                return Ok(value);
            }
            if attempt < MAX_SNAPSHOT_ATTEMPTS {
                tokio::task::yield_now().await;
            }
        }
        Err(GitError::parse(
            "repository snapshot",
            format!("repository changed during {MAX_SNAPSHOT_ATTEMPTS} collection attempts"),
        ))
    }

    async fn collect_snapshot(
        &self,
        options: SnapshotOptions,
        generation: u64,
    ) -> Result<Option<RepoSnapshot>> {
        let (target, index) = tokio::try_join!(
            self.read_head_target(),
            read_index_state(&self.paths.git_dir)
        )?;
        let head = self.read_head(&target);
        let files = self.read_status();
        let branches = self.read_local_branches();
        let remote_branches = self.read_remote_branches(options.include_remotes);
        let remotes = self.read_remotes(options.include_remotes);
        let tags = self.read_tags(options.include_tags);
        let commits = self.read_commits(target.oid.as_ref(), options.commit_limit);
        let reflog = self.read_reflog(target.oid.is_some(), options.reflog_limit);
        let stashes = self.read_stashes();
        let operation = detect_operation(&self.paths.git_dir);
        let (
            head,
            files,
            local_branches,
            remote_branches,
            remotes,
            tags,
            commits,
            reflog,
            stashes,
            operation,
        ) = tokio::join!(
            head,
            files,
            branches,
            remote_branches,
            remotes,
            tags,
            commits,
            reflog,
            stashes,
            operation,
        );
        let (final_target, final_index) = tokio::try_join!(
            self.read_head_target(),
            read_index_state(&self.paths.git_dir)
        )?;
        if target != final_target || index != final_index {
            return Ok(None);
        }
        let reflog = reflog?;
        let mut local_branches = local_branches?;
        order_by_checkout_recency(&mut local_branches, &reflog);
        Ok(Some(RepoSnapshot {
            root: self.paths.worktree_root.clone(),
            head: head?,
            operation,
            files: files?,
            local_branches,
            remote_branches: remote_branches?,
            remotes: remotes?,
            tags: tags?,
            commits: commits?,
            reflog,
            stashes: stashes?,
            generation,
        }))
    }

    async fn read_head_target(&self) -> Result<HeadTarget> {
        let branch = read_head_branch(&self.paths.git_dir);
        let oid = self.run_optional(self.command(CommandKind::Read).args([
            "rev-parse",
            "--verify",
            "--quiet",
            "HEAD^{commit}",
        ]));
        let (branch, oid_output) = tokio::try_join!(branch, oid)?;
        let oid_text = oid_output
            .as_ref()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default();
        Ok(HeadTarget {
            branch,
            oid: (!oid_text.is_empty()).then_some(ObjectId(oid_text)),
        })
    }

    async fn read_head(&self, target: &HeadTarget) -> Result<Head> {
        let Some(oid) = target.oid.clone() else {
            return Ok(Head::Unborn {
                name: if target.branch.is_empty() {
                    "HEAD".to_owned()
                } else {
                    target.branch.clone()
                },
            });
        };
        let description = self
            .runner
            .run(
                self.command(CommandKind::Read)
                    .args(["show", "-s", "--format=%s", oid.as_str()]),
            )
            .await?;
        let description = String::from_utf8_lossy(&description.stdout)
            .trim()
            .to_owned();
        if target.branch.is_empty() {
            Ok(Head::Detached { oid, description })
        } else {
            Ok(Head::Branch {
                name: target.branch.clone(),
                oid: Some(oid),
                description,
            })
        }
    }

    async fn read_status(&self) -> Result<Vec<crate::FileStatus>> {
        let output = self
            .runner
            .run(self.command(CommandKind::Read).args([
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--find-renames",
            ]))
            .await?;
        crate::parse::status::parse(&output.stdout)
    }

    fn snapshot_invalidated(&self, generation: u64) -> bool {
        generation != self.snapshot_invalidation.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HeadTarget {
    branch: String,
    oid: Option<ObjectId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexState {
    index: Option<IndexMetadata>,
    lock_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexMetadata {
    len: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

async fn read_head_branch(git_dir: &Path) -> Result<String> {
    let path = git_dir.join("HEAD");
    let content = tokio::fs::read(&path)
        .await
        .map_err(|source| GitError::Spawn {
            argv: vec![format!("read {}", path.display())],
            source,
        })?;
    let target = content.strip_prefix(b"ref: ").unwrap_or_default();
    let target = target.strip_suffix(b"\n").unwrap_or(target);
    let target = target.strip_suffix(b"\r").unwrap_or(target);
    Ok(String::from_utf8_lossy(target.strip_prefix(b"refs/heads/").unwrap_or(target)).into_owned())
}

async fn read_index_state(git_dir: &Path) -> Result<IndexState> {
    let index = optional_metadata(&git_dir.join("index"))
        .await?
        .map(IndexMetadata::from_metadata)
        .transpose()?;
    let lock_present = optional_metadata(&git_dir.join("index.lock"))
        .await?
        .is_some();
    Ok(IndexState {
        index,
        lock_present,
    })
}

async fn optional_metadata(path: &Path) -> Result<Option<std::fs::Metadata>> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(GitError::Spawn {
            argv: vec![format!("metadata {}", path.display())],
            source,
        }),
    }
}

impl IndexMetadata {
    fn from_metadata(metadata: std::fs::Metadata) -> Result<Self> {
        let modified = metadata.modified().map_err(|source| GitError::Spawn {
            argv: vec!["read index modification time".to_owned()],
            source,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                len: metadata.len(),
                modified,
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                len: metadata.len(),
                modified,
            })
        }
    }
}

async fn detect_operation(git_dir: &Path) -> OperationState {
    let merge = git_dir.join("rebase-merge");
    let apply = git_dir.join("rebase-apply");
    let merge_exists = tokio::fs::metadata(&merge)
        .await
        .is_ok_and(|metadata| metadata.is_dir());
    let apply_exists = tokio::fs::metadata(&apply)
        .await
        .is_ok_and(|metadata| metadata.is_dir());
    if merge_exists || apply_exists {
        let directory = if merge_exists { &merge } else { &apply };
        let done = read_count(directory.join("done")).await;
        let remaining = read_count(directory.join("git-rebase-todo")).await;
        return OperationState::Rebasing {
            interactive: merge_exists
                || tokio::fs::metadata(directory.join("interactive"))
                    .await
                    .is_ok(),
            onto: read_trimmed(directory.join("onto")).await,
            head_name: read_trimmed(directory.join("head-name")).await,
            done,
            total: match (done, remaining) {
                (Some(done), Some(remaining)) => Some(done + remaining),
                _ => None,
            },
        };
    }
    if tokio::fs::metadata(git_dir.join("MERGE_HEAD"))
        .await
        .is_ok()
    {
        return OperationState::Merging;
    }
    if tokio::fs::metadata(git_dir.join("CHERRY_PICK_HEAD"))
        .await
        .is_ok()
    {
        return OperationState::CherryPicking;
    }
    if tokio::fs::metadata(git_dir.join("REVERT_HEAD"))
        .await
        .is_ok()
    {
        return OperationState::Reverting;
    }
    if tokio::fs::metadata(git_dir.join("BISECT_START"))
        .await
        .is_ok()
    {
        return OperationState::Bisecting;
    }
    OperationState::None
}

async fn read_trimmed(path: PathBuf) -> Option<String> {
    tokio::fs::read(path)
        .await
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .filter(|value| !value.is_empty())
}

async fn read_count(path: PathBuf) -> Option<usize> {
    tokio::fs::read(path).await.ok().map(|bytes| {
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| {
                let line = line.trim_ascii();
                !line.is_empty() && !line.starts_with(b"#")
            })
            .count()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32},
    };
    use tokio::sync::Mutex;

    fn repository() -> Repository {
        Repository {
            paths: crate::RepoPaths {
                worktree_root: PathBuf::from("/worktree"),
                git_dir: PathBuf::from("/worktree/.git"),
                common_dir: PathBuf::from("/worktree/.git"),
            },
            runner: Arc::new(crate::Runner::default()),
            mutation_lock: Mutex::new(()),
            generation: std::sync::atomic::AtomicU64::new(0),
            snapshot_invalidation: std::sync::atomic::AtomicU64::new(1),
            diff_context: AtomicU32::new(crate::DEFAULT_DIFF_CONTEXT),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn snapshot_retries_on_invalidation() {
        let repository = repository();
        let attempts = std::cell::Cell::new(0);
        let yielded = Arc::new(AtomicBool::new(false));
        let observer = Arc::clone(&yielded);
        let observer = tokio::spawn(async move {
            observer.store(true, Ordering::Release);
        });
        let value = repository
            .retry_invalidated(|| {
                let attempt = attempts.get() + 1;
                attempts.set(attempt);
                if attempt > 1 {
                    assert!(yielded.load(Ordering::Acquire));
                }
                if attempt == 1 {
                    repository
                        .snapshot_invalidation
                        .fetch_add(1, Ordering::AcqRel);
                }
                std::future::ready(Ok(Some(attempt)))
            })
            .await
            .unwrap();
        observer.await.unwrap();

        assert_eq!(attempts.get(), 2);
        assert_eq!(value, 2);
    }

    #[tokio::test]
    async fn snapshot_retry_exhaustion_is_bounded() {
        let repository = repository();
        let attempts = std::cell::Cell::new(0);

        let error = repository
            .retry_invalidated(|| {
                attempts.set(attempts.get() + 1);
                std::future::ready(Ok::<Option<()>, GitError>(None))
            })
            .await
            .unwrap_err();

        assert_eq!(attempts.get(), MAX_SNAPSHOT_ATTEMPTS);
        assert!(matches!(
            error,
            GitError::Parse {
                context: "repository snapshot",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn index_state_tracks_lock_presence_without_reading_index_contents() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("index"), b"index bytes").unwrap();
        let unlocked = read_index_state(directory.path()).await.unwrap();
        std::fs::write(directory.path().join("index.lock"), b"pending").unwrap();
        let locked = read_index_state(directory.path()).await.unwrap();

        assert!(unlocked.index.is_some());
        assert!(!unlocked.lock_present);
        assert!(locked.lock_present);
        assert_eq!(unlocked.index, locked.index);
    }
}
