//! The Hub: contexts, repos rail, worktrees or pull requests, detail panel (UX-SPEC §3.1–§3.5).
//!
//! This module is the Hub's **model and controller**. The rendering lives in
//! [`crate::views`]: [`repos_rail`](crate::views::repos_rail),
//! [`worktrees_list`](crate::views::worktrees_list),
//! [`detail_panel`](crate::views::detail_panel) and
//! [`prs_screen`](crate::views::prs_screen). What stays here is everything that decides *what
//! the next keystroke does*: the layout ladder, the inspection and pull-request caches, and one
//! handler per Hub action of `docs/KEYMAP.md`.
//!
//! # Shape
//!
//! [`HubScreen`] is not a gpui view — the shell owns the window's only [`gpui::Render`] — so it
//! cannot use `cx.listener`. Its mutable state therefore lives in a [`HubState`] entity, and
//! every `on_action` closure carries a cheap [`HubCtx`] clone (the two entities, the bridge and
//! the scroll handles). That is what lets a listener mutate the caches from `&mut App` alone.
//!
//! # Freshness (§2.6 [D-4])
//!
//! The selected worktree is re-inspected **without a fetch**, debounced
//! [`AUTO_INSPECT_DEBOUNCE`] after the cursor settles; `I` runs a full inspect **with** fetch as
//! an explicit job. Nothing safety-adjacent renders without its stamp.

use std::{
    collections::HashMap,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use fleet_core::{
    config::Agent,
    github::{PrTab, PullRequest, worktree_matches_pr},
    ids::{ContextId, RepoId, WorktreeId},
    inspection::WorktreeInspection,
    model::{CloneJob, Repo, Worktree},
    sessions::SessionState,
};
use fleet_proto::{
    error::ProtoError,
    request::RequestBody,
    response::{PrSlice, PruneResult, ResponseBody},
    snapshot::Snapshot,
};
use fleet_ui_kit::{
    ActiveTheme, ConfirmKey, Fact, FactList, Icon, SplitLayout, StatusKind, Toast, ToastDuration,
};
use gpui::{
    AnyElement, App, ClipboardItem, Entity, FocusHandle, IntoElement, SharedString,
    UniformListScrollHandle, Window, div, prelude::*,
};

use crate::{
    actions::{fleet, hub, prs, repos, worktrees},
    bridge::Bridge,
    dialogs::Dialogs,
    state::{AppState, HubPane, HubTab, Overlay, RepoScope, Screen, dwell_for, move_cursor},
    views::{
        detail_panel::{self, PrProps, RepoProps, WorktreeProps},
        hub_context_bar,
        prs_screen::{self, PrInputs, PrProps as PrScreenProps, PrRow},
        repos_rail::{self, RailKind, RailProps, RailRow},
        worktrees_list::{self, ListProps, RowInputs, WorktreeRow},
    },
};

/// How long the cursor must sit still before the selected worktree is re-inspected (§2.6 D-4).
pub const AUTO_INSPECT_DEBOUNCE: Duration = Duration::from_millis(400);
/// How long a pull-request slice stays fresh, mirroring `github.prTtlSeconds`.
pub const PR_TTL: Duration = Duration::from_secs(90);
/// Below this window width the detail panel docks instead of insetting (§3.4).
pub const DETAIL_INSET_MIN_WIDTH: f32 = 1120.0;
/// The rail's expanded width (§3.2).
pub const RAIL_WIDTH: f32 = 240.0;
/// The detail panel's inset width (§3.4).
pub const DETAIL_WIDTH: f32 = 340.0;

/// The window geometry one frame is drawn against.
#[derive(Debug, Clone, Copy)]
struct Viewport {
    /// Logical window width in pixels.
    width: f32,
    /// How many rows the list pane can show.
    rows: usize,
}

// ---------------------------------------------------------------------------- pure helpers

/// The current time in whole seconds since the Unix epoch.
///
/// Every age on the Hub is derived from an ISO-8601 stamp the daemon wrote, so the client needs
/// one wall-clock reading per frame and nothing more.
#[must_use]
pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// Parses the ISO-8601 UTC stamps the daemon writes into epoch seconds.
///
/// Accepts `YYYY-MM-DDTHH:MM:SS`, an optional fractional part, and either `Z` or a `±HH:MM`
/// offset. `fleet-app` has no date-time dependency on purpose: this is the only arithmetic the
/// client does on a timestamp, and a wrong answer here would mis-age a safety fact, so it is
/// unit-tested rather than trusted.
#[must_use]
pub fn epoch_secs(iso: &str) -> Option<i64> {
    let bytes = iso.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if bytes[10] != b'T' && bytes[10] != b' ' {
        return None;
    }
    let number = |from: usize, to: usize| iso.get(from..to)?.parse::<i64>().ok();
    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;

    let tail = iso.get(19..).unwrap_or_default();
    let tail = tail.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(offset) = tail.strip_prefix('+').or_else(|| tail.strip_prefix('-')) {
        let sign = if tail.starts_with('-') { -1 } else { 1 };
        let hours = offset.get(0..2)?.parse::<i64>().ok()?;
        let minutes = offset
            .get(3..5)
            .or_else(|| offset.get(2..4))
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        seconds -= sign * (hours * 3_600 + minutes * 60);
    }
    Some(seconds)
}

/// Days from 1970-01-01 to `y-m-d`, by Howard Hinnant's civil-calendar algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The age of an ISO-8601 stamp in seconds, never negative (a clock skew is not a future fact).
#[must_use]
pub fn age_secs(iso: &str, now: i64) -> Option<i64> {
    epoch_secs(iso).map(|then| (now - then).max(0))
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
pub fn list_pane_ch(window_width: f32, rail_collapsed: bool, detail_open: bool) -> f32 {
    let rail = if rail_collapsed {
        repos_rail::COLLAPSED_WIDTH
    } else {
        RAIL_WIDTH
    };
    let detail = if detail_open && !detail_is_docked(window_width) {
        DETAIL_WIDTH
    } else {
        0.0
    };
    ((window_width - rail - detail) / fleet_ui_kit::theme::CH).max(1.0)
}

// ---------------------------------------------------------------------------- caches

/// One entry of the client's inspection cache.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inspected {
    /// The daemon's facts, absent until the first answer lands.
    pub data: Option<WorktreeInspection>,
    /// A transport or job failure, shown verbatim (§3.4).
    pub error: Option<String>,
    /// Whether a refresh is in flight; previous values dim but never blank.
    pub loading: bool,
}

impl Inspected {
    /// A slot with fresh facts.
    #[must_use]
    pub fn ready(data: WorktreeInspection) -> Self {
        Self {
            data: Some(data),
            error: None,
            loading: false,
        }
    }

    /// A slot whose inspection failed.
    #[must_use]
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            data: None,
            error: Some(error.into()),
            loading: false,
        }
    }

    /// A slot waiting for its first answer.
    #[must_use]
    pub fn pending() -> Self {
        Self {
            data: None,
            error: None,
            loading: true,
        }
    }
}

/// The two pull-request tabs, with the fetch state §3.5 renders.
#[derive(Debug, Clone, Default)]
pub struct PrCache {
    /// The `Mine` slice.
    pub mine: Option<PrSlice>,
    /// The `Review` slice.
    pub review: Option<PrSlice>,
    /// Whether a fetch covering both tabs is running.
    pub loading: bool,
    /// The most recent fetch error; stale rows stay listed beneath it.
    pub error: Option<String>,
    /// When the last fetch completed, for the TTL.
    pub fetched_at: Option<Instant>,
    /// When the running fetch started, so a lost answer cannot wedge the screen.
    pub started_at: Option<Instant>,
}

impl PrCache {
    /// The slice of one tab.
    #[must_use]
    pub fn slice(&self, tab: PrTab) -> Option<&PrSlice> {
        match tab {
            PrTab::Mine => self.mine.as_ref(),
            PrTab::Review => self.review.as_ref(),
        }
    }

    /// Whether the screen should ask the daemon again (§3.5, `github.prTtlSeconds`).
    ///
    /// A fetch whose answer never arrived (a bridge that went away mid-flight) stops counting
    /// as running after one TTL, so the screen recovers instead of showing `⟳ refreshing`
    /// forever.
    #[must_use]
    pub fn needs_fetch(&self, now: Instant) -> bool {
        if self.loading
            && self
                .started_at
                .is_some_and(|at| now.saturating_duration_since(at) < PR_TTL)
        {
            return false;
        }
        match self.fetched_at {
            None => true,
            Some(at) => now.saturating_duration_since(at) >= PR_TTL,
        }
    }

    /// Whether nothing has ever been fetched, which is what shows skeleton rows.
    #[must_use]
    pub fn is_cold(&self) -> bool {
        self.mine.is_none() && self.review.is_none()
    }
}

/// What a destructive key is about to do once its confirm is accepted (§3.8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingAction {
    /// `d` in the worktrees list.
    DeleteWorktree(WorktreeId),
    /// `d` in the repos rail; always the `Y` key (§A12).
    DeleteRepo(RepoId),
    /// `x` in the worktrees list, after the dry run listed what it would delete.
    Prune {
        /// The repository scope of the prune, `None` for every repo in the context.
        repo: Option<RepoId>,
        /// What the dry run said it would delete.
        deleted: Vec<WorktreeId>,
    },
    /// `K` in the worktrees list.
    KillSession(WorktreeId),
    /// `D` on a context; always the `Y` key.
    DeleteContext(ContextId),
}

/// A confirm the Hub has prepared but not yet shown.
///
/// **Integration request (see the final report).** `Dialogs::Confirm` is a unit variant and the
/// dialog element is a sibling of this screen in the shell's tree, so the Hub can prepare the
/// facts but cannot render them or observe `confirm::Accept`. The request is stored here so
/// that wiring it up is a one-line change once `AppState` carries the pending confirm.
#[derive(Debug, Clone)]
pub struct ConfirmRequest {
    /// The sentence at the top of the dialog.
    pub title: SharedString,
    /// The facts, stated and stamped (§1.7).
    pub facts: FactList,
    /// `y` or the escalated `Y` (§A12).
    pub key: ConfirmKey,
    /// What happens on accept.
    pub action: PendingAction,
}

/// The Hub's own mutable state, held as an entity so `on_action` listeners can write it.
#[derive(Debug, Default)]
pub struct HubState {
    /// The client's inspection cache, keyed by worktree.
    pub inspections: HashMap<WorktreeId, Inspected>,
    /// The pull-request tabs.
    pub prs: PrCache,
    /// Bumped on every cursor move; an older debounce wakes up and does nothing.
    pub inspect_generation: u64,
    /// Pull requests whose worktree is being created, so their glyph spins in place (§3.5).
    pub creating: Vec<(RepoId, u64)>,
    /// The confirm the last destructive key prepared.
    pub pending_confirm: Option<ConfirmRequest>,
    /// The trash entry `u` would restore (§A6).
    pub last_trash_entry: Option<String>,
}

// ---------------------------------------------------------------------------- the model

/// Everything both the renderer and the key handlers need, derived once per use.
pub struct HubModel {
    /// The rail rows, filtered when the rail owns the filter.
    pub rail: Vec<RailRow>,
    /// The worktree rows, sorted and filtered.
    pub worktrees: Vec<WorktreeRow>,
    /// The pull-request rows of the active tab, filtered.
    pub prs: Vec<PrRow>,
    /// How many worktrees the scope holds before the filter.
    pub worktree_total: usize,
    /// How many pull requests the §3.5 cap hid.
    pub pr_hidden: usize,
}

/// Builds the whole Hub model from the snapshot and the caches.
#[must_use]
pub fn model(state: &AppState, hub: &HubState, now: i64) -> HubModel {
    let Some(snapshot) = state.snapshot.as_ref() else {
        return HubModel {
            rail: Vec::new(),
            worktrees: Vec::new(),
            prs: Vec::new(),
            worktree_total: 0,
            pr_hidden: 0,
        };
    };
    let context = snapshot.active_context.as_ref();
    let glyphs = |repo: &RepoId| repo_glyphs(snapshot, repo);
    let mut rail = repos_rail::rail_rows(
        context,
        &snapshot.repos,
        &snapshot.clones,
        &snapshot.worktrees,
        &glyphs,
        &snapshot.jobs,
    );

    let query = state.filter.query.clone();
    if state.hub_pane == HubPane::Repos && !query.is_empty() {
        rail = repos_rail::filter_rows(&rail, &query);
    }
    let list_query = if state.hub_pane == HubPane::List {
        query.as_str()
    } else {
        ""
    };

    let mut scoped: Vec<&Worktree> = snapshot
        .worktrees
        .iter()
        .filter(|worktree| in_context(snapshot, context, &worktree.repo_id))
        .filter(|worktree| match &state.scope {
            RepoScope::All => true,
            RepoScope::Repo(repo) => &worktree.repo_id == repo,
        })
        .collect();
    worktrees_list::sort_rows(&mut scoped);
    let worktree_total = scoped.len();
    let rows = worktrees_list::build_rows(&RowInputs {
        worktrees: scoped,
        statuses: &snapshot.statuses,
        sessions: &snapshot.sessions,
        hosts: &snapshot.hosts,
        jobs: &snapshot.jobs,
        inspections: &hub.inspections,
        now,
    });
    let worktrees: Vec<WorktreeRow> = rows
        .into_iter()
        .filter(|row| worktrees_list::matches(row, list_query))
        .collect();

    let slice = hub.prs.slice(state.pr_tab);
    let prs: Vec<PrRow> = prs_screen::build_rows(&PrInputs {
        slice,
        worktrees: &snapshot.worktrees,
        statuses: &snapshot.statuses,
        sessions: &snapshot.sessions,
        creating: &hub.creating,
        now,
    })
    .into_iter()
    .filter(|row| prs_screen::matches(row, list_query))
    .collect();

    HubModel {
        rail,
        worktrees,
        prs,
        worktree_total,
        pr_hidden: prs_screen::hidden_rows(slice),
    }
}

/// Whether a repository belongs to the active context.
fn in_context(snapshot: &Snapshot, context: Option<&ContextId>, repo: &RepoId) -> bool {
    let Some(context) = context else {
        return true;
    };
    snapshot
        .repos
        .iter()
        .any(|candidate| &candidate.id == repo && &candidate.context_id == context)
}

/// The §2.5 glyphs of one repository's worktrees, for the rail's worst-of aggregate.
fn repo_glyphs(snapshot: &Snapshot, repo: &RepoId) -> Vec<StatusKind> {
    snapshot
        .worktrees
        .iter()
        .filter(|worktree| &worktree.repo_id == repo)
        .map(|worktree| {
            // §3.3 cold load: a worktree the daemon has not reported a status for yet is
            // `unknown`, never `none` — absence of knowledge is never good news (§1.3).
            let session = snapshot
                .statuses
                .iter()
                .find(|status| status.worktree_id == worktree.id)
                .map_or(SessionState::Unknown, |status| status.session);
            let unreachable = worktree.host.as_ref().is_some_and(|host| {
                snapshot
                    .hosts
                    .iter()
                    .any(|status| &status.id == host && !status.reachable)
            });
            let slept = snapshot.sessions.iter().any(|candidate| {
                matches!(
                    &candidate.kind,
                    fleet_core::sessions::SessionKind::Worktree(id) if id == &worktree.id
                ) && candidate.slept_at.is_some()
            });
            worktrees_list::row_glyph(session, slept, false, unreachable, false)
        })
        .collect()
}

/// The facts a delete confirm states, in the shape §3.8.3 quotes them.
///
/// When the worktree was never inspected every decisive fact is unknown, which is exactly what
/// escalates the confirm key from `y` to `Y` (§1.7, §A12) — [`FactList::confirm_key`] derives
/// that rather than the caller.
#[must_use]
pub fn delete_facts(inspected: Option<&Inspected>) -> FactList {
    let Some(data) = inspected.and_then(|slot| slot.data.as_ref()) else {
        return FactList::from_facts([
            Fact::unknown("never inspected — safety facts unavailable"),
            Fact::unknown("unique commits unknown"),
            Fact::unknown("merge state unknown"),
        ]);
    };
    let mut facts = vec![
        if data.dirty {
            Fact::risk(match data.dirty_files {
                Some(count) => format!("{count} uncommitted files"),
                None => "uncommitted changes".to_owned(),
            })
        } else {
            Fact::safe("clean")
        },
        match data.unique_commits {
            Some(0) => Fact::safe("no unique commits"),
            Some(count) => Fact::risk(format!("{count} unique commits")),
            None => Fact::unknown("unique commit count unavailable"),
        },
        if data.merged {
            Fact::safe(format!("merged into {}", data.target_branch))
        } else {
            Fact::risk(format!("not merged into {}", data.target_branch))
        },
        if data.published {
            Fact::safe("published")
        } else {
            Fact::risk("never published")
        },
        match data.session {
            SessionState::Attached => Fact::risk("session attached"),
            SessionState::Detached => Fact::risk("session running, detached"),
            SessionState::Unknown => Fact::unknown("session state unknown"),
            SessionState::None => Fact::safe("no session"),
        },
    ];
    if let Some(pr) = data.pr.as_ref() {
        facts.push(Fact::safe(format!("PR #{}", pr.number)));
    }
    facts.extend(data.warnings.iter().map(Fact::unknown));
    FactList::from_facts(facts)
}

/// The facts a prune confirm states: what it would delete, and what it refused to.
#[must_use]
pub fn prune_facts(result: &PruneResult) -> FactList {
    let mut facts: Vec<Fact> = result
        .deleted
        .iter()
        .map(|id| Fact::safe(format!("delete {}", id.slug())))
        .collect();
    facts.extend(result.skipped.iter().map(|skipped| {
        Fact::risk(format!(
            "keep {} — {}",
            skipped.worktree_id.slug(),
            skipped.reason
        ))
    }));
    FactList::from_facts(facts)
}

// ---------------------------------------------------------------------------- the screen

/// The Hub screen.
pub struct HubScreen {
    hub: Entity<HubState>,
    rail_scroll: UniformListScrollHandle,
    list_scroll: UniformListScrollHandle,
    pr_scroll: UniformListScrollHandle,
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
        }
    }

    /// The Hub's caches, exposed so a test or a future dialog wiring can read them.
    #[must_use]
    pub fn state(&self) -> &Entity<HubState> {
        &self.hub
    }

    /// Renders the Hub into the frame's body.
    pub fn render(
        &mut self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let ctx = HubCtx {
            state: state.clone(),
            hub: self.hub.clone(),
            bridge: bridge.clone(),
            rail_scroll: self.rail_scroll.clone(),
            list_scroll: self.list_scroll.clone(),
            pr_scroll: self.pr_scroll.clone(),
        };
        ctx.tick_pull_requests(cx);
        ctx.tick_inspection(cx);

        let now = now_epoch();
        let viewport = window.viewport_size();
        let width = f32::from(viewport.width);
        let rows = visible_rows(f32::from(viewport.height), cx);
        let model = model(state.read(cx), self.hub.read(cx), now);
        publish_breadcrumb(state, &model, cx);
        let body = self.body(
            state.read(cx),
            self.hub.read(cx),
            &model,
            Viewport { width, rows },
            now,
            cx,
        );

        div()
            .track_focus(focus)
            .size_full()
            .child(body)
            // ---- §3.1 contexts and global Hub keys
            .on_action(ctx.act(|ctx, _: &hub::MoveDown, window, cx| ctx.move_by(1, window, cx)))
            .on_action(ctx.act(|ctx, _: &hub::MoveUp, window, cx| ctx.move_by(-1, window, cx)))
            .on_action(
                ctx.act(|ctx, _: &hub::GoTop, window, cx| ctx.move_to_end(false, window, cx)),
            )
            .on_action(
                ctx.act(|ctx, _: &hub::GoBottom, window, cx| ctx.move_to_end(true, window, cx)),
            )
            .on_action(
                ctx.act(|ctx, _: &hub::HalfPageDown, window, cx| ctx.half_page(1, window, cx)),
            )
            .on_action(
                ctx.act(|ctx, _: &hub::HalfPageUp, window, cx| ctx.half_page(-1, window, cx)),
            )
            .on_action(ctx.act(|ctx, _: &hub::GoAllRepos, _w, cx| ctx.go_all_repos(cx)))
            .on_action(ctx.act(|ctx, _: &hub::NextContext, _w, cx| ctx.cycle_context(1, cx)))
            .on_action(ctx.act(|ctx, _: &hub::PrevContext, _w, cx| ctx.cycle_context(-1, cx)))
            .on_action(ctx.act(|ctx, _: &hub::SelectContext1, _w, cx| ctx.select_context(1, cx)))
            .on_action(ctx.act(|ctx, _: &hub::SelectContext2, _w, cx| ctx.select_context(2, cx)))
            .on_action(ctx.act(|ctx, _: &hub::SelectContext3, _w, cx| ctx.select_context(3, cx)))
            .on_action(ctx.act(|ctx, _: &hub::SelectContext4, _w, cx| ctx.select_context(4, cx)))
            .on_action(ctx.act(|ctx, _: &hub::SelectContext5, _w, cx| ctx.select_context(5, cx)))
            .on_action(ctx.act(|ctx, _: &hub::SelectContext6, _w, cx| ctx.select_context(6, cx)))
            .on_action(ctx.act(|ctx, _: &hub::SelectContext7, _w, cx| ctx.select_context(7, cx)))
            .on_action(ctx.act(|ctx, _: &hub::SelectContext8, _w, cx| ctx.select_context(8, cx)))
            .on_action(ctx.act(|ctx, _: &hub::SelectContext9, _w, cx| ctx.select_context(9, cx)))
            .on_action(ctx.act(|ctx, _: &hub::NewContext, _w, cx| {
                ctx.open_dialog(Dialogs::NewContext, cx);
            }))
            .on_action(ctx.act(|ctx, _: &hub::EditContext, _w, cx| {
                ctx.open_dialog(Dialogs::EditContext, cx);
            }))
            .on_action(ctx.act(|ctx, _: &hub::DeleteContext, _w, cx| ctx.delete_context(cx)))
            .on_action(ctx.act(|ctx, _: &hub::OpenInBrowser, _w, cx| ctx.open_in_browser(cx)))
            .on_action(ctx.act(|ctx, _: &fleet::Refresh, _w, cx| ctx.refresh(cx)))
            .on_action(ctx.act(|ctx, _: &fleet::UpdateFleet, _w, _cx| {
                ctx.bridge.send(RequestBody::Update);
            }))
            .on_action(ctx.act(|ctx, _: &fleet::OpenAgentClaude, _w, cx| {
                ctx.open_agent(Agent::Claude, cx);
            }))
            .on_action(ctx.act(|ctx, _: &fleet::OpenAgentOpencode, _w, cx| {
                ctx.open_agent(Agent::Opencode, cx);
            }))
            // ---- §3.2 repos rail
            .on_action(ctx.act(|ctx, _: &repos::Open, _w, cx| ctx.open_repo(cx)))
            .on_action(ctx.act(|ctx, _: &repos::Clone, _w, cx| {
                ctx.open_dialog(Dialogs::CloneRepo, cx);
            }))
            .on_action(ctx.act(|ctx, _: &repos::Delete, _w, cx| ctx.delete_repo(cx)))
            .on_action(ctx.act(|ctx, _: &repos::DismissClone, _w, cx| ctx.dismiss_clone(cx)))
            .on_action(ctx.act(|ctx, _: &repos::EditHooks, _w, cx| ctx.edit_hooks(cx)))
            .on_action(ctx.act(|ctx, _: &repos::MoveToContext, _w, cx| {
                ctx.open_dialog(Dialogs::AssignRepo, cx);
            }))
            // ---- §3.3 worktrees list
            .on_action(ctx.act(|ctx, _: &worktrees::Open, _w, cx| ctx.open_worktree(true, cx)))
            .on_action(ctx.act(|ctx, _: &worktrees::OpenKeepAwake, _w, cx| {
                ctx.open_worktree(false, cx);
            }))
            .on_action(ctx.act(|ctx, _: &worktrees::Create, _w, cx| {
                ctx.open_dialog(Dialogs::CreateWorktree, cx);
            }))
            .on_action(ctx.act(|ctx, _: &worktrees::Delete, _w, cx| ctx.delete_worktree(cx)))
            .on_action(ctx.act(|ctx, _: &worktrees::UndoDelete, _w, cx| ctx.undo_delete(cx)))
            .on_action(ctx.act(|ctx, _: &worktrees::Prune, _w, cx| ctx.prune(cx)))
            .on_action(ctx.act(|ctx, _: &worktrees::Sleep, _w, cx| ctx.sleep(cx)))
            .on_action(ctx.act(|ctx, _: &worktrees::Kill, _w, cx| ctx.kill(cx)))
            .on_action(ctx.act(|ctx, _: &worktrees::Inspect, _w, cx| ctx.inspect_selected(cx)))
            .on_action(ctx.act(|ctx, _: &worktrees::CopyPath, _w, cx| ctx.copy_path(cx)))
            .on_action(ctx.act(|ctx, _: &worktrees::CopyBranch, _w, cx| ctx.copy_branch(cx)))
            // ---- §3.5 pull requests
            .on_action(ctx.act(|ctx, _: &prs::NextTab, _w, cx| ctx.switch_tab(PrTab::Review, cx)))
            .on_action(ctx.act(|ctx, _: &prs::PrevTab, _w, cx| ctx.switch_tab(PrTab::Mine, cx)))
            .on_action(ctx.act(|ctx, _: &prs::Open, _w, cx| ctx.open_pr(true, true, cx)))
            .on_action(ctx.act(|ctx, _: &prs::OpenKeepAwake, _w, cx| ctx.open_pr(true, false, cx)))
            .on_action(ctx.act(|ctx, _: &prs::CreateWithoutOpening, _w, cx| {
                ctx.open_pr(false, false, cx);
            }))
            .on_action(ctx.act(|ctx, _: &prs::Inspect, _w, cx| ctx.inspect_pr(cx)))
            .on_action(ctx.act(|ctx, _: &prs::CopyUrl, _w, cx| ctx.copy_pr_url(cx)))
            .on_action(ctx.act(|ctx, _: &prs::Refresh, _w, cx| ctx.fetch_pull_requests(true, cx)))
            .on_action(ctx.act(|ctx, _: &prs::Back, _w, cx| ctx.back_to_worktrees(cx)))
            .into_any_element()
    }

    /// The rail, the list or the PR screen, and the detail panel.
    fn body(
        &self,
        state: &AppState,
        hub: &HubState,
        model: &HubModel,
        viewport: Viewport,
        now: i64,
        cx: &App,
    ) -> AnyElement {
        let Viewport {
            width,
            rows: visible_rows,
        } = viewport;
        let stale = state
            .snapshot_age(Instant::now())
            .filter(|_| !state.daemon.is_connected())
            .map(|age| SharedString::from(format!("{age}s")));
        let context_name = state
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                let active = snapshot.active_context.as_ref()?;
                snapshot
                    .contexts
                    .iter()
                    .find(|context| &context.id == active)
                    .map(|context| SharedString::from(context.name.clone()))
            })
            .unwrap_or_else(|| SharedString::new_static("this context"));
        let filter = state
            .filter
            .is_active()
            .then(|| SharedString::from(state.filter.query.clone()));

        let rail = repos_rail::render(
            RailProps {
                rows: model.rail.clone(),
                cursor: state.cursors.repos,
                focused: state.hub_pane == HubPane::Repos,
                collapsed: state.rail_collapsed,
                context_name: context_name.clone(),
                filter: filter.clone().filter(|_| state.hub_pane == HubPane::Repos),
                stale: stale.clone(),
            },
            &self.rail_scroll,
        );

        let pane_ch = list_pane_ch(width, state.rail_collapsed, state.detail_open);
        let scope_name = match &state.scope {
            RepoScope::All => SharedString::new_static("All"),
            RepoScope::Repo(repo) => SharedString::from(repo.name().to_owned()),
        };

        let list = match state.screen {
            Screen::Hub {
                tab: HubTab::Worktrees,
            } => worktrees_list::render(
                ListProps {
                    rows: model.worktrees.clone(),
                    cursor: state.cursors.worktrees,
                    focused: state.hub_pane == HubPane::List,
                    pane_ch,
                    scope: scope_name.clone(),
                    scope_is_all: state.scope == RepoScope::All,
                    total: model.worktree_total,
                    filter: filter.clone().filter(|_| state.hub_pane == HubPane::List),
                    stale,
                    visible_rows,
                    loading: state.snapshot.is_none(),
                },
                &self.list_scroll,
            ),
            _ => {
                let slice = hub.prs.slice(state.pr_tab);
                prs_screen::render(
                    PrScreenProps {
                        rows: model.prs.clone(),
                        cursor: self.pr_cursor(state),
                        focused: state.hub_pane == HubPane::List,
                        tab: state.pr_tab,
                        mine_count: hub.prs.mine.as_ref().map(|slice| slice.prs.len()),
                        review_count: hub.prs.review.as_ref().map(|slice| slice.prs.len()),
                        fetched_age: slice.and_then(|slice| age_secs(&slice.fetched_at, now)),
                        loading: hub.prs.loading,
                        cold: hub.prs.is_cold() && hub.prs.loading,
                        error: hub
                            .prs
                            .error
                            .clone()
                            .or_else(|| slice.and_then(|slice| slice.error.clone()))
                            .map(SharedString::from),
                        hidden: model.pr_hidden,
                        pane_ch,
                        multi_repo: state.scope == RepoScope::All,
                        scope: scope_name,
                        filter: filter.filter(|_| state.hub_pane == HubPane::List),
                    },
                    &self.pr_scroll,
                    cx,
                )
            }
        };

        let mut split = SplitLayout::horizontal()
            .leading(rail)
            .trailing(list)
            .divider(false);
        if state.rail_collapsed {
            split = split.leading_size(gpui::px(repos_rail::COLLAPSED_WIDTH));
        } else {
            split = split.leading_size(gpui::px(RAIL_WIDTH));
        }

        let Some(detail) = self.detail(state, hub, model, now, cx) else {
            return split.into_any_element();
        };
        if detail_is_docked(width) {
            return div()
                .relative()
                .size_full()
                .child(split)
                .child(detail_panel::panel(detail, true))
                .into_any_element();
        }
        SplitLayout::horizontal()
            .leading(split.into_any_element())
            .trailing(detail_panel::panel(detail, false))
            .trailing_size(gpui::px(DETAIL_WIDTH))
            .divider(false)
            .into_any_element()
    }

    /// The detail panel's body for whatever the cursor is on (§3.4).
    fn detail(
        &self,
        state: &AppState,
        hub: &HubState,
        model: &HubModel,
        now: i64,
        cx: &App,
    ) -> Option<AnyElement> {
        if !state.detail_open {
            return None;
        }
        let snapshot = state.snapshot.as_ref()?;
        let home = state.home.parent().map(|path| path.display().to_string())?;

        if state.hub_pane == HubPane::Repos {
            let row = model.rail.get(state.cursors.repos)?;
            let repo_id = row.repo.as_ref()?;
            if matches!(row.kind, RailKind::Cloning | RailKind::CloneFailed) {
                let clone = snapshot
                    .clones
                    .iter()
                    .find(|clone: &&CloneJob| &clone.id == repo_id)?;
                return Some(detail_panel::clone_job(clone, &home, cx));
            }
            let repo = snapshot
                .repos
                .iter()
                .find(|repo: &&Repo| &repo.id == repo_id)?;
            let worktrees = snapshot
                .worktrees
                .iter()
                .filter(|worktree| &worktree.repo_id == repo_id)
                .count();
            let live = snapshot
                .statuses
                .iter()
                .filter(|status| status.session == SessionState::Attached)
                .filter(|status| status.worktree_id.repo() == repo_id.as_str())
                .count();
            return Some(detail_panel::repo(
                RepoProps {
                    repo,
                    worktrees,
                    live,
                    pool: snapshot.pools.iter().find(|pool| &pool.repo == repo_id),
                    home: &home,
                    now,
                },
                cx,
            ));
        }

        if matches!(
            state.screen,
            Screen::Hub {
                tab: HubTab::Worktrees
            }
        ) {
            let row = model.worktrees.get(state.cursors.worktrees)?;
            let worktree = snapshot
                .worktrees
                .iter()
                .find(|worktree| worktree.id == row.id)?;
            return Some(detail_panel::worktree(
                WorktreeProps {
                    worktree,
                    status: snapshot
                        .statuses
                        .iter()
                        .find(|status| status.worktree_id == row.id),
                    slept: row.glyph == StatusKind::Sleeping,
                    host_unreachable: row.host_unreachable,
                    inspected: hub.inspections.get(&row.id),
                    home: &home,
                    now,
                },
                cx,
            ));
        }

        let row = model.prs.get(self.pr_cursor(state))?;
        let pr = find_pr(hub, state.pr_tab, &row.repo, row.number)?;
        let local = snapshot
            .worktrees
            .iter()
            .find(|worktree| worktree_matches_pr(worktree, pr));
        Some(detail_panel::pull_request(
            PrProps {
                pr,
                local,
                status: local.and_then(|worktree| {
                    snapshot
                        .statuses
                        .iter()
                        .find(|status| status.worktree_id == worktree.id)
                }),
                home: &home,
                now,
            },
            cx,
        ))
    }

    /// The cursor of the active PR tab; each tab remembers its own row (§1.5).
    fn pr_cursor(&self, state: &AppState) -> usize {
        match state.pr_tab {
            PrTab::Mine => state.cursors.prs_mine,
            PrTab::Review => state.cursors.prs_review,
        }
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

/// Publishes the focused row into the status-bar breadcrumb (`APP-CONTRACTS` §2).
///
/// This writes without `cx.notify()` on purpose: the breadcrumb is derived from the same frame
/// that is being drawn, and notifying here would schedule an endless repaint.
fn publish_breadcrumb(state: &Entity<AppState>, model: &HubModel, cx: &mut App) {
    let row = {
        let read = state.read(cx);
        match (read.hub_pane, &read.screen) {
            (HubPane::Repos, _) => model
                .rail
                .get(read.cursors.repos)
                .map(|row| row.name.to_string()),
            (
                HubPane::List,
                Screen::Hub {
                    tab: HubTab::Worktrees,
                },
            ) => model
                .worktrees
                .get(read.cursors.worktrees)
                .map(|row| row.branch.to_string()),
            (HubPane::List, _) => {
                let cursor = match read.pr_tab {
                    PrTab::Mine => read.cursors.prs_mine,
                    PrTab::Review => read.cursors.prs_review,
                };
                model.prs.get(cursor).map(|row| format!("#{}", row.number))
            }
        }
    };
    state.update(cx, |state, _| {
        if state.breadcrumb_row != row {
            state.breadcrumb_row = row;
        }
    });
}

// ---------------------------------------------------------------------------- the controller

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

impl HubCtx {
    /// Wraps a method as a gpui action listener, cloning the context into it.
    fn act<A: gpui::Action>(
        &self,
        handler: impl Fn(&Self, &A, &mut Window, &mut App) + 'static,
    ) -> impl Fn(&A, &mut Window, &mut App) + 'static {
        let ctx = self.clone();
        move |action, window, cx| handler(&ctx, action, window, cx)
    }

    /// Sends a request and applies its single answer on the foreground executor.
    fn ask(
        &self,
        body: RequestBody,
        cx: &mut App,
        apply: impl FnOnce(Result<ResponseBody, ProtoError>, &Self, &mut gpui::AsyncApp) + 'static,
    ) {
        let reply = self.bridge.request(body);
        let ctx = self.clone();
        cx.spawn(async move |cx| {
            if let Ok(result) = reply.recv().await {
                apply(result, &ctx, cx);
            }
        })
        .detach();
    }

    /// Records a toast under the §2.7 law.
    fn toast(&self, text: impl Into<SharedString>, icon: Icon, short: bool, cx: &mut App) {
        let now = Instant::now();
        let toast = Toast::new(text).icon(icon);
        let (toast, duration) = if short {
            (toast.short(), ToastDuration::Short)
        } else {
            (toast, ToastDuration::Normal)
        };
        self.state.update(cx, |state, cx| {
            state.toast(toast, now, dwell_for(duration));
            cx.notify();
        });
    }

    /// Refuses a mutation while the daemon is gone, flashing the banner instead (§3.12 C).
    fn refuses(&self, cx: &mut App) -> bool {
        let refuses = self.state.read(cx).refuses_mutations();
        if refuses {
            self.toast("fleetd is not reachable", Icon::Unplug, true, cx);
        }
        refuses
    }

    /// The current model, rebuilt from the same functions the renderer uses.
    fn model(&self, cx: &App) -> HubModel {
        model(self.state.read(cx), self.hub.read(cx), now_epoch())
    }

    // ------------------------------------------------------------------ cursor

    /// Which list the cursor keys address right now.
    fn cursor_len(&self, model: &HubModel, cx: &App) -> usize {
        let state = self.state.read(cx);
        match (state.hub_pane, &state.screen) {
            (HubPane::Repos, _) => model.rail.len(),
            (
                HubPane::List,
                Screen::Hub {
                    tab: HubTab::Worktrees,
                },
            ) => model.worktrees.len(),
            (HubPane::List, _) => model.prs.len(),
        }
    }

    fn cursor_index(&self, cx: &App) -> usize {
        let state = self.state.read(cx);
        match (state.hub_pane, &state.screen) {
            (HubPane::Repos, _) => state.cursors.repos,
            (
                HubPane::List,
                Screen::Hub {
                    tab: HubTab::Worktrees,
                },
            ) => state.cursors.worktrees,
            (HubPane::List, _) => match state.pr_tab {
                PrTab::Mine => state.cursors.prs_mine,
                PrTab::Review => state.cursors.prs_review,
            },
        }
    }

    fn set_cursor(&self, index: usize, len: usize, moving_down: bool, cx: &mut App) {
        let handle = {
            let state = self.state.read(cx);
            match (state.hub_pane, &state.screen) {
                (HubPane::Repos, _) => self.rail_scroll.clone(),
                (
                    HubPane::List,
                    Screen::Hub {
                        tab: HubTab::Worktrees,
                    },
                ) => self.list_scroll.clone(),
                (HubPane::List, _) => self.pr_scroll.clone(),
            }
        };
        self.state.update(cx, |state, cx| {
            match (state.hub_pane, &state.screen) {
                (HubPane::Repos, _) => state.cursors.repos = index,
                (
                    HubPane::List,
                    Screen::Hub {
                        tab: HubTab::Worktrees,
                    },
                ) => state.cursors.worktrees = index,
                (HubPane::List, _) => match state.pr_tab {
                    PrTab::Mine => state.cursors.prs_mine = index,
                    PrTab::Review => state.cursors.prs_review = index,
                },
            }
            cx.notify();
        });
        let mut cursor = fleet_ui_kit::ListCursor::new(len);
        cursor.set(index);
        fleet_ui_kit::ListView::reveal(&handle, &cursor, moving_down);
        self.schedule_inspection(cx);
    }

    fn move_by(&self, delta: isize, _window: &mut Window, cx: &mut App) {
        let model = self.model(cx);
        let len = self.cursor_len(&model, cx);
        let next = move_cursor(self.cursor_index(cx), delta, len);
        self.set_cursor(next, len, delta > 0, cx);
    }

    fn move_to_end(&self, bottom: bool, _window: &mut Window, cx: &mut App) {
        let model = self.model(cx);
        let len = self.cursor_len(&model, cx);
        let next = if bottom { len.saturating_sub(1) } else { 0 };
        self.set_cursor(next, len, bottom, cx);
    }

    fn half_page(&self, sign: isize, window: &mut Window, cx: &mut App) {
        let height = f32::from(window.viewport_size().height);
        let rows = crate::state::half_page(visible_rows(height, cx));
        self.move_by(sign * rows, window, cx);
    }

    // ------------------------------------------------------------------ contexts

    fn select_context(&self, digit: usize, cx: &mut App) {
        let id = {
            let state = self.state.read(cx);
            let contexts = state
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.contexts.clone())
                .unwrap_or_default();
            hub_context_bar::context_for_digit(&contexts, digit).cloned()
        };
        if let Some(id) = id {
            self.activate_context(id, cx);
        }
    }

    fn cycle_context(&self, delta: isize, cx: &mut App) {
        let id = {
            let state = self.state.read(cx);
            let contexts = state
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.contexts.clone())
                .unwrap_or_default();
            hub_context_bar::cycle(&contexts, state.active_context(), delta).cloned()
        };
        if let Some(id) = id {
            self.activate_context(id, cx);
        }
    }

    /// Switching context is an explicit user action, so it is allowed to reset the scope and
    /// the cursors — background events never are (§3.3 cursor stability).
    fn activate_context(&self, id: ContextId, cx: &mut App) {
        if self.refuses(cx) {
            return;
        }
        self.bridge
            .send(RequestBody::SetActiveContext { id: Some(id) });
        self.state.update(cx, |state, cx| {
            state.scope = RepoScope::All;
            state.cursors.repos = 0;
            state.cursors.worktrees = 0;
            cx.notify();
        });
        self.hub.update(cx, |hub, _| {
            hub.prs = PrCache::default();
        });
    }

    fn delete_context(&self, cx: &mut App) {
        let Some(id) = self.state.read(cx).active_context().cloned() else {
            return;
        };
        let facts = FactList::from_facts([Fact::risk(
            "deleting a context deletes its repos and worktrees",
        )]);
        self.prepare_confirm(
            ConfirmRequest {
                title: SharedString::from(format!("Delete context {id}?")),
                facts,
                key: ConfirmKey::Upper,
                action: PendingAction::DeleteContext(id),
            },
            cx,
        );
    }

    fn go_all_repos(&self, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.scope = RepoScope::All;
            state.cursors.repos = 0;
            state.cursors.worktrees = 0;
            cx.notify();
        });
    }

    // ------------------------------------------------------------------ selection helpers

    fn selected_rail_row(&self, cx: &App) -> Option<RailRow> {
        let model = self.model(cx);
        model.rail.get(self.state.read(cx).cursors.repos).cloned()
    }

    fn selected_worktree(&self, cx: &App) -> Option<WorktreeRow> {
        let model = self.model(cx);
        model
            .worktrees
            .get(self.state.read(cx).cursors.worktrees)
            .cloned()
    }

    fn selected_pr(&self, cx: &App) -> Option<PrRow> {
        let model = self.model(cx);
        let state = self.state.read(cx);
        let cursor = match state.pr_tab {
            PrTab::Mine => state.cursors.prs_mine,
            PrTab::Review => state.cursors.prs_review,
        };
        model.prs.get(cursor).cloned()
    }

    /// The repository the list is scoped to, or the one under the cursor.
    fn scoped_repo(&self, cx: &App) -> Option<RepoId> {
        match &self.state.read(cx).scope {
            RepoScope::Repo(repo) => Some(repo.clone()),
            RepoScope::All => self.selected_worktree(cx).map(|row| row.repo),
        }
    }

    // ------------------------------------------------------------------ repos rail

    fn open_repo(&self, cx: &mut App) {
        let Some(row) = self.selected_rail_row(cx) else {
            return;
        };
        if row.kind == RailKind::CloneFailed {
            // §3.2: `Enter` on a failed clone opens the Jobs panel focused on that job.
            self.state.update(cx, |state, cx| {
                state.open_overlay(Overlay::Jobs);
                cx.notify();
            });
            return;
        }
        self.state.update(cx, |state, cx| {
            state.scope = match &row.repo {
                Some(repo) if row.kind != RailKind::All => RepoScope::Repo(repo.clone()),
                _ => RepoScope::All,
            };
            state.cursors.worktrees = 0;
            state.hub_pane = HubPane::List;
            cx.notify();
        });
        self.schedule_inspection(cx);
    }

    fn delete_repo(&self, cx: &mut App) {
        let Some(repo) = self.selected_rail_row(cx).and_then(|row| row.repo) else {
            return;
        };
        let worktrees = self
            .state
            .read(cx)
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .worktrees
                    .iter()
                    .filter(|worktree| worktree.repo_id == repo)
                    .count()
            })
            .unwrap_or_default();
        let facts = FactList::from_facts([
            Fact::risk(format!("{worktrees} worktrees are deleted with it")),
            Fact::risk("the pristine clone is moved to trash"),
        ]);
        self.prepare_confirm(
            ConfirmRequest {
                title: SharedString::from(format!("Delete {repo}?")),
                facts,
                key: ConfirmKey::Upper,
                action: PendingAction::DeleteRepo(repo),
            },
            cx,
        );
    }

    /// `x` is bound only on a clone-failed row; on every other row it is a no-op (KEYMAP §Repos).
    fn dismiss_clone(&self, cx: &mut App) {
        let Some(row) = self.selected_rail_row(cx) else {
            return;
        };
        if row.kind != RailKind::CloneFailed {
            return;
        }
        let Some(repo) = row.repo else { return };
        if self.refuses(cx) {
            return;
        }
        self.bridge.send(RequestBody::DismissClone { repo });
    }

    /// `e` — edit the selected repository's hooks (KEYMAP A16).
    ///
    /// **Integration request.** `Dialogs` has no `EditHooks` variant, so the Hub cannot open the
    /// editor; the daemon side (`RequestBody::SetRepoHooks`) is ready. Until the variant exists
    /// this states the refusal and its reason, which is the §2.7 "action refused" toast rather
    /// than a silent no-op.
    fn edit_hooks(&self, cx: &mut App) {
        if self
            .selected_rail_row(cx)
            .and_then(|row| row.repo)
            .is_none()
        {
            return;
        }
        self.toast("Hook editing needs the repo dialog", Icon::Info, false, cx);
    }

    // ------------------------------------------------------------------ worktrees

    fn open_worktree(&self, sleep_previous: bool, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        if self.refuses(cx) {
            return;
        }
        let id = row.id.clone();
        self.bridge
            .send(RequestBody::TouchWorktreeOpened { id: id.clone() });
        self.ask(
            RequestBody::EnsureSession {
                worktree: Some(id),
                agent: None,
                sleep_previous,
            },
            cx,
            |result, ctx, cx| ctx.enter_session(result, cx),
        );
    }

    /// Moves to the Workspace once the daemon confirms the session exists.
    fn enter_session(&self, result: Result<ResponseBody, ProtoError>, cx: &mut gpui::AsyncApp) {
        match result {
            Ok(ResponseBody::Session(session)) => {
                self.state.update(cx, |state, cx| {
                    state.touch_session(session.id.clone());
                    state.screen = Screen::Workspace {
                        session: session.id,
                    };
                    cx.notify();
                });
            }
            Ok(_) => {}
            Err(error) => self.report(error, cx),
        }
    }

    /// A failed request is sticky, never a toast (§1.8).
    fn report(&self, error: ProtoError, cx: &mut gpui::AsyncApp) {
        self.state.update(cx, |state, cx| {
            state.sticky_error = Some(crate::state::StickyError {
                text: error.message,
                job: None,
                retryable: false,
            });
            cx.notify();
        });
    }

    fn delete_worktree(&self, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        let facts = delete_facts(self.hub.read(cx).inspections.get(&row.id));
        let key = facts.confirm_key();
        self.prepare_confirm(
            ConfirmRequest {
                title: SharedString::from(format!("Delete {}?", row.branch)),
                facts,
                key,
                action: PendingAction::DeleteWorktree(row.id.clone()),
            },
            cx,
        );
        // §3.8.3: the confirm quotes the freshest facts it can get, so re-inspect behind it.
        self.inspect(row.id, false, cx);
    }

    fn undo_delete(&self, cx: &mut App) {
        let entry = self.hub.read(cx).last_trash_entry.clone();
        let Some(entry) = entry else {
            self.toast("Nothing to undo", Icon::Info, true, cx);
            return;
        };
        if self.refuses(cx) {
            return;
        }
        self.bridge.send(RequestBody::RestoreTrash { entry });
        self.hub.update(cx, |hub, _| hub.last_trash_entry = None);
    }

    /// `x` — the dry run first, then either a refusal toast or the confirm (§3.8.3).
    fn prune(&self, cx: &mut App) {
        if self.refuses(cx) {
            return;
        }
        let repo = self.scoped_repo(cx);
        self.ask(
            RequestBody::PruneWorktrees {
                dry_run: true,
                fetch: false,
                kill_sessions: false,
                repo: repo.clone(),
            },
            cx,
            move |result, ctx, cx| match result {
                Ok(ResponseBody::Pruned(preview)) => ctx.show_prune(repo, preview, cx),
                Ok(_) => {}
                Err(error) => ctx.report(error, cx),
            },
        );
    }

    fn show_prune(&self, repo: Option<RepoId>, preview: PruneResult, cx: &mut gpui::AsyncApp) {
        if preview.deleted.is_empty() {
            let scope = repo
                .as_ref()
                .map_or_else(|| "this context".to_owned(), |repo| repo.name().to_owned());
            let skipped = preview.skipped.len();
            let now = Instant::now();
            self.state.update(cx, |state, cx| {
                state.toast(
                    Toast::new(format!(
                        "Nothing to prune in {scope} \u{2014} {skipped} skipped · J for reasons"
                    ))
                    .icon(Icon::Info),
                    now,
                    dwell_for(ToastDuration::Normal),
                );
                cx.notify();
            });
            return;
        }
        let facts = prune_facts(&preview);
        let key = facts.confirm_key();
        let request = ConfirmRequest {
            title: SharedString::from(format!("Prune {} worktrees?", preview.deleted.len())),
            facts,
            key,
            action: PendingAction::Prune {
                repo,
                deleted: preview.deleted,
            },
        };
        self.hub.update(cx, |hub, _| {
            hub.pending_confirm = Some(request);
        });
        self.state.update(cx, |state, cx| {
            state.open_overlay(Overlay::Dialog(Dialogs::Confirm));
            cx.notify();
        });
    }

    fn sleep(&self, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        if self.refuses(cx) {
            return;
        }
        self.ask(
            RequestBody::SleepWorktree { id: row.id },
            cx,
            |result, ctx, cx| match result {
                Ok(ResponseBody::Slept(report)) if !report.kept.is_empty() => {
                    let kept = report
                        .kept
                        .iter()
                        .map(|kept| format!("{} ({})", kept.window, kept.reason))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let now = Instant::now();
                    ctx.state.update(cx, |state, cx| {
                        state.toast(
                            Toast::new(format!("Slept · kept {kept}")).icon(Icon::Moon),
                            now,
                            dwell_for(ToastDuration::Normal),
                        );
                        cx.notify();
                    });
                }
                Ok(_) => {}
                Err(error) => ctx.report(error, cx),
            },
        );
    }

    fn kill(&self, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        let labels = if row.keep_alive.is_empty() {
            Fact::safe("no keep-alive processes")
        } else {
            Fact::risk(format!("kills {}", row.keep_alive.join(", ")))
        };
        let facts = FactList::from_facts([labels, Fact::risk("terminals are killed, not slept")]);
        let key = facts.confirm_key();
        self.prepare_confirm(
            ConfirmRequest {
                title: SharedString::from(format!("Kill the session of {}?", row.branch)),
                facts,
                key,
                action: PendingAction::KillSession(row.id),
            },
            cx,
        );
    }

    fn inspect_selected(&self, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        self.inspect(row.id, true, cx);
    }

    /// `I` inspects with a fetch; the debounce inspects without one (§2.6 D-4).
    fn inspect(&self, id: WorktreeId, fetch: bool, cx: &mut App) {
        if self.state.read(cx).daemon.is_lost() {
            return;
        }
        self.hub.update(cx, |hub, _| {
            let slot = hub.inspections.entry(id.clone()).or_default();
            slot.loading = true;
        });
        self.ask(
            RequestBody::InspectWorktrees {
                ids: vec![id.clone()],
                repo: None,
                fetch,
            },
            cx,
            move |result, ctx, cx| {
                ctx.hub.update(cx, |hub, _| match result {
                    Ok(ResponseBody::Inspections(inspections)) => {
                        for inspection in inspections {
                            hub.inspections.insert(
                                inspection.worktree_id.clone(),
                                Inspected::ready(inspection),
                            );
                        }
                    }
                    Ok(_) => {
                        hub.inspections.remove(&id);
                    }
                    Err(error) => {
                        hub.inspections.insert(id, Inspected::failed(error.message));
                    }
                });
                ctx.state.update(cx, |_, cx| cx.notify());
            },
        );
    }

    fn copy_path(&self, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        self.ask(
            RequestBody::WorktreePath { id: row.id },
            cx,
            |result, ctx, cx| {
                if let Ok(ResponseBody::Path(path)) = result {
                    cx.update(|cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(path));
                    });
                    let now = Instant::now();
                    ctx.state.update(cx, |state, cx| {
                        state.toast_short("Path copied", Icon::ClipboardCheck, now);
                        cx.notify();
                    });
                }
            },
        );
    }

    fn copy_branch(&self, cx: &mut App) {
        let Some(row) = self.selected_worktree(cx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(row.branch.to_string()));
        self.toast("Branch copied", Icon::ClipboardCheck, true, cx);
    }

    // ------------------------------------------------------------------ pull requests

    fn switch_tab(&self, tab: PrTab, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            if state.pr_tab != tab {
                state.pr_tab = tab;
                cx.notify();
            }
        });
    }

    fn back_to_worktrees(&self, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.screen = Screen::Hub {
                tab: HubTab::Worktrees,
            };
            state.filter = crate::state::FilterState::default();
            cx.notify();
        });
    }

    /// Fetches both tabs. `force` is `r`; otherwise the daemon's TTL decides.
    fn fetch_pull_requests(&self, force: bool, cx: &mut App) {
        if self.state.read(cx).daemon.is_lost() {
            return;
        }
        let (repo, context) = {
            let state = self.state.read(cx);
            match &state.scope {
                RepoScope::Repo(repo) => (Some(repo.clone()), None),
                RepoScope::All => (None, state.active_context().cloned()),
            }
        };
        self.hub.update(cx, |hub, _| {
            hub.prs.loading = true;
            hub.prs.started_at = Some(Instant::now());
            hub.prs.error = None;
        });
        for tab in [PrTab::Mine, PrTab::Review] {
            self.ask(
                RequestBody::ListPullRequests {
                    repo: repo.clone(),
                    context: context.clone(),
                    tab,
                    force,
                },
                cx,
                move |result, ctx, cx| ctx.apply_slices(tab, result, cx),
            );
        }
    }

    fn apply_slices(
        &self,
        tab: PrTab,
        result: Result<ResponseBody, ProtoError>,
        cx: &mut gpui::AsyncApp,
    ) {
        let review = self.hub.update(cx, |hub, _| {
            match result {
                Ok(ResponseBody::PullRequests(slices)) => {
                    for slice in slices {
                        match slice.tab {
                            PrTab::Mine => hub.prs.mine = Some(slice),
                            PrTab::Review => hub.prs.review = Some(slice),
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => hub.prs.error = Some(error.message),
            }
            if tab == PrTab::Review {
                hub.prs.loading = false;
                hub.prs.started_at = None;
                hub.prs.fetched_at = Some(Instant::now());
            }
            hub.prs.review.as_ref().map_or(0, |slice| slice.prs.len())
        });
        self.state.update(cx, |state, cx| {
            state.review_pr_count = review;
            cx.notify();
        });
    }

    /// `Enter` / `O` / `c` on the PR screen: open the local worktree, or create it first.
    fn open_pr(&self, open: bool, sleep_previous: bool, cx: &mut App) {
        let Some(row) = self.selected_pr(cx) else {
            return;
        };
        if self.refuses(cx) {
            return;
        }
        if let Some(id) = row.local.clone() {
            if !open {
                return;
            }
            self.bridge
                .send(RequestBody::TouchWorktreeOpened { id: id.clone() });
            self.ask(
                RequestBody::EnsureSession {
                    worktree: Some(id),
                    agent: None,
                    sleep_previous,
                },
                cx,
                |result, ctx, cx| ctx.enter_session(result, cx),
            );
            return;
        }
        let key = (row.repo.clone(), row.number);
        self.hub.update(cx, |hub, _| hub.creating.push(key.clone()));
        self.ask(
            RequestBody::CreateWorktreeFromPr {
                repo: row.repo.clone(),
                number: row.number,
            },
            cx,
            move |result, ctx, cx| {
                ctx.hub.update(cx, |hub, _| {
                    hub.creating.retain(|candidate| candidate != &key);
                });
                match result {
                    Ok(ResponseBody::Worktree { worktree, .. }) if open => {
                        ctx.ask_from_async(worktree.id, sleep_previous, cx);
                    }
                    Ok(_) => {
                        ctx.state.update(cx, |_, cx| cx.notify());
                    }
                    Err(error) => ctx.report(error, cx),
                }
            },
        );
    }

    /// The second half of "create then open", already on the async context.
    fn ask_from_async(&self, id: WorktreeId, sleep_previous: bool, cx: &mut gpui::AsyncApp) {
        let reply = self.bridge.request(RequestBody::EnsureSession {
            worktree: Some(id),
            agent: None,
            sleep_previous,
        });
        let ctx = self.clone();
        cx.spawn(async move |cx| {
            if let Ok(result) = reply.recv().await {
                ctx.enter_session(result, cx);
            }
        })
        .detach();
    }

    fn inspect_pr(&self, cx: &mut App) {
        let Some(id) = self.selected_pr(cx).and_then(|row| row.local) else {
            return;
        };
        self.inspect(id, true, cx);
    }

    fn copy_pr_url(&self, cx: &mut App) {
        let Some(row) = self.selected_pr(cx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(row.url.to_string()));
        self.toast("PR URL copied", Icon::ClipboardCheck, true, cx);
    }

    // ------------------------------------------------------------------ shared keys

    fn open_agent(&self, agent: Agent, cx: &mut App) {
        if self.refuses(cx) {
            return;
        }
        self.ask(
            RequestBody::EnsureSession {
                worktree: None,
                agent: Some(agent),
                sleep_previous: true,
            },
            cx,
            |result, ctx, cx| ctx.enter_session(result, cx),
        );
    }

    fn refresh(&self, cx: &mut App) {
        if self.refuses(cx) {
            return;
        }
        let repo = match &self.state.read(cx).scope {
            RepoScope::Repo(repo) => Some(repo.clone()),
            RepoScope::All => None,
        };
        self.bridge.send(RequestBody::RefreshStatuses { repo });
        self.fetch_pull_requests(true, cx);
    }

    /// `b` — the PR's URL on the PR screen, the inspected PR or the repo elsewhere.
    fn open_in_browser(&self, cx: &mut App) {
        let url = if matches!(self.state.read(cx).screen, Screen::Hub { tab: HubTab::Prs }) {
            self.selected_pr(cx).map(|row| row.url.to_string())
        } else if self.state.read(cx).hub_pane == HubPane::Repos {
            self.selected_rail_row(cx)
                .and_then(|row| row.repo)
                .map(|repo| format!("https://github.com/{repo}"))
        } else {
            self.selected_worktree(cx).and_then(|row| {
                let hub = self.hub.read(cx);
                hub.inspections
                    .get(&row.id)
                    .and_then(|slot| slot.data.as_ref())
                    .and_then(|data| data.pr.as_ref())
                    .map(|pr| pr.url.clone())
                    .or_else(|| Some(format!("https://github.com/{}", row.repo)))
            })
        };
        if let Some(url) = url {
            cx.open_url(&url);
        }
    }

    fn open_dialog(&self, dialog: Dialogs, cx: &mut App) {
        if self.refuses(cx) {
            return;
        }
        self.state.update(cx, |state, cx| {
            state.open_overlay(Overlay::Dialog(dialog));
            cx.notify();
        });
    }

    /// Stores a prepared confirm and opens the shared §3.8 dialog frame.
    fn prepare_confirm(&self, request: ConfirmRequest, cx: &mut App) {
        if self.refuses(cx) {
            return;
        }
        self.hub.update(cx, |hub, _| {
            hub.pending_confirm = Some(request);
        });
        self.state.update(cx, |state, cx| {
            state.open_overlay(Overlay::Dialog(Dialogs::Confirm));
            cx.notify();
        });
    }

    // ------------------------------------------------------------------ background cadence

    /// Fetches the PR tabs the first time the screen is shown and after the TTL (§3.5).
    fn tick_pull_requests(&self, cx: &mut App) {
        let wanted = {
            let state = self.state.read(cx);
            matches!(state.screen, Screen::Hub { tab: HubTab::Prs })
                && state.daemon.is_connected()
                && self.hub.read(cx).prs.needs_fetch(Instant::now())
        };
        if wanted {
            self.fetch_pull_requests(false, cx);
        }
    }

    /// Inspects the selected worktree the first time it is seen (§2.6 D-4 a).
    fn tick_inspection(&self, cx: &mut App) {
        let wanted = {
            let state = self.state.read(cx);
            if !state.daemon.is_connected()
                || !matches!(
                    state.screen,
                    Screen::Hub {
                        tab: HubTab::Worktrees
                    }
                )
            {
                None
            } else {
                self.selected_worktree(cx)
                    .map(|row| row.id)
                    .filter(|id| !self.hub.read(cx).inspections.contains_key(id))
            }
        };
        if let Some(id) = wanted {
            self.hub.update(cx, |hub, _| {
                hub.inspections.insert(id.clone(), Inspected::pending());
            });
            self.inspect(id, false, cx);
        }
    }

    /// Re-inspects the selected worktree once the cursor has been still for the debounce.
    fn schedule_inspection(&self, cx: &mut App) {
        let generation = self.hub.update(cx, |hub, _| {
            hub.inspect_generation = hub.inspect_generation.wrapping_add(1);
            hub.inspect_generation
        });
        let ctx = self.clone();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(AUTO_INSPECT_DEBOUNCE).await;
            let stale = ctx
                .hub
                .update(cx, |hub, _| hub.inspect_generation != generation);
            if stale {
                return;
            }
            let id = cx.update(|cx| ctx.selected_worktree(cx).map(|row| row.id));
            if let Some(id) = id {
                cx.update(|cx| ctx.inspect(id, false, cx));
            }
        })
        .detach();
    }
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

#[cfg(test)]
mod tests {
    use fleet_core::{ids::RepoId, sessions::SessionState};

    use super::*;

    fn inspection(dirty: bool, unique: Option<u64>, merged: bool) -> WorktreeInspection {
        WorktreeInspection {
            worktree_id: WorktreeId::try_from("buk/payroll#a")
                .unwrap_or_else(|error| panic!("{error}")),
            repo_id: RepoId::try_from("buk/payroll").unwrap_or_else(|error| panic!("{error}")),
            host: "local".to_owned(),
            path: "/tmp/wt".to_owned(),
            branch: "feat/x".to_owned(),
            base_ref: "origin/main".to_owned(),
            head: Some("abc".to_owned()),
            target_branch: "main".to_owned(),
            upstream: None,
            ahead: None,
            behind: None,
            upstream_gone: false,
            dirty,
            dirty_files: dirty.then_some(12),
            merged_into_target: merged,
            unique_commits: unique,
            published: true,
            merged,
            pr: None,
            session: SessionState::None,
            running: Vec::new(),
            inspected_at: "2026-09-04T11:59:00Z".to_owned(),
            warnings: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn iso_stamps_parse_to_epoch_seconds() {
        assert_eq!(epoch_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_secs("2026-09-04T12:00:00Z"), Some(1_788_523_200));
        assert_eq!(epoch_secs("2026-09-04T12:00:00.123Z"), Some(1_788_523_200));
        assert_eq!(epoch_secs("2026-09-04T14:00:00+02:00"), Some(1_788_523_200));
        assert_eq!(epoch_secs("not a date"), None);
        assert_eq!(epoch_secs("2026-13-04T12:00:00Z"), None);
    }

    #[test]
    fn ages_never_go_negative() {
        let now = 1_788_523_200;
        assert_eq!(age_secs("2026-09-04T11:59:00Z", now), Some(60));
        assert_eq!(age_secs("2026-09-04T12:01:00Z", now), Some(0));
        assert_eq!(age_secs("garbage", now), None);
    }

    #[test]
    fn the_ladder_matches_the_documented_frame() {
        // §2.1: 1280 px gives the list 138 ch, and 93 ch with the detail panel inset.
        assert!((list_pane_ch(1280.0, false, false) - 138.6).abs() < 0.2);
        assert!((list_pane_ch(1280.0, true, false) - 164.8).abs() < 0.2);
        assert!((list_pane_ch(1280.0, false, true) - 93.3).abs() < 0.2);
    }

    #[test]
    fn a_narrow_window_docks_the_panel_instead_of_stealing_width() {
        assert!(detail_is_docked(1100.0));
        assert!(!detail_is_docked(1280.0));
        // Docked, the list keeps its full width.
        let docked = list_pane_ch(1100.0, false, true);
        assert!((docked - list_pane_ch(1100.0, false, false)).abs() < f32::EPSILON);
        assert!(docked > 72.0, "never below the 72 ch floor of §3.4");
    }

    #[test]
    fn a_never_inspected_worktree_escalates_the_confirm_key() {
        let facts = delete_facts(None);
        assert_eq!(facts.confirm_key(), ConfirmKey::Upper);
    }

    #[test]
    fn a_clean_merged_worktree_confirms_with_the_lowercase_key() {
        let facts = delete_facts(Some(&Inspected::ready(inspection(false, Some(0), true))));
        assert_eq!(facts.confirm_key(), ConfirmKey::Lower);
        assert!(facts.is_compact());
    }

    #[test]
    fn an_unknown_unique_commit_count_escalates_the_confirm_key() {
        let facts = delete_facts(Some(&Inspected::ready(inspection(false, None, true))));
        assert_eq!(facts.confirm_key(), ConfirmKey::Upper);
    }

    #[test]
    fn a_dirty_worktree_states_its_file_count_as_a_risk() {
        let facts = delete_facts(Some(&Inspected::ready(inspection(true, Some(2), false))));
        assert!(!facts.is_compact());
        assert!(
            facts
                .ordered()
                .iter()
                .any(|fact| fact.text.as_ref() == "12 uncommitted files")
        );
    }

    #[test]
    fn the_pr_cache_expires_on_the_documented_ttl() {
        let mut cache = PrCache::default();
        assert!(cache.needs_fetch(Instant::now()));
        assert!(cache.is_cold());
        let now = Instant::now();
        cache.fetched_at = Some(now);
        assert!(!cache.needs_fetch(now));
        assert!(!cache.needs_fetch(now + PR_TTL - Duration::from_secs(1)));
        assert!(cache.needs_fetch(now + PR_TTL));
        cache.loading = true;
        cache.started_at = Some(now);
        cache.fetched_at = None;
        assert!(!cache.needs_fetch(now), "a running fetch is not repeated");
        assert!(
            cache.needs_fetch(now + PR_TTL),
            "a fetch whose answer never arrived stops blocking the screen"
        );
    }

    #[test]
    fn an_inspection_slot_never_blanks_its_previous_values() {
        let mut slot = Inspected::ready(inspection(true, Some(1), false));
        slot.loading = true;
        assert!(slot.data.is_some());
        assert!(Inspected::pending().data.is_none());
        assert_eq!(
            Inspected::failed("gh unavailable").error.as_deref(),
            Some("gh unavailable")
        );
    }
}
