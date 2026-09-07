use super::*;

impl WorkspaceScreen {
    /// Creates, focuses, releases and evicts the Fleet-drawn panes.
    ///
    /// Creation is lazy — the first time a `fleet://` tab is actually selected — because a pane
    /// starts a `git` worker thread and immediately runs a full snapshot of the repository.
    /// Opening a worktree must not pay for a tab the user never looks at.
    pub(super) fn sync_panes(
        &mut self,
        model: &Model,
        bridge: &Bridge,
        state: &Entity<AppState>,
        window: &mut Window,
        cx: &mut App,
    ) {
        // A pane that emitted `Quit` shut its own `git` worker down and can never come back to
        // life, so it is dropped and rebuilt on the next activation.
        let left: Vec<WorktreeId> = std::mem::take(&mut self.local.borrow_mut().state.pane_quit);
        for worktree in left {
            self.panes.remove(&worktree);
        }
        // The daemon's snapshot is authoritative: a worktree it no longer lists can never be
        // painted again, so its pane — and the git worker behind it — goes with it.
        if let Some(snapshot) = state.read(cx).snapshot.as_ref() {
            let live: std::collections::HashSet<&WorktreeId> =
                snapshot.worktrees.iter().map(|entry| &entry.id).collect();
            self.panes.retain(|worktree, _| live.contains(worktree));
        }

        let active = model.native.then(|| model.worktree.clone()).flatten();
        let Some((worktree, path)) = active else {
            // Not on a Fleet-drawn tab: nothing owns the keyboard on our behalf, and every
            // pane that exists is idle in the background.
            self.local.borrow_mut().state.pane_focused = false;
            for pane in self.panes.values() {
                pane.view
                    .update(cx, |pane, cx| pane.set_active(false, window, cx));
            }
            return;
        };

        if !self.panes.contains_key(&worktree) {
            let view = cx.new(|cx| Lazygit::embedded(path, cx));
            let mut subscriptions = Vec::new();
            // The pane is a separate entity, so the shell has to be told to repaint when its
            // own state moves — a `git` result arriving, a cursor moving, an overlay opening.
            let repaint = state.clone();
            subscriptions.push(cx.observe(&view, move |_view, cx| {
                repaint.update(cx, |_, cx| cx.notify());
            }));
            // `q` inside the pane means "leave this tab", not "quit Fleet": the tab the user
            // came from is the one they want back.
            let (quit_local, quit_bridge, quit_state) = self.handles(bridge, state);
            let quit_worktree = worktree.clone();
            subscriptions.push(cx.subscribe(&view, move |_view, event, cx| match event {
                LazygitEvent::Quit => {
                    quit_local
                        .borrow_mut()
                        .state
                        .pane_quit
                        .push(quit_worktree.clone());
                    let previous = neighbour_terminal(&quit_state, -1, cx);
                    select_terminal(&quit_local, &quit_bridge, &quit_state, previous, cx);
                }
            }));
            self.panes.insert(
                worktree.clone(),
                Pane {
                    view,
                    _subscriptions: subscriptions,
                },
            );
        }

        // A Fleet overlay — a dialog, the palette, the filter, the jobs sheet — owns the
        // keyboard over every screen (§2.8), so the pane must let go while one is open and
        // take the keyboard back, unasked, when it closes.
        let owns = !model.overlay_open;
        self.local.borrow_mut().state.pane_focused = owns;
        for (candidate, pane) in &self.panes {
            let active = owns && candidate == &worktree;
            pane.view
                .update(cx, |pane, cx| pane.set_active(active, window, cx));
        }
    }

    /// The band a Fleet-drawn tab fills.
    ///
    /// The element id is per worktree so gpui keeps each pane's hover, scroll and animation
    /// state apart when the user moves between worktrees.
    pub(super) fn pane_area(&self, model: &Model, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let pane = model
            .worktree
            .as_ref()
            .and_then(|(worktree, _)| self.panes.get(worktree));
        let body: AnyElement = match pane {
            Some(pane) => pane.view.clone().into_any_element(),
            // One frame at most: `sync_panes` creates the view before this runs, unless the
            // snapshot has no worktree for the session (an agent session, or a race with a
            // deletion), in which case the tab has nothing to show and says so.
            None => div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Text::ui("no worktree for this tab").muted())
                .into_any_element(),
        };
        let id = model.worktree.as_ref().map_or_else(
            || "workspace-native-pane".to_owned(),
            |(worktree, _)| format!("workspace-native-pane-{worktree}"),
        );
        div()
            .id(SharedString::from(id))
            .relative()
            .flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.colors.bg)
            .child(body)
            .into_any_element()
    }
}

/// One Fleet-drawn tab, kept alive across frames.
///
/// The view holds a `fleet-git` worker thread and a cached diff model, so it is created once
/// per worktree and reused: rebuilding it on every activation would re-run `git status`,
/// `git log` and `git branch` and lose the cursor the user left in the Files pane.
pub(super) struct Pane {
    pub(super) view: Entity<Lazygit>,
    /// Repaint on notify and the `Quit` handler. Dropping these ends both.
    pub(super) _subscriptions: Vec<Subscription>,
}
