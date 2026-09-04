//! [`Shell`]: the window's only view.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{ActiveTheme, AppFrame, Icon, KitAssets, Theme, ThemeMode, ToastStack, Veil};
use gpui::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable, IntoElement, Menu, MenuItem,
    Render, Subscription, Task, TitlebarOptions, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, size,
};

use crate::{
    actions::{
        confirm, daemon as daemon_actions, dialog, filter, fleet, help, hub, jobs, palette, prefix,
        quit_daemon_dialog, quit_dialog, scroll, workspace,
    },
    bridge::Bridge,
    dialogs::Dialogs,
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
    doctor: Option<Vec<fleet_proto::response::DoctorCheck>>,
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
        // The prefix is one-shot: whatever the next key was, and whether or not it matched a
        // binding, the mode goes back to Terminal. A keystroke observer is the only hook that
        // sees a key an action already consumed, so it is the only place this can live.
        subscriptions.push(cx.observe_keystrokes(
            |shell: &mut Self, _event, _window, cx: &mut Context<Self>| {
                shell.state.update(cx, |state, cx| {
                    if state.prefix_saw_key() {
                        cx.notify();
                    }
                });
            },
        ));

        let tasks = vec![Self::spawn_event_loop(&bridge, cx), Self::spawn_ticker(cx)];

        Self {
            state,
            bridge,
            body_focus: cx.focus_handle(),
            overlay_focus: cx.focus_handle(),
            hub: HubScreen::new(cx),
            workspace: WorkspaceScreen::new(cx),
            jobs: JobsPanel::new(cx),
            doctor: None,
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
                let updated = shell.update(cx, |shell, cx| {
                    shell.state.update(cx, |state, cx| {
                        state.apply_bridge_event(event, now);
                        cx.notify();
                    });
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
                        if state.tick(now) {
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
        if self.doctor.take().is_some() {
            cx.notify();
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
        cx.spawn(async move |shell, cx| {
            let Ok(Ok(ResponseBody::Doctor(checks))) = reply.recv().await else {
                return;
            };
            let _ignored = shell.update(cx, |shell, cx| {
                shell.doctor = Some(checks);
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------------ quit flow

    fn quit(&mut self, _: &fleet::Quit, _: &mut Window, cx: &mut Context<Self>) {
        let (warn, running) = {
            let state = self.state.read(cx);
            (
                state.warn_before_quit,
                state
                    .snapshot
                    .as_ref()
                    .map_or(0, |snapshot| running_count(&snapshot.jobs)),
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

    /// The §3.13 first-run card: a migration, not an onboarding.
    fn first_run_card(&self, state: &AppState, focus: &FocusHandle, cx: &App) -> AnyElement {
        let has_swarm = crate::views::first_run::has_swarm_state(
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
        let doctor = self.doctor.clone();
        let (mut chain, overlay, screen, mut splash, banner, veil, first_run) = {
            let state = state_handle.read(cx);
            (
                state.context_chain(),
                state.overlay.clone(),
                state.screen.clone(),
                daemon::splash(state, now, cx),
                daemon::banner(&state.daemon, now),
                state.drops_terminal_keys(),
                state.is_first_run(),
            )
        };
        if doctor.is_some() {
            chain = vec!["Daemon", "Doctor"];
            splash = None;
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

        // §3.12 A and B are full-window surfaces with no chrome at all.
        if let Some(splash) = splash {
            if !self.body_focus.is_focused(window) {
                window.focus(&self.body_focus, cx);
            }
            return div()
                .size_full()
                .key_context(ROOT_CONTEXT)
                .bg(cx.theme().colors.bg)
                .on_action(cx.listener(Self::quit))
                .on_action(cx.listener(Self::daemon_retry))
                .on_action(cx.listener(Self::daemon_open_log))
                .on_action(cx.listener(Self::run_doctor))
                .child(Self::contexts(
                    &chain,
                    div()
                        .track_focus(&self.body_focus)
                        .size_full()
                        .child(splash)
                        .into_any_element(),
                ))
                .into_any_element();
        }

        let focus = self.body_focus.clone();
        let overlay_focus = self.overlay_focus.clone();
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

        // The focused element carries the key-context chain: the overlay when one is open,
        // the body otherwise, so overlays really do shadow the Hub and the Workspace.
        let (body, overlay_element) = match overlay_element {
            Some(element) => (body, Some(Self::contexts(&chain, element))),
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
            .overlay(toasts);
        if let Some(banner) = banner {
            frame = frame.banner(banner);
        }
        if let Some(overlay) = overlay_element {
            frame = frame.overlay(overlay);
        }

        div()
            .size_full()
            .key_context(ROOT_CONTEXT)
            .bg(cx.theme().colors.bg)
            // Global
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::quit_and_stop_daemon))
            .on_action(cx.listener(Self::open_palette))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::open_help))
            .on_action(cx.listener(Self::open_jobs))
            .on_action(cx.listener(Self::focus_sticky_error))
            .on_action(cx.listener(Self::cancel))
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
            .child(frame)
            .into_any_element()
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

/// Opens the window and runs the app. Returns when the last window closes.
pub fn run() -> anyhow::Result<()> {
    let home = fleet_home();
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
                }
                Err(error) => {
                    eprintln!("fleet: could not open the window: {error}");
                    cx.quit();
                }
            }
        });
    Ok(())
}
