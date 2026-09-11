//! The typed failures of the checkpoint service.
//!
//! Typed rather than `anyhow`, against the daemon's usual grain, for one reason: §5 says `[u]` is
//! drawn only where a checkpoint exists, so every surface that draws it has to be able to tell
//! "there is nothing to revert to" from "Git failed" without reading English. A revert that
//! silently did nothing is the failure this service exists to avoid, so "nothing to revert to" is
//! a variant, never an `Ok` with an empty report.

use std::{io, path::PathBuf};

use fleet_core::agents::ThreadId;
use fleet_proto::agents::{CheckpointId, InvalidCheckpointId};

use crate::DaemonError;

/// Result type used across the checkpoint service.
pub type Result<T> = std::result::Result<T, CheckpointError>;

/// A typed checkpoint or revert failure.
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    /// The thread has no checkpoint with that identity.
    ///
    /// Reached by a `[u]` drawn from a listing the garbage collector has since swept, and by a
    /// revert on a thread that never ran a turn. Both are the same answer to the user.
    #[error("checkpoint {checkpoint} of agent thread {thread}")]
    Missing {
        /// Thread the revert named.
        thread: ThreadId,
        /// Checkpoint the revert named.
        checkpoint: CheckpointId,
    },
    /// The identifier is not the shape [`CheckpointId::from_parts`] writes.
    ///
    /// The identifier becomes the leaf of a ref name, so this is refused before Git is reached at
    /// all: a peer must never be able to name `refs/heads/main` through it.
    #[error(transparent)]
    InvalidId(#[from] InvalidCheckpointId),
    /// A path the revert would touch did not survive the round trip through UTF-8.
    ///
    /// Git speaks bytes and this service speaks `String`, so a path that is not valid UTF-8
    /// reaches it with replacement characters in it. Writing that path back would create a *new*
    /// file beside the one the user has, so the whole revert is refused instead: refusing loudly
    /// is recoverable, half-reverting a tree is not.
    #[error(
        "agent thread {thread} has a checkpointed path that is not valid UTF-8, so Fleet refused to revert it"
    )]
    NonUtf8Path {
        /// Thread the revert named.
        thread: ThreadId,
    },
    /// A file-scoped capture was asked to cover no files at all.
    ///
    /// A programming error rather than a user one, and refused rather than silently recorded:
    /// an empty scope would capture an empty tree, and reverting *that* would delete every file
    /// the turn touched.
    #[error("a file checkpoint for agent thread {thread} must name at least one path")]
    EmptyScope {
        /// Thread the capture was for.
        thread: ThreadId,
    },
    /// A path a capture named is not inside the thread's worktree.
    ///
    /// Claude reports absolute paths and Codex worktree-relative ones, so both are accepted and
    /// everything else — a `..` that climbs out, an absolute path elsewhere on disk, a root — is
    /// refused. A checkpoint that could record a file outside the worktree could restore one.
    #[error("{path} is not inside worktree {}", worktree.display())]
    PathOutsideWorktree {
        /// The worktree the capture was scoped to.
        worktree: PathBuf,
        /// The path that was refused, as the caller spelled it.
        path: String,
    },
    /// The recorded worktree is not a Git working tree any more.
    #[error("worktree {} is not a Git working tree", worktree.display())]
    NotAWorktree {
        /// The path that was checked.
        worktree: PathBuf,
    },
    /// A Git invocation failed.
    #[error("git {operation} failed: {message}")]
    Git {
        /// The plumbing operation, never the argv: an argv carries worktree paths into logs.
        operation: &'static str,
        /// Git's own concise text.
        message: String,
    },
    /// A checkpoint commit carries a body this build cannot read.
    ///
    /// A checkpoint written by a newer Fleet is refused rather than guessed at, exactly as an
    /// unknown transcript header version is: reverting to a tree whose recorded scope was
    /// misread would restore the wrong set of files.
    #[error("checkpoint {checkpoint} was written by another Fleet build: {message}")]
    Metadata {
        /// Checkpoint whose body could not be read.
        checkpoint: CheckpointId,
        /// What was wrong with it.
        message: String,
    },
    /// A filesystem operation failed.
    #[error("filesystem operation failed for {}: {source}", path.display())]
    Filesystem {
        /// Path involved in the failed operation.
        path: PathBuf,
        /// Operating-system error.
        #[source]
        source: io::Error,
    },
}

impl CheckpointError {
    /// Builds a Git failure from a completed invocation's own output.
    pub(super) fn git(operation: &'static str, status: i32, stderr: &str, stdout: &str) -> Self {
        let message = [stderr, stdout]
            .into_iter()
            .map(str::trim)
            .find(|text| !text.is_empty())
            .unwrap_or("no output");
        Self::Git {
            operation,
            message: format!("exited {status}: {message}"),
        }
    }
}

impl From<CheckpointError> for DaemonError {
    fn from(error: CheckpointError) -> Self {
        match &error {
            // The same wording `fleet_proto::agents::checkpoint_missing_error` carries, so a
            // local refusal and a remote owner's refusal read identically; `NotFound` renders
            // the "not found: " prefix the rest of the daemon uses.
            CheckpointError::Missing { .. } => Self::NotFound(error.to_string()),
            CheckpointError::InvalidId(_)
            | CheckpointError::NonUtf8Path { .. }
            | CheckpointError::EmptyScope { .. }
            | CheckpointError::PathOutsideWorktree { .. } => Self::Validation(error.to_string()),
            CheckpointError::NotAWorktree { .. } | CheckpointError::Metadata { .. } => {
                Self::Conflict(error.to_string())
            }
            CheckpointError::Git { .. } => Self::Git(error.to_string()),
            CheckpointError::Filesystem { path, .. } => Self::fs(
                path.clone(),
                io::Error::other(
                    // The `#[source]` chain is flattened here on purpose: `DaemonError::fs`
                    // renders its own source, and the surface shows one sentence.
                    error.to_string(),
                ),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use fleet_core::agents::TurnId;
    use fleet_proto::{
        agents::{CheckpointScope, checkpoint_missing_error},
        error::ErrorKind,
    };

    use super::*;

    #[test]
    fn a_missing_checkpoint_says_exactly_what_the_protocol_says() {
        let thread = ThreadId::new();
        let checkpoint = CheckpointId::from_parts(3, CheckpointScope::Turn, TurnId::new());
        let local = DaemonError::from(CheckpointError::Missing {
            thread,
            checkpoint: checkpoint.clone(),
        });
        let wire = fleet_proto::error::ProtoError::from(local);
        let remote = checkpoint_missing_error(thread, &checkpoint);

        assert_eq!(wire.kind, ErrorKind::NotFound);
        assert_eq!(wire.kind, remote.kind);
        assert!(
            wire.message.ends_with(&remote.message),
            "{} must carry {}",
            wire.message,
            remote.message
        );
    }

    #[test]
    fn a_hostile_identifier_is_a_validation_failure_and_never_a_git_one() {
        let error = DaemonError::from(CheckpointError::InvalidId(InvalidCheckpointId {
            value: "../../heads/main".to_owned(),
        }));

        assert_eq!(
            fleet_proto::error::ProtoError::from(error).kind,
            ErrorKind::Validation
        );
    }

    #[test]
    fn a_git_failure_quotes_git_and_never_the_argv() {
        let error = CheckpointError::git("write-tree", 128, "fatal: not a tree object\n", "");

        assert_eq!(
            error.to_string(),
            "git write-tree failed: exited 128: fatal: not a tree object"
        );
        assert_eq!(
            fleet_proto::error::ProtoError::from(DaemonError::from(error)).kind,
            ErrorKind::Git
        );
    }

    #[test]
    fn a_git_failure_with_no_output_still_reads_as_a_sentence() {
        assert_eq!(
            CheckpointError::git("update-ref", 1, "  ", "\n").to_string(),
            "git update-ref failed: exited 1: no output"
        );
    }
}
