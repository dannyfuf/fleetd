use super::*;

/// The snapshot's contexts, or an empty slice before the first snapshot lands.
fn contexts(state: &AppState) -> &[fleet_core::model::Context] {
    state
        .snapshot
        .as_ref()
        .map_or(&[], |snapshot| snapshot.contexts.as_slice())
}

pub(super) fn reconcile_index<Row, Key: Clone + PartialEq>(
    rows: &[Row],
    current: usize,
    anchor: &mut Option<Key>,
    key: impl Fn(&Row) -> Key,
) -> usize {
    if rows.is_empty() {
        return 0;
    }
    if let Some(selected) = anchor.as_ref()
        && let Some(index) = rows.iter().position(|row| key(row) == *selected)
    {
        return index;
    }
    let index = current.min(rows.len() - 1);
    *anchor = Some(key(&rows[index]));
    index
}

pub(super) fn pr_navigation_matches(
    screen: &Screen,
    selected: Option<&PrIdentity>,
    requested: &PrIdentity,
) -> bool {
    matches!(screen, Screen::Hub { tab: HubTab::Prs }) && selected == Some(requested)
}

impl HubCtx {
    /// Wraps a method as a gpui action listener, cloning the context into it.
    pub(super) fn act<A: gpui::Action>(
        &self,
        handler: impl Fn(&Self, &A, &mut Window, &mut App) + 'static,
    ) -> impl Fn(&A, &mut Window, &mut App) + 'static {
        let ctx = self.clone();
        move |action, window, cx| handler(&ctx, action, window, cx)
    }

    /// Sends a request and applies its single answer on the foreground executor.
    pub(super) fn ask(
        &self,
        body: RequestBody,
        cx: &mut App,
        apply: impl FnOnce(Result<ResponseBody, ProtoError>, &Self, &mut gpui::AsyncApp) + 'static,
    ) {
        let reply = self.bridge.request(body);
        let ctx = self.clone();
        cx.spawn(async move |cx| {
            let result = reply
                .recv()
                .await
                .unwrap_or_else(|_| Err(client_error("the Fleet daemon reply channel closed")));
            apply(result, &ctx, cx);
        })
        .detach();
    }

    /// Records a toast under the §2.7 law.
    pub(super) fn toast(
        &self,
        text: impl Into<SharedString>,
        icon: Icon,
        short: bool,
        cx: &mut App,
    ) {
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
    pub(super) fn refuses(&self, cx: &mut App) -> bool {
        let refuses = self.state.read(cx).refuses_mutations();
        if refuses {
            self.toast("fleetd is not reachable", Icon::Unplug, true, cx);
        }
        refuses
    }

    /// The current model, rebuilt from the same functions the renderer uses.
    pub(super) fn model(&self, cx: &App) -> Rc<HubModel> {
        projection::prepare(self.state.read(cx), self.hub.read(cx), now_unix())
    }

    /// Which list the cursor keys address right now.
    pub(super) fn cursor_len(&self, model: &HubModel, cx: &App) -> usize {
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

    pub(super) fn cursor_index(&self, cx: &App) -> usize {
        let state = self.state.read(cx);
        match (state.hub_pane, &state.screen) {
            (HubPane::Repos, _) => state.cursors.repos,
            (
                HubPane::List,
                Screen::Hub {
                    tab: HubTab::Worktrees,
                },
            ) => state.cursors.worktrees,
            (HubPane::List, _) => pr_cursor(state),
        }
    }

    pub(super) fn set_cursor(&self, index: usize, len: usize, moving_down: bool, cx: &mut App) {
        let model = self.model(cx);
        let (pane, screen, tab, handle) = {
            let state = self.state.read(cx);
            let handle = match (state.hub_pane, &state.screen) {
                (HubPane::Repos, _) => self.rail_scroll.clone(),
                (
                    HubPane::List,
                    Screen::Hub {
                        tab: HubTab::Worktrees,
                    },
                ) => self.list_scroll.clone(),
                (HubPane::List, _) => self.pr_scroll.clone(),
            };
            (state.hub_pane, state.screen.clone(), state.pr_tab, handle)
        };
        self.hub.update(cx, |hub, _| match (pane, &screen) {
            (HubPane::Repos, _) => {
                hub.selection.rail = model.rail.get(index).map(|row| row.repo.clone());
            }
            (
                HubPane::List,
                Screen::Hub {
                    tab: HubTab::Worktrees,
                },
            ) => {
                hub.selection.worktree = model.worktrees.get(index).map(|row| row.id.clone());
            }
            (HubPane::List, _) => {
                let selected = model
                    .prs
                    .get(index)
                    .map(|row| (row.repo.clone(), row.number));
                match tab {
                    PrTab::Mine => hub.selection.prs_mine = selected,
                    PrTab::Review => hub.selection.prs_review = selected,
                }
            }
        });
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

    pub(super) fn reconcile_selection(&self, model: &HubModel, cx: &mut App) -> bool {
        let mut selection = self.hub.read(cx).selection.clone();
        let mut cursors = self.state.read(cx).cursors.clone();
        cursors.repos = reconcile_index(&model.rail, cursors.repos, &mut selection.rail, |row| {
            row.repo.clone()
        });
        match &self.state.read(cx).screen {
            Screen::Hub {
                tab: HubTab::Worktrees,
            } => {
                cursors.worktrees = reconcile_index(
                    &model.worktrees,
                    cursors.worktrees,
                    &mut selection.worktree,
                    |row| row.id.clone(),
                );
            }
            Screen::Hub { tab: HubTab::Prs } => match self.state.read(cx).pr_tab {
                PrTab::Mine => {
                    cursors.prs_mine = reconcile_index(
                        &model.prs,
                        cursors.prs_mine,
                        &mut selection.prs_mine,
                        |row| (row.repo.clone(), row.number),
                    );
                }
                PrTab::Review => {
                    cursors.prs_review = reconcile_index(
                        &model.prs,
                        cursors.prs_review,
                        &mut selection.prs_review,
                        |row| (row.repo.clone(), row.number),
                    );
                }
            },
            _ => {}
        }
        let displayed = model.displayed();
        let changed = {
            let state = self.state.read(cx);
            cursors != state.cursors || displayed != state.displayed_hub
        };
        self.hub.update(cx, |hub, _| hub.selection = selection);
        if changed {
            self.state.update(cx, |state, cx| {
                state.cursors = cursors;
                state.displayed_hub = displayed;
                cx.notify();
            });
        }
        changed
    }

    pub(super) fn pr_navigation_is_current(&self, key: &PrIdentity, cx: &App) -> bool {
        let state = self.state.read(cx);
        let selected = self
            .selected_pr(cx)
            .map(|row| (row.repo.clone(), row.number));
        pr_navigation_matches(&state.screen, selected.as_ref(), key)
    }

    pub(super) fn move_by(&self, delta: isize, _window: &mut Window, cx: &mut App) {
        let model = self.model(cx);
        let len = self.cursor_len(&model, cx);
        let next = move_cursor(self.cursor_index(cx), delta, len);
        self.set_cursor(next, len, delta > 0, cx);
    }

    pub(super) fn move_to_end(&self, bottom: bool, _window: &mut Window, cx: &mut App) {
        let model = self.model(cx);
        let len = self.cursor_len(&model, cx);
        let next = if bottom { len.saturating_sub(1) } else { 0 };
        self.set_cursor(next, len, bottom, cx);
    }

    pub(super) fn half_page(&self, sign: isize, window: &mut Window, cx: &mut App) {
        let height = f32::from(window.viewport_size().height);
        let rows = crate::state::half_page(visible_rows(height, cx));
        self.move_by(sign * rows, window, cx);
    }

    pub(super) fn select_context(&self, digit: usize, cx: &mut App) {
        let state = self.state.read(cx);
        let id = hub_context_bar::context_for_digit(contexts(state), digit).cloned();
        if let Some(id) = id {
            self.activate_context(id, cx);
        }
    }

    pub(super) fn cycle_context(&self, delta: isize, cx: &mut App) {
        let state = self.state.read(cx);
        let id = hub_context_bar::cycle(contexts(state), state.active_context(), delta).cloned();
        if let Some(id) = id {
            self.activate_context(id, cx);
        }
    }

    /// Switching context is an explicit user action, so it is allowed to reset the scope and
    /// the cursors — background events never are (§3.3 cursor stability).
    pub(super) fn activate_context(&self, id: ContextId, cx: &mut App) {
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
            hub.selection = SelectionAnchors::default();
            hub.invalidate();
        });
    }

    pub(super) fn delete_context(&self, cx: &mut App) {
        let Some(id) = self.state.read(cx).active_context().cloned() else {
            return;
        };
        let Some(snapshot) = self.state.read(cx).snapshot.as_ref() else {
            return;
        };
        let name = snapshot
            .contexts
            .iter()
            .find(|context| context.id == id)
            .map_or_else(|| id.to_string(), |context| context.name.clone());
        let repos: Vec<_> = snapshot
            .repos
            .iter()
            .filter(|repo| repo.context_id == id)
            .map(|repo| repo.id.clone())
            .collect();
        let worktrees: Vec<_> = snapshot
            .worktrees
            .iter()
            .filter(|worktree| repos.contains(&worktree.repo_id))
            .map(|worktree| worktree.id.clone())
            .collect();
        let sessions = snapshot
            .sessions
            .iter()
            .filter(|session| {
                matches!(
                    &session.kind,
                    fleet_core::sessions::SessionKind::Worktree(id) if worktrees.contains(id)
                )
            })
            .count();
        self.prepare_confirm(
            dialogs::ConfirmRequest::DeleteContext {
                context: id,
                name,
                repos: repos.len(),
                worktrees: worktrees.len(),
                sessions,
            },
            cx,
        );
    }

    pub(super) fn go_all_repos(&self, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.scope = RepoScope::All;
            state.cursors.repos = 0;
            state.cursors.worktrees = 0;
            cx.notify();
        });
        self.hub.update(cx, |hub, _| {
            hub.selection.rail = None;
            hub.selection.worktree = None;
        });
    }

    pub(super) fn selected_rail_row(&self, cx: &App) -> Option<RailRow> {
        let model = self.model(cx);
        model.rail.get(self.state.read(cx).cursors.repos).cloned()
    }

    pub(super) fn selected_worktree(&self, cx: &App) -> Option<WorktreeRow> {
        let model = self.model(cx);
        model
            .worktrees
            .get(self.state.read(cx).cursors.worktrees)
            .cloned()
    }

    pub(super) fn selected_pr(&self, cx: &App) -> Option<PrRow> {
        let model = self.model(cx);
        model.prs.get(pr_cursor(self.state.read(cx))).cloned()
    }

    /// The repository the list is scoped to, or the one under the cursor.
    pub(super) fn scoped_repo(&self, cx: &App) -> Option<RepoId> {
        match &self.state.read(cx).scope {
            RepoScope::Repo(repo) => Some(repo.clone()),
            RepoScope::All => self.selected_worktree(cx).map(|row| row.repo),
        }
    }
}
