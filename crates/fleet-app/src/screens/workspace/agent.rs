use super::*;

use crate::{
    actions::native_agent,
    bridge::BridgeCommand,
    screens::agent_thread::{
        AgentThreadEvent, AgentThreadView, ThreadHost, decisions::DecisionKey,
        presentation::header_word,
    },
    views::workspace_tabs::TabTarget,
};
use fleet_core::{
    agents::{AgentKind, AgentThreadSummary, PermissionMode, Seq, ThreadId},
    ids::WorktreeId,
};

/// The open agent tabs, shared with the `'static` action listeners on the workspace root.
pub(super) type AgentViews = HashMap<ThreadId, AgentTab>;

/// One open agent tab, kept alive across frames like a Fleet-drawn pane.
pub(super) struct AgentTab {
    pub(super) view: Entity<AgentThreadView>,
    /// Repaint on notify plus the command relay. Dropping these ends both.
    pub(super) _subscriptions: Vec<Subscription>,
}

/// The agent threads of the worktree a session belongs to, in daemon order.
pub(super) fn threads_of<'a>(app: &'a AppState, session: &Session) -> Vec<&'a AgentThreadSummary> {
    let SessionKind::Worktree(worktree) = &session.kind else {
        return Vec::new();
    };
    app.agents.of_worktree(worktree)
}

/// The tab strip target currently shown.
pub(super) fn active_target(model: &Model) -> Option<TabTarget> {
    model.agent.map_or_else(
        || model.terminal.map(TabTarget::Terminal),
        |thread| Some(TabTarget::Agent(thread)),
    )
}

/// Selects one position of the combined strip, whichever kind it is.
pub(super) fn select_target(
    local: &Rc<RefCell<Local>>,
    bridge: &Bridge,
    state: &Entity<AppState>,
    target: Option<TabTarget>,
    cx: &mut App,
) {
    match target {
        Some(TabTarget::Terminal(terminal)) => {
            leave_agent_tab(state, cx);
            select_terminal(local, bridge, state, Some(terminal), cx);
        }
        Some(TabTarget::Agent(thread)) => activate_agent_tab(state, thread, cx),
        None => {}
    }
}

/// Returns the workspace to its terminals, restoring the terminal key contexts.
pub(super) fn leave_agent_tab(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        let Some(worktree) = active_worktree(app) else {
            return;
        };
        if app.agents.deactivate(&worktree) {
            cx.notify();
        }
    });
}

/// Shows one agent thread as the active tab of its worktree's workspace.
pub(super) fn activate_agent_tab(state: &Entity<AppState>, thread: ThreadId, cx: &mut App) {
    state.update(cx, |app, cx| {
        let Some(worktree) = active_worktree(app) else {
            return;
        };
        app.agents.activate(worktree, thread);
        cx.notify();
    });
}

/// The worktree of the session the workspace is showing.
fn active_worktree(app: &AppState) -> Option<WorktreeId> {
    let session = app.active_session()?;
    match &session.kind {
        SessionKind::Worktree(worktree) => Some(worktree.clone()),
        SessionKind::Agent { .. } => None,
    }
}

/// Creates a thread and selects its tab, falling back to the PTY path when it is unavailable.
pub(super) fn create_thread(
    bridge: &Bridge,
    state: &Entity<AppState>,
    provider: AgentKind,
    cx: &mut App,
) {
    let Some(worktree) = active_worktree(state.read(cx)) else {
        return;
    };
    let reply = bridge.request_agent(BridgeCommand::AgentThreadCreate {
        worktree,
        provider,
        model: None,
        mode: PermissionMode::Ask,
        resume_cursor: None,
        title: None,
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        cx.update(|cx| match answer {
            Ok(Ok(ResponseBody::AgentThreadCreated(summary))) => {
                let thread = summary.thread;
                state.update(cx, |app, cx| {
                    app.agents.apply_summary(summary);
                    cx.notify();
                });
                activate_agent_tab(&state, thread, cx);
            }
            Ok(Err(error)) => state.update(cx, |app, cx| {
                record_mutation_failure(app, create_failure(provider, &error.message));
                cx.notify();
            }),
            Ok(Ok(_)) | Err(_) => state.update(cx, |app, cx| {
                record_mutation_failure(app, create_failure(provider, "the daemon did not answer"));
                cx.notify();
            }),
        });
    })
    .detach();
}

/// The refusal copy, which always names the explicit terminal fallback (§10).
fn create_failure(provider: AgentKind, message: &str) -> String {
    format!(
        "{}: {message} \u{b7} press ctrl-s F for the terminal fallback",
        provider.executable()
    )
}

impl WorkspaceScreen {
    /// Creates, focuses and evicts the entities behind the agent tabs.
    ///
    /// A view is built the first time its tab is selected, because building one installs a
    /// projection copy and asks the daemon for the thread's event tail.
    pub(super) fn sync_agent_views(
        &mut self,
        model: &Model,
        bridge: &Bridge,
        state: &Entity<AppState>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let live: std::collections::HashSet<ThreadId> = {
            let app = state.read(cx);
            app.agents
                .summaries()
                .iter()
                .map(|summary| summary.thread)
                .filter(|thread| !app.agents.is_closed(*thread))
                .collect()
        };
        self.agent_views
            .borrow_mut()
            .retain(|thread, _| live.contains(thread));
        resend_seen_cursors(bridge, state, cx);

        let Some(thread) = model.agent else {
            return;
        };
        let known = self.agent_views.borrow().contains_key(&thread);
        if !known {
            let Some(projection) = state
                .read(cx)
                .agents
                .projection(thread)
                .cloned()
                .or_else(|| starting_projection(state.read(cx), thread))
            else {
                return;
            };
            let view = cx.new(|cx| AgentThreadView::new(projection, cx));
            let mut subscriptions = Vec::new();
            let repaint = state.clone();
            subscriptions.push(cx.observe(&view, move |_view, cx| {
                repaint.update(cx, |_, cx| cx.notify());
            }));
            let (relay_bridge, relay_state) = (bridge.clone(), state.clone());
            subscriptions.push(cx.subscribe(&view, move |_view, event, cx| match event {
                AgentThreadEvent::Command(command) => relay_bridge.send_agent(command.clone()),
                AgentThreadEvent::OpenInEditor(path) => {
                    open_in_editor(&relay_bridge, &relay_state, path, cx);
                }
                AgentThreadEvent::Notice(text) => {
                    let text = text.clone();
                    relay_state.update(cx, |app, cx| {
                        app.toast_short(text, Icon::Info, std::time::Instant::now());
                        cx.notify();
                    });
                }
            }));
            self.agent_views.borrow_mut().insert(
                thread,
                AgentTab {
                    view,
                    _subscriptions: subscriptions,
                },
            );
            // §6: opening a tab asks for the projection and the events after what we hold.
            open_thread(bridge, state, thread, None, cx);
        }

        let Some(view) = self.agent_view(thread) else {
            return;
        };
        // The mirror is authoritative; the view adopts it and moves only the rows a stream
        // touched, so a fast model does not rebuild the transcript per token (§5).
        let (projection, commands) = {
            let app = state.read(cx);
            (
                app.agents.projection(thread).cloned(),
                app.agents.commands(thread),
            )
        };
        if let Some(projection) = projection {
            view.update(cx, |view, cx| {
                view.set_commands(commands);
                view.sync(&projection, cx);
            });
        }
        // P3-T04: the thread states the machine it runs on, and a link the daemon reports as
        // `Down` is what stands the composer down instead of letting a send fail on submit.
        let host = model.host.as_ref().map(|(name, reachability)| ThreadHost {
            name: name.clone(),
            unreachable: reachability.is_unreachable(),
        });
        view.update(cx, |view, cx| view.set_host(host, cx));
        self.offer_worktree_files(model, thread, cx);
        // A resync is requested exactly once per detected gap (§6, client reconnect).
        if state.read(cx).agents.needs_resync(thread) {
            let from = state
                .read(cx)
                .agents
                .projection(thread)
                .map(|projection| projection.last_seq);
            open_thread(bridge, state, thread, from, cx);
        }
        if model.overlay_open {
            return;
        }
        view.update(cx, |view, cx| view.focus_composer(window, cx));
        let composing = view.read(cx).is_composing(cx);
        let scrolling = view.read(cx).is_scrolling();
        let question_cursor = view.read(cx).question_cursor();
        state.update(cx, |app, cx| {
            let mut changed = app.agents.set_composing(thread, composing);
            changed |= app.agents.set_scrolling(thread, scrolling);
            changed |= app.agents.set_question_cursor(thread, question_cursor);
            if changed {
                cx.notify();
            }
        });
        mark_seen(bridge, state, view.read(cx).last_seq(), thread, cx);
    }

    /// Lists the worktree's paths for `@` completion, once per opened tab.
    ///
    /// The scan runs on the background executor because a render must never touch the disk;
    /// the view keeps whatever it already has until the listing arrives.
    fn offer_worktree_files(&self, model: &Model, thread: ThreadId, cx: &mut App) {
        // §12: a remote worktree's path belongs to the other machine's filesystem. Walking it
        // here would either find nothing or complete against an unrelated local directory, so
        // a remote thread is simply offered no `@` listing until the daemon can serve one.
        let Some(path) = model
            .worktree
            .as_ref()
            .and_then(|(_, location)| location.local_path())
        else {
            return;
        };
        if !self.agent_files.borrow_mut().insert(thread) {
            return;
        }
        let Some(view) = self.agent_view(thread) else {
            return;
        };
        let listing = cx
            .background_executor()
            .spawn(async move { worktree_files(&path) });
        cx.spawn(async move |cx| {
            let files = listing.await;
            cx.update(|cx| {
                view.update(cx, |view, _| view.set_files(files));
            });
        })
        .detach();
    }

    /// The entity behind one open agent tab.
    pub(super) fn agent_view(&self, thread: ThreadId) -> Option<Entity<AgentThreadView>> {
        self.agent_views
            .borrow()
            .get(&thread)
            .map(|tab| tab.view.clone())
    }

    /// The band an agent tab fills.
    pub(super) fn agent_area(&self, model: &Model, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let body: AnyElement = model
            .agent
            .and_then(|thread| self.agent_view(thread))
            .map_or_else(
                || {
                    div()
                        .flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .child(Text::ui("opening thread\u{2026}").muted())
                        .into_any_element()
                },
                gpui::IntoElement::into_any_element,
            );
        div()
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

    /// The session-header word for the active agent tab (§3.3).
    pub(super) fn agent_header_word(
        &self,
        state: &AppState,
        model: &Model,
    ) -> Option<SharedString> {
        let thread = model.agent?;
        // The tab badge and the context-bar counters read this same value, which is what §3.3
        // means by "the tab, the header and the context bar agree".
        Some(SharedString::new_static(header_word(
            state.agents.attention(thread),
        )))
    }

    /// The native-agent action listeners, installed beside the terminal ones.
    pub(super) fn with_agent_actions(
        &self,
        root: Div,
        bridge: &Bridge,
        state: &Entity<AppState>,
    ) -> Div {
        macro_rules! on_view {
            ($root:expr, $action:ty, $call:expr) => {{
                let view_state = state.clone();
                let views = Rc::clone(&self.agent_views);
                $root.on_action(move |_: &$action, _window, cx| {
                    let Some(view) = active_view(&views, &view_state, cx) else {
                        return;
                    };
                    #[allow(clippy::redundant_closure_call)]
                    view.update(cx, |view, cx| ($call)(view, cx));
                })
            }};
        }

        let mut root = root;
        root = on_view!(
            root,
            native_agent::Send,
            |view: &mut AgentThreadView, cx| { view.send(cx) }
        );
        root = on_view!(
            root,
            native_agent::Queue,
            |view: &mut AgentThreadView, cx| { view.queue(cx) }
        );
        root = on_view!(
            root,
            native_agent::Stop,
            |view: &mut AgentThreadView, cx| { view.stop(cx) }
        );
        root = on_view!(
            root,
            native_agent::PlanMode,
            |view: &mut AgentThreadView, cx| view.toggle_plan_mode(cx)
        );
        root = on_view!(
            root,
            native_agent::History,
            |view: &mut AgentThreadView, cx| view.history(cx)
        );
        root = on_view!(
            root,
            native_agent::HistoryNext,
            |view: &mut AgentThreadView, cx| view.history_next(cx)
        );
        root = on_view!(
            root,
            native_agent::Files,
            |view: &mut AgentThreadView, cx| view
                .open_picker(crate::screens::agent_thread::picker::PickerKind::Files, cx)
        );
        root = on_view!(
            root,
            native_agent::Commands,
            |view: &mut AgentThreadView, cx| view.open_picker(
                crate::screens::agent_thread::picker::PickerKind::Commands,
                cx
            )
        );
        root = on_view!(
            root,
            native_agent::Model,
            |view: &mut AgentThreadView, cx| view
                .open_picker(crate::screens::agent_thread::picker::PickerKind::Models, cx)
        );
        root = on_view!(
            root,
            native_agent::ExpandRow,
            |view: &mut AgentThreadView, cx| view.expand_row(cx)
        );
        root = on_view!(
            root,
            native_agent::Revert,
            |view: &mut AgentThreadView, cx| { view.revert(cx) }
        );
        root = on_view!(
            root,
            native_agent::OpenInEditor,
            |view: &mut AgentThreadView, cx| view.open_in_editor(cx)
        );
        root = on_view!(
            root,
            native_agent::EditCommand,
            |view: &mut AgentThreadView, cx| view.edit_command(cx)
        );
        root = on_view!(
            root,
            native_agent::Scroll,
            |view: &mut AgentThreadView, cx| view.toggle_scroll_mode(cx)
        );
        // §9's scroll mode moves the transcript itself, not the terminal underneath: these are
        // the native agent's own actions so they can never reach the PTY handlers beside them.
        root = on_view!(
            root,
            native_agent::ScrollLineDown,
            |view: &mut AgentThreadView, cx| view.scroll_rows(1.0, cx)
        );
        root = on_view!(
            root,
            native_agent::ScrollLineUp,
            |view: &mut AgentThreadView, cx| view.scroll_rows(-1.0, cx)
        );
        root = on_view!(
            root,
            native_agent::ScrollHalfPageDown,
            |view: &mut AgentThreadView, cx| view.scroll_viewports(0.5, cx)
        );
        root = on_view!(
            root,
            native_agent::ScrollHalfPageUp,
            |view: &mut AgentThreadView, cx| view.scroll_viewports(-0.5, cx)
        );
        root = on_view!(
            root,
            native_agent::ScrollPageDown,
            |view: &mut AgentThreadView, cx| view.scroll_viewports(1.0, cx)
        );
        root = on_view!(
            root,
            native_agent::ScrollPageUp,
            |view: &mut AgentThreadView, cx| view.scroll_viewports(-1.0, cx)
        );
        root = on_view!(
            root,
            native_agent::ScrollTop,
            |view: &mut AgentThreadView, cx| view.scroll_to_top(cx)
        );
        root = on_view!(
            root,
            native_agent::ScrollBottom,
            |view: &mut AgentThreadView, cx| view.scroll_to_bottom(cx)
        );
        root = on_view!(
            root,
            native_agent::ScrollExit,
            |view: &mut AgentThreadView, cx| view.set_scroll_mode(false, cx)
        );

        macro_rules! on_decision {
            ($root:expr, $action:ty, $key:expr) => {{
                let view_state = state.clone();
                let views = Rc::clone(&self.agent_views);
                $root.on_action(move |_: &$action, _window, cx| {
                    let Some(view) = active_view(&views, &view_state, cx) else {
                        return;
                    };
                    view.update(cx, |view, cx| view.decide($key, cx));
                })
            }};
        }
        root = on_decision!(root, native_agent::AllowOnce, DecisionKey::AllowOnce);
        root = on_decision!(root, native_agent::AllowSession, DecisionKey::AllowSession);
        root = on_decision!(root, native_agent::Deny, DecisionKey::Deny);
        root = on_decision!(root, native_agent::DenyAndStop, DecisionKey::DenyAndStop);
        root = on_decision!(root, native_agent::Choose1, DecisionKey::Choose(0));
        root = on_decision!(root, native_agent::Choose2, DecisionKey::Choose(1));
        root = on_decision!(root, native_agent::Choose3, DecisionKey::Choose(2));
        root = on_decision!(root, native_agent::Choose4, DecisionKey::Choose(3));
        root = on_decision!(root, native_agent::Toggle, DecisionKey::Toggle);
        root = on_decision!(root, native_agent::Answer, DecisionKey::Answer);
        root = on_decision!(root, native_agent::ApprovePlan, DecisionKey::ApprovePlan);
        root = on_decision!(root, native_agent::AskChanges, DecisionKey::AskChanges);
        root = on_decision!(root, native_agent::ViewPlan, DecisionKey::ViewPlan);

        let (new_bridge, new_state) = (bridge.clone(), state.clone());
        root = root.on_action(move |_: &native_agent::NewClaude, _window, cx| {
            create_thread(&new_bridge, &new_state, AgentKind::Claude, cx);
        });
        let (open_bridge, open_state) = (bridge.clone(), state.clone());
        root = root.on_action(move |_: &native_agent::NewOpenCode, _window, cx| {
            create_thread(&open_bridge, &open_state, AgentKind::OpenCode, cx);
        });
        let (close_bridge, close_state) = (bridge.clone(), state.clone());
        root = root.on_action(move |_: &native_agent::CloseTab, _window, cx| {
            close_agent_tab(&close_bridge, &close_state, cx);
        });
        let (fallback_state, fallback_views) = (state.clone(), Rc::clone(&self.agent_views));
        root.on_action(move |_: &native_agent::TerminalFallback, _window, cx| {
            let provider = active_view(&fallback_views, &fallback_state, cx)
                .map(|view| view.read(cx).projection().provider);
            let worktree = active_worktree(fallback_state.read(cx));
            open_terminal_fallback(&fallback_state, provider, worktree, cx);
        })
    }
}

/// The entity behind the tab the strip is showing, when it is an agent tab.
fn active_view(
    views: &Rc<RefCell<AgentViews>>,
    state: &Entity<AppState>,
    cx: &App,
) -> Option<Entity<AgentThreadView>> {
    let app = state.read(cx);
    let session = app.active_session()?;
    let SessionKind::Worktree(worktree) = &session.kind else {
        return None;
    };
    let thread = app.agents.active(worktree)?;
    views.borrow().get(&thread).map(|tab| tab.view.clone())
}

/// `ctrl-s x` on an agent tab drops the client lease; the thread keeps running.
///
/// The daemon answers the close with an ack and goes on listing the thread — §6 keeps every
/// thread browsable — so taking the tab out of the strip is the client's own bookkeeping. Without
/// it the tab is redrawn by the very next summary broadcast and `^s x` looks like it did nothing.
fn close_agent_tab(bridge: &Bridge, state: &Entity<AppState>, cx: &mut App) {
    let Some(thread) = state.read(cx).active_session().and_then(|session| {
        let SessionKind::Worktree(worktree) = &session.kind else {
            return None;
        };
        state.read(cx).agents.active(worktree)
    }) else {
        return;
    };
    bridge.send_agent(BridgeCommand::AgentThreadClose { thread });
    leave_agent_tab(state, cx);
    state.update(cx, |app, cx| {
        if app.agents.close(thread) {
            cx.notify();
        }
    });
}

/// Asks for a thread's projection and event tail, and installs whatever comes back (§6).
///
/// A gap in the reply is recorded rather than papered over: the mirror refuses the tail and the
/// next frame issues one more open from the sequence it did apply.
fn open_thread(
    bridge: &Bridge,
    state: &Entity<AppState>,
    thread: ThreadId,
    from_seq: Option<Seq>,
    cx: &mut App,
) {
    let reply = bridge.request_agent(BridgeCommand::AgentThreadOpen { thread, from_seq });
    // One request is in flight from here: the flag comes back only if the reply is a gap.
    state.update(cx, |app, _| app.agents.clear_resync(thread));
    let state = state.clone();
    cx.spawn(async move |cx| {
        let answer = reply.recv().await;
        cx.update(|cx| match answer {
            Ok(Ok(ResponseBody::AgentThreadSnapshot {
                projection,
                events_after,
            })) => state.update(cx, |app, cx| {
                app.agents.install_snapshot(projection, &events_after);
                cx.notify();
            }),
            Ok(Err(error)) => state.update(cx, |app, cx| {
                record_mutation_failure(app, format!("agent thread: {}", error.message));
                cx.notify();
            }),
            // A lost or unexpected reply leaves the tab on its placeholder projection; the next
            // summary or event re-arms the resync rather than clearing the transcript.
            Ok(Ok(_)) | Err(_) => {}
        });
    })
    .detach();
}

/// `o` on a focused row: the same `$EDITOR` terminal tab Settings' `E` opens.
fn open_in_editor(bridge: &Bridge, state: &Entity<AppState>, path: &str, cx: &mut App) {
    let (session, cwd) = {
        let app = state.read(cx);
        let Some(session) = app.active_session().cloned() else {
            return;
        };
        let cwd = worktree_of(app, &session)
            .map(|worktree| worktree.path.clone())
            .unwrap_or_else(|| session.cwd.clone());
        (session, cwd)
    };
    bridge.send(RequestBody::NewTerminal {
        session: session.id.clone(),
        name: workspace_tabs::unique_terminal_name(&session, "edit"),
        command: format!("{} {}", crate::dialogs::editor_command(), shell_quote(path)),
        cwd,
    });
}

/// Single-quotes a path so a name with spaces survives the daemon's shell.
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// The explicit migration escape hatch: the PTY agent popup, on demand (§10).
///
/// §1 makes the terminal the fallback *for the thread you are in*, so the popup runs that
/// thread's provider rather than always Claude.
fn open_terminal_fallback(
    state: &Entity<AppState>,
    provider: Option<AgentKind>,
    worktree: Option<WorktreeId>,
    cx: &mut App,
) {
    let agent = match provider {
        Some(AgentKind::OpenCode) => fleet_core::config::Agent::Opencode,
        _ => fleet_core::config::Agent::Claude,
    };
    state.update(cx, |app, cx| {
        app.toggle_agent_popup(agent, worktree);
        cx.notify();
    });
}

/// Tells the daemon what this window has actually shown, clearing `finished` and `unread`.
fn mark_seen(bridge: &Bridge, state: &Entity<AppState>, seq: Seq, thread: ThreadId, cx: &mut App) {
    let already = state.read(cx).agents.seen(thread);
    if already >= seq {
        return;
    }
    state.update(cx, |app, cx| {
        app.agents.mark_seen(thread, seq);
        cx.notify();
    });
    bridge.send_agent(BridgeCommand::AgentMarkSeen { thread, seq });
}

/// Re-reports what this window has read for every thread the daemon has forgotten.
///
/// The daemon keeps `last_seen_seq` in runtime state only (§3.3), so a restart brings every
/// already-read tab back as amber `needs you`. The local cursor is the surviving truth, and it
/// is sent for every thread rather than only for the tab that is being shown — a tab the user
/// never revisits would otherwise keep its dot forever.
fn resend_seen_cursors(bridge: &Bridge, state: &Entity<AppState>, cx: &mut App) {
    let stale = state.read(cx).agents.stale_seen();
    if stale.is_empty() {
        return;
    }
    state.update(cx, |app, _| {
        for (thread, seq) in &stale {
            app.agents.mark_reported(*thread, *seq);
        }
    });
    for (thread, seq) in stale {
        bridge.send_agent(BridgeCommand::AgentMarkSeen { thread, seq });
    }
}

/// How many worktree paths `@` offers before it stops walking.
const MAX_COMPLETION_FILES: usize = 2_000;

/// A shallow, bounded listing of the worktree, skipping the directories nobody completes into.
fn worktree_files(root: &std::path::Path) -> Vec<String> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
        if out.len() >= MAX_COMPLETION_FILES {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                walk(root, &path, out);
            } else if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.display().to_string());
            }
            if out.len() >= MAX_COMPLETION_FILES {
                return;
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    files
}

/// The placeholder projection a tab shows before its snapshot lands.
fn starting_projection(app: &AppState, thread: ThreadId) -> Option<ThreadProjection> {
    let summary = app.agents.summary(thread)?;
    Some(ThreadProjection::new(
        summary.thread,
        summary.worktree.clone(),
        summary.provider,
    ))
}
