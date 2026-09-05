# `fleet-git` public API

Complete listing of every public type, field, and function in `fleet-git` 0.1.
Everything reachable from the crate root is listed here, so a UI can be written
against this file without reading the crate.

Conventions used throughout:

- `Result<T>` is `fleet_git::Result<T>` = `std::result::Result<T, GitError>`.
- Every method whose receiver is `&self` on `Repository` is cancel-safe in the
  sense that dropping the future kills the child process (`kill_on_drop`).
- All mutations serialize on one per-repository async mutex.
- Byte-typed fields (`Vec<u8>`, `PathBuf`) preserve Git's exact bytes; nothing
  is lossily decoded unless the type says `String`.

---

## Crate root re-exports

```rust
pub mod parse;              // byte-level parsers (see below)
pub mod watch;              // filesystem watcher
pub mod sequence_editor;    // GIT_SEQUENCE_EDITOR callback

pub use command::{GitCommand, GitOutput, Runner};
pub use command_log::{CommandEvent, CommandOutcome, CommandRecord};
pub use error::{GitError, Result};
pub use model::*;           // every type in the "Data model" section below
pub use rebase::{FixupFlag, MoveDirection, RebaseAction, RebaseBase, RebasePlan, TodoEdit};
pub use repository::Repository;
```

---

## Errors

```rust
pub type Result<T> = std::result::Result<T, GitError>;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// Git could not be started or communicated with.
    Spawn { argv: Vec<String>, source: std::io::Error },
    /// Git exited unsuccessfully.
    Exit {
        status: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        argv: Vec<String>,
        message: String,
    },
    /// Machine-readable Git output or an instruction could not be parsed.
    Parse { context: &'static str, message: String },
    /// The supplied path does not belong to a non-bare working tree.
    NotARepository(std::path::PathBuf),
    /// A process exceeded its configured deadline.
    Timeout { argv: Vec<String>, timeout: std::time::Duration },
    /// Git stopped after producing merge conflicts.
    Conflict {
        status: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        argv: Vec<String>,
        message: String,
    },
}
```

`argv` is always redacted: URL userinfo is replaced with `[REDACTED]`.
`Conflict` is only ever produced for commands that can stop on conflicts
(`merge`, `rebase`, `cherry-pick`, `revert`, `pull`, `stash apply/pop/branch`
and the `--continue` / `--skip` variants); every other failure is `Exit`.

---

## Process runner

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCommand {
    pub argv: Vec<std::ffi::OsString>,
    pub cwd: std::path::PathBuf,
    pub env: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    pub timeout: Option<std::time::Duration>,
    pub kill_on_drop: bool,
    pub stdin: Option<Vec<u8>>,
    pub kind: CommandKind,
    pub literal_pathspecs: bool,
    pub background_read: bool,
    pub accepted_exit_codes: Vec<i32>,
    pub may_conflict: bool,
}

impl GitCommand {
    pub fn new(cwd: impl Into<PathBuf>, kind: CommandKind) -> Self;
    pub fn arg(self, value: impl Into<OsString>) -> Self;
    pub fn args(self, values: impl IntoIterator<Item = impl Into<OsString>>) -> Self;
    pub fn env(self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self;
    pub fn timeout(self, timeout: Duration) -> Self;
    pub fn stdin(self, stdin: impl Into<Vec<u8>>) -> Self;
    pub fn literal_pathspecs(self) -> Self;   // sets GIT_LITERAL_PATHSPECS=1
    pub fn foreground_read(self) -> Self;     // clears GIT_OPTIONAL_LOCKS=0
    pub fn accept_exit_code(self, status: i32) -> Self;
    pub fn may_conflict(self) -> Self;
}

#[derive(Debug)]
pub struct GitOutput {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: std::time::Duration,
    pub record: CommandRecord,
}

#[derive(Debug, Clone)]
pub struct Runner { /* Arc-shared */ }

impl Default for Runner { fn default() -> Self; }   // Runner::new(256, 256)

impl Runner {
    pub fn new(recent_capacity: usize, event_capacity: usize) -> Self;
    pub fn with_git_program(program: impl Into<OsString>) -> Self;
    pub fn env(self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self;
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<CommandEvent>;
    pub fn recent(&self) -> Vec<CommandRecord>;
    pub async fn run(&self, specification: GitCommand) -> Result<GitOutput>;
}
```

Every child always gets `GIT_TERMINAL_PROMPT=0` and `LC_ALL=C`. Reads default to
`GIT_OPTIONAL_LOCKS=0`. Default timeout is 60 s; default stdout/stderr preview
cap in the log is 16 KiB.

`Runner::env` and `Runner::with_git_program` are builder-only and must be called
before the runner is cloned or shared.

---

## Command log

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Running,
    Success { status: Option<i32>, stdout_preview: Vec<u8>, stderr_preview: Vec<u8> },
    Failed  { status: Option<i32>, stdout_preview: Vec<u8>, stderr_preview: Vec<u8> },
    TimedOut,
    SpawnFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRecord {
    pub id: u64,
    pub kind: CommandKind,
    pub display_argv: Vec<String>,   // redacted, argv[0] == "git"
    pub started_at: std::time::SystemTime,
    pub elapsed: Option<std::time::Duration>,
    pub outcome: CommandOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandEvent {
    Started(CommandRecord),
    Finished(CommandRecord),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind { Read, Mutation, Network }
```

---

## Repository

```rust
#[derive(Debug)]
pub struct Repository { /* opaque */ }

impl Repository {
    pub async fn discover(path: impl AsRef<Path>) -> Result<Self>;
    pub async fn discover_with_runner(path: impl AsRef<Path>, runner: Arc<Runner>) -> Result<Self>;
    pub fn paths(&self) -> &RepoPaths;
    pub fn runner(&self) -> &Arc<Runner>;
    pub fn subscribe_commands(&self) -> tokio::sync::broadcast::Receiver<CommandEvent>;
    pub fn recent_commands(&self) -> Vec<CommandRecord>;
}
```

`discover` follows a `.git` file indirection, so a linked worktree yields a
`git_dir` distinct from its `common_dir`. A bare repository or a
non-repository path yields `GitError::NotARepository`.

### Reads

```rust
pub async fn snapshot(&self, options: SnapshotOptions) -> Result<RepoSnapshot>;
pub async fn diff_file(&self, path: &Path, side: DiffSide) -> Result<Diff>;
pub async fn diff_paths(&self, paths: &[PathBuf], side: DiffSide) -> Result<Diff>;
pub async fn diff_commit(&self, oid: &ObjectId, paths: &[PathBuf]) -> Result<Diff>;
pub async fn diff_stash(&self, index: usize) -> Result<Diff>;
pub async fn diff_range(&self, from: &Ref, to: &Ref) -> Result<Diff>;
pub async fn diff_branch(&self, name: &Ref) -> Result<Diff>;
pub async fn commit_files(&self, oid: &ObjectId) -> Result<Vec<CommitFile>>;
pub async fn commits_for_ref(
    &self,
    reference: &Ref,
    upstream: Option<&Ref>,
    limit: usize,
) -> Result<Vec<Commit>>;
pub async fn remotes(&self) -> Result<Vec<Remote>>;
pub async fn show_commit_message(&self, oid: &ObjectId) -> Result<String>;
pub async fn file_at_ref(&self, reference: &Ref, path: &Path) -> Result<Vec<u8>>;
pub async fn conflicted_file(&self, path: &Path) -> Result<ConflictFile>;
```

- `snapshot` runs independent reads concurrently and bumps `generation`.
- `diff_file` renders an untracked path as an all-added diff.
- `diff_paths` is the same read over several literal paths at once — what a
  file-tree directory row shows. Tracked paths are one `git diff`; each
  untracked path is appended as its own all-added `--no-index` patch, so the
  returned `files` list is every tracked file first, then the untracked ones.
  An empty `paths` slice yields an empty diff and runs no command.
- `diff_branch` diffs `merge-base(HEAD, name)` against `name`.
- `diff_commit` with an empty `paths` slice diffs the whole commit.
- **Every one of the six `diff_*` reads passes `-U<n>`**, where `n` is
  `diff_context()` — `DEFAULT_DIFF_CONTEXT` (3, git's own default) until a
  caller changes it.

```rust
pub const DEFAULT_DIFF_CONTEXT: u32 = 3;
pub const MAX_DIFF_CONTEXT: u32 = 200;

pub fn diff_context(&self) -> u32;
pub fn set_diff_context(&self, lines: u32);   // clamped to 0..=MAX_DIFF_CONTEXT
```

  The width is carried on the repository rather than threaded through the six
  signatures on purpose: a patch built from a *displayed* hunk
  (`apply_patch_selection`) is validated against a diff read again at apply
  time, and those two reads must agree on `-U` or the hunk indices no longer
  line up. One shared value is the only way that cannot drift. `set_diff_context`
  takes `&self` (an `AtomicU32`), so a `Repository` behind an `Arc` can be
  retuned without a lock.
- `commits_for_ref` is `git log <reference>`, newest first, for a sub-commits
  view. `upstream` decides `Commit::pushed` — every commit in
  `<upstream>..<reference>` is unpushed — and `None` leaves the flag `false`
  everywhere. An unresolvable `reference` yields an empty list rather than an
  error.
- `snapshot` orders `local_branches` the way lazygit does: the checked-out
  branch first, then every branch the HEAD reflog records a checkout **from**
  (most recent first), then the rest in `-committerdate` order. The reflog it
  already loaded supplies this, so the ordering costs no extra `git` call, and
  a branch it matched carries `Branch::checked_out_at`.

### Index and worktree mutations

```rust
pub async fn stage_paths(&self, paths: &[PathBuf]) -> Result<MutationResult>;
pub async fn unstage_paths(&self, paths: &[PathBuf]) -> Result<MutationResult>;
pub async fn stage_all(&self) -> Result<MutationResult>;
pub async fn unstage_all(&self) -> Result<MutationResult>;
pub async fn discard_paths(&self, paths: &[PathBuf]) -> Result<MutationResult>;
pub async fn discard_all(&self) -> Result<MutationResult>;
pub async fn apply_patch_selection(
    &self,
    selection: PatchSelection,
    action: PatchAction,
) -> Result<MutationResult>;
pub async fn commit(&self, message: &str, options: CommitOptions) -> Result<MutationResult>;
```

`apply_patch_selection` accepts only these side/action pairs; anything else is
`GitError::Parse`:

| `side`               | `action`   | Git invocation                  |
| -------------------- | ---------- | ------------------------------- |
| `DiffSide::Unstaged` | `Stage`    | `git apply --cached`            |
| `DiffSide::Unstaged` | `Discard`  | `git apply --reverse`           |
| `DiffSide::Staged`   | `Unstage`  | `git apply --cached --reverse`  |

`commit` with an **empty** `message` means "keep the message the commit already
has": it runs `git commit --no-edit` rather than `-m ""`, which Git rejects. That
is the amend-in-place shape, so `commit("", CommitOptions { amend: true, .. })`
rewrites `HEAD` with the current index and its original message.

### Refs

```rust
pub async fn checkout(&self, reference: &Ref) -> Result<MutationResult>;
pub async fn checkout_new_branch(&self, name: &str, start_point: Option<&Ref>) -> Result<MutationResult>;
pub async fn create_branch(&self, name: &str, start_point: Option<&Ref>) -> Result<MutationResult>;
pub async fn delete_branch(&self, name: &str, force: bool) -> Result<MutationResult>;
pub async fn rename_branch(&self, old: &str, new: &str) -> Result<MutationResult>;
pub async fn merge(&self, name: &Ref, options: MergeOptions) -> Result<MutationResult>;
pub async fn merge_continue(&self) -> Result<MutationResult>;
pub async fn merge_abort(&self) -> Result<MutationResult>;
pub async fn rebase_onto(&self, name: &Ref) -> Result<MutationResult>;
pub async fn rebase_continue(&self) -> Result<MutationResult>;
pub async fn rebase_abort(&self) -> Result<MutationResult>;
pub async fn rebase_skip(&self) -> Result<MutationResult>;
pub async fn cherry_pick(&self, oids: &[ObjectId]) -> Result<MutationResult>;
pub async fn cherry_pick_continue(&self) -> Result<MutationResult>;
pub async fn cherry_pick_abort(&self) -> Result<MutationResult>;
pub async fn revert(&self, oid: &ObjectId) -> Result<MutationResult>;
pub async fn reset(&self, to: &Ref, mode: ResetMode) -> Result<MutationResult>;
pub async fn create_tag(&self, name: &str, target: &Ref, message: Option<&str>) -> Result<MutationResult>;
pub async fn delete_tag(&self, name: &str) -> Result<MutationResult>;
pub async fn checkout_remote_branch(&self, remote: &str, name: &str) -> Result<MutationResult>;
pub async fn set_upstream(&self, branch: &str, remote: &str, remote_branch: &str) -> Result<MutationResult>;
pub async fn unset_upstream(&self, branch: &str) -> Result<MutationResult>;
pub async fn resolve_conflict(&self, path: &Path, choice: ConflictChoice) -> Result<MutationResult>;
```

`create_tag` with `Some(message)` creates an annotated tag, with `None` a
lightweight one.

`rebase_onto` passes `--autostash`, like lazygit, so a dirty worktree does not
stop the rebase before it starts.

### Stash

```rust
pub async fn stash_push(&self, options: StashOptions) -> Result<MutationResult>;
pub async fn stash_apply(&self, index: usize) -> Result<MutationResult>;
pub async fn stash_pop(&self, index: usize) -> Result<MutationResult>;
pub async fn stash_drop(&self, index: usize) -> Result<MutationResult>;
pub async fn stash_branch(&self, name: &str, index: usize) -> Result<MutationResult>;
```

### Network

```rust
pub async fn fetch(&self, request: FetchRequest) -> Result<MutationResult>;
pub async fn pull(&self, request: PullRequest) -> Result<MutationResult>;
pub async fn push(&self, request: PushRequest) -> Result<MutationResult>;
```

### Interactive rebase

```rust
pub async fn interactive_rebase(&self, plan: RebasePlan, helper_exe: &Path) -> Result<MutationResult>;
pub async fn squash_into_previous(&self, oid: &ObjectId, helper_exe: &Path) -> Result<MutationResult>;
pub async fn fixup_into_previous(&self, oid: &ObjectId, helper_exe: &Path) -> Result<MutationResult>;
pub async fn drop_commit(&self, oid: &ObjectId, helper_exe: &Path) -> Result<MutationResult>;
pub async fn reword_commit(&self, oid: &ObjectId, new_message: &str, helper_exe: &Path) -> Result<MutationResult>;
pub async fn move_commit(&self, oid: &ObjectId, direction: MoveDirection, helper_exe: &Path) -> Result<MutationResult>;
pub async fn edit_commit(&self, oid: &ObjectId, helper_exe: &Path) -> Result<MutationResult>;
```

`helper_exe` is the path to an executable that calls
`sequence_editor::maybe_run_from_env()` first thing in `main` — either
`fleet-git-seqedit` (shipped by this crate) or the UI binary itself.

---

## Interactive-rebase plan types

```rust
pub const SEQUENCE_INSTRUCTION_ENV: &str = "FLEET_GIT_SEQUENCE_INSTRUCTION";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RebaseBase {
    Root,
    Reference(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FixupFlag { KeepMessage, EditMessage }   // `fixup -C` / `fixup -c`

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RebaseAction {
    Pick,
    Squash,
    Fixup { flag: Option<FixupFlag> },
    Reword,
    Edit,
    Drop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TodoEdit {
    /// Change one commit's action. The OID must match exactly one todo line.
    Change { oid: ObjectId, action: RebaseAction },
    /// Move todo lines by a signed offset in actionable-line positions
    /// (positive moves toward HEAD). Offsets are clamped to the todo bounds.
    Move { oids: Vec<ObjectId>, offset: isize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebasePlan {
    pub base: RebaseBase,
    pub onto: Option<String>,
    pub edits: Vec<TodoEdit>,
    pub autostash: bool,
    pub keep_empty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveDirection { Up, Down }   // Up == toward HEAD
```

### `fleet_git::sequence_editor`

```rust
pub const SEQUENCE_INSTRUCTION_ENV: &str;

/// Returns `None` when the process was not invoked as a sequence editor, so the
/// caller should proceed with its normal startup.
pub fn maybe_run_from_env() -> Option<std::process::ExitCode>;

/// Applies an encoded `RebasePlan` to a todo file, replacing it atomically.
pub fn run_sequence_editor(todo_path: &Path, encoded_plan: &str) -> Result<()>;
```

Comment lines and blank lines in the todo file are preserved verbatim.

---

## Watcher

```rust
pub mod watch {
    #[derive(Debug)]
    pub struct RepoWatcher { /* opaque; aborts its task on drop */ }

    impl RepoWatcher {
        pub fn new(paths: &RepoPaths)
            -> Result<(Self, async_channel::Receiver<ChangeEvent>)>;
    }
}
```

Watches the worktree recursively plus the real and common Git directories,
debounces bursts by 150 ms, and drops `.git/objects/**` and `index.lock` churn.
Keep the returned `RepoWatcher` alive for as long as you read the receiver.

---

## Data model

```rust
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectId(pub String);
impl ObjectId { pub fn as_str(&self) -> &str; }
// plus Display, From<String>, From<&str>

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref(pub String);
// plus From<String>, From<&str>

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoPaths {
    pub worktree_root: PathBuf,
    pub git_dir: PathBuf,      // per-worktree
    pub common_dir: PathBuf,   // shared refs/objects
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotOptions {
    pub commit_limit: usize,     // default 300
    pub reflog_limit: usize,     // default 100
    pub include_tags: bool,      // default true
    pub include_remotes: bool,   // default true
}
impl Default for SnapshotOptions { .. }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch { name: String, oid: Option<ObjectId>, description: String },
    Detached { oid: ObjectId, description: String },
    Unborn { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationState {
    None,
    Merging,
    Rebasing {
        interactive: bool,
        onto: Option<String>,
        head_name: Option<String>,
        done: Option<usize>,
        total: Option<usize>,
    },
    CherryPicking,
    Reverting,
    Bisecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Unmodified, Added, Modified, Deleted, Renamed, Copied,
    TypeChanged, Untracked, Ignored, Unmerged, Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    BothAdded, BothModified, BothDeleted, AddedByUs, DeletedByUs,
    DeletedByUsModifiedByThem, ModifiedByUsDeletedByThem, Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    pub path: PathBuf,
    pub previous_path: Option<PathBuf>,   // rename/copy source
    pub index: ChangeKind,
    pub worktree: ChangeKind,
    pub conflict: Option<ConflictKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream { pub name: String, pub ahead: usize, pub behind: usize }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub oid: ObjectId,
    pub is_head: bool,
    pub upstream: Option<Upstream>,
    pub subject: String,
    pub committed_at: i64,        // Unix seconds
    pub checked_out_at: Option<i64>, // Unix seconds, from the HEAD reflog
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBranch {
    pub name: String,            // "origin/main"
    pub branch: String,          // "main"
    pub oid: ObjectId,
    pub subject: String,
    pub committed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBranchGroup { pub remote: String, pub branches: Vec<RemoteBranch> }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub fetch_url: Option<String>,
    pub push_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub oid: ObjectId,           // peeled commit
    pub created_at: i64,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub oid: ObjectId,
    pub parents: Vec<ObjectId>,
    pub author_name: String,
    pub author_email: String,
    pub authored_at: i64,
    pub committed_at: i64,
    pub subject: String,
    pub body: String,
    pub decorations: Vec<String>,   // e.g. ["HEAD -> main", "tag: v1"]
    pub pushed: bool,               // contained in the branch's upstream
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogEntry {
    pub oid: ObjectId,
    pub parents: Vec<ObjectId>,
    pub selector: String,        // "HEAD@{0}"
    pub subject: String,
    pub committed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    pub index: usize,
    pub oid: ObjectId,
    pub created_at: i64,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSnapshot {
    pub root: PathBuf,
    pub head: Head,
    pub operation: OperationState,
    pub files: Vec<FileStatus>,
    pub local_branches: Vec<Branch>,
    pub remote_branches: Vec<RemoteBranchGroup>,
    pub remotes: Vec<Remote>,
    pub tags: Vec<Tag>,
    pub commits: Vec<Commit>,
    pub reflog: Vec<ReflogEntry>,
    pub stashes: Vec<StashEntry>,
    pub generation: u64,
}
```

### Diffs

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSide { Unstaged, Staged }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind { Added, Deleted, Modified, Renamed, Copied, TypeChanged, Unmerged, Unknown }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeChange { pub old: Option<String>, pub new: Option<String> }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange { pub start: u32, pub count: u32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind { Context, Added, Removed, NoNewline, Other }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: Vec<u8>,        // without the +/-/space prefix and trailing LF
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old: LineRange,
    pub new: LineRange,
    pub header: Vec<u8>,         // text after the second range in the @@ line
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub old_path: Option<PathBuf>,   // None for an added file
    pub new_path: Option<PathBuf>,   // None for a deleted file
    pub kind: DiffKind,
    pub binary: bool,
    pub mode: Option<ModeChange>,
    pub headers: Vec<Vec<u8>>,       // raw header lines, verbatim
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diff { pub files: Vec<DiffFile> }
impl Diff { pub fn to_text_lossy(&self) -> String; }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    pub path: PathBuf,
    pub previous_path: Option<PathBuf>,
    pub kind: DiffKind,
}
```

### Patch selection

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkSelection {
    pub hunk_index: usize,           // index into DiffFile::hunks
    pub lines: Option<Vec<usize>>,   // indexes into Hunk::lines; None = whole hunk
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchSelection {
    pub path: PathBuf,
    pub side: DiffSide,
    pub hunks: Vec<HunkSelection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchAction { Stage, Unstage, Discard }
```

### Options and requests

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationResult {
    pub records: Vec<CommandRecord>,
    pub warning: Option<String>,     // notable output from a successful command
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CommitOptions { pub amend: bool, pub allow_empty: bool, pub signoff: bool, pub no_verify: bool }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MergeOptions { pub ff_only: bool, pub no_ff: bool, pub squash: bool }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode { Soft, Mixed, Hard }

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StashOptions {
    pub message: Option<String>,
    pub include_untracked: bool,
    pub staged_only: bool,
    pub keep_index: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FetchRequest { pub remote: Option<String>, pub prune: bool, pub all: bool }

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PullRequest {
    pub remote: Option<String>,
    pub branch: Option<String>,
    pub rebase: bool,
    pub ff_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushRequest {
    pub remote: Option<String>,
    pub branch: Option<String>,
    pub set_upstream: bool,
    pub force_with_lease: bool,
    pub force: bool,
    pub tags: bool,
}
```

### Conflicts

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice { Ours, Theirs, Both }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictSection {
    pub start: usize,                // byte offset of `<<<<<<<`
    pub end: usize,                  // byte offset just past `>>>>>>>` line
    pub ours: Vec<u8>,
    pub base: Option<Vec<u8>>,       // present for diff3 markers
    pub theirs: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictFile {
    pub path: PathBuf,
    pub content: Vec<u8>,
    pub conflicts: Vec<ConflictSection>,
}
```

### Watcher event

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeEvent {
    pub paths: Vec<PathBuf>,             // sorted and deduplicated
    pub debounce: std::time::Duration,
}
```

---

## Parsers (`fleet_git::parse`)

Byte-level parsers over Git's stable machine-readable formats. `Repository`
calls these internally; they are public so a UI can reuse them for output it
obtains itself. Each expects exactly the format documented in its module.

```rust
pub mod parse {
    pub mod status {
        /// `git status --porcelain=v1 -z`
        pub fn parse(input: &[u8]) -> Result<Vec<FileStatus>>;
    }
    pub mod refs {
        /// `for-each-ref` with %(HEAD)%00%(refname:short)%00%(upstream:short)%00
        /// %(upstream:track)%00%(subject)%00%(objectname)%00%(committerdate:unix)%00
        pub fn local_branches(input: &[u8]) -> Result<Vec<Branch>>;
        /// %(refname:short)%00%(objectname)%00%(subject)%00%(committerdate:unix)%00%(symref)%00
        pub fn remote_branches(input: &[u8]) -> Result<Vec<RemoteBranchGroup>>;
        /// %(refname:short)%00%(*objectname)%00%(creatordate:unix)%00%(subject)%00
        pub fn tags(input: &[u8]) -> Result<Vec<Tag>>;
    }
    pub mod commits {
        /// %x1e%H%x00%P%x00%aN%x00%ae%x00%at%x00%ct%x00%s%x00%b%x00%D%x00
        pub fn commits(input: &[u8]) -> Result<Vec<Commit>>;
        /// %x1e%H%x00%P%x00%gd%x00%gs%x00%ct%x00
        pub fn reflog(input: &[u8]) -> Result<Vec<ReflogEntry>>;
        /// %x1e%gd%x00%H%x00%ct%x00%gs%x00
        pub fn stashes(input: &[u8]) -> Result<Vec<StashEntry>>;
    }
    pub mod diff {
        /// A `--no-color --no-ext-diff --patch` unified patch.
        pub fn parse(input: &[u8]) -> Result<Diff>;
        /// Renders a parsed diff back into an applicable unified patch.
        pub fn render(diff: &Diff) -> Vec<u8>;
        /// `git diff-tree --name-status -z`
        pub fn commit_files(input: &[u8]) -> Result<Vec<CommitFile>>;
    }
}
```

---

## Binary

```
fleet-git-seqedit
```

A three-line executable whose `main` calls
`fleet_git::sequence_editor::maybe_run_from_env()`. Tests locate it with
`env!("CARGO_BIN_EXE_fleet-git-seqedit")`; a shipped application can pass its
own executable path instead.
