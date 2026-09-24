use super::*;

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PrCacheKey {
    repo: Option<RepoId>,
    context: Option<ContextId>,
    generation: u64,
}

impl PrCacheKey {
    pub(super) fn from_state(state: &AppState) -> Self {
        match &state.scope {
            RepoScope::Repo(repo) => Self {
                repo: Some(repo.clone()),
                context: None,
                generation: state.link_generation,
            },
            RepoScope::All => Self {
                repo: None,
                context: effective_context(state).map(|context| context.id.clone()),
                generation: state.link_generation,
            },
        }
    }
}

#[derive(Debug, Clone, Default)]
struct PrTabFetch {
    request: Option<u64>,
    loading: bool,
    error: Option<String>,
    fetched_at: Option<Instant>,
    started_at: Option<Instant>,
}

impl PrTabFetch {
    fn needs_fetch(&self, now: Instant, freshness: PrFreshness) -> bool {
        if self.loading
            && self
                .started_at
                .is_some_and(|at| !freshness.expired(at, now))
        {
            return false;
        }
        self.fetched_at.is_none_or(|at| freshness.expired(at, now))
    }

    fn deadline(&self, freshness: PrFreshness) -> Option<Instant> {
        self.loading
            .then_some(self.started_at)
            .flatten()
            .or(self.fetched_at)
            .map(|at| freshness.deadline(at))
    }
}

/// Shared Hub/Workspace freshness policy for successful, failed, and abandoned PR lookups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PrFreshness(Duration);

impl Default for PrFreshness {
    fn default() -> Self {
        Self(PR_TTL)
    }
}

impl PrFreshness {
    #[must_use]
    pub(crate) fn from_effective(config: &crate::bridge::EffectiveConfig) -> Self {
        Self(config.pr_ttl)
    }

    #[must_use]
    pub(crate) fn duration(self) -> Duration {
        self.0
    }

    #[must_use]
    fn deadline(self, fetched_at: Instant) -> Instant {
        fetched_at + self.0
    }

    #[must_use]
    fn expired(self, fetched_at: Instant, now: Instant) -> bool {
        now >= self.deadline(fetched_at)
    }
}

/// The two pull-request tabs, with the fetch state §3.5 renders.
#[derive(Debug, Clone, Default)]
pub struct PrCache {
    /// The `Mine` slice.
    pub mine: Option<PrSlice>,
    /// The `Review` slice.
    pub review: Option<PrSlice>,
    key: Option<PrCacheKey>,
    mine_fetch: PrTabFetch,
    review_fetch: PrTabFetch,
    sequence: u64,
    freshness: PrFreshness,
    config_loading: bool,
    config_loaded: bool,
    /// Whether the Review tab is the context's Reviews board, so `gh` is asked for Mine alone
    /// (UX-SPEC §3.5). Set from `board.reviews` before every fetch decision.
    review_retired: bool,
    /// The Reviews board's facts the Review tab and the chip draw, folded outside render.
    pub(super) review_board: ReviewBoardFacts,
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

    pub(super) fn slice_for(&self, tab: PrTab, key: &PrCacheKey) -> Option<&PrSlice> {
        (self.key.as_ref() == Some(key))
            .then(|| self.slice(tab))
            .flatten()
    }

    pub(super) fn matches(&self, key: &PrCacheKey) -> bool {
        self.key.as_ref() == Some(key)
    }

    fn fetch(&self, tab: PrTab) -> &PrTabFetch {
        match tab {
            PrTab::Mine => &self.mine_fetch,
            PrTab::Review => &self.review_fetch,
        }
    }

    fn fetch_mut(&mut self, tab: PrTab) -> &mut PrTabFetch {
        match tab {
            PrTab::Mine => &mut self.mine_fetch,
            PrTab::Review => &mut self.review_fetch,
        }
    }

    #[must_use]
    pub fn loading(&self, tab: PrTab) -> bool {
        self.fetch(tab).loading
    }

    #[must_use]
    pub fn error(&self, tab: PrTab) -> Option<&str> {
        self.fetch(tab).error.as_deref()
    }

    /// Whether the screen should ask the daemon again (§3.5, `github.prTtlSeconds`).
    ///
    /// A fetch whose answer never arrived (a bridge that went away mid-flight) stops counting
    /// as running after one TTL, so the screen recovers instead of showing `⟳ refreshing`
    /// forever.
    #[must_use]
    pub fn needs_fetch(&self, now: Instant) -> bool {
        self.mine_fetch.needs_fetch(now, self.freshness)
            || (!self.review_retired && self.review_fetch.needs_fetch(now, self.freshness))
    }

    fn refresh_deadline(&self) -> Option<Instant> {
        [
            Some(self.mine_fetch.deadline(self.freshness)),
            (!self.review_retired).then(|| self.review_fetch.deadline(self.freshness)),
        ]
        .into_iter()
        .flatten()
        .flatten()
        .min()
    }

    /// Whether the Review tab's `gh` fetch is retired because the daemon serves Reviews boards.
    #[must_use]
    pub fn review_retired(&self) -> bool {
        self.review_retired
    }

    /// Retires (or restores) the Review tab's `gh` fetch. Retiring drops whatever the old list
    /// held, so neither the tab nor the worktree badges keep a slice nothing refreshes.
    pub(super) fn set_review_retired(&mut self, retired: bool) {
        if self.review_retired == retired {
            return;
        }
        self.review_retired = retired;
        if retired {
            self.review = None;
            self.review_fetch = PrTabFetch::default();
        }
    }

    /// The tabs one fetch asks `gh` for: Mine alone once the Review tab is a board.
    fn fetched_tabs(&self) -> &'static [PrTab] {
        if self.review_retired {
            &[PrTab::Mine]
        } else {
            &[PrTab::Mine, PrTab::Review]
        }
    }

    fn begin_config_fetch(&mut self) -> bool {
        if self.config_loaded || self.config_loading {
            return false;
        }
        self.config_loading = true;
        true
    }

    fn finish_config_fetch(&mut self, result: &Result<ResponseBody, ProtoError>) {
        self.config_loading = false;
        self.config_loaded = true;
        if let Ok(ResponseBody::Config(config)) = result {
            let effective = crate::bridge::EffectiveConfig::from_config(config);
            self.freshness = PrFreshness::from_effective(&effective);
        }
    }

    fn invalidate_config(&mut self) {
        self.config_loaded = false;
    }

    fn needs_fetch_for(&self, key: &PrCacheKey, now: Instant) -> bool {
        self.key.as_ref() != Some(key) || self.needs_fetch(now)
    }

    /// Whether nothing has ever been fetched, which is what shows skeleton rows.
    #[must_use]
    pub fn is_cold(&self, tab: PrTab) -> bool {
        self.slice(tab).is_none()
    }

    pub(super) fn begin_fetch(&mut self, key: PrCacheKey, now: Instant) -> Vec<(PrTab, u64)> {
        if self.key.as_ref() != Some(&key) {
            self.mine = None;
            self.review = None;
            self.mine_fetch = PrTabFetch::default();
            self.review_fetch = PrTabFetch::default();
            self.key = Some(key);
        }
        let mut requests: Vec<(PrTab, u64)> =
            self.fetched_tabs().iter().map(|tab| (*tab, 0)).collect();
        for (tab, request) in &mut requests {
            self.sequence = self.sequence.wrapping_add(1);
            *request = self.sequence;
            let fetch = self.fetch_mut(*tab);
            fetch.request = Some(*request);
            fetch.loading = true;
            fetch.error = None;
            fetch.started_at = Some(now);
        }
        requests
    }

    pub(super) fn apply(
        &mut self,
        key: &PrCacheKey,
        tab: PrTab,
        request: u64,
        result: Result<ResponseBody, ProtoError>,
        now: Instant,
    ) -> bool {
        if self.key.as_ref() != Some(key) || self.fetch(tab).request != Some(request) {
            return false;
        }
        let response = match result {
            Ok(ResponseBody::PullRequests(slices)) => slices
                .into_iter()
                .find(|slice| slice.tab == tab)
                .ok_or_else(|| "daemon omitted the requested pull-request tab".to_owned()),
            Ok(_) => Err("daemon returned an unexpected pull-request response".to_owned()),
            Err(error) => Err(error.message),
        };
        {
            let fetch = self.fetch_mut(tab);
            fetch.request = None;
            fetch.loading = false;
            fetch.started_at = None;
            fetch.fetched_at = Some(now);
        }
        match response {
            Ok(slice) => {
                self.fetch_mut(tab).error = None;
                match tab {
                    PrTab::Mine => self.mine = Some(slice),
                    PrTab::Review => self.review = Some(slice),
                }
            }
            Err(error) => self.fetch_mut(tab).error = Some(error),
        }
        true
    }
}

/// What the Review tab and the context bar's Review chip read from the Reviews board, folded in
/// the Hub's observation of [`AppState`] and never in render (UX-SPEC §2.2, §3.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ReviewBoardFacts {
    key: Option<ReviewFoldKey>,
    /// Reviews waiting on the user: the chip and the Review tab's count.
    pub(super) waiting: usize,
    /// `Some` while the mirror holds the Reviews board and it has no cards: `No reviews yet.`,
    /// with `true` when the GitHub review schedule is offered beside it.
    pub(super) empty: Option<bool>,
}

/// Every input [`review_board_facts`] reads, as revisions and cheap values.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReviewFoldKey {
    context: Option<ContextId>,
    snapshot: u64,
    board: u64,
    marks: u64,
    schedules: u64,
    held: Option<fleet_core::ids::BoardId>,
    offers_schedules: bool,
}

impl ReviewFoldKey {
    fn of(state: &AppState) -> Self {
        Self {
            context: state.active_context().cloned(),
            snapshot: state.snapshot_revision,
            board: state.board.revision,
            marks: state.board.marks.revision,
            schedules: state.schedules.revision,
            held: state.board().map(|view| view.board.id.clone()),
            offers_schedules: state.supports_schedules(),
        }
    }
}

/// The active context's Reviews board, when the mirror is the one holding it.
fn held_reviews_board(state: &AppState) -> Option<&fleet_core::board::BoardView> {
    let context = state.active_context()?;
    state
        .board()
        .filter(|view| !view.board.kind.is_tasks() && &view.board.context_id == context)
}

/// Reviews waiting on the user, and the empty state, from what the app already holds.
///
/// One rule answers on both sides: a card counts when it wants the user
/// (`ops::query::attention`), or when it stands in a `Started` column with no live or owed run.
/// With the Reviews board in the mirror the card marks fold it
/// ([`CardMarks::waiting_on_you`](crate::state::CardMarks)); anywhere else the snapshot's summary
/// carries the same rule split in two, `attention_count + idle_started`, so the count does not
/// jump when the Review tab opens or closes (UX-SPEC §2.2).
pub(super) fn review_board_facts(state: &AppState) -> (usize, Option<bool>) {
    if let Some(view) = held_reviews_board(state) {
        let waiting = usize::try_from(state.board.marks.waiting_on_you).unwrap_or(usize::MAX);
        let empty = view.cards.iter().all(|card| card.archived);
        let offer = state.supports_schedules()
            && state.schedules.entry(&view.board.id).is_some_and(|entry| {
                !entry.loading && entry.error.is_none() && entry.schedules.is_empty()
            });
        return (waiting, empty.then_some(offer));
    }
    let waiting = state
        .active_context()
        .zip(state.snapshot.as_ref())
        .and_then(|(context, snapshot)| {
            // Found by its kind, not by a derived id: the app never derives a board id (§0),
            // and a Reviews board whose natural id was taken lives under another one.
            snapshot.boards.iter().find(|board| {
                board.worktree_id.is_none()
                    && !board.kind.is_tasks()
                    && &board.context_id == context
            })
        })
        .map_or(0, |summary| {
            usize::try_from(summary.attention_count.saturating_add(summary.idle_started))
                .unwrap_or(usize::MAX)
        });
    (waiting, None)
}

impl HubBridge {
    /// The daemon bridge the board mirror's loaders take, when this Hub has a live one.
    fn live(&self) -> Option<&Bridge> {
        match self {
            Self::Live(bridge) => Some(bridge),
            #[cfg(test)]
            Self::Test(_) => None,
        }
    }
}

impl HubCtx {
    /// The Review tab's half of the Hub's observation (UX-SPEC §3.5).
    ///
    /// While the tab draws the Reviews board it owns the board mirror: the scope is
    /// `Reviews(active context)`, the list pane has the keyboard (the rail is hidden), and the
    /// board's schedules are asked for once so the empty state knows whether to offer one.
    /// Leaving the tab gives the mirror back to the context scope the Hub's Board tab shows,
    /// which is also what stops the loader asking for a board nothing draws. The count the chip
    /// and the tab read is folded here, on a daemon that serves Reviews boards.
    pub(super) fn synchronize_review_board(&self, cx: &mut App) {
        let (shown, context, holds_reviews, supported) = {
            let state = self.state.read(cx);
            (
                state.review_board_is_shown(),
                state.active_context().cloned(),
                matches!(
                    state.board.scope,
                    Some(crate::state::BoardScope::Reviews(_))
                ),
                state.supports_review_boards(),
            )
        };
        self.hub.update(cx, |hub, _| {
            hub.observe_review_schedule_activation(shown, context.as_ref());
        });
        if shown {
            self.state.update(cx, |state, cx| {
                if state.hub_pane != HubPane::List {
                    state.hub_pane = HubPane::List;
                    cx.notify();
                }
            });
            if let (Some(context), Some(bridge), true) = (context, self.bridge.live(), supported) {
                // Notifies only when the mirror moved and sends nothing when the slot is
                // current, so it is safe on every notify; gated on the capability so an old
                // daemon's refusal is not toasted once per frame.
                crate::screens::board::enter_reviews_scope(context, &self.state, bridge, cx);
            }
            self.load_review_schedules(cx);
        } else if holds_reviews {
            let on_board_tab = matches!(
                self.state.read(cx).screen,
                Screen::Hub { tab: HubTab::Board }
            );
            if let (false, Some(bridge)) = (on_board_tab, self.bridge.live()) {
                crate::screens::board::enter_context_scope(&self.state, bridge, cx);
            }
        }
        if supported {
            self.fold_review_board(cx);
        }
    }

    /// Folds the Reviews board into the chip's and the tab's count and the empty state.
    fn fold_review_board(&self, cx: &mut App) {
        let key = ReviewFoldKey::of(self.state.read(cx));
        if self.hub.read(cx).prs.review_board.key.as_ref() == Some(&key) {
            return;
        }
        let (waiting, empty) = review_board_facts(self.state.read(cx));
        let changed = self.hub.update(cx, |hub, _| {
            let facts = &mut hub.prs.review_board;
            facts.key = Some(key);
            let changed = facts.waiting != waiting || facts.empty != empty;
            facts.waiting = waiting;
            facts.empty = empty;
            changed
        });
        self.state.update(cx, |state, cx| {
            if state.review_pr_count != waiting || changed {
                state.review_pr_count = waiting;
                cx.notify();
            }
        });
    }

    /// Asks for the shown Reviews board's schedules once per activation, so `No reviews yet.`
    /// can say whether the GitHub review schedule is still to be added.
    fn load_review_schedules(&self, cx: &mut App) {
        let board = {
            let state = self.state.read(cx);
            if !state.supports_schedules() || state.refuses_mutations() {
                return;
            }
            let Some(view) = held_reviews_board(state) else {
                return;
            };
            view.board.id.clone()
        };
        let Some(bridge) = self.bridge.live() else {
            return;
        };
        let activated = self
            .hub
            .update(cx, |hub, _| hub.take_review_schedule_activation());
        let wanted = self
            .state
            .read(cx)
            .schedules
            .entry(&board)
            .is_none_or(|entry| activated && entry.error.is_some() && !entry.loading);
        if !wanted {
            return;
        }
        // The dialog's loader: one request in flight per board, and an answer the entry is
        // still stale after (a `SchedulesChanged` landed while it was in flight) reads again.
        crate::dialogs::load_schedules(board, &self.state, bridge, cx);
    }
}

pub(super) fn replace_pr_badges(
    badges: &mut HashMap<(RepoId, String), (u64, fleet_ui_kit::PrBadgeState)>,
    repos: &HashSet<RepoId>,
    replacement: impl IntoIterator<Item = ((RepoId, String), (u64, fleet_ui_kit::PrBadgeState))>,
) {
    badges.retain(|(repo, _), _| !repos.contains(repo));
    badges.extend(replacement);
}

impl HubCtx {
    fn inspection_sweep_ids(&self, cx: &App) -> Vec<WorktreeId> {
        let state = self.state.read(cx);
        if !state.daemon.is_connected() {
            return Vec::new();
        }
        let Some(snapshot) = state.snapshot.as_ref() else {
            return Vec::new();
        };
        let context = effective_context(state).map(|context| &context.id);
        snapshot
            .worktrees
            .iter()
            .filter(|worktree| worktree.host.is_none())
            .filter(|worktree| {
                context.is_none_or(|context| {
                    snapshot
                        .repos
                        .iter()
                        .any(|repo| repo.id == worktree.repo_id && &repo.context_id == context)
                })
            })
            .filter(|worktree| match &state.scope {
                RepoScope::All => true,
                RepoScope::Repo(repo) => &worktree.repo_id == repo,
            })
            .filter(|worktree| {
                !snapshot.jobs.iter().any(|job| {
                    matches!(
                        job.status,
                        JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
                    ) && matches!(job.kind, JobKind::CreateWorktree | JobKind::DeleteWorktree)
                        && worktrees_list::job_targets_worktree(job, &worktree.id)
                })
            })
            .map(|worktree| worktree.id.clone())
            .collect()
    }

    fn request_inspection_sweep(&self, cx: &mut App) {
        let ids = self.inspection_sweep_ids(cx);
        if ids.is_empty() {
            return;
        }
        let Some(request) = self.hub.update(cx, |hub, _| hub.begin_inspection_sweep()) else {
            return;
        };
        self.ask(
            RequestBody::InspectWorktrees {
                ids,
                repo: None,
                fetch: false,
                background: true,
            },
            cx,
            move |result, ctx, cx| {
                let changed = ctx.hub.update(cx, |hub, _| {
                    hub.apply_inspection_sweep(request, result, Instant::now())
                });
                if changed {
                    ctx.state.update(cx, |_, cx| cx.notify());
                }
                let pending = ctx.hub.read_with(cx, |hub, _| hub.inspection_sweep_pending);
                if pending {
                    cx.update(|cx| ctx.request_inspection_sweep(cx));
                }
                cx.update(|cx| ctx.schedule_inspection_sweep(cx));
            },
        );
    }

    fn tick_inspection_sweep(&self, reconciled_snapshot: bool, cx: &mut App) {
        let (connected, hub_visible, entered, reconnected, scope_changed, timer_needs_update) =
            self.hub.update(cx, |hub, cx| {
                let state = self.state.read(cx);
                let connected = state.daemon.is_connected();
                let hub_visible = matches!(state.screen, Screen::Hub { .. });
                let worktrees_visible = matches!(
                    state.screen,
                    Screen::Hub {
                        tab: HubTab::Worktrees
                    }
                );
                let context = effective_context(state).map(|context| &context.id);
                let scope_changed = hub.inspection_sweep_scope.as_ref().is_none_or(|scope| {
                    scope.context.as_ref() != context || scope.repo != state.scope
                });
                if scope_changed {
                    hub.inspection_sweep_scope = Some(InspectionSweepScope {
                        context: effective_context(state).map(|context| context.id.clone()),
                        repo: state.scope.clone(),
                    });
                }
                let entered = worktrees_visible && !hub.inspection_sweep_worktrees_visible;
                let reconnected = connected && !hub.inspection_sweep_connected;
                hub.inspection_sweep_worktrees_visible = worktrees_visible;
                hub.inspection_sweep_connected = connected;
                let timer_needs_update = if connected {
                    hub.inspection_sweep_task.is_none()
                        || hub.inspection_sweep_timer_hub_visible != Some(hub_visible)
                } else {
                    hub.inspection_sweep_task.is_some()
                        || hub.inspection_sweep_timer_hub_visible.is_some()
                };
                (
                    connected,
                    hub_visible,
                    entered,
                    reconnected,
                    scope_changed,
                    timer_needs_update,
                )
            });
        if timer_needs_update {
            self.schedule_inspection_sweep_for(connected, hub_visible, cx);
        }
        if !connected || !(reconciled_snapshot || entered || reconnected || scope_changed) {
            return;
        }

        let ids = self.inspection_sweep_ids(cx);
        let now = Instant::now();
        let should_request = self.hub.update(cx, |hub, _| {
            let mut gained_missing = false;
            if reconciled_snapshot || scope_changed {
                let missing = ids
                    .iter()
                    .filter(|id| !hub.inspections.contains_key(*id))
                    .cloned()
                    .collect::<HashSet<_>>();
                gained_missing = missing
                    .iter()
                    .any(|id| !hub.inspection_sweep_missing.contains(id));
                hub.inspection_sweep_missing = missing;
            }
            let recent = hub.inspection_sweep_finished_at.is_some_and(|finished| {
                now.saturating_duration_since(finished) < INSPECTION_SWEEP_ENTRY_FRESHNESS
            });
            let coalescing_trigger = scope_changed || gained_missing;
            let entry_trigger = (entered || reconnected) && !recent;
            if connected && !ids.is_empty() && (coalescing_trigger || entry_trigger) {
                if hub.inspection_sweep_request.is_some() {
                    if coalescing_trigger {
                        hub.inspection_sweep_pending = true;
                    }
                    false
                } else {
                    true
                }
            } else {
                false
            }
        });
        if should_request {
            self.request_inspection_sweep(cx);
        }
    }

    fn schedule_inspection_sweep(&self, cx: &mut App) {
        let (connected, hub_visible) = {
            let state = self.state.read(cx);
            (
                state.daemon.is_connected(),
                matches!(state.screen, Screen::Hub { .. }),
            )
        };
        self.schedule_inspection_sweep_for(connected, hub_visible, cx);
    }

    fn schedule_inspection_sweep_for(&self, connected: bool, hub_visible: bool, cx: &mut App) {
        let generation = self.hub.update(cx, |hub, _| {
            if !connected {
                hub.inspection_sweep_task = None;
                hub.inspection_sweep_timer_hub_visible = None;
                hub.inspection_sweep_generation = hub.inspection_sweep_generation.wrapping_add(1);
                return None;
            }
            if hub.inspection_sweep_task.is_some()
                && hub.inspection_sweep_timer_hub_visible == Some(hub_visible)
            {
                return None;
            }
            hub.inspection_sweep_task = None;
            hub.inspection_sweep_timer_hub_visible = Some(hub_visible);
            hub.inspection_sweep_generation = hub.inspection_sweep_generation.wrapping_add(1);
            Some(hub.inspection_sweep_generation)
        });
        let Some(generation) = generation else {
            return;
        };
        let interval = if hub_visible {
            INSPECTION_SWEEP_VISIBLE_INTERVAL
        } else {
            INSPECTION_SWEEP_HIDDEN_INTERVAL
        };
        let state = self.state.downgrade();
        let hub = self.hub.downgrade();
        let bridge = self.bridge.clone();
        let rail_scroll = self.rail_scroll.clone();
        let list_scroll = self.list_scroll.clone();
        let pr_scroll = self.pr_scroll.clone();
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(interval).await;
            cx.update(|cx| {
                let (Some(state), Some(hub)) = (state.upgrade(), hub.upgrade()) else {
                    return;
                };
                if hub.read(cx).inspection_sweep_generation != generation {
                    return;
                }
                let ctx = HubCtx {
                    state,
                    hub,
                    bridge,
                    rail_scroll,
                    list_scroll,
                    pr_scroll,
                };
                ctx.hub.update(cx, |hub, _| {
                    hub.inspection_sweep_task = None;
                    hub.inspection_sweep_timer_hub_visible = None;
                });
                ctx.request_inspection_sweep(cx);
                ctx.schedule_inspection_sweep(cx);
            });
        });
        self.hub
            .update(cx, |hub, _| hub.inspection_sweep_task = Some(task));
    }

    /// `I` inspects with a fetch; the debounce inspects without one (§2.6 D-4).
    pub(super) fn inspect(&self, id: WorktreeId, fetch: bool, cx: &mut App) {
        if self.state.read(cx).daemon.is_lost() {
            return;
        }
        let request = self.hub.update(cx, |hub, _| {
            hub.invalidate();
            hub.begin_inspection(id.clone())
        });
        // Same reason as `fetch_pull_requests`: §3.4's dimmed fact list must land before the
        // `git fetch`, not after it.
        self.state.update(cx, |_, cx| cx.notify());
        self.ask(
            RequestBody::InspectWorktrees {
                ids: vec![id.clone()],
                repo: None,
                fetch,
                background: false,
            },
            cx,
            move |result, ctx, cx| {
                ctx.hub
                    .update(cx, |hub, _| hub.apply_inspection(&id, request, result));
                ctx.state.update(cx, |_, cx| cx.notify());
            },
        );
    }

    /// Fetches both tabs, or Mine alone when the daemon serves Reviews boards (§3.5): the
    /// Review tab is the board then, and an old daemon keeps the old list. `force` is `r`;
    /// otherwise the daemon's TTL decides.
    pub(super) fn fetch_pull_requests(&self, force: bool, cx: &mut App) {
        if self.state.read(cx).daemon.is_lost() {
            return;
        }
        let (key, retired) = {
            let state = self.state.read(cx);
            (
                PrCacheKey::from_state(state),
                state.supports_review_boards(),
            )
        };
        let requests = self.hub.update(cx, |hub, _| {
            hub.invalidate();
            hub.prs.set_review_retired(retired);
            hub.prs.begin_fetch(key.clone(), Instant::now())
        });
        // `HubState` is its own entity and the shell only observes `AppState`, so a mutation
        // here repaints nothing on its own. §3.5's `⟳ refreshing` has to appear *now* — the
        // whole point of the indicator is the `gh` round trip it covers.
        self.state.update(cx, |_, cx| cx.notify());
        self.schedule_pr_refresh(cx);
        for (tab, request) in requests {
            self.ask(
                RequestBody::ListPullRequests {
                    repo: key.repo.clone(),
                    context: key.context.clone(),
                    tab,
                    force,
                },
                cx,
                {
                    let key = key.clone();
                    move |result, ctx, cx| ctx.apply_slices(key, tab, request, result, cx)
                },
            );
        }
    }

    pub(super) fn apply_slices(
        &self,
        key: PrCacheKey,
        tab: PrTab,
        request: u64,
        result: Result<ResponseBody, ProtoError>,
        cx: &mut gpui::AsyncApp,
    ) {
        let current_key = cx.update(|cx| PrCacheKey::from_state(self.state.read(cx)));
        if current_key != key {
            return;
        }
        let applied = self.hub.update(cx, |hub, _| {
            if !hub.prs.apply(&key, tab, request, result, Instant::now()) {
                return false;
            }
            hub.invalidate();
            true
        });
        if !applied {
            return;
        }
        let (review, badges) = cx.update(|cx| {
            let hub = self.hub.read(cx);
            let badges = hub
                .prs
                .mine
                .iter()
                .chain(hub.prs.review.iter())
                .flat_map(|slice| slice.prs.iter())
                .map(|pr| {
                    (
                        (pr.repo_id.clone(), pr.head_ref_name.clone()),
                        (
                            pr.number,
                            crate::presentation::pr_badge_state(
                                fleet_core::github::derive_pr_state(
                                    pr.is_draft,
                                    pr.checks,
                                    pr.review_decision,
                                ),
                            ),
                        ),
                    )
                })
                .collect::<Vec<_>>();
            (
                // Once the Review tab is a board its count is the board's, folded in
                // `fold_review_board`; the `gh` list only counts on a daemon without it.
                (!hub.prs.review_retired())
                    .then(|| hub.prs.review.as_ref().map_or(0, |slice| slice.prs.len())),
                badges,
            )
        });
        let scoped_repos: HashSet<RepoId> = cx.update(|cx| {
            self.state
                .read(cx)
                .snapshot
                .as_ref()
                .map(|snapshot| {
                    snapshot
                        .repos
                        .iter()
                        .filter(|repo| {
                            key.repo.as_ref().is_some_and(|wanted| &repo.id == wanted)
                                || (key.repo.is_none()
                                    && key
                                        .context
                                        .as_ref()
                                        .is_none_or(|wanted| &repo.context_id == wanted))
                        })
                        .map(|repo| repo.id.clone())
                        .collect()
                })
                .unwrap_or_default()
        });
        self.state.update(cx, |state, cx| {
            if let Some(review) = review {
                state.review_pr_count = review;
            }
            replace_pr_badges(&mut state.pr_badges, &scoped_repos, badges);
            cx.notify();
        });
        cx.update(|cx| self.schedule_pr_refresh(cx));
    }

    /// Fetches the PR tabs the first time the screen is shown and after the TTL (§3.5).
    pub(super) fn tick_pull_requests(&self, cx: &mut App) {
        let visible_and_connected = {
            let state = self.state.read(cx);
            matches!(state.screen, Screen::Hub { tab: HubTab::Prs }) && state.daemon.is_connected()
        };
        if !visible_and_connected {
            self.hub.update(cx, |hub, _| hub.pr_refresh_task = None);
            return;
        }
        let load_config = self.hub.update(cx, |hub, _| hub.prs.begin_config_fetch());
        if load_config {
            self.ask(RequestBody::GetConfig, cx, |result, ctx, cx| {
                ctx.hub
                    .update(cx, |hub, _| hub.prs.finish_config_fetch(&result));
                cx.update(|cx| ctx.tick_pull_requests(cx));
            });
            return;
        }
        let (key, retired) = {
            let state = self.state.read(cx);
            (
                PrCacheKey::from_state(state),
                state.supports_review_boards(),
            )
        };
        let wanted = self.hub.update(cx, |hub, _| {
            hub.prs.set_review_retired(retired);
            hub.prs.needs_fetch_for(&key, Instant::now())
        });
        if wanted {
            self.fetch_pull_requests(false, cx);
        } else {
            self.schedule_pr_refresh(cx);
        }
    }

    fn schedule_pr_refresh(&self, cx: &mut App) {
        let deadline = self.hub.read(cx).prs.refresh_deadline();
        let generation = self.hub.update(cx, |hub, _| {
            hub.pr_refresh_task = None;
            hub.pr_refresh_generation = hub.pr_refresh_generation.wrapping_add(1);
            hub.pr_refresh_generation
        });
        let Some(deadline) = deadline else {
            return;
        };
        let delay = deadline.saturating_duration_since(Instant::now());
        // The task is stored on the Hub, so it holds the entities weakly: a strong `HubCtx`
        // here would keep the Hub alive through itself for the whole TTL.
        let state = self.state.downgrade();
        let hub = self.hub.downgrade();
        let bridge = self.bridge.clone();
        let rail_scroll = self.rail_scroll.clone();
        let list_scroll = self.list_scroll.clone();
        let pr_scroll = self.pr_scroll.clone();
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(delay).await;
            cx.update(|cx| {
                let (Some(state), Some(hub)) = (state.upgrade(), hub.upgrade()) else {
                    return;
                };
                if hub.read(cx).pr_refresh_generation != generation {
                    return;
                }
                let ctx = HubCtx {
                    state,
                    hub,
                    bridge,
                    rail_scroll,
                    list_scroll,
                    pr_scroll,
                };
                ctx.hub.update(cx, |hub, _| {
                    hub.pr_refresh_task = None;
                    hub.prs.invalidate_config();
                });
                ctx.tick_pull_requests(cx);
            });
        });
        self.hub
            .update(cx, |hub, _| hub.pr_refresh_task = Some(task));
    }

    pub(super) fn synchronize(&self, cx: &mut App) {
        let reconciled_snapshot = self.hub.update(cx, |hub, cx| {
            let state = self.state.read(cx);
            if hub.last_reconciled_revision == state.snapshot_revision {
                return false;
            }
            let worktrees = state
                .snapshot
                .as_ref()
                .map_or(&[][..], |snapshot| snapshot.worktrees.as_slice());
            hub.reconcile_inspection_identities(worktrees);
            hub.last_reconciled_revision = state.snapshot_revision;
            true
        });
        self.tick_inspection_sweep(reconciled_snapshot, cx);
        // Before the visibility check: leaving the Hub leaves the Review tab too, and the
        // chip's count is read wherever the context bar is.
        self.synchronize_review_board(cx);
        let visible = matches!(self.state.read(cx).screen, Screen::Hub { .. });
        if !visible {
            self.hub.update(cx, |hub, _| {
                hub.inspect_task = None;
                hub.inspect_target = None;
                hub.pr_refresh_task = None;
            });
            return;
        }
        let model = self.model(cx);
        self.reconcile_selection(&model, cx);
        let changed = self.hub.update(cx, |hub, _| {
            let changed = !Rc::ptr_eq(&hub.prepared, &model);
            hub.prepared = model.clone();
            changed
        });
        publish_breadcrumb(&self.state, &model, cx);
        self.tick_pull_requests(cx);
        let selected = self.inspection_target(cx);
        if selected != self.hub.read(cx).inspect_target {
            self.schedule_inspection(cx);
        }
        if changed {
            self.state.update(cx, |_, cx| cx.notify());
        }
    }

    pub(super) fn inspection_target(&self, cx: &App) -> Option<WorktreeId> {
        let state = self.state.read(cx);
        if !state.daemon.is_connected()
            || state.hub_pane != HubPane::List
            || !matches!(
                state.screen,
                Screen::Hub {
                    tab: HubTab::Worktrees
                }
            )
        {
            return None;
        }
        self.selected_worktree(cx).map(|row| row.id)
    }

    pub(super) fn schedule_inspection(&self, cx: &mut App) {
        let target = self.inspection_target(cx);
        let generation = self.hub.update(cx, |hub, _| {
            hub.inspect_task = None;
            hub.inspect_target = target.clone();
            hub.inspect_generation = hub.inspect_generation.wrapping_add(1);
            hub.inspect_generation
        });
        let Some(target) = target else {
            return;
        };
        let state = self.state.downgrade();
        let hub = self.hub.downgrade();
        let bridge = self.bridge.clone();
        let rail_scroll = self.rail_scroll.clone();
        let list_scroll = self.list_scroll.clone();
        let pr_scroll = self.pr_scroll.clone();
        // `idle` counts armed debounces (`docs/TESTING-HARNESS.md` §2). The guard is dropped with
        // the task, so a cursor move that supersedes this window counts it down too.
        let mut armed = self.state.read(cx).harness.arm_debounce();
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(AUTO_INSPECT_DEBOUNCE).await;
            armed.disarm();
            cx.update(|cx| {
                let (Some(state), Some(hub)) = (state.upgrade(), hub.upgrade()) else {
                    return;
                };
                if hub.read(cx).inspect_generation != generation {
                    return;
                }
                let ctx = HubCtx {
                    state,
                    hub,
                    bridge,
                    rail_scroll,
                    list_scroll,
                    pr_scroll,
                };
                if ctx.inspection_target(cx).as_ref() == Some(&target) {
                    ctx.inspect(target, false, cx);
                }
            });
        });
        self.hub.update(cx, |hub, _| hub.inspect_task = Some(task));
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    fn response(tab: PrTab) -> Result<ResponseBody, ProtoError> {
        Ok(ResponseBody::PullRequests(vec![PrSlice {
            tab,
            fetched_at: String::new(),
            loading: false,
            error: None,
            total: 0,
            prs: Vec::new(),
        }]))
    }

    #[test]
    fn configured_pr_ttl_schedules_refresh() {
        let mut config = fleet_core::config::default_config("/tmp/fleet");
        config.github.pr_ttl_seconds = 7;
        let mut cache = PrCache::default();
        cache.finish_config_fetch(&Ok(ResponseBody::Config(config)));
        let now = Instant::now();
        let key = PrCacheKey {
            repo: None,
            context: None,
            generation: 1,
        };
        let requests = cache.begin_fetch(key.clone(), now);
        assert!(cache.apply(&key, PrTab::Mine, requests[0].1, response(PrTab::Mine), now));
        assert!(cache.apply(
            &key,
            PrTab::Review,
            requests[1].1,
            response(PrTab::Review),
            now
        ));

        assert_eq!(cache.refresh_deadline(), Some(now + Duration::from_secs(7)));
        assert!(!cache.needs_fetch(now + Duration::from_secs(6)));
        assert!(cache.needs_fetch(now + Duration::from_secs(7)));
    }

    /// The stored refresh task used to hold a strong `HubCtx`, so the Hub owned itself for a
    /// whole TTL; it must hold the entities weakly, as `schedule_inspection` does.
    #[gpui::test]
    fn a_pending_pr_refresh_does_not_keep_the_hub_alive(cx: &mut gpui::TestAppContext) {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet-pr-refresh", now);
        state.daemon = crate::state::DaemonLink::Connected;
        state.screen = Screen::Hub { tab: HubTab::Prs };
        let state = cx.new(|_| state);
        let (ctx, _harness) = crate::screens::hub::tests::test_hub_ctx_for(state.clone(), cx);
        ctx.hub.update(cx, |hub, _| {
            let key = PrCacheKey {
                repo: None,
                context: None,
                generation: 0,
            };
            let requests = hub.prs.begin_fetch(key.clone(), now);
            assert!(
                hub.prs
                    .apply(&key, PrTab::Mine, requests[0].1, response(PrTab::Mine), now)
            );
            assert!(hub.prs.apply(
                &key,
                PrTab::Review,
                requests[1].1,
                response(PrTab::Review),
                now
            ));
            assert!(hub.prs.refresh_deadline().is_some());
        });

        cx.update(|cx| ctx.schedule_pr_refresh(cx));

        let hub = ctx.hub.downgrade();
        drop(ctx);
        drop(state);
        cx.update(|_| {});

        assert!(
            hub.upgrade().is_none(),
            "the pending PR refresh must not own the Hub it is stored on"
        );
    }

    #[gpui::test]
    fn a_pending_inspection_sweep_does_not_keep_the_hub_alive(cx: &mut gpui::TestAppContext) {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet-inspection-sweep", now);
        state.daemon = crate::state::DaemonLink::Connected;
        state.screen = Screen::Hub { tab: HubTab::Prs };
        let state = cx.new(|_| state);
        let (ctx, _harness) = crate::screens::hub::tests::test_hub_ctx_for(state.clone(), cx);

        cx.update(|cx| ctx.schedule_inspection_sweep(cx));
        assert!(
            ctx.hub
                .read_with(cx, |hub, _| hub.inspection_sweep_task.is_some())
        );

        let hub = ctx.hub.downgrade();
        drop(ctx);
        drop(state);
        cx.update(|_| {});

        assert!(
            hub.upgrade().is_none(),
            "the pending inspection sweep must not own the Hub it is stored on"
        );
    }

    #[test]
    fn configured_pr_ttl_has_one_second_floor() {
        for configured in [0, -1] {
            let mut config = fleet_core::config::default_config("/tmp/fleet");
            config.github.pr_ttl_seconds = configured;
            let effective = crate::bridge::EffectiveConfig::from_config(&config);
            let freshness = PrFreshness::from_effective(&effective);

            assert_eq!(freshness.duration(), Duration::from_secs(1));
        }
    }
}
