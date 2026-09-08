use super::*;

use crate::views::workspace_tabs::TabTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PendingSelectionScroll {
    terminal: TerminalId,
    history_epoch: u64,
    issued_seq: u64,
    issued_base: u64,
    target_base: u64,
    caret_lines: i32,
}

impl PendingSelectionScroll {
    pub(super) fn queue(
        slot: &mut Option<Self>,
        terminal: TerminalId,
        grid: &crate::state::MirrorGrid,
        lines: i32,
    ) {
        let retained = slot.as_ref().copied().filter(|pending| {
            pending.terminal == terminal && pending.history_epoch == grid.viewport.history_epoch
        });
        let issued_base = viewport_base(grid);
        let base = retained.map_or(issued_base, |pending| pending.target_base);
        let limit = grid.viewport.scrollback_len as u64;
        let target_base = shifted_viewport_base(base, limit, lines);
        let moved = viewport_delta(base, target_base);
        if moved == 0 && retained.is_none() {
            *slot = None;
            return;
        }
        let caret_lines = retained
            .map_or(0, |pending| pending.caret_lines)
            .saturating_add(moved);
        *slot = Some(Self {
            terminal,
            history_epoch: grid.viewport.history_epoch,
            issued_seq: grid.seq,
            issued_base,
            target_base,
            caret_lines,
        });
    }

    pub(super) fn reconcile(
        &mut self,
        terminal: TerminalId,
        grid: &crate::state::MirrorGrid,
    ) -> Option<i32> {
        if self.terminal != terminal || self.history_epoch != grid.viewport.history_epoch {
            return Some(0);
        }
        if grid.seq <= self.issued_seq {
            return None;
        }
        let arrived_base = viewport_base(grid);
        if arrived_base == self.target_base {
            return Some(self.caret_lines);
        }
        if arrived_base == self.issued_base {
            return None;
        }
        Some(viewport_delta(self.issued_base, arrived_base))
    }
}

fn viewport_delta(from: u64, to: u64) -> i32 {
    let delta = to as i128 - from as i128;
    match i32::try_from(delta) {
        Ok(delta) => delta,
        Err(_) if delta < 0 => i32::MIN,
        Err(_) => i32::MAX,
    }
}

fn shifted_viewport_base(base: u64, limit: u64, lines: i32) -> u64 {
    if lines >= 0 {
        base.saturating_add(u64::from(lines.unsigned_abs()))
            .min(limit)
    } else {
        base.saturating_sub(u64::from(lines.unsigned_abs()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExpectedResponse {
    Ack,
    Terminal,
    Session,
}

fn response_matches(expected: ExpectedResponse, response: &ResponseBody) -> bool {
    matches!(
        (expected, response),
        (ExpectedResponse::Ack, ResponseBody::Ack)
            | (ExpectedResponse::Terminal, ResponseBody::Terminal(_))
            | (ExpectedResponse::Session, ResponseBody::Session(_))
    )
}

pub(super) fn mutation_failure(
    answer: Result<Result<ResponseBody, fleet_proto::error::ProtoError>, async_channel::RecvError>,
    expected: ExpectedResponse,
    operation: &str,
) -> Option<String> {
    match answer {
        Ok(Err(error)) => Some(error.message),
        Err(_) => Some(format!("could not {operation}: daemon reply was lost")),
        Ok(Ok(response)) if response_matches(expected, &response) => None,
        Ok(Ok(_)) => Some(format!(
            "could not {operation}: daemon returned an unexpected response"
        )),
    }
}

fn show_sticky_error(state: &Entity<AppState>, text: String, cx: &mut App) {
    state.update(cx, |app, cx| {
        record_mutation_failure(app, text);
        cx.notify();
    });
}

pub(super) fn record_mutation_failure(app: &mut AppState, text: String) {
    app.sticky_error = Some(crate::state::StickyError {
        text,
        job: None,
        retryable: false,
    });
}

pub(super) trait MutationRequester {
    fn request(
        &self,
        body: RequestBody,
    ) -> async_channel::Receiver<Result<ResponseBody, fleet_proto::error::ProtoError>>;
}

impl MutationRequester for Bridge {
    fn request(
        &self,
        body: RequestBody,
    ) -> async_channel::Receiver<Result<ResponseBody, fleet_proto::error::ProtoError>> {
        Bridge::request(self, body)
    }
}

#[derive(Debug)]
pub(super) struct MutationRequest {
    pub(super) body: RequestBody,
    pub(super) expected: ExpectedResponse,
    pub(super) operation: &'static str,
}

impl MutationRequest {
    fn close(terminal: TerminalId) -> Self {
        Self {
            body: RequestBody::CloseTerminal { terminal },
            expected: ExpectedResponse::Ack,
            operation: "close terminal",
        }
    }

    pub(super) fn restart(session: &Session) -> Option<Self> {
        Some(Self {
            body: RequestBody::RestartTerminal {
                terminal: active_pty_terminal_of(session)?,
            },
            expected: ExpectedResponse::Terminal,
            operation: "restart terminal",
        })
    }

    fn select(session: SessionId, terminal: TerminalId) -> Self {
        Self {
            body: RequestBody::SelectTerminal { session, terminal },
            expected: ExpectedResponse::Session,
            operation: "select terminal",
        }
    }
}

pub(super) fn request_mutation(
    requester: &impl MutationRequester,
    request: MutationRequest,
    state: Entity<AppState>,
    cx: &mut App,
) {
    let MutationRequest {
        body,
        expected,
        operation,
    } = request;
    let reply = requester.request(body);
    cx.spawn(async move |cx| {
        let failure = mutation_failure(reply.recv().await, expected, operation);
        if let Some(error) = failure {
            cx.update(|cx| show_sticky_error(&state, error, cx));
        }
    })
    .detach();
}

pub(super) fn session_intent_is_current(expected: &SessionId, current: Option<&SessionId>) -> bool {
    current == Some(expected)
}

pub(super) fn new_terminal_reply_target(
    originating_session: &SessionId,
    current_session: Option<&SessionId>,
    response: &ResponseBody,
) -> Option<TerminalId> {
    if !session_intent_is_current(originating_session, current_session) {
        return None;
    }
    match response {
        ResponseBody::Terminal(terminal) => Some(terminal.id),
        _ => None,
    }
}

pub(super) fn active_pty_terminal_of(session: &Session) -> Option<TerminalId> {
    let active = session.active_terminal?;
    session
        .terminals
        .iter()
        .find(|terminal| terminal.id == active && !terminal.is_native())
        .map(|terminal| terminal.id)
}

impl WorkspaceScreen {
    /// Every `ctrl-s <key>` binding the shell does not already own.
    pub(super) fn with_prefix_actions(
        &self,
        root: Div,
        bridge: &Bridge,
        state: &Entity<AppState>,
    ) -> Div {
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::SendLiteral, _window, cx| {
                if let Some((terminal, primed)) = terminal_input_target(&state, cx) {
                    if local.borrow_mut().clear_selections() {
                        state.update(cx, |_, cx| cx.notify());
                    }
                    let accepted = local.borrow_mut().send_or_queue(
                        &bridge,
                        Some(terminal),
                        primed,
                        PendingInput::Key(KeyEvent {
                            key: Key::Char('s'),
                            mods: Modifiers::CTRL,
                            text: None,
                            action: KeyAction::Press,
                        }),
                    );
                    surface::report_input_delivery(accepted, &state, cx);
                }
            })
        };
        let root = {
            let (local, bridge, _) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::GoHub, _window, cx| {
                // The session keeps running in fleetd; only this client's attachment ends.
                local.borrow_mut().detach(&bridge, None);
                // The shell owns the screen change, so this listener hands the action back.
                cx.propagate();
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::SleepAndGoHub, _window, cx| {
                let session = state.read(cx).active_session().map(|s| s.id.clone());
                let Some(session) = session else { return };
                let reply = bridge.request(RequestBody::SleepSession {
                    session: session.clone(),
                });
                let state = state.clone();
                let bridge = bridge.clone();
                let local = Rc::clone(&local);
                cx.spawn(async move |cx| {
                    let answer = reply.recv().await;
                    cx.update(|cx| match answer {
                        Ok(Ok(ResponseBody::Slept(_))) => {
                            let current = state
                                .read(cx)
                                .active_session()
                                .map(|active| active.id.clone());
                            if !session_intent_is_current(&session, current.as_ref()) {
                                return;
                            }
                            local.borrow_mut().detach(&bridge, None);
                            state.update(cx, |app, cx| {
                                app.leave_prefix();
                                app.screen = Screen::hub();
                                cx.notify();
                            });
                        }
                        Ok(Err(error)) => show_sticky_error(&state, error.message, cx),
                        Ok(Ok(_)) => show_sticky_error(
                            &state,
                            "could not sleep session: daemon returned an unexpected response"
                                .to_owned(),
                            cx,
                        ),
                        Err(_) => show_sticky_error(
                            &state,
                            "could not sleep session: daemon reply was lost".to_owned(),
                            cx,
                        ),
                    });
                })
                .detach();
            })
        };
        let root = {
            let state = state.clone();
            root.on_action(move |_: &prefix::ToggleWatchPane, _, cx| {
                crate::views::watch_pane::toggle(&state, cx)
            })
        };
        let root = {
            let state = state.clone();
            let bridge = bridge.clone();
            root.on_action(move |_: &prefix::DismissWatch, _, cx| {
                crate::views::watch_pane::dismiss_selected(&state, &bridge, cx)
            })
        };
        let root = {
            let state = state.clone();
            root.on_action(move |_: &prefix::NextWatch, _, cx| {
                state.update(cx, |app, cx| {
                    app.cycle_watch(true, Instant::now());
                    cx.notify();
                });
            })
        };
        let root = {
            let state = state.clone();
            root.on_action(move |_: &prefix::PrevWatch, _, cx| {
                state.update(cx, |app, cx| {
                    app.cycle_watch(false, Instant::now());
                    cx.notify();
                });
            })
        };
        let root = self.tab_actions(root, bridge, state);
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::NewTerminal, _window, cx| {
                let Some(request) = state.read(cx).active_session().map(shell_tab_request) else {
                    return;
                };
                request_shell_tab(&local, request, &bridge, &state, cx);
            })
        };
        let root = {
            let (_, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::CloseTerminal, _window, cx| {
                let Some(terminal) = active_terminal_record(&state, cx) else {
                    return;
                };
                if terminal.keep_alive.is_empty() {
                    request_mutation(
                        &bridge,
                        MutationRequest::close(terminal.id),
                        state.clone(),
                        cx,
                    );
                } else {
                    let index = state
                        .read(cx)
                        .active_session()
                        .and_then(|session| {
                            session
                                .terminals
                                .iter()
                                .position(|candidate| candidate.id == terminal.id)
                        })
                        .map_or(1, |index| index + 1);
                    dialogs::request_confirm(
                        cx,
                        dialogs::ConfirmRequest::CloseTerminal {
                            terminal: terminal.id,
                            index,
                            name: terminal.name,
                            running: terminal.foreground_command,
                        },
                    );
                    state.update(cx, |app, cx| {
                        app.leave_prefix();
                        app.open_overlay(Overlay::Dialog(Dialogs::Confirm));
                        cx.notify();
                    });
                }
            })
        };
        let root = {
            let (_, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::RestartCommand, _window, cx| {
                let request = state
                    .read(cx)
                    .active_session()
                    .and_then(MutationRequest::restart);
                if let Some(request) = request {
                    request_mutation(&bridge, request, state.clone(), cx);
                }
            })
        };
        let root = {
            let (_, _, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::RenameTerminal, _window, cx| {
                state.update(cx, |app, cx| {
                    app.leave_prefix();
                    app.open_overlay(Overlay::Dialog(Dialogs::RenameTerminal));
                    cx.notify();
                });
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::Paste, _window, cx| {
                paste_clipboard(&local, &bridge, &state, cx);
            })
        };
        let root = {
            let (_, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::CopyWorktreePath, _window, cx| {
                let Some(worktree) = active_worktree_id(&state, cx) else {
                    return;
                };
                let reply = bridge.request(RequestBody::WorktreePath { id: worktree });
                let state = state.clone();
                cx.spawn(async move |cx| {
                    let answer = reply.recv().await;
                    cx.update(|cx| match answer {
                        Ok(Ok(ResponseBody::Path(path))) => {
                            cx.write_to_clipboard(ClipboardItem::new_string(path));
                            state.update(cx, |app, cx| {
                                app.toast_short(
                                    "path copied",
                                    Icon::ClipboardCheck,
                                    Instant::now(),
                                );
                                cx.notify();
                            });
                        }
                        Ok(Err(error)) => show_sticky_error(&state, error.message, cx),
                        Ok(Ok(_)) => show_sticky_error(
                            &state,
                            "could not copy worktree path: daemon returned an unexpected response"
                                .to_owned(),
                            cx,
                        ),
                        Err(_) => show_sticky_error(
                            &state,
                            "could not copy worktree path: daemon reply was lost".to_owned(),
                            cx,
                        ),
                    });
                })
                .detach();
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::LastSession, _window, cx| {
                let alternate = state.read(cx).session_mru.alternate().cloned();
                match alternate {
                    Some(session) => {
                        local.borrow_mut().detach(&bridge, None);
                        open_session(&state, session, cx);
                    }
                    None => state.update(cx, |app, cx| {
                        app.toast_short("no other session", Icon::Info, Instant::now());
                        cx.notify();
                    }),
                }
            })
        };
        {
            let (_, _, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::SessionSwitcher, _window, cx| {
                state.update(cx, |app, cx| {
                    app.leave_prefix();
                    app.palette_seed = Some("sessions".to_owned());
                    app.open_overlay(Overlay::Palette);
                    cx.notify();
                });
            })
        }
    }

    /// `ctrl-s 1`–`9`, `h` / `l`, `p` / `n` and `Tab`.
    pub(super) fn tab_actions(&self, root: Div, bridge: &Bridge, state: &Entity<AppState>) -> Div {
        macro_rules! select_tab {
            ($root:expr, $action:ty, $position:expr) => {{
                let (local, bridge, state) = self.handles(bridge, state);
                $root.on_action(move |_: &$action, _window, cx| {
                    let target = {
                        let app = state.read(cx);
                        app.active_session().and_then(|session| {
                            let agents = threads_of(app, session);
                            workspace_tabs::target_at(session, &agents, $position)
                        })
                    };
                    select_target(&local, &bridge, &state, target, cx);
                })
            }};
        }
        let root = select_tab!(root, prefix::SelectTab1, 0);
        let root = select_tab!(root, prefix::SelectTab2, 1);
        let root = select_tab!(root, prefix::SelectTab3, 2);
        let root = select_tab!(root, prefix::SelectTab4, 3);
        let root = select_tab!(root, prefix::SelectTab5, 4);
        let root = select_tab!(root, prefix::SelectTab6, 5);
        let root = select_tab!(root, prefix::SelectTab7, 6);
        let root = select_tab!(root, prefix::SelectTab8, 7);
        let root = select_tab!(root, prefix::SelectTab9, 8);

        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::PrevTab, _window, cx| {
                let target = neighbour_tab(&state, -1, cx);
                select_target(&local, &bridge, &state, target, cx);
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &prefix::NextTab, _window, cx| {
                let target = neighbour_tab(&state, 1, cx);
                select_target(&local, &bridge, &state, target, cx);
            })
        };
        let (local, bridge, state) = self.handles(bridge, state);
        root.on_action(move |_: &prefix::LastTab, _window, cx| {
            let terminal = {
                let app = state.read(cx);
                app.active_session().and_then(|session| {
                    app.terminal_mru
                        .get(&session.id)
                        .and_then(|mru| mru.alternate().copied())
                })
            };
            select_terminal(&local, &bridge, &state, terminal, cx);
        })
    }

    /// Scroll mode: viewport movement, the line-wise selection and the yank.
    pub(super) fn with_scroll_actions(
        &self,
        root: Div,
        bridge: &Bridge,
        state: &Entity<AppState>,
    ) -> Div {
        macro_rules! terminal_scroll {
            ($root:expr, $action:ty, $command:expr, $key:expr, $mods:expr) => {{
                let (_, bridge, state) = self.handles(bridge, state);
                $root.on_action(move |_: &$action, _, cx| {
                    let app = state.read(cx);
                    if app.terminal_mode != TerminalMode::Terminal || app.drops_terminal_keys() {
                        return;
                    }
                    let Some(terminal) = app
                        .active_session()
                        .and_then(|session| session.active_terminal)
                    else {
                        return;
                    };
                    bridge.send(RequestBody::ScrollOrKeyTerminal {
                        terminal,
                        scroll: $command,
                        key: KeyEvent {
                            key: $key,
                            mods: $mods,
                            text: None,
                            action: KeyAction::Press,
                        },
                    });
                })
            }};
        }
        let root = terminal_scroll!(
            root,
            scroll::TerminalPageUp,
            ScrollCommand::Pages(-1),
            Key::PageUp,
            Modifiers::SHIFT
        );
        let root = terminal_scroll!(
            root,
            scroll::TerminalPageDown,
            ScrollCommand::Pages(1),
            Key::PageDown,
            Modifiers::SHIFT
        );
        let root = terminal_scroll!(
            root,
            scroll::TerminalTop,
            ScrollCommand::Top,
            Key::Home,
            Modifiers::SUPER
        );
        let root = terminal_scroll!(
            root,
            scroll::TerminalBottom,
            ScrollCommand::Bottom,
            Key::End,
            Modifiers::SUPER
        );
        // `j` and `k` move the caret first and only scroll once it is against an edge, which is
        // what lets a selection be extended without the text moving under the eyes.
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::LineDown, _window, cx| {
                step_line(&local, &bridge, &state, 1, cx);
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::LineUp, _window, cx| {
                step_line(&local, &bridge, &state, -1, cx);
            })
        };
        // Every page motion moves the caret by the same number of lines. Without that the
        // viewport slides out from under a live selection and the anchor stops being reachable,
        // which is what made a multi-page selection impossible.
        macro_rules! scroll_pages {
            ($root:expr, $action:ty, $down:expr) => {{
                let (local, bridge, state) = self.handles(bridge, state);
                $root.on_action(move |_: &$action, _window, cx| {
                    let page = i32::from(visible_rows(&state, cx)).max(1);
                    let lines = if $down { page } else { -page };
                    scroll_lines(&local, &bridge, &state, lines, cx);
                })
            }};
        }
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::HalfPageDown, _window, cx| {
                let half = half_page(&state, cx);
                scroll_lines(&local, &bridge, &state, half, cx);
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::HalfPageUp, _window, cx| {
                let half = half_page(&state, cx);
                scroll_lines(&local, &bridge, &state, -half, cx);
            })
        };
        let root = scroll_pages!(root, scroll::PageDown, true);
        let root = scroll_pages!(root, scroll::PageUp, false);
        // `g` and `G` jump somewhere the caret cannot be derived from a delta. Parking it at
        // either extreme lets the next frame clamp it onto the new viewport.
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::Top, _window, cx| {
                let mut local = local.borrow_mut();
                local.state.pending_selection_scroll = None;
                local.caret = 0;
                drop(local);
                scroll_viewport(&bridge, &state, ScrollCommand::Top, cx);
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::Bottom, _window, cx| {
                let mut local = local.borrow_mut();
                local.state.pending_selection_scroll = None;
                local.caret = u64::MAX;
                drop(local);
                scroll_viewport(&bridge, &state, ScrollCommand::Bottom, cx);
            })
        };

        let root = {
            let (local, _, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::StartSelection, _window, cx| {
                if let Some(grid) = state.read(cx).active_grid() {
                    let mut borrowed = local.borrow_mut();
                    borrowed.start_line_selection(grid);
                    borrowed.row_caches.clear();
                }
                state.update(cx, |_, cx| cx.notify());
            })
        };
        let root = {
            let (local, _, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::Yank, _window, cx| {
                let selection = {
                    let borrowed = local.borrow();
                    borrowed
                        .anchor
                        .map(|anchor| try_selection_text(&borrowed.history, anchor, borrowed.caret))
                };
                let Some(selection) = selection else {
                    return;
                };
                let Some(text) = selection else {
                    state.update(cx, |app, cx| {
                        app.toast_short("selection scrolled away", Icon::Info, Instant::now());
                        cx.notify();
                    });
                    return;
                };
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                local.borrow_mut().clear_line_selection();
                state.update(cx, |app, cx| {
                    app.toast_short("copied", Icon::ClipboardCheck, Instant::now());
                    cx.notify();
                });
            })
        };
        // `Esc` clears a selection first and only then leaves the mode, so a mistyped `v` costs
        // one key rather than a re-entry into Scroll.
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::Escape, _window, cx| {
                if local.borrow_mut().clear_line_selection() {
                    state.update(cx, |_, cx| cx.notify());
                    return;
                }
                exit_scroll(&local, &bridge, &state, cx);
            })
        };
        let root = {
            let (local, bridge, state) = self.handles(bridge, state);
            root.on_action(move |_: &scroll::Exit, _window, cx| {
                exit_scroll(&local, &bridge, &state, cx);
            })
        };
        let root = reserved_search::<scroll::Search>(root, state);
        let root = reserved_search::<scroll::SearchNext>(root, state);
        reserved_search::<scroll::SearchPrev>(root, state)
    }
}

/// Asks fleetd for a plain shell tab in the session's worktree path.
///
/// `ctrl-s c` and the `+` at the end of the strip are the same request, so they share this.
pub(super) fn request_shell_tab(
    local: &Rc<RefCell<Local>>,
    request: RequestBody,
    bridge: &Bridge,
    state: &Entity<AppState>,
    cx: &mut App,
) {
    let originating_session = match &request {
        RequestBody::NewTerminal { session, .. } => session.clone(),
        _ => return,
    };
    let reply = bridge.request(request);
    let state = state.clone();
    let bridge = bridge.clone();
    let local = Rc::clone(local);
    cx.spawn(async move |cx| {
        match reply.recv().await {
            // §3.6: the tab the user just asked for is the tab they are about to type into.
            // Selecting it is what moves the blue underline *and* what makes the daemon clear
            // its unseen-output dot; without it the next keystrokes go to the previous PTY.
            Ok(Ok(response @ ResponseBody::Terminal(_))) => cx.update(|cx| {
                let current = state
                    .read(cx)
                    .active_session()
                    .map(|session| session.id.clone());
                if let Some(terminal) =
                    new_terminal_reply_target(&originating_session, current.as_ref(), &response)
                {
                    select_terminal(&local, &bridge, &state, Some(terminal), cx);
                }
            }),
            // A refused request is sticky, never silent (§1.8) — this key used to fail quietly.
            Ok(Err(error)) => cx.update(|cx| {
                state.update(cx, |app, cx| {
                    app.sticky_error = Some(crate::state::StickyError {
                        text: error.message,
                        job: None,
                        retryable: false,
                    });
                    cx.notify();
                });
            }),
            Ok(Ok(_)) => cx.update(|cx| {
                show_sticky_error(
                    &state,
                    "could not create terminal: daemon returned an unexpected response".to_owned(),
                    cx,
                );
            }),
            Err(_) => cx.update(|cx| {
                show_sticky_error(
                    &state,
                    "could not create terminal: daemon reply was lost".to_owned(),
                    cx,
                );
            }),
        }
    })
    .detach();
}

/// Moves the viewport and repaints.
pub(super) fn scroll_viewport(
    bridge: &Bridge,
    state: &Entity<AppState>,
    command: ScrollCommand,
    cx: &mut App,
) {
    if let Some(terminal) = active_terminal(state, cx) {
        bridge.send(RequestBody::ScrollTerminal {
            terminal,
            scroll: command,
        });
    }
    state.update(cx, |_, cx| cx.notify());
}

/// `j` / `k`: move the caret, and scroll only once it is against an edge.
pub(super) fn step_line(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    delta: i32,
    cx: &mut App,
) {
    let (base, rows) = viewport_span(state, cx);
    if local.borrow_mut().move_caret_within(delta, base, rows) {
        state.update(cx, |_, cx| cx.notify());
        return;
    }
    // The caret is against an edge: the viewport moves under it instead, and the caret goes
    // with it so a selection keeps extending line by line.
    scroll_lines(local, bridge, state, delta, cx);
}

/// The absolute line the viewport's top row shows, and how many rows it has.
pub(super) fn viewport_span(state: &Entity<AppState>, cx: &App) -> (u64, u16) {
    state
        .read(cx)
        .active_grid()
        .map_or((0, 0), |grid| (viewport_base(grid), grid.rows))
}

/// Scrolls by whole lines and carries the caret along, so a live selection grows with the view.
pub(super) fn scroll_lines(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    lines: i32,
    cx: &mut App,
) {
    {
        let app = state.read(cx);
        if let Some(session) = app.active_session()
            && let Some(terminal) = active_pty_terminal_of(session)
            && let Some(grid) = app.grids.get(&terminal)
        {
            PendingSelectionScroll::queue(
                &mut local.borrow_mut().state.pending_selection_scroll,
                terminal,
                grid,
                lines,
            );
        }
    }
    scroll_viewport(bridge, state, ScrollCommand::Lines(lines), cx);
}

/// Leaves Scroll mode: the viewport snaps back to the live bottom (KEYMAP §Scroll).
pub(super) fn exit_scroll(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    cx: &mut App,
) {
    {
        let mut local = local.borrow_mut();
        local.clear_line_selection();
        local.state.pending_selection_scroll = None;
    }
    if let Some(terminal) = active_terminal(state, cx) {
        bridge.send(RequestBody::ScrollTerminal {
            terminal,
            scroll: ScrollCommand::Bottom,
        });
    }
    state.update(cx, |app, cx| {
        if app.terminal_mode == TerminalMode::Scroll {
            app.terminal_mode = app.resting_terminal_mode();
        }
        cx.notify();
    });
}

/// The active terminal of the active session.
pub(super) fn active_terminal(state: &Entity<AppState>, cx: &App) -> Option<TerminalId> {
    state
        .read(cx)
        .active_session()
        .and_then(|session| session.active_terminal)
}

/// The active terminal's record, cloned so the state borrow ends before the caller mutates.
pub(super) fn active_terminal_record(state: &Entity<AppState>, cx: &App) -> Option<Terminal> {
    let app = state.read(cx);
    let session = app.active_session()?;
    let active = session.active_terminal?;
    session
        .terminals
        .iter()
        .find(|terminal| terminal.id == active)
        .cloned()
}

/// The worktree the open session belongs to, when it is a worktree session.
pub(super) fn active_worktree_id(state: &Entity<AppState>, cx: &App) -> Option<WorktreeId> {
    match &state.read(cx).active_session()?.kind {
        SessionKind::Worktree(id) => Some(id.clone()),
        SessionKind::Agent(_) => None,
    }
}

/// How many rows the mirror currently holds.
pub(super) fn visible_rows(state: &Entity<AppState>, cx: &App) -> u16 {
    state.read(cx).active_grid().map_or(0, |grid| grid.rows)
}

/// Half a page of the current grid, never zero, as `ctrl-d` / `ctrl-u` move.
pub(super) fn half_page(state: &Entity<AppState>, cx: &App) -> i32 {
    i32::from(visible_rows(state, cx) / 2).max(1)
}

/// The strip position `ctrl-s h` / `ctrl-s l` moves to, across terminals and agent threads.
pub(super) fn neighbour_tab(state: &Entity<AppState>, delta: isize, cx: &App) -> Option<TabTarget> {
    let app = state.read(cx);
    let session = app.active_session()?;
    let agents = threads_of(app, session);
    let active = match &session.kind {
        SessionKind::Worktree(worktree) => app
            .agents
            .active(worktree)
            .map(TabTarget::Agent)
            .or_else(|| session.active_terminal.map(TabTarget::Terminal)),
        SessionKind::Agent { .. } => session.active_terminal.map(TabTarget::Terminal),
    };
    workspace_tabs::neighbour_target(session, &agents, active, delta)
}

/// The terminal `ctrl-s h` / `ctrl-s l` moves to.
pub(super) fn neighbour_terminal(
    state: &Entity<AppState>,
    delta: isize,
    cx: &App,
) -> Option<TerminalId> {
    let app = state.read(cx);
    let session = app.active_session()?;
    workspace_tabs::neighbour(session, session.active_terminal, delta)
}

/// Selects a terminal in the daemon and records it in the session's MRU.
pub(super) fn select_terminal(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    terminal: Option<TerminalId>,
    cx: &mut App,
) {
    let Some(terminal) = terminal else {
        return;
    };
    let Some((session, native)) = state.read(cx).active_session().map(|session| {
        let native = session
            .terminals
            .iter()
            .any(|entry| entry.id == terminal && entry.is_native());
        (session.id.clone(), native)
    }) else {
        return;
    };
    request_mutation(
        bridge,
        MutationRequest::select(session.clone(), terminal),
        state.clone(),
        cx,
    );
    local.borrow_mut().clear_line_selection();
    state.update(cx, |app, cx| {
        app.touch_terminal(&session, terminal);
        // The snapshot decides which tab is really active; this only keeps the mode honest
        // while the round trip is in flight. The kind is read from the session record we
        // already have, so switching to a `fleet://` tab does not flash a Terminal frame.
        app.terminal_mode = if native {
            TerminalMode::Native
        } else {
            TerminalMode::Terminal
        };
        cx.notify();
    });
}

/// Switches the Workspace to another session, sleeping nothing.
pub(super) fn open_session(state: &Entity<AppState>, session: SessionId, cx: &mut App) {
    dialogs::open_session(session, state, cx);
}

/// `ctrl-s c` and the `+` both open a plain shell, so the first one is `sh`, then `sh2`, `sh3`.
pub(super) fn shell_tab_request(session: &Session) -> RequestBody {
    RequestBody::NewTerminal {
        session: session.id.clone(),
        name: workspace_tabs::unique_terminal_name(session, "sh"),
        command: SHELL_TAB_COMMAND.to_owned(),
        cwd: session.cwd.clone(),
    }
}
