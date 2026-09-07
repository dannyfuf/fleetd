use std::path::PathBuf;

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
