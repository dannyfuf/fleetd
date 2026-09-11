//! Fleet-owned turn checkpoints: a hidden Git ref of the worktree before each turn, and of a
//! file before the first edit that touches it.
//!
//! `docs/NATIVE-AGENTS.md` §5 fixes three decisions this service exists to implement, and each
//! one is a deliberate refusal of an easier design:
//!
//! **A checkpoint is Fleet's, not the harness's.** It is taken before Fleet asks a harness to do
//! anything, so it exists whether or not the turn completes, whether or not the harness crashes
//! mid-edit, and whether or not the harness has a rewind of its own. Codex has one —
//! `thread/rollback` — and it is explicitly *not* used: it is deprecated upstream and documented
//! to modify only the thread's history, leaving local file changes exactly where they were. That
//! is the opposite of the trade a user pressing `[u]` wants.
//!
//! **A revert restores files and nothing else.** It does not touch the harness's conversation,
//! does not interrupt a turn, and does not move `HEAD`, a branch, the index, or the stash.
//! Reverting a working tree and rewinding a model's context are different operations, and
//! conflating them is how a user loses work they wanted to keep. The model still remembers the
//! edit it made; the files no longer have it. That asymmetry is the point — the next turn is told
//! about the revert by the user, in the composer, the same way it would be told anything else.
//!
//! **The ref namespace is the store.** `refs/fleet/checkpoints/<thread>/<checkpoint>` is not a
//! cache of a table: it is the only record, for reasons written out in [`refs`]. The projector's
//! `checkpoints` table records `AgentEvent::Compacted` boundaries and is erased and rebuilt from
//! the log by `rebuild_thread`, which would take a checkpoint index with it while leaving the
//! refs it named orphaned and unattributable.
//!
//! ```text
//! before a turn  ─► capture_turn  ─► add --all → write-tree → commit-tree → update-ref
//! before an edit ─► capture_files ─► add -- <paths> → … (the same three)
//! [u] pressed    ─► revert        ─► snapshot the tree now, diff it against the checkpoint,
//!                                    checkout-index what changed, unlink what the turn added
//! thread deleted ─► forget_thread ─► update-ref -d, per thread, never per prefix
//! ```
//!
//! Every Git call runs with `GIT_INDEX_FILE` pointed at a scratch index, so nothing here can
//! disturb what the user has staged or contend with a `git` they ran themselves ([`git`]).

mod error;
mod gc;
mod git;
mod refs;
mod requests;
mod revert;
#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::Utc;
use fleet_core::agents::{ThreadId, TurnId};
use fleet_proto::agents::{CheckpointId, CheckpointScope};

pub use error::{CheckpointError, Result};
pub use refs::Checkpoint;
pub(super) use requests::run_sweep;

use git::{Plumbing, ScratchIndex};
use refs::Metadata;

/// Fleet-owned checkpoints for every worktree this daemon owns.
///
/// Cloning shares the per-thread capture locks, which is what makes the clone safe to hand to the
/// agent manager: two captures for one thread are serialized, and two captures for two threads
/// are not.
#[derive(Clone)]
pub struct Checkpoints {
    inner: Arc<Inner>,
}

struct Inner {
    git: Plumbing,
    /// One lock per thread, held across a whole capture or revert.
    ///
    /// Per thread rather than one for the service, because a capture stats a whole worktree: a
    /// turn starting in one repository must not wait on a revert in another. Per thread rather
    /// than per worktree, because the ordinal it allocates is per thread — two threads in one
    /// worktree cannot collide on a ref name.
    locks: Mutex<HashMap<ThreadId, Arc<tokio::sync::Mutex<()>>>>,
}

impl Checkpoints {
    /// Builds the service over the daemon's process boundary.
    #[must_use]
    pub fn new(shell: Arc<dyn crate::adapters::shell::Shell>) -> Self {
        Self {
            inner: Arc::new(Inner {
                git: Plumbing::new(shell),
                locks: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Records the whole worktree before `turn` is submitted to a harness.
    ///
    /// Called on the path that starts a turn, before the harness is asked to do anything. A
    /// failure here is the caller's to decide about, and the caller's rule is written down in
    /// `docs/NATIVE-AGENTS.md` §5: a checkpoint that could not be taken means `[u]` is not drawn,
    /// never that the turn is refused. The user's work is more important than Fleet's undo.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError`] when the path is not a working tree or Git refuses a call.
    pub async fn capture_turn(
        &self,
        worktree: &Path,
        thread: ThreadId,
        turn: TurnId,
    ) -> Result<Checkpoint> {
        self.capture(worktree, thread, turn, CheckpointScope::Turn, None)
            .await
    }

    /// Records `paths` before an edit inside `turn` touches them.
    ///
    /// A path that does not exist yet is recorded as *absent*: its checkpoint tree simply has no
    /// entry for it, and reverting deletes the file the edit went on to create. That is why the
    /// requested paths are kept in the checkpoint's own record rather than derived from its tree.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::EmptyScope`] when `paths` is empty,
    /// [`CheckpointError::PathOutsideWorktree`] when one of them is not inside `worktree`, and
    /// the Git failures [`Checkpoints::capture_turn`] can return.
    pub async fn capture_files(
        &self,
        worktree: &Path,
        thread: ThreadId,
        turn: TurnId,
        paths: &[String],
    ) -> Result<Checkpoint> {
        if paths.is_empty() {
            return Err(CheckpointError::EmptyScope { thread });
        }
        let mut relative = Vec::with_capacity(paths.len());
        for path in paths {
            relative.push(relative_path(worktree, path)?);
        }
        relative.sort_unstable();
        relative.dedup();
        self.capture(
            worktree,
            thread,
            turn,
            CheckpointScope::File,
            Some(relative),
        )
        .await
    }

    /// Lists one thread's checkpoints, oldest first.
    ///
    /// A worktree that is no longer a Git working tree — deleted, or moved out from under the
    /// thread — answers an empty list rather than an error: "there is nothing to revert to" is
    /// the true answer, and it is the answer `[u]` needs in order not to be drawn.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError::Git`] when the ref listing itself fails.
    pub async fn list(&self, worktree: &Path, thread: ThreadId) -> Result<Vec<Checkpoint>> {
        if !self.inner.git.is_worktree(worktree).await? {
            tracing::debug!(
                target: "fleet::agents",
                thread = %thread,
                worktree = %worktree.display(),
                "listing no checkpoints: the recorded worktree is not a git working tree"
            );
            return Ok(Vec::new());
        }
        let prefix = refs::thread_prefix(thread);
        let mut checkpoints: Vec<Checkpoint> = self
            .inner
            .git
            .list_refs(worktree, &prefix)
            .await?
            .into_iter()
            .filter_map(|row| {
                let (owner, id) = refs::parse_ref(&row.name)?;
                (owner == thread)
                    .then(|| Checkpoint::from_ref(id, row.created_at))
                    .flatten()
            })
            .collect();
        checkpoints.sort_unstable_by_key(|checkpoint| checkpoint.ordinal);
        Ok(checkpoints)
    }

    async fn capture(
        &self,
        worktree: &Path,
        thread: ThreadId,
        turn: TurnId,
        scope: CheckpointScope,
        paths: Option<Vec<String>>,
    ) -> Result<Checkpoint> {
        let lock = self.lock_for(thread);
        let _guard = lock.lock().await;
        if !self.inner.git.is_worktree(worktree).await? {
            return Err(CheckpointError::NotAWorktree {
                worktree: worktree.to_path_buf(),
            });
        }

        // Only the paths that exist are handed to `git add`: a pathspec matching nothing is an
        // error, and "this file does not exist yet" is exactly what a pre-edit checkpoint of a
        // new file has to record.
        let staged = match &paths {
            Some(paths) => Some(existing_paths(worktree, paths).await),
            None => None,
        };
        let index = ScratchIndex::new();
        let tree = self
            .inner
            .git
            .snapshot_tree(worktree, &index, staged.as_deref())
            .await?;

        let ordinal = self.next_ordinal(worktree, thread).await?;
        let id = CheckpointId::from_parts(ordinal, scope, turn);
        let metadata = Metadata::new(thread, turn, scope, ordinal, paths.unwrap_or_default());
        let commit = self
            .inner
            .git
            .commit_tree(worktree, &tree, &metadata.message()?)
            .await?;
        self.inner
            .git
            .update_ref(worktree, &refs::ref_name(thread, &id), &commit)
            .await?;
        tracing::debug!(
            target: "fleet::agents",
            thread = %thread,
            turn = %turn,
            checkpoint = %id,
            scope = scope.as_token(),
            "recorded a fleet checkpoint"
        );
        Ok(Checkpoint {
            id,
            scope,
            turn,
            ordinal,
            at: Utc::now(),
        })
    }

    /// The next capture order for a thread, read from the refs themselves.
    ///
    /// Derived rather than counted in memory, so it survives a daemon restart and stays correct
    /// for a thread this process has never hydrated. It is allocated under the thread's capture
    /// lock, and `update-ref` is told the ref must not already exist, so a collision fails loudly
    /// instead of overwriting a recorded tree.
    async fn next_ordinal(&self, worktree: &Path, thread: ThreadId) -> Result<u32> {
        let highest = self
            .inner
            .git
            .list_refs(worktree, &refs::thread_prefix(thread))
            .await?
            .into_iter()
            .filter_map(|row| {
                let (_, id) = refs::parse_ref(&row.name)?;
                CheckpointId::parse(id.as_str()).ok().map(|id| id.ordinal)
            })
            .max()
            .unwrap_or(0);
        Ok(highest.saturating_add(1))
    }

    fn lock_for(&self, thread: ThreadId) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.inner
                .locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(thread)
                .or_default(),
        )
    }

    fn forget_lock(&self, thread: ThreadId) {
        self.inner
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&thread);
    }
}

/// Narrows a harness-supplied path to a worktree-relative one, or refuses it.
///
/// Both spellings a harness uses are accepted — Claude reports absolute paths, Codex reports
/// worktree-relative ones — and everything else is refused: an absolute path outside the
/// worktree, a `..` that climbs out of it, a root, and an empty string. A checkpoint must never
/// be able to record, or restore, a file the agent's worktree does not contain.
fn relative_path(worktree: &Path, path: &str) -> Result<String> {
    let refuse = || CheckpointError::PathOutsideWorktree {
        worktree: worktree.to_path_buf(),
        path: path.to_owned(),
    };
    let candidate = Path::new(path);
    let relative = if candidate.is_absolute() {
        candidate.strip_prefix(worktree).map_err(|_| refuse())?
    } else {
        candidate
    };
    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            // `./a` is the same file as `a`; everything else can leave the worktree.
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(refuse());
            }
        }
    }
    let normalized = normalized.to_str().ok_or_else(refuse)?.to_owned();
    if normalized.is_empty() {
        return Err(refuse());
    }
    Ok(normalized)
}

/// Keeps only the paths that exist on disk right now.
///
/// Both a capture and a revert need this, and for the same reason: `git add -- <pathspec>` fails
/// the whole invocation when one pathspec matches nothing, so a scope naming a file that does not
/// exist — the usual case before an edit creates one, and again if that edit never ran — has to be
/// narrowed before Git sees it. A tree-to-tree `diff` has no such problem, so the *unnarrowed*
/// scope is still what the difference is computed over.
pub(super) async fn existing_paths(worktree: &Path, paths: &[String]) -> Vec<String> {
    let mut existing = Vec::with_capacity(paths.len());
    for path in paths {
        match tokio::fs::symlink_metadata(worktree.join(path)).await {
            Ok(_) => existing.push(path.clone()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                // Unreadable is not absent: recording it as absent would make a revert delete a
                // file it never captured, so it is staged and Git reports the real problem.
                tracing::debug!(
                    target: "fleet::agents",
                    path = %path,
                    %error,
                    "could not stat a path a checkpoint covers; letting git decide"
                );
                existing.push(path.clone());
            }
        }
    }
    existing
}
