//! Hub coordination, with prepared rows and selection-owned background work.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::{Duration, Instant},
};

use fleet_core::{
    board::BoardSummary,
    github::{PrTab, PullRequest, worktree_matches_pr},
    ids::{ContextId, RepoId, WorktreeId},
    model::{CloneJob, Context, Repo, Worktree},
    sessions::{AgentActivity, SessionState},
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    job::{JobKind, JobStatus},
    request::RequestBody,
    response::{PrSlice, ResponseBody},
};
use fleet_ui_kit::{
    ActiveTheme, HarnessTargetExt, Icon, InputMode, SplitLayout, TextInput, TextInputEvent, Toast,
    ToastDuration,
};
use gpui::{
    AnyElement, App, ClipboardItem, Entity, FocusHandle, IntoElement, Pixels, ScrollHandle,
    SharedString, Subscription, Task, UniformListScrollHandle, Window, div, prelude::*,
};

use crate::{
    actions::{fleet, hub, prs, repos, worktrees},
    bridge::Bridge,
    dialogs::{self, Dialogs},
    presentation::{DisplayedTarget, age_secs, now_unix},
    state::{AppState, HubPane, HubTab, Overlay, RepoScope, Screen, dwell_for, move_cursor},
    views::{
        detail::{self, Inspected, PrProps, RepoProps, WorktreeProps},
        hub_context_bar,
        prs_screen::{self, PrInputs, PrProps as PrScreenProps, PrRow},
        repos_rail::{self, RailKind, RailProps, RailRow},
        worktrees_list::{self, ListProps, RowInputs, WorktreeRow},
    },
};

mod actions;
mod cache;
mod composition;
mod navigation;
mod palette;
mod pr_pointer;
mod projection;
mod sidebar;
#[cfg(test)]
pub(crate) mod tests;

pub use cache::PrCache;
pub(crate) use cache::PrFreshness;
use composition::publish_breadcrumb;
pub use projection::HubModel;

type PrIdentity = (RepoId, u64);

/// The context every Hub and chrome projection uses when the daemon has no explicit selection.
#[must_use]
pub(crate) fn effective_context(state: &AppState) -> Option<&Context> {
    let contexts = state.snapshot.as_ref()?.contexts.as_slice();
    state
        .active_context()
        .and_then(|active| contexts.iter().find(|context| &context.id == active))
        .or_else(|| contexts.first())
}

fn client_error(message: impl Into<String>) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unknown,
        message: message.into(),
    }
}

/// How long the cursor must sit still before the selected worktree is re-inspected (§2.6 D-4).
pub const AUTO_INSPECT_DEBOUNCE: Duration = Duration::from_millis(400);
/// How often the Hub refreshes every eligible worktree while the Hub is visible (§2.6 D-4).
pub const INSPECTION_SWEEP_VISIBLE_INTERVAL: Duration = Duration::from_secs(60);
/// How often the Hub prefetches inspection facts while another screen is visible (§2.6 D-4).
pub const INSPECTION_SWEEP_HIDDEN_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Minimum age of the last completed sweep before re-entering Worktrees starts another one.
pub const INSPECTION_SWEEP_ENTRY_FRESHNESS: Duration = Duration::from_secs(30);
/// How long a pull-request slice stays fresh, mirroring `github.prTtlSeconds`.
pub const PR_TTL: Duration = Duration::from_secs(90);
/// Below this window width the detail panel docks instead of insetting (§3.4).
const DETAIL_INSET_MIN_WIDTH: f32 = 1120.0;

/// What one frame of the Hub body is drawn against: the window geometry, the clock and the
/// pointer contract of the worktree rows.
struct Frame<'a> {
    /// Logical window width in pixels.
    width: f32,
    /// The epoch second the frame is drawn at.
    now: i64,
    /// What a worktree row does when the pointer uses it.
    handlers: &'a worktrees_list::RowHandlers,
    /// What the sidebar's rows and edge do when the pointer uses them.
    sidebar: &'a repos_rail::SidebarHandlers,
}

/// Whether the detail panel docks over the list instead of being inset (§3.4).
#[must_use]
pub fn detail_is_docked(window_width: f32) -> bool {
    window_width < DETAIL_INSET_MIN_WIDTH
}

/// The sidebar's width this frame: its icon column while collapsed (`H`), else the width its
/// edge was dragged to, else `metrics.sidebar_w` — always within the drag range.
#[must_use]
pub(crate) fn sidebar_width(state: &AppState, metrics: &fleet_ui_kit::theme::Metrics) -> Pixels {
    if state.rail_collapsed {
        metrics.sidebar_collapsed_w
    } else {
        state
            .sidebar_w
            .unwrap_or(metrics.sidebar_w)
            .clamp(metrics.sidebar_min_w, metrics.sidebar_max_w)
    }
}

/// The list pane's width in `ch`, which is what resolves every §2.9 ladder.
///
/// The sidebar never moves and the docked panel never steals width, so the only variables are
/// the sidebar's width (collapsed, default or dragged) and an **inset** detail panel.
#[must_use]
fn list_pane_ch(
    window_width: f32,
    sidebar_width: Pixels,
    detail_open: bool,
    metrics: &fleet_ui_kit::theme::Metrics,
) -> f32 {
    let detail = if detail_open && !detail_is_docked(window_width) {
        f32::from(metrics.detail_w)
    } else {
        0.0
    };
    ((window_width - f32::from(sidebar_width) - detail) / fleet_ui_kit::theme::CH).max(1.0)
}

/// The Hub's own mutable state, held as an entity so `on_action` listeners can write it.
#[derive(Default)]
pub struct HubState {
    /// The client's inspection cache, keyed by worktree.
    pub inspections: HashMap<WorktreeId, Inspected>,
    /// The pull-request tabs.
    pub prs: PrCache,
    /// Bumped on every cursor move; an older debounce wakes up and does nothing.
    pub inspect_generation: u64,
    inspect_task: Option<Task<()>>,
    inspection_sweep_task: Option<Task<()>>,
    inspection_sweep_generation: u64,
    inspection_sweep_request: Option<u64>,
    inspection_sweep_pending: bool,
    inspection_sweep_finished_at: Option<Instant>,
    inspection_sweep_scope: Option<InspectionSweepScope>,
    inspection_sweep_missing: HashSet<WorktreeId>,
    inspection_sweep_timer_hub_visible: Option<bool>,
    inspection_sweep_worktrees_visible: bool,
    inspection_sweep_connected: bool,
    pr_refresh_task: Option<Task<()>>,
    /// Bumped on every reschedule; an older PR refresh timer wakes up and does nothing.
    pr_refresh_generation: u64,
    inspect_target: Option<WorktreeId>,
    inspection_sequence: u64,
    /// Newest explicit per-row request, retained after completion to fence older sweep replies.
    inspection_requests: HashMap<WorktreeId, u64>,
    inspection_identities: HashMap<WorktreeId, WorktreeIdentity>,
    last_reconciled_revision: u64,
    #[cfg(test)]
    inspection_reconciliations: usize,
    presentation_revision: u64,
    projection: RefCell<projection::ProjectionCache>,
    prepared: Rc<HubModel>,
    selection: SelectionAnchors,
    /// Whether the Review tab is active, for detecting one schedules retry opportunity per
    /// activation without coupling request initiation to rendering.
    review_schedule_active: bool,
    /// The context whose Review tab activation is being tracked.
    review_schedule_context: Option<ContextId>,
    /// Kept until the Reviews board arrives, since changing scopes clears the board mirror.
    review_schedule_retry_pending: bool,
    /// Pull requests whose worktree is being created, so their glyph spins in place (§3.5).
    pub creating: Vec<(RepoId, u64)>,
    creation_intents: HashMap<PrIdentity, PrCreateIntent>,
    restoring_trash: Option<String>,
    /// `$HOME`, so the prepared worktree paths are tilde-collapsed once.
    home: Option<std::path::PathBuf>,
}

impl HubState {
    fn observe_review_schedule_activation(&mut self, shown: bool, context: Option<&ContextId>) {
        let activated = shown
            && (!self.review_schedule_active || self.review_schedule_context.as_ref() != context);
        self.review_schedule_active = shown;
        self.review_schedule_context = shown.then(|| context.cloned()).flatten();
        if activated {
            self.review_schedule_retry_pending = true;
        } else if !shown {
            self.review_schedule_retry_pending = false;
        }
    }

    fn take_review_schedule_activation(&mut self) -> bool {
        std::mem::take(&mut self.review_schedule_retry_pending)
    }

    /// Marks the client-owned caches as changed, so the next projection is rebuilt.
    fn invalidate(&mut self) {
        self.presentation_revision = self.presentation_revision.wrapping_add(1);
    }

    fn begin_inspection(&mut self, id: WorktreeId) -> u64 {
        self.inspection_sequence = self.inspection_sequence.wrapping_add(1);
        let request = self.inspection_sequence;
        self.inspection_requests.insert(id.clone(), request);
        let slot = self.inspections.entry(id).or_default();
        slot.loading = true;
        slot.error = None;
        request
    }

    fn begin_inspection_sweep(&mut self) -> Option<u64> {
        if self.inspection_sweep_request.is_some() {
            return None;
        }
        self.inspection_sequence = self.inspection_sequence.wrapping_add(1);
        let request = self.inspection_sequence;
        self.inspection_sweep_request = Some(request);
        self.inspection_sweep_pending = false;
        Some(request)
    }

    fn apply_inspection_sweep(
        &mut self,
        request: u64,
        result: Result<ResponseBody, ProtoError>,
        now: Instant,
    ) -> bool {
        if self.inspection_sweep_request != Some(request) {
            return false;
        }
        self.inspection_sweep_request = None;
        let inspections = match result {
            Ok(ResponseBody::Inspections(inspections)) => {
                self.inspection_sweep_finished_at = Some(now);
                inspections
            }
            Ok(response) => {
                self.inspection_sweep_finished_at = None;
                tracing::warn!(
                    ?response,
                    "inspection sweep returned an unexpected response"
                );
                return false;
            }
            Err(error) => {
                self.inspection_sweep_finished_at = None;
                tracing::debug!(error = %error.message, "inspection sweep failed");
                return false;
            }
        };
        let mut changed = false;
        for inspection in inspections {
            let id = inspection.worktree_id.clone();
            if self
                .inspection_requests
                .get(&id)
                .is_some_and(|newer| *newer > request)
            {
                tracing::debug!(worktree = %id, "discarded stale inspection sweep result");
                continue;
            }
            let slot = self.inspections.entry(id.clone()).or_default();
            if inspection.error.is_some()
                && slot
                    .data
                    .as_ref()
                    .is_some_and(|current| current.error.is_none())
            {
                tracing::debug!(worktree = %id, "inspection sweep kept newer good facts");
                continue;
            }
            let loading = slot.loading;
            if slot.data.as_ref() != Some(&inspection) || slot.error.is_some() {
                slot.data = Some(inspection);
                slot.error = None;
                slot.loading = loading;
                changed = true;
            }
            self.inspection_sweep_missing.remove(&id);
        }
        if changed {
            self.invalidate();
        }
        changed
    }

    fn apply_inspection(
        &mut self,
        id: &WorktreeId,
        request: u64,
        result: Result<ResponseBody, ProtoError>,
    ) -> bool {
        if self.inspection_requests.get(id) != Some(&request) {
            return false;
        }
        let slot = self.inspections.entry(id.clone()).or_default();
        match result {
            Ok(ResponseBody::Inspections(inspections)) => {
                if let Some(inspection) = inspections
                    .into_iter()
                    .find(|inspection| &inspection.worktree_id == id)
                {
                    *slot = Inspected::ready(inspection);
                } else {
                    slot.loading = false;
                    slot.error = None;
                }
            }
            Ok(_) => {
                slot.loading = false;
                slot.error = Some("daemon returned an unexpected inspection response".to_owned());
            }
            Err(error) => {
                slot.loading = false;
                slot.error = Some(error.message);
            }
        }
        self.invalidate();
        true
    }

    fn reconcile_inspection_identities(&mut self, worktrees: &[Worktree]) {
        #[cfg(test)]
        {
            self.inspection_reconciliations += 1;
        }
        let current: HashMap<_, _> = worktrees
            .iter()
            .map(|worktree| (worktree.id.clone(), WorktreeIdentity::from(worktree)))
            .collect();
        self.inspections.retain(|id, _| {
            self.inspection_identities
                .get(id)
                .zip(current.get(id))
                .is_some_and(|(old, new)| old == new)
        });
        self.inspection_requests.retain(|id, _| {
            self.inspection_identities
                .get(id)
                .zip(current.get(id))
                .is_some_and(|(old, new)| old == new)
        });
        self.inspection_identities = current;
    }

    fn begin_pr_creation(&mut self, key: PrIdentity, open: bool, sleep_previous: bool) -> bool {
        if let Some(intent) = self.creation_intents.get_mut(&key) {
            if open {
                intent.open = true;
                intent.sleep_previous = sleep_previous;
            }
            return false;
        }
        self.creating.push(key.clone());
        self.creation_intents.insert(
            key,
            PrCreateIntent {
                open,
                sleep_previous,
            },
        );
        self.invalidate();
        true
    }

    fn finish_pr_creation(&mut self, key: &PrIdentity) -> Option<PrCreateIntent> {
        self.creating.retain(|candidate| candidate != key);
        let intent = self.creation_intents.remove(key);
        self.invalidate();
        intent
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InspectionSweepScope {
    context: Option<ContextId>,
    repo: RepoScope,
}

#[derive(Clone)]
enum HubBridge {
    Live(Bridge),
    #[cfg(test)]
    Test(Rc<dyn Fn(RequestBody) -> async_channel::Receiver<Result<ResponseBody, ProtoError>>>),
}

impl HubBridge {
    fn request(
        &self,
        body: RequestBody,
    ) -> async_channel::Receiver<Result<ResponseBody, ProtoError>> {
        match self {
            Self::Live(bridge) => bridge.request(body),
            #[cfg(test)]
            Self::Test(request) => request(body),
        }
    }

    fn send(&self, body: RequestBody) {
        match self {
            Self::Live(bridge) => bridge.send(body),
            #[cfg(test)]
            Self::Test(request) => {
                let _reply = request(body);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorktreeIdentity {
    repo: RepoId,
    path: String,
    created_at: String,
}

impl From<&Worktree> for WorktreeIdentity {
    fn from(worktree: &Worktree) -> Self {
        Self {
            repo: worktree.repo_id.clone(),
            path: worktree.path.clone(),
            created_at: worktree.created_at.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PrCreateIntent {
    open: bool,
    sleep_previous: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SelectionAnchors {
    rail: Option<Option<RepoId>>,
    worktree: Option<WorktreeId>,
    prs_mine: Option<PrIdentity>,
    prs_review: Option<PrIdentity>,
}

/// The Hub screen.
pub struct HubScreen {
    hub: Entity<HubState>,
    rail_scroll: UniformListScrollHandle,
    list_scroll: UniformListScrollHandle,
    pr_scroll: UniformListScrollHandle,
    detail_scroll: ScrollHandle,
    observation: Option<Subscription>,
    /// One live filter editor for the lifetime of the Hub, shared by the rail and the list:
    /// §3.10 opens the filter over whichever pane has focus, and only ever one of them.
    filter_input: Entity<TextInput>,
    /// Mirrors the editor into `FilterState.query`, which every projection and the harness
    /// dump read.
    filter_subscription: Option<Subscription>,
    /// What a click on the PR screen's tabs and rows does, built once `bind` has the context.
    pr_handlers: Option<prs_screen::PrHandlers>,
    home: Option<std::path::PathBuf>,
}

impl HubScreen {
    /// Builds the screen. Called once, while the shell is being built.
    #[must_use]
    pub fn new(cx: &mut App) -> Self {
        let filter_input = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            // §3.10 replaces the 30 px pane header in place, so the editor brings no box of
            // its own: one line of text and a caret, sized by the header.
            input.set_embedded(true, cx);
            input
        });
        Self {
            hub: cx.new(|_| HubState {
                home: crate::presentation::home_dir(),
                ..HubState::default()
            }),
            rail_scroll: UniformListScrollHandle::new(),
            list_scroll: UniformListScrollHandle::new(),
            pr_scroll: UniformListScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            observation: None,
            filter_input,
            filter_subscription: None,
            pr_handlers: None,
            home: crate::presentation::home_dir(),
        }
    }

    /// Bind once during Shell construction; render retains this lazy adapter for existing callers.
    pub fn bind(&mut self, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
        if self.observation.is_some() {
            return;
        }
        let weak_state = state.downgrade();
        let hub = self.hub.clone();
        self.filter_subscription = Some(cx.subscribe(
            &self.filter_input,
            move |input, event: &TextInputEvent, cx| {
                if !matches!(event, TextInputEvent::Changed) {
                    return;
                }
                let Some(state) = weak_state.upgrade() else {
                    return;
                };
                let query = input.read(cx).text().to_owned();
                let pane = FilteredPane::of(state.read(cx));
                let narrowed = state.update(cx, |app, cx| {
                    if app.filter.query == query {
                        return false;
                    }
                    app.filter.query = query;
                    // §3.10: a narrower list starts again at its top row.
                    pane.reset_cursor(&mut app.cursors);
                    cx.notify();
                    true
                });
                if narrowed {
                    hub.update(cx, |hub, _| pane.forget_anchor(&mut hub.selection));
                }
            },
        ));
        let ctx = self.context(state, bridge);
        self.pr_handlers = Some(ctx.pr_handlers());
        let observed = ctx.clone();
        let filter_input = self.filter_input.clone();
        self.observation = Some(cx.observe(state, move |state, cx| {
            observed.synchronize(cx);
            // The two-stage `Esc`, `Enter` and every screen change reset the query in state;
            // the editor follows it rather than the other way round.
            let query = state.read(cx).filter.query.clone();
            if filter_input.read(cx).text() != query {
                filter_input.update(cx, |input, cx| input.set_text(query, cx));
            }
        }));
        cx.defer(move |cx| ctx.synchronize(cx));
    }

    pub(crate) fn context(&self, state: &Entity<AppState>, bridge: &Bridge) -> HubCtx {
        HubCtx {
            state: state.clone(),
            hub: self.hub.clone(),
            bridge: HubBridge::Live(bridge.clone()),
            rail_scroll: self.rail_scroll.clone(),
            list_scroll: self.list_scroll.clone(),
            pr_scroll: self.pr_scroll.clone(),
        }
    }

    /// Focus the Hub filter after `/` opens it.
    pub(crate) fn focus_filter(&self, window: &mut Window, cx: &mut App) {
        self.filter_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    /// The Hub filter's live input handle, for the shell's focus reconciliation.
    pub(crate) fn filter_focus_handle(&self, cx: &App) -> FocusHandle {
        self.filter_input.read(cx).focus_handle()
    }
}

/// Which list §3.10's filter is narrowing.
#[derive(Clone, Copy)]
enum FilteredPane {
    Rail,
    Worktrees,
    PrsMine,
    PrsReview,
    /// The board owns its own filter, and the Workspace has no Hub list to narrow.
    None,
}

impl FilteredPane {
    fn of(app: &AppState) -> Self {
        match (app.hub_pane, &app.screen) {
            (HubPane::Repos, _) => Self::Rail,
            (
                HubPane::List,
                Screen::Hub {
                    tab: HubTab::Worktrees,
                },
            ) => Self::Worktrees,
            (HubPane::List, Screen::Hub { tab: HubTab::Prs }) => match app.pr_tab {
                PrTab::Mine => Self::PrsMine,
                PrTab::Review => Self::PrsReview,
            },
            (HubPane::List, Screen::Hub { tab: HubTab::Board } | Screen::Workspace { .. }) => {
                Self::None
            }
        }
    }

    /// Puts the cursor back on the top row, because §3.10 says the top match is what `Enter`
    /// opens once the query has narrowed the list.
    fn reset_cursor(self, cursors: &mut crate::state::Cursors) {
        match self {
            Self::Rail => cursors.repos = 0,
            Self::Worktrees => cursors.worktrees = 0,
            Self::PrsMine => cursors.prs_mine = 0,
            Self::PrsReview => cursors.prs_review = 0,
            Self::None => {}
        }
    }

    /// Drops the anchored row, so the reset above survives the next projection: the pane
    /// anchors the row it is showing so a refresh keeps the selection under the user, and a
    /// narrowed list is not a refresh.
    fn forget_anchor(self, selection: &mut SelectionAnchors) {
        match self {
            Self::Rail => selection.rail = None,
            Self::Worktrees => selection.worktree = None,
            Self::PrsMine => selection.prs_mine = None,
            Self::PrsReview => selection.prs_review = None,
            Self::None => {}
        }
    }
}

/// The cursor of the active PR tab; each tab remembers its own row (§1.5).
fn pr_cursor(state: &AppState) -> usize {
    match state.pr_tab {
        PrTab::Mine => state.cursors.prs_mine,
        PrTab::Review => state.cursors.prs_review,
    }
}

/// The pull request behind a row, looked up in the cache that produced it.
fn find_pr<'a>(
    hub: &'a HubState,
    tab: PrTab,
    repo: &RepoId,
    number: u64,
) -> Option<&'a PullRequest> {
    hub.prs
        .slice(tab)?
        .prs
        .iter()
        .find(|pr| &pr.repo_id == repo && pr.number == number)
}

/// Everything an `on_action` listener needs, cloned into each closure.
#[derive(Clone)]
pub(crate) struct HubCtx {
    state: Entity<AppState>,
    hub: Entity<HubState>,
    bridge: HubBridge,
    rail_scroll: UniformListScrollHandle,
    list_scroll: UniformListScrollHandle,
    pr_scroll: UniformListScrollHandle,
}

/// How many rows the list pane can show at this window height.
///
/// `ctrl-d` / `ctrl-u` and the header's `first–last/total` range both depend on it, so it is
/// derived from the live viewport and the theme's metrics rather than assumed.
fn visible_rows(window_height: f32, cx: &App) -> usize {
    row_capacity(window_height, cx.theme().metrics)
}

fn row_capacity(window_height: f32, metrics: fleet_ui_kit::theme::Metrics) -> usize {
    let chrome = f32::from(metrics.title_bar_h)
        + f32::from(metrics.status_bar_h)
        + 2.0 * f32::from(metrics.pane_header_h);
    (((window_height - chrome).max(0.0) / f32::from(metrics.row_h).max(1.0)) as usize).max(2)
}

/// The context's own board, as the title bar's Board segment counts it (§2.2).
pub(crate) fn context_board_summary<'a>(
    boards: &'a [BoardSummary],
    context: Option<&ContextId>,
) -> Option<&'a BoardSummary> {
    boards
        .iter()
        // The context's task board, never its Reviews board: both are unscoped by worktree.
        .find(|board| {
            board.worktree_id.is_none()
                && board.kind.is_tasks()
                && Some(&board.context_id) == context
        })
}
