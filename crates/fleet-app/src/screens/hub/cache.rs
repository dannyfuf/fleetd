use super::*;

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

impl HubCtx {
    /// `I` inspects with a fetch; the debounce inspects without one (§2.6 D-4).
    pub(super) fn inspect(&self, id: WorktreeId, fetch: bool, cx: &mut App) {
        if self.state.read(cx).daemon.is_lost() {
            return;
        }
        self.hub.update(cx, |hub, _| {
            hub.invalidate();
            let slot = hub.inspections.entry(id.clone()).or_default();
            slot.loading = true;
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
                ctx.hub.update(cx, |hub, _| {
                    hub.invalidate();
                    match result {
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
                    }
                });
                ctx.state.update(cx, |_, cx| cx.notify());
            },
        );
    }

    /// Fetches both tabs. `force` is `r`; otherwise the daemon's TTL decides.
    pub(super) fn fetch_pull_requests(&self, force: bool, cx: &mut App) {
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
            hub.invalidate();
            hub.prs.loading = true;
            hub.prs.started_at = Some(Instant::now());
            hub.prs.error = None;
        });
        // `HubState` is its own entity and the shell only observes `AppState`, so a mutation
        // here repaints nothing on its own. §3.5's `⟳ refreshing` has to appear *now* — the
        // whole point of the indicator is the `gh` round trip it covers.
        self.state.update(cx, |_, cx| cx.notify());
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

    pub(super) fn apply_slices(
        &self,
        tab: PrTab,
        result: Result<ResponseBody, ProtoError>,
        cx: &mut gpui::AsyncApp,
    ) {
        let (review, badges) = self.hub.update(cx, |hub, _| {
            hub.invalidate();
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
        self.state.update(cx, |state, cx| {
            state.review_pr_count = review;
            state.pr_badges.extend(badges);
            cx.notify();
        });
    }

    /// Fetches the PR tabs the first time the screen is shown and after the TTL (§3.5).
    pub(super) fn tick_pull_requests(&self, cx: &mut App) {
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

    pub(super) fn synchronize(&self, cx: &mut App) {
        let visible = matches!(self.state.read(cx).screen, Screen::Hub { .. });
        if !visible {
            self.hub.update(cx, |hub, _| {
                hub.inspect_task = None;
                hub.inspect_target = None;
            });
            return;
        }
        let model = self.model(cx);
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
