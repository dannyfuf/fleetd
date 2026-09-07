//! The application's snapshot and terminal-grid state mirror.
//!
//! [`AppState`] is a gpui entity owned by [`crate::shell::Shell`]. It holds the daemon's
//! authoritative [`Snapshot`], one [`MirrorGrid`] per attached terminal, and every piece of
//! purely local state the UX spec calls for: which screen and mode we are in, the cursor of
//! each list, the MRU orders that make `ctrl-s w` and `ctrl-s Tab` one keystroke, the live
//! toasts, the sticky error slot, and the daemon connection state of §3.12.
//!
//! Everything that decides *what the next keystroke does* lives here as a plain function or a
//! `&mut self` reducer with no gpui and no I/O, so it can be unit tested. Screens read the
//! result; they never derive it a second time.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use fleet_core::{
    board::{BackendDescriptor, BoardView, Card},
    config::{Agent, NotificationsConfig},
    github::PrTab,
    ids::{ContextId, JobId, RepoId, SessionId, TerminalId},
    sessions::{AgentActivity, Session, SessionKind, SessionState, aggregate_agent_activity},
};
use fleet_proto::{
    event::{Event, ToastLevel},
    job::{JobRecord, JobStatus},
    snapshot::{HostStatus, Snapshot},
    terminal::{
        Cell, CellWidth, Color, CursorShape, CursorState, FrameUpdate, TerminalModes, ViewportInfo,
    },
};
use fleet_ui_kit::{Icon, Mode as ModeWord, PrBadgeState, Toast, ToastDuration, Tone};

use crate::{
    bridge::BridgeEvent,
    dialogs::Dialogs,
    notify_sound::{NotificationSound, SystemSound},
};

/// Longest window in which two identical toasts coalesce into one `×n` toast (§2.7).
pub const TOAST_COALESCE_WINDOW: Duration = Duration::from_secs(1);
/// Maximum number of stacked toasts (§2.7).
pub const MAX_TOASTS: usize = 3;
/// How long a `Starting fleetd…` splash waits before it appends the socket path (§3.12 A).
pub const SPLASH_DETAIL_DELAY: Duration = Duration::from_secs(3);
/// How long the mandatory "terminal sessions did not survive" banner stays up (§3.12).
pub const RESTART_BANNER_DWELL: Duration = Duration::from_secs(6);
/// How long a plain "reconnected" banner stays up (§3.12).
pub const RECONNECT_BANNER_DWELL: Duration = Duration::from_millis(800);
/// What the board says when no context is active, i.e. when there is no board to ask for.
///
/// The sentence names the keys that fix it: the board is `EnsureBoard(active_context)`, so
/// picking a context is the whole remedy (BOARD §8, §2.1).
pub const NO_ACTIVE_CONTEXT: &str =
    "No active context \u{2014} pick one with 1\u{2013}9 or gt / gT";
/// How many entries an MRU list keeps.
const MRU_CAPACITY: usize = 32;
/// Minimum observed working time before an idle transition is treated as a completed turn.
const AGENT_FINISH_MIN_WORKING: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------- screens and modes

/// Which pane of the Hub owns the cursor. The detail panel is never in the cycle (§1.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubPane {
    /// The repos rail.
    Repos,
    /// The worktrees or pull-requests list.
    List,
}

/// Which list the Hub is showing (`p` toggles).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubTab {
    /// The worktrees list.
    Worktrees,
    /// The pull-requests screen.
    Prs,
    /// The active context's board.
    Board,
}

/// Keyboard selection within the board's status columns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BoardFocus {
    /// Zero-based status column.
    pub column: usize,
    /// Zero-based card within the column.
    pub row: usize,
}

/// Optional secondary grouping beneath a status column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    /// Group by priority.
    Priority,
    /// Group by assignee.
    Assignee,
    /// Group by label.
    Labels,
}

/// The active context's board data and local presentation state (BOARD §8).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoardState {
    /// Last authoritative board response.
    pub view: Option<BoardView>,
    /// Whether an EnsureBoard request is in flight.
    pub loading: bool,
    /// Most recent load failure, retained until an explicit retry.
    pub error: Option<String>,
    /// Keyboard selection.
    pub focus: BoardFocus,
    /// Card substring filter.
    pub filter: String,
    /// Whether the filter input still owns the keyboard (§3.10's two-stage `Esc`).
    ///
    /// The board is the one list whose filter is not the Hub's [`FilterState`]: its rows are
    /// cards in columns, not worktrees, so `Overlay::Filter` — which moves the worktree cursor
    /// and opens a worktree on `Enter` — cannot serve it. The screen publishes the `Filter` key
    /// context while this is set, which is what makes the bare letters type instead of fire.
    pub filter_editing: bool,
    /// Optional secondary grouping.
    pub group_secondary: Option<GroupBy>,
}

impl BoardState {
    /// Whether cards are currently being filtered.
    #[must_use]
    pub fn is_filtered(&self) -> bool {
        !self.filter.trim().is_empty()
    }
}

/// The two top-level screens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    /// Contexts, repos, worktrees or PRs, detail panel.
    Hub {
        /// Which list the Hub is showing.
        tab: HubTab,
    },
    /// One session's terminals.
    Workspace {
        /// The session being displayed.
        session: SessionId,
    },
}

impl Screen {
    /// The Hub, on the worktrees list.
    #[must_use]
    pub const fn hub() -> Self {
        Self::Hub {
            tab: HubTab::Worktrees,
        }
    }
}

/// The Workspace's sub-mode. Meaningless on the Hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMode {
    /// Every key goes to the PTY except `ctrl-s`.
    Terminal,
    /// Every key goes to the Fleet-drawn pane in the tab except `ctrl-s`.
    ///
    /// The resting mode of a `fleet://` tab. It is [`TerminalMode::Terminal`] with a different
    /// consumer: the same one app key, everything else handled inside the tab — which is why
    /// `Workspace > Native` binds exactly what `Workspace > Terminal` binds.
    Native,
    /// One-shot, entered by `ctrl-s`, left by the very next key.
    Prefix,
    /// Scrollback and copy mode.
    Scroll,
}

/// The floating agent popup's terminal sub-mode.
///
/// This is deliberately separate from [`TerminalMode`]: a Workspace keeps its own mode while
/// the popup is above it, so hiding the popup restores the exact surface the user left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPopupMode {
    /// Every unreserved key goes to the agent PTY.
    Terminal,
    /// One-shot, entered by `ctrl-s`, left by the very next key.
    Prefix,
    /// Scrollback and copy mode.
    Scroll,
}

/// The persistent, screen-independent floating-agent state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentPopupState {
    /// Which fixed daemon-owned agent session is visible.
    pub agent: Agent,
    /// Which terminal input mode owns the popup keyboard.
    pub mode: AgentPopupMode,
    /// The mode restored after the one-shot prefix consumes its next key.
    ///
    /// This matters when `ctrl-s` is entered from Scroll: returning directly to Terminal would
    /// bypass Scroll's selection and viewport cleanup while making the mode word lie.
    pub prefix_return: AgentPopupMode,
}

/// The result of pressing an agent toggle key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPopupTransition {
    /// A closed popup opened.
    Opened,
    /// The other agent replaced the visible one.
    Switched,
    /// Pressing the already-visible agent's key hid the popup.
    Hidden,
}

/// The overlay that currently owns the keyboard. Overlays shadow the Hub and the Workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    /// The filter input over the focused list.
    Filter,
    /// The command palette.
    Palette,
    /// The right-docked jobs panel.
    Jobs,
    /// A modal dialog.
    Dialog(Dialogs),
}

impl Overlay {
    /// The key contexts this overlay pushes, outermost first.
    #[must_use]
    pub fn context_chain(&self) -> Vec<&'static str> {
        match self {
            Self::Filter => vec!["Filter"],
            Self::Palette => vec!["Palette"],
            Self::Jobs => vec!["Jobs"],
            Self::Dialog(dialog) => vec!["Dialog", dialog.context_name()],
        }
    }

    /// The word the status bar shows while this overlay is open (§2.8).
    #[must_use]
    pub const fn mode_word(&self) -> ModeWord {
        match self {
            Self::Filter => ModeWord::Filter,
            Self::Palette => ModeWord::Palette,
            Self::Jobs => ModeWord::Jobs,
            Self::Dialog(_) => ModeWord::Dialog,
        }
    }
}

/// The eight modes of §2.8, derived from the screen and the overlay stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Hub lists.
    Normal,
    /// Keys go to the PTY.
    Terminal,
    /// Keys go to the Fleet-drawn pane in the active tab.
    Native,
    /// One key after `ctrl-s`.
    Prefix,
    /// Scrollback and copy mode.
    Scroll,
    /// Filter input.
    Filter,
    /// Command palette.
    Palette,
    /// A dialog is open.
    Dialog,
    /// The jobs panel is open.
    Jobs,
}

impl Mode {
    /// The kit's mode word for this mode.
    #[must_use]
    pub const fn word(self) -> ModeWord {
        match self {
            Self::Normal => ModeWord::Normal,
            // §2.8 has no ninth word and a native tab is still "the Workspace has the
            // keyboard", so the status bar keeps saying TERMINAL; the tab strip's glyph is
            // what distinguishes the two.
            Self::Terminal | Self::Native => ModeWord::Terminal,
            Self::Prefix => ModeWord::Prefix,
            Self::Scroll => ModeWord::Scroll,
            Self::Filter => ModeWord::Filter,
            Self::Palette => ModeWord::Palette,
            Self::Dialog => ModeWord::Dialog,
            Self::Jobs => ModeWord::Jobs,
        }
    }
}

// ---------------------------------------------------------------------------- daemon link

/// The three daemon situations of §3.12, plus the two transient banners that follow case C.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonLink {
    /// A. Cold start: `Starting fleetd…`, full window, no chrome, nothing bound.
    Starting,
    /// B. `fleetd could not start.` Full window with the log tail and recovery keys.
    Failed {
        /// The failure, as reported by the client.
        message: String,
        /// The last lines of `~/.fleet/logs/fleetd.log`.
        log_tail: Vec<String>,
        /// Whether a stale socket is the known cause.
        stale_socket: bool,
    },
    /// Connected and answering pings.
    Connected,
    /// C. The daemon died while attached; the client is backing off between reconnects.
    Lost {
        /// How many reconnect attempts have failed.
        attempt: u32,
        /// Whether `Esc` dismissed the banner. The dot stays red either way.
        dismissed: bool,
    },
    /// The daemon came back. The wording depends on whether the PTYs died with it (§3.12 D-17).
    Reconnected {
        /// True when fleetd restarted, so terminal sessions did **not** survive.
        restarted: bool,
        /// When the banner appeared.
        since: Instant,
    },
}

impl DaemonLink {
    /// Whether the app is fully usable.
    #[must_use]
    pub const fn is_connected(&self) -> bool {
        matches!(self, Self::Connected | Self::Reconnected { .. })
    }

    /// Whether mutating keys must be refused and terminal grids veiled (§3.12 C).
    #[must_use]
    pub const fn is_lost(&self) -> bool {
        matches!(self, Self::Lost { .. })
    }
}

/// The reconnect backoff of §3.12 C: 1, 2, 4, 8 seconds, capped at 8.
#[must_use]
pub fn reconnect_backoff(attempt: u32) -> Duration {
    let seconds = 1_u64 << attempt.min(3);
    Duration::from_secs(seconds.min(8))
}

// ---------------------------------------------------------------------------- mirror grid

/// The client-side mirror of one daemon-owned terminal.
///
/// The daemon sends a full frame on attach and dirty-row diffs afterwards; this applies both
/// and is the only thing the terminal element paints from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorGrid {
    /// Column count of the mirrored grid.
    pub cols: u16,
    /// Row count of the mirrored grid.
    pub rows: u16,
    /// One row of cells per grid row, always exactly `rows` long.
    pub lines: Vec<Vec<Cell>>,
    /// Soft-wrap continuation flag for each grid row.
    pub wrapped: Vec<bool>,
    /// The cursor as of the last applied frame.
    pub cursor: CursorState,
    /// The scrollback viewport as of the last applied frame.
    pub viewport: ViewportInfo,
    /// The terminal modes as of the last applied frame.
    pub modes: TerminalModes,
    /// The most recent PTY title, when one was reported.
    pub title: Option<String>,
    /// The sequence number of the last applied frame.
    pub seq: u64,
    /// Whether a full frame has been applied yet.
    pub primed: bool,
    /// Whether frames were dropped since the last full one.
    ///
    /// A diff only describes the rows that changed *since the previous frame*, so once one is
    /// missed the mirror can never catch up on its own: the rows changed inside the gap are
    /// never re-sent. While this is set the last good frame stays on screen — it is still the
    /// best answer available — and every diff is refused until a full frame re-primes it.
    pub desynced: bool,
    /// `Some(code)` once the PTY exited; the grid then freezes at its last frame.
    pub exit_code: Option<Option<i32>>,
}

impl MirrorGrid {
    /// An empty grid of the given size.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            lines: vec![Vec::new(); rows as usize],
            wrapped: vec![false; rows as usize],
            cursor: CursorState {
                row: 0,
                col: 0,
                visible: true,
                shape: CursorShape::Block,
            },
            viewport: ViewportInfo {
                scrollback_len: 0,
                offset: 0,
                history_epoch: 0,
            },
            modes: TerminalModes::default(),
            title: None,
            seq: 0,
            primed: false,
            desynced: false,
            exit_code: None,
        }
    }

    /// Applies a frame update, returning whether anything changed.
    ///
    /// A diff frame that arrives before the first full frame, out of sequence, or after a
    /// dropped one, is refused: only a full frame can re-prime the mirror.
    pub fn apply(&mut self, frame: &FrameUpdate) -> bool {
        if self.primed && frame.seq <= self.seq {
            return false;
        }
        if !frame.full && self.primed && frame.seq > self.seq.saturating_add(1) {
            self.desynced = true;
        }
        // A shift can reuse rows only within the same grid and history identity. Recover
        // through a full frame rather than rotating stale cells across an epoch boundary.
        if !frame.full
            && frame.shift.is_some()
            && (frame.viewport.history_epoch != self.viewport.history_epoch
                || frame.cols != self.cols
                || frame.rows != self.rows
                || frame.modes.alt_screen != self.modes.alt_screen)
        {
            self.desynced = true;
        }
        if !frame.full && (!self.primed || self.desynced) {
            return false;
        }
        if frame.full {
            self.primed = true;
            self.desynced = false;
        }
        if frame.cols != self.cols || frame.rows != self.rows || frame.full {
            self.resize(frame.cols, frame.rows);
        }
        if !frame.full
            && let Some(shift) = frame.shift
        {
            let count = (shift.unsigned_abs() as usize).min(self.lines.len());
            if shift > 0 {
                self.lines.rotate_left(count);
                self.wrapped.rotate_left(count);
                self.wrapped[self.lines.len() - count..].fill(false);
                let start = self.lines.len() - count;
                for row in &mut self.lines[start..] {
                    row.clear();
                }
            } else {
                self.lines.rotate_right(count);
                self.wrapped.rotate_right(count);
                self.wrapped[..count].fill(false);
                for row in &mut self.lines[..count] {
                    row.clear();
                }
            }
        }
        for row in &frame.rows_changed {
            if let Some(line) = self.lines.get_mut(row.index as usize) {
                line.clone_from(&row.cells);
            }
            if let Some(wrapped) = self.wrapped.get_mut(row.index as usize) {
                *wrapped = row.wrapped;
            }
        }
        self.cursor = frame.cursor;
        self.viewport = frame.viewport;
        self.modes = frame.modes;
        if let Some(title) = &frame.title {
            self.title = Some(title.clone());
        }
        self.seq = frame.seq;
        true
    }

    /// Resizes the mirror, keeping the rows that survive.
    fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.lines.resize(rows as usize, Vec::new());
        self.wrapped.resize(rows as usize, false);
    }

    /// The cell at a position, or `None` outside the grid or past the end of a short row.
    #[must_use]
    pub fn cell(&self, row: u16, col: u16) -> Option<&Cell> {
        self.lines.get(row as usize)?.get(col as usize)
    }

    /// The plain text of one row, used by selection, search and tests.
    #[must_use]
    pub fn row_text(&self, row: u16) -> String {
        self.lines
            .get(row as usize)
            .map_or_else(String::new, |line| {
                line.iter()
                    .filter(|cell| cell.width != CellWidth::Spacer)
                    .map(|cell| cell.text.as_str())
                    .collect()
            })
    }
}

/// A blank cell, used when a row is shorter than the grid.
#[must_use]
pub fn blank_cell() -> Cell {
    Cell {
        text: " ".into(),
        fg: Color::Default,
        bg: Color::Default,
        underline_color: None,
        attrs: fleet_proto::terminal::CellAttrs::empty(),
        width: CellWidth::Narrow,
    }
}

// ---------------------------------------------------------------------------- MRU

/// A most-recently-used list. The front is the most recent entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mru<T> {
    entries: Vec<T>,
}

impl<T> Default for Mru<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<T: Clone + PartialEq> Mru<T> {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves `entry` to the front, capping the list.
    pub fn touch(&mut self, entry: T) {
        self.entries.retain(|existing| existing != &entry);
        self.entries.insert(0, entry);
        self.entries.truncate(MRU_CAPACITY);
    }

    /// Forgets an entry, e.g. a killed session or a closed terminal.
    pub fn forget(&mut self, entry: &T) {
        self.entries.retain(|existing| existing != entry);
    }

    /// Keeps only the entries a predicate accepts — used to reconcile with a fresh snapshot.
    pub fn retain(&mut self, keep: impl Fn(&T) -> bool) {
        self.entries.retain(|entry| keep(entry));
    }

    /// The current entry.
    #[must_use]
    pub fn current(&self) -> Option<&T> {
        self.entries.first()
    }

    /// The alternate entry: what `ctrl-s w` and `ctrl-s Tab` jump to.
    #[must_use]
    pub fn alternate(&self) -> Option<&T> {
        self.entries.get(1)
    }

    /// The entries, most recent first.
    #[must_use]
    pub fn entries(&self) -> &[T] {
        &self.entries
    }
}

// ---------------------------------------------------------------------------- toasts

/// A toast plus the instants that govern its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveToast {
    /// The kit toast being rendered.
    pub toast: Toast,
    /// When it first appeared. Coalescing compares against this.
    pub shown_at: Instant,
    /// When it must be removed.
    pub expires_at: Instant,
}

/// Pushes a toast, applying the §2.7 law: identical text within one second coalesces into
/// `×n`, and the stack never exceeds [`MAX_TOASTS`].
///
/// Errors are never toasts (§1.8); a `Danger` tone is downgraded to `Warning` so a caller
/// cannot smuggle one in.
pub fn push_toast(toasts: &mut Vec<LiveToast>, toast: Toast, now: Instant, dwell: Duration) {
    let mut toast = toast;
    if toast.tone == Tone::Danger {
        toast.tone = Tone::Warning;
    }
    if let Some(existing) = toasts
        .iter_mut()
        .find(|live| live.toast.text == toast.text && now - live.shown_at <= TOAST_COALESCE_WINDOW)
    {
        existing.toast.count += 1;
        existing.expires_at = now + dwell;
        return;
    }
    toasts.push(LiveToast {
        toast,
        shown_at: now,
        expires_at: now + dwell,
    });
    while toasts.len() > MAX_TOASTS {
        toasts.remove(0);
    }
}

/// Removes expired toasts, returning whether anything was removed.
pub fn expire_toasts(toasts: &mut Vec<LiveToast>, now: Instant) -> bool {
    let before = toasts.len();
    toasts.retain(|live| live.expires_at > now);
    toasts.len() != before
}

// ---------------------------------------------------------------------------- sticky error

/// The status bar's sticky error slot: the last failed job, addressable with `!` (§1.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickyError {
    /// The one-line message.
    pub text: String,
    /// The failed job, when the error came from one.
    pub job: Option<JobId>,
    /// Whether `R` can retry it.
    pub retryable: bool,
}

/// The newest failed job, which owns the sticky error slot.
#[must_use]
pub fn latest_failed_job(jobs: &[JobRecord]) -> Option<&JobRecord> {
    jobs.iter()
        .filter(|job| matches!(job.status, JobStatus::Failed { .. }))
        .max_by(|left, right| left.finished_at.cmp(&right.finished_at))
}

/// Every job that is queued, running or cancelling — the work the quit dialogs enumerate.
#[must_use]
pub fn running_jobs(jobs: &[JobRecord]) -> Vec<&JobRecord> {
    jobs.iter()
        .filter(|job| {
            matches!(
                job.status,
                JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
            )
        })
        .collect()
}

/// Extracts a percentage from a job progress line, e.g. `Receiving objects: 40% (81/202)`.
#[must_use]
pub fn parse_percent(progress: &str) -> Option<u8> {
    let (head, _) = progress.split_once('%')?;
    let digits: String = head
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse::<u16>().ok().map(|value| value.min(100) as u8)
}

// ---------------------------------------------------------------------------- context-bar chips

/// The zero-suppressed counters of §2.3.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChipCounts {
    /// Running jobs.
    pub running: usize,
    /// Failed jobs that have not been seen.
    pub failed: usize,
    /// Attached sessions.
    pub live: usize,
    /// Detached sessions, awake or slept.
    pub sleeping: usize,
    /// Unknown sessions plus unreachable hosts. Never collapsed into another chip (§D-1).
    pub unknown: usize,
    /// Pull requests waiting for review.
    pub review: usize,
}

/// Counts the context-bar chips from the parts of the snapshot that feed them.
#[must_use]
pub fn chip_counts(
    jobs: &[JobRecord],
    sessions: &[SessionState],
    hosts: &[HostStatus],
    review_prs: usize,
) -> ChipCounts {
    ChipCounts {
        running: running_jobs(jobs).len(),
        failed: jobs
            .iter()
            .filter(|job| matches!(job.status, JobStatus::Failed { .. }))
            .count(),
        live: sessions
            .iter()
            .filter(|state| **state == SessionState::Attached)
            .count(),
        sleeping: sessions
            .iter()
            .filter(|state| **state == SessionState::Detached)
            .count(),
        unknown: sessions
            .iter()
            .filter(|state| **state == SessionState::Unknown)
            .count()
            + hosts.iter().filter(|host| !host.reachable).count(),
        review: review_prs,
    }
}

// ---------------------------------------------------------------------------- lists

/// Which repository the lists are scoped to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RepoScope {
    /// The `All` pseudo-repo.
    #[default]
    All,
    /// One repository.
    Repo(RepoId),
}

/// One cursor per list, so switching panes and screens never loses a position (§1.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cursors {
    /// The repos rail.
    pub repos: usize,
    /// The worktrees list.
    pub worktrees: usize,
    /// The `Mine` PR tab.
    pub prs_mine: usize,
    /// The `Review` PR tab.
    pub prs_review: usize,
    /// The jobs panel.
    pub jobs: usize,
}

/// Moves a cursor by `delta` rows, clamped to the list. Background events never move it (§5.11).
#[must_use]
pub fn move_cursor(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let last = len - 1;
    let next = current as isize + delta;
    next.clamp(0, last as isize) as usize
}

/// Clamps a cursor after the list behind it changed length.
#[must_use]
pub fn clamp_cursor(current: usize, len: usize) -> usize {
    if len == 0 { 0 } else { current.min(len - 1) }
}

/// How many rows `ctrl-d` and `ctrl-u` move, given the rows a pane can show.
#[must_use]
pub fn half_page(visible_rows: usize) -> isize {
    (visible_rows.max(2) / 2) as isize
}

/// The filter of the focused list (§3.10).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterState {
    /// The live query. An empty query means no filter.
    pub query: String,
    /// Whether the input still owns the keyboard. `false` means the filter is retained.
    pub editing: bool,
}

impl FilterState {
    /// Whether rows are currently being filtered.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.query.is_empty()
    }
}

/// What `Esc` does in filter mode. The two stages are the reason `Esc` never quits (§A13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterEscape {
    /// Leave the input, keep the filter, and show the retained `⌕query` chip.
    LeaveInput,
    /// Clear the filter and restore the pane header.
    ClearFilter,
}

/// The two-stage `Esc` of §3.10.
#[must_use]
pub const fn filter_escape(editing: bool) -> FilterEscape {
    if editing {
        FilterEscape::LeaveInput
    } else {
        FilterEscape::ClearFilter
    }
}

// ---------------------------------------------------------------------------- app state

/// The whole client-side state of the app.
#[derive(Debug)]
pub struct AppState {
    /// `$FLEET_HOME`.
    pub home: PathBuf,
    /// The daemon connection situation (§3.12).
    pub daemon: DaemonLink,
    /// When the current [`DaemonLink`] was entered, for the splash and banner timings.
    pub daemon_since: Instant,
    /// The sibling/on-path fleetd binary is newer than the connected daemon process.
    pub daemon_outdated: bool,
    /// The authoritative daemon snapshot, absent until the first one arrives.
    pub snapshot: Option<Snapshot>,
    /// Once a populated daemon state has been observed, FirstRun may never reappear.
    ///
    /// This is deliberately monotonic: a delayed, partial, or stale empty snapshot must not put
    /// an established app back on the migration card where a terminal's bare `i` means import.
    has_seen_non_empty_state: bool,
    /// When the snapshot was received, which is what `stale · <age>` ages (§1.3).
    pub snapshot_at: Option<Instant>,
    /// One mirror grid per attached terminal.
    pub grids: HashMap<TerminalId, MirrorGrid>,
    /// Read-only child output and per-session pane preferences.
    pub watches: crate::watches::Watches,
    /// The screen being shown.
    pub screen: Screen,
    /// Board view and presentation state for the active context.
    pub board: BoardState,
    /// Whether the board needs an authoritative refresh.
    pub board_stale: bool,
    /// Invalidates asynchronous responses when the context or connection changes.
    board_generation: u64,
    /// The backend kinds this daemon registers, from `ListBoardBackends`.
    ///
    /// The app knows no backend by name: the header's label, the settings dialog's kind cycler
    /// and every settings row it draws come from these descriptors, so a backend the daemon
    /// adds needs no change here at all.
    pub board_backends: Vec<BackendDescriptor>,
    /// Whether a `ListBoardBackends` request has already been issued for this connection.
    ///
    /// Set when the request goes out, not when it answers: the board re-renders on every
    /// frame, and a flag cleared by a failure would ask the daemon again sixty times a second.
    board_backends_asked: bool,
    /// Which Hub pane owns the cursor.
    pub hub_pane: HubPane,
    /// Which PR tab is selected.
    pub pr_tab: PrTab,
    /// The repository the lists are scoped to.
    pub scope: RepoScope,
    /// One cursor per list.
    pub cursors: Cursors,
    /// The Workspace sub-mode.
    pub terminal_mode: TerminalMode,
    /// The floating agent popup, independent of the base Hub or Workspace screen.
    pub agent_popup: Option<AgentPopupState>,
    /// Effective history and wheel configuration.
    pub terminal_config: fleet_core::config::TerminalConfig,
    /// Enabled channels for agent-finished notifications.
    pub notifications: NotificationsConfig,
    /// The overlay that owns the keyboard, when any.
    pub overlay: Option<Overlay>,
    /// The filter of the focused list.
    pub filter: FilterState,
    /// Whether the detail panel is open (`i`). Never focusable.
    pub detail_open: bool,
    /// Whether the repos rail is collapsed to its 44 px icon rail (`H`).
    pub rail_collapsed: bool,
    /// Whether the Workspace hides its header and tab strip (`ctrl-s z`).
    pub zoomed: bool,
    /// Session MRU: `ctrl-s w` jumps to [`Mru::alternate`].
    pub session_mru: Mru<SessionId>,
    /// Per-session terminal MRU: `ctrl-s Tab` jumps to [`Mru::alternate`].
    pub terminal_mru: HashMap<SessionId, Mru<TerminalId>>,
    /// The live toasts, oldest first.
    pub toasts: Vec<LiveToast>,
    /// Most recently observed aggregate activity and when that state was first seen, per session.
    last_agent_activity: HashMap<SessionId, (AgentActivity, Instant)>,
    /// Audible completion signal, replaceable by a recording implementation in tests.
    notification_sound: Box<dyn NotificationSound>,
    /// The sticky error slot.
    pub sticky_error: Option<StickyError>,
    /// Failed jobs whose sticky slot was acknowledged by opening Jobs.
    pub seen_failed: Vec<JobId>,
    /// Failed job the next Jobs opening should focus, even after acknowledging its sticky slot.
    pub jobs_focus: Option<JobId>,
    /// Initial palette query consumed when the palette next opens.
    pub palette_seed: Option<String>,
    /// Last worktree trash entry returned by fleetd, for `u`.
    pub last_trash_entry: Option<String>,
    /// Pull-request badges shared by Hub and Workspace, keyed by repository and head branch.
    pub pr_badges: HashMap<(RepoId, String), (u64, PrBadgeState)>,
    /// `config.jobs.warnBeforeQuit`, mirrored so `ctrl-q` can decide without a round trip.
    pub warn_before_quit: bool,
    /// How many PRs the `review` tab holds. The PR screen owns the fetch, the context bar
    /// owns the chip, so the count is published here (§2.3).
    pub review_pr_count: usize,
    /// The version of an available Fleet update, for the `↑<version>` chip (§2.3, D-2).
    pub update_version: Option<String>,
    /// The last element of the status-bar breadcrumb: the cursor row of the focused list.
    /// Screens publish it; the shell renders `context › repo › row` (§2.2).
    pub breadcrumb_row: Option<String>,
    /// Terminals the user renamed with `ctrl-s ,`.
    ///
    /// §3.6 lets a program's OSC title name its tab **until the terminal is explicitly
    /// renamed**, and the daemon's `Terminal` record has no "was renamed" flag to arbitrate
    /// with — so the client remembers its own renames and lets them win.
    pub renamed_terminals: HashSet<TerminalId>,
    /// How many times the client's link to fleetd has been (re)established.
    ///
    /// A reconnect builds a brand-new `Client` **and** a brand-new daemon-side `Connection`
    /// whose `attached` set starts empty, so every attachment the app believes it holds is
    /// silently gone and frames stop arriving. Screens record the generation they attached
    /// under and re-issue their `AttachTerminal` when this number has moved.
    pub link_generation: u64,
    /// The answer to the last `D`, which is what puts the app on the §3.12 `Daemon > Doctor`
    /// surface. It lives here rather than on the shell because `D` is raised from two places —
    /// the daemon-down splash and Settings › About — and both must land on the same screen.
    pub doctor: Option<Vec<fleet_proto::response::DoctorCheck>>,
}

impl AppState {
    /// The state the app starts in: cold start, Hub, worktrees, nothing open.
    #[must_use]
    pub fn new(home: impl Into<PathBuf>, now: Instant) -> Self {
        Self {
            home: home.into(),
            daemon: DaemonLink::Starting,
            daemon_since: now,
            daemon_outdated: false,
            snapshot: None,
            has_seen_non_empty_state: false,
            snapshot_at: None,
            grids: HashMap::new(),
            watches: crate::watches::Watches::default(),
            screen: Screen::hub(),
            board: BoardState::default(),
            board_stale: true,
            board_generation: 0,
            board_backends: Vec::new(),
            board_backends_asked: false,
            hub_pane: HubPane::List,
            pr_tab: PrTab::Mine,
            scope: RepoScope::All,
            cursors: Cursors::default(),
            terminal_mode: TerminalMode::Terminal,
            agent_popup: None,
            terminal_config: fleet_core::config::TerminalConfig::default(),
            notifications: NotificationsConfig::default(),
            overlay: None,
            filter: FilterState::default(),
            detail_open: false,
            rail_collapsed: false,
            zoomed: false,
            session_mru: Mru::new(),
            terminal_mru: HashMap::new(),
            toasts: Vec::new(),
            last_agent_activity: HashMap::new(),
            notification_sound: Box::new(SystemSound),
            sticky_error: None,
            seen_failed: Vec::new(),
            jobs_focus: None,
            palette_seed: None,
            last_trash_entry: None,
            pr_badges: HashMap::new(),
            warn_before_quit: true,
            review_pr_count: 0,
            update_version: None,
            breadcrumb_row: None,
            renamed_terminals: HashSet::new(),
            link_generation: 0,
            doctor: None,
        }
    }

    /// The loaded board for the active context.
    #[must_use]
    pub fn board(&self) -> Option<&BoardView> {
        self.board.view.as_ref()
    }

    /// Applies an authoritative board view; responses for another context are ignored.
    pub fn apply_board_view(&mut self, view: BoardView) {
        if self.active_context() != Some(&view.board.context_id) {
            return;
        }
        self.board_stale = false;
        self.board.view = Some(view);
        self.board.loading = false;
        self.board.error = None;
        self.clamp_board_focus();
    }

    /// Upserts a card only into its currently loaded board.
    pub fn apply_card(&mut self, card: Card) {
        let Some(view) = self.board.view.as_mut() else {
            return;
        };
        if view.board.id != card.board_id {
            return;
        }
        if !view
            .board
            .statuses
            .iter()
            .any(|status| status.id == card.status_id)
        {
            self.board_stale = true;
            return;
        }
        if let Some(current) = view.cards.iter_mut().find(|current| current.id == card.id) {
            *current = card;
        } else {
            view.cards.push(card);
        }
        view.cards.sort_by(|a, b| {
            (&a.status_id, a.position, &a.created_at, a.number).cmp(&(
                &b.status_id,
                b.position,
                &b.created_at,
                b.number,
            ))
        });
        self.clamp_board_focus();
    }

    /// Clears board data and invalidates requests from the previous context or connection.
    pub fn clear_board(&mut self) {
        if matches!(
            self.overlay,
            Some(Overlay::Dialog(
                Dialogs::CardDetail
                    | Dialogs::CardCreate
                    | Dialogs::CardPicker
                    | Dialogs::BoardSettings
            ))
        ) {
            self.close_overlay();
        }
        self.board = BoardState::default();
        self.board_stale = true;
        self.board_generation = self.board_generation.wrapping_add(1);
        // A reconnect can land on a different fleetd with a different registry, and the
        // descriptors are what the header and the settings dialog are drawn from. The list
        // itself is kept until a newer one arrives so the header's label does not flicker.
        self.board_backends_asked = false;
    }

    /// Whether a `ListBoardBackends` request should go out now, marking it as issued.
    pub(crate) fn begin_backends_load(&mut self) -> bool {
        if self.board_backends_asked || self.refuses_mutations() {
            return false;
        }
        self.board_backends_asked = true;
        true
    }

    /// Adopts the daemon's backend registry.
    pub fn apply_backends(&mut self, backends: Vec<BackendDescriptor>) {
        self.board_backends = backends;
    }

    /// Releases the once-per-connection guard so a failed ask can be made again.
    ///
    /// The flag is set before the request, so without this one refused `ListBoardBackends`
    /// leaves the header showing the raw kind and the settings dialog with no backend rows for
    /// the rest of the connection — a permanent consequence of a momentary failure.
    pub(crate) fn backends_load_failed(&mut self) {
        self.board_backends_asked = false;
    }

    /// The descriptor of one backend kind, when the daemon registers it.
    #[must_use]
    pub fn backend_descriptor(&self, kind: &str) -> Option<&BackendDescriptor> {
        self.board_backends
            .iter()
            .find(|descriptor| descriptor.kind == kind)
    }

    /// The human name of a backend kind, falling back to the raw key.
    ///
    /// The fallback matters on the first frames of a connection and against an older daemon:
    /// a header that draws nothing at all where the backend goes reads as a broken board.
    #[must_use]
    pub fn backend_label(&self, kind: &str) -> String {
        self.backend_descriptor(kind)
            .map_or_else(|| kind.to_owned(), |descriptor| descriptor.label.clone())
    }

    /// The standard card fields this board's backend cannot write back.
    ///
    /// Empty on a local board, exactly as `fleet_core::board::ops` reads it: a local board
    /// declares no backend, so nothing it holds is read-only.
    #[must_use]
    pub fn readonly_fields(&self) -> &[String] {
        self.board()
            .filter(|view| !view.board.backend.is_local())
            .map_or(&[][..], |view| &view.board.sync.readonly_fields)
    }

    /// Whether the daemon would refuse a local edit to `field` on the shown board.
    #[must_use]
    pub fn is_readonly_field(&self, field: &str) -> bool {
        self.readonly_fields()
            .iter()
            .any(|readonly| readonly == field)
    }

    /// Claims one load; an error waits for reload instead of retrying every render.
    pub(crate) fn begin_board_load(&mut self) -> Option<(ContextId, u64)> {
        if !matches!(self.screen, Screen::Hub { tab: HubTab::Board }) {
            return None;
        }
        if self.refuses_mutations() {
            // "Cold" means a load is in flight. With the daemon gone none ever will be, so the
            // board says why instead of drawing skeleton columns forever.
            if self.board.view.is_none() {
                self.board.error = Some("fleetd is not reachable".to_owned());
            }
            return None;
        }
        let Some(context) = self.active_context().cloned() else {
            // Every board is `EnsureBoard(active_context)`, so with no active context there is
            // nothing to ask for and skeleton columns would promise a load that never goes
            // out. `apply_snapshot` clears the board the moment one is activated, which drops
            // this message and makes the load stale again.
            if self.board.view.is_none() {
                self.board.error = Some(NO_ACTIVE_CONTEXT.to_owned());
            }
            return None;
        };
        if self.board.loading || !self.board_stale {
            return None;
        }
        self.board.loading = true;
        self.board.error = None;
        self.board_stale = false;
        Some((context, self.board_generation))
    }

    /// Completes a load only if its context and generation still own the board slot.
    pub(crate) fn finish_board_load(
        &mut self,
        context: &ContextId,
        generation: u64,
        result: Result<BoardView, String>,
    ) {
        if generation != self.board_generation || self.active_context() != Some(context) {
            return;
        }
        self.board.loading = false;
        match result {
            Ok(view) if &view.board.context_id == context => {
                let stale = self.board_stale;
                self.apply_board_view(view);
                self.board_stale = stale;
            }
            Ok(_) => self.board.error = Some("EnsureBoard returned a different context".into()),
            Err(error) => self.board.error = Some(error),
        }
    }

    /// Keeps the board selection inside the columns and rows that are actually drawn.
    ///
    /// The filter is part of that: a selection that indexes a hidden card is a selection the
    /// user cannot see, and every key that acts on "the focused card" would act on the wrong
    /// one.
    pub(crate) fn clamp_board_focus(&mut self) {
        let Some(view) = self.board.view.as_ref() else {
            return;
        };
        self.board.focus.column = clamp_cursor(self.board.focus.column, view.board.statuses.len());
        let len = view
            .board
            .statuses
            .get(self.board.focus.column)
            .map_or(0, |status| {
                crate::views::board_screen::visible_cards(view, &status.id, &self.board.filter)
                    .len()
            });
        self.board.focus.row = clamp_cursor(self.board.focus.row, len);
    }

    /// Whether the board's filter input, rather than the board itself, owns the keyboard.
    #[must_use]
    pub fn board_filter_owns_keys(&self) -> bool {
        self.overlay.is_none()
            && self.agent_popup.is_none()
            && matches!(self.screen, Screen::Hub { tab: HubTab::Board })
            && self.board.filter_editing
    }

    /// `Esc` on the board: leave the filter input, then clear the filter (§3.10, [D-15]).
    ///
    /// Returns whether it consumed the key; when it did not, `Esc` belongs to whoever owns the
    /// surface behind the board.
    pub fn board_filter_escape(&mut self) -> bool {
        match filter_escape(self.board.filter_editing) {
            FilterEscape::LeaveInput => {
                self.board.filter_editing = false;
                true
            }
            FilterEscape::ClearFilter if self.board.is_filtered() => {
                self.board.filter.clear();
                self.clamp_board_focus();
                true
            }
            FilterEscape::ClearFilter => false,
        }
    }

    /// The nested key contexts of the focused element, outermost first.
    ///
    /// [`crate::keymap`] predicates are written against exactly this chain, which is why the
    /// Hub's panes are `Hub > Repos` and a dialog is `Dialog > <name>`. The daemon banner is
    /// appended **innermost** so its `r` / `l` / `Esc` outrank the Hub's while it is showing,
    /// and dismissing it (`Esc`) gives them straight back.
    #[must_use]
    pub fn context_chain(&self) -> Vec<&'static str> {
        // An open overlay owns the keyboard on every base surface — the first-run card and the
        // §3.12 daemon splashes included. `apply_bridge_event` closes what was open when the
        // link *fails*, but nothing stops one being opened afterwards, and `ctrl-q` does
        // exactly that: it opens §3.8.8 whenever a job was running in the last snapshot. An
        // overlay whose keys are not in the chain answers nothing, and `Esc` cannot leave it.
        if let Some(overlay) = &self.overlay {
            return overlay.context_chain();
        }
        // §3.12 B replaces the whole window, so its keys outrank every base surface's.
        if matches!(self.daemon, DaemonLink::Failed { .. }) {
            return vec!["Daemon", "Down"];
        }
        if self.is_first_run() {
            return vec!["FirstRun"];
        }
        if let Some(popup) = self.agent_popup {
            let mut chain = vec![
                "Agent",
                match popup.mode {
                    AgentPopupMode::Terminal => "Terminal",
                    AgentPopupMode::Prefix => "Prefix",
                    AgentPopupMode::Scroll => "Scroll",
                },
            ];
            if let DaemonLink::Lost {
                dismissed: false, ..
            } = self.daemon
            {
                chain.extend_from_slice(&["Daemon", "Banner"]);
            }
            return chain;
        }
        // §3.10 on the board: while its filter input owns the keyboard the bare letters must
        // type, so the board publishes the same `Filter` context the Hub's filter overlay does
        // instead of `Hub > Board`. Nothing else can shadow a whole context's letters.
        if self.board_filter_owns_keys() {
            return vec!["Filter", "BoardFilter"];
        }
        let mut chain = match &self.screen {
            Screen::Hub { tab } => vec![
                "Hub",
                match (self.hub_pane, tab) {
                    (_, HubTab::Board) => "Board",
                    (HubPane::Repos, _) => "Repos",
                    (HubPane::List, HubTab::Worktrees) => "Worktrees",
                    (HubPane::List, HubTab::Prs) => "Prs",
                },
            ],
            Screen::Workspace { .. } => vec![
                "Workspace",
                match self.terminal_mode {
                    TerminalMode::Terminal => "Terminal",
                    TerminalMode::Native => "Native",
                    TerminalMode::Prefix => "Prefix",
                    TerminalMode::Scroll => "Scroll",
                },
            ],
        };
        if let DaemonLink::Lost {
            dismissed: false, ..
        } = self.daemon
        {
            chain.extend_from_slice(&["Daemon", "Banner"]);
        }
        chain
    }

    /// The mode word for the status bar (§2.8).
    #[must_use]
    pub fn mode(&self) -> Mode {
        if let Some(overlay) = &self.overlay {
            return match overlay {
                Overlay::Filter => Mode::Filter,
                Overlay::Palette => Mode::Palette,
                Overlay::Jobs => Mode::Jobs,
                Overlay::Dialog(_) => Mode::Dialog,
            };
        }
        if let Some(popup) = self.agent_popup {
            return match popup.mode {
                AgentPopupMode::Terminal => Mode::Terminal,
                AgentPopupMode::Prefix => Mode::Prefix,
                AgentPopupMode::Scroll => Mode::Scroll,
            };
        }
        if self.board_filter_owns_keys() {
            return Mode::Filter;
        }
        match self.screen {
            Screen::Hub { .. } => Mode::Normal,
            Screen::Workspace { .. } => match self.terminal_mode {
                TerminalMode::Terminal => Mode::Terminal,
                TerminalMode::Native => Mode::Native,
                TerminalMode::Prefix => Mode::Prefix,
                TerminalMode::Scroll => Mode::Scroll,
            },
        }
    }

    /// Whether the first-run card replaces the whole window (§3.13).
    #[must_use]
    pub fn is_first_run(&self) -> bool {
        !self.has_seen_non_empty_state
            && self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.contexts.is_empty()
                    && snapshot.repos.is_empty()
                    && snapshot.clones.is_empty()
            })
    }

    /// Whether keys typed into a terminal grid must be dropped rather than buffered (§3.12 C).
    #[must_use]
    pub const fn drops_terminal_keys(&self) -> bool {
        self.daemon.is_lost()
    }

    /// Whether a mutating key should flash the banner instead of acting (§3.12 C).
    #[must_use]
    pub const fn refuses_mutations(&self) -> bool {
        self.daemon.is_lost()
    }

    /// Enters the one-shot prefix mode.
    pub fn enter_prefix(&mut self) {
        if matches!(self.screen, Screen::Workspace { .. }) {
            self.terminal_mode = TerminalMode::Prefix;
        }
    }

    /// Opens, switches, or hides the floating agent popup without changing the base screen.
    pub fn toggle_agent_popup(&mut self, agent: Agent) -> AgentPopupTransition {
        match self.agent_popup {
            Some(current) if current.agent == agent => {
                self.agent_popup = None;
                AgentPopupTransition::Hidden
            }
            Some(_) => {
                self.agent_popup = Some(AgentPopupState {
                    agent,
                    mode: AgentPopupMode::Terminal,
                    prefix_return: AgentPopupMode::Terminal,
                });
                AgentPopupTransition::Switched
            }
            None => {
                self.agent_popup = Some(AgentPopupState {
                    agent,
                    mode: AgentPopupMode::Terminal,
                    prefix_return: AgentPopupMode::Terminal,
                });
                AgentPopupTransition::Opened
            }
        }
    }

    /// Hides the floating agent surface. The daemon session is intentionally untouched.
    pub fn hide_agent_popup(&mut self) -> bool {
        self.agent_popup.take().is_some()
    }

    /// Enters the popup's one-shot prefix mode.
    pub fn enter_agent_prefix(&mut self) {
        if let Some(popup) = &mut self.agent_popup {
            popup.prefix_return = popup.mode;
            popup.mode = AgentPopupMode::Prefix;
        }
    }

    /// Leaves the popup prefix after its one following key.
    pub fn leave_agent_prefix(&mut self) -> bool {
        let Some(popup) = &mut self.agent_popup else {
            return false;
        };
        if popup.mode == AgentPopupMode::Prefix {
            popup.mode = popup.prefix_return;
            true
        } else {
            false
        }
    }

    /// The daemon session currently selected by the floating agent popup.
    #[must_use]
    pub fn agent_popup_session(&self) -> Option<&Session> {
        let agent = self.agent_popup?.agent;
        let id = fleet_core::sessions::agent_session_id(agent).ok()?;
        self.snapshot
            .as_ref()?
            .sessions
            .iter()
            .find(|session| session.id == id)
    }

    /// Latest aggregate activity for a daemon session, shared by Workspace status and the popup.
    #[must_use]
    pub fn session_agent_activity(&self, session: &SessionId) -> AgentActivity {
        self.last_agent_activity
            .get(session)
            .map_or(AgentActivity::Unknown, |(activity, _)| *activity)
    }

    /// The mirror grid of the popup agent's first terminal.
    #[must_use]
    pub fn agent_popup_grid(&self) -> Option<&MirrorGrid> {
        let terminal = self.agent_popup_session()?.terminals.first()?.id;
        self.grids.get(&terminal)
    }

    /// The selected popup agent whose fixed runtime session is absent from a reachable daemon.
    ///
    /// The shell uses this as the pure half of its single-flight recovery gate after reconnects
    /// and authoritative snapshots that remove an agent session.
    #[must_use]
    pub fn missing_agent_popup_session(&self) -> Option<Agent> {
        let popup = self.agent_popup?;
        if !self.daemon.is_connected() {
            return None;
        }
        let session = fleet_core::sessions::agent_session_id(popup.agent).ok()?;
        self.snapshot
            .as_ref()?
            .sessions
            .iter()
            .all(|candidate| candidate.id != session)
            .then_some(popup.agent)
    }

    /// Leaves the prefix, whatever the key was. Called for **every** key seen in `Prefix`,
    /// which is what makes the mode one-shot without a timeout.
    ///
    /// Returns whether the mode actually changed.
    pub fn leave_prefix(&mut self) -> bool {
        if self.terminal_mode == TerminalMode::Prefix {
            self.terminal_mode = self.resting_terminal_mode();
            true
        } else {
            false
        }
    }

    /// The mode a Workspace tab rests in: what owns the keyboard when no Fleet mode is active.
    #[must_use]
    pub fn resting_terminal_mode(&self) -> TerminalMode {
        if self.active_terminal_is_native() {
            TerminalMode::Native
        } else {
            TerminalMode::Terminal
        }
    }

    /// Whether the active session's active tab is drawn by Fleet rather than by a PTY.
    #[must_use]
    pub fn active_terminal_is_native(&self) -> bool {
        self.active_terminal_record()
            .is_some_and(fleet_core::sessions::Terminal::is_native)
    }

    /// The active session's active terminal record, straight from the snapshot.
    #[must_use]
    pub fn active_terminal_record(&self) -> Option<&fleet_core::sessions::Terminal> {
        let session = self.active_session()?;
        let active = session.active_terminal?;
        session
            .terminals
            .iter()
            .find(|terminal| terminal.id == active)
    }

    /// Re-derives the resting mode after a snapshot moved, created or replaced the active tab.
    ///
    /// `Prefix` and `Scroll` are transient Fleet modes the user is standing in; a snapshot must
    /// not yank them away underneath. Everything else follows the tab.
    pub fn sync_terminal_mode(&mut self) {
        if matches!(
            self.terminal_mode,
            TerminalMode::Terminal | TerminalMode::Native
        ) {
            self.terminal_mode = self.resting_terminal_mode();
        }
    }

    /// `^s v` toggles only local watch visibility and always returns to terminal mode.
    pub fn toggle_watch_pane(&mut self, now: Instant) {
        self.leave_prefix();
        if let Some(session) = self.active_session().map(|s| s.id.clone())
            && !self.watches.toggle(&session)
        {
            self.toast_short("no subagent watches", Icon::Info, now);
        }
    }

    /// `^s N/P` shows and cycles watches without changing the active terminal.
    pub fn cycle_watch(&mut self, forward: bool, now: Instant) {
        self.leave_prefix();
        if let Some(session) = self.active_session().map(|s| s.id.clone())
            && !self.watches.cycle(&session, forward)
        {
            self.toast_short("no subagent watches", Icon::Info, now);
        }
    }

    /// `^s V` / ×: returns an exited watch to dismiss; a running watch is only hidden.
    pub fn close_selected_watch(&mut self, now: Instant) -> Option<fleet_core::watches::WatchId> {
        self.leave_prefix();
        let session = self.active_session()?.id.clone();
        let id = self.watches.panes.get(&session)?.selected?;
        if self.watches.entries.get(&id)?.watch.status == fleet_core::watches::WatchStatus::Running
        {
            self.watches.hide(&session);
            self.toast_short("watch still running; pane hidden", Icon::Info, now);
            return None;
        }
        Some(id)
    }

    /// Opens an overlay, replacing whatever was open.
    pub fn open_overlay(&mut self, overlay: Overlay) {
        if matches!(overlay, Overlay::Jobs) {
            self.jobs_focus = self
                .sticky_error
                .as_ref()
                .and_then(|error| error.job.clone());
            if let Some(job) = self.jobs_focus.clone()
                && !self.seen_failed.contains(&job)
            {
                self.seen_failed.push(job);
            }
            if let Some(snapshot) = self.snapshot.as_ref() {
                self.sticky_error =
                    crate::views::sticky_error::sticky_error_for(&snapshot.jobs, &self.seen_failed);
            }
        }
        self.overlay = Some(overlay);
    }

    /// Closes the topmost overlay, returning whether one was open.
    pub fn close_overlay(&mut self) -> bool {
        self.overlay.take().is_some()
    }

    /// `Esc` outside a dialog: clear the filter if any, else close the topmost overlay, else
    /// nothing. It never quits (§A13).
    pub fn cancel(&mut self) -> bool {
        if self.overlay.is_none()
            && self.agent_popup.is_none()
            && matches!(self.screen, Screen::Hub { tab: HubTab::Board })
            && self.board_filter_escape()
        {
            return true;
        }
        if self.filter.is_active() || self.filter.editing {
            match filter_escape(self.filter.editing) {
                FilterEscape::LeaveInput => {
                    self.filter.editing = false;
                    if matches!(self.overlay, Some(Overlay::Filter)) {
                        self.overlay = None;
                    }
                }
                FilterEscape::ClearFilter => self.filter.query.clear(),
            }
            return true;
        }
        self.close_overlay()
    }

    /// Records a toast under the §2.7 law.
    pub fn toast(&mut self, toast: Toast, now: Instant, dwell: Duration) {
        push_toast(&mut self.toasts, toast, now, dwell);
    }

    /// Records a short clipboard-style acknowledgement.
    pub fn toast_short(&mut self, text: impl Into<gpui::SharedString>, icon: Icon, now: Instant) {
        self.toast(
            Toast::new(text).icon(icon).short(),
            now,
            Duration::from_millis(1_600),
        );
    }

    /// Applies a daemon toast event. Errors are sticky, never transient (§1.8).
    pub fn apply_toast_event(&mut self, level: ToastLevel, message: String, now: Instant) {
        match level {
            ToastLevel::Error => {
                self.sticky_error = Some(StickyError {
                    text: message,
                    job: None,
                    retryable: false,
                });
            }
            ToastLevel::Warning => self.toast(
                Toast::new(message).icon(Icon::Info).tone(Tone::Warning),
                now,
                dwell_for(ToastDuration::Normal),
            ),
            ToastLevel::Info => self.toast(
                Toast::new(message).icon(Icon::Info),
                now,
                dwell_for(ToastDuration::Normal),
            ),
        }
    }

    /// Replaces agent-activity baselines without presenting historical completions.
    fn seed_agent_activity(&mut self, snapshot: &Snapshot, now: Instant) {
        self.last_agent_activity = snapshot_agent_activities(snapshot)
            .into_iter()
            .map(|(session, activity)| (session, (activity, now)))
            .collect();
    }

    /// Records one aggregate activity observation and returns whether it completed real work.
    fn observe_agent_activity(
        &mut self,
        session: SessionId,
        activity: AgentActivity,
        now: Instant,
    ) -> bool {
        let finished = self
            .last_agent_activity
            .get(&session)
            .is_some_and(|(previous, seen_at)| {
                *previous == AgentActivity::Working
                    && activity == AgentActivity::Idle
                    && now.saturating_duration_since(*seen_at) >= AGENT_FINISH_MIN_WORKING
            });
        match self.last_agent_activity.get_mut(&session) {
            Some((previous, _)) if *previous == activity => {}
            Some(entry) => *entry = (activity, now),
            None => {
                self.last_agent_activity.insert(session, (activity, now));
            }
        }
        finished
    }

    /// Presents an agent completion through each enabled notification channel.
    fn notify_agent_finished(&mut self, label: &str, now: Instant) {
        if self.notifications.toast {
            self.toast(
                Toast::new(format!("{label}: agent finished"))
                    .icon(Icon::CircleCheck)
                    .tone(Tone::Success),
                now,
                dwell_for(ToastDuration::Normal),
            );
        }
        if self.notifications.sound {
            self.notification_sound.play();
        }
    }

    /// Compares all aggregate session activities in a full snapshot.
    fn observe_snapshot_agent_activity(&mut self, snapshot: &Snapshot, now: Instant) {
        let activities = snapshot_agent_activities(snapshot);
        let live: HashSet<SessionId> = activities
            .iter()
            .map(|(session, _)| session.clone())
            .chain(
                snapshot
                    .sessions
                    .iter()
                    .filter(|session| matches!(&session.kind, SessionKind::Agent(_)))
                    .map(|session| session.id.clone()),
            )
            .collect();
        let mut finished = Vec::new();
        for (session, activity) in activities {
            if self.observe_agent_activity(session.clone(), activity, now) {
                finished.push((
                    session.clone(),
                    agent_notification_label(snapshot, &session),
                ));
            }
        }
        self.last_agent_activity
            .retain(|session, _| live.contains(session));
        for (_, label) in finished {
            self.notify_agent_finished(&label, now);
        }
    }

    /// Replaces the snapshot mirror and re-derives everything that hangs off it.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot, now: Instant) {
        if self.active_context() != snapshot.active_context.as_ref() {
            self.clear_board();
        }
        self.observe_snapshot_agent_activity(&snapshot, now);
        self.has_seen_non_empty_state |= !snapshot.contexts.is_empty()
            || !snapshot.repos.is_empty()
            || !snapshot.clones.is_empty();
        self.sticky_error =
            crate::views::sticky_error::sticky_error_for(&snapshot.jobs, &self.seen_failed);
        self.cursors.repos = clamp_cursor(self.cursors.repos, snapshot.repos.len() + 1);
        self.cursors.worktrees = clamp_cursor(self.cursors.worktrees, snapshot.worktrees.len());
        self.cursors.jobs = clamp_cursor(self.cursors.jobs, snapshot.jobs.len());
        self.forget_vanished(&snapshot);
        self.snapshot = Some(snapshot);
        self.snapshot_at = Some(now);
        // The snapshot is what decides which tab is active, so it is also what decides whether
        // the Workspace is over a PTY or over a Fleet-drawn pane.
        self.sync_terminal_mode();
    }

    /// Drops the mirrors and MRU entries of everything the daemon no longer lists.
    ///
    /// The snapshot is authoritative, so a terminal or session that is gone from it can never
    /// be painted again. A `MirrorGrid` is `rows × cols` [`Cell`]s, each holding a heap
    /// `SharedString`: a 200 × 60 grid is roughly 12 000 of them, and a day of opening and
    /// closing terminals used to keep every one of them alive for the life of the process,
    /// because `ctrl-s x` only told the daemon and `TerminalExited` only set an exit code.
    fn forget_vanished(&mut self, snapshot: &Snapshot) {
        let live_terminals: HashSet<TerminalId> = snapshot
            .sessions
            .iter()
            .flat_map(|session| session.terminals.iter().map(|terminal| terminal.id))
            .collect();
        self.grids
            .retain(|terminal, _| live_terminals.contains(terminal));

        let live_sessions: HashSet<&SessionId> = snapshot
            .sessions
            .iter()
            .map(|session| &session.id)
            .collect();
        self.renamed_terminals
            .retain(|terminal| live_terminals.contains(terminal));
        self.terminal_mru
            .retain(|session, _| live_sessions.contains(session));
        for mru in self.terminal_mru.values_mut() {
            mru.retain(|terminal| live_terminals.contains(terminal));
        }
    }

    /// Applies one terminal frame to its mirror grid, creating the grid on the first frame.
    pub fn apply_frame(&mut self, frame: &FrameUpdate) -> bool {
        let grid = self
            .grids
            .entry(frame.terminal)
            .or_insert_with(|| MirrorGrid::new(frame.cols, frame.rows));
        grid.apply(frame)
    }

    /// Records that a terminal's PTY exited; the grid freezes at its last frame (§3.6).
    pub fn apply_terminal_exit(&mut self, terminal: TerminalId, code: Option<i32>) {
        if let Some(grid) = self.grids.get_mut(&terminal) {
            grid.exit_code = Some(code);
        }
    }

    /// Records a terminal title change.
    pub fn apply_terminal_title(&mut self, terminal: TerminalId, title: String) {
        if let Some(grid) = self.grids.get_mut(&terminal) {
            grid.title = Some(title);
        }
    }

    /// Drops a mirror grid, e.g. after a terminal is closed.
    pub fn forget_terminal(&mut self, terminal: TerminalId) {
        self.grids.remove(&terminal);
        for mru in self.terminal_mru.values_mut() {
            mru.forget(&terminal);
        }
    }

    /// Records that the user named this terminal, so its name outranks the program's title.
    pub fn mark_renamed(&mut self, terminal: TerminalId) {
        self.renamed_terminals.insert(terminal);
    }

    /// Moves the session MRU as a session is opened.
    pub fn touch_session(&mut self, session: SessionId) {
        self.session_mru.touch(session);
    }

    /// Moves a session's terminal MRU as a tab is selected.
    pub fn touch_terminal(&mut self, session: &SessionId, terminal: TerminalId) {
        self.terminal_mru
            .entry(session.clone())
            .or_default()
            .touch(terminal);
    }

    /// The active context, when the snapshot names one.
    #[must_use]
    pub fn active_context(&self) -> Option<&ContextId> {
        self.snapshot.as_ref()?.active_context.as_ref()
    }

    /// The age of the snapshot in seconds, for the `stale · <age>` stamp (§1.3).
    #[must_use]
    pub fn snapshot_age(&self, now: Instant) -> Option<u64> {
        self.snapshot_at
            .map(|at| now.saturating_duration_since(at).as_secs())
    }

    /// The path of `fleetd.log`, which the daemon surfaces open (§3.12).
    #[must_use]
    pub fn daemon_log_path(&self) -> PathBuf {
        daemon_log_path(&self.home)
    }

    /// The session shown by the Workspace, when that is the current screen.
    #[must_use]
    pub fn active_session(&self) -> Option<&Session> {
        let Screen::Workspace { session } = &self.screen else {
            return None;
        };
        self.snapshot
            .as_ref()?
            .sessions
            .iter()
            .find(|candidate| &candidate.id == session)
    }

    /// The mirror grid of the active session's active terminal.
    #[must_use]
    pub fn active_grid(&self) -> Option<&MirrorGrid> {
        let terminal = self.active_session()?.active_terminal?;
        self.grids.get(&terminal)
    }

    /// Advances everything that expires on its own: toasts, the reconnect banner, the
    /// cold-start splash. Returns whether the frame has to be repainted.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut changed = expire_toasts(&mut self.toasts, now)
            || self
                .active_session()
                .is_some_and(|s| self.watches.running_visible(&s.id));
        match self.daemon {
            DaemonLink::Reconnected { restarted, since } => {
                let dwell = if restarted {
                    RESTART_BANNER_DWELL
                } else {
                    RECONNECT_BANNER_DWELL
                };
                if now.saturating_duration_since(since) >= dwell {
                    self.daemon = DaemonLink::Connected;
                }
                changed = true;
            }
            // The spinner, the 3 s socket line and the reconnect countdown all animate.
            DaemonLink::Starting | DaemonLink::Lost { .. } => changed = true,
            DaemonLink::Connected | DaemonLink::Failed { .. } => {}
        }
        changed
    }

    /// Applies one message from the daemon bridge.
    pub fn apply_bridge_event(&mut self, event: BridgeEvent, now: Instant) {
        match event {
            BridgeEvent::TerminalConfig(config) => self.terminal_config = config,
            BridgeEvent::NotificationConfig(config) => self.notifications = config,
            BridgeEvent::Connected(snapshot) => {
                self.daemon = DaemonLink::Connected;
                self.daemon_since = now;
                self.link_generation = self.link_generation.wrapping_add(1);
                self.clear_board();
                self.watches.reconnect();
                self.seed_agent_activity(&snapshot, now);
                self.apply_snapshot(*snapshot, now);
            }
            BridgeEvent::ConnectFailed {
                message,
                log_tail,
                stale_socket,
            } => {
                self.daemon = DaemonLink::Failed {
                    message,
                    log_tail,
                    stale_socket,
                };
                self.daemon_since = now;
                // §3.12 B takes the whole window and draws no overlay layer, so anything that
                // was open would keep its key context alive with nothing on screen to close.
                self.overlay = None;
            }
            BridgeEvent::Disconnected { attempt } => {
                self.clear_board();
                let dismissed = matches!(
                    self.daemon,
                    DaemonLink::Lost {
                        dismissed: true,
                        ..
                    }
                );
                self.daemon = DaemonLink::Lost { attempt, dismissed };
                self.daemon_since = now;
            }
            BridgeEvent::Reconnected {
                restarted,
                snapshot,
            } => {
                if restarted {
                    // ARCHITECTURE: PTYs do not survive fleetd. Dropping the mirrors is what
                    // stops the app from painting a grid that no longer has a process.
                    self.grids.clear();
                    self.watches = crate::watches::Watches::default();
                }
                self.daemon = DaemonLink::Reconnected {
                    restarted,
                    since: now,
                };
                self.daemon_since = now;
                // The daemon-side connection is new and holds no attachments, whether or not
                // fleetd itself restarted.
                self.link_generation = self.link_generation.wrapping_add(1);
                self.clear_board();
                self.watches.reconnect();
                self.seed_agent_activity(&snapshot, now);
                self.apply_snapshot(*snapshot, now);
            }
            BridgeEvent::Daemon(event) => self.apply_daemon_event(*event, now),
            // Broadcast lag may affect any terminal, including one with no subsequent frame.
            // Marking every mirror desynced is what stops
            // a diff from being applied on top of rows that are already wrong; the shell then
            // asks for a full frame per terminal, which is the only thing that repairs them.
            BridgeEvent::EventsLagged { .. } => {
                self.board_stale = true;
                self.desync_grids();
                self.watches.invalidate();
            }
        }
    }

    /// Marks every mirror as having missed frames, and reports which terminals they belong to.
    ///
    /// The caller sends `RequestFullFrame` for each: `fleet-app` mirrors raw
    /// `Event::TerminalFrame`s itself instead of going through `fleet-client`'s
    /// `TerminalHandle`, so it owns this recovery.
    pub fn desync_grids(&mut self) -> Vec<TerminalId> {
        for grid in self.grids.values_mut() {
            grid.desynced = true;
        }
        self.grids.keys().copied().collect()
    }

    /// Applies one ordinary daemon event to the mirror.
    pub fn apply_daemon_event(&mut self, event: Event, now: Instant) {
        match event {
            Event::BoardChanged { board_id, .. } => {
                if self.board().is_some_and(|view| view.board.id == board_id) || self.board.loading
                {
                    self.board_stale = true;
                }
            }
            Event::WatchStarted(watch) => self.watches.started(watch, now),
            Event::WatchOutput { watch, chunks } => self.watches.output(watch, chunks),
            Event::WatchExited(watch) => self.watches.exited(watch, now),
            Event::WatchDismissed(id) => self.watches.dismissed(id),
            Event::SnapshotChanged(snapshot) => self.apply_snapshot(snapshot, now),
            Event::JobUpdated(job) => self.apply_job(job, now),
            Event::SessionChanged(session) => self.apply_session(session),
            Event::AgentActivityChanged {
                session,
                terminal_id,
                agent,
                activity,
                changed_at,
            } => self.apply_agent_activity(session, terminal_id, agent, activity, changed_at, now),
            Event::TerminalFrame(frame) => {
                self.apply_frame(&frame);
            }
            Event::TerminalExited { terminal, code } => self.apply_terminal_exit(terminal, code),
            Event::TerminalTitle { terminal, title } => self.apply_terminal_title(terminal, title),
            Event::Toast { level, message } => self.apply_toast_event(level, message, now),
            Event::DaemonShuttingDown => {
                self.daemon = DaemonLink::Lost {
                    attempt: 0,
                    dismissed: false,
                };
                self.daemon_since = now;
            }
        }
    }

    /// Applies one terminal activity edge and presents a qualifying aggregate completion.
    pub fn apply_agent_activity(
        &mut self,
        session: SessionId,
        terminal_id: TerminalId,
        agent: Option<String>,
        activity: AgentActivity,
        changed_at: String,
        now: Instant,
    ) {
        let label = self.snapshot.as_ref().map_or_else(
            || session.as_str().to_owned(),
            |snapshot| agent_notification_label(snapshot, &session),
        );
        let aggregate = self
            .snapshot
            .as_mut()
            .and_then(|snapshot| {
                patch_snapshot_agent_activity(
                    snapshot,
                    &session,
                    terminal_id,
                    agent,
                    activity,
                    changed_at,
                )
            })
            .unwrap_or(activity);
        if self.observe_agent_activity(session, aggregate, now) {
            self.notify_agent_finished(&label, now);
        }
    }

    /// Patches one job into the snapshot mirror and re-derives the sticky error slot.
    pub fn apply_job(&mut self, job: JobRecord, now: Instant) {
        let outcome = crate::views::job_ticker::job_outcome_toast(
            &job,
            matches!(self.overlay, Some(Overlay::Jobs)),
        );
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        match snapshot
            .jobs
            .iter_mut()
            .find(|existing| existing.id == job.id)
        {
            Some(existing) => *existing = job,
            None => snapshot.jobs.push(job),
        }
        self.sticky_error =
            crate::views::sticky_error::sticky_error_for(&snapshot.jobs, &self.seen_failed);
        if let Some(text) = outcome {
            self.toast(
                Toast::new(text).icon(Icon::CircleCheck),
                now,
                dwell_for(ToastDuration::Normal),
            );
        }
    }

    /// Patches one session into the snapshot mirror.
    pub fn apply_session(&mut self, session: Session) {
        let Some(snapshot) = self.snapshot.as_mut() else {
            return;
        };
        match snapshot
            .sessions
            .iter_mut()
            .find(|existing| existing.id == session.id)
        {
            Some(existing) => *existing = session,
            None => snapshot.sessions.push(session),
        }
    }
}

/// Aggregate activity keyed by the real session id represented in a snapshot.
fn snapshot_agent_activities(snapshot: &Snapshot) -> Vec<(SessionId, AgentActivity)> {
    snapshot
        .sessions
        .iter()
        .filter_map(|session| {
            let SessionKind::Worktree(worktree) = &session.kind else {
                return None;
            };
            let activity = snapshot
                .statuses
                .iter()
                .find(|status| &status.worktree_id == worktree)
                .map_or(AgentActivity::Unknown, |status| status.agent_activity);
            Some((session.id.clone(), activity))
        })
        .collect()
}

/// The canonical `repo/slug` label used for a worktree session.
fn agent_notification_label(snapshot: &Snapshot, session: &SessionId) -> String {
    snapshot
        .worktrees
        .iter()
        .find(|worktree| worktree.session == session.as_str())
        .map_or_else(
            || session.as_str().to_owned(),
            |worktree| worktree.session.clone(),
        )
}

/// Patches an activity event into the current snapshot and returns its new session aggregate.
fn patch_snapshot_agent_activity(
    snapshot: &mut Snapshot,
    session: &SessionId,
    terminal_id: TerminalId,
    agent: Option<String>,
    activity: AgentActivity,
    changed_at: String,
) -> Option<AgentActivity> {
    let (worktree, index) = {
        let session = snapshot
            .sessions
            .iter()
            .find(|candidate| &candidate.id == session)?;
        let SessionKind::Worktree(worktree) = &session.kind else {
            return None;
        };
        let index = session
            .terminals
            .iter()
            .position(|terminal| terminal.id == terminal_id)?;
        (worktree.clone(), u32::try_from(index).ok()?)
    };
    let status = snapshot
        .statuses
        .iter_mut()
        .find(|status| status.worktree_id == worktree)?;
    let window = status
        .windows
        .iter_mut()
        .find(|window| window.index == index)?;
    window.agent = agent;
    window.agent_activity = activity;
    window.agent_activity_changed_at = Some(changed_at);
    let (aggregate, changed_at) = aggregate_agent_activity(&status.windows);
    status.agent_activity = aggregate;
    status.agent_activity_changed_at = changed_at;
    Some(aggregate)
}

/// `<home>/logs/fleetd.log`.
#[must_use]
pub fn daemon_log_path(home: &Path) -> PathBuf {
    home.join("logs").join("fleetd.log")
}

/// The dwell of a kit toast duration, as milliseconds are a theme token the state cannot read.
#[must_use]
pub const fn dwell_for(duration: ToastDuration) -> Duration {
    match duration {
        ToastDuration::Short => Duration::from_millis(1_600),
        ToastDuration::Normal => Duration::from_millis(3_200),
    }
}

// ---------------------------------------------------------------------------- breadcrumb

/// The status bar's `context › repo › row` breadcrumb (§2.2), zero-suppressed.
#[must_use]
pub fn breadcrumb(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" › ")
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use fleet_core::{
        model::Worktree,
        sessions::{SessionKind, WorktreeStatus, WorktreeWindowStatus},
    };
    use fleet_proto::terminal::{CellAttrs, RowUpdate};

    use super::*;

    fn frame(seq: u64, full: bool, rows: Vec<RowUpdate>) -> FrameUpdate {
        FrameUpdate {
            terminal: TerminalId(1),
            seq,
            cols: 4,
            rows: 2,
            full,
            shift: None,
            rows_changed: rows,
            cursor: CursorState {
                row: 0,
                col: 0,
                visible: true,
                shape: CursorShape::Block,
            },
            viewport: ViewportInfo {
                scrollback_len: 0,
                offset: 0,
                history_epoch: 0,
            },
            modes: TerminalModes::default(),
            title: None,
        }
    }

    #[test]
    fn forward_sequence_gap_requires_full_recovery() {
        let mut grid = MirrorGrid::new(4, 2);
        grid.apply(&frame(1, true, vec![row(0, "old"), row(1, "keep")]));
        assert!(!grid.apply(&frame(3, false, vec![row(0, "lost")])));
        assert!(grid.desynced);
        assert_eq!(grid.row_text(0), "old");
        assert!(!grid.apply(&frame(4, false, vec![row(1, "bad")])));
        assert!(grid.apply(&frame(5, true, vec![row(0, "new"), row(1, "good")])));
        assert!(!grid.desynced);
        assert!(!grid.apply(&frame(2, true, vec![row(0, "stale")])));
        assert_eq!(grid.row_text(0), "new");
    }

    #[test]
    fn shifted_rows_move_before_replacements_in_both_directions() {
        let mut grid = MirrorGrid::new(4, 2);
        let mut wrapped = row(1, "bbb");
        wrapped.wrapped = true;
        grid.apply(&frame(1, true, vec![row(0, "aaa"), wrapped]));
        let mut down = frame(2, false, vec![row(1, "ccc")]);
        down.shift = Some(1);
        assert!(grid.apply(&down));
        assert_eq!(grid.wrapped, vec![true, false]);
        assert_eq!(
            (grid.row_text(0), grid.row_text(1)),
            ("bbb".into(), "ccc".into())
        );
        let mut up = frame(3, false, vec![row(0, "aaa")]);
        up.shift = Some(-1);
        assert!(grid.apply(&up));
        assert_eq!(grid.wrapped, vec![false, true]);
        assert_eq!(
            (grid.row_text(0), grid.row_text(1)),
            ("aaa".into(), "bbb".into())
        );
    }

    #[test]
    fn shift_across_history_epochs_freezes_rows_until_full_recovery() {
        let mut grid = MirrorGrid::new(4, 2);
        grid.apply(&frame(1, true, vec![row(0, "old"), row(1, "keep")]));
        let before = grid.lines.clone();
        let mut shifted = frame(2, false, vec![row(1, "new")]);
        shifted.shift = Some(1);
        shifted.viewport.history_epoch = 1;
        assert!(!grid.apply(&shifted));
        assert!(grid.desynced);
        assert_eq!(grid.lines, before);
        let mut recovery = frame(3, true, vec![row(0, "new"), row(1, "live")]);
        recovery.viewport.history_epoch = 1;
        assert!(grid.apply(&recovery));
        assert!(!grid.desynced);
        assert_eq!(grid.viewport.history_epoch, 1);
        assert_eq!(grid.row_text(0), "new");
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            boards: Vec::new(),
            generated_at: "2026-09-04T12:00:00Z".to_owned(),
            contexts: Vec::new(),
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: None,
            sessions: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: fleet_proto::snapshot::DaemonInfo {
                version: "0.1.0".to_owned(),
                pid: 4211,
                started_at: "2026-09-04T09:00:00Z".to_owned(),
                home: "/tmp/fleet".to_owned(),
            },
        }
    }

    fn board_view(context: &str) -> BoardView {
        let context = fleet_core::model::Context {
            id: context.parse().unwrap(),
            name: context.into(),
            owners: Vec::new(),
            created_at: "2026-09-06T12:00:00Z".into(),
        };
        BoardView {
            board: fleet_core::board::new_board(&context, &context.created_at),
            cards: Vec::new(),
        }
    }

    fn board_state() -> AppState {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet-board-test", now);
        let mut snapshot = snapshot();
        snapshot.active_context = Some("work".parse().unwrap());
        snapshot.contexts.push(fleet_core::model::Context {
            id: "work".parse().unwrap(),
            name: "Work".into(),
            owners: Vec::new(),
            created_at: "2026-09-06T12:00:00Z".into(),
        });
        state.apply_bridge_event(BridgeEvent::Connected(Box::new(snapshot)), now);
        state.screen = Screen::Hub { tab: HubTab::Board };
        state
    }

    fn descriptor(kind: &str, label: &str) -> BackendDescriptor {
        BackendDescriptor {
            kind: kind.to_owned(),
            label: label.to_owned(),
            capabilities: fleet_core::board::BackendCapabilities::default(),
            settings_schema: Vec::new(),
        }
    }

    #[test]
    fn the_backend_registry_is_fetched_once_per_connection_and_survives_a_failure() {
        let mut state = board_state();
        assert!(state.begin_backends_load(), "the first render asks once");
        assert!(
            !state.begin_backends_load(),
            "a board renders every frame; a second ask is sixty requests a second"
        );
        // A reconnect can land on a different fleetd, so the registry is asked for again — but
        // the descriptors already in hand stay until the newer ones arrive.
        state.apply_backends(vec![descriptor("jira", "Jira (acli)")]);
        let now = Instant::now();
        state.apply_bridge_event(BridgeEvent::Disconnected { attempt: 1 }, now);
        assert_eq!(state.backend_label("jira"), "Jira (acli)");
        assert!(
            !state.begin_backends_load(),
            "a lost daemon answers nothing"
        );
        state.apply_bridge_event(
            BridgeEvent::Connected(Box::new(snapshot())),
            now + Duration::from_secs(1),
        );
        assert!(state.begin_backends_load());
        // The flag is claimed before the request goes out, so a refused answer has to release
        // it: otherwise one failed ask leaves the header on the raw kind and the settings
        // dialog with no backend rows for the whole connection.
        state.backends_load_failed();
        assert!(
            state.begin_backends_load(),
            "a failed ask must be askable again"
        );
    }

    #[test]
    fn an_unknown_backend_kind_is_labelled_by_its_own_key() {
        let mut state = board_state();
        assert_eq!(state.backend_label("jira"), "jira");
        state.apply_backends(vec![descriptor("local", "Local")]);
        assert_eq!(state.backend_label("local"), "Local");
        assert_eq!(state.backend_label("jira"), "jira");
    }

    #[test]
    fn only_a_linked_board_has_read_only_fields() {
        let mut state = board_state();
        assert!(state.readonly_fields().is_empty(), "no board is loaded");
        let mut view = board_view("work");
        view.board.sync.readonly_fields = vec!["priority".into(), "due_date".into()];
        state.apply_board_view(view);
        assert!(
            state.readonly_fields().is_empty(),
            "a local board declares no backend, so `ops` refuses nothing"
        );
        let board = state.board.view.as_mut().unwrap_or_else(|| panic!("board"));
        board.board.backend.kind = "jira".into();
        assert!(state.is_readonly_field("priority"));
        assert!(state.is_readonly_field("due_date"));
        assert!(!state.is_readonly_field("title"));
    }

    #[test]
    fn board_load_is_single_flight_and_retains_errors_until_reload() {
        let mut state = board_state();
        let (context, generation) = state.begin_board_load().unwrap();
        assert!(state.board.loading);
        assert!(state.begin_board_load().is_none());
        state.finish_board_load(&context, generation, Err("offline".into()));
        assert!(!state.board.loading);
        assert_eq!(state.board.error.as_deref(), Some("offline"));
        assert!(state.begin_board_load().is_none());
        state.board_stale = true;
        let (context, generation) = state.begin_board_load().unwrap();
        state.finish_board_load(&context, generation, Ok(board_view("work")));
        assert!(state.board.error.is_none());
        assert!(state.board().is_some());
        assert!(!state.board_stale);
    }

    #[test]
    fn a_lost_daemon_says_so_on_the_board_instead_of_loading_forever() {
        let mut state = board_state();
        state.apply_bridge_event(
            BridgeEvent::Disconnected { attempt: 2 },
            Instant::now() + Duration::from_secs(1),
        );
        assert!(state.begin_board_load().is_none());
        assert!(!state.board.loading, "a refused load is never in flight");
        assert_eq!(
            state.board.error.as_deref(),
            Some("fleetd is not reachable"),
            "cold columns would claim a load that can never arrive"
        );
        assert!(state.board_stale, "the board reloads once fleetd is back");
    }

    #[test]
    fn a_board_with_no_active_context_says_which_keys_pick_one() {
        let now = Instant::now();
        let mut state = board_state();
        let mut none = snapshot();
        none.active_context = None;
        state.apply_snapshot(none, now);

        assert!(state.begin_board_load().is_none());
        assert!(
            !state.board.loading,
            "no request went out, so nothing is in flight"
        );
        assert!(state.board().is_none());
        assert_eq!(
            state.board.error.as_deref(),
            Some(NO_ACTIVE_CONTEXT),
            "skeleton columns would promise a load that can never be sent"
        );
        assert!(state.board_stale, "the load is still owed");

        let mut active = snapshot();
        active.active_context = Some("work".parse().unwrap());
        state.apply_snapshot(active, now);
        assert!(
            state.board.error.is_none(),
            "activating a context answers the message"
        );
        let (context, generation) = state
            .begin_board_load()
            .unwrap_or_else(|| panic!("the load goes out once a context is active"));
        state.finish_board_load(&context, generation, Ok(board_view("work")));
        assert!(state.board().is_some());
    }

    #[test]
    fn board_load_rejects_old_responses_after_context_round_trip_or_reconnect() {
        let mut state = board_state();
        let (context, generation) = state.begin_board_load().unwrap();
        let mut next = snapshot();
        next.active_context = Some("personal".parse().unwrap());
        state.apply_snapshot(next.clone(), Instant::now());
        state.finish_board_load(&context, generation, Ok(board_view("work")));
        assert!(state.board().is_none());
        next.active_context = Some(context.clone());
        state.apply_snapshot(next.clone(), Instant::now());
        let (_, current) = state.begin_board_load().unwrap();
        state.finish_board_load(&context, generation, Ok(board_view("work")));
        assert!(state.board.loading);
        assert!(state.board().is_none());
        state.apply_bridge_event(
            BridgeEvent::Reconnected {
                restarted: false,
                snapshot: Box::new(next),
            },
            Instant::now(),
        );
        state.finish_board_load(&context, current, Ok(board_view("work")));
        assert!(state.board().is_none());
        assert!(!state.board.loading);
    }

    #[test]
    fn board_events_invalidate_only_the_displayed_board_and_survive_inflight_loads() {
        use fleet_proto::event::BoardChangeReason;
        let mut state = board_state();
        state.apply_board_view(board_view("work"));
        let event = |id: &str| Event::BoardChanged {
            board_id: id.parse().unwrap(),
            reason: BoardChangeReason::CardChanged,
        };
        state.apply_daemon_event(event("personal"), Instant::now());
        assert!(!state.board_stale);
        state.apply_daemon_event(event("work"), Instant::now());
        assert!(state.board_stale);
        let (context, generation) = state.begin_board_load().unwrap();
        state.apply_daemon_event(event("work"), Instant::now());
        state.finish_board_load(&context, generation, Ok(board_view("work")));
        assert!(
            state.board_stale,
            "a change during the request still needs a refresh"
        );
        assert!(state.begin_board_load().is_some());
    }

    #[test]
    fn card_reducer_upserts_only_matching_boards_and_clamps_selection() {
        let mut state = board_state();
        let mut view = board_view("work");
        let card = fleet_core::board::create_card(
            &mut view.board,
            &[],
            "card-1".parse().unwrap(),
            fleet_core::board::CardDraft {
                title: "First".into(),
                ..Default::default()
            },
            "2026-09-06T12:00:00Z",
        )
        .unwrap();
        state.apply_board_view(view);
        state.apply_card(card.clone());
        let mut edited = card;
        edited.title = "Edited".into();
        state.board.focus = BoardFocus {
            column: usize::MAX,
            row: usize::MAX,
        };
        state.apply_card(edited.clone());
        assert_eq!(state.board().unwrap().cards.len(), 1);
        assert_eq!(state.board().unwrap().cards[0].title, "Edited");
        assert!(state.board.focus.column < state.board().unwrap().board.statuses.len());
        assert_eq!(state.board.focus.row, 0);
        let mut unknown = edited.clone();
        unknown.status_id = "new-status".parse().unwrap();
        state.apply_card(unknown);
        assert!(state.board_stale);
        assert_eq!(state.board().unwrap().cards[0].status_id, edited.status_id);
        let mut earlier = edited.clone();
        earlier.id = "earlier".parse().unwrap();
        earlier.position = 0;
        edited.position = 10;
        state.apply_card(earlier);
        state.apply_card(edited.clone());
        assert_eq!(state.board().unwrap().cards[0].id.as_str(), "earlier");
        state.board.view.as_mut().unwrap().cards.remove(0);
        edited.board_id = "personal".parse().unwrap();
        edited.title = "Wrong board".into();
        state.apply_card(edited);
        state.apply_board_view(board_view("personal"));
        assert_eq!(state.board().unwrap().cards[0].title, "Edited");
        state.board.filter = "query".into();
        state.clear_board();
        assert_eq!(state.board, BoardState::default());
        assert!(state.board_stale);
    }

    #[test]
    fn board_context_overrides_the_repo_pane_and_dialogs_shadow_it() {
        let mut state = board_state();
        state.hub_pane = HubPane::Repos;
        assert_eq!(state.context_chain(), ["Hub", "Board"]);
        state.open_overlay(Overlay::Dialog(Dialogs::CardDetail));
        assert_eq!(state.context_chain(), ["Dialog", "CardDetail"]);
    }

    #[test]
    fn the_board_filter_input_shadows_every_board_letter() {
        let mut state = board_state();
        assert_eq!(state.context_chain(), ["Hub", "Board"]);
        assert_eq!(state.mode(), Mode::Normal);
        state.board.filter_editing = true;
        assert_eq!(
            state.context_chain(),
            ["Filter", "BoardFilter"],
            "`c`, `d` and `s` must type, not fire"
        );
        assert_eq!(state.mode(), Mode::Filter);
        // A dialog opened on top still owns the keyboard.
        state.open_overlay(Overlay::Dialog(Dialogs::CardCreate));
        assert_eq!(state.context_chain(), ["Dialog", "CardCreate"]);
    }

    #[test]
    fn board_escape_leaves_the_input_then_clears_the_filter_and_never_quits() {
        let mut state = board_state();
        state.board.filter = "login".into();
        state.board.filter_editing = true;
        assert!(state.board_filter_escape());
        assert!(!state.board.filter_editing, "stage one leaves the input");
        assert_eq!(state.board.filter, "login", "and keeps the filter");
        assert!(state.cancel());
        assert!(
            state.board.filter.is_empty(),
            "stage two uses the base Cancel action"
        );
        assert!(
            !state.board_filter_escape(),
            "a third Esc belongs to whoever is behind the board"
        );
    }

    #[test]
    fn board_focus_is_clamped_against_the_filtered_column_not_the_raw_one() {
        let mut state = board_state();
        let mut view = board_view("work");
        let mut cards = Vec::new();
        for (index, title) in ["Fix login", "Ship the board"].iter().enumerate() {
            let card = fleet_core::board::create_card(
                &mut view.board,
                &cards,
                format!("card-{index}").parse().unwrap(),
                fleet_core::board::CardDraft {
                    title: (*title).to_owned(),
                    ..Default::default()
                },
                "2026-09-06T12:00:00Z",
            )
            .unwrap();
            cards.push(card);
        }
        view.cards = cards;
        state.apply_board_view(view);
        state.board.focus.column = state
            .board()
            .unwrap()
            .board
            .statuses
            .iter()
            .position(|status| status.id == state.board().unwrap().cards[0].status_id)
            .unwrap();
        state.board.focus.row = 1;
        state.clamp_board_focus();
        assert_eq!(state.board.focus.row, 1);
        state.board.filter = "login".into();
        state.clamp_board_focus();
        assert_eq!(
            state.board.focus.row, 0,
            "only one card survives the filter"
        );
    }

    #[derive(Debug)]
    struct RecordingSound(Arc<AtomicUsize>);

    impl NotificationSound for RecordingSound {
        fn play(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn state_with_recording_sound(now: Instant) -> (AppState, Arc<AtomicUsize>) {
        let plays = Arc::new(AtomicUsize::new(0));
        let mut state = AppState::new("/tmp/fleet", now);
        state.notification_sound = Box::new(RecordingSound(Arc::clone(&plays)));
        (state, plays)
    }

    fn agent_snapshot(entries: &[(&str, &str, u64, AgentActivity)]) -> Snapshot {
        let mut snapshot = snapshot();
        for (session_id, slug, terminal_id, activity) in entries {
            let worktree_id: fleet_core::ids::WorktreeId = format!("buk/payroll#{slug}")
                .parse()
                .unwrap_or_else(|error| panic!("{error}"));
            let mut session = session_with(session_id, &[*terminal_id]);
            session.kind = SessionKind::Worktree(worktree_id.clone());
            snapshot.sessions.push(session);
            snapshot.worktrees.push(Worktree {
                id: worktree_id.clone(),
                repo_id: "buk/payroll"
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
                slug: (*slug).to_owned(),
                branch: format!("feat/{slug}"),
                base_ref: "origin/main".to_owned(),
                path: format!("/tmp/{slug}"),
                session: (*session_id).to_owned(),
                host: None,
                created_at: "2026-09-05T12:00:00Z".to_owned(),
                last_opened_at: None,
                degraded: None,
            });
            snapshot.statuses.push(WorktreeStatus {
                worktree_id,
                session: SessionState::Detached,
                windows: vec![WorktreeWindowStatus {
                    index: 0,
                    name: "cc".to_owned(),
                    command: "claude".to_owned(),
                    keep_alive: vec!["claude".to_owned()],
                    agent: Some("claude".to_owned()),
                    agent_activity: *activity,
                    agent_activity_changed_at: Some("2026-09-05T12:00:00Z".to_owned()),
                }],
                running: vec!["claude".to_owned()],
                agent_activity: *activity,
                agent_activity_changed_at: Some("2026-09-05T12:00:00Z".to_owned()),
            });
        }
        snapshot
    }

    fn agent_event(session: &str, terminal_id: u64, activity: AgentActivity) -> Event {
        Event::AgentActivityChanged {
            session: session.parse().unwrap_or_else(|error| panic!("{error}")),
            terminal_id: TerminalId(terminal_id),
            agent: Some("claude".to_owned()),
            activity,
            changed_at: "2026-09-05T12:00:00Z".to_owned(),
        }
    }

    fn row(index: u16, text: &str) -> RowUpdate {
        RowUpdate {
            index,
            cells: text
                .chars()
                .map(|character| Cell {
                    text: character.to_string().into(),
                    fg: Color::Default,
                    bg: Color::Default,
                    underline_color: None,
                    attrs: CellAttrs::empty(),
                    width: CellWidth::Narrow,
                })
                .collect(),
            wrapped: false,
        }
    }

    #[test]
    fn watch_pane_smoke_sequence_applies_real_events_without_changing_terminal_focus() {
        use fleet_core::watches::{Watch, WatchChunk, WatchId, WatchStatus, WatchStream};
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet-watch-phase2", now);
        let session = session_with("fleet/watch", &[1]);
        let mut snapshot = snapshot();
        snapshot.sessions = vec![session.clone()];
        state.apply_bridge_event(BridgeEvent::Connected(Box::new(snapshot)), now);
        state.screen = Screen::Workspace {
            session: session.id.clone(),
        };
        state.enter_prefix();
        state.cycle_watch(true, now);
        assert_eq!(state.terminal_mode, TerminalMode::Terminal);
        assert_eq!(
            state.toasts.last().unwrap().toast.text.as_ref(),
            "no subagent watches"
        );
        state.toasts.clear();
        let mut watch = Watch {
            id: WatchId(1),
            session: session.id.clone(),
            terminal: TerminalId(1),
            label: "codex".into(),
            command: vec!["sh".into()],
            cwd: None,
            pid: Some(123),
            started_at: chrono::Utc::now().to_rfc3339(),
            status: WatchStatus::Running,
            source: fleet_core::watches::WatchSource::Cooperative,
            log_file: None,
        };
        state.apply_daemon_event(Event::WatchStarted(watch.clone()), now);
        assert!(state.watches.panes[&session.id].visible);
        assert_eq!(state.watches.panes[&session.id].selected, Some(watch.id));
        assert!(state.tick(now + Duration::from_secs(1)));
        for forward in [true, false] {
            state.watches.hide(&session.id);
            state.enter_prefix();
            state.cycle_watch(forward, now);
            assert_eq!(state.terminal_mode, TerminalMode::Terminal);
            assert!(state.watches.panes[&session.id].visible);
            assert_eq!(state.watches.panes[&session.id].selected, Some(watch.id));
            assert_eq!(
                state.active_session().unwrap().active_terminal,
                Some(TerminalId(1))
            );
            assert!(state.toasts.is_empty());
        }
        state.enter_prefix();
        assert_eq!(state.close_selected_watch(now), None);
        assert_eq!(
            state.watches.entries[&watch.id].watch.status,
            WatchStatus::Running
        );
        assert!(!state.watches.panes[&session.id].visible);
        assert_eq!(
            state.toasts.last().unwrap().toast.text.as_ref(),
            "watch still running; pane hidden"
        );
        state.toggle_watch_pane(now);
        assert!(state.watches.panes[&session.id].visible);
        for i in 0..5 {
            state.apply_daemon_event(
                Event::WatchOutput {
                    watch: watch.id,
                    chunks: vec![
                        WatchChunk {
                            seq: i * 2,
                            stream: WatchStream::Stdout,
                            text: format!("line {}\n", i + 1),
                        },
                        WatchChunk {
                            seq: i * 2 + 1,
                            stream: WatchStream::Stderr,
                            text: format!("warn {}\n", i + 1),
                        },
                    ],
                },
                now,
            );
        }
        assert_eq!(state.watches.entries[&watch.id].lines.len(), 10);
        watch.status = WatchStatus::Exited {
            code: Some(2),
            signal: None,
        };
        state.apply_daemon_event(
            Event::WatchExited(watch.clone()),
            now + Duration::from_secs(2),
        );
        assert_eq!(state.watches.entries[&watch.id].watch.status, watch.status);
        let duration = state.watches.entries[&watch.id].elapsed(now + Duration::from_secs(10));
        assert!(duration >= Duration::from_secs(2) && duration < Duration::from_secs(3));
        state.enter_prefix();
        state.toggle_watch_pane(now);
        assert!(!state.watches.panes[&session.id].visible);
        state.enter_prefix();
        state.toggle_watch_pane(now);
        assert!(state.watches.panes[&session.id].visible);
        state.zoomed = true;
        assert!(state.watches.panes[&session.id].visible);
        state.enter_prefix();
        assert_eq!(state.close_selected_watch(now), Some(watch.id));
        state.apply_daemon_event(Event::WatchDismissed(watch.id), now);
        assert!(!state.watches.panes[&session.id].visible);
        assert!(state.watches.entries.is_empty());
        assert_eq!(state.terminal_mode, TerminalMode::Terminal);
        assert_eq!(
            state.active_session().unwrap().active_terminal,
            Some(TerminalId(1))
        );
    }

    #[test]
    fn a_dropped_frame_refuses_diffs_until_a_full_frame_repairs_the_mirror() {
        // quality-F2: the client's broadcast buffer overflowed. The rows that changed inside
        // the gap are never re-sent, so applying the *next* diff leaves them permanently wrong.
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.apply_frame(&frame(1, true, vec![row(0, "abcd"), row(1, "efgh")]));

        let stale = state.desync_grids();
        assert_eq!(
            stale,
            vec![TerminalId(1)],
            "the shell re-primes each of these"
        );

        // The last good frame stays on screen — it is still the best answer available.
        let grid = state
            .grids
            .get(&TerminalId(1))
            .unwrap_or_else(|| panic!("no grid"));
        assert!(grid.primed, "the painted rows are not blanked");
        assert_eq!(grid.row_text(0), "abcd");

        // A diff that skipped the gap is refused.
        state.apply_frame(&frame(9, false, vec![row(1, "zzzz")]));
        let grid = state
            .grids
            .get(&TerminalId(1))
            .unwrap_or_else(|| panic!("no grid"));
        assert_eq!(grid.row_text(1), "efgh", "a post-gap diff is not applied");

        // The full frame the shell asked for repairs it, and diffs flow again.
        state.apply_frame(&frame(10, true, vec![row(0, "wxyz"), row(1, "1234")]));
        state.apply_frame(&frame(11, false, vec![row(1, "5678")]));
        let grid = state
            .grids
            .get(&TerminalId(1))
            .unwrap_or_else(|| panic!("no grid"));
        assert!(!grid.desynced);
        assert_eq!(grid.row_text(0), "wxyz");
        assert_eq!(grid.row_text(1), "5678");
    }

    #[test]
    fn a_lagged_bridge_event_desyncs_every_mirror() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.apply_frame(&frame(1, true, vec![row(0, "abcd")]));
        state.apply_bridge_event(BridgeEvent::EventsLagged { dropped: 12 }, now);
        assert!(
            state
                .grids
                .values()
                .all(|grid| grid.desynced && grid.primed)
        );
    }

    #[test]
    fn mirror_grid_applies_full_then_diff_frames() {
        let mut grid = MirrorGrid::new(4, 2);
        let mut first = row(0, "abcd");
        first.wrapped = true;
        assert!(grid.apply(&frame(1, true, vec![first, row(1, "efgh")])));
        assert_eq!(grid.row_text(0), "abcd");
        assert_eq!(grid.row_text(1), "efgh");
        assert_eq!(grid.wrapped, vec![true, false]);

        assert!(grid.apply(&frame(2, false, vec![row(1, "zzzz")])));
        assert_eq!(grid.row_text(0), "abcd", "untouched rows survive a diff");
        assert_eq!(grid.row_text(1), "zzzz");
        assert_eq!(grid.wrapped, vec![true, false]);
        assert_eq!(grid.seq, 2);
    }

    #[test]
    fn mirror_grid_drops_diffs_before_the_first_full_frame() {
        let mut grid = MirrorGrid::new(4, 2);
        assert!(!grid.apply(&frame(1, false, vec![row(0, "abcd")])));
        assert_eq!(grid.row_text(0), "");
        assert!(!grid.primed);
    }

    #[test]
    fn mirror_grid_drops_out_of_order_diffs() {
        let mut grid = MirrorGrid::new(4, 2);
        grid.apply(&frame(5, true, vec![row(0, "abcd")]));
        assert!(!grid.apply(&frame(4, false, vec![row(0, "zzzz")])));
        assert_eq!(grid.row_text(0), "abcd");
    }

    #[test]
    fn mirror_grid_resizes_on_a_full_frame() {
        let mut grid = MirrorGrid::new(4, 2);
        let mut wider = frame(1, true, vec![row(2, "xy")]);
        wider.cols = 8;
        wider.rows = 3;
        assert!(grid.apply(&wider));
        assert_eq!((grid.cols, grid.rows), (8, 3));
        assert_eq!(grid.lines.len(), 3);
        assert_eq!(grid.row_text(2), "xy");
    }

    #[test]
    fn mru_moves_entries_to_the_front_and_exposes_the_alternate() {
        let mut mru = Mru::new();
        mru.touch("a");
        mru.touch("b");
        mru.touch("c");
        assert_eq!(mru.current(), Some(&"c"));
        assert_eq!(mru.alternate(), Some(&"b"));
        mru.touch("a");
        assert_eq!(mru.entries(), &["a", "c", "b"]);
        mru.forget(&"c");
        assert_eq!(mru.entries(), &["a", "b"]);
    }

    #[test]
    fn toasts_coalesce_within_a_second_and_cap_at_three() {
        let now = Instant::now();
        let mut toasts = Vec::new();
        let dwell = Duration::from_millis(3_200);
        push_toast(&mut toasts, Toast::new("Path copied"), now, dwell);
        push_toast(
            &mut toasts,
            Toast::new("Path copied"),
            now + Duration::from_millis(300),
            dwell,
        );
        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts[0].toast.count, 2);

        push_toast(
            &mut toasts,
            Toast::new("Path copied"),
            now + Duration::from_secs(5),
            dwell,
        );
        assert_eq!(toasts.len(), 2, "past the window it is a new toast");

        for index in 0..3 {
            push_toast(
                &mut toasts,
                Toast::new(format!("toast {index}")),
                now + Duration::from_secs(6),
                dwell,
            );
        }
        assert_eq!(toasts.len(), MAX_TOASTS);
        assert_eq!(toasts[0].toast.text.as_ref(), "toast 0");
    }

    #[test]
    #[should_panic(expected = "a toast is never Tone::Danger")]
    fn toasts_are_never_errors() {
        let now = Instant::now();
        let mut toasts = Vec::new();
        push_toast(
            &mut toasts,
            Toast::new("boom").tone(Tone::Danger),
            now,
            Duration::from_secs(1),
        );
    }

    #[test]
    fn toasts_expire() {
        let now = Instant::now();
        let mut toasts = Vec::new();
        push_toast(
            &mut toasts,
            Toast::new("gone"),
            now,
            Duration::from_millis(100),
        );
        assert!(!expire_toasts(&mut toasts, now));
        assert!(expire_toasts(&mut toasts, now + Duration::from_millis(200)));
        assert!(toasts.is_empty());
    }

    #[test]
    fn working_to_idle_after_two_seconds_notifies_once() {
        let now = Instant::now();
        let (mut state, plays) = state_with_recording_sound(now);
        state.apply_bridge_event(
            BridgeEvent::Connected(Box::new(agent_snapshot(&[(
                "payroll/feat",
                "feat",
                1,
                AgentActivity::Unknown,
            )]))),
            now,
        );

        state.apply_daemon_event(agent_event("payroll/feat", 1, AgentActivity::Working), now);
        state.apply_daemon_event(
            agent_event("payroll/feat", 1, AgentActivity::Idle),
            now + Duration::from_secs(2),
        );
        state.apply_daemon_event(
            agent_event("payroll/feat", 1, AgentActivity::Idle),
            now + Duration::from_secs(3),
        );

        assert_eq!(state.toasts.len(), 1);
        assert_eq!(
            state.toasts[0].toast.text.as_ref(),
            "payroll/feat: agent finished"
        );
        assert_eq!(state.toasts[0].toast.icon, Some(Icon::CircleCheck));
        assert_eq!(state.toasts[0].toast.tone, Tone::Success);
        assert_eq!(plays.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn popup_agent_activity_survives_snapshots_and_notifies_once() {
        let now = Instant::now();
        let (mut state, plays) = state_with_recording_sound(now);
        let session = fleet_core::sessions::agent_session_id(Agent::Claude)
            .unwrap_or_else(|error| panic!("{error}"));
        let mut fixed_agent = session_with(session.as_str(), &[41]);
        fixed_agent.kind = SessionKind::Agent(Agent::Claude);
        let mut current = snapshot();
        current.sessions.push(fixed_agent);
        state.apply_bridge_event(BridgeEvent::Connected(Box::new(current.clone())), now);
        state.toggle_agent_popup(Agent::Claude);

        state.apply_daemon_event(
            agent_event(session.as_str(), 41, AgentActivity::Working),
            now,
        );
        assert_eq!(
            state.session_agent_activity(&session),
            AgentActivity::Working
        );

        state.apply_snapshot(current, now + Duration::from_secs(1));
        assert_eq!(
            state.session_agent_activity(&session),
            AgentActivity::Working,
            "an unrelated snapshot must not erase fixed-agent activity"
        );

        state.apply_daemon_event(
            agent_event(session.as_str(), 41, AgentActivity::Idle),
            now + Duration::from_secs(2),
        );
        assert_eq!(state.toasts.len(), 1);
        assert_eq!(plays.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn short_working_flicker_does_not_notify() {
        let now = Instant::now();
        let (mut state, plays) = state_with_recording_sound(now);
        state.apply_daemon_event(agent_event("payroll/feat", 1, AgentActivity::Working), now);
        state.apply_daemon_event(
            agent_event("payroll/feat", 1, AgentActivity::Idle),
            now + Duration::from_millis(1_999),
        );
        assert!(state.toasts.is_empty());
        assert_eq!(plays.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unknown_to_idle_does_not_notify() {
        let now = Instant::now();
        let (mut state, plays) = state_with_recording_sound(now);
        state.apply_daemon_event(
            agent_event("payroll/feat", 1, AgentActivity::Idle),
            now + Duration::from_secs(3),
        );
        assert!(state.toasts.is_empty());
        assert_eq!(plays.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn reconnect_with_idle_sessions_seeds_silently() {
        let now = Instant::now();
        let (mut state, plays) = state_with_recording_sound(now);
        state.apply_daemon_event(agent_event("payroll/feat", 1, AgentActivity::Working), now);
        state.apply_bridge_event(
            BridgeEvent::Reconnected {
                restarted: false,
                snapshot: Box::new(agent_snapshot(&[(
                    "payroll/feat",
                    "feat",
                    1,
                    AgentActivity::Idle,
                )])),
            },
            now + Duration::from_secs(3),
        );
        assert!(state.toasts.is_empty());
        assert_eq!(plays.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn two_sessions_finishing_produce_two_notifications() {
        let now = Instant::now();
        let (mut state, plays) = state_with_recording_sound(now);
        state.apply_daemon_event(agent_event("payroll/one", 1, AgentActivity::Working), now);
        state.apply_daemon_event(agent_event("payroll/two", 2, AgentActivity::Working), now);
        state.apply_daemon_event(
            agent_event("payroll/one", 1, AgentActivity::Idle),
            now + Duration::from_secs(2),
        );
        state.apply_daemon_event(
            agent_event("payroll/two", 2, AgentActivity::Idle),
            now + Duration::from_secs(2),
        );
        assert_eq!(state.toasts.len(), 2);
        assert_eq!(plays.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn sound_can_be_disabled_without_disabling_the_toast() {
        let now = Instant::now();
        let (mut state, plays) = state_with_recording_sound(now);
        state.notifications.sound = false;
        state.apply_daemon_event(agent_event("payroll/feat", 1, AgentActivity::Working), now);
        state.apply_daemon_event(
            agent_event("payroll/feat", 1, AgentActivity::Idle),
            now + Duration::from_secs(2),
        );
        assert_eq!(state.toasts.len(), 1);
        assert_eq!(plays.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_full_snapshot_recovers_a_missed_finish_event() {
        let now = Instant::now();
        let (mut state, plays) = state_with_recording_sound(now);
        state.apply_bridge_event(
            BridgeEvent::Connected(Box::new(agent_snapshot(&[(
                "payroll/feat",
                "feat",
                1,
                AgentActivity::Working,
            )]))),
            now,
        );
        state.apply_snapshot(
            agent_snapshot(&[("payroll/feat", "feat", 1, AgentActivity::Idle)]),
            now + Duration::from_secs(2),
        );
        assert_eq!(state.toasts.len(), 1);
        assert_eq!(plays.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cursor_movement_clamps_instead_of_wrapping() {
        assert_eq!(move_cursor(0, -1, 5), 0);
        assert_eq!(move_cursor(4, 1, 5), 4);
        assert_eq!(move_cursor(2, 2, 5), 4);
        assert_eq!(move_cursor(3, -2, 5), 1);
        assert_eq!(move_cursor(3, 1, 0), 0);
        assert_eq!(clamp_cursor(9, 3), 2);
        assert_eq!(clamp_cursor(9, 0), 0);
        assert_eq!(half_page(20), 10);
        assert_eq!(half_page(1), 1);
    }

    #[test]
    fn reconnect_backoff_matches_the_spec() {
        let seconds: Vec<u64> = (0..6).map(|n| reconnect_backoff(n).as_secs()).collect();
        assert_eq!(seconds, vec![1, 2, 4, 8, 8, 8]);
    }

    #[test]
    fn percent_is_parsed_from_a_progress_line() {
        assert_eq!(parse_percent("Receiving objects:  40% (81/202)"), Some(40));
        assert_eq!(parse_percent("100% done"), Some(100));
        assert_eq!(parse_percent("pnpm install (2/3)"), None);
        assert_eq!(parse_percent("%"), None);
    }

    #[test]
    fn breadcrumb_suppresses_empty_parts() {
        assert_eq!(
            breadcrumb(&["buk", "payroll", "feat/payroll-fix"]),
            "buk › payroll › feat/payroll-fix"
        );
        assert_eq!(breadcrumb(&["buk", "", "row"]), "buk › row");
        assert_eq!(breadcrumb(&[]), "");
    }

    #[test]
    fn chip_counts_never_collapse_unknown_into_another_chip() {
        let sessions = vec![
            SessionState::Attached,
            SessionState::Attached,
            SessionState::Detached,
            SessionState::Unknown,
            SessionState::None,
        ];
        let hosts = vec![HostStatus {
            id: "devbox".parse().unwrap_or_else(|error| panic!("{error}")),
            reachable: false,
            checked_at: "2026-09-04T12:00:00Z".to_owned(),
            error: None,
        }];
        let counts = chip_counts(&[], &sessions, &hosts, 4);
        assert_eq!(counts.live, 2);
        assert_eq!(counts.sleeping, 1);
        assert_eq!(counts.unknown, 2, "one unknown session plus one dead host");
        assert_eq!(counts.review, 4);
    }

    fn job(id: &str, status: JobStatus, finished: Option<&str>) -> JobRecord {
        JobRecord {
            id: id.parse().unwrap_or_else(|error| panic!("{error}")),
            kind: fleet_proto::job::JobKind::Clone,
            target: "nixos".to_owned(),
            title: "clone nixos".to_owned(),
            status,
            progress: None,
            log_path: "/tmp/j.log".to_owned(),
            started_at: "2026-09-04T12:00:00Z".to_owned(),
            finished_at: finished.map(str::to_owned),
            cancellable: true,
            retryable: true,
        }
    }

    #[test]
    fn the_newest_failure_owns_the_sticky_slot() {
        let jobs = vec![
            job(
                "j-1",
                JobStatus::Failed {
                    error: "old".to_owned(),
                },
                Some("2026-09-04T12:00:00Z"),
            ),
            job("j-2", JobStatus::Running, None),
            job(
                "j-3",
                JobStatus::Failed {
                    error: "new".to_owned(),
                },
                Some("2026-09-04T12:05:00Z"),
            ),
        ];
        let failed = latest_failed_job(&jobs).unwrap_or_else(|| panic!("expected a failure"));
        assert_eq!(failed.id.as_str(), "j-3");
        assert_eq!(running_jobs(&jobs).len(), 1);
    }

    #[test]
    fn context_chain_follows_the_screen_the_overlay_and_the_daemon() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        assert_eq!(state.context_chain(), vec!["Hub", "Worktrees"]);

        state.hub_pane = HubPane::Repos;
        assert_eq!(state.context_chain(), vec!["Hub", "Repos"]);

        state.hub_pane = HubPane::List;
        state.screen = Screen::Hub { tab: HubTab::Prs };
        assert_eq!(state.context_chain(), vec!["Hub", "Prs"]);

        state.screen = Screen::Workspace {
            session: "payroll/feat"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        };
        assert_eq!(state.context_chain(), vec!["Workspace", "Terminal"]);
        state.enter_prefix();
        assert_eq!(state.context_chain(), vec!["Workspace", "Prefix"]);
        state.terminal_mode = TerminalMode::Scroll;
        assert_eq!(state.context_chain(), vec!["Workspace", "Scroll"]);

        state.open_overlay(Overlay::Jobs);
        assert_eq!(state.context_chain(), vec!["Jobs"]);
        state.open_overlay(Overlay::Dialog(Dialogs::Confirm));
        assert_eq!(state.context_chain(), vec!["Dialog", "Confirm"]);
        state.close_overlay();

        state.daemon = DaemonLink::Lost {
            attempt: 1,
            dismissed: false,
        };
        assert_eq!(
            state.context_chain(),
            vec!["Workspace", "Scroll", "Daemon", "Banner"],
            "an undismissed banner owns r / l / Esc"
        );
        state.daemon = DaemonLink::Lost {
            attempt: 1,
            dismissed: true,
        };
        assert_eq!(state.context_chain(), vec!["Workspace", "Scroll"]);

        state.daemon = DaemonLink::Failed {
            message: "no socket".to_owned(),
            log_tail: Vec::new(),
            stale_socket: true,
        };
        assert_eq!(state.context_chain(), vec!["Daemon", "Down"]);
    }

    #[test]
    fn agent_popup_open_switch_hide_preserves_the_underlying_focus_state() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.hub_pane = HubPane::Repos;
        let base = state.screen.clone();

        assert_eq!(
            state.toggle_agent_popup(Agent::Claude),
            AgentPopupTransition::Opened
        );
        assert_eq!(state.screen, base);
        assert_eq!(state.hub_pane, HubPane::Repos);
        assert_eq!(state.context_chain(), vec!["Agent", "Terminal"]);

        state.open_overlay(Overlay::Dialog(Dialogs::Help));
        assert_eq!(state.context_chain(), vec!["Dialog", "Help"]);
        assert_eq!(state.mode(), Mode::Dialog);
        assert!(state.close_overlay());
        assert_eq!(state.context_chain(), vec!["Agent", "Terminal"]);

        state.enter_agent_prefix();
        assert_eq!(state.context_chain(), vec!["Agent", "Prefix"]);
        assert!(state.leave_agent_prefix());
        assert_eq!(
            state.toggle_agent_popup(Agent::Opencode),
            AgentPopupTransition::Switched
        );
        assert_eq!(state.screen, base);
        assert_eq!(
            state.agent_popup.map(|popup| popup.agent),
            Some(Agent::Opencode)
        );

        assert_eq!(
            state.toggle_agent_popup(Agent::Opencode),
            AgentPopupTransition::Hidden
        );
        assert!(state.agent_popup.is_none());
        assert_eq!(state.context_chain(), vec!["Hub", "Repos"]);
        assert_eq!(state.screen, base);
        assert_eq!(state.hub_pane, HubPane::Repos);
    }

    #[test]
    fn agent_popup_does_not_change_workspace_terminal_mode() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.screen = Screen::Workspace {
            session: "owner/repo"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        };
        state.terminal_mode = TerminalMode::Scroll;

        state.toggle_agent_popup(Agent::Claude);
        state.enter_agent_prefix();
        assert_eq!(state.terminal_mode, TerminalMode::Scroll);
        assert!(state.hide_agent_popup());
        assert_eq!(state.terminal_mode, TerminalMode::Scroll);
        assert_eq!(state.context_chain(), vec!["Workspace", "Scroll"]);
    }

    #[test]
    fn agent_prefix_restores_scroll_instead_of_bypassing_its_cleanup() {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.toggle_agent_popup(Agent::Claude);
        state
            .agent_popup
            .as_mut()
            .unwrap_or_else(|| panic!("popup must be open"))
            .mode = AgentPopupMode::Scroll;

        state.enter_agent_prefix();
        assert_eq!(
            state.agent_popup.map(|popup| popup.mode),
            Some(AgentPopupMode::Prefix)
        );
        assert!(state.leave_agent_prefix());
        assert_eq!(
            state.agent_popup.map(|popup| popup.mode),
            Some(AgentPopupMode::Scroll)
        );
    }

    #[test]
    fn missing_visible_agent_session_is_recoverable_only_while_connected() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.toggle_agent_popup(Agent::Claude);
        state.snapshot = Some(snapshot());

        assert_eq!(state.missing_agent_popup_session(), None);
        state.daemon = DaemonLink::Connected;
        assert_eq!(state.missing_agent_popup_session(), Some(Agent::Claude));
    }

    #[test]
    fn an_overlay_owns_the_keyboard_on_the_first_run_card() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.apply_snapshot(snapshot(), now);
        assert!(state.is_first_run(), "the sample snapshot is empty");
        assert_eq!(state.context_chain(), vec!["FirstRun"]);

        // §3.13 binds `?` and `,` on the card; without this the dialog opens with the
        // `FirstRun` chain and `Esc` can never match, trapping the app.
        state.open_overlay(Overlay::Dialog(Dialogs::Help));
        assert_eq!(state.context_chain(), vec!["Dialog", "Help"]);
        state.open_overlay(Overlay::Dialog(Dialogs::Settings));
        assert_eq!(state.context_chain(), vec!["Dialog", "Settings"]);
        state.close_overlay();
        assert_eq!(state.context_chain(), vec!["FirstRun"]);
    }

    #[test]
    fn first_run_never_reappears_after_a_populated_snapshot() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        let mut populated = snapshot();
        populated.contexts.push(fleet_core::model::Context {
            id: "alpha".parse().unwrap_or_else(|error| panic!("{error}")),
            name: "Alpha".to_owned(),
            owners: vec!["acme".to_owned()],
            created_at: "2026-09-05T00:00:00Z".to_owned(),
        });
        state.apply_snapshot(populated, now);
        assert!(!state.is_first_run());

        state.apply_snapshot(snapshot(), now);

        assert!(!state.is_first_run());
        assert_ne!(state.context_chain(), vec!["FirstRun"]);
    }

    #[test]
    fn a_failed_link_takes_the_window_and_drops_the_overlay() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.open_overlay(Overlay::Dialog(Dialogs::Help));
        state.apply_bridge_event(
            BridgeEvent::ConnectFailed {
                message: "no socket".to_owned(),
                log_tail: Vec::new(),
                stale_socket: true,
            },
            now,
        );
        assert!(
            state.overlay.is_none(),
            "§3.12 B draws no overlay layer, so nothing may stay open behind it"
        );
        assert_eq!(state.context_chain(), vec!["Daemon", "Down"]);
    }

    #[test]
    fn an_overlay_opened_after_the_link_failed_still_owns_the_keyboard() {
        // KM-01: `apply_bridge_event` closes what was open when the link fails, but nothing
        // stops one being opened *afterwards* — `ctrl-q` does exactly that. An overlay whose
        // keys are not in the chain answers nothing, and `Esc` cannot leave it.
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.apply_bridge_event(
            BridgeEvent::ConnectFailed {
                message: "no socket".to_owned(),
                log_tail: Vec::new(),
                stale_socket: true,
            },
            now,
        );
        assert_eq!(state.context_chain(), vec!["Daemon", "Down"]);
        state.open_overlay(Overlay::Dialog(Dialogs::Quit));
        assert_eq!(state.context_chain(), vec!["Dialog", "Quit"]);
        state.close_overlay();
        assert_eq!(state.context_chain(), vec!["Daemon", "Down"]);
    }

    #[test]
    fn every_new_link_bumps_the_generation_screens_re_attach_on() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        let start = state.link_generation;
        state.apply_bridge_event(BridgeEvent::Connected(Box::new(snapshot())), now);
        assert_eq!(state.link_generation, start + 1);

        // A disconnect alone changes nothing: the attachment is still notionally held.
        state.apply_bridge_event(BridgeEvent::Disconnected { attempt: 1 }, now);
        assert_eq!(state.link_generation, start + 1);

        // Both reconnect shapes replace the socket, so both invalidate every attachment.
        state.apply_bridge_event(
            BridgeEvent::Reconnected {
                restarted: false,
                snapshot: Box::new(snapshot()),
            },
            now,
        );
        assert_eq!(state.link_generation, start + 2);
        state.apply_bridge_event(
            BridgeEvent::Reconnected {
                restarted: true,
                snapshot: Box::new(snapshot()),
            },
            now,
        );
        assert_eq!(state.link_generation, start + 3);
    }

    #[test]
    fn prefix_is_one_shot() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.screen = Screen::Workspace {
            session: "payroll/feat"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        };
        state.enter_prefix();
        assert_eq!(state.mode(), Mode::Prefix);
        assert!(state.leave_prefix());
        assert_eq!(state.mode(), Mode::Terminal);
        assert!(!state.leave_prefix(), "leaving twice is a no-op");
    }

    #[test]
    fn prefix_is_unreachable_from_the_hub() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.enter_prefix();
        assert_eq!(state.mode(), Mode::Normal);
    }

    #[test]
    fn escape_clears_the_filter_in_two_stages_and_never_quits() {
        assert_eq!(filter_escape(true), FilterEscape::LeaveInput);
        assert_eq!(filter_escape(false), FilterEscape::ClearFilter);

        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.open_overlay(Overlay::Filter);
        state.filter.query = "rut".to_owned();
        state.filter.editing = true;

        assert!(state.cancel());
        assert!(!state.filter.editing, "the first Esc leaves the input");
        assert_eq!(state.filter.query, "rut", "and keeps the filter");
        assert!(state.overlay.is_none());

        assert!(state.cancel());
        assert!(state.filter.query.is_empty(), "the second Esc clears it");

        assert!(!state.cancel(), "a third Esc is a no-op, never a quit");
    }

    #[test]
    fn mode_word_follows_the_overlay_stack() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        assert_eq!(state.mode().word(), ModeWord::Normal);
        state.open_overlay(Overlay::Palette);
        assert_eq!(state.mode().word(), ModeWord::Palette);
        state.open_overlay(Overlay::Dialog(Dialogs::Quit));
        assert_eq!(state.mode().word(), ModeWord::Dialog);
    }

    #[test]
    fn a_daemon_restart_drops_the_mirror_grids_and_says_so() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.apply_frame(&frame(1, true, vec![row(0, "hi")]));
        assert_eq!(state.grids.len(), 1);

        state.apply_bridge_event(
            BridgeEvent::Disconnected { attempt: 2 },
            now + Duration::from_secs(1),
        );
        assert!(state.daemon.is_lost());
        assert!(state.refuses_mutations());
        assert!(state.drops_terminal_keys());

        state.apply_bridge_event(
            BridgeEvent::Reconnected {
                restarted: true,
                snapshot: Box::new(snapshot()),
            },
            now + Duration::from_secs(2),
        );
        assert!(state.grids.is_empty(), "PTYs do not survive fleetd");
        assert!(matches!(
            state.daemon,
            DaemonLink::Reconnected {
                restarted: true,
                ..
            }
        ));
    }

    #[test]
    fn the_reconnect_banner_expires_on_a_tick() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.daemon = DaemonLink::Reconnected {
            restarted: false,
            since: now,
        };
        assert!(state.tick(now));
        assert!(matches!(state.daemon, DaemonLink::Reconnected { .. }));
        assert!(state.tick(now + Duration::from_secs(1)));
        assert_eq!(state.daemon, DaemonLink::Connected);
        assert!(
            !state.tick(now + Duration::from_secs(2)),
            "a connected, idle app never repaints on a tick"
        );
    }

    #[test]
    fn a_daemon_that_will_not_start_owns_the_whole_window() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.apply_bridge_event(
            BridgeEvent::ConnectFailed {
                message: "no socket".to_owned(),
                log_tail: vec!["boom".to_owned()],
                stale_socket: true,
            },
            now,
        );
        assert_eq!(state.context_chain(), vec!["Daemon", "Down"]);
    }

    #[test]
    fn terminal_exit_and_forget_touch_only_that_grid() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.apply_frame(&frame(1, true, vec![row(0, "hi")]));
        assert!(state.grids.contains_key(&TerminalId(1)));
        state.apply_terminal_exit(TerminalId(1), Some(1));
        assert_eq!(
            state.grids.get(&TerminalId(1)).and_then(|g| g.exit_code),
            Some(Some(1))
        );
        state.forget_terminal(TerminalId(1));
        assert!(state.grids.is_empty());
    }

    fn session_with(id: &str, terminals: &[u64]) -> fleet_core::sessions::Session {
        use fleet_core::sessions::{SessionKind, Terminal, TerminalStatus};
        fleet_core::sessions::Session {
            id: id.parse().unwrap_or_else(|error| panic!("{error}")),
            kind: SessionKind::Worktree(
                "buk/payroll#feat"
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
            ),
            cwd: "/tmp".to_owned(),
            terminals: terminals
                .iter()
                .map(|id| Terminal {
                    id: TerminalId(*id),
                    name: format!("t{id}"),
                    command: "clear".to_owned(),
                    cwd: "/tmp".to_owned(),
                    shell_pid: None,
                    foreground_command: None,
                    status: TerminalStatus::Running,
                    title: None,
                    keep_alive: Vec::new(),
                    has_unseen_output: false,
                    kind: fleet_core::sessions::TerminalKind::Pty,
                })
                .collect(),
            active_terminal: terminals.first().map(|id| TerminalId(*id)),
            slept_at: None,
            kept_terminals: Vec::new(),
        }
    }

    /// A `fleet://` tab puts the Workspace in `Native`, and moving off it puts it back.
    ///
    /// The mode is what selects the key context, and `Workspace > Native` binds only `ctrl-s`,
    /// so getting this wrong either deafens the pane or lets Fleet keys leak into it.
    #[test]
    fn the_workspace_mode_follows_the_kind_of_the_active_tab() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        let session: SessionId = "payroll/feat".parse().unwrap_or_else(|e| panic!("{e}"));
        state.screen = Screen::Workspace {
            session: session.clone(),
        };

        // Otherwise the empty sample snapshot reads as the first run and owns the chain.
        state.has_seen_non_empty_state = true;

        let mut open = snapshot();
        let mut record = session_with("payroll/feat", &[1, 2, 3]);
        record.terminals[2].kind = fleet_core::sessions::TerminalKind::Native;
        record.active_terminal = Some(TerminalId(1));
        open.sessions = vec![record.clone()];
        state.apply_snapshot(open, now);
        assert_eq!(state.terminal_mode, TerminalMode::Terminal);
        assert_eq!(state.context_chain(), vec!["Workspace", "Terminal"]);
        assert!(!state.active_terminal_is_native());

        // `ctrl-s 3`.
        let mut on_native = snapshot();
        record.active_terminal = Some(TerminalId(3));
        on_native.sessions = vec![record.clone()];
        state.apply_snapshot(on_native, now);
        assert!(state.active_terminal_is_native());
        assert_eq!(state.terminal_mode, TerminalMode::Native);
        assert_eq!(state.context_chain(), vec!["Workspace", "Native"]);
        // §2.8 has no ninth word: the status bar still says the Workspace has the keyboard.
        assert_eq!(state.mode().word(), ModeWord::Terminal);

        // `ctrl-s` over the pane still enters the prefix, and leaving it comes back to Native.
        state.enter_prefix();
        assert_eq!(state.context_chain(), vec!["Workspace", "Prefix"]);
        assert!(state.leave_prefix());
        assert_eq!(state.terminal_mode, TerminalMode::Native);

        // A snapshot must never yank a transient Fleet mode away underneath the user.
        state.terminal_mode = TerminalMode::Scroll;
        let mut again = snapshot();
        again.sessions = vec![record.clone()];
        state.apply_snapshot(again, now);
        assert_eq!(state.terminal_mode, TerminalMode::Scroll);

        // And `ctrl-s 1` goes back to a PTY.
        state.terminal_mode = TerminalMode::Native;
        let mut back = snapshot();
        record.active_terminal = Some(TerminalId(1));
        back.sessions = vec![record];
        state.apply_snapshot(back, now);
        assert_eq!(state.terminal_mode, TerminalMode::Terminal);
        assert_eq!(state.context_chain(), vec!["Workspace", "Terminal"]);
    }

    #[test]
    fn a_snapshot_without_a_terminal_frees_its_mirror_and_its_mru_entry() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        let session: SessionId = "payroll/feat".parse().unwrap_or_else(|e| panic!("{e}"));

        let mut open = snapshot();
        open.sessions = vec![session_with("payroll/feat", &[1, 2])];
        state.apply_snapshot(open, now);
        state.apply_frame(&frame(1, true, vec![row(0, "one")]));
        let mut second = frame(1, true, vec![row(0, "two")]);
        second.terminal = TerminalId(2);
        state.apply_frame(&second);
        state.touch_terminal(&session, TerminalId(1));
        state.touch_terminal(&session, TerminalId(2));
        assert_eq!(state.grids.len(), 2);

        // `ctrl-s x` on terminal 2: the daemon drops it from the session, and nothing can ever
        // paint its 12 000 cells again.
        let mut closed = snapshot();
        closed.sessions = vec![session_with("payroll/feat", &[1])];
        state.apply_snapshot(closed, now);
        assert_eq!(state.grids.keys().collect::<Vec<_>>(), vec![&TerminalId(1)]);
        assert_eq!(
            state.terminal_mru.get(&session).map(Mru::entries),
            Some(&vec![TerminalId(1)][..])
        );

        // Killing the session frees the rest, MRU included.
        state.apply_snapshot(snapshot(), now);
        assert!(state.grids.is_empty());
        assert!(state.terminal_mru.is_empty());
    }
}
