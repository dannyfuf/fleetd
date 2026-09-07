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
    model::{CloneJob, Repo, Worktree},
    sessions::{AgentActivity, SessionState},
};
use fleet_proto::{
    error::ProtoError,
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
    presentation::{age_secs, now_unix},
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
mod tests;

pub use cache::PrCache;
use composition::publish_breadcrumb;
pub use projection::HubModel;

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
    inspect_target: Option<WorktreeId>,
    presentation_revision: u64,
    projection: RefCell<projection::ProjectionCache>,
    prepared: Rc<HubModel>,
    /// Pull requests whose worktree is being created, so their glyph spins in place (§3.5).
    pub creating: Vec<(RepoId, u64)>,
}

impl HubState {
    /// Marks the client-owned caches as changed, so the next projection is rebuilt.
    fn invalidate(&mut self) {
        self.presentation_revision = self.presentation_revision.wrapping_add(1);
    }
}

/// The Hub screen.
pub struct HubScreen {
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

    fn context(&self, state: &Entity<AppState>, bridge: &Bridge) -> HubCtx {
        HubCtx {
            state: state.clone(),
            hub: self.hub.clone(),
            bridge: bridge.clone(),
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
struct HubCtx {
    state: Entity<AppState>,
    hub: Entity<HubState>,
    bridge: Bridge,
    rail_scroll: UniformListScrollHandle,
    list_scroll: UniformListScrollHandle,
    pr_scroll: UniformListScrollHandle,
}

/// How many rows the list pane can show at this window height.
///
/// `ctrl-d` / `ctrl-u` and the header's `first–last/total` range both depend on it, so it is
/// derived from the live viewport and the theme's metrics rather than assumed.
fn visible_rows(window_height: f32, cx: &App) -> usize {
    let metrics = cx.theme().metrics;
    let chrome = f32::from(metrics.context_bar_h)
        + f32::from(metrics.status_bar_h)
        + f32::from(metrics.pane_header_h);
    (((window_height - chrome).max(0.0) / f32::from(metrics.row_h).max(1.0)) as usize).max(2)
}
