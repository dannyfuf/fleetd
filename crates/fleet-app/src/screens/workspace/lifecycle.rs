use super::*;
use crate::screens::hub::PrFreshness;

fn pr_lookup_pending(requested: &[RepoId], repo: &RepoId) -> bool {
    requested.contains(repo)
}

fn expire_pr_lookup(requested: &mut Vec<RepoId>, repo: &RepoId) {
    requested.retain(|candidate| candidate != repo);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrLookupOutcome {
    Succeeded { at: Instant },
    Failed { at: Instant },
}

impl PrLookupOutcome {
    const fn succeeded(self) -> bool {
        matches!(self, Self::Succeeded { .. })
    }
}

fn pr_lookup_expiry(outcome: PrLookupOutcome, freshness: PrFreshness) -> Instant {
    let at = match outcome {
        PrLookupOutcome::Succeeded { at } | PrLookupOutcome::Failed { at } => at,
    };
    at + freshness.duration()
}

fn apply_pr_lookup_result(
    app: &mut AppState,
    repo: &RepoId,
    result: Result<ResponseBody, fleet_proto::error::ProtoError>,
    at: Instant,
) -> PrLookupOutcome {
    let Ok(ResponseBody::PullRequests(slices)) = result else {
        return PrLookupOutcome::Failed { at };
    };
    let replacement = slices
        .into_iter()
        .flat_map(|slice| slice.prs)
        .map(|pr| {
            (
                (pr.repo_id, pr.head_ref_name),
                (
                    pr.number,
                    badge_state(pr.is_draft, pr.checks, pr.review_decision),
                ),
            )
        })
        .collect::<Vec<_>>();
    app.pr_badges
        .retain(|(cached_repo, _), _| cached_repo != repo);
    app.pr_badges.extend(replacement);
    PrLookupOutcome::Succeeded { at }
}

impl WorkspaceScreen {
    /// Attaches, detaches and flushes so the daemon always mirrors what is on screen.
    pub(super) fn reconcile(
        &self,
        model: &Model,
        bridge: &Bridge,
        state: &Entity<AppState>,
        cell: Size<Pixels>,
        cx: &mut App,
    ) {
        let mut local = self.local.borrow_mut();
        local.wheel.reconcile(model.terminal);
        // A new link means the daemon forgot every attachment, so the terminal on screen has
        // to be claimed again even though it did not change.
        let relinked = local.attached_generation != model.link_generation;
        // A Fleet-drawn tab has no PTY on the daemon side: attaching to it would answer
        // `NotFound`, and detaching from the previous tab still has to happen.
        let target = model.attach_target();
        let terminal_changed = local.attached != target;
        if terminal_changed || relinked {
            local.pending.clear();
            local.anchor = None;
            local.anchor_history_epoch = None;
            local.history.clear();
            local.row_caches.clear();
            local.state.pending_selection_scroll = None;
            if terminal_changed {
                local.mouse_selection = None;
            }
            let size = target.map_or(FALLBACK_GRID, |terminal| local.size_for(terminal, cell));
            if let Some(terminal) = target {
                local.sizes.insert(terminal, size);
            }
            drop(local);
            reconcile_attachment(
                &self.local,
                AttachmentSpec {
                    target,
                    generation: model.link_generation,
                    preserve: model.popup_terminal,
                    size,
                },
                bridge,
                state,
                cx,
            );
            local = self.local.borrow_mut();
        }

        // Both views deliberately keep the shared terminal attached. While the popup is visible
        // it owns the PTY dimensions; when it disappears, restore the Workspace's cached size
        // even though this view's bounds and terminal id did not change.
        if local.state.popup_owned_size
            && !model.popup_owns_terminal
            && let Some(terminal) = target
            && let Some(&(cols, rows)) = local.sizes.get(&terminal)
        {
            bridge.send(RequestBody::ResizeTerminal {
                terminal,
                cols,
                rows,
            });
        }
        local.state.popup_owned_size = model.popup_owns_terminal;

        // Primary and alternate screens do not share coordinates, and changing the column count
        // changes the meaning of a cell address. The daemon advances `history_epoch` whenever
        // retained absolute row identities may have rebased.
        if model.primed {
            if local.mouse_selection.is_some_and(|selection| {
                selection.alt_screen != model.alt_screen || Some(selection.cols) != model.grid_cols
            }) {
                local.mouse_selection = None;
                local.row_caches.clear();
            }
            invalidate_history_epoch(&mut local, model.history_epoch);
            if local.row_caches.values().any(|cache| {
                Some(cache.cols) != model.grid_cols || cache.alt_screen != model.alt_screen
            }) {
                local.row_caches.clear();
            }
        }

        // §3.6 "Attaching": input produced before the first frame is flushed in order once the
        // mirror is primed, and dropped if the terminal went away in the meantime.
        if model.primed && !local.pending.is_empty() {
            match target {
                Some(terminal) => {
                    for request in drain_pending_requests(&mut local.pending, terminal) {
                        bridge.send(request);
                    }
                }
                None => local.pending.clear(),
            }
        }
    }

    /// Retains only rows covered by the active selection.
    pub(super) fn cache_viewport(&self, state: &Entity<AppState>, cx: &App) {
        let app = state.read(cx);
        if let Some(terminal) = app
            .active_session()
            .and_then(|session| session.active_terminal)
            && let Some(grid) = app.grids.get(&terminal).filter(|grid| grid.primed)
        {
            self.local.borrow_mut().cache_viewport(terminal, grid, true);
        }
    }

    /// Keeps the keyboard selection inside the viewport and retains its history.
    pub(super) fn track_selection(&self, state: &Entity<AppState>, cx: &App) {
        let app = state.read(cx);
        if let Some(session) = app.active_session()
            && let Some(terminal) = session.active_terminal
            && let Some(grid) = app.grids.get(&terminal)
        {
            let mut local = self.local.borrow_mut();
            if let Some(lines) = local
                .state
                .pending_selection_scroll
                .as_mut()
                .and_then(|pending| pending.reconcile(terminal, grid))
            {
                local.state.pending_selection_scroll = None;
                local.shift_caret(lines);
            }
            local.track_selection(grid, true);
        }
    }

    /// Starts the 400 ms timer that reveals the prefix hint, once per prefix (§3.6).
    pub(super) fn arm_prefix_hint(&self, model: &Model, state: &Entity<AppState>, cx: &mut App) {
        self.local
            .borrow_mut()
            .hint
            .reconcile(model.mode == TerminalMode::Prefix, state, cx);
    }

    /// Fills the shared branch → pull-request cache once per repository when Hub has not.
    pub(super) fn lookup_pr(
        &self,
        model: &Model,
        bridge: &Bridge,
        state: &Entity<AppState>,
        cx: &mut App,
    ) {
        let (Some(repo), Some(_)) = (model.repo.clone(), model.branch_key.as_ref()) else {
            return;
        };
        if pr_lookup_pending(&self.local.borrow().state.pr_requested, &repo) {
            return;
        }
        self.local
            .borrow_mut()
            .state
            .pr_requested
            .push(repo.clone());
        let config_reply = bridge.request(RequestBody::GetConfig);
        let bridge = bridge.clone();
        let state = state.downgrade();
        let local = Rc::clone(&self.local);
        let task_repo = repo.clone();
        let task = cx.spawn(async move |cx| {
            let freshness = match config_reply.recv().await {
                Ok(Ok(ResponseBody::Config(config))) => {
                    let effective = crate::bridge::EffectiveConfig::from_config(&config);
                    crate::screens::hub::PrFreshness::from_effective(&effective)
                }
                _ => crate::screens::hub::PrFreshness::default(),
            };
            let reply = bridge.request(RequestBody::ListPullRequests {
                repo: Some(task_repo.clone()),
                context: None,
                tab: PrTab::Mine,
                force: false,
            });
            let result = reply.recv().await;
            let completed_at = Instant::now();
            let outcome = match result {
                Ok(result) => state
                    .update(cx, |app, cx| {
                        let outcome = apply_pr_lookup_result(app, &task_repo, result, completed_at);
                        if outcome.succeeded() {
                            cx.notify();
                        }
                        outcome
                    })
                    .unwrap_or(PrLookupOutcome::Failed { at: completed_at }),
                Err(_) => PrLookupOutcome::Failed { at: completed_at },
            };
            let retry_at = pr_lookup_expiry(outcome, freshness);
            cx.background_executor()
                .timer(retry_at.saturating_duration_since(Instant::now()))
                .await;
            expire_pr_lookup(&mut local.borrow_mut().state.pr_requested, &task_repo);
            let _ = state.update(cx, |_, cx| cx.notify());
        });
        self.local.borrow_mut().state.pr_tasks.insert(repo, task);
    }

    /// Reconcile state before rendering, including hidden-surface teardown.
    pub(crate) fn synchronize(
        &mut self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        window: &mut Window,
        cx: &mut App,
    ) {
        if !matches!(state.read(cx).screen, Screen::Workspace { .. }) {
            let preserve = state
                .read(cx)
                .agent_popup_session()
                .and_then(|session| session.terminals.first())
                .map(|terminal| terminal.id);
            let mut local = self.local.borrow_mut();
            local.detach(bridge, preserve);
            local.pending.clear();
            local.clear_selections();
            local.state.pending_selection_scroll = None;
            local.hint.clear();
            local.wheel.reconcile(None);
            local.state.pane_focused = false;
            drop(local);
            for pane in self.panes.values() {
                pane.view
                    .update(cx, |pane, cx| pane.set_active(false, window, cx));
            }
            self.model = None;
            return;
        }

        self.local.borrow_mut().state.pane_focused = false;
        let Some(model) = state
            .read(cx)
            .active_session()
            .map(|session| Model::build(state.read(cx), session))
        else {
            self.local.borrow_mut().clear_selections();
            self.model = None;
            return;
        };
        let cell = cell_size(cx.theme());
        if let Some(snapshot) = &state.read(cx).snapshot {
            let mut local = self.local.borrow_mut();
            local.sizes.retain(|id, _| {
                snapshot
                    .sessions
                    .iter()
                    .any(|session| session.terminals.iter().any(|terminal| terminal.id == *id))
            });
            local
                .state
                .pr_requested
                .retain(|id| snapshot.repos.iter().any(|repo| repo.id == *id));
            local
                .state
                .pr_tasks
                .retain(|id, _| snapshot.repos.iter().any(|repo| repo.id == *id));
        }

        self.local.borrow_mut().padding = Some(cx.theme().space.sm);
        self.reconcile(&model, bridge, state, cell, cx);
        self.cache_viewport(state, cx);
        self.track_selection(state, cx);
        self.arm_prefix_hint(&model, state, cx);
        self.lookup_pr(&model, bridge, state, cx);
        self.sync_panes(&model, bridge, state, window, cx);
        self.sync_agent_views(&model, bridge, state, window, cx);

        if let Some(grid) = state.read(cx).active_grid().filter(|grid| grid.primed) {
            self.local
                .borrow_mut()
                .presentation
                .update(model.terminal, grid, cx.theme());
        }
        self.model = Some(model);
    }
}

/// Drops every selection and cached row whose absolute line identities the daemon may have
/// rebased, which is what a new `history_epoch` announces.
pub(super) fn invalidate_history_epoch(local: &mut Local, current_epoch: Option<u64>) -> bool {
    let mut changed = false;
    if local
        .mouse_selection
        .is_some_and(|selection| Some(selection.history_epoch) != current_epoch)
    {
        local.mouse_selection = None;
        changed = true;
    }
    if local.anchor.is_some() && local.anchor_history_epoch != current_epoch {
        local.anchor = None;
        local.anchor_history_epoch = None;
        local.history.clear();
        changed = true;
    }
    if local
        .row_caches
        .values()
        .any(|cache| Some(cache.history_epoch) != current_epoch)
    {
        local.row_caches.clear();
        changed = true;
    }
    changed
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn failed_and_stale_pr_lookup_retries() {
        let repo: RepoId = "owner/repo"
            .parse()
            .unwrap_or_else(|error| panic!("{error}"));
        let mut config = fleet_core::config::default_config("/tmp/fleet");
        config.github.pr_ttl_seconds = 7;
        let effective = crate::bridge::EffectiveConfig::from_config(&config);
        let freshness = PrFreshness::from_effective(&effective);
        let now = Instant::now();
        let failed = PrLookupOutcome::Failed { at: now };
        let succeeded = PrLookupOutcome::Succeeded { at: now };

        assert!(now + Duration::from_secs(6) < pr_lookup_expiry(failed, freshness));
        assert!(now + Duration::from_secs(7) >= pr_lookup_expiry(failed, freshness));
        assert!(now + Duration::from_secs(6) < pr_lookup_expiry(succeeded, freshness));
        assert!(now + Duration::from_secs(7) >= pr_lookup_expiry(succeeded, freshness));

        let mut app = AppState::new("/tmp/fleet", now);
        app.pr_badges.insert(
            (repo.clone(), "main".to_owned()),
            (17, PrBadgeState::Approved),
        );
        let stale_badges = app.pr_badges.clone();
        let outcome = apply_pr_lookup_result(
            &mut app,
            &repo,
            Err(fleet_proto::error::ProtoError {
                kind: fleet_proto::error::ErrorKind::Unknown,
                message: "GitHub unavailable".to_owned(),
            }),
            now,
        );

        assert_eq!(outcome, failed);
        assert_eq!(app.pr_badges, stale_badges);
    }
}
