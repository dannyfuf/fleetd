//! Daemon mirrors and local interaction state, reduced without foreground I/O.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use fleet_core::{
    agents::AttentionKind,
    board::BackendDescriptor,
    config::{Agent, NotificationsConfig},
    github::PrTab,
    ids::{ContextId, HostId, JobId, RepoId, SessionId, TerminalId, WorktreeId},
    sessions::{AgentActivity, Session, SessionKind, aggregate_agent_activity},
};
use fleet_proto::{
    event::{Event, ToastLevel},
    job::{JobRecord, JobStatus},
    snapshot::{LinkState, Snapshot},
    terminal::{
        Cell, CellWidth, CursorShape, CursorState, FrameUpdate, TerminalModes, ViewportInfo,
    },
};
use fleet_ui_kit::{Icon, PrBadgeState, Toast, ToastDuration, Tone};

use crate::{
    bridge::BridgeEvent,
    dialogs::Dialogs,
    notify_sound::{NotificationSound, SystemSound},
};

mod agents;
mod board;
mod connection;
mod harness;
mod jobs_filter;
mod navigation;
mod notifications;
mod snapshot;
mod terminal;
#[cfg(test)]
mod test_support;

pub use agents::{AgentCounts, AgentThreads};
pub use board::{BoardFocus, BoardScope, BoardState, GroupBy, WORKTREE_BOARDS_UNSUPPORTED};
pub use connection::{DaemonLink, DaemonLossReason, daemon_log_path, reconnect_backoff};
use harness::HarnessCache;
pub use harness::{
    AgentThreadSnapshot, AgentsSnapshot, BoundsSnapshot, CursorSnapshot, DialogSnapshot,
    FieldSnapshot, HarnessProjection, HarnessState, IdleSnapshot, IdleWake, JobSnapshot,
    ListSnapshot, MUTATION_SETTLE_GRACE, RowSnapshot, SNAPSHOT_VERSION, SettleCounter,
    TargetSnapshot, TerminalSnapshot, ToastSnapshot, UiSnapshot, ViewportSnapshot, WindowSnapshot,
};
pub use jobs_filter::JobFilter;
use navigation::clamp_cursor;
pub use navigation::{
    AgentPopupMode, AgentPopupState, AgentPopupTransition, Cursors, FilterEscape, FilterState,
    HubPane, HubTab, Mode, Mru, Overlay, RepoScope, Screen, TerminalMode, WORKSPACE_TAB_LIMIT,
    WORKSPACE_TAB_LIMIT_NOTICE, filter_escape, half_page, move_cursor,
};
pub use notifications::{
    LiveToast, StickyError, ToastTarget, dwell_for, expire_toasts, latest_failed_job, running_jobs,
};
pub use snapshot::{ChipCounts, breadcrumb};
pub use terminal::MirrorGrid;

/// Longest window in which two identical toasts coalesce into one `×n` toast (§2.7).
const TOAST_COALESCE_WINDOW: Duration = Duration::from_millis(fleet_ui_kit::COALESCE_WINDOW_MS);
/// Maximum number of stacked toasts (§2.7).
const MAX_TOASTS: usize = fleet_ui_kit::ToastStack::MAX;
/// How long a `Starting fleetd…` splash waits before it appends the socket path (§3.12 A).
pub const SPLASH_DETAIL_DELAY: Duration = Duration::from_secs(3);
/// How long the mandatory "fleetd restarted" banner stays up (§3.12).
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

/// The Jobs panel's own selection and filter, mirrored for non-render projections.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobsPanelMirror {
    /// Selected row in the panel's filtered rows.
    pub cursor: usize,
    /// Filter currently applied by the panel.
    pub filter: JobFilter,
}

/// What one inspection of the Workspace's worktree says about its git state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceGit {
    /// The worktree inspected; the facts are dropped when the Workspace shows another.
    pub worktree: WorktreeId,
    /// Commits ahead of the upstream, when divergence could be computed.
    pub ahead: Option<u64>,
    /// Commits behind the upstream, when divergence could be computed.
    pub behind: Option<u64>,
    /// Whether tracked or untracked changes exist.
    pub dirty: bool,
    /// How many porcelain entries are dirty, when status collection succeeded.
    pub dirty_files: Option<u64>,
}

/// The whole client-side state of the app.
#[derive(Debug)]
pub struct AppState {
    /// `$FLEET_HOME`.
    pub home: PathBuf,
    /// The daemon connection situation (§3.12).
    pub daemon: DaemonLink,
    /// Optional IPC-v4 behaviors advertised by the connected daemon.
    pub daemon_capabilities: HashSet<String>,
    /// When the current [`DaemonLink`] was entered, for the splash and banner timings.
    pub daemon_since: Instant,
    /// The sibling/on-path fleetd binary is newer than the connected daemon process.
    pub daemon_outdated: bool,
    /// The authoritative daemon snapshot, absent until the first one arrives.
    pub snapshot: Option<Snapshot>,
    /// Bumped by every applied snapshot mutation, so projections key their caches on one
    /// integer instead of deep-comparing the collections they read.
    pub snapshot_revision: u64,
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
    /// The Jobs panel's own cursor and filter.
    pub jobs_panel: JobsPanelMirror,
    /// Stable identities from the rows currently prepared for the Hub.
    pub displayed_hub: crate::presentation::DisplayedHub,
    /// The Workspace sub-mode.
    pub terminal_mode: TerminalMode,
    /// Whether a native agent tab's `^s` is held, waiting for its second key.
    ///
    /// The chord publishes no key context (the tab's chain is derived from daemon state and
    /// has no room for a one-shot context), so the harness snapshot does not project it; it is state
    /// only so the ⌃S command menu can appear over the thread while it is held. The shell's
    /// keystroke interceptor sets it on `^s` and clears it on the very next key.
    pub agent_chord_armed: bool,
    /// The floating agent popup, independent of the base Hub or Workspace screen.
    pub agent_popup: Option<AgentPopupState>,
    /// The native structured agent threads: daemon summaries, opened projections, seen cursors.
    pub agents: AgentThreads,
    /// Effective history and wheel configuration.
    pub terminal_config: fleet_core::config::TerminalConfig,
    /// Enabled channels for semantic agent-attention notifications.
    pub notifications: NotificationsConfig,
    /// The overlay that owns the keyboard, when any.
    pub overlay: Option<Overlay>,
    /// The context word derived from the open dialog's draft.
    ///
    /// Dialog drafts live in `DialogHost`; this mirror lets `context_chain` remain the single
    /// authoritative chain used by rendering and the input-generation gate. The dialog and word
    /// travel together so a newly opened dialog can never inherit the previous dialog's word.
    dialog_key_context: Option<(Dialogs, &'static str)>,
    /// The filter of the focused list.
    pub filter: FilterState,
    /// Whether the user opened (`Some(true)`) or closed (`Some(false)`) the detail panel with `i`;
    /// `None` until they do, which means the width decides ([`AppState::detail_visible`]).
    /// Never focusable.
    pub detail_open: Option<bool>,
    /// Whether the Hub sidebar is collapsed to its icon column (`H`).
    pub rail_collapsed: bool,
    /// The width the Hub sidebar's edge was dragged to; `None` is `metrics.sidebar_w`. Kept while
    /// Fleet runs, not across restarts.
    pub sidebar_w: Option<gpui::Pixels>,
    /// Whether the Workspace hides its header and tab strip (`ctrl-s z`).
    pub zoomed: bool,
    /// Session MRU: `ctrl-s w` jumps to [`Mru::alternate`].
    pub session_mru: Mru<SessionId>,
    /// Per-session terminal MRU: `ctrl-s Tab` jumps to [`Mru::alternate`].
    pub terminal_mru: HashMap<SessionId, Mru<TerminalId>>,
    /// The live toasts, oldest first.
    pub toasts: Vec<LiveToast>,
    /// Most recently observed aggregate heuristic activity, per session.
    last_agent_activity: HashMap<SessionId, AgentActivity>,
    /// Most recently observed authoritative terminal attention, per session.
    last_agent_attention: HashMap<SessionId, Option<AttentionKind>>,
    /// Audible completion signal, replaceable by a recording implementation in tests.
    notification_sound: Box<dyn NotificationSound>,
    /// The sticky error slot.
    pub sticky_error: Option<StickyError>,
    /// Failed jobs whose sticky slot was acknowledged by opening Jobs.
    pub seen_failed: HashSet<JobId>,
    /// Failed job the next Jobs opening should focus, even after acknowledging its sticky slot.
    pub jobs_focus: Option<JobId>,
    /// Whether the Jobs panel, when it next opens, expands the log of `jobs_focus` as `⏎`
    /// would: `View log` asks for the log, not just the row. One-shot; the panel clears it.
    pub jobs_open_log: bool,
    /// Initial palette query consumed when the palette next opens.
    pub palette_seed: Option<String>,
    /// The pull requests the PR screen had loaded when the palette was last opened from the
    /// shell, consumed with [`Self::palette_seed`].
    pub palette_prs: Option<std::rc::Rc<[crate::dialogs::PalettePr]>>,
    /// A pull request the palette sent the PR screen to: the Hub anchors its cursor on it the
    /// next time it reconciles that list, then clears this.
    pub pending_pr_focus: Option<(RepoId, u64)>,
    /// An action a surface that just closed asked to run on the surface behind it, by name:
    /// Help's rows and step buttons. The shell dispatches it once the frame that gave the
    /// keyboard back has painted, so it reaches the same listener its key would.
    pub pending_action: Option<&'static str>,
    /// Last worktree trash entry returned by fleetd, for `u`.
    pub last_trash_entry: Option<String>,
    /// Pull-request badges shared by Hub and Workspace, keyed by repository and head branch.
    pub pr_badges: HashMap<(RepoId, String), (u64, PrBadgeState)>,
    /// The git facts of the worktree the Workspace shows, from its last inspection: the title
    /// bar's `↑2 ↓0` and `3 files changed` chips (UX-SPEC §3.6).
    pub workspace_git: Option<WorkspaceGit>,
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
    /// Explicit names override OSC titles; the daemon record has no rename flag.
    pub renamed_terminals: HashSet<TerminalId>,
    /// Each new connection has no attachments. Screens reattach when this generation changes.
    pub link_generation: u64,
    /// Terminals the daemon asked this client to attach again (§3 `TerminalReattach`).
    ///
    /// A remote link that comes back re-creates the daemon-side attachment set without changing
    /// a single terminal id, so nothing a screen already reads would tell it to act. The event
    /// is recorded here and consumed by the surface that owns the terminal.
    pub reattach_pending: HashSet<TerminalId>,
    /// Diagnostic results shared by the daemon splash and Settings.
    pub doctor: Option<Vec<fleet_proto::response::DoctorCheck>>,
    /// What only the external test harness needs and nothing else in the app keeps: window
    /// metrics, recorded target rectangles and the pending-work counters `idle` is derived
    /// from (`docs/TESTING-HARNESS.md` §3).
    pub harness: HarnessState,
    /// The memoised harness projection, keyed on every input its builder reads.
    ///
    /// `RefCell` for the same reason the Hub's projection cache uses one: a memo is an
    /// implementation detail of a `&self` read, not observable state, and nothing subscribes to
    /// it. It stays `None` until the harness socket asks for the first snapshot, so an app
    /// running without the harness never allocates it.
    harness_cache: RefCell<Option<Box<HarnessCache>>>,
}

impl AppState {
    /// The state the app starts in: cold start, Hub, worktrees, nothing open.
    #[must_use]
    pub fn new(home: impl Into<PathBuf>, now: Instant) -> Self {
        Self {
            home: home.into(),
            daemon: DaemonLink::Starting,
            daemon_capabilities: HashSet::new(),
            daemon_since: now,
            daemon_outdated: false,
            snapshot: None,
            snapshot_revision: 0,
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
            jobs_panel: JobsPanelMirror::default(),
            displayed_hub: crate::presentation::DisplayedHub::default(),
            terminal_mode: TerminalMode::Terminal,
            agent_chord_armed: false,
            agent_popup: None,
            agents: AgentThreads::default(),
            terminal_config: fleet_core::config::TerminalConfig::default(),
            notifications: NotificationsConfig::default(),
            overlay: None,
            dialog_key_context: None,
            filter: FilterState::default(),
            detail_open: None,
            rail_collapsed: false,
            sidebar_w: None,
            zoomed: false,
            session_mru: Mru::default(),
            terminal_mru: HashMap::new(),
            toasts: Vec::new(),
            last_agent_activity: HashMap::new(),
            last_agent_attention: HashMap::new(),
            notification_sound: Box::new(SystemSound),
            sticky_error: None,
            seen_failed: HashSet::new(),
            jobs_focus: None,
            jobs_open_log: false,
            palette_seed: None,
            palette_prs: None,
            pending_pr_focus: None,
            pending_action: None,
            last_trash_entry: None,
            pr_badges: HashMap::new(),
            workspace_git: None,
            warn_before_quit: true,
            review_pr_count: 0,
            update_version: None,
            breadcrumb_row: None,
            renamed_terminals: HashSet::new(),
            link_generation: 0,
            reattach_pending: HashSet::new(),
            doctor: None,
            harness: HarnessState::default(),
            harness_cache: RefCell::new(None),
        }
    }

    /// Whether the detail panel is drawn: what the user chose with `i`, else `wide` — the
    /// panel is on by default where it fits beside the list (UX-SPEC §3.4).
    #[must_use]
    pub fn detail_visible(&self, wide: bool) -> bool {
        self.detail_open.unwrap_or(wide)
    }

    /// Mirrors the Jobs panel's own cursor and filter. Returns true if anything changed.
    pub fn set_jobs_panel(&mut self, mirror: JobsPanelMirror) -> bool {
        if self.jobs_panel == mirror {
            return false;
        }
        self.jobs_panel = mirror;
        true
    }
}

#[cfg(test)]
mod jobs_panel_mirror_tests {
    use super::*;

    #[test]
    fn jobs_panel_setter_reports_only_real_changes() {
        let mut state = AppState::new("/tmp/fleet-jobs-panel-mirror", Instant::now());
        assert!(!state.set_jobs_panel(JobsPanelMirror::default()));

        let mirror = JobsPanelMirror {
            cursor: 2,
            filter: JobFilter::Failed,
        };
        assert!(state.set_jobs_panel(mirror));
        assert_eq!(state.jobs_panel, mirror);
        assert!(!state.set_jobs_panel(mirror));
    }
}
