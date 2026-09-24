use std::path::PathBuf;

use super::{DiffKind, ObjectId, Ref};

/// One path the checked-out branch changed against its base, committed or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchFile {
    /// Current path, relative to the worktree root.
    pub path: PathBuf,
    /// Previous path for a rename or copy.
    pub previous_path: Option<PathBuf>,
    /// What happened to the path: added, deleted, modified, renamed.
    pub kind: DiffKind,
    /// Lines added, or `None` for a binary file.
    pub added: Option<u64>,
    /// Lines removed, or `None` for a binary file.
    pub removed: Option<u64>,
}

/// One commit on the branch that its base does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AheadCommit {
    /// Full commit id.
    pub oid: ObjectId,
    /// First line of the message.
    pub subject: String,
}

/// What a worktree changed against the ref it was created from.
///
/// The files compare the working tree — uncommitted and untracked changes included — with the
/// merge base of `HEAD` and the base, so they are exactly what a pull request from this worktree
/// would contain if everything were committed now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchChanges {
    /// The base ref as the caller named it.
    pub base: Ref,
    /// The commit the files are compared against.
    pub merge_base: ObjectId,
    /// Every changed path, sorted by path.
    pub files: Vec<BranchFile>,
    /// The newest commits `HEAD` has that the base does not, newest first, capped by the caller.
    pub commits: Vec<AheadCommit>,
    /// How many commits `HEAD` is ahead of the base in total.
    pub ahead: u64,
}
