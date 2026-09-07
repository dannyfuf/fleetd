use super::*;

impl AgentPopup {
    pub(super) fn terminal_area(
        &self,
        model: &Model,
        state: &Entity<AppState>,
        bridge: &Bridge,
        focus: &FocusHandle,
        window: &Window,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let owner = PendingOwner {
            agent: model.agent,
            generation: model.generation,
        };
        let app = state.read(cx);
        let mirror = model.terminal.and_then(|terminal| app.grids.get(&terminal));
        let selection = mirror.and_then(|grid| self.local.borrow().selection(grid));
        let focused = focus.is_focused(window);
        let hint_visible = self.local.borrow().hint.visible();
        let grid: AnyElement = match mirror.filter(|grid| grid.primed) {
            Some(grid) => {
                let terminal = model.terminal;
                let geometry = Rc::clone(&self.local);
                let resize = Rc::clone(&self.local);
                let resize_bridge = bridge.clone();
                let mut painted = self
                    .local
                    .borrow()
                    .grid("agent-popup-terminal-grid", grid, focused)
                    .on_geometry(move |bounds, metrics| {
                        if let Some(terminal) = terminal {
                            geometry.borrow_mut().geometry = Some((terminal, bounds, metrics));
                        }
                    })
                    .on_resize(move |cols, rows, _window, _cx| {
                        let Some(terminal) = terminal else { return };
                        let size = (
                            u16::try_from(cols).unwrap_or(u16::MAX),
                            u16::try_from(rows).unwrap_or(u16::MAX),
                        );
                        let mut local = resize.borrow_mut();
                        if local.state.size != Some(size) {
                            local.state.size = Some(size);
                            resize_bridge.send(RequestBody::ResizeTerminal {
                                terminal,
                                cols: size.0,
                                rows: size.1,
                            });
                        }
                    });
                if let Some(selection) = selection {
                    painted = painted.selection(selection);
                }
                painted.into_any_element()
            }
            None => attaching(theme).into_any_element(),
        };
        let area_local = Rc::clone(&self.local);
        let wheel_local = Rc::clone(&self.local);
        let wheel_bridge = bridge.clone();
        let wheel_state = state.clone();
        let terminal = model.terminal;
        let area = div()
            .id("agent-popup-terminal-area")
            .on_scroll_wheel(move |event: &ScrollWheelEvent, _, cx| {
                let Some(terminal) = terminal else { return };
                let app = wheel_state.read(cx);
                if !agent_terminal_is_live_owner(app, owner, Some(terminal)) {
                    cx.stop_propagation();
                    return;
                }
                if let Some(request) = wheel_local.borrow_mut().wheel_request(app, terminal, event)
                {
                    wheel_bridge.send(request);
                }
                cx.stop_propagation();
            })
            .relative()
            .flex()
            // `Veil` is a block container, so this nested area is no longer a flex item of the
            // card. Give its grid a definite height to fill instead.
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(measure(move |bounds| area_local.borrow_mut().area = bounds))
            .child(grid)
            .children((model.mode == AgentPopupMode::Scroll).then(|| {
                ScrollPill::new(model.scroll_offset, model.scrollback_len)
                    .selecting(selection.is_some())
                    .alt_screen(model.alt_screen)
            }))
            .child(
                PrefixHint::new(model.mode == AgentPopupMode::Prefix && hint_visible).hints(
                    KeyHintRow::new()
                        .key("q", "hide")
                        .key("a/A", "switch")
                        .key("[", "scroll")
                        .key("]", "paste")
                        .key("?", "help")
                        .key("r", "restart"),
                ),
            );
        self.with_mouse(area, state, focus, owner, model.terminal)
            .into_any_element()
    }

    pub(super) fn with_keys(&self, root: Div, state: &Entity<AppState>, bridge: &Bridge) -> Div {
        let local = Rc::clone(&self.local);
        let key_state = state.clone();
        let key_bridge = bridge.clone();
        let root = root.on_key_down(move |event: &KeyDownEvent, _window, cx| {
            if forward_terminal_key(
                &local,
                &key_state,
                &key_bridge,
                &event.keystroke,
                event.is_held,
                cx,
            ) {
                cx.stop_propagation();
            }
        });

        let enter_state = state.clone();
        let root = root.on_action(move |_: &agent::EnterPrefix, _window, cx| {
            enter_state.update(cx, |app, cx| {
                app.enter_agent_prefix();
                cx.notify();
            });
        });
        let hide_local = Rc::clone(&self.local);
        let hide_state = state.clone();
        let hide_bridge = bridge.clone();
        let root = root.on_action(move |_: &agent::Hide, _window, cx| {
            hide(&hide_local, &hide_state, &hide_bridge, cx);
        });
        let root = self.with_clipboard(root, state, bridge);
        let root = self.with_prefix(root, state, bridge);
        self.with_scroll(root, state, bridge)
    }

    pub(super) fn with_clipboard(
        &self,
        root: Div,
        state: &Entity<AppState>,
        bridge: &Bridge,
    ) -> Div {
        let copy_local = Rc::clone(&self.local);
        let copy_state = state.clone();
        let copy_bridge = bridge.clone();
        let root = root.on_action(move |_: &agent::CopySelection, _window, cx| {
            copy_selection(&copy_local, &copy_state, &copy_bridge, cx);
        });
        let paste_local = Rc::clone(&self.local);
        let paste_state = state.clone();
        let paste_bridge = bridge.clone();
        root.on_action(move |_: &agent::PasteClipboard, _window, cx| {
            paste(&paste_local, &paste_state, &paste_bridge, cx);
        })
    }

    pub(super) fn with_mouse(
        &self,
        area: gpui::Stateful<Div>,
        state: &Entity<AppState>,
        focus: &FocusHandle,
        owner: PendingOwner,
        terminal: Option<TerminalId>,
    ) -> gpui::Stateful<Div> {
        let down_local = Rc::clone(&self.local);
        let down_state = state.clone();
        let down_focus = focus.clone();
        let area = area.on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window, cx| {
                let Some(terminal) = terminal else {
                    cx.stop_propagation();
                    return;
                };
                if !agent_terminal_is_live_owner(down_state.read(cx), owner, Some(terminal)) {
                    cx.stop_propagation();
                    return;
                }
                window.focus(&down_focus, cx);
                let Some(cell) = mouse_cell(&down_local, &down_state, event.position, cx) else {
                    return;
                };
                let granularity = click_granularity(event.click_count);
                let Some(initial) = popup_grid(&down_state, cx)
                    .and_then(|grid| absolute_selection_at(grid, cell.viewport, granularity))
                else {
                    return;
                };
                down_local
                    .borrow_mut()
                    .begin_mouse_selection(initial, cell, granularity);
                down_state.update(cx, |_, cx| cx.notify());
                cx.stop_propagation();
            },
        );
        let move_local = Rc::clone(&self.local);
        let move_state = state.clone();
        let area = area.on_mouse_move(move |event: &MouseMoveEvent, _window, cx| {
            if event.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let Some(selection_terminal) = move_local
                .borrow()
                .mouse_selection
                .filter(|selection| selection.dragging)
                .and_then(|_| move_local.borrow().attached)
            else {
                return;
            };
            if !agent_terminal_is_live_owner(move_state.read(cx), owner, Some(selection_terminal)) {
                cx.stop_propagation();
                return;
            }
            let Some(cell) = mouse_cell(&move_local, &move_state, event.position, cx) else {
                return;
            };
            let changed = popup_grid(&move_state, cx)
                .is_some_and(|grid| move_local.borrow_mut().extend_mouse_selection(grid, cell));
            if changed {
                move_state.update(cx, |_, cx| cx.notify());
            }
            cx.stop_propagation();
        });
        let up_local = Rc::clone(&self.local);
        let up_state = state.clone();
        let area = area.on_mouse_up(MouseButton::Left, move |_, _, cx| {
            if finish_mouse_selection(&up_local, &up_state, owner, cx) {
                cx.stop_propagation();
            }
        });
        let out_local = Rc::clone(&self.local);
        let out_state = state.clone();
        area.on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
            if finish_mouse_selection(&out_local, &out_state, owner, cx) {
                cx.stop_propagation();
            }
        })
    }
}

/// Ends a popup drag and copies its selection, reporting whether the release was consumed.
fn finish_mouse_selection(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    owner: PendingOwner,
    cx: &mut App,
) -> bool {
    let Some(terminal) = local
        .borrow()
        .mouse_selection
        .filter(|selection| selection.dragging)
        .and_then(|_| local.borrow().attached)
    else {
        return false;
    };
    if !agent_terminal_is_live_owner(state.read(cx), owner, Some(terminal)) {
        local.borrow_mut().cancel_drag();
        return true;
    }
    let Some(selected) = end_mouse_drag(&mut local.borrow_mut()) else {
        return false;
    };
    if selected {
        let text = local
            .borrow()
            .selection_text_for(state.read(cx), selected_terminal(state.read(cx)));
        if matches!(text, CurrentSelectionText::Missing) {
            selection_scrolled_away(local, state, cx);
            return false;
        }
        if let CurrentSelectionText::Text(text) = text
            && !text.is_empty()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
    state.update(cx, |_, cx| cx.notify());
    true
}

/// Claims a release only while a terminal drag is active.
pub(super) fn end_mouse_drag(local: &mut Local) -> Option<bool> {
    local
        .mouse_selection
        .filter(|selection| selection.dragging)?;
    local.settle_selection()
}

/// The popup's terminal, while the popup is the surface a key or scroll may reach.
pub(super) fn popup_terminal(app: &AppState) -> Option<TerminalId> {
    if app.overlay.is_some() || app.drops_terminal_keys() {
        return None;
    }
    Some(app.agent_popup_session()?.terminals.first()?.id)
}

pub(super) fn popup_input_target(app: &AppState) -> Option<PopupInputTarget> {
    if app.overlay.is_some() || app.drops_terminal_keys() {
        return None;
    }
    let popup = app.agent_popup?;
    let terminal = app
        .agent_popup_session()
        .and_then(|session| session.terminals.first())
        .map(|terminal| terminal.id);
    Some(PopupInputTarget {
        owner: PendingOwner {
            agent: popup.agent,
            generation: app.link_generation,
        },
        terminal,
        primed: terminal.is_some_and(|id| app.grids.get(&id).is_some_and(|grid| grid.primed)),
    })
}

/// Whether this rendered popup instance is still the authoritative topmost pointer owner.
pub(super) fn agent_terminal_is_live_owner(
    app: &AppState,
    owner: PendingOwner,
    terminal: Option<TerminalId>,
) -> bool {
    app.overlay.is_none()
        && app
            .agent_popup
            .is_some_and(|popup| popup.agent == owner.agent)
        && app.link_generation == owner.generation
        && terminal.is_some()
        && app
            .agent_popup_session()
            .and_then(|session| session.terminals.first())
            .map(|terminal| terminal.id)
            == terminal
}

pub(super) fn forward_terminal_key(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    keystroke: &Keystroke,
    is_held: bool,
    cx: &mut App,
) -> bool {
    let target = {
        let app = state.read(cx);
        let Some(popup) = app.agent_popup else {
            return false;
        };
        if app.overlay.is_some() || popup.mode != AgentPopupMode::Terminal {
            return false;
        }
        if app.drops_terminal_keys() {
            local.borrow_mut().discard_pending();
            return true;
        }
        popup_input_target(app)
    };
    let Some(target) = target else {
        return false;
    };
    let accepted = Cell::new(true);
    let handled = surface::route_key(local, state, keystroke, is_held, cx, |input| {
        accepted.set(send_or_queue(local, bridge, target, input));
    });
    if handled {
        surface::report_input_delivery(accepted.get(), state, cx);
    }
    handled
}

pub(super) fn popup_grid<'a>(
    state: &'a Entity<AppState>,
    cx: &'a App,
) -> Option<&'a crate::state::MirrorGrid> {
    state.read(cx).agent_popup_grid()
}

pub(super) fn popup_rows(app: &AppState) -> i32 {
    i32::from(app.agent_popup_grid().map_or(0, |grid| grid.rows))
}

#[must_use = "rejected input must be surfaced to the user"]
pub(super) fn send_or_queue(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    target: PopupInputTarget,
    input: PendingInput,
) -> bool {
    let mut local = local.borrow_mut();
    if target.primed && target.terminal.is_some() {
        local.send_or_queue(bridge, target.terminal, true, input)
    } else {
        local.queue_input(target.owner, input)
    }
}

pub(super) fn paste(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    let Some(target) = popup_input_target(state.read(cx)) else {
        return;
    };
    let accepted = Cell::new(true);
    surface::route_paste(local, state, cx, |input| {
        accepted.set(send_or_queue(local, bridge, target, input));
    });
    surface::report_input_delivery(accepted.get(), state, cx);
}

pub(super) fn copy_selection(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    if surface::route_copy(local, state, selected_terminal(state.read(cx)), cx) {
        return;
    }
    let app = state.read(cx);
    if copy_reaches_pty(app.agent_popup.map(|popup| popup.mode))
        && let Some(target) = popup_input_target(app)
    {
        let accepted = send_or_queue(local, bridge, target, PendingInput::Key(copy_keystroke()));
        surface::report_input_delivery(accepted, state, cx);
    }
}

pub(super) fn copy_reaches_pty(mode: Option<AgentPopupMode>) -> bool {
    mode == Some(AgentPopupMode::Terminal)
}

/// The terminal a popup selection belongs to, whatever mode the surface is in.
fn selected_terminal(app: &AppState) -> Option<TerminalId> {
    app.agent_popup_session()
        .and_then(|session| session.terminals.first())
        .map(|terminal| terminal.id)
}

pub(super) fn mouse_cell(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    position: gpui::Point<Pixels>,
    cx: &App,
) -> Option<MouseCell> {
    let app = state.read(cx);
    let terminal = app.agent_popup_session()?.terminals.first()?.id;
    let grid = app.grids.get(&terminal)?;
    let (_, bounds, metrics) = local
        .borrow()
        .geometry
        .filter(|(id, _, _)| *id == terminal)?;
    popup_mouse_cell_at(grid, bounds, position, metrics)
}

/// Popup geometry comes from `TerminalGrid::on_geometry`, whose bounds are already inset.
pub(super) fn popup_mouse_cell_at(
    grid: &crate::state::MirrorGrid,
    bounds: gpui::Bounds<Pixels>,
    position: gpui::Point<Pixels>,
    metrics: fleet_ui_kit::CellMetrics,
) -> Option<MouseCell> {
    MouseCell::at(grid, bounds, position, metrics, px(0.0))
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PopupInputTarget {
    pub(super) owner: PendingOwner,
    pub(super) terminal: Option<TerminalId>,
    pub(super) primed: bool,
}
