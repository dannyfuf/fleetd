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
    github::PrTab,
    ids::{ContextId, JobId, RepoId, SessionId, TerminalId},
    sessions::{Session, SessionState},
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

use crate::{bridge::BridgeEvent, dialogs::Dialogs};

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
/// How many entries an MRU list keeps.
const MRU_CAPACITY: usize = 32;

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
    /// One-shot, entered by `ctrl-s`, left by the very next key.
    Prefix,
    /// Scrollback and copy mode.
    Scroll,
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
            Self::Terminal => ModeWord::Terminal,
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
    /// The screen being shown.
    pub screen: Screen,
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
    /// Effective history and wheel configuration.
    pub terminal_config: fleet_core::config::TerminalConfig,
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
            screen: Screen::hub(),
            hub_pane: HubPane::List,
            pr_tab: PrTab::Mine,
            scope: RepoScope::All,
            cursors: Cursors::default(),
            terminal_mode: TerminalMode::Terminal,
            terminal_config: fleet_core::config::TerminalConfig::default(),
            overlay: None,
            filter: FilterState::default(),
            detail_open: false,
            rail_collapsed: false,
            zoomed: false,
            session_mru: Mru::new(),
            terminal_mru: HashMap::new(),
            toasts: Vec::new(),
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
        let mut chain = match &self.screen {
            Screen::Hub { tab } => vec![
                "Hub",
                match (self.hub_pane, tab) {
                    (HubPane::Repos, _) => "Repos",
                    (HubPane::List, HubTab::Worktrees) => "Worktrees",
                    (HubPane::List, HubTab::Prs) => "Prs",
                },
            ],
            Screen::Workspace { .. } => vec![
                "Workspace",
                match self.terminal_mode {
                    TerminalMode::Terminal => "Terminal",
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
        match self.screen {
            Screen::Hub { .. } => Mode::Normal,
            Screen::Workspace { .. } => match self.terminal_mode {
                TerminalMode::Terminal => Mode::Terminal,
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

    /// Leaves the prefix, whatever the key was. Called for **every** key seen in `Prefix`,
    /// which is what makes the mode one-shot without a timeout.
    ///
    /// Returns whether the mode actually changed.
    pub fn leave_prefix(&mut self) -> bool {
        if self.terminal_mode == TerminalMode::Prefix {
            self.terminal_mode = TerminalMode::Terminal;
            true
        } else {
            false
        }
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

    /// Replaces the snapshot mirror and re-derives everything that hangs off it.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot, now: Instant) {
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
        let mut changed = expire_toasts(&mut self.toasts, now);
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
            BridgeEvent::Connected(snapshot) => {
                self.daemon = DaemonLink::Connected;
                self.daemon_since = now;
                self.link_generation = self.link_generation.wrapping_add(1);
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
                }
                self.daemon = DaemonLink::Reconnected {
                    restarted,
                    since: now,
                };
                self.daemon_since = now;
                // The daemon-side connection is new and holds no attachments, whether or not
                // fleetd itself restarted.
                self.link_generation = self.link_generation.wrapping_add(1);
                self.apply_snapshot(*snapshot, now);
            }
            BridgeEvent::Daemon(event) => self.apply_daemon_event(*event, now),
            // Broadcast lag may affect any terminal, including one with no subsequent frame.
            // Marking every mirror desynced is what stops
            // a diff from being applied on top of rows that are already wrong; the shell then
            // asks for a full frame per terminal, which is the only thing that repairs them.
            BridgeEvent::EventsLagged { .. } => {
                self.desync_grids();
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
            Event::SnapshotChanged(snapshot) => self.apply_snapshot(snapshot, now),
            Event::JobUpdated(job) => self.apply_job(job, now),
            Event::SessionChanged(session) => self.apply_session(session),
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
                })
                .collect(),
            active_terminal: terminals.first().map(|id| TerminalId(*id)),
            slept_at: None,
            kept_terminals: Vec::new(),
        }
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
