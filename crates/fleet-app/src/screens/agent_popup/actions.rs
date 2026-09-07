use super::*;

impl AgentPopup {
    pub(super) fn with_prefix(&self, root: Div, state: &Entity<AppState>, bridge: &Bridge) -> Div {
        let literal_local = Rc::clone(&self.local);
        let literal_state = state.clone();
        let literal_bridge = bridge.clone();
        let root = root.on_action(move |_: &prefix::SendLiteral, _window, cx| {
            send_literal(&literal_local, &literal_state, &literal_bridge, cx);
        });
        let paste_local = Rc::clone(&self.local);
        let paste_state = state.clone();
        let paste_bridge = bridge.clone();
        let root = root.on_action(move |_: &prefix::Paste, _window, cx| {
            paste(&paste_local, &paste_state, &paste_bridge, cx);
        });
        let scroll_state = state.clone();
        let root = root.on_action(move |_: &prefix::EnterScroll, _window, cx| {
            enter_scroll(&scroll_state, cx);
        });
        let restart_state = state.clone();
        let restart_bridge = bridge.clone();
        let root = root.on_action(move |_: &prefix::RestartCommand, _window, cx| {
            restart(&restart_state, &restart_bridge, cx);
        });
        let cancel_state = state.clone();
        root.on_action(move |_: &prefix::Cancel, _window, cx| {
            cancel_state.update(cx, |app, cx| {
                app.leave_agent_prefix();
                cx.notify();
            });
        })
    }

    pub(super) fn with_scroll(&self, root: Div, state: &Entity<AppState>, bridge: &Bridge) -> Div {
        let local = Rc::clone(&self.local);
        let action_state = state.clone();
        let action_bridge = bridge.clone();
        let root = root.on_action(move |_: &scroll::LineDown, _, cx| {
            step_line(&local, &action_state, &action_bridge, 1, cx)
        });
        let local = Rc::clone(&self.local);
        let action_state = state.clone();
        let action_bridge = bridge.clone();
        let root = root.on_action(move |_: &scroll::LineUp, _, cx| {
            step_line(&local, &action_state, &action_bridge, -1, cx)
        });
        // Every page motion is the same handler over a different number of viewport rows.
        macro_rules! page_scroll {
            ($root:expr, $action:ty, $lines:expr) => {{
                let local = Rc::clone(&self.local);
                let state = state.clone();
                let bridge = bridge.clone();
                $root.on_action(move |_: &$action, _, cx| {
                    let lines = ($lines)(popup_rows(state.read(cx)));
                    scroll_selection_lines(&local, &state, &bridge, lines, cx);
                })
            }};
        }
        let root = page_scroll!(root, scroll::HalfPageDown, |rows: i32| rows.max(2) / 2);
        let root = page_scroll!(root, scroll::HalfPageUp, |rows: i32| -(rows.max(2) / 2));
        let root = page_scroll!(root, scroll::PageDown, |rows: i32| rows.max(1));
        let root = page_scroll!(root, scroll::PageUp, |rows: i32| -rows.max(1));
        let local = Rc::clone(&self.local);
        let root =
            scroll_command_action::<scroll::Top>(root, state, bridge, local, ScrollCommand::Top);
        let local = Rc::clone(&self.local);
        let root = scroll_command_action::<scroll::Bottom>(
            root,
            state,
            bridge,
            local,
            ScrollCommand::Bottom,
        );
        let select_local = Rc::clone(&self.local);
        let select_state = state.clone();
        let root = root.on_action(move |_: &scroll::StartSelection, _, cx| {
            let Some(grid) = popup_grid(&select_state, cx).filter(|grid| grid.rows > 0) else {
                return;
            };
            select_local.borrow_mut().start_line_selection(grid);
            select_state.update(cx, |_, cx| cx.notify());
        });
        let yank_local = Rc::clone(&self.local);
        let yank_state = state.clone();
        let root = root.on_action(move |_: &scroll::Yank, _, cx| {
            let text = {
                let local = yank_local.borrow();
                local
                    .anchor
                    .map(|anchor| selection_text(&local.history, anchor, local.caret))
            };
            if let Some(text) = text {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                yank_local.borrow_mut().clear_selections();
                yank_state.update(cx, |app, cx| {
                    app.toast_short("copied", fleet_ui_kit::Icon::ClipboardCheck, Instant::now());
                    cx.notify();
                });
            }
        });
        let exit_local = Rc::clone(&self.local);
        let exit_state = state.clone();
        let exit_bridge = bridge.clone();
        let root = root.on_action(move |_: &scroll::Exit, _, cx| {
            exit_scroll(&exit_local, &exit_state, &exit_bridge, cx)
        });
        let escape_local = Rc::clone(&self.local);
        let escape_state = state.clone();
        let escape_bridge = bridge.clone();
        let root = root.on_action(move |_: &scroll::Escape, _, cx| {
            if escape_local.borrow_mut().clear_selections() {
                escape_state.update(cx, |_, cx| cx.notify());
            } else {
                exit_scroll(&escape_local, &escape_state, &escape_bridge, cx);
            }
        });
        // Search is reserved in the authoritative table, matching Workspace's current v1 behavior.
        let root = reserved_search::<scroll::Search>(root, state);
        let root = reserved_search::<scroll::SearchNext>(root, state);
        reserved_search::<scroll::SearchPrev>(root, state)
    }
}

pub(super) fn send_literal(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &App,
) {
    if let Some(target) = popup_input_target(state.read(cx)) {
        send_or_queue(
            local,
            bridge,
            target,
            PendingInput::Key(KeyEvent {
                key: Key::Char('s'),
                mods: Modifiers::CTRL,
                text: None,
                action: KeyAction::Press,
            }),
        );
    }
}

pub(super) fn enter_scroll(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        let alt_screen = app
            .agent_popup_grid()
            .is_some_and(|grid| grid.modes.alt_screen);
        if let Some(popup) = &mut app.agent_popup {
            if alt_screen {
                popup.mode = AgentPopupMode::Terminal;
                app.toast_short(
                    "no scrollback in alt-screen",
                    fleet_ui_kit::Icon::ChevronsUp,
                    Instant::now(),
                );
            } else {
                popup.mode = AgentPopupMode::Scroll;
            }
        }
        cx.notify();
    });
}

pub(super) fn restart(state: &Entity<AppState>, bridge: &Bridge, cx: &App) {
    if let Some(terminal) = popup_terminal(state.read(cx)) {
        bridge.send(RequestBody::RestartTerminal { terminal });
    }
}

pub(super) fn hide(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    let preserve = state
        .read(cx)
        .active_session()
        .and_then(|session| session.active_terminal);
    detach_local(local, bridge, preserve);
    state.update(cx, |app, cx| {
        app.hide_agent_popup();
        cx.notify();
    });
}

pub(super) fn scroll_lines(state: &Entity<AppState>, bridge: &Bridge, lines: i32, cx: &mut App) {
    if let Some(terminal) = popup_terminal(state.read(cx)) {
        bridge.send(RequestBody::ScrollTerminal {
            terminal,
            scroll: ScrollCommand::Lines(lines),
        });
    }
    state.update(cx, |_, cx| cx.notify());
}

pub(super) fn step_line(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    delta: i32,
    cx: &mut App,
) {
    let (base, rows) =
        popup_grid(state, cx).map_or((0, 0), |grid| (viewport_base(grid), grid.rows));
    if local.borrow_mut().move_caret_within(delta, base, rows) {
        state.update(cx, |_, cx| cx.notify());
        return;
    }
    scroll_selection_lines(local, state, bridge, delta, cx);
}

pub(super) fn scroll_selection_lines(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    lines: i32,
    cx: &mut App,
) {
    local.borrow_mut().shift_caret(lines);
    scroll_lines(state, bridge, lines, cx);
}

pub(super) fn scroll_command_action<A: gpui::Action>(
    root: Div,
    state: &Entity<AppState>,
    bridge: &Bridge,
    local: Rc<RefCell<Local>>,
    command: ScrollCommand,
) -> Div {
    let state = state.clone();
    let bridge = bridge.clone();
    root.on_action(move |_: &A, _, cx| {
        match command {
            ScrollCommand::Top => local.borrow_mut().caret = 0,
            ScrollCommand::Bottom => local.borrow_mut().caret = u64::MAX,
            _ => {}
        }
        if let Some(terminal) = popup_terminal(state.read(cx)) {
            bridge.send(RequestBody::ScrollTerminal {
                terminal,
                scroll: command,
            });
        }
    })
}

pub(super) fn exit_scroll(
    local: &Rc<RefCell<Local>>,
    state: &Entity<AppState>,
    bridge: &Bridge,
    cx: &mut App,
) {
    local.borrow_mut().clear_selections();
    if let Some(terminal) = popup_terminal(state.read(cx)) {
        bridge.send(RequestBody::ScrollTerminal {
            terminal,
            scroll: ScrollCommand::Bottom,
        });
    }
    state.update(cx, |app, cx| {
        if let Some(popup) = &mut app.agent_popup {
            popup.mode = AgentPopupMode::Terminal;
        }
        cx.notify();
    });
}
