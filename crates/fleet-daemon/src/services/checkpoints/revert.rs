//! Restoring a worktree from a checkpoint, and nothing else.
//!
//! The algorithm is deliberately "diff the two trees, then apply the difference" rather than
//! `git read-tree -m -u`, for three reasons that all matter to the surface:
//!
//! 1. **It names what it did.** The user gets `restored 3 files, deleted 1`, which a two-tree
//!    merge does not report. A revert that cannot say what it changed is indistinguishable from
//!    one that did nothing.
//! 2. **It writes only what changed.** A two-tree merge rewrites every path it touches; on a
//!    worktree with 20 000 files this is the difference between a revert that costs a frame and
//!    one that costs a second and invalidates every build cache.
//! 3. **It scopes to paths.** `[u] revert this edit` restores the files one edit touched and
//!    leaves the rest of the turn's work alone, which is the same code path with a pathspec.
//!
//! What it never touches: `HEAD`, any branch, the user's index (every call runs against a scratch
//! index), the stash, the reflog, the harness process, and the transcript. A revert is a
//! filesystem operation that happens to be recorded in Git.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use fleet_core::agents::ThreadId;
use fleet_proto::agents::{AgentRevertReport, CheckpointId, CheckpointScope, REVERT_PATH_SAMPLE};

use super::{
    Checkpoints,
    error::{CheckpointError, Result},
    git::{Change, ScratchIndex},
    refs::{self, Metadata},
};

impl Checkpoints {
    /// Restores `worktree` to the state `checkpoint` recorded.
    ///
    /// Turn-scoped checkpoints restore the whole worktree; file-scoped ones restore exactly the
    /// paths that capture covered. In both cases a file the turn *created* is removed, because
    /// "the state before the turn" includes the absence of it.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::InvalidId`] for an identifier no daemon wrote,
    /// [`CheckpointError::Missing`] when the thread has no such checkpoint — which is the answer
    /// for a thread that never ran a turn, and the reason a revert never silently does nothing —
    /// [`CheckpointError::NotAWorktree`] when the recorded worktree is gone, and
    /// [`CheckpointError::Git`] or [`CheckpointError::Filesystem`] when restoring fails partway.
    pub async fn revert(
        &self,
        worktree: &Path,
        thread: ThreadId,
        checkpoint: &CheckpointId,
    ) -> Result<AgentRevertReport> {
        let parsed = CheckpointId::parse(checkpoint.as_str())?;
        let lock = self.lock_for(thread);
        let _guard = lock.lock().await;
        if !self.inner.git.is_worktree(worktree).await? {
            return Err(CheckpointError::NotAWorktree {
                worktree: worktree.to_path_buf(),
            });
        }

        let name = refs::ref_name(thread, &parsed.id);
        let Some(recorded) = self.inner.git.resolve_tree(worktree, &name).await? else {
            return Err(CheckpointError::Missing {
                thread,
                checkpoint: parsed.id,
            });
        };
        let metadata = Metadata::decode(
            &parsed.id,
            &self.inner.git.read_commit(worktree, &name).await?,
        )?;
        let scope = match metadata.scope {
            CheckpointScope::Turn => None,
            CheckpointScope::File => Some(metadata.paths.clone()),
        };

        let index = ScratchIndex::new();
        // The snapshot's scope is narrowed to what exists; the diff's is not. A file checkpoint
        // whose path the edit never created must still revert — as a no-op — rather than fail on
        // a pathspec Git cannot match.
        let staged = match &scope {
            Some(paths) => Some(super::existing_paths(worktree, paths).await),
            None => None,
        };
        let current = self
            .inner
            .git
            .snapshot_tree(worktree, &index, staged.as_deref())
            .await?;
        let differences = self
            .inner
            .git
            .diff_trees(
                worktree,
                &recorded,
                &current,
                scope.as_deref().unwrap_or_default(),
            )
            .await?;

        let mut restore = Vec::new();
        let mut remove = Vec::new();
        for difference in differences {
            // Git speaks bytes; this service speaks `String`. A path that did not survive UTF-8
            // would be written back under a *different* name, leaving the user two files where
            // they had one, so the whole revert is refused before anything is touched.
            if difference.path.contains('\u{FFFD}') {
                return Err(CheckpointError::NonUtf8Path { thread });
            }
            match difference.change {
                // In the checkpoint and not in the tree now, or different in the two: the
                // checkpoint's version is the one to write.
                Change::Deleted | Change::Modified => restore.push(difference.path),
                // The turn created it, so "before the turn" is its absence.
                Change::Added => remove.push(difference.path),
            }
        }

        if !restore.is_empty() {
            self.inner
                .git
                .read_tree(worktree, &index, &recorded)
                .await?;
            self.inner
                .git
                .checkout_paths(worktree, &index, &restore)
                .await?;
        }
        for path in &remove {
            remove_file(worktree, path).await?;
        }

        let report = AgentRevertReport {
            thread,
            checkpoint: parsed.id,
            restored: u32::try_from(restore.len()).unwrap_or(u32::MAX),
            deleted: u32::try_from(remove.len()).unwrap_or(u32::MAX),
            paths: sample(&restore, &remove),
        };
        tracing::info!(
            target: "fleet::agents",
            thread = %thread,
            checkpoint = %report.checkpoint,
            restored = report.restored,
            deleted = report.deleted,
            "reverted an agent worktree to a fleet checkpoint"
        );
        Ok(report)
    }
}

/// Removes one worktree-relative file, then the directories that removal emptied.
///
/// Empty parents are pruned because `git checkout` prunes them: a reverted turn that created
/// `src/generated/` must not leave the directory behind for a build script to find.
async fn remove_file(worktree: &Path, path: &str) -> Result<()> {
    let absolute = worktree.join(path);
    match tokio::fs::remove_file(&absolute).await {
        Ok(()) => {}
        // Already gone is the goal state. Anything else is reported: a revert that could not
        // remove a file the turn added has not restored the tree it claims to have.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(CheckpointError::Filesystem {
                path: absolute,
                source,
            });
        }
    }
    prune_empty_parents(worktree, &absolute).await;
    Ok(())
}

/// Walks up from a removed file, removing directories that are now empty, and stops at the
/// worktree root.
async fn prune_empty_parents(worktree: &Path, removed: &Path) {
    let mut directory = removed.parent().map(PathBuf::from);
    while let Some(candidate) = directory {
        if candidate == worktree || !candidate.starts_with(worktree) {
            return;
        }
        match tokio::fs::remove_dir(&candidate).await {
            Ok(()) => directory = candidate.parent().map(PathBuf::from),
            // Not empty, or not ours to remove: the walk is over. Nothing here is a failure of
            // the revert, which has already restored every file it recorded.
            Err(error) => {
                tracing::trace!(
                    target: "fleet::agents",
                    directory = %candidate.display(),
                    %error,
                    "stopped pruning empty directories after a checkpoint revert"
                );
                return;
            }
        }
    }
}

/// The bounded, sorted, de-duplicated sample of paths a report carries.
fn sample(restored: &[String], deleted: &[String]) -> Vec<String> {
    restored
        .iter()
        .chain(deleted)
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(REVERT_PATH_SAMPLE)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_sample_is_sorted_deduplicated_and_bounded() {
        let restored: Vec<String> = (0..REVERT_PATH_SAMPLE + 10)
            .map(|index| format!("src/{index:03}.rs"))
            .collect();
        let deleted = vec!["src/000.rs".to_owned(), "a.rs".to_owned()];

        let sample = sample(&restored, &deleted);

        assert_eq!(sample.len(), REVERT_PATH_SAMPLE);
        assert_eq!(sample.first().map(String::as_str), Some("a.rs"));
        assert!(sample.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
