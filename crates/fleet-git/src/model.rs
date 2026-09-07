//! Owned data models shared by the backend and UI.

mod diff;
mod mutation;
mod refs;
mod repository;
mod status;

pub use diff::{
    CommitFile, ConflictChoice, ConflictFile, ConflictSection, Diff, DiffFile, DiffKind, DiffLine,
    DiffSide, Hunk, HunkSelection, LineKind, LineRange, ModeChange, PatchAction, PatchSelection,
};
pub use mutation::{
    CommandKind, CommitOptions, FetchRequest, MergeOptions, MutationResult, PullRequest,
    PushRequest, ResetMode, StashOptions,
};
pub use refs::{
    Branch, Commit, ObjectId, Ref, ReflogEntry, Remote, RemoteBranch, RemoteBranchGroup,
    StashEntry, Tag, Upstream,
};
pub use repository::{ChangeEvent, Head, OperationState, RepoPaths, RepoSnapshot, SnapshotOptions};
pub use status::{ChangeKind, ConflictKind, FileStatus};
