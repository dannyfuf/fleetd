use super::*;

impl WorkspaceScreen {
    /// The grid, its overlays, and the invisible element that measures it.
    ///
    /// A Fleet-drawn tab replaces the whole band: no grid, no scroll pill, no measuring
    /// element, because none of them describe a pane that is not a terminal.
    pub(super) fn terminal_area(
        &self,
        model: &Model,
        bridge: &Bridge,
        state: &Entity<AppState>,
        focus: &FocusHandle,
        focused: bool,
        cx: &App,
    ) -> AnyElement {
        if model.native {
            return self.pane_area(model, cx);
        }
        let theme = cx.theme();
        let app = state.read(cx);
        let mirror = app.active_grid();
        let selection = mirror.and_then(|grid| self.local.borrow().selection(grid));
        let hint_visible = self.local.borrow().hint.visible();

        let grid: AnyElement = match mirror.filter(|grid| grid.primed) {
            Some(grid) => {
                let resize_local = Rc::clone(&self.local);
                let geometry_local = Rc::clone(&self.local);
                let resize_bridge = bridge.clone();
                let resize_state = state.clone();
                // A full-Workspace agent session can also be opened in the popup. Both views
                // remain attached, but only the popup may resize their shared PTY while visible.
                // This grid still records its desired size so hiding restores it immediately.
                let resize_terminal = model.terminal;
                let resize_sends = !model.popup_owns_terminal;
                let mut painted = self
                    .local
                    .borrow()
                    .grid("workspace-terminal-grid", grid, focused)
                    .on_geometry(move |bounds, metrics| {
                        if let Some(terminal) = resize_terminal {
                            geometry_local.borrow_mut().geometry =
                                Some((terminal, bounds, metrics));
                        }
                    })
                    .on_resize(move |cols, rows, _window, cx| {
                        let Some(terminal) = resize_terminal else {
                            return;
                        };
                        let cols = u16::try_from(cols).unwrap_or(u16::MAX);
                        let rows = u16::try_from(rows).unwrap_or(u16::MAX);
                        let mut local = resize_local.borrow_mut();
                        let selection_cleared = local
                            .mouse_selection
                            .is_some_and(|selection| selection.cols != cols);
                        if selection_cleared {
                            local.mouse_selection = None;
                            local.row_caches.clear();
                        }
                        let changed = local.sizes.get(&terminal) != Some(&(cols, rows));
                        if changed {
                            local.sizes.insert(terminal, (cols, rows));
                        }
                        if changed && resize_sends {
                            resize_bridge.send(RequestBody::ResizeTerminal {
                                terminal,
                                cols,
                                rows,
                            });
                        }
                        drop(local);
                        if selection_cleared {
                            resize_state.update(cx, |_, cx| cx.notify());
                        }
                    });
                if let Some(selection) = selection {
                    painted = painted.selection(selection);
                }
                painted.into_any_element()
            }
            None => attaching(theme).into_any_element(),
        };

        // The pixel area the grid is laid out into, remembered for the *next* attach: without
        // it every new or newly selected terminal was attached at 80 × 24 and then resized,
        // which costs a full-screen redraw at the wrong size before the right one arrives.
        let area_local = Rc::clone(&self.local);
        let wheel_local = Rc::clone(&self.local);
        let wheel_bridge = bridge.clone();
        let wheel_state = state.clone();
        let wheel_terminal = model.terminal;
        let area = div()
            .id("terminal-wheel-area")
            .on_scroll_wheel(move |event: &ScrollWheelEvent, _, cx| {
                let Some(terminal) = wheel_terminal else {
                    return;
                };
                let app = wheel_state.read(cx);
                if !workspace_terminal_is_live_owner(app, Some(terminal)) {
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
            .flex_1()
            .w_full()
            .overflow_hidden()
            .child(measure(move |bounds| area_local.borrow_mut().area = bounds))
            .child(grid)
            .children((model.mode == TerminalMode::Scroll).then(|| {
                ScrollPill::new(model.scroll_offset, model.scrollback_len)
                    .selecting(selection.is_some())
                    .alt_screen(model.alt_screen)
            }))
            .child(
                PrefixHint::new(model.mode == TerminalMode::Prefix && hint_visible)
                    .hints(prefix_hints()),
            );
        self.with_mouse_selection(area, state, focus, model.terminal)
            .into_any_element()
    }

    /// Installs every listener the Workspace owns on the focused element.
    pub(super) fn with_keys(&self, root: Div, bridge: &Bridge, state: &Entity<AppState>) -> Div {
        let root = self.with_key_forwarding(root, bridge, state);
        let root = self.with_clipboard_actions(root, bridge, state);
        let root = self.with_prefix_actions(root, bridge, state);
        let root = self.with_agent_actions(root, bridge, state);
        self.with_scroll_actions(root, bridge, state)
    }

    /// Terminal-mode key forwarding, the inline rename editor and the close confirmation.
    pub(super) fn with_key_forwarding(
        &self,
        root: Div,
        bridge: &Bridge,
        state: &Entity<AppState>,
    ) -> Div {
        let (local, bridge, state) = self.handles(bridge, state);
        root.on_key_down(move |event: &KeyDownEvent, _window, cx| {
            if forward_terminal_key(&local, &state, &bridge, &event.keystroke, event.is_held, cx) {
                cx.stop_propagation();
            }
        })
    }

    /// Standard clipboard actions while the terminal owns focus.
    pub(super) fn with_clipboard_actions(
        &self,
        root: Div,
        bridge: &Bridge,
        state: &Entity<AppState>,
    ) -> Div {
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(
                move |_: &crate::actions::workspace::PasteClipboard, _window, cx| {
                    paste_clipboard(&local, &bridge, &state, cx);
                },
            )
        };
        let (local, bridge, state) = self.handles(bridge, state);
        root.on_action(
            move |_: &crate::actions::workspace::CopySelection, _window, cx| {
                copy_selection(&local, &state, &bridge, cx);
            },
        )
    }

    /// Plain left-drag selection, independent of the program running in the PTY.
    pub(super) fn with_mouse_selection(
        &self,
        area: gpui::Stateful<Div>,
        state: &Entity<AppState>,
        focus: &FocusHandle,
        terminal: Option<TerminalId>,
    ) -> gpui::Stateful<Div> {
        let down_local = Rc::clone(&self.local);
        let down_state = state.clone();
        let down_focus = focus.clone();
        let area = area.on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window, cx| {
                if !workspace_terminal_is_live_owner(down_state.read(cx), terminal) {
                    cx.stop_propagation();
                    return;
                }
                window.focus(&down_focus, cx);
                let Some(cell) = mouse_cell(&down_local, &down_state, event.position, window, cx)
                else {
                    return;
                };
                let granularity = click_granularity(event.click_count);
                let initial = down_state.read(cx).active_grid().and_then(|grid| {
                    absolute_selection_at(grid, cell.viewport, granularity).or_else(|| {
                        absolute_selection_at(grid, cell.viewport, SelectionGranularity::Cell)
                    })
                });
                let Some(initial) = initial else {
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
        let area = area.on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
            if !workspace_terminal_is_live_owner(move_state.read(cx), terminal) {
                cx.stop_propagation();
                return;
            }
            if let Some(terminal) = move_state
                .read(cx)
                .active_session()
                .and_then(|session| session.active_terminal)
            {
                move_local.borrow_mut().wheel.point_at(terminal);
            }

            if event.pressed_button != Some(MouseButton::Left)
                || !move_local
                    .borrow()
                    .mouse_selection
                    .is_some_and(|selection| selection.dragging)
            {
                return;
            }
            let Some(cell) = mouse_cell(&move_local, &move_state, event.position, window, cx)
            else {
                return;
            };
            let changed = move_state
                .read(cx)
                .active_grid()
                .is_some_and(|grid| move_local.borrow_mut().extend_mouse_selection(grid, cell));
            if changed {
                move_state.update(cx, |_, cx| cx.notify());
            }
            cx.stop_propagation();
        });

        let up_local = Rc::clone(&self.local);
        let up_state = state.clone();
        let area = area.on_mouse_up(MouseButton::Left, move |_event, _window, cx| {
            if !workspace_terminal_is_live_owner(up_state.read(cx), terminal) {
                if up_local.borrow_mut().cancel_drag() {
                    up_state.update(cx, |_, cx| cx.notify());
                    cx.stop_propagation();
                }
                return;
            }
            if finish_mouse_selection(&up_local, &up_state, cx) {
                cx.stop_propagation();
            }
        });
        let out_local = Rc::clone(&self.local);
        let out_state = state.clone();
        area.on_mouse_up_out(MouseButton::Left, move |_event, _window, cx| {
            if !workspace_terminal_is_live_owner(out_state.read(cx), terminal) {
                if out_local.borrow_mut().cancel_drag() {
                    out_state.update(cx, |_, cx| cx.notify());
                    cx.stop_propagation();
                }
                return;
            }
            // GPUI runs mouse-up-out in capture, before sibling on_click handlers.
            // Only consume a release that belongs to an active terminal drag.
            if finish_mouse_selection(&out_local, &out_state, cx) {
                cx.stop_propagation();
            }
        })
    }
}

/// Sends clipboard text through the same ordered attach gate as terminal keys.
pub(super) fn paste_clipboard(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    cx: &mut App,
) {
    let Some((terminal, primed)) = terminal_input_target(state, cx) else {
        return;
    };
    let mut accepted = true;
    surface::route_paste(local, state, cx, |input| {
        accepted = local
            .borrow_mut()
            .send_or_queue(bridge, Some(terminal), primed, input);
    });
    surface::report_input_delivery(accepted, state, cx);
}

pub(super) fn terminal_input_target(
    state: &Entity<AppState>,
    cx: &App,
) -> Option<(TerminalId, bool)> {
    terminal_input_target_of(state.read(cx))
}

/// The PTY a Workspace keystroke belongs to, and whether it is primed.
///
/// `None` whenever nothing is listening: an overlay or the popup owns the keys, the link is
/// down, or the tab on screen is drawn by Fleet. That last case covers the native agent tabs,
/// which are client state over the same strip and leave `session.active_terminal` pointing at
/// the PTY the user came from; without the guard every character typed into the composer is
/// also written to that background PTY.
pub(super) fn terminal_input_target_of(app: &AppState) -> Option<(TerminalId, bool)> {
    if app.overlay.is_some()
        || app.agent_popup.is_some()
        || app.drops_terminal_keys()
        || app.active_tab_is_fleet_drawn()
    {
        return None;
    }
    let terminal = active_pty_terminal_of(app.active_session()?)?;
    let primed = app.grids.get(&terminal).is_some_and(|grid| grid.primed);
    Some((terminal, primed))
}

/// Pointer input is never replayed. A listener from an older rendered frame may only mutate the
/// Workspace terminal that is still the authoritative, uncovered topmost owner.
pub(super) fn workspace_terminal_is_live_owner(
    app: &AppState,
    terminal: Option<TerminalId>,
) -> bool {
    matches!(app.screen, Screen::Workspace { .. })
        && app.overlay.is_none()
        && app.agent_popup.is_none()
        && terminal.is_some()
        && app
            .active_session()
            .and_then(|session| session.active_terminal)
            == terminal
        && !app.active_tab_is_fleet_drawn()
}

pub(super) fn forward_terminal_key(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    keystroke: &gpui::Keystroke,
    is_held: bool,
    cx: &mut App,
) -> bool {
    {
        let app = state.read(cx);
        if !matches!(app.screen, Screen::Workspace { .. })
            || app.overlay.is_some()
            || app.agent_popup.is_some()
            || app.terminal_mode != TerminalMode::Terminal
        {
            return false;
        }
        if app.drops_terminal_keys() {
            local.borrow_mut().pending.clear();
            return true;
        }
    }
    let Some((terminal, primed)) = terminal_input_target(state, cx) else {
        return false;
    };
    let mut accepted = true;
    let routed = surface::route_key(local, state, keystroke, is_held, cx, |input| {
        accepted = local
            .borrow_mut()
            .send_or_queue(bridge, Some(terminal), primed, input);
    });
    surface::report_input_delivery(accepted, state, cx);
    routed
}

pub(super) fn copy_selection(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    if surface::route_copy(local, state, selected_terminal(state, cx), cx) {
        return;
    }
    // A binding normally prevents `on_key_down` from seeing the keystroke. Forward the unmatched
    // cmd-c explicitly so Fleet never steals a terminal program's shortcut.
    if state.read(cx).terminal_mode == TerminalMode::Terminal
        && let Some((terminal, primed)) = terminal_input_target(state, cx)
    {
        let accepted = local.borrow_mut().send_or_queue(
            bridge,
            Some(terminal),
            primed,
            PendingInput::Key(copy_keystroke()),
        );
        surface::report_input_delivery(accepted, state, cx);
    }
}

/// The terminal a Workspace selection belongs to, whatever mode the surface is in.
fn selected_terminal(state: &Entity<AppState>, cx: &App) -> Option<TerminalId> {
    state
        .read(cx)
        .active_session()
        .and_then(|session| session.active_terminal)
}

/// Maps a mouse position with the same measured metrics the grid painter uses.
pub(super) fn mouse_cell(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    position: gpui::Point<Pixels>,
    window: &Window,
    cx: &App,
) -> Option<MouseCell> {
    let grid = state.read(cx).active_grid()?;
    let metrics = CellMetrics::measure(cx.theme(), window, cx);
    let local = local.borrow();
    MouseCell::at(grid, local.area, position, metrics, local.grid_padding())
}

/// Claims a release only for an active terminal drag; completed selections do not own clicks.
pub(super) fn end_mouse_drag(local: &mut Local) -> Option<bool> {
    local
        .mouse_selection
        .filter(|selection| selection.dragging)?;
    local.settle_selection()
}

/// Ends and copies a terminal drag, returning whether its release should be consumed.
pub(super) fn finish_mouse_selection(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    cx: &mut App,
) -> bool {
    let Some(selected) = end_mouse_drag(&mut local.borrow_mut()) else {
        return false;
    };
    if selected {
        let text = local
            .borrow()
            .selection_text_for(state.read(cx), selected_terminal(state, cx));
        if matches!(text, CurrentSelectionText::Missing) {
            selection_scrolled_away(local, state, cx);
            return true;
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
