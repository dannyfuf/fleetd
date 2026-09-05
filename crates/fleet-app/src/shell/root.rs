//! [`Shell`]: the window's only view.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{ActiveTheme, AppFrame, Icon, KitAssets, Theme, ThemeMode, ToastStack, Veil};
use gpui::{
    Action, AnyElement, App, Bounds, Context, Div, Entity, FocusHandle, Focusable, IntoElement,
    Keystroke, Menu, MenuItem, Render, Subscription, Task, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};

use crate::{
    actions::{
        confirm, daemon as daemon_actions, dialog, filter, first_run as first_run_actions, fleet,
        help, hub, jobs, palette, prefix, quit_daemon_dialog, quit_dialog, repos, scroll,
        workspace,
    },
    bridge::{Bridge, BridgeEvent},
    dialogs::Dialogs,
    drive,
    keymap::{self, ROOT_CONTEXT},
    screens::{hub::HubScreen, jobs::JobsPanel, workspace::WorkspaceScreen},
    shell::{
        chrome, daemon,
        quit::{QuitDecision, StopDecision, quit_decision, running_count, stop_decision},
    },
    state::{AppState, DaemonLink, HubPane, HubTab, Overlay, Screen, TerminalMode},
};

/// How often the shell wakes to expire toasts and advance the daemon countdown.
const TICK: Duration = Duration::from_millis(250);
/// The window's default and minimum size (§0).
const DEFAULT_SIZE: (f32, f32) = (1280.0, 800.0);
/// The smallest window the ladders of §2.9 are defined for.
const MIN_SIZE: (f32, f32) = (900.0, 560.0);

/// Takes the one key following `ctrl-s` from live state, even before GPUI repaints its contexts.
fn take_live_prefix_action(
    state: &mut AppState,
    keystroke: &Keystroke,
) -> (bool, Option<Box<dyn Action>>) {
    // Overlays and daemon surfaces are allowed to shadow the Workspace even if they were opened
    // by mouse or arrived while the prefix was live. Looking at the authoritative state chain,
    // rather than just `terminal_mode`, keeps this interceptor out of those inner contexts.
    if state.context_chain().as_slice() != ["Workspace", "Prefix"] {
        return (false, None);
    }
    let action = keymap::action_for_keystroke("Workspace > Prefix", keystroke);
    state.leave_prefix();
    (true, action)
}

/// Whether the migration action is still valid at the instant its key reaches the root.
fn first_run_import_allowed(is_first_run: bool, state_file_exists: bool) -> bool {
    is_first_run && !state_file_exists
}

/// The root view: frame, routing, chrome, focus and the quit flow.
pub struct Shell {
    state: Entity<AppState>,
    bridge: Bridge,
    /// Focused while the Hub or the Workspace owns the keyboard.
    body_focus: FocusHandle,
    /// Focused while a dialog, the palette, the filter or the jobs panel is open, so that an
    /// overlay's key context really does shadow the screen behind it.
    overlay_focus: FocusHandle,
    hub: HubScreen,
    workspace: WorkspaceScreen,
    jobs: JobsPanel,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl Shell {
    /// Builds the shell, starts the daemon bridge and wires both background loops.
    pub fn new(home: PathBuf, cx: &mut Context<Self>) -> Self {
        let bridge = Bridge::start(home.clone());
        let state = cx.new(|_| AppState::new(home, Instant::now()));

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe(&state, |_, _, cx| cx.notify()));
        // The prefix is one-shot and is driven from live state, not the last rendered context
        // tree. The latter is necessarily one frame behind `EnterPrefix`; without this
        // interceptor a fast (and every driver-issued) `ctrl-s s` sees `Workspace > Terminal`
        // for both keys and silently drops the second one.
        let prefix_state = state.clone();
        subscriptions.push(cx.intercept_keystrokes(move |event, window, cx| {
            let (consumed, action) = prefix_state.update(cx, |state, cx| {
                let result = take_live_prefix_action(state, &event.keystroke);
                if result.0 {
                    cx.notify();
                }
                result
            });
            if !consumed {
                return;
            }
            if let Some(action) = action {
                window.dispatch_action(action, cx);
            }
            // The second prefix key is always consumed, bound or not, and can never reach PTY.
            cx.stop_propagation();
        }));

        let tasks = vec![Self::spawn_event_loop(&bridge, cx), Self::spawn_ticker(cx)];

        Self {
            state,
            bridge,
            body_focus: cx.focus_handle(),
            overlay_focus: cx.focus_handle(),
            hub: HubScreen::new(cx),
            workspace: WorkspaceScreen::new(cx),
            jobs: JobsPanel::new(cx),
            _subscriptions: subscriptions,
            _tasks: tasks,
        }
    }

    /// The shared application state, for screens that need their own handle.
    #[must_use]
    pub fn state(&self) -> &Entity<AppState> {
        &self.state
    }

    /// The daemon bridge.
    #[must_use]
    pub fn bridge(&self) -> &Bridge {
        &self.bridge
    }

    fn spawn_event_loop(bridge: &Bridge, cx: &mut Context<Self>) -> Task<()> {
        let events = bridge.events();
        cx.spawn(async move |shell, cx| {
            while let Ok(event) = events.recv().await {
                let now = Instant::now();
                // A lagged broadcast leaves every mirror with a hole no later diff can fill;
                // only a full frame repairs it, and nothing else in the app asks for one.
                let lagged = matches!(event, BridgeEvent::EventsLagged { .. });
                let updated = shell.update(cx, |shell, cx| {
                    let stale = shell.state.update(cx, |state, cx| {
                        let synced = state
                            .grids
                            .iter()
                            .filter_map(|(id, grid)| (!grid.desynced).then_some(*id))
                            .collect::<Vec<_>>();
                        state.apply_bridge_event(event, now);
                        cx.notify();
                        state
                            .grids
                            .iter()
                            .filter_map(|(id, grid)| {
                                (lagged || (grid.desynced && synced.contains(id))).then_some(*id)
                            })
                            .collect::<Vec<_>>()
                    });
                    for terminal in stale {
                        shell
                            .bridge
                            .send(RequestBody::RequestFullFrame { terminal });
                    }
                });
                if updated.is_err() {
                    return;
                }
            }
        })
    }

    fn spawn_ticker(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |shell, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let now = Instant::now();
                let ticked = shell.update(cx, |shell, cx| {
                    shell.state.update(cx, |state, cx| {
                        let outdated = state.snapshot.as_ref().is_some_and(|snapshot| {
                            daemon::daemon_binary_is_newer(&snapshot.daemon.started_at)
                        });
                        let outdated_changed = state.daemon_outdated != outdated;
                        state.daemon_outdated = outdated;
                        if state.tick(now) || outdated_changed {
                            cx.notify();
                        }
                    });
                });
                if ticked.is_err() {
                    return;
                }
            }
        })
    }

    // ------------------------------------------------------------------ overlays and modes

    fn open(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.open_overlay(overlay);
            cx.notify();
        });
    }

    fn close_overlay(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            if state.close_overlay() {
                cx.notify();
            }
        });
    }

    fn open_palette(&mut self, _: &fleet::OpenPalette, _: &mut Window, cx: &mut Context<Self>) {
        self.open(Overlay::Palette, cx);
    }

    fn open_settings(&mut self, _: &fleet::OpenSettings, _: &mut Window, cx: &mut Context<Self>) {
        self.open(Overlay::Dialog(Dialogs::Settings), cx);
    }

    fn open_help(&mut self, _: &fleet::OpenHelp, _: &mut Window, cx: &mut Context<Self>) {
        self.open(Overlay::Dialog(Dialogs::Help), cx);
    }

    fn open_jobs(&mut self, _: &fleet::OpenJobs, _: &mut Window, cx: &mut Context<Self>) {
        self.open(Overlay::Jobs, cx);
    }

    fn go_jobs(&mut self, _: &hub::GoJobs, _: &mut Window, cx: &mut Context<Self>) {
        self.open(Overlay::Jobs, cx);
    }

    /// `!` focuses the sticky error, which means opening the Jobs panel on the failed job.
    fn focus_sticky_error(
        &mut self,
        _: &fleet::FocusStickyError,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Overlay::Jobs, cx);
    }

    fn open_filter(&mut self, _: &hub::OpenFilter, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.filter.editing = true;
            state.open_overlay(Overlay::Filter);
            cx.notify();
        });
    }

    fn cancel(&mut self, _: &fleet::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            if state.cancel() {
                cx.notify();
            }
        });
    }

    fn filter_escape(&mut self, _: &filter::Escape, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            if state.cancel() {
                cx.notify();
            }
        });
    }

    fn close_palette(&mut self, _: &palette::Close, _: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(cx);
    }

    fn close_jobs(&mut self, _: &jobs::Close, _: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(cx);
    }

    fn close_help(&mut self, _: &help::Close, _: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(cx);
    }

    fn cancel_dialog(&mut self, _: &dialog::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(cx);
    }

    fn reject_confirm(&mut self, _: &confirm::Reject, _: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(cx);
    }

    // ------------------------------------------------------------------ first run (§3.13)

    /// `i`: `fleet import --from-swarm`, as a daemon job.
    ///
    /// The three keys the card advertises are handled here rather than in [`HubScreen`],
    /// because during first run the Hub is not rendered at all and its listeners are therefore
    /// nowhere on the dispatch path. The Hub's own listeners are deeper, so they still win
    /// whenever it *is* on screen.
    fn first_run_import(
        &mut self,
        _: &first_run_actions::Import,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let allowed = {
            let state = self.state.read(cx);
            first_run_import_allowed(state.is_first_run(), state.home.join("state.json").exists())
        };
        if !allowed {
            self.state.update(cx, |state, cx| {
                state.toast_short(
                    "Fleet state already exists; import was not started",
                    Icon::Info,
                    Instant::now(),
                );
                cx.notify();
            });
            return;
        }
        // §2.7 forbids a "job started" toast: the import reports through the job ticker and
        // the Jobs panel, and lands the user in a populated Hub when it finishes.
        self.bridge.send(RequestBody::ImportFromSwarm);
    }

    /// `N` on the first-run card: create the first context.
    fn new_context(&mut self, _: &hub::NewContext, _: &mut Window, cx: &mut Context<Self>) {
        self.open(Overlay::Dialog(Dialogs::NewContext), cx);
    }

    /// `n` on the first-run card: clone the first repository.
    fn clone_repo(&mut self, _: &repos::Clone, _: &mut Window, cx: &mut Context<Self>) {
        self.open(Overlay::Dialog(Dialogs::CloneRepo), cx);
    }

    // ------------------------------------------------------------------ hub navigation

    fn focus_prev_pane(&mut self, _: &hub::FocusPrevPane, _: &mut Window, cx: &mut Context<Self>) {
        self.set_pane(HubPane::Repos, cx);
    }

    fn focus_next_pane(&mut self, _: &hub::FocusNextPane, _: &mut Window, cx: &mut Context<Self>) {
        self.set_pane(HubPane::List, cx);
    }

    fn go_repos(&mut self, _: &hub::GoRepos, _: &mut Window, cx: &mut Context<Self>) {
        self.set_pane(HubPane::Repos, cx);
    }

    fn go_worktrees(&mut self, _: &hub::GoWorktrees, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.screen = Screen::Hub {
                tab: HubTab::Worktrees,
            };
            state.hub_pane = HubPane::List;
            cx.notify();
        });
    }

    fn go_prs(&mut self, _: &hub::GoPrs, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.screen = Screen::Hub { tab: HubTab::Prs };
            state.hub_pane = HubPane::List;
            cx.notify();
        });
    }

    fn toggle_pr_screen(
        &mut self,
        _: &hub::TogglePrScreen,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            if let Screen::Hub { tab } = &state.screen {
                let next = match tab {
                    HubTab::Worktrees => HubTab::Prs,
                    HubTab::Prs => HubTab::Worktrees,
                };
                state.screen = Screen::Hub { tab: next };
                // §3.10: a filter does not survive a screen change.
                state.filter = crate::state::FilterState::default();
                cx.notify();
            }
        });
    }

    fn set_pane(&mut self, pane: HubPane, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            if matches!(state.screen, Screen::Hub { .. }) && state.hub_pane != pane {
                state.hub_pane = pane;
                cx.notify();
            }
        });
    }

    fn toggle_detail(&mut self, _: &hub::ToggleDetail, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.detail_open = !state.detail_open;
            cx.notify();
        });
    }

    fn toggle_rail(&mut self, _: &hub::ToggleRepoRail, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.rail_collapsed = !state.rail_collapsed;
            cx.notify();
        });
    }

    // ------------------------------------------------------------------ workspace modes

    fn enter_prefix(&mut self, _: &workspace::EnterPrefix, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.enter_prefix();
            cx.notify();
        });
    }

    fn cancel_prefix(&mut self, _: &prefix::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.leave_prefix();
            cx.notify();
        });
    }

    fn prefix_go_hub(&mut self, _: &prefix::GoHub, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.leave_prefix();
            state.screen = Screen::Hub {
                tab: HubTab::Worktrees,
            };
            state.hub_pane = HubPane::List;
            cx.notify();
        });
    }

    fn prefix_enter_scroll(
        &mut self,
        _: &prefix::EnterScroll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            let alt_screen = state
                .active_grid()
                .is_some_and(|grid| grid.modes.alt_screen);
            state.leave_prefix();
            if alt_screen {
                // §3.6: scroll mode is suppressed while an alt-screen app is running.
                state.toast_short(
                    "no scrollback in alt-screen",
                    Icon::ChevronsUp,
                    Instant::now(),
                );
            } else {
                state.terminal_mode = TerminalMode::Scroll;
            }
            cx.notify();
        });
    }

    fn prefix_toggle_zoom(
        &mut self,
        _: &prefix::ToggleZoom,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.leave_prefix();
            state.zoomed = !state.zoomed;
            cx.notify();
        });
    }

    fn leave_scroll(&mut self, _: &scroll::Exit, _: &mut Window, cx: &mut Context<Self>) {
        self.exit_scroll(cx);
    }

    fn escape_scroll(&mut self, _: &scroll::Escape, _: &mut Window, cx: &mut Context<Self>) {
        self.exit_scroll(cx);
    }

    fn exit_scroll(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            if state.terminal_mode == TerminalMode::Scroll {
                state.terminal_mode = TerminalMode::Terminal;
                cx.notify();
            }
        });
    }

    // ------------------------------------------------------------------ daemon surfaces

    fn daemon_retry(&mut self, _: &daemon_actions::Retry, _: &mut Window, cx: &mut Context<Self>) {
        self.retry_daemon(cx);
    }

    fn daemon_reconnect(
        &mut self,
        _: &daemon_actions::Reconnect,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.retry_daemon(cx);
    }

    fn retry_daemon(&mut self, cx: &mut Context<Self>) {
        self.bridge.reconnect();
        self.state.update(cx, |state, cx| {
            state.daemon = DaemonLink::Starting;
            state.daemon_since = Instant::now();
            cx.notify();
        });
    }

    fn daemon_open_log(
        &mut self,
        _: &daemon_actions::OpenLog,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.state.read(cx).daemon_log_path();
        cx.open_with_system(&path);
    }

    fn daemon_dismiss_banner(
        &mut self,
        _: &daemon_actions::DismissBanner,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let showed_doctor = self.state.update(cx, |state, cx| {
            let showed = state.doctor.take().is_some();
            if showed {
                cx.notify();
            }
            showed
        });
        if showed_doctor {
            return;
        }
        self.state.update(cx, |state, cx| {
            if let DaemonLink::Lost { attempt, .. } = state.daemon {
                state.daemon = DaemonLink::Lost {
                    attempt,
                    dismissed: true,
                };
                cx.notify();
            }
        });
    }

    fn run_doctor(
        &mut self,
        _: &daemon_actions::RunDoctor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reply = self.bridge.request(RequestBody::Doctor);
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let answer = reply.recv().await;
            cx.update(|cx| {
                state.update(cx, |app, cx| {
                    match answer {
                        Ok(Ok(ResponseBody::Doctor(checks))) => app.doctor = Some(checks),
                        // §3.12 B is exactly where `D` matters and exactly where the bridge is
                        // offline, so the refusal has to land in the sticky slot (§1.8) rather
                        // than be dropped on the floor.
                        Ok(Err(error)) => {
                            app.sticky_error = Some(crate::state::StickyError {
                                text: error.message,
                                job: None,
                                retryable: false,
                            });
                        }
                        Ok(Ok(_)) | Err(_) => {
                            app.sticky_error = Some(crate::state::StickyError {
                                text: "doctor: the daemon did not answer".to_owned(),
                                job: None,
                                retryable: false,
                            });
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    // ------------------------------------------------------------------ quit flow

    fn quit(&mut self, _: &fleet::Quit, _: &mut Window, cx: &mut Context<Self>) {
        let (warn, running) = {
            let state = self.state.read(cx);
            // §3.8.8 exists to say "these keep running **in fleetd**". While the daemon is
            // starting or will not start, the last snapshot's running jobs belong to a daemon
            // that is already gone, so there is nothing for the confirm to promise and
            // `ctrl-q` quits — which is exactly what KEYMAP guarantees on those surfaces.
            let daemon_gone = matches!(
                state.daemon,
                DaemonLink::Starting | DaemonLink::Failed { .. }
            );
            (
                state.warn_before_quit,
                if daemon_gone {
                    0
                } else {
                    state
                        .snapshot
                        .as_ref()
                        .map_or(0, |snapshot| running_count(&snapshot.jobs))
                },
            )
        };
        match quit_decision(warn, running) {
            QuitDecision::QuitNow => self.quit_now(cx),
            QuitDecision::Confirm => self.open(Overlay::Dialog(Dialogs::Quit), cx),
        }
    }

    fn quit_and_stop_daemon(
        &mut self,
        _: &fleet::QuitAndStopDaemon,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (running, sessions) = {
            let state = self.state.read(cx);
            state.snapshot.as_ref().map_or((0, 0), |snapshot| {
                (running_count(&snapshot.jobs), snapshot.sessions.len())
            })
        };
        match stop_decision(running, sessions) {
            StopDecision::StopNow => self.stop_daemon_and_quit(cx),
            StopDecision::Confirm => self.open(Overlay::Dialog(Dialogs::QuitDaemon), cx),
        }
    }

    fn accept_quit(&mut self, _: &quit_dialog::Accept, _: &mut Window, cx: &mut Context<Self>) {
        self.quit_now(cx);
    }

    fn reject_quit(&mut self, _: &quit_dialog::Reject, _: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay(cx);
    }

    fn quit_dialog_jobs(
        &mut self,
        _: &quit_dialog::OpenJobs,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(Overlay::Jobs, cx);
    }

    /// `W` in §3.8.8: a legitimate "don't ask again", because nothing is lost by quitting.
    fn never_warn(&mut self, _: &quit_dialog::NeverWarn, _: &mut Window, cx: &mut Context<Self>) {
        self.bridge.send(RequestBody::SetConfig {
            patch: serde_json::json!({ "jobs": { "warnBeforeQuit": false } }),
        });
        self.state
            .update(cx, |state, _| state.warn_before_quit = false);
        self.quit_now(cx);
    }

    fn accept_stop_daemon(
        &mut self,
        _: &quit_daemon_dialog::Accept,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop_daemon_and_quit(cx);
    }

    fn reject_stop_daemon(
        &mut self,
        _: &quit_daemon_dialog::Reject,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(cx);
    }

    fn quit_now(&mut self, cx: &mut Context<Self>) {
        self.bridge.shutdown();
        cx.quit();
    }

    fn stop_daemon_and_quit(&mut self, cx: &mut Context<Self>) {
        let reply = self.bridge.request(RequestBody::DaemonShutdown {
            stop_sessions: true,
        });
        let bridge = self.bridge.clone();
        cx.spawn(async move |_, cx| {
            let _ignored = reply.recv().await;
            bridge.shutdown();
            cx.update(|cx| cx.quit());
        })
        .detach();
    }

    // ------------------------------------------------------------------ rendering

    /// Wraps `child` in one div per key context, outermost first.
    ///
    /// The focused element is `child` itself — every screen and overlay tracks the shell's
    /// focus handle — so the dispatch path reads `Fleet > Hub > Worktrees > <screen>` and both
    /// the shell's listeners (above) and the screen's own (at the focus node) are on it.
    fn contexts(chain: &[&'static str], child: AnyElement) -> AnyElement {
        let mut element = child;
        for context in chain.iter().rev() {
            element = div()
                .size_full()
                .key_context(*context)
                .child(element)
                .into_any_element();
        }
        element
    }

    /// The same nesting, but as a **layer** rather than a flex child.
    ///
    /// An overlay is handed to [`AppFrame::overlay`] / [`AppFrame::body_overlay`], which emit
    /// their layers as children of a flex column. A `size_full` wrapper there is an in-flow
    /// item that eats the whole column, collapsing the body to zero and shoving the status bar
    /// under the context bar (§2.1: the chrome never moves). Absolute positioning takes the
    /// wrapper out of the flow, and — because gpui resolves an absolute child against its
    /// parent's box — also gives the dialog, sheet and palette inside it the frame's geometry,
    /// which is what pins the palette to y = 120 (§3.9).
    fn overlay_contexts(chain: &[&'static str], child: AnyElement) -> AnyElement {
        let mut element = child;
        for context in chain.iter().rev() {
            element = div()
                .absolute()
                .inset_0()
                .key_context(*context)
                .child(element)
                .into_any_element();
        }
        element
    }

    /// The §3.13 first-run card: a migration, not an onboarding.
    fn first_run_card(&self, state: &AppState, focus: &FocusHandle, cx: &App) -> AnyElement {
        let has_swarm =
            first_run_import_allowed(state.is_first_run(), state.home.join("state.json").exists())
                && crate::views::first_run::has_swarm_state(
                    crate::views::first_run::user_home().as_deref(),
                );
        div()
            .track_focus(focus)
            .size_full()
            .child(crate::views::first_run::card(
                &state.home,
                state
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.daemon.version.as_str()),
                has_swarm,
                cx,
            ))
            .into_any_element()
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.body_focus.clone()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let state_handle = self.state.clone();
        let bridge = self.bridge.clone();

        // Everything below reads the state immutably; the borrow ends before the screens are
        // rendered, which is the only place that needs `&mut App`.
        let doctor = state_handle.read(cx).doctor.clone();
        let (mut chain, overlay, screen, mut splash, banner, veil, first_run) = {
            let state = state_handle.read(cx);
            (
                state.context_chain(),
                state.overlay.clone(),
                state.screen.clone(),
                daemon::splash(state, now, cx),
                daemon::banner(&state.daemon, state.daemon_outdated, now),
                state.drops_terminal_keys(),
                state.is_first_run(),
            )
        };
        if doctor.is_some() {
            splash = None;
            // An overlay still shadows the doctor table, exactly as it shadows the Hub.
            if overlay.is_none() {
                chain = vec!["Daemon", "Doctor"];
            }
        }
        let context_bar = chrome::context_bar(state_handle.read(cx), cx);
        let status_bar = chrome::status_bar(state_handle.read(cx), cx);
        let toasts = ToastStack::new(
            state_handle
                .read(cx)
                .toasts
                .iter()
                .map(|live| live.toast.clone()),
        );

        let focus = self.body_focus.clone();
        let overlay_focus = self.overlay_focus.clone();

        // Every overlay is built before the §3.12 branch, because a dialog opened *on* a splash
        // — `ctrl-q` opens §3.8.8 whenever the last snapshot had a running job — has to be drawn
        // and has to carry its key context, or it is an invisible modal nothing can dismiss.
        let overlay_element: Option<AnyElement> = match &overlay {
            Some(Overlay::Jobs) => {
                Some(
                    self.jobs
                        .render(&state_handle, &bridge, &overlay_focus, window, cx),
                )
            }
            Some(Overlay::Dialog(dialog)) => {
                Some(dialog.render(&state_handle, &bridge, &overlay_focus, window, cx))
            }
            Some(Overlay::Palette) => Some(crate::dialogs::palette::render(
                &state_handle,
                &bridge,
                &overlay_focus,
                window,
                cx,
            )),
            Some(Overlay::Filter) => Some(crate::dialogs::filter::render(
                &state_handle,
                &bridge,
                &overlay_focus,
                window,
                cx,
            )),
            None => None,
        };

        // §3.12 A and B are full-window surfaces with no chrome at all.
        if let Some(splash) = splash {
            let wanted = if overlay_element.is_some() {
                &self.overlay_focus
            } else {
                &self.body_focus
            };
            if !wanted.is_focused(window) {
                window.focus(wanted, cx);
            }
            let surface = div()
                .track_focus(&self.body_focus)
                .size_full()
                .child(splash)
                .into_any_element();
            // The chain belongs to whichever element is focused, exactly as in the normal
            // branch: an overlay shadows the splash the same way it shadows the Hub.
            let (surface, layer) = match overlay_element {
                Some(element) => (surface, Some(Self::overlay_contexts(&chain, element))),
                None => (Self::contexts(&chain, surface), None),
            };
            return Self::with_actions(
                div()
                    .relative()
                    .size_full()
                    .key_context(ROOT_CONTEXT)
                    .bg(cx.theme().colors.bg),
                cx,
            )
            .child(surface)
            .children(layer)
            .into_any_element();
        }
        let body: AnyElement = if let Some(checks) = doctor.as_ref() {
            div()
                .track_focus(&focus)
                .size_full()
                .child(crate::views::doctor_view::view(
                    checks,
                    state_handle
                        .read(cx)
                        .snapshot
                        .as_ref()
                        .map(|_| fleet_proto::PROTOCOL_VERSION),
                    cx,
                ))
                .into_any_element()
        } else if first_run {
            self.first_run_card(state_handle.read(cx), &focus, cx)
        } else {
            match &screen {
                Screen::Hub { .. } => self.hub.render(&state_handle, &bridge, &focus, window, cx),
                Screen::Workspace { .. } => {
                    let workspace =
                        self.workspace
                            .render(&state_handle, &bridge, &focus, window, cx);
                    Veil::new(veil).child(workspace).into_any_element()
                }
            }
        };

        // The focused element carries the key-context chain: the overlay when one is open,
        // the body otherwise, so overlays really do shadow the Hub and the Workspace.
        let (body, overlay_element) = match overlay_element {
            Some(element) => (body, Some(Self::overlay_contexts(&chain, element))),
            None => (Self::contexts(&chain, body), None),
        };

        // Exactly one of the two handles is focused, and it is the one attached to the element
        // that carries the key-context chain.
        let wanted = if overlay_element.is_some() {
            &self.overlay_focus
        } else {
            &self.body_focus
        };
        if !wanted.is_focused(window) {
            window.focus(wanted, cx);
        }

        let mut frame = AppFrame::new()
            .context_bar(context_bar)
            .body(body)
            .status_bar(status_bar)
            // §2.2: the toast stack sits bottom-right *above* the status bar, so it belongs to
            // the band between the bars, not to the whole window.
            .body_overlay(toasts);
        if let Some(banner) = banner {
            frame = frame.banner(banner);
        }
        if let Some(layer) = overlay_element {
            // §3.7: the Jobs sheet is clipped to the band between the two bars, so both bars
            // stay reachable while it is open. Every other overlay spans the window.
            frame = if matches!(overlay, Some(Overlay::Jobs)) {
                frame.body_overlay(layer)
            } else {
                frame.overlay(layer)
            };
        }

        Self::with_actions(
            div()
                .size_full()
                .key_context(ROOT_CONTEXT)
                .bg(cx.theme().colors.bg),
            cx,
        )
        .child(frame)
        .into_any_element()
    }
}

impl Shell {
    /// Installs every global action listener on `root`.
    ///
    /// Both branches of [`Shell::render`] use it, splash included: a surface that registers a
    /// subset silently swallows the keys it left out — `ctrl-shift-q` and `Esc` among them —
    /// and there is no way back out of whatever the missing key was meant to leave.
    fn with_actions(root: Div, cx: &mut Context<Self>) -> Div {
        root
            // Global
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::quit_and_stop_daemon))
            .on_action(cx.listener(Self::open_palette))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::open_help))
            .on_action(cx.listener(Self::open_jobs))
            .on_action(cx.listener(Self::focus_sticky_error))
            .on_action(cx.listener(Self::cancel))
            // First run (§3.13) — fallbacks for the three keys the card advertises
            .on_action(cx.listener(Self::first_run_import))
            .on_action(cx.listener(Self::new_context))
            .on_action(cx.listener(Self::clone_repo))
            // Hub navigation
            .on_action(cx.listener(Self::focus_prev_pane))
            .on_action(cx.listener(Self::focus_next_pane))
            .on_action(cx.listener(Self::go_repos))
            .on_action(cx.listener(Self::go_worktrees))
            .on_action(cx.listener(Self::go_prs))
            .on_action(cx.listener(Self::go_jobs))
            .on_action(cx.listener(Self::toggle_pr_screen))
            .on_action(cx.listener(Self::open_filter))
            .on_action(cx.listener(Self::toggle_detail))
            .on_action(cx.listener(Self::toggle_rail))
            // Workspace modes
            .on_action(cx.listener(Self::enter_prefix))
            .on_action(cx.listener(Self::cancel_prefix))
            .on_action(cx.listener(Self::prefix_go_hub))
            .on_action(cx.listener(Self::prefix_enter_scroll))
            .on_action(cx.listener(Self::prefix_toggle_zoom))
            .on_action(cx.listener(Self::leave_scroll))
            .on_action(cx.listener(Self::escape_scroll))
            // Overlays
            .on_action(cx.listener(Self::close_palette))
            .on_action(cx.listener(Self::close_jobs))
            .on_action(cx.listener(Self::close_help))
            .on_action(cx.listener(Self::cancel_dialog))
            .on_action(cx.listener(Self::reject_confirm))
            .on_action(cx.listener(Self::filter_escape))
            // Daemon
            .on_action(cx.listener(Self::daemon_retry))
            .on_action(cx.listener(Self::daemon_reconnect))
            .on_action(cx.listener(Self::daemon_open_log))
            .on_action(cx.listener(Self::daemon_dismiss_banner))
            .on_action(cx.listener(Self::run_doctor))
            // Quit dialogs
            .on_action(cx.listener(Self::accept_quit))
            .on_action(cx.listener(Self::reject_quit))
            .on_action(cx.listener(Self::quit_dialog_jobs))
            .on_action(cx.listener(Self::never_warn))
            .on_action(cx.listener(Self::accept_stop_daemon))
            .on_action(cx.listener(Self::reject_stop_daemon))
    }
}

/// `$FLEET_HOME`, or `~/.fleet`.
#[must_use]
pub fn fleet_home() -> PathBuf {
    if let Some(home) = std::env::var_os("FLEET_HOME") {
        return PathBuf::from(home);
    }
    dirs_home().map_or_else(|| PathBuf::from(".fleet"), |home| home.join(".fleet"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Installs the process-wide `tracing` subscriber: `$RUST_LOG` (default `info`), to stderr.
///
/// Called once from [`run`], so an app-side error is visible in whatever the launcher redirected
/// stderr into. Errors are swallowed: a subscriber already installed by an embedder is fine.
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ignored = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Opens the window and runs the app. Returns when the last window closes.
pub fn run() -> anyhow::Result<()> {
    init_tracing();
    let home = fleet_home();
    tracing::info!(home = %home.display(), "fleet: starting");
    gpui_platform::application()
        .with_assets(KitAssets)
        .run(move |cx: &mut App| {
            Theme::init(ThemeMode::Dark, cx);
            keymap::init(cx);
            cx.set_menus(vec![Menu {
                name: "Fleet".into(),
                items: vec![MenuItem::action("Quit", fleet::Quit)],
                disabled: false,
            }]);
            cx.on_window_closed(|cx: &mut App, _window_id| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Fleet".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(gpui::point(px(12.0), px(12.0))),
                }),
                ..Default::default()
            };
            match cx.open_window(options, |_window, cx| cx.new(|cx| Shell::new(home, cx))) {
                Ok(window) => {
                    let _ignored = window.update(cx, |_, window, _| window.activate_window());
                    cx.activate(true);
                    // Developer-only: drive the GUI from a script file (docs/DEVELOPMENT.md).
                    if let Some(script) = drive::script_path() {
                        let _ignored = window.update(cx, |_, window, cx| {
                            drive::spawn(script, window, cx).detach();
                        });
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "fleet: could not open the window");
                    eprintln!("fleet: could not open the window: {error}");
                    cx.quit();
                }
            }
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_prefix_key_resolves_from_live_state_before_repaint() {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.screen = Screen::Workspace {
            session: "owner/repo"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        };
        state.enter_prefix();

        let key = Keystroke::parse("s").unwrap_or_else(|error| panic!("{error}"));
        let (consumed, action) = take_live_prefix_action(&mut state, &key);
        assert!(consumed);
        assert_eq!(
            action
                .unwrap_or_else(|| panic!("ctrl-s s must resolve"))
                .name(),
            Action::name(&prefix::GoHub)
        );
        assert_eq!(state.terminal_mode, TerminalMode::Terminal);

        let unknown = Keystroke::parse("d").unwrap_or_else(|error| panic!("{error}"));
        state.enter_prefix();
        let (consumed, action) = take_live_prefix_action(&mut state, &unknown);
        assert!(
            consumed,
            "an unknown second key is still a one-shot prefix key"
        );
        assert!(action.is_none());
        assert_eq!(state.terminal_mode, TerminalMode::Terminal);
    }

    #[test]
    fn live_prefix_never_steals_a_key_from_an_overlay() {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.screen = Screen::Workspace {
            session: "owner/repo"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        };
        state.enter_prefix();
        state.open_overlay(Overlay::Jobs);

        let key = Keystroke::parse("s").unwrap_or_else(|error| panic!("{error}"));
        let (consumed, action) = take_live_prefix_action(&mut state, &key);

        assert!(!consumed);
        assert!(action.is_none());
        assert_eq!(state.terminal_mode, TerminalMode::Prefix);
    }

    #[test]
    fn import_is_rejected_for_a_stale_first_run_context_when_state_exists() {
        assert!(first_run_import_allowed(true, false));
        assert!(!first_run_import_allowed(true, true));
        assert!(!first_run_import_allowed(false, false));
        assert!(!first_run_import_allowed(false, true));
    }
}
