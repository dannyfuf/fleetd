use super::*;

mod requests;

use requests::{
    close_agent_tab, load_older_page, mark_seen, open_in_editor, open_terminal_fallback,
    open_thread, refresh_checkpoints, resend_seen_cursors,
};

use crate::{
    actions::native_agent,
    bridge::BridgeCommand,
    screens::agent_thread::{
        AgentThreadEvent, AgentThreadView, ThreadHost, picker::PickerKind,
        presentation::header_word,
    },
    views::workspace_tabs::TabTarget,
};
use fleet_core::{
    agents::{AgentKind, AgentThreadSummary, PermissionMode, ThreadId},
    ids::WorktreeId,
};

/// The open agent tabs, shared with the `'static` action listeners on the workspace root.
pub(super) type AgentViews = HashMap<ThreadId, AgentTab>;

/// One open agent tab, kept alive across frames like a Fleet-drawn pane.
pub(super) struct AgentTab {
    pub(super) view: Entity<AgentThreadView>,
    /// Repaint on notify plus the command relay. Dropping these ends both.
    pub(super) _subscriptions: Vec<Subscription>,
    /// The one `@` completion listing this tab asked for.
    ///
    /// The latch lives on the tab it guards, not in a screen-wide set: a thread the snapshot
    /// stops listing loses its view, and a set that outlived it refused to list the rebuilt
    /// tab's paths for the rest of the session. Holding the `Task` also ends an in-flight scan
    /// with the view it was for.
    pub(super) files: Option<Task<()>>,
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
            subscriptions.push(observe_view_state(&view, thread, state, cx));
            let (relay_bridge, relay_state) = (bridge.clone(), state.clone());
            subscriptions.push(cx.subscribe(&view, move |view, event, cx| match event {
                AgentThreadEvent::Command(command) => relay_bridge.send_agent(command.clone()),
                // The one answer that has to come back into the view: `[u]` is drawn from it.
                AgentThreadEvent::RefreshCheckpoints => {
                    refresh_checkpoints(&relay_bridge, &view, thread, cx);
                }
                AgentThreadEvent::LoadOlder => {
                    load_older_page(&relay_bridge, &relay_state, thread, cx);
                }
                AgentThreadEvent::OpenInEditor(path) => {
                    open_in_editor(&relay_bridge, &relay_state, path, cx);
                }
                AgentThreadEvent::Copy(text) => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
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
                    files: None,
                },
            );
            // §6: opening a tab asks for the projection and the events after what we hold.
            open_thread(bridge, state, thread, None, cx);
            // And what the worktree can be reverted to, which decides whether `[u]` is drawn.
            if let Some(view) = self.agent_view(thread) {
                refresh_checkpoints(bridge, &view, thread, cx);
            }
        }

        let Some(view) = self.agent_view(thread) else {
            return;
        };
        // The mirror is authoritative; the view adopts it and moves only the rows a stream
        // touched, so a fast model does not rebuild the transcript per token (§5).
        let (projection, commands, skills) = {
            let app = state.read(cx);
            (
                app.agents.projection(thread).cloned(),
                app.agents.commands(thread),
                app.agents.skills(thread),
            )
        };
        if let Some(projection) = projection {
            view.update(cx, |view, cx| {
                view.set_commands(commands);
                view.set_skills(skills);
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
        relay_view_state(&view, thread, state, cx);
        mark_seen(bridge, state, view.read(cx).last_seq(), thread, cx);
    }

    /// Lists the worktree's paths for `@` completion, once per opened tab.
    ///
    /// The scan runs on the background executor because a render must never touch the disk;
    /// the view keeps whatever it already has until the listing arrives.
    pub(super) fn offer_worktree_files(&self, model: &Model, thread: ThreadId, cx: &mut App) {
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
        if self
            .agent_views
            .borrow()
            .get(&thread)
            .is_none_or(|tab| tab.files.is_some())
        {
            return;
        }
        let Some(view) = self.agent_view(thread) else {
            return;
        };
        let listing = cx
            .background_executor()
            .spawn(async move { worktree_files(&path) });
        let scan = cx.spawn(async move |cx| {
            let files = listing.await;
            cx.update(|cx| {
                view.update(cx, |view, _| view.set_files(files));
            });
        });
        if let Some(tab) = self.agent_views.borrow_mut().get_mut(&thread) {
            tab.files = Some(scan);
        }
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
        cx: &App,
    ) -> Option<SharedString> {
        let thread = model.agent?;
        // spec-B §B5.5 rule 4: `stopping…` is held until the daemon reports liveness
        // cleared, not until the interrupt request returns, so it is the view — not the
        // summary — that knows whether one is still in flight.
        if self
            .agent_view(thread)
            .is_some_and(|view| view.read(cx).is_stopping())
        {
            return Some(SharedString::new_static("stopping\u{2026}"));
        }
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

        // §12's row-focus verbs act on the row the transcript's focus ring is on, which exists
        // only inside scroll mode — the `AgentRow` context is never on the chain outside it.
        macro_rules! on_row {
            ($root:expr, $action:ty, $verb:expr) => {{
                let view_state = state.clone();
                let views = Rc::clone(&self.agent_views);
                $root.on_action(move |_: &$action, _window, cx| {
                    let Some(view) = active_view(&views, &view_state, cx) else {
                        return;
                    };
                    view.update(cx, |view, cx| view.focused_row_verb($verb, cx));
                })
            }};
        }

        let mut root = root;
        root = on_view!(
            root,
            native_agent::Send,
            |view: &mut AgentThreadView, cx| { view.send(cx) }
        );
        // §7.2: `⏎` while a turn runs is a **steer**, dispatched immediately. It is the same
        // intent as an idle send, which is why one function serves both bindings — there is no
        // queue to put it in.
        root = on_view!(
            root,
            native_agent::Steer,
            |view: &mut AgentThreadView, cx| { view.send(cx) }
        );
        root = on_view!(
            root,
            native_agent::SendBackground,
            |view: &mut AgentThreadView, cx| { view.send_background(cx) }
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
            |view: &mut AgentThreadView, cx| view.open_picker(PickerKind::Files, cx)
        );
        root = on_view!(
            root,
            native_agent::Commands,
            |view: &mut AgentThreadView, cx| view.open_picker(PickerKind::Commands, cx)
        );
        root = on_view!(
            root,
            native_agent::Model,
            |view: &mut AgentThreadView, cx| view.open_picker(PickerKind::Models, cx)
        );
        root = on_view!(
            root,
            native_agent::Traits,
            |view: &mut AgentThreadView, cx| view.open_picker(PickerKind::Traits, cx)
        );
        root = on_view!(
            root,
            native_agent::AccessMode,
            |view: &mut AgentThreadView, cx| view.open_picker(PickerKind::Access, cx)
        );
        root = on_view!(
            root,
            native_agent::Skills,
            |view: &mut AgentThreadView, cx| view.open_picker(PickerKind::Skills, cx)
        );
        root = on_view!(
            root,
            native_agent::ExpandRow,
            |view: &mut AgentThreadView, cx| view.expand_row(cx)
        );
        root = on_row!(root, native_agent::Revert, fleet_ui_kit::RowAction::Revert);
        root = on_row!(
            root,
            native_agent::OpenInEditor,
            fleet_ui_kit::RowAction::Open
        );
        root = on_row!(root, native_agent::CopyRow, fleet_ui_kit::RowAction::Copy);
        root = on_row!(root, native_agent::DiffRow, fleet_ui_kit::RowAction::Diff);
        root = on_view!(
            root,
            native_agent::Scroll,
            |view: &mut AgentThreadView, cx| view.toggle_scroll_mode(cx)
        );
        // §9's scroll mode moves the transcript itself, not the terminal underneath: these are
        // the native agent's own actions so they can never reach the PTY handlers beside them.
        // §12: `j`/`k` move the **focused row** and scroll to it, rather than nudging the
        // viewport: the focus is what `⏎`/`u`/`o`/`y`/`d` then act on.
        root = on_view!(
            root,
            native_agent::ScrollLineDown,
            |view: &mut AgentThreadView, cx| view.move_row_focus(1, cx)
        );
        root = on_view!(
            root,
            native_agent::ScrollLineUp,
            |view: &mut AgentThreadView, cx| view.move_row_focus(-1, cx)
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

        // §6.2: the drawer owns the key *vocabulary*, and the status bar mirrors the same
        // source, so neither can advertise a scope the other does not offer. Each binding hands
        // the keystroke to `Decision::action_for_key` rather than naming an action of its own —
        // which is also what keeps `⏎` unbound on an approval, because the decision refuses it.
        macro_rules! on_decision {
            ($root:expr, $action:ty, $key:expr) => {{
                let view_state = state.clone();
                let views = Rc::clone(&self.agent_views);
                $root.on_action(move |_: &$action, _window, cx| {
                    let Some(view) = active_view(&views, &view_state, cx) else {
                        return;
                    };
                    view.update(cx, |view, cx| view.decide_key($key, cx));
                })
            }};
        }
        root = on_decision!(root, native_agent::AllowOnce, "y");
        root = on_decision!(root, native_agent::AllowSession, "a");
        root = on_decision!(root, native_agent::Deny, "n");
        root = on_decision!(root, native_agent::DenyAndStop, "escape");
        root = on_decision!(root, native_agent::EditCommand, "e");
        root = on_decision!(root, native_agent::Choose1, "1");
        root = on_decision!(root, native_agent::Choose2, "2");
        root = on_decision!(root, native_agent::Choose3, "3");
        root = on_decision!(root, native_agent::Choose4, "4");
        root = on_decision!(root, native_agent::Choose5, "5");
        root = on_decision!(root, native_agent::Toggle, "space");
        root = on_decision!(root, native_agent::Answer, "enter");
        root = on_decision!(root, native_agent::Previous, "p");
        root = on_decision!(root, native_agent::Implement, "y");
        root = on_decision!(root, native_agent::Refine, "n");

        let (new_bridge, new_state) = (bridge.clone(), state.clone());
        root = root.on_action(move |_: &native_agent::NewClaude, _window, cx| {
            create_thread(&new_bridge, &new_state, AgentKind::Claude, cx);
        });
        let (open_bridge, open_state) = (bridge.clone(), state.clone());
        root = root.on_action(move |_: &native_agent::NewCodex, _window, cx| {
            create_thread(&open_bridge, &open_state, AgentKind::Codex, cx);
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

/// Mirrors one thread view's `AppState`-visible state, notifying only when it actually moved.
///
/// `AppState` is the single model, so `state.update(cx, |_, cx| cx.notify())` re-runs every
/// observer the shell has — `synchronize_surfaces` included — for a repaint the view's own
/// notify already scheduled. Only three values are read off the view by `AppState`-derived
/// chrome, so the relay writes those and notifies on change, which is the guarded shape
/// gpui-state-and-memory asks for.
pub(super) fn relay_view_state(
    view: &Entity<AgentThreadView>,
    thread: ThreadId,
    state: &Entity<AppState>,
    cx: &mut App,
) {
    let composing = view.read(cx).is_composing(cx);
    let scrolling = view.read(cx).is_scrolling();
    let question_cursor = view.read(cx).question_cursor();
    // §12: the `AgentRow` context is derived from whether a row actually carries the focus ring,
    // so `⏎`/`u`/`o`/`y`/`d` are bound exactly when there is a row for them to act on.
    let row_focus = scrolling && view.read(cx).transcript().read(cx).focused_row().is_some();
    state.update(cx, |app, cx| {
        let mut changed = app.agents.set_composing(thread, composing);
        changed |= app.agents.set_scrolling(thread, scrolling);
        changed |= app.agents.set_question_cursor(thread, question_cursor);
        changed |= app.agents.set_row_focus(thread, row_focus);
        if changed {
            cx.notify();
        }
    });
}

/// The subscription that keeps that mirror current, installed once per tab.
pub(super) fn observe_view_state(
    view: &Entity<AgentThreadView>,
    thread: ThreadId,
    state: &Entity<AppState>,
    cx: &mut App,
) -> Subscription {
    let repaint = state.clone();
    cx.observe(view, move |view, cx| {
        relay_view_state(&view, thread, &repaint, cx);
    })
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
