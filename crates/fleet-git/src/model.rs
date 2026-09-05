//! Owned data models shared by the backend and UI.

use std::{fmt, path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

use crate::command_log::CommandRecord;

/// Intended effect and scheduling priority of a Git command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// Non-mutating repository inspection.
    Read,
    /// Local repository or worktree mutation.
    Mutation,
    /// Remote network operation.
    Network,
}

/// A hexadecimal Git object identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjectId(pub String);

impl ObjectId {
    /// Returns the hexadecimal identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<String> for ObjectId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ObjectId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

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

/// Tracking relationship for a local branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    /// Short remote ref name.
    pub name: String,
    /// Commits local is ahead of upstream.
    pub ahead: usize,
    /// Commits local is behind upstream.
    pub behind: usize,
}

/// Local branch summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// Short branch name.
    pub name: String,
    /// Tip object.
    pub oid: ObjectId,
    /// Whether this is the checked-out branch.
    pub is_head: bool,
    /// Tracking relationship.
    pub upstream: Option<Upstream>,
    /// Tip subject.
    pub subject: String,
    /// Committer timestamp in Unix seconds.
    pub committed_at: i64,
    /// When the branch was last checked out, from the HEAD reflog, when it appears there.
    ///
    /// lazygit's "recency" column is checkout recency, not commit recency
    /// (`pkg/commands/git_commands/branch_loader.go`), so this is what the UI prints when it is
    /// set and `committed_at` is only the fallback.
    pub checked_out_at: Option<i64>,
}

/// Remote-tracking branch summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBranch {
    /// Full short name, such as `origin/main`.
    pub name: String,
    /// Name without the remote prefix.
    pub branch: String,
    /// Tip object.
    pub oid: ObjectId,
    /// Tip subject.
    pub subject: String,
    /// Committer timestamp in Unix seconds.
    pub committed_at: i64,
}

/// Remote branches grouped by remote name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBranchGroup {
    /// Remote name.
    pub remote: String,
    /// Branches belonging to the remote.
    pub branches: Vec<RemoteBranch>,
}

/// A configured remote and its separate fetch/push URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// Remote name.
    pub name: String,
    /// Fetch URL, if configured.
    pub fetch_url: Option<String>,
    /// Push URL, if configured.
    pub push_url: Option<String>,
}

/// Tag summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Tag name.
    pub name: String,
    /// Peeled commit or tagged object identifier.
    pub oid: ObjectId,
    /// Creator timestamp in Unix seconds, or zero when unavailable.
    pub created_at: i64,
    /// Tag or target subject.
    pub subject: String,
}

/// Commit or reflog summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Commit identifier.
    pub oid: ObjectId,
    /// Parent commit identifiers.
    pub parents: Vec<ObjectId>,
    /// Author display name.
    pub author_name: String,
    /// Author email address.
    pub author_email: String,
    /// Author timestamp in Unix seconds.
    pub authored_at: i64,
    /// Committer timestamp in Unix seconds.
    pub committed_at: i64,
    /// First line of the message.
    pub subject: String,
    /// Remaining message body.
    pub body: String,
    /// Ref decorations emitted by Git.
    pub decorations: Vec<String>,
    /// Whether the commit is contained in the branch's upstream. Always `false`
    /// when the checked-out branch has no upstream configured.
    pub pushed: bool,
}

/// Reflog record with selector and action text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogEntry {
    /// Commit identifier at this entry.
    pub oid: ObjectId,
    /// Parent identifiers.
    pub parents: Vec<ObjectId>,
    /// Reflog selector such as `HEAD@{0}`.
    pub selector: String,
    /// Reflog action/subject.
    pub subject: String,
    /// Committer timestamp in Unix seconds.
    pub committed_at: i64,
}

/// Stash-list record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    /// Zero-based stash index.
    pub index: usize,
    /// Stash commit object.
    pub oid: ObjectId,
    /// Creation timestamp in Unix seconds.
    pub created_at: i64,
    /// Stash subject.
    pub subject: String,
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

/// Side of the index used for a working-tree diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSide {
    /// Index versus worktree.
    Unstaged,
    /// HEAD versus index.
    Staged,
}

/// File-level diff classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// Added path.
    Added,
    /// Deleted path.
    Deleted,
    /// Modified content.
    Modified,
    /// Renamed path.
    Renamed,
    /// Copied path.
    Copied,
    /// File type changed.
    TypeChanged,
    /// Unmerged path.
    Unmerged,
    /// Other or unknown change.
    Unknown,
}

/// Old and new file mode strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeChange {
    /// Previous mode.
    pub old: Option<String>,
    /// New mode.
    pub new: Option<String>,
}

/// Inclusive-start, counted line range in a unified hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    /// One-based start line, or zero for an empty side.
    pub start: u32,
    /// Number of lines.
    pub count: u32,
}

/// Kind of one unified-diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Unchanged context.
    Context,
    /// Added content.
    Added,
    /// Removed content.
    Removed,
    /// `\ No newline at end of file` marker.
    NoNewline,
    /// An unrecognized hunk line retained verbatim.
    Other,
}

/// One line inside a parsed hunk; `content` excludes the prefix and trailing LF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// Semantic line kind.
    pub kind: LineKind,
    /// Raw content bytes.
    pub content: Vec<u8>,
    /// Old-side line number.
    pub old_no: Option<u32>,
    /// New-side line number.
    pub new_no: Option<u32>,
}

/// Parsed unified-diff hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Old-side range.
    pub old: LineRange,
    /// New-side range.
    pub new: LineRange,
    /// Raw text following the two ranges in the `@@` line.
    pub header: Vec<u8>,
    /// Parsed lines.
    pub lines: Vec<DiffLine>,
}

/// One file in a parsed patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    /// Old path, without the `a/` prefix.
    pub old_path: Option<PathBuf>,
    /// New path, without the `b/` prefix.
    pub new_path: Option<PathBuf>,
    /// File-level change kind.
    pub kind: DiffKind,
    /// Whether Git reports a binary patch.
    pub binary: bool,
    /// Optional mode transition.
    pub mode: Option<ModeChange>,
    /// Raw file header lines needed to reconstruct an applicable patch.
    pub headers: Vec<Vec<u8>>,
    /// Parsed hunks.
    pub hunks: Vec<Hunk>,
}

/// Parsed patch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diff {
    /// Files in command order.
    pub files: Vec<DiffFile>,
}

impl Diff {
    /// Reconstructs the parsed patch as lossy UTF-8 for display.
    #[must_use]
    pub fn to_text_lossy(&self) -> String {
        String::from_utf8_lossy(&crate::parse::diff::render(self)).into_owned()
    }
}

/// Lightweight file summary returned for a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    /// Current path.
    pub path: PathBuf,
    /// Previous path for rename/copy.
    pub previous_path: Option<PathBuf>,
    /// File change kind.
    pub kind: DiffKind,
}

/// Selected hunks and optional line indexes for one hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkSelection {
    /// Zero-based hunk index.
    pub hunk_index: usize,
    /// Zero-based changed-line indexes within the hunk; `None` selects the whole hunk.
    pub lines: Option<Vec<usize>>,
}

/// Partial-patch request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchSelection {
    /// Repository-relative path.
    pub path: PathBuf,
    /// Side whose diff is selected.
    pub side: DiffSide,
    /// Hunk selections.
    pub hunks: Vec<HunkSelection>,
}

/// Operation applied to a selected patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchAction {
    /// Add worktree changes to the index.
    Stage,
    /// Remove index changes while preserving the worktree.
    Unstage,
    /// Reverse worktree changes.
    Discard,
}

/// Records generated by a serialized mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationResult {
    /// Finished command records belonging to the mutation.
    pub records: Vec<CommandRecord>,
    /// Successful but notable Git output.
    pub warning: Option<String>,
}

/// Commit command flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CommitOptions {
    /// Amend HEAD.
    pub amend: bool,
    /// Permit an empty tree change.
    pub allow_empty: bool,
    /// Add a Signed-off-by trailer.
    pub signoff: bool,
    /// Skip commit hooks.
    pub no_verify: bool,
}

/// An explicitly typed checkout target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref(pub String);

impl From<String> for Ref {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Ref {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Merge flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MergeOptions {
    /// Require a fast-forward.
    pub ff_only: bool,
    /// Force creation of a merge commit.
    pub no_ff: bool,
    /// Apply changes without committing.
    pub squash: bool,
}

/// Reset mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    /// Move HEAD only.
    Soft,
    /// Reset HEAD and index.
    Mixed,
    /// Reset HEAD, index, and worktree.
    Hard,
}

/// Stash creation flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StashOptions {
    /// Optional stash message.
    pub message: Option<String>,
    /// Include untracked paths.
    pub include_untracked: bool,
    /// Stash only staged changes.
    pub staged_only: bool,
    /// Keep index entries staged.
    pub keep_index: bool,
}

/// Fetch request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FetchRequest {
    /// One remote; absent means the configured default.
    pub remote: Option<String>,
    /// Delete stale remote-tracking refs.
    pub prune: bool,
    /// Fetch all remotes.
    pub all: bool,
}

/// Pull request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PullRequest {
    /// Optional remote.
    pub remote: Option<String>,
    /// Optional remote branch.
    pub branch: Option<String>,
    /// Rebase local commits.
    pub rebase: bool,
    /// Require a fast-forward.
    pub ff_only: bool,
}

/// Push request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushRequest {
    /// Optional remote.
    pub remote: Option<String>,
    /// Optional source branch.
    pub branch: Option<String>,
    /// Set upstream tracking.
    pub set_upstream: bool,
    /// Safely force only if the remote ref is unchanged.
    pub force_with_lease: bool,
    /// Unconditionally force.
    pub force: bool,
    /// Push tags in addition to the ref.
    pub tags: bool,
}

/// User-facing conflict resolution choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Keep our side.
    Ours,
    /// Keep their side.
    Theirs,
    /// Concatenate ours followed by theirs.
    Both,
}

/// One conflict-marker region in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictSection {
    /// Byte offset where the opening marker begins.
    pub start: usize,
    /// Byte offset just after the closing marker line.
    pub end: usize,
    /// Content from our side.
    pub ours: Vec<u8>,
    /// Diff3 base content, when present.
    pub base: Option<Vec<u8>>,
    /// Content from their side.
    pub theirs: Vec<u8>,
}

/// Raw conflicted file plus parsed marker regions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictFile {
    /// Repository-relative path.
    pub path: PathBuf,
    /// Complete bytes currently in the worktree.
    pub content: Vec<u8>,
    /// Marker regions in byte order.
    pub conflicts: Vec<ConflictSection>,
}

/// Coarse filesystem change emitted by the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeEvent {
    /// Paths retained after ignore filtering.
    pub paths: Vec<PathBuf>,
    /// Time spent coalescing the event burst.
    pub debounce: Duration,
}
