use super::{
    Branch, Commit, FileStatus, ObjectId, ReflogEntry, Remote, RemoteBranchGroup, StashEntry, Tag,
};
use std::{path::PathBuf, time::Duration};

/// Resolved filesystem locations for a linked worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoPaths {
    /// Root of the checked-out worktree.
    pub worktree_root: PathBuf,
    /// Per-worktree Git directory (the target of a `.git` indirection file).
    pub git_dir: PathBuf,
    /// Shared Git directory containing refs and objects.
    pub common_dir: PathBuf,
}

/// Options controlling the bounded initial snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotOptions {
    /// Maximum commits to load.
    pub commit_limit: usize,
    /// Maximum reflog records to load.
    pub reflog_limit: usize,
    /// Whether tag refs are loaded.
    pub include_tags: bool,
    /// Whether remotes and remote branches are loaded.
    pub include_remotes: bool,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            commit_limit: 300,
            reflog_limit: 100,
            include_tags: true,
            include_remotes: true,
        }
    }
}

/// Current HEAD identity and its short commit description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// HEAD points at a local branch.
    Branch {
        /// Short branch name.
        name: String,
        /// Commit identifier, absent in an unborn repository.
        oid: Option<ObjectId>,
        /// One-line commit description.
        description: String,
    },
    /// HEAD points directly at a commit.
    Detached {
        /// Current commit identifier.
        oid: ObjectId,
        /// One-line commit description.
        description: String,
    },
    /// The repository has no commits yet.
    Unborn {
        /// Intended branch name.
        name: String,
    },
}

/// An in-progress repository operation detected from Git sentinel files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationState {
    /// No operation is in progress.
    None,
    /// A merge is in progress.
    Merging,
    /// A rebase is in progress.
    Rebasing {
        /// Whether the merge-style interactive rebase directory is present.
        interactive: bool,
        /// Requested target, when recorded by Git.
        onto: Option<String>,
        /// Original branch/ref, when recorded by Git.
        head_name: Option<String>,
        /// Number of completed todo steps.
        done: Option<usize>,
        /// Total number of todo steps.
        total: Option<usize>,
    },
    /// A cherry-pick is in progress.
    CherryPicking,
    /// A revert is in progress.
    Reverting,
    /// A bisect is in progress.
    Bisecting,
}

/// Complete bounded repository refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSnapshot {
    /// Worktree root.
    pub root: PathBuf,
    /// Current HEAD.
    pub head: Head,
    /// Current operation state.
    pub operation: OperationState,
    /// Changed paths.
    pub files: Vec<FileStatus>,
    /// Local branches.
    pub local_branches: Vec<Branch>,
    /// Remote branches grouped per remote.
    pub remote_branches: Vec<RemoteBranchGroup>,
    /// Configured remotes.
    pub remotes: Vec<Remote>,
    /// Tags.
    pub tags: Vec<Tag>,
    /// Recent commits.
    pub commits: Vec<Commit>,
    /// Recent HEAD reflog entries.
    pub reflog: Vec<ReflogEntry>,
    /// Stashes.
    pub stashes: Vec<StashEntry>,
    /// Monotonic snapshot generation for this `Repository` instance.
    pub generation: u64,
}

/// Coarse filesystem change emitted by the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeEvent {
    /// Paths retained after ignore filtering; repository roots stand for a full
    /// invalidation when a burst exceeds the watcher’s path budget.
    pub paths: Vec<PathBuf>,
    /// Time spent coalescing the event burst.
    pub debounce: Duration,
}
