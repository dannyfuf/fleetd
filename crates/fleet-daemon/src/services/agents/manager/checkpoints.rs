//! Taking a Fleet-owned checkpoint, and the one rule that governs it.
//!
//! **A capture failure never refuses a turn** (`docs/NATIVE-AGENTS.md` §5). The user's work
//! matters more than Fleet's undo, and a turn refused because a `git write-tree` failed would be
//! the worst trade in the product. Every failure here is a log line plus one consequence the
//! listing already reports: `[u]` is not drawn for what was not captured.
//!
//! The service is installed after construction rather than taken as a constructor argument,
//! because `Services::build` creates both it and this manager and neither can precede the other.

use std::path::PathBuf;

use fleet_core::{
    agents::{ThreadId, TurnId},
    ids::WorktreeId,
};

use super::AgentSessionManager;

impl AgentSessionManager {
    /// Captures the worktree before a turn runs, or logs why it could not.
    ///
    /// §5's rule, and the only rule here: **a capture failure never refuses the turn.** The
    /// user's work matters more than Fleet's undo, and a turn refused because a `git write-tree`
    /// failed would be the worst trade in the product. The consequence of a failure is that
    /// `[u] revert turn` is not drawn for that turn, which is exactly what the listing reports.
    pub(super) async fn capture_turn_checkpoint(
        &self,
        thread: ThreadId,
        turn: TurnId,
        worktree: &WorktreeId,
    ) {
        let Some(checkpoints) = self
            .inner
            .checkpoints
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        else {
            return;
        };
        let path = match self.inner.worktrees.path(worktree.clone()).await {
            Ok(path) => PathBuf::from(path),
            Err(error) => {
                tracing::debug!(
                    target: "fleet::agents",
                    %error,
                    %thread,
                    "no checkpoint for this turn: the worktree path could not be resolved",
                );
                return;
            }
        };
        if let Err(error) = checkpoints.capture_turn(&path, thread, turn).await {
            tracing::warn!(
                target: "fleet::agents",
                %error,
                %thread,
                %turn,
                "could not checkpoint the worktree before this turn; the turn proceeds without one",
            );
        }
    }

    /// Captures the paths one edit is about to touch, or logs why it could not.
    ///
    /// Same rule as [`AgentSessionManager::capture_turn_checkpoint`]: the edit is already on its
    /// way to the harness and this is a best-effort record of what it is about to overwrite.
    pub(super) async fn capture_file_checkpoint(
        &self,
        thread: ThreadId,
        turn: TurnId,
        worktree: &WorktreeId,
        paths: Vec<String>,
    ) {
        if paths.is_empty() {
            return;
        }
        let Some(checkpoints) = self
            .inner
            .checkpoints
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        else {
            return;
        };
        let path = match self.inner.worktrees.path(worktree.clone()).await {
            Ok(path) => PathBuf::from(path),
            Err(error) => {
                tracing::debug!(
                    target: "fleet::agents",
                    %error,
                    %thread,
                    "no checkpoint for this edit: the worktree path could not be resolved",
                );
                return;
            }
        };
        if let Err(error) = checkpoints.capture_files(&path, thread, turn, &paths).await {
            tracing::warn!(
                target: "fleet::agents",
                %error,
                %thread,
                %turn,
                files = paths.len(),
                "could not checkpoint the files this edit touches; the edit proceeds without one",
            );
        }
    }
}
