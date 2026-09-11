//! Garbage collection: a checkpoint ref for a thread that no longer exists is removed.
//!
//! Two entry points, and the difference between them is who decided the thread is gone:
//! [`Checkpoints::forget_thread`] is called when Fleet deletes one, and
//! [`Checkpoints::retain`] sweeps a worktree against the set of threads that are still listed —
//! a daemon that was killed mid-delete, or a database restored from a backup, leaves refs behind
//! that only a sweep can find.
//!
//! **Deletion is per thread, never per prefix of anything else.** `docs/NATIVE-AGENTS.md` §8
//! states the rule for transcripts, and it holds here for the same reason: a checkpoint's
//! ordinal is only meaningful inside its thread, so "delete checkpoints below ordinal N" would
//! silently change what `[u]` on an older turn means. Either a thread's whole checkpoint history
//! is there, or the thread is gone.
//!
//! Deleting a ref makes its commit and any blobs only it referenced unreachable. Nothing here
//! runs `git gc`: the objects are collected by the repository's own maintenance, and forcing a
//! repack on a worktree the user is working in would be a far bigger surprise than a few
//! kilobytes of unreachable objects.

use std::{collections::HashSet, path::Path};

use fleet_core::agents::ThreadId;

use super::{Checkpoints, error::Result, refs};

impl Checkpoints {
    /// Removes every checkpoint of one thread, and reports how many refs were deleted.
    ///
    /// Idempotent: a thread with no checkpoints, and a worktree that is no longer a working tree,
    /// both answer zero. Deleting checkpoints for a thread Fleet has forgotten must never be the
    /// thing that fails a deletion.
    ///
    /// # Errors
    ///
    /// Returns [`super::CheckpointError::Git`] when the ref listing or a deletion fails.
    pub async fn forget_thread(&self, worktree: &Path, thread: ThreadId) -> Result<usize> {
        let deleted = self.delete_thread_refs(worktree, thread).await?;
        self.forget_lock(thread);
        Ok(deleted)
    }

    /// Removes the checkpoints of every thread in `worktree` that is not in `live`.
    ///
    /// `live` is the authority and the caller owns it: it must be the set of threads the daemon
    /// still lists for this worktree, taken *before* the sweep. A sweep with a partial set would
    /// delete a live thread's undo history, so a caller that cannot enumerate threads must not
    /// call this at all.
    ///
    /// # Errors
    ///
    /// Returns [`super::CheckpointError::Git`] when the ref listing or a deletion fails.
    pub async fn retain(&self, worktree: &Path, live: &HashSet<ThreadId>) -> Result<usize> {
        if !self.inner.git.is_worktree(worktree).await? {
            return Ok(0);
        }
        let namespace = format!("{}/", refs::NAMESPACE);
        let rows = self.inner.git.list_refs(worktree, &namespace).await?;
        let mut orphans: Vec<(ThreadId, String)> = Vec::new();
        for row in rows {
            match refs::parse_ref(&row.name) {
                Some((thread, _)) if !live.contains(&thread) => orphans.push((thread, row.name)),
                Some(_) => {}
                // A ref under Fleet's namespace that is not shaped like a checkpoint was not
                // written by this build. It is left exactly where it is: a garbage collector
                // that deletes what it cannot attribute is not a garbage collector.
                None => tracing::debug!(
                    target: "fleet::agents",
                    reference = %row.name,
                    "leaving an unrecognized ref in fleet's checkpoint namespace alone"
                ),
            }
        }

        let mut swept = HashSet::new();
        for (thread, name) in &orphans {
            self.inner.git.delete_ref(worktree, name).await?;
            swept.insert(*thread);
        }
        for thread in &swept {
            self.forget_lock(*thread);
        }
        if !orphans.is_empty() {
            tracing::info!(
                target: "fleet::agents",
                worktree = %worktree.display(),
                threads = swept.len(),
                references = orphans.len(),
                "swept fleet checkpoints for threads that no longer exist"
            );
        }
        Ok(orphans.len())
    }

    async fn delete_thread_refs(&self, worktree: &Path, thread: ThreadId) -> Result<usize> {
        if !self.inner.git.is_worktree(worktree).await? {
            return Ok(0);
        }
        let prefix = refs::thread_prefix(thread);
        let names: Vec<String> = self
            .inner
            .git
            .list_refs(worktree, &prefix)
            .await?
            .into_iter()
            .filter(|row| refs::parse_ref(&row.name).is_some_and(|(owner, _)| owner == thread))
            .map(|row| row.name)
            .collect();
        for name in &names {
            self.inner.git.delete_ref(worktree, name).await?;
        }
        if !names.is_empty() {
            tracing::debug!(
                target: "fleet::agents",
                thread = %thread,
                references = names.len(),
                "removed the fleet checkpoints of a deleted agent thread"
            );
        }
        Ok(names.len())
    }
}
