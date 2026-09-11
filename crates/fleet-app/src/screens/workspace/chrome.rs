use super::*;

impl WorkspaceScreen {
    pub(super) fn header(
        &self,
        model: &Model,
        pr: Option<(u64, PrBadgeState)>,
        agent: Option<SharedString>,
    ) -> AnyElement {
        let mut header = WorkspaceHeader::new(model.title.clone())
            .status(model.status)
            .keep_alive(model.keep_alive.iter().cloned())
            .jobs(model.running_jobs, model.failed_jobs)
            // §3.6: the frame's VT modes are the only thing that explains why a documented key
            // behaves differently — no scrollback in alt-screen, the app owning drag-select
            // under mouse reporting. They are badged here, in reserved chrome, and never over
            // the grid, whose cells are live output. Zero-suppressed.
            .modes(model.modes.iter().copied())
            .waking(model.waking);
        if let Some(repo) = &model.repo {
            header = header.repo(SharedString::from(repo.to_string()));
        }
        if let Some((host, reachable)) = &model.host {
            header = header.host(host.clone(), reachable.is_reachable());
        }
        if let Some((number, badge)) = pr {
            header = header.pr(number, badge);
        }
        // §3.3: an agent tab states its own attention beside the session's status glyph.
        if let Some(word) = agent {
            header = header.agent(word);
        }

        header.into_any_element()
    }

    pub(super) fn tab_strip(
        &self,
        model: &Model,
        bridge: &Bridge,
        state: &Entity<AppState>,
        cx: &mut App,
    ) -> AnyElement {
        let app = state.read(cx);
        let Some(session) = app.active_session() else {
            return div().into_any_element();
        };
        let status = match &session.kind {
            SessionKind::Worktree(worktree) => app.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .statuses
                    .iter()
                    .find(|status| &status.worktree_id == worktree)
            }),
            SessionKind::Agent { .. } => None,
        };
        let agents = threads_of(app, session);
        let mut tabs = self.local.borrow_mut().state.tab_labels.tabs(
            session,
            status,
            model.terminal,
            &app.renamed_terminals,
        );
        let first_agent = tabs.len();
        let selected = active_target(model);
        for (offset, summary) in agents.iter().enumerate() {
            let active = selected == Some(workspace_tabs::TabTarget::Agent(summary.thread));
            tabs.push(workspace_tabs::agent_tab(
                summary,
                first_agent + offset + 1,
                app.agents.attention(summary.thread),
                active,
            ));
        }
        let active = workspace_tabs::active_position(session, &agents, selected);

        let (new_request, new_bridge, new_state) =
            (shell_tab_request(session), bridge.clone(), state.clone());
        let new_local = Rc::clone(&self.local);
        let (select_local, select_bridge, select_state) =
            (Rc::clone(&self.local), bridge.clone(), state.clone());
        TerminalTabStrip::new(tabs)
            .active(active)
            // Mouse parity for `ctrl-s 1`-`9` (§3.6). It is also the only way a mouse-first
            // user reaches a Fleet-drawn tab, which is why the strip is finally wired.
            .on_select(move |position, _window, cx| {
                let target = {
                    let app = select_state.read(cx);
                    app.active_session().and_then(|session| {
                        let agents = threads_of(app, session);
                        workspace_tabs::target_at(session, &agents, position)
                    })
                };
                select_target(&select_local, &select_bridge, &select_state, target, cx);
            })
            // Mouse parity for `ctrl-s c` (§3.6): the `+` is the same request.
            .on_new(move |_window, cx| {
                request_shell_tab(&new_local, new_request.clone(), &new_bridge, &new_state, cx);
            })
            .into_any_element()
    }

    /// The screen with no session behind it, which only happens between two snapshots.
    pub(super) fn empty(&self, focus: &FocusHandle, cx: &mut App) -> Div {
        let theme = cx.theme();
        div()
            .track_focus(focus)
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .bg(theme.colors.bg)
            .child(Text::ui("no session").muted())
    }
}

/// The read-only trailing region follows the current window width.
#[must_use]
pub(super) fn watch_width(window_width: f32) -> f32 {
    (window_width * 0.4).clamp(360.0, 640.0)
}

/// The six prefix keys the delayed hint strip lists (§3.6).
pub(super) fn prefix_hints() -> KeyHintRow {
    KeyHintRow::new()
        .key("s", "hub")
        .key("1-9", "tab")
        .key("c", "new")
        .key("x", "close")
        .key("[", "scroll")
        .key("a", "claude")
}
