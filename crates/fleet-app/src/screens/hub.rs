//! Hub coordination, with prepared rows and selection-owned background work.

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

use fleet_core::{
    github::{PrTab, PullRequest, worktree_matches_pr},
    ids::{ContextId, RepoId, WorktreeId},
    model::{CloneJob, Context, Repo, Worktree},
    sessions::{AgentActivity, SessionState},
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    request::RequestBody,
    response::{PrSlice, ResponseBody},
};
use fleet_ui_kit::{ActiveTheme, Icon, SplitLayout, StatusKind, Toast, ToastDuration};
use gpui::{
    AnyElement, App, ClipboardItem, Entity, FocusHandle, IntoElement, ScrollHandle, SharedString,
    Subscription, Task, UniformListScrollHandle, Window, div, prelude::*,
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
mod projection;
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
/// How long a pull-request slice stays fresh, mirroring `github.prTtlSeconds`.
pub const PR_TTL: Duration = Duration::from_secs(90);
/// Below this window width the detail panel docks instead of insetting (§3.4).
const DETAIL_INSET_MIN_WIDTH: f32 = 1120.0;

/// The window geometry one frame is drawn against.
#[derive(Debug, Clone, Copy)]
struct Viewport {
    /// Logical window width in pixels.
    width: f32,
    /// How many rows the list pane can show.
    rows: usize,
}

/// Whether the detail panel docks over the list instead of being inset (§3.4).
#[must_use]
pub fn detail_is_docked(window_width: f32) -> bool {
    window_width < DETAIL_INSET_MIN_WIDTH
}

/// The list pane's width in `ch`, which is what resolves every §2.9 ladder.
///
/// The rail never moves and the docked panel never steals width, so the only two variables are
/// `H` (the 44 px icon rail) and an **inset** detail panel.
#[must_use]
fn list_pane_ch(
    window_width: f32,
    rail_collapsed: bool,
    detail_open: bool,
    metrics: &fleet_ui_kit::theme::Metrics,
) -> f32 {
    let rail = if rail_collapsed {
        repos_rail::COLLAPSED_WIDTH
    } else {
        f32::from(metrics.rail_w)
    };
    let detail = if detail_open && !detail_is_docked(window_width) {
        f32::from(metrics.detail_w)
    } else {
        0.0
    };
    ((window_width - rail - detail) / fleet_ui_kit::theme::CH).max(1.0)
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
    pr_refresh_task: Option<Task<()>>,
    /// Bumped on every reschedule; an older PR refresh timer wakes up and does nothing.
    pr_refresh_generation: u64,
    inspect_target: Option<WorktreeId>,
    inspection_sequence: u64,
    inspection_requests: HashMap<WorktreeId, u64>,
    inspection_identities: HashMap<WorktreeId, WorktreeIdentity>,
    last_reconciled_revision: u64,
    #[cfg(test)]
    inspection_reconciliations: usize,
    presentation_revision: u64,
    projection: RefCell<projection::ProjectionCache>,
    prepared: Rc<HubModel>,
    selection: SelectionAnchors,
    /// Pull requests whose worktree is being created, so their glyph spins in place (§3.5).
    pub creating: Vec<(RepoId, u64)>,
    creation_intents: HashMap<PrIdentity, PrCreateIntent>,
    restoring_trash: Option<String>,
}

impl HubState {
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

    fn apply_inspection(
        &mut self,
        id: &WorktreeId,
        request: u64,
        result: Result<ResponseBody, ProtoError>,
    ) -> bool {
        if self.inspection_requests.get(id) != Some(&request) {
            return false;
        }
        self.inspection_requests.remove(id);
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
    board: super::board::BoardScreen,
    hub: Entity<HubState>,
    rail_scroll: UniformListScrollHandle,
    list_scroll: UniformListScrollHandle,
    pr_scroll: UniformListScrollHandle,
    detail_scroll: ScrollHandle,
    observation: Option<Subscription>,
    home: Option<std::path::PathBuf>,
}

impl HubScreen {
    /// Builds the screen. Called once, while the shell is being built.
    #[must_use]
    pub fn new(cx: &mut App) -> Self {
        Self {
            board: super::board::BoardScreen::new(cx),
            hub: cx.new(|_| HubState::default()),
            rail_scroll: UniformListScrollHandle::new(),
            list_scroll: UniformListScrollHandle::new(),
            pr_scroll: UniformListScrollHandle::new(),
            detail_scroll: ScrollHandle::new(),
            observation: None,
            home: crate::presentation::home_dir(),
        }
    }

    /// Bind once during Shell construction; render retains this lazy adapter for existing callers.
    pub fn bind(&mut self, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
        if self.observation.is_some() {
            return;
        }
        let ctx = self.context(state, bridge);
        let observed = ctx.clone();
        self.observation = Some(cx.observe(state, move |_, cx| observed.synchronize(cx)));
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
    let chrome = f32::from(metrics.context_bar_h)
        + f32::from(metrics.status_bar_h)
        + 2.0 * f32::from(metrics.pane_header_h);
    (((window_height - chrome).max(0.0) / f32::from(metrics.row_h).max(1.0)) as usize).max(2)
}

/// The Hub's screen tabs; summary counts are context scoped, independent of repo scope.
fn hub_tabs(state: &AppState) -> fleet_ui_kit::SegmentedTabs {
    use fleet_ui_kit::{SegmentedTab, SegmentedTabs};
    let summary = state.snapshot.as_ref().and_then(|snapshot| {
        snapshot
            .boards
            .iter()
            .find(|board| Some(&board.context_id) == state.active_context())
    });
    let label = if summary.is_some_and(|board| board.conflict_count > 0) {
        "Board •"
    } else {
        "Board"
    };
    let board = summary
        .map_or_else(
            || SegmentedTab::bare(label),
            |summary| SegmentedTab::new(label, summary.open_count),
        )
        .loading(state.board.loading);
    let active = match state.screen {
        Screen::Hub { tab: HubTab::Prs } => 1,
        Screen::Hub { tab: HubTab::Board } => 2,
        _ => 0,
    };
    SegmentedTabs::new([
        SegmentedTab::bare("Worktrees"),
        SegmentedTab::bare("Pull requests"),
        board,
    ])
    .active(active)
    .underlined(false)
    .on_select(|index, window, cx| {
        let action: Box<dyn gpui::Action> = match index {
            1 => Box::new(hub::GoPrs),
            2 => Box::new(crate::actions::board::GoBoard),
            _ => Box::new(hub::GoWorktrees),
        };
        window.dispatch_action(action, cx);
    })
}
