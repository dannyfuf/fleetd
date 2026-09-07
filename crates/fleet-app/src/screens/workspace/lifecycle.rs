use super::*;

impl WorkspaceScreen {
    /// Attaches, detaches and flushes so the daemon always mirrors what is on screen.
    pub(super) fn reconcile(&self, model: &Model, bridge: &Bridge, cell: Size<Pixels>) {
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
            if terminal_changed {
                local.mouse_selection = None;
            }
            let size = target.map_or(FALLBACK_GRID, |terminal| local.size_for(terminal, cell));
            if let Some(terminal) = target {
                local.sizes.insert(terminal, size);
            }
            local.attach(
                target,
                model.link_generation,
                model.popup_terminal,
                size,
                bridge,
            );
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
        if let Some(grid) = state.read(cx).active_grid() {
            self.local.borrow_mut().track_selection(grid, true);
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
        let (Some(repo), Some(branch)) = (model.repo.clone(), model.branch_key.clone()) else {
            return;
        };
        {
            let local = self.local.borrow();
            if state
                .read(cx)
                .pr_badges
                .contains_key(&(repo.clone(), branch.clone()))
                || local.state.pr_requested.contains(&repo)
            {
                return;
            }
        }
        self.local
            .borrow_mut()
            .state
            .pr_requested
            .push(repo.clone());
        let reply = bridge.request(RequestBody::ListPullRequests {
            repo: Some(repo.clone()),
            context: None,
            tab: PrTab::Mine,
            force: false,
        });
        let state = state.downgrade();
        let task = cx.spawn(async move |cx| {
            let Ok(Ok(ResponseBody::PullRequests(slices))) = reply.recv().await else {
                return;
            };
            let _ = state.update(cx, |app, cx| {
                for slice in slices {
                    for pr in slice.prs {
                        app.pr_badges.insert(
                            (pr.repo_id, pr.head_ref_name),
                            (
                                pr.number,
                                badge_state(pr.is_draft, pr.checks, pr.review_decision),
                            ),
                        );
                    }
                }
                cx.notify();
            });
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
        self.reconcile(&model, bridge, cell);
        self.cache_viewport(state, cx);
        self.track_selection(state, cx);
        self.arm_prefix_hint(&model, state, cx);
        self.lookup_pr(&model, bridge, state, cx);
        self.sync_panes(&model, bridge, state, window, cx);

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
