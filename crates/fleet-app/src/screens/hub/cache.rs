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
            || self.review_fetch.needs_fetch(now, self.freshness)
    }

    fn refresh_deadline(&self) -> Option<Instant> {
        [
            self.mine_fetch.deadline(self.freshness),
            self.review_fetch.deadline(self.freshness),
        ]
        .into_iter()
        .flatten()
        .min()
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

    pub(super) fn begin_fetch(&mut self, key: PrCacheKey, now: Instant) -> [(PrTab, u64); 2] {
        if self.key.as_ref() != Some(&key) {
            self.mine = None;
            self.review = None;
            self.mine_fetch = PrTabFetch::default();
            self.review_fetch = PrTabFetch::default();
            self.key = Some(key);
        }
        let mut requests = [(PrTab::Mine, 0), (PrTab::Review, 0)];
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

pub(super) fn replace_pr_badges(
    badges: &mut HashMap<(RepoId, String), (u64, fleet_ui_kit::PrBadgeState)>,
    repos: &HashSet<RepoId>,
    replacement: impl IntoIterator<Item = ((RepoId, String), (u64, fleet_ui_kit::PrBadgeState))>,
) {
    badges.retain(|(repo, _), _| !repos.contains(repo));
    badges.extend(replacement);
}

impl HubCtx {
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
            },
            cx,
            move |result, ctx, cx| {
                ctx.hub
                    .update(cx, |hub, _| hub.apply_inspection(&id, request, result));
                ctx.state.update(cx, |_, cx| cx.notify());
            },
        );
    }

    /// Fetches both tabs. `force` is `r`; otherwise the daemon's TTL decides.
    pub(super) fn fetch_pull_requests(&self, force: bool, cx: &mut App) {
        if self.state.read(cx).daemon.is_lost() {
            return;
        }
        let key = {
            let state = self.state.read(cx);
            PrCacheKey::from_state(state)
        };
        let requests = self.hub.update(cx, |hub, _| {
            hub.invalidate();
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
                hub.prs.review.as_ref().map_or(0, |slice| slice.prs.len()),
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
            state.review_pr_count = review;
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
        let wanted = {
            let state = self.state.read(cx);
            let key = PrCacheKey::from_state(state);
            self.hub.read(cx).prs.needs_fetch_for(&key, Instant::now())
        };
        if wanted {
            self.fetch_pull_requests(false, cx);
        } else {
            self.schedule_pr_refresh(cx);
        }
    }

    fn schedule_pr_refresh(&self, cx: &mut App) {
        let deadline = self.hub.read(cx).prs.refresh_deadline();
        self.hub.update(cx, |hub, _| hub.pr_refresh_task = None);
        let Some(deadline) = deadline else {
            return;
        };
        let delay = deadline.saturating_duration_since(Instant::now());
        let ctx = self.clone();
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(delay).await;
            cx.update(|cx| {
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
        let visible = matches!(self.state.read(cx).screen, Screen::Hub { .. });
        if !visible {
            self.hub.update(cx, |hub, _| {
                hub.inspect_task = None;
                hub.inspect_target = None;
                hub.pr_refresh_task = None;
            });
            return;
        }
        self.hub.update(cx, |hub, cx| {
            let state = self.state.read(cx);
            if hub.last_reconciled_revision == state.snapshot_revision {
                return;
            }
            let worktrees = state
                .snapshot
                .as_ref()
                .map_or(&[][..], |snapshot| snapshot.worktrees.as_slice());
            hub.reconcile_inspection_identities(worktrees);
            hub.last_reconciled_revision = state.snapshot_revision;
        });
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
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(AUTO_INSPECT_DEBOUNCE).await;
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
