//! The central UI state and its reducers.
//!
//! [`GitUiState`] holds the last good [`RepoSnapshot`] behind an `Arc`, every panel's cursor, the
//! overlay stack, the command log and the transient feedback. Everything that decides *what the
//! next keystroke does* lives here as a plain `&mut self` reducer or a free function with no gpui
//! and no I/O, so it is unit testable; the views read the result and never derive it twice.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use fleet_git::{
    Branch, ChangeKind, Commit, CommitFile, ConflictFile, Diff, DiffSide, FileStatus, Head,
    HunkSelection, ObjectId, OperationState, ReflogEntry, Remote, RemoteBranch, RepoSnapshot,
    StashEntry, StashOptions, Tag,
};
use fleet_ui_kit::{ListCursor, Toast};

use crate::bridge::GitRequest;
use crate::views::diff_model::{DiffRow, DiffViewMode};
use crate::views::file_tree::{FileRow, FileTree};

/// How many command-log records to keep.
pub const COMMAND_LOG_CAP: usize = 400;

/// Which panel owns the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PanelId {
    /// Panel 1: repository status.
    #[default]
    Status,
    /// Panel 2: working-tree files.
    Files,
    /// Panel 3: branches, remotes and tags.
    Branches,
    /// Panel 4: commits and the reflog.
    Commits,
    /// Panel 5: stash entries.
    Stash,
    /// The main panel on the right.
    Main,
}

impl PanelId {
    /// The five side panels, in the order `<tab>` cycles them.
    pub const SIDE: [PanelId; 5] = [
        PanelId::Status,
        PanelId::Files,
        PanelId::Branches,
        PanelId::Commits,
        PanelId::Stash,
    ];

    /// The panel's title as lazygit spells it.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            PanelId::Status => "Status",
            PanelId::Files => "Files",
            PanelId::Branches => "Local branches",
            PanelId::Commits => "Commits",
            PanelId::Stash => "Stash",
            PanelId::Main => "Diff",
        }
    }

    /// The `[n]` jump label lazygit prints in the pane title.
    #[must_use]
    pub fn jump_label(self) -> &'static str {
        match self {
            PanelId::Status => "1",
            PanelId::Files => "2",
            PanelId::Branches => "3",
            PanelId::Commits => "4",
            PanelId::Stash => "5",
            PanelId::Main => "0",
        }
    }

    /// The next side panel, wrapping.
    #[must_use]
    pub fn next_side(self) -> PanelId {
        let index = Self::SIDE.iter().position(|panel| *panel == self);
        match index {
            Some(index) => Self::SIDE[(index + 1) % Self::SIDE.len()],
            None => PanelId::Files,
        }
    }

    /// The previous side panel, wrapping.
    #[must_use]
    pub fn prev_side(self) -> PanelId {
        let index = Self::SIDE.iter().position(|panel| *panel == self);
        match index {
            Some(index) => Self::SIDE[(index + Self::SIDE.len() - 1) % Self::SIDE.len()],
            None => PanelId::Files,
        }
    }
}

/// lazygit's `+` / `_` screen modes. The cycle wraps, matching `screen_mode_actions.go`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScreenMode {
    /// Every panel visible.
    #[default]
    Normal,
    /// The focused side panel and the main panel share the window.
    Half,
    /// Only the focused panel.
    Full,
}

impl ScreenMode {
    /// The next mode, wrapping.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Normal => Self::Half,
            Self::Half => Self::Full,
            Self::Full => Self::Normal,
        }
    }

    /// The previous mode, wrapping.
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            Self::Normal => Self::Full,
            Self::Half => Self::Normal,
            Self::Full => Self::Half,
        }
    }
}

/// The tab strip of panel 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BranchTab {
    /// Local branches.
    #[default]
    Local,
    /// Remotes, and their branches once entered.
    Remotes,
    /// Tags.
    Tags,
}

/// The tab strip of panel 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CommitTab {
    /// The commit log.
    #[default]
    Commits,
    /// The reflog.
    Reflog,
}

/// What the main panel is showing.
#[derive(Clone, Debug, Default)]
pub enum MainContent {
    /// The Status panel is focused: repository summary.
    #[default]
    Summary,
    /// A working-tree file, both sides.
    FileDiff {
        /// The file.
        path: PathBuf,
        /// Worktree vs index.
        unstaged: Option<Arc<Diff>>,
        /// Index vs HEAD.
        staged: Option<Arc<Diff>>,
    },
    /// A commit's diff and file list.
    CommitDiff {
        /// The commit.
        oid: ObjectId,
        /// Its diff.
        diff: Option<Arc<Diff>>,
        /// Its changed files.
        files: Vec<CommitFile>,
    },
    /// A branch's diff against its merge base with HEAD.
    BranchDiff {
        /// The branch.
        name: String,
        /// Its diff.
        diff: Option<Arc<Diff>>,
    },
    /// A stash entry's diff.
    StashDiff {
        /// Stash index.
        index: usize,
        /// Its diff.
        diff: Option<Arc<Diff>>,
    },
    /// A ref's commits — lazygit's sub-commits view, drilled into from a branch.
    SubCommits {
        /// The ref whose log this is.
        reference: String,
        /// Its commits, newest first.
        commits: Vec<Commit>,
        /// The sub-commit whose patch the lower half shows.
        shown: Option<ObjectId>,
        /// That patch, once read.
        diff: Option<Arc<Diff>>,
    },
    /// A commit's changed files — lazygit's commit-files view, drilled into from a commit.
    CommitFiles {
        /// The commit.
        oid: ObjectId,
        /// Its subject, for the header row.
        subject: String,
        /// Its changed files.
        files: Vec<CommitFile>,
        /// The whole-commit patch, shown while the header row is selected.
        whole: Option<Arc<Diff>>,
        /// The file whose patch the lower half shows; `None` is the header row.
        shown: Option<PathBuf>,
        /// That patch, once read.
        diff: Option<Arc<Diff>>,
    },
    /// A remote's URLs.
    RemoteInfo {
        /// Remote name.
        name: String,
    },
    /// A tag's target and subject.
    TagInfo {
        /// Tag name.
        name: String,
    },
    /// Conflict resolution for one path.
    Conflict {
        /// The conflicted path.
        path: PathBuf,
        /// The parsed file, once read.
        file: Option<Arc<ConflictFile>>,
        /// Which conflict section is selected.
        section: usize,
    },
    /// Nothing to show; the string is the reason.
    Empty(String),
}

/// Staging mode: a hunk/line selection over one side of one file's diff.
#[derive(Clone, Debug)]
pub struct Staging {
    /// The file being staged.
    pub path: PathBuf,
    /// Which side the selection applies to.
    pub side: DiffSide,
    /// The cursor row, an index into the flattened staging rows.
    pub cursor: usize,
    /// The anchor row of a range selection (`v`).
    pub anchor: Option<usize>,
    /// `false` selects whole hunks, `true` selects individual lines.
    pub line_mode: bool,
    /// Whether the cursor has already been placed on a change line.
    ///
    /// The periodic refresh re-reads the file diff, and the arriving diff used to re-snap the
    /// cursor to the first change every two seconds — which threw the selection away while the
    /// user was still moving it. Snapping is a one-shot per open / side flip / mode toggle.
    pub snapped: bool,
}

/// A single-line or multi-line text buffer with a character caret.
///
/// The caret is a **character** offset, never a byte offset, so multi-byte input cannot split a
/// grapheme in half.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Buffer {
    value: String,
    caret: usize,
    multiline: bool,
}

impl Buffer {
    /// An empty single-line buffer.
    #[must_use]
    pub fn single_line() -> Self {
        Self::default()
    }

    /// An empty buffer that accepts newlines.
    #[must_use]
    pub fn multi_line() -> Self {
        Self {
            multiline: true,
            ..Self::default()
        }
    }

    /// Pre-fills the buffer, caret at the end.
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.value = text.into();
        self.caret = self.value.chars().count();
        self
    }

    /// The buffer's contents.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The caret, as a character index.
    #[must_use]
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Whether the buffer accepts newlines.
    #[must_use]
    pub fn is_multiline(&self) -> bool {
        self.multiline
    }

    /// Whether the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    fn byte_of(&self, caret: usize) -> usize {
        self.value
            .char_indices()
            .nth(caret)
            .map_or(self.value.len(), |(index, _)| index)
    }

    /// Inserts text at the caret, dropping newlines in a single-line buffer.
    pub fn insert(&mut self, text: &str) {
        let filtered: String = text
            .chars()
            .filter(|character| {
                if *character == '\n' {
                    self.multiline
                } else {
                    !character.is_control()
                }
            })
            .collect();
        if filtered.is_empty() {
            return;
        }
        let at = self.byte_of(self.caret);
        self.value.insert_str(at, &filtered);
        self.caret += filtered.chars().count();
    }

    /// Deletes the character before the caret. Returns whether anything changed.
    pub fn backspace(&mut self) -> bool {
        if self.caret == 0 {
            return false;
        }
        let start = self.byte_of(self.caret - 1);
        let end = self.byte_of(self.caret);
        self.value.replace_range(start..end, "");
        self.caret -= 1;
        true
    }

    /// Deletes the word before the caret (`ctrl-w`).
    pub fn delete_word(&mut self) -> bool {
        let mut moved = false;
        while self.caret > 0 && self.char_before().is_some_and(char::is_whitespace) {
            moved |= self.backspace();
        }
        while self.caret > 0 && self.char_before().is_some_and(|c| !c.is_whitespace()) {
            moved |= self.backspace();
        }
        moved
    }

    /// Deletes from the start of the line to the caret (`ctrl-u`).
    pub fn delete_to_line_start(&mut self) -> bool {
        let mut moved = false;
        while self.caret > 0 && self.char_before() != Some('\n') {
            moved |= self.backspace();
        }
        moved
    }

    fn char_before(&self) -> Option<char> {
        if self.caret == 0 {
            return None;
        }
        self.value.chars().nth(self.caret - 1)
    }

    /// Moves the caret one character left.
    pub fn left(&mut self) {
        self.caret = self.caret.saturating_sub(1);
    }

    /// Moves the caret one character right.
    pub fn right(&mut self) {
        self.caret = (self.caret + 1).min(self.value.chars().count());
    }

    /// Moves the caret to the start of the buffer.
    pub fn home(&mut self) {
        self.caret = 0;
    }

    /// Moves the caret to the end of the buffer.
    pub fn end(&mut self) {
        self.caret = self.value.chars().count();
    }

    /// The lines of the buffer plus the caret's (line, column), for rendering.
    #[must_use]
    pub fn lines_with_caret(&self) -> (Vec<String>, usize, usize) {
        let mut lines: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut caret_line = 0;
        let mut caret_column = 0;
        for (index, character) in self.value.chars().enumerate() {
            if index == self.caret {
                caret_line = lines.len();
                caret_column = current.chars().count();
            }
            if character == '\n' {
                lines.push(std::mem::take(&mut current));
            } else {
                current.push(character);
            }
        }
        if self.caret >= self.value.chars().count() {
            caret_line = lines.len();
            caret_column = current.chars().count();
        }
        lines.push(current);
        (lines, caret_line, caret_column)
    }
}

/// What submitting a prompt does.
#[derive(Clone, Debug)]
pub enum PromptKind {
    /// Commit the index with the typed message.
    Commit {
        /// Amend the previous commit instead of creating one.
        amend: bool,
    },
    /// Create and check out a branch with the typed name.
    NewBranch {
        /// Start point, when creating from a commit.
        start_point: Option<String>,
    },
    /// Rename the named branch.
    RenameBranch(String),
    /// Create a tag at the given target.
    NewTag(String),
    /// Reword a commit.
    Reword(ObjectId),
    /// Set the named branch's upstream from a `remote/branch` string.
    SetUpstream(String),
    /// Stash with the typed message.
    Stash(StashOptions),
    /// Create a branch from a stash entry.
    BranchFromStash(usize),
    /// Push with `--set-upstream` to the typed remote.
    PushSetUpstream,
}

/// A prompt overlay.
#[derive(Clone, Debug)]
pub struct Prompt {
    /// The dialog title.
    pub title: String,
    /// A one-line explanation under the title.
    pub subtitle: Option<String>,
    /// The editable buffer.
    pub buffer: Buffer,
    /// What confirming does.
    pub kind: PromptKind,
}

/// What confirming a confirmation does.
#[derive(Clone, Debug)]
pub enum ConfirmOutcome {
    /// Send this request.
    Request(Box<GitRequest>),
    /// Send this request, and open the stronger dialog only if it fails.
    ///
    /// This is lazygit's delete escalation: `git branch -d` runs first and the "force?" dialog
    /// appears only when Git refuses because the branch is not merged.
    RequestOrEscalate {
        /// The safe attempt. Must be a [`GitRequest::Mutate`], whose label keys the follow-up.
        request: Box<GitRequest>,
        /// The dialog that opens when the safe attempt fails.
        escalate: Box<Confirm>,
    },
    /// Open a prompt.
    Prompt(Box<Prompt>),
}

/// A confirmation overlay.
#[derive(Clone, Debug)]
pub struct Confirm {
    /// The dialog title.
    pub title: String,
    /// The thing being acted on, shown on its own line.
    pub target: String,
    /// One line per consequence.
    pub facts: Vec<String>,
    /// Whether this is the destructive escalation.
    pub danger: bool,
    /// What confirming does.
    pub outcome: ConfirmOutcome,
}

/// What choosing a menu item does.
#[derive(Clone, Debug)]
pub enum MenuAction {
    /// Send this request.
    Request(Box<GitRequest>),
    /// Open a confirmation.
    Confirm(Box<Confirm>),
    /// Open a prompt.
    Prompt(Box<Prompt>),
}

/// One row of a menu.
#[derive(Clone, Debug)]
pub struct MenuItem {
    /// The shortcut shown on the right.
    pub key: String,
    /// The row label.
    pub label: String,
    /// What choosing it does.
    pub action: MenuAction,
}

/// A menu overlay: lazygit's option menus.
#[derive(Clone, Debug)]
pub struct Menu {
    /// The menu title.
    pub title: String,
    /// Every row, unfiltered.
    pub items: Vec<MenuItem>,
    /// The cursor into the *filtered* rows.
    pub cursor: usize,
    /// The filter buffer, present while `/` filtering is active.
    pub filter: Option<Buffer>,
}

impl Menu {
    /// A menu with a title and rows.
    #[must_use]
    pub fn new(title: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self {
            title: title.into(),
            items,
            cursor: 0,
            filter: None,
        }
    }

    /// The rows the filter admits, with their index into [`Menu::items`].
    #[must_use]
    pub fn visible(&self) -> Vec<(usize, &MenuItem)> {
        let query = self
            .filter
            .as_ref()
            .map(|buffer| buffer.value().to_lowercase())
            .unwrap_or_default();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                query.is_empty() || item.label.to_lowercase().contains(query.as_str())
            })
            .collect()
    }

    /// The selected row, if any.
    #[must_use]
    pub fn selected(&self) -> Option<&MenuItem> {
        let visible = self.visible();
        visible.get(self.cursor).map(|(_, item)| *item)
    }

    /// The row whose shortcut letter is `key`, as lazygit's menus dispatch them.
    ///
    /// The whole menu is searched, not just the filtered rows: the letter *is* the row's
    /// identity. Callers must only consult this while no filter prompt is open, because a
    /// printable key then belongs to the filter.
    #[must_use]
    pub fn item_for_key(&self, key: &str) -> Option<&MenuItem> {
        self.items.iter().find(|item| item.key == key)
    }
}

/// One entry of the overlay stack. The stack is LIFO; `<esc>` pops one layer.
#[derive(Clone, Debug)]
pub enum Overlay {
    /// A destructive confirmation.
    Confirm(Confirm),
    /// A text prompt.
    Prompt(Prompt),
    /// An option menu.
    Menu(Menu),
    /// The generated keybinding help.
    Help {
        /// Scroll offset in rows.
        top: usize,
    },
}

/// Every list cursor, one per list the UI renders.
#[derive(Clone, Debug)]
pub struct Cursors {
    /// Panel 2.
    pub files: ListCursor,
    /// Panel 3, Local tab.
    pub branches: ListCursor,
    /// Panel 3, Remotes tab, remote list.
    pub remotes: ListCursor,
    /// Panel 3, Remotes tab, branches of the entered remote.
    pub remote_branches: ListCursor,
    /// Panel 3, Tags tab.
    pub tags: ListCursor,
    /// Panel 4, Commits tab.
    pub commits: ListCursor,
    /// Panel 4, Reflog tab.
    pub reflog: ListCursor,
    /// Panel 5.
    pub stashes: ListCursor,
    /// The main panel's diff rows.
    pub main: ListCursor,
}

impl Default for Cursors {
    fn default() -> Self {
        Self {
            files: ListCursor::new(0),
            branches: ListCursor::new(0),
            remotes: ListCursor::new(0),
            remote_branches: ListCursor::new(0),
            tags: ListCursor::new(0),
            commits: ListCursor::new(0),
            reflog: ListCursor::new(0),
            stashes: ListCursor::new(0),
            main: ListCursor::new(0),
        }
    }
}

/// The whole UI state.
pub struct GitUiState {
    /// The path the app was launched with; replaced by the worktree root once discovered.
    pub root: PathBuf,
    /// The last good snapshot. `None` before the first load completes.
    pub snapshot: Option<Arc<RepoSnapshot>>,
    /// `generation` of the newest accepted snapshot. A snapshot arriving with a lower or equal
    /// generation is stale and is dropped.
    pub epoch: u64,
    /// A refresh is in flight.
    pub refreshing: bool,
    /// The repository could not be opened.
    pub fatal: Option<String>,
    /// The last failure, shown in the status bar until the next success.
    pub last_error: Option<String>,

    /// Screen mode (`+` / `_`).
    pub screen_mode: ScreenMode,
    /// Which panel owns the keyboard.
    pub focused: PanelId,
    /// Where focus returns to when the main panel is escaped.
    pub previous_panel: PanelId,
    /// Panel 3's tab.
    pub branch_tab: BranchTab,
    /// Panel 4's tab.
    pub commit_tab: CommitTab,
    /// The remote whose branches the Remotes tab is listing.
    pub remote_drill: Option<String>,
    /// Whether the command log band is visible (`@`).
    pub show_command_log: bool,

    /// Selections by identity, so a refresh follows the item rather than the index.
    pub sel_file: Option<PathBuf>,
    /// Selected local branch.
    pub sel_branch: Option<String>,
    /// Selected remote.
    pub sel_remote: Option<String>,
    /// Selected remote branch, fully qualified.
    pub sel_remote_branch: Option<String>,
    /// Selected tag.
    pub sel_tag: Option<String>,
    /// Selected commit.
    pub sel_commit: Option<ObjectId>,
    /// Selected reflog entry.
    pub sel_reflog: Option<String>,
    /// Selected stash index.
    pub sel_stash: Option<usize>,
    /// Every list cursor.
    pub cursors: Cursors,
    /// The Files pane's row model: lazygit's file tree, its collapse set and its flat/tree flag.
    pub file_tree: FileTree,

    /// What the main panel shows.
    pub main: MainContent,
    /// Horizontal scroll of the main panel, in pixels.
    ///
    /// Pixels rather than characters: the payload is shifted with a negative margin, exactly as
    /// Zed offsets its paint origin, so the clamp is a real content width instead of nothing.
    pub main_h_scroll: f32,
    /// Whether the diff renders unified or side by side.
    pub diff_mode: DiffViewMode,
    /// How many context lines every diff read asks git for (`{` / `}`).
    pub diff_context: u32,
    /// Staging mode, when the main panel is in it.
    pub staging: Option<Staging>,

    /// LIFO overlay stack; only the top layer receives keys.
    pub overlays: Vec<Overlay>,
    /// The dialog to open when the mutation carrying this label fails, armed by
    /// [`ConfirmOutcome::RequestOrEscalate`].
    pub escalation: Option<(String, Box<Confirm>)>,

    /// The command log, newest last.
    pub command_log: VecDeque<String>,
    /// Commits marked with `c` for a later `v` (cherry-pick).
    pub copied: Vec<ObjectId>,
    /// Live toasts.
    pub toasts: Vec<Toast>,
    /// When each toast expires.
    pub toast_deadlines: Vec<Instant>,
    /// How many mutations have been dispatched and not yet answered.
    ///
    /// This is the counter `q` asks about and the one that says "an operation is in progress";
    /// reads are counted separately in [`GitUiState::pending_reads`] so a failing diff can never
    /// clear it.
    pub pending: usize,
    /// How many reads have been dispatched and not yet answered.
    pub pending_reads: usize,
}

impl GitUiState {
    /// The state the app starts in: nothing loaded, Files focused, no overlay.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            snapshot: None,
            epoch: 0,
            refreshing: true,
            fatal: None,
            last_error: None,
            screen_mode: ScreenMode::Normal,
            focused: PanelId::Files,
            previous_panel: PanelId::Files,
            branch_tab: BranchTab::Local,
            commit_tab: CommitTab::Commits,
            remote_drill: None,
            show_command_log: true,
            sel_file: None,
            sel_branch: None,
            sel_remote: None,
            sel_remote_branch: None,
            sel_tag: None,
            sel_commit: None,
            sel_reflog: None,
            sel_stash: None,
            cursors: Cursors::default(),
            file_tree: FileTree::default(),
            main: MainContent::Summary,
            main_h_scroll: 0.0,
            diff_mode: DiffViewMode::default(),
            diff_context: fleet_git::DEFAULT_DIFF_CONTEXT,
            staging: None,
            overlays: Vec::new(),
            escalation: None,
            command_log: VecDeque::new(),
            copied: Vec::new(),
            toasts: Vec::new(),
            toast_deadlines: Vec::new(),
            pending: 0,
            pending_reads: 0,
        }
    }

    // ---------------------------------------------------------------- request bookkeeping

    /// One mutation was dispatched.
    pub fn begin_mutation(&mut self) {
        self.pending += 1;
    }

    /// One mutation was answered, successfully or not.
    pub fn finish_mutation(&mut self) {
        self.pending = self.pending.saturating_sub(1);
    }

    /// One read was dispatched.
    pub fn begin_read(&mut self) {
        self.pending_reads += 1;
    }

    /// One read was answered, successfully or not. Never touches the mutation counter.
    pub fn finish_read(&mut self) {
        self.pending_reads = self.pending_reads.saturating_sub(1);
    }

    /// Whether a mutation is still in flight. `q` asks first while this holds.
    #[must_use]
    pub fn mutation_in_flight(&self) -> bool {
        self.pending > 0
    }

    // ---------------------------------------------------------------- read model

    /// The working-tree files.
    #[must_use]
    pub fn files(&self) -> &[FileStatus] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.files)
    }

    /// The local branches.
    #[must_use]
    pub fn branches(&self) -> &[Branch] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.local_branches)
    }

    /// The configured remotes.
    #[must_use]
    pub fn remotes(&self) -> &[Remote] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.remotes)
    }

    /// The branches of the entered remote.
    #[must_use]
    pub fn remote_branches(&self) -> &[RemoteBranch] {
        let Some(remote) = self.remote_drill.as_deref() else {
            return &[];
        };
        self.snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .remote_branches
                    .iter()
                    .find(|group| group.remote == remote)
            })
            .map_or(&[][..], |group| &group.branches)
    }

    /// The tags.
    #[must_use]
    pub fn tags(&self) -> &[Tag] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.tags)
    }

    /// The commit log.
    #[must_use]
    pub fn commits(&self) -> &[Commit] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.commits)
    }

    /// The reflog.
    #[must_use]
    pub fn reflog(&self) -> &[ReflogEntry] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.reflog)
    }

    /// The stash entries.
    #[must_use]
    pub fn stashes(&self) -> &[StashEntry] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| &snapshot.stashes)
    }

    /// The current HEAD.
    #[must_use]
    pub fn head(&self) -> Option<&Head> {
        self.snapshot.as_ref().map(|snapshot| &snapshot.head)
    }

    /// The in-progress operation, if any.
    #[must_use]
    pub fn operation(&self) -> OperationState {
        self.snapshot
            .as_ref()
            .map_or(OperationState::None, |snapshot| snapshot.operation.clone())
    }

    /// The branch HEAD points at, when it is a branch.
    #[must_use]
    pub fn head_branch(&self) -> Option<&str> {
        match self.head() {
            Some(Head::Branch { name, .. }) | Some(Head::Unborn { name }) => Some(name.as_str()),
            _ => None,
        }
    }

    /// The selected Files row: a file, or a directory in tree mode.
    #[must_use]
    pub fn file_row(&self) -> Option<&FileRow> {
        self.file_tree.row(self.cursors.files.index())
    }

    /// The selected working-tree file, or `None` when the cursor is on a directory row.
    #[must_use]
    pub fn selected_file(&self) -> Option<&FileStatus> {
        let index = self.file_row()?.file?;
        self.files().get(index)
    }

    /// The selected local branch.
    #[must_use]
    pub fn selected_branch(&self) -> Option<&Branch> {
        self.branches().get(self.cursors.branches.index())
    }

    /// The selected remote.
    #[must_use]
    pub fn selected_remote(&self) -> Option<&Remote> {
        self.remotes().get(self.cursors.remotes.index())
    }

    /// The selected remote branch.
    #[must_use]
    pub fn selected_remote_branch(&self) -> Option<&RemoteBranch> {
        self.remote_branches()
            .get(self.cursors.remote_branches.index())
    }

    /// The selected tag.
    #[must_use]
    pub fn selected_tag(&self) -> Option<&Tag> {
        self.tags().get(self.cursors.tags.index())
    }

    /// The selected commit.
    #[must_use]
    pub fn selected_commit(&self) -> Option<&Commit> {
        self.commits().get(self.cursors.commits.index())
    }

    /// The selected reflog entry.
    #[must_use]
    pub fn selected_reflog(&self) -> Option<&ReflogEntry> {
        self.reflog().get(self.cursors.reflog.index())
    }

    /// The selected stash entry.
    #[must_use]
    pub fn selected_stash(&self) -> Option<&StashEntry> {
        self.stashes().get(self.cursors.stashes.index())
    }

    /// The top overlay, if any.
    #[must_use]
    pub fn overlay(&self) -> Option<&Overlay> {
        self.overlays.last()
    }

    /// The top overlay, mutably.
    pub fn overlay_mut(&mut self) -> Option<&mut Overlay> {
        self.overlays.last_mut()
    }

    // ---------------------------------------------------------------- key contexts

    /// The nested gpui key contexts, outermost first, **below** the root context.
    ///
    /// An overlay replaces the whole chain rather than adding to it, which is how lazygit's
    /// "a prompt swallows the keymap" rule is reproduced.
    #[must_use]
    pub fn context_chain(&self) -> Vec<&'static str> {
        if let Some(overlay) = self.overlay() {
            return match overlay {
                Overlay::Confirm(_) => vec!["Dialog", "Confirm"],
                Overlay::Prompt(_) => vec!["Dialog", "Prompt"],
                Overlay::Menu(menu) if menu.filter.is_some() => vec!["Dialog", "MenuFilter"],
                Overlay::Menu(_) => vec!["Dialog", "Menu"],
                Overlay::Help { .. } => vec!["Dialog", "Help"],
            };
        }
        match self.focused {
            PanelId::Status => vec!["Panels", "Status"],
            PanelId::Files => vec!["Panels", "Files"],
            PanelId::Branches => match self.branch_tab {
                BranchTab::Local => vec!["Panels", "Branches"],
                BranchTab::Remotes => vec!["Panels", "Remotes"],
                BranchTab::Tags => vec!["Panels", "Tags"],
            },
            PanelId::Commits => match self.commit_tab {
                CommitTab::Commits => vec!["Panels", "Commits"],
                CommitTab::Reflog => vec!["Panels", "Reflog"],
            },
            PanelId::Stash => vec!["Panels", "Stash"],
            PanelId::Main => {
                if self.staging.is_some() {
                    vec!["Panels", "Main", "Staging"]
                } else {
                    match self.main {
                        MainContent::Conflict { .. } => vec!["Panels", "Main", "Conflict"],
                        MainContent::SubCommits { .. } => vec!["Panels", "Main", "SubCommits"],
                        MainContent::CommitFiles { .. } => vec!["Panels", "Main", "CommitFiles"],
                        _ => vec!["Panels", "Main"],
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------- reducers

    /// Installs a fresh snapshot, keeping every cursor on the item it was on.
    ///
    /// Returns the follow-up requests the new snapshot implies.
    pub fn apply_snapshot(&mut self, snapshot: Box<RepoSnapshot>) -> Vec<GitRequest> {
        // Snapshots can complete out of order; a stale one must never overwrite a newer one.
        if self.snapshot.is_some() && snapshot.generation <= self.epoch {
            return Vec::new();
        }
        self.epoch = snapshot.generation;
        self.refreshing = false;
        let snapshot = Arc::new(*snapshot);

        self.file_tree.rebuild(&snapshot.files);
        self.resync_file_rows();

        let branches = &snapshot.local_branches;
        let wanted = self.sel_branch.clone();
        self.cursors.branches.retain(branches.len(), |_| {
            wanted
                .as_ref()
                .and_then(|name| branches.iter().position(|branch| &branch.name == name))
        });
        self.sel_branch = branches
            .get(self.cursors.branches.index())
            .map(|branch| branch.name.clone());

        let remotes = &snapshot.remotes;
        let wanted = self.sel_remote.clone();
        self.cursors.remotes.retain(remotes.len(), |_| {
            wanted
                .as_ref()
                .and_then(|name| remotes.iter().position(|remote| &remote.name == name))
        });
        self.sel_remote = remotes
            .get(self.cursors.remotes.index())
            .map(|remote| remote.name.clone());

        let tags = &snapshot.tags;
        let wanted = self.sel_tag.clone();
        self.cursors.tags.retain(tags.len(), |_| {
            wanted
                .as_ref()
                .and_then(|name| tags.iter().position(|tag| &tag.name == name))
        });
        self.sel_tag = tags
            .get(self.cursors.tags.index())
            .map(|tag| tag.name.clone());

        let commits = &snapshot.commits;
        let wanted = self.sel_commit.clone();
        self.cursors.commits.retain(commits.len(), |_| {
            wanted
                .as_ref()
                .and_then(|oid| commits.iter().position(|commit| &commit.oid == oid))
        });
        self.sel_commit = commits
            .get(self.cursors.commits.index())
            .map(|commit| commit.oid.clone());

        let reflog = &snapshot.reflog;
        let wanted = self.sel_reflog.clone();
        self.cursors.reflog.retain(reflog.len(), |_| {
            wanted
                .as_ref()
                .and_then(|selector| reflog.iter().position(|entry| &entry.selector == selector))
        });
        self.sel_reflog = reflog
            .get(self.cursors.reflog.index())
            .map(|entry| entry.selector.clone());

        let stashes = &snapshot.stashes;
        let wanted = self.sel_stash;
        self.cursors.stashes.retain(stashes.len(), |_| {
            wanted.and_then(|index| stashes.iter().position(|entry| entry.index == index))
        });
        self.sel_stash = stashes
            .get(self.cursors.stashes.index())
            .map(|entry| entry.index);

        if let Some(remote) = self.remote_drill.clone() {
            let branches = snapshot
                .remote_branches
                .iter()
                .find(|group| group.remote == remote)
                .map_or(0, |group| group.branches.len());
            self.cursors.remote_branches.set_len(branches);
        }

        self.snapshot = Some(snapshot);
        self.refresh_main()
    }

    /// Re-derives the main panel from the focused panel's selection, keeping any cached diff.
    ///
    /// Returns the read requests needed to fill it.
    pub fn refresh_main(&mut self) -> Vec<GitRequest> {
        if self.staging.is_some() {
            // Staging keeps the file diff it already has; a snapshot refresh re-reads it.
            if let Some(staging) = &self.staging {
                return vec![GitRequest::FileDiff(staging.path.clone())];
            }
        }
        // A main-panel drill-down (sub-commits, commit files) survives a refresh: it is a view
        // of history, not of the working tree, and rebuilding it from the side panel's selection
        // would throw the user out of it on the next watcher tick.
        if self.focused == PanelId::Main
            && matches!(
                self.main,
                MainContent::SubCommits { .. } | MainContent::CommitFiles { .. }
            )
        {
            return Vec::new();
        }
        let panel = if self.focused == PanelId::Main {
            self.previous_panel
        } else {
            self.focused
        };
        match panel {
            PanelId::Status => {
                self.main = MainContent::Summary;
                Vec::new()
            }
            PanelId::Files => match self.file_row().cloned() {
                // A directory row shows the combined patch of every file under it. It reuses
                // `MainContent::FileDiff` on purpose: to the main panel a directory is just a
                // path with two sides, and `GitRequest::PathsDiff` answers with the same event.
                //
                // TODO(diff view): `panels/main_panel.rs` titles this "Unstaged changes" /
                // "Staged changes" without naming the path, which reads the same for a file and
                // for a directory. If the diff renderer ever grows a subtitle, the directory
                // path in `MainContent::FileDiff::path` is what it should print.
                Some(row) if row.is_dir => {
                    let path = row.path.clone();
                    let (unstaged, staged) = match &self.main {
                        MainContent::FileDiff {
                            path: old,
                            unstaged,
                            staged,
                        } if *old == path => (unstaged.clone(), staged.clone()),
                        _ => (None, None),
                    };
                    self.main = MainContent::FileDiff {
                        path: path.clone(),
                        unstaged,
                        staged,
                    };
                    vec![GitRequest::PathsDiff {
                        key: path,
                        paths: row.children,
                    }]
                }
                Some(row) => {
                    let Some(file) = row.file.and_then(|index| self.files().get(index)) else {
                        self.main = MainContent::Empty("No changed files.".to_owned());
                        return Vec::new();
                    };
                    let path = file.path.clone();
                    let conflicted = file.conflict.is_some();
                    if conflicted {
                        let section = match &self.main {
                            MainContent::Conflict {
                                path: old, section, ..
                            } if *old == path => *section,
                            _ => 0,
                        };
                        let file = match &self.main {
                            MainContent::Conflict {
                                path: old, file, ..
                            } if *old == path => file.clone(),
                            _ => None,
                        };
                        self.main = MainContent::Conflict {
                            path: path.clone(),
                            file,
                            section,
                        };
                        vec![GitRequest::Conflict(path)]
                    } else {
                        let (unstaged, staged) = match &self.main {
                            MainContent::FileDiff {
                                path: old,
                                unstaged,
                                staged,
                            } if *old == path => (unstaged.clone(), staged.clone()),
                            _ => (None, None),
                        };
                        self.main = MainContent::FileDiff {
                            path: path.clone(),
                            unstaged,
                            staged,
                        };
                        vec![GitRequest::FileDiff(path)]
                    }
                }
                None => {
                    self.main = MainContent::Empty("No changed files.".to_owned());
                    Vec::new()
                }
            },
            PanelId::Branches => match self.branch_tab {
                BranchTab::Local => match self.selected_branch() {
                    Some(branch) => {
                        let name = branch.name.clone();
                        let diff = match &self.main {
                            MainContent::BranchDiff { name: old, diff } if *old == name => {
                                diff.clone()
                            }
                            _ => None,
                        };
                        self.main = MainContent::BranchDiff {
                            name: name.clone(),
                            diff,
                        };
                        vec![GitRequest::BranchDiff(name)]
                    }
                    None => {
                        self.main =
                            MainContent::Empty("No branches in this repository.".to_owned());
                        Vec::new()
                    }
                },
                BranchTab::Remotes => {
                    if self.remote_drill.is_some() {
                        match self.selected_remote_branch() {
                            Some(branch) => {
                                let name = branch.name.clone();
                                let diff = match &self.main {
                                    MainContent::BranchDiff { name: old, diff } if *old == name => {
                                        diff.clone()
                                    }
                                    _ => None,
                                };
                                self.main = MainContent::BranchDiff {
                                    name: name.clone(),
                                    diff,
                                };
                                vec![GitRequest::BranchDiff(name)]
                            }
                            None => {
                                self.main =
                                    MainContent::Empty("No branches on this remote.".to_owned());
                                Vec::new()
                            }
                        }
                    } else {
                        match self.selected_remote() {
                            Some(remote) => {
                                self.main = MainContent::RemoteInfo {
                                    name: remote.name.clone(),
                                };
                            }
                            None => {
                                self.main = MainContent::Empty("No remotes.".to_owned());
                            }
                        }
                        Vec::new()
                    }
                }
                BranchTab::Tags => {
                    match self.selected_tag() {
                        Some(tag) => {
                            self.main = MainContent::TagInfo {
                                name: tag.name.clone(),
                            };
                        }
                        None => self.main = MainContent::Empty("No tags.".to_owned()),
                    }
                    Vec::new()
                }
            },
            PanelId::Commits => {
                let oid = match self.commit_tab {
                    CommitTab::Commits => self.selected_commit().map(|commit| commit.oid.clone()),
                    CommitTab::Reflog => self.selected_reflog().map(|entry| entry.oid.clone()),
                };
                match oid {
                    Some(oid) => {
                        let (diff, files) = match &self.main {
                            MainContent::CommitDiff {
                                oid: old,
                                diff,
                                files,
                            } if *old == oid => (diff.clone(), files.clone()),
                            _ => (None, Vec::new()),
                        };
                        self.main = MainContent::CommitDiff {
                            oid: oid.clone(),
                            diff,
                            files,
                        };
                        vec![GitRequest::CommitDiff(oid)]
                    }
                    None => {
                        self.main = MainContent::Empty("No commits on this branch.".to_owned());
                        Vec::new()
                    }
                }
            }
            PanelId::Stash => match self.selected_stash() {
                Some(entry) => {
                    let index = entry.index;
                    let diff = match &self.main {
                        MainContent::StashDiff { index: old, diff } if *old == index => {
                            diff.clone()
                        }
                        _ => None,
                    };
                    self.main = MainContent::StashDiff { index, diff };
                    vec![GitRequest::StashDiff(index)]
                }
                None => {
                    self.main = MainContent::Empty("No stash entries.".to_owned());
                    Vec::new()
                }
            },
            PanelId::Main => Vec::new(),
        }
    }

    /// Points the Files cursor back at [`GitUiState::sel_file`] after the row list changed.
    ///
    /// A collapse deletes the row the cursor was on, so the lookup falls back to the deepest
    /// visible ancestor: the directory that swallowed the selection, which is where lazygit
    /// leaves the cursor.
    pub fn resync_file_rows(&mut self) {
        let tree = &self.file_tree;
        let wanted = self.sel_file.clone();
        self.cursors.files.retain(tree.len(), |_| {
            wanted.as_ref().and_then(|path| tree.index_of(path))
        });
        self.sel_file = self
            .file_tree
            .row(self.cursors.files.index())
            .map(|row| row.path.clone());
    }

    /// `` ` `` / `~` — switch the Files pane between the tree and the flat layout.
    pub fn toggle_file_tree_mode(&mut self) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        self.file_tree.toggle_mode(&snapshot.files);
        self.resync_file_rows();
    }

    /// `enter` on a directory row — collapse it, or expand it again.
    pub fn toggle_file_collapsed(&mut self, path: &Path) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        self.file_tree.toggle_collapsed(path, &snapshot.files);
        self.resync_file_rows();
    }

    /// `-` — collapse every directory.
    pub fn collapse_all_files(&mut self) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        self.file_tree.collapse_all(&snapshot.files);
        self.resync_file_rows();
    }

    /// `=` — expand every directory.
    pub fn expand_all_files(&mut self) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        self.file_tree.expand_all(&snapshot.files);
        self.resync_file_rows();
    }

    /// Records the identity of whatever the focused panel now points at.
    pub fn remember_selection(&mut self) {
        match self.focused {
            PanelId::Files => {
                self.sel_file = self.file_row().map(|row| row.path.clone());
            }
            PanelId::Branches => match self.branch_tab {
                BranchTab::Local => {
                    self.sel_branch = self.selected_branch().map(|branch| branch.name.clone());
                }
                BranchTab::Remotes => {
                    if self.remote_drill.is_some() {
                        self.sel_remote_branch = self
                            .selected_remote_branch()
                            .map(|branch| branch.name.clone());
                    } else {
                        self.sel_remote = self.selected_remote().map(|remote| remote.name.clone());
                    }
                }
                BranchTab::Tags => {
                    self.sel_tag = self.selected_tag().map(|tag| tag.name.clone());
                }
            },
            PanelId::Commits => match self.commit_tab {
                CommitTab::Commits => {
                    self.sel_commit = self.selected_commit().map(|commit| commit.oid.clone());
                }
                CommitTab::Reflog => {
                    self.sel_reflog = self.selected_reflog().map(|entry| entry.selector.clone());
                }
            },
            PanelId::Stash => {
                self.sel_stash = self.selected_stash().map(|entry| entry.index);
            }
            PanelId::Status | PanelId::Main => {}
        }
    }

    /// Installs a diff that just arrived. Returns whether anything changed.
    pub fn apply_file_diff(&mut self, path: &Path, unstaged: Arc<Diff>, staged: Arc<Diff>) -> bool {
        match &mut self.main {
            MainContent::FileDiff {
                path: current,
                unstaged: u,
                staged: s,
            } if current == path => {
                keep_or_replace(u, unstaged);
                keep_or_replace(s, staged);
                true
            }
            _ => false,
        }
    }

    /// The cursor of the list the focused panel renders.
    #[must_use]
    pub fn focused_cursor(&self) -> &ListCursor {
        match self.focused {
            PanelId::Status => &self.cursors.main,
            PanelId::Files => &self.cursors.files,
            PanelId::Branches => match self.branch_tab {
                BranchTab::Local => &self.cursors.branches,
                BranchTab::Remotes if self.remote_drill.is_some() => &self.cursors.remote_branches,
                BranchTab::Remotes => &self.cursors.remotes,
                BranchTab::Tags => &self.cursors.tags,
            },
            PanelId::Commits => match self.commit_tab {
                CommitTab::Commits => &self.cursors.commits,
                CommitTab::Reflog => &self.cursors.reflog,
            },
            PanelId::Stash => &self.cursors.stashes,
            PanelId::Main => &self.cursors.main,
        }
    }

    /// The cursor of the list the focused panel renders, mutably.
    pub fn focused_cursor_mut(&mut self) -> &mut ListCursor {
        match self.focused {
            PanelId::Status => &mut self.cursors.main,
            PanelId::Files => &mut self.cursors.files,
            PanelId::Branches => match self.branch_tab {
                BranchTab::Local => &mut self.cursors.branches,
                BranchTab::Remotes if self.remote_drill.is_some() => {
                    &mut self.cursors.remote_branches
                }
                BranchTab::Remotes => &mut self.cursors.remotes,
                BranchTab::Tags => &mut self.cursors.tags,
            },
            PanelId::Commits => match self.commit_tab {
                CommitTab::Commits => &mut self.cursors.commits,
                CommitTab::Reflog => &mut self.cursors.reflog,
            },
            PanelId::Stash => &mut self.cursors.stashes,
            PanelId::Main => &mut self.cursors.main,
        }
    }

    /// Pushes an overlay.
    pub fn push_overlay(&mut self, overlay: Overlay) {
        self.overlays.push(overlay);
    }

    /// Pops the top overlay. Returns whether one was open.
    pub fn pop_overlay(&mut self) -> bool {
        self.overlays.pop().is_some()
    }

    /// Raises a toast that expires after `dwell`.
    pub fn toast(&mut self, toast: Toast, now: Instant, dwell: std::time::Duration) {
        if self.toasts.len() >= 3 {
            self.toasts.remove(0);
            self.toast_deadlines.remove(0);
        }
        self.toasts.push(toast);
        self.toast_deadlines.push(now + dwell);
    }

    /// Expires toasts. Returns whether anything changed.
    pub fn expire_toasts(&mut self, now: Instant) -> bool {
        let before = self.toasts.len();
        let mut kept = Vec::new();
        let mut deadlines = Vec::new();
        for (toast, deadline) in self
            .toasts
            .drain(..)
            .zip(self.toast_deadlines.drain(..).collect::<Vec<_>>())
        {
            if deadline > now {
                kept.push(toast);
                deadlines.push(deadline);
            }
        }
        self.toasts = kept;
        self.toast_deadlines = deadlines;
        before != self.toasts.len()
    }

    /// Appends a command-log line, capped at [`COMMAND_LOG_CAP`].
    pub fn log_command(&mut self, line: String) {
        if self.command_log.len() >= COMMAND_LOG_CAP {
            self.command_log.pop_front();
        }
        self.command_log.push_back(line);
    }
}

/// The one line of a failure the status bar's error slot shows.
///
/// Git puts the useful sentence anywhere in its stderr — `git merge` prints `Auto-merging …`
/// before `CONFLICT (content): …` — so the first line is often not the one worth 60 characters
/// of a one-row bar. The first line carrying a Git error marker wins; otherwise the first
/// non-blank line does.
#[must_use]
pub fn error_line(message: &str) -> String {
    const MARKERS: [&str; 5] = ["fatal:", "error:", "CONFLICT", "warning:", "hint:"];
    let mut lines = message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let mut first = None;
    let mut marked = None;
    for line in &mut lines {
        if first.is_none() {
            first = Some(line);
        }
        if marked.is_none() && MARKERS.iter().any(|marker| line.starts_with(marker)) {
            marked = Some(line);
            break;
        }
    }
    marked
        .or(first)
        .unwrap_or_else(|| message.trim())
        .to_owned()
}

/// The lazygit mode word for an operation state, or `None` when nothing is in progress.
#[must_use]
pub fn mode_word(operation: &OperationState) -> Option<&'static str> {
    match operation {
        OperationState::None => None,
        OperationState::Merging => Some("MERGING"),
        OperationState::Rebasing { .. } => Some("REBASING"),
        OperationState::CherryPicking => Some("PICKING"),
        OperationState::Reverting => Some("REVERTING"),
        OperationState::Bisecting => Some("BISECTING"),
    }
}

/// lazygit's lowercase, parenthesised mode word for the status panel.
#[must_use]
pub fn lower_mode_word(operation: &OperationState) -> Option<&'static str> {
    match operation {
        OperationState::None => None,
        OperationState::Merging => Some("merging"),
        OperationState::Rebasing { .. } => Some("rebasing"),
        OperationState::CherryPicking => Some("cherry-picking"),
        OperationState::Reverting => Some("reverting"),
        OperationState::Bisecting => Some("bisecting"),
    }
}

/// The two-character short status lazygit prints, index char first.
#[must_use]
pub fn short_status(file: &FileStatus) -> [char; 2] {
    if file.index == ChangeKind::Untracked || file.worktree == ChangeKind::Untracked {
        return ['?', '?'];
    }
    [status_char(file.index), status_char(file.worktree)]
}

fn status_char(kind: ChangeKind) -> char {
    match kind {
        ChangeKind::Unmodified => ' ',
        ChangeKind::Added => 'A',
        ChangeKind::Modified => 'M',
        ChangeKind::Deleted => 'D',
        ChangeKind::Renamed => 'R',
        ChangeKind::Copied => 'C',
        ChangeKind::TypeChanged => 'T',
        ChangeKind::Untracked => '?',
        ChangeKind::Ignored => '!',
        ChangeKind::Unmerged => 'U',
        ChangeKind::Unknown(byte) => byte as char,
    }
}

/// Whether the file has anything in the index (lazygit's `hasStagedChanges`).
#[must_use]
pub fn has_staged(file: &FileStatus) -> bool {
    let [index, _] = short_status(file);
    !matches!(index, ' ' | 'U' | '?')
}

/// Whether the file has anything in the worktree (lazygit's `HasUnstagedChanges`).
#[must_use]
pub fn has_unstaged(file: &FileStatus) -> bool {
    let [_, worktree] = short_status(file);
    worktree != ' '
}

/// lazygit's `↓n↑m` / `✓` upstream string, or `None` when the branch has no upstream.
#[must_use]
pub fn upstream_status(branch: &Branch) -> Option<String> {
    let upstream = branch.upstream.as_ref()?;
    if upstream.ahead == 0 && upstream.behind == 0 {
        return Some("✓".to_owned());
    }
    let mut text = String::new();
    if upstream.behind > 0 {
        text.push_str(&format!("↓{}", upstream.behind));
    }
    if upstream.ahead > 0 {
        text.push_str(&format!("↑{}", upstream.ahead));
    }
    Some(text)
}

/// lazygit's three-character recency string (`"  *"` for the checked-out branch).
///
/// The column is *checkout* recency: `Branch::checked_out_at` when the HEAD reflog mentions the
/// branch, and the tip's committer date only as a fallback, exactly as lazygit's branch loader
/// fills `models.Branch.Recency`.
#[must_use]
pub fn recency(branch: &Branch, now: i64) -> String {
    if branch.is_head {
        return "  *".to_owned();
    }
    let at = branch.checked_out_at.unwrap_or(branch.committed_at);
    time_ago(now.saturating_sub(at))
}

/// lazygit's `utils.UnixToTimeAgo`: a number plus one of `s m h d w M y`.
#[must_use]
pub fn time_ago(seconds: i64) -> String {
    let seconds = seconds.max(0);
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;
    if seconds < MINUTE {
        format!("{seconds}s")
    } else if seconds < HOUR {
        format!("{}m", seconds / MINUTE)
    } else if seconds < DAY {
        format!("{}h", seconds / HOUR)
    } else if seconds < WEEK {
        format!("{}d", seconds / DAY)
    } else if seconds < MONTH {
        format!("{}w", seconds / WEEK)
    } else if seconds < YEAR {
        format!("{}M", seconds / MONTH)
    } else {
        format!("{}y", seconds / YEAR)
    }
}

/// Replaces a cached patch only when its content really changed.
///
/// The periodic refresh re-reads the same diff every couple of seconds. Keeping the existing
/// `Arc` when the bytes are identical is what lets the pointer-keyed `DiffModel` cache survive a
/// refresh — and with it the syntax pass, the scroll offset and the cursor. Comparing two
/// parsed diffs is a walk over the line vectors; rebuilding the model and re-highlighting it is
/// far more expensive.
pub fn keep_or_replace(slot: &mut Option<Arc<Diff>>, fresh: Arc<Diff>) {
    match slot {
        Some(existing) if **existing == *fresh => {}
        _ => *slot = Some(fresh),
    }
}

/// The rows of the hunk the cursor sits in, as `(start, end)` indexes into `rows`.
///
/// Keyed by **(file, hunk)**, not by the hunk index alone: a diff with two `DiffFile`s numbers
/// its hunks from zero inside each file, so matching on the index alone would splice two files'
/// hunk 0 into one selection.
#[must_use]
pub fn hunk_range(rows: &[DiffRow], cursor: usize) -> Option<(usize, usize)> {
    let row = rows.get(cursor)?;
    let hunk = row.hunk?;
    let key = (row.file, hunk);
    let matches = |candidate: &DiffRow| {
        candidate.line.is_some() && (candidate.file, candidate.hunk) == (key.0, Some(key.1))
    };
    let start = rows.iter().position(matches)?;
    let end = rows.iter().rposition(matches)?;
    Some((start, end))
}

/// The `HunkSelection`s the rows `start..=end` describe, restricted to one file.
///
/// `fleet_git::PatchSelection` names exactly one path and its `hunk_index` is an index into that
/// file's hunks, so a range that happens to span two `DiffFile`s contributes only the rows of the
/// file the range starts in. The returned `Vec` is empty when nothing selectable was covered.
#[must_use]
pub fn selection_hunks(rows: &[DiffRow], start: usize, end: usize) -> Vec<HunkSelection> {
    let Some(slice) = rows.get(start..=end) else {
        return Vec::new();
    };
    let Some(file) = slice
        .iter()
        .find(|row| row.is_change() && row.hunk.is_some() && row.line.is_some())
        .map(|row| row.file)
    else {
        return Vec::new();
    };
    let mut hunks: Vec<HunkSelection> = Vec::new();
    for row in slice {
        let (Some(hunk_index), Some(line)) = (row.hunk, row.line) else {
            continue;
        };
        if row.file != file || !row.is_change() {
            continue;
        }
        match hunks
            .iter_mut()
            .find(|selection| selection.hunk_index == hunk_index)
        {
            Some(selection) => {
                if let Some(lines) = &mut selection.lines {
                    lines.push(line);
                }
            }
            None => hunks.push(HunkSelection {
                hunk_index,
                lines: Some(vec![line]),
            }),
        }
    }
    hunks
}

/// The short hash lazygit shows: the first eight characters.
#[must_use]
pub fn short_oid(oid: &ObjectId) -> String {
    oid.as_str().chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::diff_model::RowKind;
    use std::path::PathBuf;

    #[test]
    fn the_error_slot_prefers_the_line_carrying_gits_marker() {
        assert_eq!(
            error_line("Auto-merging a.txt\nCONFLICT (content): Merge conflict in a.txt\nfailed"),
            "CONFLICT (content): Merge conflict in a.txt"
        );
        assert_eq!(
            error_line("fatal: a branch named 'main' already exists"),
            "fatal: a branch named 'main' already exists"
        );
        assert_eq!(error_line("\n  plain trouble  \nmore"), "plain trouble");
        assert_eq!(error_line(""), "");
    }

    fn file(path: &str, index: ChangeKind, worktree: ChangeKind) -> FileStatus {
        FileStatus {
            path: PathBuf::from(path),
            previous_path: None,
            index,
            worktree,
            conflict: None,
        }
    }

    #[test]
    fn short_status_matches_git_porcelain() {
        assert_eq!(
            short_status(&file("a", ChangeKind::Untracked, ChangeKind::Untracked)),
            ['?', '?']
        );
        assert_eq!(
            short_status(&file("a", ChangeKind::Modified, ChangeKind::Modified)),
            ['M', 'M']
        );
        assert_eq!(
            short_status(&file("a", ChangeKind::Added, ChangeKind::Unmodified)),
            ['A', ' ']
        );
        assert_eq!(
            short_status(&file("a", ChangeKind::Unmodified, ChangeKind::Modified)),
            [' ', 'M']
        );
    }

    #[test]
    fn staged_and_unstaged_predicates_follow_lazygit() {
        let fully_staged = file("a", ChangeKind::Modified, ChangeKind::Unmodified);
        assert!(has_staged(&fully_staged));
        assert!(!has_unstaged(&fully_staged));

        let untracked = file("a", ChangeKind::Untracked, ChangeKind::Untracked);
        assert!(!has_staged(&untracked));
        assert!(has_unstaged(&untracked));

        let mixed = file("a", ChangeKind::Modified, ChangeKind::Modified);
        assert!(has_staged(&mixed));
        assert!(has_unstaged(&mixed));
    }

    #[test]
    fn upstream_status_puts_behind_first() {
        let mut branch = Branch {
            name: "main".to_owned(),
            oid: ObjectId::from("abc"),
            is_head: false,
            upstream: Some(fleet_git::Upstream {
                name: "origin/main".to_owned(),
                ahead: 3,
                behind: 5,
            }),
            subject: String::new(),
            committed_at: 0,
            checked_out_at: None,
        };
        assert_eq!(upstream_status(&branch).as_deref(), Some("↓5↑3"));
        branch.upstream = Some(fleet_git::Upstream {
            name: "origin/main".to_owned(),
            ahead: 0,
            behind: 0,
        });
        assert_eq!(upstream_status(&branch).as_deref(), Some("✓"));
        branch.upstream = None;
        assert_eq!(upstream_status(&branch), None);
    }

    #[test]
    fn screen_mode_cycles_in_both_directions() {
        assert_eq!(ScreenMode::Normal.next(), ScreenMode::Half);
        assert_eq!(ScreenMode::Half.next(), ScreenMode::Full);
        assert_eq!(ScreenMode::Full.next(), ScreenMode::Normal);
        assert_eq!(ScreenMode::Normal.prev(), ScreenMode::Full);
    }

    #[test]
    fn side_panels_cycle_and_wrap() {
        assert_eq!(PanelId::Status.next_side(), PanelId::Files);
        assert_eq!(PanelId::Stash.next_side(), PanelId::Status);
        assert_eq!(PanelId::Status.prev_side(), PanelId::Stash);
    }

    #[test]
    fn an_overlay_replaces_the_context_chain() {
        let mut state = GitUiState::new(PathBuf::from("/tmp"));
        state.focused = PanelId::Files;
        assert_eq!(state.context_chain(), vec!["Panels", "Files"]);
        state.push_overlay(Overlay::Help { top: 0 });
        assert_eq!(state.context_chain(), vec!["Dialog", "Help"]);
        assert!(state.pop_overlay());
        assert_eq!(state.context_chain(), vec!["Panels", "Files"]);
    }

    #[test]
    fn a_stale_snapshot_is_dropped() {
        let mut state = GitUiState::new(PathBuf::from("/tmp"));
        let mut snapshot = snapshot_with_generation(7);
        state.apply_snapshot(Box::new(snapshot.clone()));
        assert_eq!(state.epoch, 7);
        snapshot.generation = 5;
        snapshot
            .files
            .push(file("late", ChangeKind::Modified, ChangeKind::Modified));
        state.apply_snapshot(Box::new(snapshot));
        assert_eq!(state.epoch, 7);
        assert!(state.files().is_empty());
    }

    #[test]
    fn the_file_cursor_follows_the_file_across_a_refresh() {
        let mut state = GitUiState::new(PathBuf::from("/tmp"));
        let mut snapshot = snapshot_with_generation(1);
        snapshot.files = vec![
            file("a", ChangeKind::Modified, ChangeKind::Unmodified),
            file("b", ChangeKind::Modified, ChangeKind::Unmodified),
            file("c", ChangeKind::Modified, ChangeKind::Unmodified),
        ];
        state.apply_snapshot(Box::new(snapshot.clone()));
        state.cursors.files.set(2);
        state.remember_selection();
        assert_eq!(state.sel_file, Some(PathBuf::from("c")));

        snapshot.generation = 2;
        snapshot.files.remove(0);
        state.apply_snapshot(Box::new(snapshot));
        assert_eq!(state.cursors.files.index(), 1);
        assert_eq!(
            state.selected_file().map(|f| f.path.clone()),
            Some(PathBuf::from("c"))
        );
    }

    #[test]
    fn a_directory_row_survives_a_refresh_and_asks_for_its_children_diff() {
        let mut state = GitUiState::new(PathBuf::from("/tmp"));
        let mut snapshot = snapshot_with_generation(1);
        snapshot.files = vec![
            file("src/app/one", ChangeKind::Unmodified, ChangeKind::Modified),
            file("src/app/two", ChangeKind::Unmodified, ChangeKind::Modified),
            file("top", ChangeKind::Unmodified, ChangeKind::Modified),
        ];
        state.focused = PanelId::Files;
        state.apply_snapshot(Box::new(snapshot.clone()));
        // Row 0 is the compressed `src/app` directory, then its two files, then `top`.
        assert_eq!(state.file_tree.len(), 4);
        state.cursors.files.set(0);
        state.remember_selection();
        assert_eq!(state.sel_file, Some(PathBuf::from("src/app")));

        let requests = state.refresh_main();
        assert!(matches!(
            requests.as_slice(),
            [GitRequest::PathsDiff { key, paths }]
                if key == Path::new("src/app") && paths.len() == 2
        ));
        assert!(state.selected_file().is_none(), "a directory is not a file");

        // A new file under the directory must not move the cursor off it.
        snapshot.generation = 2;
        snapshot.files.push(file(
            "src/app/three",
            ChangeKind::Modified,
            ChangeKind::Unmodified,
        ));
        state.apply_snapshot(Box::new(snapshot));
        assert_eq!(state.cursors.files.index(), 0);
        assert_eq!(state.sel_file, Some(PathBuf::from("src/app")));
        let row = state.file_row().expect("the directory row");
        assert!(row.is_dir && row.staged && row.unstaged);
    }

    #[test]
    fn collapsing_a_directory_moves_the_selection_onto_it() {
        let mut state = GitUiState::new(PathBuf::from("/tmp"));
        let mut snapshot = snapshot_with_generation(1);
        snapshot.files = vec![
            file("src/app/one", ChangeKind::Unmodified, ChangeKind::Modified),
            file("top", ChangeKind::Unmodified, ChangeKind::Modified),
        ];
        state.focused = PanelId::Files;
        state.apply_snapshot(Box::new(snapshot));
        state.cursors.files.set(1);
        state.remember_selection();
        assert_eq!(state.sel_file, Some(PathBuf::from("src/app/one")));

        state.toggle_file_collapsed(Path::new("src/app"));
        assert_eq!(state.cursors.files.index(), 0);
        assert_eq!(state.sel_file, Some(PathBuf::from("src/app")));

        // The flat layout lists every file and no directory.
        state.toggle_file_tree_mode();
        assert!(!state.file_tree.tree_mode());
        assert_eq!(state.file_tree.len(), 2);
        assert!(state.file_tree.rows().iter().all(|row| !row.is_dir));
    }

    #[test]
    fn the_buffer_edits_by_characters() {
        let mut buffer = Buffer::single_line();
        buffer.insert("héllo");
        assert_eq!(buffer.value(), "héllo");
        assert_eq!(buffer.caret(), 5);
        buffer.left();
        buffer.backspace();
        assert_eq!(buffer.value(), "hélo");
        buffer.insert("\n");
        assert_eq!(buffer.value(), "hélo");
        let mut multi = Buffer::multi_line();
        multi.insert("one\ntwo");
        assert_eq!(multi.lines_with_caret().0, vec!["one", "two"]);
        multi.delete_to_line_start();
        assert_eq!(multi.value(), "one\n");
    }

    #[test]
    fn time_ago_uses_lazygits_unit_letters() {
        assert_eq!(time_ago(3), "3s");
        assert_eq!(time_ago(120), "2m");
        assert_eq!(time_ago(7_200), "2h");
        assert_eq!(time_ago(172_800), "2d");
        assert_eq!(time_ago(1_209_600), "2w");
    }

    #[test]
    fn a_failing_read_never_clears_the_mutation_guard() {
        let mut state = GitUiState::new(PathBuf::from("/tmp"));
        state.begin_mutation();
        state.begin_read();
        state.begin_read();
        assert!(state.mutation_in_flight());

        // Two reads fail while the mutation is still out; the guard must survive both.
        state.finish_read();
        state.finish_read();
        state.finish_read(); // an extra answer cannot push the counter negative
        assert_eq!(state.pending_reads, 0);
        assert!(state.mutation_in_flight());

        state.finish_mutation();
        assert!(!state.mutation_in_flight());
        state.finish_mutation();
        assert_eq!(state.pending, 0);
    }

    fn diff_row(file: usize, hunk: Option<usize>, line: Option<usize>, kind: RowKind) -> DiffRow {
        DiffRow {
            kind,
            text: String::new(),
            old_no: None,
            new_no: None,
            file,
            hunk,
            line,
            words: Vec::new(),
        }
    }

    /// Two files, one hunk each, numbered 0 in both.
    fn two_file_rows() -> Vec<DiffRow> {
        vec![
            diff_row(0, None, None, RowKind::FileHeader),
            diff_row(0, Some(0), None, RowKind::HunkHeader),
            diff_row(0, Some(0), Some(0), RowKind::Added),
            diff_row(0, Some(0), Some(1), RowKind::Removed),
            diff_row(1, None, None, RowKind::FileHeader),
            diff_row(1, Some(0), None, RowKind::HunkHeader),
            diff_row(1, Some(0), Some(0), RowKind::Added),
        ]
    }

    #[test]
    fn a_hunk_range_stops_at_the_file_boundary() {
        let rows = two_file_rows();
        // Keyed by hunk index alone this would run from row 2 to row 6, across both files.
        assert_eq!(hunk_range(&rows, 2), Some((2, 3)));
        assert_eq!(hunk_range(&rows, 6), Some((6, 6)));
        assert_eq!(hunk_range(&rows, 0), None);
        assert_eq!(hunk_range(&rows, 99), None);
    }

    #[test]
    fn a_selection_covers_one_file_even_when_the_range_spans_two() {
        let rows = two_file_rows();
        let hunks = selection_hunks(&rows, 2, 6);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].hunk_index, 0);
        // Only the first file's two changed lines, never the second file's line 0.
        assert_eq!(hunks[0].lines.as_deref(), Some(&[0usize, 1][..]));

        let second = selection_hunks(&rows, 6, 6);
        assert_eq!(second[0].lines.as_deref(), Some(&[0usize][..]));
        assert!(selection_hunks(&rows, 0, 1).is_empty());
    }

    #[test]
    fn a_menu_letter_finds_its_row() {
        let item = |key: &str| MenuItem {
            key: key.to_owned(),
            label: key.to_owned(),
            action: MenuAction::Request(Box::new(GitRequest::Snapshot)),
        };
        let menu = Menu::new("Reset options", vec![item("s"), item("m"), item("h")]);
        assert_eq!(
            menu.item_for_key("h").map(|item| item.key.clone()),
            Some("h".to_owned())
        );
        assert!(menu.item_for_key("x").is_none());
    }

    #[test]
    fn recency_prefers_the_checkout_timestamp() {
        let mut branch = Branch {
            name: "feature".to_owned(),
            oid: ObjectId::from("abc"),
            is_head: false,
            upstream: None,
            subject: String::new(),
            committed_at: 0,
            checked_out_at: Some(100),
        };
        assert_eq!(recency(&branch, 160), "1m");
        branch.checked_out_at = None;
        assert_eq!(recency(&branch, 160), "2m");
        branch.is_head = true;
        assert_eq!(recency(&branch, 160), "  *");
    }

    fn snapshot_with_generation(generation: u64) -> RepoSnapshot {
        RepoSnapshot {
            root: PathBuf::from("/tmp"),
            head: Head::Unborn {
                name: "main".to_owned(),
            },
            operation: OperationState::None,
            files: Vec::new(),
            local_branches: Vec::new(),
            remote_branches: Vec::new(),
            remotes: Vec::new(),
            tags: Vec::new(),
            commits: Vec::new(),
            reflog: Vec::new(),
            stashes: Vec::new(),
            generation,
        }
    }
}
