use std::path::PathBuf;

/// Status of a path in either index or worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// No change on this side.
    Unmodified,
    /// Newly added.
    Added,
    /// Modified.
    Modified,
    /// Deleted.
    Deleted,
    /// Renamed.
    Renamed,
    /// Copied.
    Copied,
    /// File type changed.
    TypeChanged,
    /// Untracked by Git.
    Untracked,
    /// Ignored by Git.
    Ignored,
    /// Unmerged.
    Unmerged,
    /// An unrecognized porcelain status byte.
    Unknown(u8),
}

/// Classification of an unmerged path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both sides added the path.
    BothAdded,
    /// Both sides modified the path.
    BothModified,
    /// Both sides deleted the path.
    BothDeleted,
    /// Added by us, deleted by them.
    AddedByUs,
    /// Deleted by us, added by them.
    DeletedByUs,
    /// Deleted by us, modified by them.
    DeletedByUsModifiedByThem,
    /// Modified by us, deleted by them.
    ModifiedByUsDeletedByThem,
    /// Other unmerged state.
    Other,
}

/// One porcelain-v1 status record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    /// Current path.
    pub path: PathBuf,
    /// Source path for a rename or copy.
    pub previous_path: Option<PathBuf>,
    /// Index-side state.
    pub index: ChangeKind,
    /// Worktree-side state.
    pub worktree: ChangeKind,
    /// Conflict classification, when unmerged.
    pub conflict: Option<ConflictKind>,
}
