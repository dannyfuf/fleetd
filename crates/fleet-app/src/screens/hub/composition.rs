use super::*;

impl HubScreen {
    /// Renders the Hub into the frame's body.
    pub fn render(
        &mut self,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.bind(state, bridge, cx);
        let ctx = self.context(state, bridge);
        let on_board = matches!(state.read(cx).screen, Screen::Hub { tab: HubTab::Board });
        // The board owns the whole body and its own keys, so the Hub's panes are not even
        // composed while it is up (BOARD §8).
        let body = if on_board {
            self.board.render(state, bridge, focus, window, cx)
        } else {
            let now = now_unix();
            let viewport = window.viewport_size();
            let width = f32::from(viewport.width);
            let rows = visible_rows(f32::from(viewport.height), cx);
            let model = self.hub.read(cx).prepared.clone();
            self.body(
                state.read(cx),
                self.hub.read(cx),
                &model,
                Viewport { width, rows },
                now,
                cx,
            )
        };
        let tabs = hub_tabs(state.read(cx));

        div()
            .when(!on_board, |el| el.track_focus(focus))
            .size_full()
            .flex()
            .flex_col()
            .child(div().px(cx.theme().space.md).child(tabs))
            .child(div().flex_1().min_h_0().child(body))
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
    pub(super) fn body(
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
                header_override: (state.filter.editing && state.hub_pane == HubPane::Repos)
                    .then(|| dialogs::filter::bar(state).into_any_element()),
                rows: model.rail.clone(),
                cursor: state.cursors.repos,
                focused: state.hub_pane == HubPane::Repos,
                collapsed: state.rail_collapsed,
                context_name: context_name.clone(),
                filter: filter.clone().filter(|_| state.hub_pane == HubPane::Repos),
                stale: stale.clone(),
            },
            &self.rail_scroll,
            cx,
        );

        let pane_ch = list_pane_ch(
            width,
            state.rail_collapsed,
            state.detail_open,
            &cx.theme().metrics,
        );
        let scope_name = match &state.scope {
            RepoScope::All => SharedString::new_static("All"),
            RepoScope::Repo(repo) => SharedString::from(repo.name().to_owned()),
        };

        let list = match state.screen {
            Screen::Hub {
                tab: HubTab::Worktrees,
            } => worktrees_list::render(
                ListProps {
                    header_override: (state.filter.editing && state.hub_pane == HubPane::List)
                        .then(|| dialogs::filter::bar(state).into_any_element()),
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
                now.saturating_sub(model.prepared_at),
            ),
            _ => {
                let cache_key = cache::PrCacheKey::from_state(state);
                let slice = hub.prs.slice_for(state.pr_tab, &cache_key);
                let cache_matches_scope = hub.prs.matches(&cache_key);
                prs_screen::render(
                    PrScreenProps {
                        header_override: (state.filter.editing && state.hub_pane == HubPane::List)
                            .then(|| dialogs::filter::bar(state).into_any_element()),
                        rows: model.prs.clone(),
                        cursor: pr_cursor(state),
                        focused: state.hub_pane == HubPane::List,
                        tab: state.pr_tab,
                        mine_count: cache_matches_scope
                            .then(|| hub.prs.mine.as_ref().map(|slice| slice.prs.len()))
                            .flatten(),
                        review_count: cache_matches_scope
                            .then(|| hub.prs.review.as_ref().map(|slice| slice.prs.len()))
                            .flatten(),
                        fetched_age: slice.and_then(|slice| age_secs(&slice.fetched_at, now)),
                        loading: cache_matches_scope && hub.prs.loading(state.pr_tab),
                        cold: cache_matches_scope
                            && hub.prs.is_cold(state.pr_tab)
                            && hub.prs.loading(state.pr_tab),
                        error: cache_matches_scope
                            .then(|| hub.prs.error(state.pr_tab).map(ToOwned::to_owned))
                            .flatten()
                            .or_else(|| slice.and_then(|slice| slice.error.clone()))
                            .map(SharedString::from),
                        hidden: model.pr_hidden,
                        pane_ch,
                        multi_repo: state.scope == RepoScope::All,
                        scope: scope_name,
                        filter: filter.filter(|_| state.hub_pane == HubPane::List),
                    },
                    &self.pr_scroll,
                    now.saturating_sub(model.prepared_at),
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
            split = split.leading_size(cx.theme().metrics.rail_w);
        }

        let Some(detail) = self.detail(state, hub, model, now, cx) else {
            return split.into_any_element();
        };
        if detail_is_docked(width) {
            return div()
                .relative()
                .size_full()
                .child(split)
                .child(detail::panel(detail, true, &self.detail_scroll))
                .into_any_element();
        }
        SplitLayout::horizontal()
            .leading(split.into_any_element())
            .trailing(detail::panel(detail, false, &self.detail_scroll))
            .trailing_size(cx.theme().metrics.detail_w)
            .divider(false)
            .into_any_element()
    }

    /// The detail panel's body for whatever the cursor is on (§3.4).
    pub(super) fn detail(
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
        let home = self
            .home
            .as_deref()
            .map(|path| path.to_string_lossy())
            .unwrap_or_default();

        if state.hub_pane == HubPane::Repos {
            let row = model.rail.get(state.cursors.repos)?;
            let repo_id = row.repo.as_ref()?;
            if matches!(row.kind, RailKind::Cloning | RailKind::CloneFailed) {
                let clone = snapshot
                    .clones
                    .iter()
                    .find(|clone: &&CloneJob| &clone.id == repo_id)?;
                return Some(detail::clone_job(clone, &home, cx));
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
            return Some(detail::repo(
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
            return Some(detail::worktree_with_status(
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
                row.glyph,
                cx,
            ));
        }

        let row = model.prs.get(pr_cursor(state))?;
        let pr = find_pr(hub, state.pr_tab, &row.repo, row.number)?;
        let local = snapshot
            .worktrees
            .iter()
            .find(|worktree| worktree_matches_pr(worktree, pr));
        Some(detail::pull_request_with_status(
            PrProps {
                pr,
                local,
                host: row.host.as_ref(),
                host_link: row.host_link,
                status: local.and_then(|worktree| {
                    snapshot
                        .statuses
                        .iter()
                        .find(|status| status.worktree_id == worktree.id)
                }),
                home: &home,
                now,
            },
            Some(row.presence),
            cx,
        ))
    }
}

/// Publishes the focused row into the status-bar breadcrumb (`APP-CONTRACTS` §2).
///
/// The write is silent on purpose: this runs from `HubCtx::synchronize`, itself an observer of
/// [`AppState`], so the notification round that moved the cursor is still being delivered and
/// the bar reads the new row when it redraws for it. `synchronize` notifies on its own when the
/// projection changed; notifying here would only re-enter the round already in flight.
pub(super) fn publish_breadcrumb(state: &Entity<AppState>, model: &HubModel, cx: &mut App) {
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
            (HubPane::List, _) => model
                .prs
                .get(pr_cursor(read))
                .map(|row| format!("#{}", row.number)),
        }
    };
    state.update(cx, |state, _| state.breadcrumb_row = row);
}
