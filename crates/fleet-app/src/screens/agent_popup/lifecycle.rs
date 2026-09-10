use super::*;

impl AgentPopup {
    pub(super) fn reconcile(
        &self,
        model: &Model,
        bridge: &Bridge,
        state: &Entity<AppState>,
        cell: Size<Pixels>,
        cx: &mut App,
    ) {
        let mut local = self.local.borrow_mut();
        let owner = PendingOwner {
            agent: model.agent,
            generation: model.generation,
        };
        if !model.reachable
            || local
                .state
                .pending_owner
                .is_some_and(|pending| pending != owner)
        {
            local.discard_pending();
        }
        local.wheel.reconcile(model.terminal);
        let reattaching = model
            .terminal
            .is_some_and(|id| state.read(cx).reattach_pending.contains(&id));
        if reattaching {
            local.attached = None;
        }
        let relinked = local.attached_generation != model.generation;
        if local.attached != model.terminal || relinked {
            local.clear_selections();
            let size = if local.area.size.width > px(0.0) && local.area.size.height > px(0.0) {
                grid_size(local.area.size, cell, local.grid_padding())
            } else {
                local.state.size.unwrap_or(FALLBACK_GRID)
            };
            if model.terminal.is_some() {
                local.state.size = Some(size);
            }
            drop(local);
            reconcile_attachment(
                &self.local,
                AttachmentSpec {
                    target: model.terminal,
                    generation: model.generation,
                    preserve: model.base_terminal,
                    size,
                },
                bridge,
                state,
                cx,
            );
            local = self.local.borrow_mut();
        }
        if let Some(terminal) = model.terminal {
            state.update(cx, |app, _| {
                app.reattach_pending.remove(&terminal);
            });
        }
        if model.primed
            && (local.mouse_selection.is_some_and(|selection| {
                Some(selection.cols) != model.cols
                    || selection.alt_screen != model.alt_screen
                    || Some(selection.history_epoch) != model.history_epoch
            }) || (local.anchor.is_some()
                && (local.anchor_history_epoch != model.history_epoch
                    || local.anchor_cols != model.cols
                    || local.anchor_alt_screen != Some(model.alt_screen))))
        {
            local.clear_selections();
        }
        if model.reachable
            && model.primed
            && let Some(terminal) = model.terminal
            && local.state.pending_owner == Some(owner)
        {
            for input in local.take_pending(owner) {
                bridge.send(input.into_request(terminal));
            }
        }
    }

    pub(super) fn cache_selection(&self, model: &Model, state: &Entity<AppState>, cx: &App) {
        if let Some(terminal) = model.terminal
            && let Some(grid) = state.read(cx).grids.get(&terminal)
        {
            self.local
                .borrow_mut()
                .cache_viewport(terminal, grid, false);
        }
    }

    pub(super) fn track_selection(&self, model: &Model, state: &Entity<AppState>, cx: &App) {
        if let Some(grid) = model.terminal.and_then(|id| state.read(cx).grids.get(&id)) {
            self.local.borrow_mut().track_selection(grid, false);
        }
    }

    pub(super) fn arm_prefix_hint(&self, model: &Model, state: &Entity<AppState>, cx: &mut App) {
        self.local
            .borrow_mut()
            .hint
            .reconcile(model.mode == AgentPopupMode::Prefix, state, cx);
    }

    /// Reconcile state before rendering, including hidden-surface teardown.
    pub(crate) fn synchronize(&mut self, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
        let Some(model) = Model::build(state.read(cx)) else {
            let preserve = state
                .read(cx)
                .active_session()
                .and_then(|session| session.active_terminal);
            self.detach(bridge, preserve);
            self.model = None;
            return;
        };
        self.local.borrow_mut().padding = Some(cx.theme().space.sm);
        self.reconcile(&model, bridge, state, cell_size(cx.theme()), cx);
        self.cache_selection(&model, state, cx);
        self.track_selection(&model, state, cx);
        self.arm_prefix_hint(&model, state, cx);

        if let Some(grid) = model
            .terminal
            .and_then(|terminal| state.read(cx).grids.get(&terminal))
            .filter(|grid| grid.primed)
        {
            self.local
                .borrow_mut()
                .presentation
                .update(model.terminal, grid, cx.theme());
        }
        self.model = Some(model);
    }
}

pub(super) fn detach_local(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    preserve: Option<TerminalId>,
) {
    let mut local = local.borrow_mut();
    local.detach(bridge, preserve);
    local.clear_selections();
    local.discard_pending();
    local.wheel.reconcile(None);
    local.hint.clear();
}
