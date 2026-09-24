use super::*;

use fleet_ui_kit::{Button, ButtonSize, ButtonStyle, HarnessTargetExt as _, Kbd, Menu, MenuItem};

use crate::{
    actions::native_agent, presentation::workspace_keys, views::workspace_tabs::TabTarget,
};

impl WorkspaceScreen {
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
        let watch = app.watches.pane_visible(&session.id);
        let agents = threads_of(app, session);
        let keys = workspace_keys();
        let mut local = self.local.borrow_mut();
        let labels = &mut local.state.tab_labels;
        let mut tabs = labels.tabs(session, status, model.terminal, &app.renamed_terminals);
        let first_agent = tabs.len();
        let selected = active_target(model);
        labels.retain_agents(&agents);
        for (offset, summary) in agents.iter().enumerate() {
            let active = selected == Some(workspace_tabs::TabTarget::Agent(summary.thread));
            tabs.push(labels.agent_tab(
                summary,
                first_agent + offset + 1,
                app.agents.attention(summary.thread),
                active,
                app.agents.summaries_revision(),
            ));
        }
        drop(local);
        // `ctrl-s 1`–`9` is in each tab's tooltip, beside the index it no longer paints.
        let tabs = tabs.into_iter().enumerate().map(|(position, tab)| {
            let kbd = keys.select_tab.get(position).cloned().flatten();
            tab.kbd(kbd)
        });
        let active = workspace_tabs::active_position(session, &agents, selected);

        let (select_local, select_bridge, select_state) = self.handles(bridge, state);
        let (close_local, close_bridge, close_state) = self.handles(bridge, state);
        let (menu_local, menu_bridge, menu_state) = self.handles(bridge, state);
        let (new_local, new_bridge, new_state) = self.handles(bridge, state);
        let on_agent = model.agent.is_some();
        let mut strip = TerminalTabStrip::new(tabs)
            .active(active)
            // Terminals first, then one tab per agent thread — the same split
            // `state/harness/projection.rs` reports, so `focused` and a click name the same tab.
            .agents_from(first_agent)
            // Mouse parity for `ctrl-s 1`-`9` (§3.6); a right-click selects too.
            .on_select(move |position, _window, cx| {
                let target = target_at_position(&select_state, position, cx);
                select_target(&select_local, &select_bridge, &select_state, target, cx);
            })
            // Mouse parity for `ctrl-s x`: the `✕` and a middle-click.
            .on_close(move |position, window, cx| {
                let target = target_at_position(&close_state, position, cx);
                close_target(
                    &close_local,
                    &close_bridge,
                    &close_state,
                    target,
                    window,
                    cx,
                );
            })
            .close_kbd(keys.close.clone())
            .tab_menu(move |position, menu, _window, cx| {
                tab_menu(position, menu, &menu_local, &menu_bridge, &menu_state, cx)
            })
            .new_menu(move |menu, _window, cx| {
                new_tab_menu(menu, on_agent, &new_local, &new_bridge, &new_state, cx)
            });
        // The agent's own escape hatch: the same thread as a PTY in the popup (`ctrl-s F`).
        if on_agent {
            strip = strip.trailing(
                strip_button("tabs-fallback", "Open as terminal", keys.fallback.clone())
                    .action(Box::new(native_agent::TerminalFallback))
                    .harness_target("tabs.fallback"),
            );
        }
        // Watch exists only while the session has a subagent watch to show.
        if let Some(visible) = watch {
            strip = strip.trailing(
                strip_button("tabs-watch", "Watch", keys.watch.clone())
                    .selected(visible)
                    .action(Box::new(prefix::ToggleWatchPane))
                    .harness_target("tabs.watch"),
            );
        }
        // Changes exists for a worktree session: it reads what that worktree changed.
        if model.worktree.is_some() {
            strip = strip.trailing(
                strip_button("tabs-changes", "Changes", keys.changes.clone())
                    .selected(model.changes.is_some())
                    .action(Box::new(prefix::ToggleChanges))
                    .harness_target("tabs.changes"),
            );
        }
        strip.into_any_element()
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

/// A toggle at the right end of the tab strip: a compact ghost button showing its `⌃S` key.
fn strip_button(id: &'static str, label: &'static str, kbd: Option<Kbd>) -> Button {
    let button = Button::new(id, label)
        .style(ButtonStyle::Ghost)
        .size(ButtonSize::Compact);
    match kbd {
        Some(kbd) => button.kbd(kbd),
        None => button,
    }
}

/// What the strip's position `position` names right now.
fn target_at_position(state: &Entity<AppState>, position: usize, cx: &App) -> Option<TabTarget> {
    let app = state.read(cx);
    let session = app.active_session()?;
    let agents = threads_of(app, session);
    workspace_tabs::target_at(session, &agents, position)
}

/// Closes one tab, whichever kind it is and whether or not it is the one on screen, by the
/// rules `ctrl-s x` applies to the active one.
fn close_target(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    target: Option<TabTarget>,
    window: &mut Window,
    cx: &mut App,
) {
    match target {
        Some(TabTarget::Terminal(terminal)) => {
            let record = state.read(cx).active_session().and_then(|session| {
                session
                    .terminals
                    .iter()
                    .find(|candidate| candidate.id == terminal)
                    .cloned()
            });
            if let Some(record) = record {
                close_terminal(record, bridge, state, cx);
            }
        }
        Some(TabTarget::Agent(thread)) => {
            if state.read(cx).active_agent_thread() == Some(thread) {
                // The tab on screen closes exactly as its key closes it.
                window.dispatch_action(Box::new(native_agent::CloseTab), cx);
            } else {
                super::agent::requests::close_background_agent_tab(thread, bridge, state, cx);
            }
        }
        None => {}
    }
    // Closing is not selecting: whatever the reader had open stays open, and the one line of
    // selection state a closed terminal could leave behind goes with it.
    local.borrow_mut().clear_line_selection();
}

/// *Close others*: every other tab closes as its `✕` would, except a terminal whose keep-alive
/// process is running — that one would need its own confirm, so it stays and a toast says so.
fn close_other_tabs(
    keep: usize,
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let (targets, kept) = {
        let app = state.read(cx);
        let Some(session) = app.active_session() else {
            return;
        };
        let agents = threads_of(app, session);
        let mut kept = 0usize;
        let targets: Vec<TabTarget> = workspace_tabs::targets(session, &agents)
            .into_iter()
            .enumerate()
            .filter(|(position, _)| *position != keep)
            .map(|(_, target)| target)
            .filter(|target| match target {
                TabTarget::Terminal(id) => {
                    let running = session
                        .terminals
                        .iter()
                        .any(|terminal| terminal.id == *id && !terminal.keep_alive.is_empty());
                    kept += usize::from(running);
                    !running
                }
                TabTarget::Agent(_) => true,
            })
            .collect();
        (targets, kept)
    };
    for target in targets {
        close_target(local, bridge, state, Some(target), window, cx);
    }
    if kept > 0 {
        state.update(cx, |app, cx| {
            let tabs = if kept == 1 { "tab" } else { "tabs" };
            app.toast_short(
                format!("kept {kept} {tabs} open: each runs a process"),
                Icon::Info,
                Instant::now(),
            );
            cx.notify();
        });
    }
}

/// A tab's right-click menu: *Rename*, *Restart command* (an exited PTY only), *Close* and
/// *Close others*, each with its key. The right-click has already selected the tab.
fn tab_menu(
    position: usize,
    menu: Menu,
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    cx: &mut gpui::Context<Menu>,
) -> Menu {
    let keys = workspace_keys();
    let (target, tabs, exited) = {
        let app = state.read(cx);
        let Some(session) = app.active_session() else {
            return menu;
        };
        let agents = threads_of(app, session);
        let target = workspace_tabs::target_at(session, &agents, position);
        let exited = match target {
            Some(TabTarget::Terminal(id)) => session.terminals.iter().any(|terminal| {
                terminal.id == id
                    && !terminal.is_native()
                    && matches!(terminal.status, TerminalStatus::Exited { .. })
            }),
            _ => false,
        };
        (target, session.terminals.len() + agents.len(), exited)
    };
    let Some(target) = target else {
        return menu;
    };
    let with_kbd = |item: MenuItem, kbd: &Option<Kbd>| match kbd.clone() {
        Some(kbd) => item.kbd(kbd),
        None => item,
    };
    let mut menu = menu;
    if let TabTarget::Terminal(terminal) = target {
        menu = menu.item(with_kbd(
            MenuItem::new("Rename")
                .icon(Icon::SquarePen)
                .action(Box::new(prefix::RenameTerminal)),
            &keys.rename,
        ));
        if exited {
            let (bridge, state) = (bridge.clone(), state.clone());
            menu = menu.item(with_kbd(
                MenuItem::new("Restart command")
                    .icon(Icon::RefreshCw)
                    .on_select(move |_, cx| restart_terminal(terminal, &bridge, &state, cx)),
                &keys.restart,
            ));
        }
    }
    let close = {
        let (local, bridge, state) = (Rc::clone(local), bridge.clone(), state.clone());
        move |window: &mut Window, cx: &mut App| {
            close_target(&local, &bridge, &state, Some(target), window, cx);
        }
    };
    menu = menu.separator().item(with_kbd(
        MenuItem::new("Close").icon(Icon::X).on_select(close),
        &keys.close,
    ));
    if tabs > 1 {
        let (local, bridge, state) = (Rc::clone(local), bridge.clone(), state.clone());
        menu = menu.item(MenuItem::new("Close others").on_select(move |window, cx| {
            close_other_tabs(position, &local, &bridge, &state, window, cx);
        }));
    }
    menu
}

/// The `+` menu: every kind of tab this Workspace opens, with its key. Lazygit has no key; it
/// selects the session's git tab, or opens one. The terminal fallback is offered from an agent
/// tab, which is the only place it means anything.
fn new_tab_menu(
    menu: Menu,
    on_agent: bool,
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    cx: &mut gpui::Context<Menu>,
) -> Menu {
    let keys = workspace_keys();
    let worktree = state
        .read(cx)
        .active_session()
        .is_some_and(|session| matches!(session.kind, SessionKind::Worktree(_)));
    let item =
        |label: &'static str, icon: Icon, action: Box<dyn gpui::Action>, kbd: &Option<Kbd>| {
            let item = MenuItem::new(label).icon(icon).action(action);
            match kbd.clone() {
                Some(kbd) => item.kbd(kbd),
                None => item,
            }
        };
    let mut menu = menu
        .item(item(
            "Terminal",
            Icon::Terminal,
            Box::new(prefix::NewTerminal),
            &keys.new_terminal,
        ))
        .item(item(
            "Claude thread",
            Icon::Bot,
            Box::new(native_agent::NewClaude),
            &keys.new_claude,
        ))
        .item(item(
            "Codex thread",
            Icon::Sparkles,
            Box::new(native_agent::NewCodex),
            &keys.new_codex,
        ));
    if worktree {
        menu = menu.item(item(
            "Board",
            Icon::SquareKanban,
            Box::new(prefix::OpenBoard),
            &keys.board,
        ));
        let (local, bridge, state) = (Rc::clone(local), bridge.clone(), state.clone());
        menu = menu.separator().item(
            MenuItem::new("Lazygit")
                .icon(Icon::GitBranch)
                .on_select(move |_, cx| open_lazygit_tab(&local, &bridge, &state, cx)),
        );
    }
    if on_agent {
        menu = menu.separator().item(item(
            "Terminal fallback",
            Icon::SquareTerminal,
            Box::new(native_agent::TerminalFallback),
            &keys.fallback,
        ));
    }
    menu
}
