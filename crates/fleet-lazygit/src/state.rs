//! The central UI state and its reducers.
//!
//! [`GitUiState`] holds the last good [`RepoSnapshot`] behind an `Arc`, every panel's cursor, the
//! overlay stack, the command log and the transient feedback. Everything that decides *what the
//! next keystroke does* lives here as a plain `&mut self` reducer or a free function with no gpui
//! and no I/O, so it is unit testable; the views read the result and never derive it twice.

mod selections;
pub(crate) use selections::{BranchTab, CommitTab, Cursors, PanelId, ScreenMode};
mod content;
pub(crate) use content::{MainContent, keep_or_replace};
mod staging;
pub(crate) use staging::{Staging, hunk_range, selection_hunks};
mod overlays;
pub(crate) use overlays::{
    Buffer, Confirm, ConfirmOutcome, Menu, MenuAction, MenuItem, Overlay, Prompt, PromptKind,
};
mod presentation;
pub(crate) use presentation::{
    error_line, has_staged, has_unstaged, lower_mode_word, mode_word, recency, short_oid,
    short_status, time_ago, upstream_status,
};
#[cfg(test)]
mod tests;
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
use gpui::SharedString;

use crate::bridge::GitRequest;
use crate::views::diff_model::{DiffRow, DiffViewMode};
use crate::views::file_tree::{FileRow, FileTree};

/// How many command-log records to keep.
pub(crate) const COMMAND_LOG_CAP: usize = 400;

/// Stable identity for a reflog record. Selectors such as `HEAD@{1}` are positions and shift
/// whenever Git prepends a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReflogIdentity {
    oid: ObjectId,
    parents: Vec<ObjectId>,
    subject: String,
    committed_at: i64,
}

impl From<&ReflogEntry> for ReflogIdentity {
    fn from(entry: &ReflogEntry) -> Self {
        Self {
            oid: entry.oid.clone(),
            parents: entry.parents.clone(),
            subject: entry.subject.clone(),
            committed_at: entry.committed_at,
        }
    }
}

/// The whole UI state.
pub(crate) struct GitUiState {
    /// The path the app was launched with; replaced by the worktree root once discovered.
    pub(crate) root: PathBuf,
    /// The last good snapshot. `None` before the first load completes.
    pub(crate) snapshot: Option<Arc<RepoSnapshot>>,
    /// `generation` of the newest accepted snapshot. A snapshot arriving with a lower or equal
    /// generation is stale and is dropped.
    pub(crate) epoch: u64,
    /// A refresh is in flight.
    pub(crate) refreshing: bool,
    /// The repository could not be opened.
    pub(crate) fatal: Option<String>,
    /// The last failure, shown in the status bar until the next success.
    pub(crate) last_error: Option<String>,

    /// Screen mode (`+` / `_`).
    pub(crate) screen_mode: ScreenMode,
    /// Which panel owns the keyboard.
    pub(crate) focused: PanelId,
    /// Where focus returns to when the main panel is escaped.
    pub(crate) previous_panel: PanelId,
    /// Panel 3's tab.
    pub(crate) branch_tab: BranchTab,
    /// Panel 4's tab.
    pub(crate) commit_tab: CommitTab,
    /// The remote whose branches the Remotes tab is listing.
    pub(crate) remote_drill: Option<String>,
    /// Whether the command log band is visible (`@`).
    pub(crate) show_command_log: bool,

    /// Selections by identity, so a refresh follows the item rather than the index.
    pub(crate) sel_file: Option<PathBuf>,
    /// Selected local branch.
    pub(crate) sel_branch: Option<String>,
    /// Selected remote.
    pub(crate) sel_remote: Option<String>,
    /// Selected remote-tracking branch.
    pub(crate) sel_remote_branch: Option<String>,
    /// Selected tag.
    pub(crate) sel_tag: Option<String>,
    /// Selected commit.
    pub(crate) sel_commit: Option<ObjectId>,
    /// Selected reflog record.
    pub(crate) sel_reflog: Option<ReflogIdentity>,
    /// Selected stash commit.
    pub(crate) sel_stash: Option<ObjectId>,
    /// Every list cursor.
    pub(crate) cursors: Cursors,
    /// The Files pane's row model: lazygit's file tree, its collapse set and its flat/tree flag.
    pub(crate) file_tree: FileTree,

    /// What the main panel shows.
    pub(crate) main: MainContent,
    /// Horizontal scroll of the main panel, in pixels.
    ///
    /// Pixels rather than characters: the payload is shifted with a negative margin, so the
    /// clamp is a real content width.
    pub(crate) main_h_scroll: f32,
    /// Whether the diff renders unified or side by side.
    pub(crate) diff_mode: DiffViewMode,
    /// How many context lines every diff read asks git for (`{` / `}`).
    pub(crate) diff_context: u32,
    /// Staging mode, when the main panel is in it.
    pub(crate) staging: Option<Staging>,

    /// LIFO overlay stack; only the top layer receives keys.
    pub(crate) overlays: Vec<Overlay>,
    /// The dialog to open when the mutation carrying this label fails, armed by
    /// [`ConfirmOutcome::RequestOrEscalate`].
    pub(crate) escalation: Option<(String, Box<Confirm>)>,

    /// The command log, newest last.
    pub(crate) command_log: VecDeque<SharedString>,
    /// Commits marked with `c` for a later `v` (cherry-pick).
    pub(crate) copied: Arc<Vec<ObjectId>>,
    toasts: Vec<(Toast, Instant)>,
    /// How many mutations have been dispatched and not yet answered.
    ///
    /// This is the counter `q` asks about and the one that says "an operation is in progress";
    /// reads are counted separately in [`GitUiState::pending_reads`] so a failing diff can never
    /// clear it.
    pub(crate) pending: usize,
    /// How many reads have been dispatched and not yet answered.
    pub(crate) pending_reads: usize,
}

impl GitUiState {
    /// The state the app starts in: nothing loaded, Files focused, no overlay.
    #[must_use]
    pub(crate) fn new(root: PathBuf) -> Self {
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
            copied: Arc::default(),
            toasts: Vec::new(),
            pending: 0,
            pending_reads: 0,
        }
    }

    /// One mutation was dispatched.
    pub(crate) fn begin_mutation(&mut self) {
        self.pending += 1;
    }

    /// One mutation was answered, successfully or not.
    pub(crate) fn finish_mutation(&mut self) {
        self.pending = self.pending.saturating_sub(1);
    }

    /// One read was dispatched.
    pub(crate) fn begin_read(&mut self) {
        self.pending_reads += 1;
    }

    /// One read was answered, successfully or not. Never touches the mutation counter.
    pub(crate) fn finish_read(&mut self) {
        self.pending_reads = self.pending_reads.saturating_sub(1);
    }

    /// Whether a mutation is still in flight. `q` asks first while this holds.
    #[must_use]
    pub(crate) fn mutation_in_flight(&self) -> bool {
        self.pending > 0
    }
}
