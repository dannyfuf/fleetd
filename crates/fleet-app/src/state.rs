//! Daemon mirrors and local interaction state, reduced without foreground I/O.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use fleet_core::{
    config::{Agent, NotificationsConfig},
    github::PrTab,
    ids::{ContextId, JobId, RepoId, SessionId, TerminalId},
    sessions::{AgentActivity, Session, SessionKind, aggregate_agent_activity},
};
use fleet_proto::{
    event::{Event, ToastLevel},
    job::{JobRecord, JobStatus},
    snapshot::Snapshot,
    terminal::{
        Cell, CellWidth, CursorShape, CursorState, FrameUpdate, TerminalModes, ViewportInfo,
    },
};
use fleet_ui_kit::{Icon, Mode as ModeWord, PrBadgeState, Toast, ToastDuration, Tone};

use crate::{
    bridge::BridgeEvent,
    dialogs::Dialogs,
    notify_sound::{NotificationSound, SystemSound},
};

mod connection;
mod navigation;
mod notifications;
mod snapshot;
mod terminal;
#[cfg(test)]
mod test_support;

pub use connection::{DaemonLink, daemon_log_path, reconnect_backoff};
use navigation::clamp_cursor;
pub use navigation::{
    AgentPopupMode, AgentPopupState, AgentPopupTransition, Cursors, FilterEscape, FilterState,
    HubPane, HubTab, Mode, Mru, Overlay, RepoScope, Screen, TerminalMode, filter_escape, half_page,
    move_cursor,
};
pub use notifications::{
    LiveToast, StickyError, dwell_for, expire_toasts, latest_failed_job, running_jobs,
};
pub use snapshot::{ChipCounts, breadcrumb};
pub use terminal::MirrorGrid;

/// Longest window in which two identical toasts coalesce into one `×n` toast (§2.7).
const TOAST_COALESCE_WINDOW: Duration = Duration::from_millis(fleet_ui_kit::COALESCE_WINDOW_MS);
/// Maximum number of stacked toasts (§2.7).
const MAX_TOASTS: usize = fleet_ui_kit::ToastStack::MAX;
/// How long a `Starting fleetd…` splash waits before it appends the socket path (§3.12 A).
pub const SPLASH_DETAIL_DELAY: Duration = Duration::from_secs(3);
/// How long the mandatory "terminal sessions did not survive" banner stays up (§3.12).
pub const RESTART_BANNER_DWELL: Duration = Duration::from_secs(6);
/// How long a plain "reconnected" banner stays up (§3.12).
pub const RECONNECT_BANNER_DWELL: Duration = Duration::from_millis(800);
/// How many entries an MRU list keeps.
const MRU_CAPACITY: usize = 32;
/// Minimum observed working time before an idle transition is treated as a completed turn.
const AGENT_FINISH_MIN_WORKING: Duration = Duration::from_secs(2);

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
    pub seen_failed: HashSet<JobId>,
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
    /// Explicit names override OSC titles; the daemon record has no rename flag.
    pub renamed_terminals: HashSet<TerminalId>,
    /// Each new connection has no attachments. Screens reattach when this generation changes.
    pub link_generation: u64,
    /// Diagnostic results shared by the daemon splash and Settings.
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
            snapshot_revision: 0,
            has_seen_non_empty_state: false,
            snapshot_at: None,
            grids: HashMap::new(),
            watches: crate::watches::Watches::default(),
            screen: Screen::hub(),
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
            session_mru: Mru::default(),
            terminal_mru: HashMap::new(),
            toasts: Vec::new(),
            last_agent_activity: HashMap::new(),
            notification_sound: Box::new(SystemSound),
            sticky_error: None,
            seen_failed: HashSet::new(),
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
}
