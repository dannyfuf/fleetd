//! [`Shell`]: the window's only view.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    hash::{Hash, Hasher},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use fleet_core::config::Agent;
use fleet_proto::{request::RequestBody, response::ResponseBody};
use fleet_ui_kit::{ActiveTheme, AppFrame, Icon, KitAssets, Theme, ThemeMode, ToastStack, Veil};
use gpui::{
    Action, AnyElement, App, Bounds, Context, DispatchPhase, Div, Entity, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, Keystroke, Menu, MenuItem, MouseDownEvent, MouseEvent,
    MouseExitEvent, MouseMoveEvent, MousePressureEvent, MouseUpEvent, PinchEvent, PlatformInput,
    Render, ScrollWheelEvent, Subscription, Task, TitlebarOptions, Window, WindowBounds,
    WindowOptions, canvas, deferred, div, prelude::*, px, size,
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
    screens::{
        agent_popup::AgentPopup, hub::HubScreen, jobs::JobsPanel, workspace::WorkspaceScreen,
    },
    shell::{
        chrome, daemon,
        quit::{QuitDecision, StopDecision, quit_decision, running_count, stop_decision},
    },
    state::{
        AgentPopupTransition, AppState, DaemonLink, HubPane, HubTab, Overlay, Screen, TerminalMode,
    },
};

/// How often the shell wakes to expire toasts and advance the daemon countdown.
const TICK: Duration = Duration::from_millis(250);
/// The window's default and minimum size (§0).
const DEFAULT_SIZE: (f32, f32) = (1280.0, 800.0);
/// The smallest window the ladders of §2.9 are defined for.
const MIN_SIZE: (f32, f32) = (900.0, 560.0);
/// Maximum input retained while GPUI is painting a new keyboard-focus owner.
const STALE_KEY_CAPACITY: usize = 64;
/// Paint after Fleet's deferred surfaces so their capture bookkeeping runs before this gate.
const POINTER_GATE_PRIORITY: usize = usize::MAX;

/// The authoritative focus-owner generation, keyboard replay marker, and queued stale input.
#[derive(Debug)]
struct FocusOwnerKeys {
    live_owner: Vec<&'static str>,
    live_generation: u64,
    replayed_generation: u64,
    queued: VecDeque<KeyDownEvent>,
    /// The interceptor saw a stale key; the root capture listener still needs to retain its
    /// complete `KeyDownEvent`, which the interceptor API does not expose.
    awaiting_capture: bool,
}

impl FocusOwnerKeys {
    fn new(live_owner: Vec<&'static str>) -> Self {
        Self {
            live_owner,
            // The initial element tree has not been painted yet.
            live_generation: 1,
            replayed_generation: 0,
            queued: VecDeque::new(),
            awaiting_capture: false,
        }
    }

    /// Bumps the generation exactly when the authoritative keyboard owner changes.
    fn sync_owner(&mut self, live_owner: Vec<&'static str>) -> (u64, bool) {
        let mut changed = false;
        if self.live_owner != live_owner {
            self.live_owner = live_owner;
            self.live_generation = self.live_generation.wrapping_add(1);
            changed = true;
        }
        (self.live_generation, changed)
    }

    fn is_stale(&self) -> bool {
        self.live_generation != self.replayed_generation
    }

    fn should_queue(&self) -> bool {
        self.is_stale() || !self.queued.is_empty()
    }

    fn await_capture(&mut self) {
        self.awaiting_capture = true;
    }

    /// Queues the complete stale-frame event from the root capture listener.
    fn capture(&mut self, event: KeyDownEvent) -> Option<KeyDownEvent> {
        if !self.awaiting_capture {
            return None;
        }
        self.awaiting_capture = false;
        let dropped = if self.queued.len() == STALE_KEY_CAPACITY {
            self.queued.pop_front()
        } else {
            None
        };
        self.queued.push_back(event);
        dropped
    }

    /// Marks a completed frame current and yields its queued input. An older callback can never
    /// move the rendered generation backwards after a newer owner has become authoritative.
    fn finish_render(&mut self, generation: u64) -> Vec<KeyDownEvent> {
        if self.live_generation != generation {
            return Vec::new();
        }
        self.replayed_generation = generation;
        self.queued.drain(..).collect()
    }

    /// Whether an event resolved against this painted frame may reach its pointer handlers.
    fn rejects_pointer(
        &self,
        painted_generation: u64,
        rendered_base_exposed: bool,
        live_agent_open: bool,
    ) -> bool {
        self.live_generation != painted_generation || live_agent_open && rendered_base_exposed
    }
}

/// Takes the one key following `ctrl-s` from authoritative state, before GPUI resolves it
/// against a possibly older context tree. Bound and unbound keys both leave Prefix first.
fn take_live_prefix_action(
    state: &mut AppState,
    keystroke: &Keystroke,
) -> (bool, Option<Box<dyn Action>>) {
    let chain = state.context_chain();
    let banner_attached = matches!(chain.as_slice(), [_, "Prefix", "Daemon", "Banner"]);
    if banner_attached && keymap::action_for_keystroke("Daemon > Banner", keystroke).is_some() {
        return (false, None);
    }
    let context = match chain.as_slice() {
        ["Workspace", "Prefix"] | ["Workspace", "Prefix", "Daemon", "Banner"] => {
            state.leave_prefix();
            "Workspace > Prefix"
        }
        ["Agent", "Prefix"] | ["Agent", "Prefix", "Daemon", "Banner"] => {
            state.leave_agent_prefix();
            "Agent > Prefix"
        }
        _ => return (false, None),
    };
    (true, keymap::action_for_keystroke(context, keystroke))
}

/// The shell-owned context chain, including the doctor surface synthesized during rendering.
fn focus_owner(state: &AppState) -> Vec<&'static str> {
    if state.doctor.is_some() && state.overlay.is_none() {
        vec!["Daemon", "Doctor"]
    } else {
        state.context_chain()
    }
}

/// Belt-and-braces guard for the popup's global-quit override.
fn popup_owns_ctrl_q(state: &AppState, keystroke: &Keystroke) -> bool {
    let modifiers = keystroke.modifiers;
    state.overlay.is_none()
        && focus_owner(state).first() == Some(&"Agent")
        && keystroke.key == "q"
        && modifiers.control
        && !modifiers.alt
        && !modifiers.shift
        && !modifiers.platform
        && !modifiers.function
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AgentEnsureKey {
    agent: Agent,
    generation: u64,
}

impl Hash for AgentEnsureKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        AgentEnsureFlights::agent_index(self.agent).hash(state);
        self.generation.hash(state);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AgentEnsureClaim {
    key: AgentEnsureKey,
    nonce: u64,
}

#[derive(Debug, Default)]
struct AgentEnsureFlights {
    in_flight: HashMap<AgentEnsureKey, u64>,
    current: [Option<u64>; 2],
    next_nonce: u64,
}

impl AgentEnsureFlights {
    const fn agent_index(agent: Agent) -> usize {
        match agent {
            Agent::Claude => 0,
            Agent::Opencode => 1,
        }
    }

    fn claim(&mut self, key: AgentEnsureKey) -> Option<AgentEnsureClaim> {
        if self.in_flight.contains_key(&key) {
            return None;
        }
        self.next_nonce = self.next_nonce.wrapping_add(1);
        let claim = AgentEnsureClaim {
            key,
            nonce: self.next_nonce,
        };
        self.in_flight.insert(key, claim.nonce);
        self.current[Self::agent_index(key.agent)] = Some(claim.nonce);
        Some(claim)
    }

    /// Finishes an exact flight and reports whether its response still owns the agent claim.
    fn finish(&mut self, claim: AgentEnsureClaim) -> bool {
        if self.in_flight.get(&claim.key) != Some(&claim.nonce) {
            return false;
        }
        self.in_flight.remove(&claim.key);
        let current = &mut self.current[Self::agent_index(claim.key.agent)];
        if *current != Some(claim.nonce) {
            return false;
        }
        *current = None;
        true
    }
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
    /// Focused while the floating agent terminal is the topmost surface.
    agent_focus: FocusHandle,
    hub: HubScreen,
    workspace: WorkspaceScreen,
    agent_popup: AgentPopup,
    /// Single-flight EnsureSession claims retained across popup hide/switch transitions.
    agent_ensures: AgentEnsureFlights,
    /// Generation gate and bounded input queue shared with the application-wide interceptor.
    focus_owner_keys: Rc<RefCell<FocusOwnerKeys>>,
    jobs: JobsPanel,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl Shell {
    /// Builds the shell, starts the daemon bridge and wires both background loops.
    pub fn new(home: PathBuf, cx: &mut Context<Self>) -> Self {
        let bridge = Bridge::start(home.clone());
        let state = cx.new(|_| AppState::new(home, Instant::now()));
        let workspace = WorkspaceScreen::new(cx);
        let agent_popup = AgentPopup::new(cx);
        let agent_input = agent_popup.input();
        let focus_owner_keys = Rc::new(RefCell::new(FocusOwnerKeys::new(focus_owner(
            state.read(cx),
        ))));

        let mut subscriptions = Vec::new();
        let observed_keys = Rc::clone(&focus_owner_keys);
        subscriptions.push(cx.observe(&state, move |_, state, cx| {
            observed_keys
                .borrow_mut()
                .sync_owner(focus_owner(state.read(cx)));
            // Every authoritative owner transition must dirty the window. GPUI redraws dirty
            // windows synchronously before keyboard dispatch; the generation queue below is the
            // safety net for any transition that somehow outruns that repaint.
            cx.notify();
        }));
        // GPUI resolves key bindings and raw key listeners against the last rendered dispatch
        // tree. Queue every key received while the authoritative focus owner is ahead of that
        // tree; the render callback below replays it through normal GPUI dispatch in order.
        let key_state = state.clone();
        let key_bridge = bridge.clone();
        let intercepted_keys = Rc::clone(&focus_owner_keys);
        subscriptions.push(cx.intercept_keystrokes(move |event, window, cx| {
            let (prefix_consumed, prefix_action) = key_state.update(cx, |state, cx| {
                let result = take_live_prefix_action(state, &event.keystroke);
                if result.0 {
                    cx.notify();
                }
                result
            });
            if prefix_consumed {
                if let Some(action) = prefix_action {
                    window.dispatch_action(action, cx);
                }
                cx.stop_propagation();
                return;
            }

            let popup_ctrl_q = popup_owns_ctrl_q(key_state.read(cx), &event.keystroke);
            let mut keys = intercepted_keys.borrow_mut();
            keys.sync_owner(focus_owner(key_state.read(cx)));
            if keys.should_queue() {
                // `intercept_keystrokes` exposes only a Keystroke. Stopping here still enters
                // GPUI's raw capture path, where the root retains the full KeyDownEvent.
                keys.await_capture();
                drop(keys);
                window.refresh();
                cx.stop_propagation();
                return;
            }
            drop(keys);

            // This deliberately remains independent of the generation gate: even if a future
            // caller bypasses or accidentally marks the queue current, popup ctrl-q can only
            // execute Hide and can never fall through to Fleet's global Quit binding.
            if popup_ctrl_q {
                agent_input.hide(&key_state, &key_bridge, cx);
                cx.stop_propagation();
            }
        }));

        let tasks = vec![Self::spawn_event_loop(&bridge, cx), Self::spawn_ticker(cx)];

        Self {
            state,
            bridge,
            body_focus: cx.focus_handle(),
            overlay_focus: cx.focus_handle(),
            agent_focus: cx.focus_handle(),
            hub: HubScreen::new(cx),
            workspace,
            agent_popup,
            agent_ensures: AgentEnsureFlights::default(),
            focus_owner_keys,
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
                    shell.reconcile_agent_session(cx);
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

    fn open_agent_claude(
        &mut self,
        _: &fleet::OpenAgentClaude,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_agent(Agent::Claude, cx);
    }

    fn open_agent_opencode(
        &mut self,
        _: &fleet::OpenAgentOpencode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_agent(Agent::Opencode, cx);
    }

    /// Opens or switches the independent popup and ensures its fixed daemon session exists.
    fn toggle_agent(&mut self, agent: Agent, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let hiding_current = state.agent_popup.is_some_and(|popup| popup.agent == agent);
        let preserve = state
            .active_session()
            .and_then(|session| session.active_terminal);
        if state.refuses_mutations() && !hiding_current {
            return;
        }
        let transition = self.state.update(cx, |state, cx| {
            let transition = state.toggle_agent_popup(agent);
            cx.notify();
            transition
        });
        if matches!(
            transition,
            AgentPopupTransition::Hidden | AgentPopupTransition::Switched
        ) {
            self.agent_popup.detach(&self.bridge, preserve);
        }
        if transition == AgentPopupTransition::Hidden {
            return;
        }
        self.reconcile_agent_session(cx);
    }

    /// Keeps a visible fixed agent session present, with one request per agent/link generation.
    fn reconcile_agent_session(&mut self, cx: &mut Context<Self>) {
        if !self.state.read(cx).daemon.is_connected() {
            self.agent_popup.discard_pending();
            return;
        }
        let Some(agent) = self.state.read(cx).missing_agent_popup_session() else {
            return;
        };
        let key = AgentEnsureKey {
            agent,
            generation: self.state.read(cx).link_generation,
        };
        let Some(claim) = self.agent_ensures.claim(key) else {
            return;
        };
        let reply = self.bridge.request(RequestBody::EnsureSession {
            worktree: None,
            agent: Some(agent),
            sleep_previous: false,
        });
        cx.spawn(async move |shell, cx| {
            let answer = reply.recv().await;
            let _ = shell.update(cx, |shell, cx| {
                if !shell.agent_ensures.finish(claim) {
                    return;
                }
                match answer {
                    Ok(Ok(ResponseBody::Session(session))) => {
                        shell.state.update(cx, |app, cx| {
                            app.apply_session(session);
                            cx.notify();
                        });
                    }
                    Ok(Err(error)) => shell.fail_agent_ensure(claim.key, error.message, cx),
                    Ok(Ok(_)) => shell.fail_agent_ensure(
                        claim.key,
                        "daemon returned an unexpected response; press a/A to retry".to_owned(),
                        cx,
                    ),
                    // A lost reply accompanies a bridge disconnect. The reconnect event advances
                    // the generation and re-enters this single-flight gate.
                    Err(_) => {}
                }
            });
        })
        .detach();
    }

    fn fail_agent_ensure(
        &mut self,
        claim: AgentEnsureKey,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let still_selected = self.state.read(cx).agent_popup.is_some_and(|popup| {
            popup.agent == claim.agent && self.state.read(cx).link_generation == claim.generation
        });
        if !still_selected {
            return;
        }
        let preserve = self
            .state
            .read(cx)
            .active_session()
            .and_then(|session| session.active_terminal);
        self.agent_popup.detach(&self.bridge, preserve);
        self.state.update(cx, |app, cx| {
            app.hide_agent_popup();
            app.sticky_error = Some(crate::state::StickyError {
                text: format!("agent popup could not start: {message}"),
                job: None,
                retryable: false,
            });
            cx.notify();
        });
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
            // A Fleet-drawn tab has no scrollback to enter: there is no PTY behind it and the
            // pane scrolls itself. Saying so is better than a Scroll mode that answers nothing.
            let native = state.active_terminal_is_native();
            state.leave_prefix();
            if native {
                state.toast_short(
                    "no scrollback in this tab",
                    Icon::ChevronsUp,
                    Instant::now(),
                );
            } else if alt_screen {
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
                state.terminal_mode = state.resting_terminal_mode();
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
        let preserve = self
            .state
            .read(cx)
            .active_session()
            .and_then(|session| session.active_terminal);
        self.agent_popup.detach(&self.bridge, preserve);
        self.state.update(cx, |state, cx| {
            state.hide_agent_popup();
            state.open_overlay(Overlay::Jobs);
            cx.notify();
        });
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

    /// Drains one completed root render's queued keystrokes while the Shell is leased.
    fn drain_stale_keys_after_render(
        &mut self,
        rendered_generation: u64,
        cx: &mut Context<Self>,
    ) -> (Vec<KeyDownEvent>, bool) {
        let mut keys = self.focus_owner_keys.borrow_mut();
        let (_, changed) = keys.sync_owner(focus_owner(self.state.read(cx)));
        (keys.finish_render(rendered_generation), changed)
    }

    fn register_pointer_gate<Event: MouseEvent>(
        window: &mut Window,
        state: Entity<AppState>,
        focus_owner_keys: Rc<RefCell<FocusOwnerKeys>>,
        painted_generation: u64,
        rendered_base_exposed: bool,
    ) {
        window.on_mouse_event(move |_: &Event, phase, _window, cx| {
            if phase == DispatchPhase::Capture
                && focus_owner_keys.borrow().rejects_pointer(
                    painted_generation,
                    rendered_base_exposed,
                    state.read(cx).agent_popup.is_some(),
                )
            {
                cx.stop_propagation();
            }
        });
    }

    /// A paint-phase root listener that rejects pointer input before any bubble handler sees it.
    fn pointer_gate(
        state: Entity<AppState>,
        focus_owner_keys: Rc<RefCell<FocusOwnerKeys>>,
        painted_generation: u64,
        rendered_base_exposed: bool,
    ) -> AnyElement {
        canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                macro_rules! register {
                    ($event:ty) => {
                        Self::register_pointer_gate::<$event>(
                            window,
                            state.clone(),
                            Rc::clone(&focus_owner_keys),
                            painted_generation,
                            rendered_base_exposed,
                        );
                    };
                }
                register!(MouseDownEvent);
                register!(MouseUpEvent);
                register!(MouseMoveEvent);
                register!(MouseExitEvent);
                register!(MousePressureEvent);
                register!(ScrollWheelEvent);
                register!(PinchEvent);
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }

    /// Defers the gate above every Fleet overlay. GPUI visits capture listeners forward and bubble
    /// listeners backward, so this preserves capture bookkeeping while still suppressing every
    /// stale bubble handler.
    fn after_pointer_handlers(root: Div, pointer_gate: AnyElement) -> Div {
        root.child(deferred(pointer_gate).with_priority(POINTER_GATE_PRIORITY))
    }

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
        crate::views::watch_pane::sync(&state_handle, &bridge, cx);

        // Everything below reads the state immutably; the borrow ends before the screens are
        // rendered, which is the only place that needs `&mut App`.
        let doctor = state_handle.read(cx).doctor.clone();
        let (mut chain, overlay, agent_open, screen, mut splash, banner, veil, first_run) = {
            let state = state_handle.read(cx);
            (
                state.context_chain(),
                state.overlay.clone(),
                state.agent_popup.is_some(),
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
        let (rendered_generation, owner_changed) =
            self.focus_owner_keys.borrow_mut().sync_owner(chain.clone());
        if owner_changed {
            // This draw is already assembling the new tree; keeping the window dirty also makes
            // the generation invariant explicit for synthetic owners such as Doctor.
            window.refresh();
        }
        // Registered during this draw, so GPUI runs it on the following frame request after the
        // dispatch tree assembled below has become `rendered_frame`.
        let shell = cx.entity();
        window.on_next_frame(move |window, cx| {
            let (queued, owner_changed) = shell.update(cx, |shell, cx| {
                shell.drain_stale_keys_after_render(rendered_generation, cx)
            });
            if owner_changed {
                window.refresh();
            }
            for event in queued {
                // Dispatch only after `shell.update` has returned and released its entity lease.
                // A replay that changes owner causes later events to re-enter the bounded FIFO.
                window.dispatch_event(PlatformInput::KeyDown(event), cx);
            }
        });
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
        let agent_focus = self.agent_focus.clone();

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
            let pointer_gate = Self::pointer_gate(
                state_handle,
                Rc::clone(&self.focus_owner_keys),
                rendered_generation,
                false,
            );
            let root = Self::with_actions(
                div()
                    .relative()
                    .size_full()
                    .key_context(ROOT_CONTEXT)
                    .bg(cx.theme().colors.bg),
                Rc::clone(&self.focus_owner_keys),
                cx,
            )
            .child(surface)
            .children(layer);
            return Self::after_pointer_handlers(root, pointer_gate).into_any_element();
        }
        // Set only by the Workspace arm below, so a screen that never renders the Workspace can
        // never inherit a stale "the pane has the keyboard" from the last time it did.
        let mut pane_owns = false;
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
                    pane_owns = self.workspace.pane_owns_keyboard();
                    Veil::new(veil).child(workspace).into_any_element()
                }
            }
        };

        // Reconcile the base Workspace before the popup. If both display the same fixed agent
        // terminal, the Workspace must release its set-valued daemon attachment first and the
        // popup must claim it last; the opposite order would leave neither view attached.
        // This is not the ordinary overlay slot: it persists under Help and quit confirms, so
        // those dialogs close back to the same live agent terminal.
        let agent_element = agent_open.then(|| {
            self.agent_popup
                .render(&state_handle, &bridge, &agent_focus, window, cx)
        });

        // The focused element carries the key-context chain: the overlay when one is open,
        // the body otherwise, so overlays really do shadow the Hub and the Workspace.
        let (body, agent_element, overlay_element) = match (agent_element, overlay_element) {
            (Some(agent), Some(overlay)) => (
                body,
                Some(agent),
                Some(Self::overlay_contexts(&chain, overlay)),
            ),
            (Some(agent), None) => (body, Some(Self::overlay_contexts(&chain, agent)), None),
            (None, Some(overlay)) => (body, None, Some(Self::overlay_contexts(&chain, overlay))),
            (None, None) => (Self::contexts(&chain, body), None, None),
        };

        // Exactly one of the two handles is focused, and it is the one attached to the element
        // that carries the key-context chain.
        let wanted = if overlay_element.is_some() {
            &self.overlay_focus
        } else if agent_element.is_some() {
            &self.agent_focus
        } else {
            &self.body_focus
        };
        // …with one exception. A Fleet-drawn tab is a gpui view of its own, nested inside the
        // `Workspace > Native` context, and it must hold the keyboard for its own bindings to
        // resolve. Its focus handle is a descendant of this element, so the whole chain —
        // `Fleet > Workspace > Native > Lazygit > …` — still reaches the shell's own actions.
        // Taking focus back here every frame is what would break it.
        let pane_owns = pane_owns && overlay_element.is_none() && agent_element.is_none();
        if !pane_owns && !wanted.is_focused(window) {
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
        if let Some(agent) = agent_element {
            frame = frame.overlay(agent);
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

        let pointer_gate = Self::pointer_gate(
            state_handle,
            Rc::clone(&self.focus_owner_keys),
            rendered_generation,
            !agent_open,
        );
        let root = Self::with_actions(
            div()
                .size_full()
                .key_context(ROOT_CONTEXT)
                .bg(cx.theme().colors.bg),
            Rc::clone(&self.focus_owner_keys),
            cx,
        )
        .child(frame);
        Self::after_pointer_handlers(root, pointer_gate).into_any_element()
    }
}

impl Shell {
    /// Installs every global action listener on `root`.
    ///
    /// Both branches of [`Shell::render`] use it, splash included: a surface that registers a
    /// subset silently swallows the keys it left out — `ctrl-shift-q` and `Esc` among them —
    /// and there is no way back out of whatever the missing key was meant to leave.
    fn with_actions(
        root: Div,
        focus_owner_keys: Rc<RefCell<FocusOwnerKeys>>,
        cx: &mut Context<Self>,
    ) -> Div {
        root.capture_key_down(move |event: &KeyDownEvent, _window, cx| {
            let mut keys = focus_owner_keys.borrow_mut();
            if !keys.awaiting_capture {
                return;
            }
            if let Some(dropped) = keys.capture(event.clone()) {
                tracing::warn!(
                    keystroke = %dropped.keystroke,
                    capacity = STALE_KEY_CAPACITY,
                    "dropping oldest stale-frame key event"
                );
            }
            cx.stop_propagation();
        })
        // Global
        .on_action(cx.listener(Self::quit))
        .on_action(cx.listener(Self::quit_and_stop_daemon))
        .on_action(cx.listener(Self::open_palette))
        .on_action(cx.listener(Self::open_settings))
        .on_action(cx.listener(Self::open_help))
        .on_action(cx.listener(Self::open_jobs))
        .on_action(cx.listener(Self::open_agent_claude))
        .on_action(cx.listener(Self::open_agent_opencode))
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
            // The native git pane brings its own key table. Its contexts all sit under its own
            // `Lazygit` root, which the Workspace renders inside `Fleet > Workspace > Native`,
            // so its bindings are reachable exactly there and nowhere else. Theme and assets
            // stay installed once, above, because there is one window and one design system.
            fleet_lazygit::keymap::init(cx);
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

    fn key_down(keys: &str) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke::parse(keys).unwrap_or_else(|error| panic!("{error}")),
            is_held: false,
            prefer_character_input: false,
        }
    }

    fn queue(keys: &mut FocusOwnerKeys, event: KeyDownEvent) -> Option<KeyDownEvent> {
        keys.await_capture();
        keys.capture(event)
    }

    fn rendered(keys: &mut FocusOwnerKeys, generation: u64) -> Vec<KeyDownEvent> {
        keys.finish_render(generation)
    }

    #[test]
    fn stale_keys_wait_for_their_focus_owner_generation_to_render() {
        let mut keys = FocusOwnerKeys::new(vec!["Hub", "Worktrees"]);
        let key = key_down("j");

        assert!(keys.is_stale(), "the initial tree has not painted");
        assert!(queue(&mut keys, key.clone()).is_none());
        assert!(rendered(&mut keys, 0).is_empty());
        assert_eq!(rendered(&mut keys, 1), vec![key]);
        assert!(!keys.is_stale());

        let (generation, _) = keys.sync_owner(vec!["Agent", "Terminal"]);
        let key = key_down("x");
        assert!(queue(&mut keys, key.clone()).is_none());
        assert!(rendered(&mut keys, generation - 1).is_empty());
        assert_eq!(rendered(&mut keys, generation), vec![key]);
    }

    #[test]
    fn stale_key_queue_is_bounded_and_drops_the_oldest_key() {
        let mut keys = FocusOwnerKeys::new(vec!["Hub", "Worktrees"]);
        for index in 0..=STALE_KEY_CAPACITY {
            let mut event = key_down("a");
            event.keystroke.key = format!("key-{index}");
            let dropped = queue(&mut keys, event);
            if index < STALE_KEY_CAPACITY {
                assert!(dropped.is_none());
            } else {
                assert_eq!(
                    dropped.map(|event| event.keystroke.key),
                    Some("key-0".to_owned())
                );
            }
        }
        assert_eq!(keys.queued.len(), STALE_KEY_CAPACITY);
        assert_eq!(
            keys.queued
                .front()
                .map(|event| event.keystroke.key.as_str()),
            Some("key-1")
        );
    }

    #[test]
    fn stale_key_queue_preserves_keydown_metadata() {
        let mut keys = FocusOwnerKeys::new(vec!["Workspace", "Terminal"]);
        let mut event = key_down("a");
        event.keystroke.key_char = Some("á".to_owned());
        event.is_held = true;
        event.prefer_character_input = true;

        assert!(queue(&mut keys, event.clone()).is_none());
        assert_eq!(rendered(&mut keys, 1), vec![event]);
    }

    #[test]
    fn replay_is_drained_before_dispatch_and_later_keys_requeue_after_a_transition() {
        let mut keys = FocusOwnerKeys::new(vec!["Workspace", "Terminal"]);
        let first = key_down("ctrl-s");
        let second = key_down("]");
        queue(&mut keys, first.clone());
        queue(&mut keys, second.clone());
        let replay = rendered(&mut keys, 1);
        assert_eq!(replay, vec![first, second.clone()]);
        assert!(
            keys.queued.is_empty(),
            "the Shell lease drains the batch before dispatch begins"
        );

        // Simulate the first replay entering Prefix before the callback dispatches the second.
        let (prefix_generation, _) = keys.sync_owner(vec!["Workspace", "Prefix"]);
        assert!(keys.is_stale());
        queue(&mut keys, replay[1].clone());
        assert_eq!(rendered(&mut keys, prefix_generation), vec![second]);
    }

    #[test]
    fn workspace_prefix_consumes_bound_and_unbound_keys_with_daemon_banner() {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.screen = Screen::Workspace {
            session: "owner/repo"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        };
        state.daemon = DaemonLink::Lost {
            attempt: 1,
            dismissed: false,
        };

        state.enter_prefix();
        let bound = Keystroke::parse("c").unwrap_or_else(|error| panic!("{error}"));
        let (consumed, action) = take_live_prefix_action(&mut state, &bound);
        assert!(consumed);
        assert_eq!(
            action.as_ref().map(|action| action.name()),
            Some(Action::name(&prefix::NewTerminal))
        );
        assert_eq!(state.terminal_mode, TerminalMode::Terminal);

        for (keys, expected) in [
            ("v", Action::name(&prefix::ToggleWatchPane)),
            ("V", Action::name(&prefix::DismissWatch)),
            ("N", Action::name(&prefix::NextWatch)),
            ("P", Action::name(&prefix::PrevWatch)),
        ] {
            state.enter_prefix();
            let keystroke = Keystroke::parse(keys).unwrap_or_else(|error| panic!("{error}"));
            let (consumed, action) = take_live_prefix_action(&mut state, &keystroke);
            assert!(consumed, "{keys} must consume the live one-shot prefix");
            assert_eq!(action.as_ref().map(|action| action.name()), Some(expected));
            assert_eq!(state.terminal_mode, TerminalMode::Terminal);
        }

        state.enter_prefix();
        let unbound = Keystroke::parse("d").unwrap_or_else(|error| panic!("{error}"));
        let (consumed, action) = take_live_prefix_action(&mut state, &unbound);
        assert!(consumed);
        assert!(action.is_none());
        assert_eq!(state.terminal_mode, TerminalMode::Terminal);
    }

    #[test]
    fn agent_prefix_consumes_bound_and_unbound_keys_and_restores_scroll_with_daemon_banner() {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.toggle_agent_popup(Agent::Claude);
        state.agent_popup.as_mut().expect("popup open").mode = crate::state::AgentPopupMode::Scroll;
        state.daemon = DaemonLink::Lost {
            attempt: 1,
            dismissed: false,
        };

        state.enter_agent_prefix();
        let bound = Keystroke::parse("]").unwrap_or_else(|error| panic!("{error}"));
        let (consumed, action) = take_live_prefix_action(&mut state, &bound);
        assert!(consumed);
        assert_eq!(
            action.as_ref().map(|action| action.name()),
            Some(Action::name(&prefix::Paste))
        );
        assert_eq!(
            state.agent_popup.map(|popup| popup.mode),
            Some(crate::state::AgentPopupMode::Scroll)
        );

        state.enter_agent_prefix();
        let unbound = Keystroke::parse("d").unwrap_or_else(|error| panic!("{error}"));
        let (consumed, action) = take_live_prefix_action(&mut state, &unbound);
        assert!(consumed);
        assert!(action.is_none());
        assert_eq!(
            state.agent_popup.map(|popup| popup.mode),
            Some(crate::state::AgentPopupMode::Scroll)
        );
    }

    #[test]
    fn daemon_banner_bindings_resolve_before_live_prefix_fallback() {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.screen = Screen::Workspace {
            session: "owner/repo"
                .parse()
                .unwrap_or_else(|error| panic!("{error}")),
        };
        state.daemon = DaemonLink::Lost {
            attempt: 1,
            dismissed: false,
        };

        for key in ["r", "l", "escape"] {
            state.enter_prefix();
            let keystroke = Keystroke::parse(key).unwrap_or_else(|error| panic!("{error}"));
            assert!(!take_live_prefix_action(&mut state, &keystroke).0);
            assert_eq!(state.terminal_mode, TerminalMode::Prefix);
        }
    }

    #[test]
    fn pointer_gate_rejects_stale_frames_and_base_frames_behind_live_agent() {
        let mut keys = FocusOwnerKeys::new(vec!["Workspace", "Terminal"]);
        assert!(
            !keys.rejects_pointer(1, false, false),
            "a frame painted at the live generation is immediately current"
        );
        keys.sync_owner(vec!["Workspace", "Prefix"]);
        assert!(
            keys.rejects_pointer(1, false, false),
            "an owner transition rejects the older painted generation"
        );
        assert!(keys.rejects_pointer(2, true, true));
        assert!(!keys.rejects_pointer(2, false, true));
    }

    #[test]
    fn pointer_gate_paints_after_every_overlay_layer() {
        assert!(POINTER_GATE_PRIORITY > fleet_ui_kit::OverlayLayer::Toast.priority());
    }

    #[test]
    fn popup_ctrl_q_guard_is_exact() {
        let mut state = AppState::new("/tmp/fleet", Instant::now());
        state.toggle_agent_popup(Agent::Claude);
        let quit = Keystroke::parse("ctrl-q").unwrap_or_else(|error| panic!("{error}"));
        assert!(popup_owns_ctrl_q(&state, &quit));
        let stop = Keystroke::parse("ctrl-shift-q").unwrap_or_else(|error| panic!("{error}"));
        assert!(!popup_owns_ctrl_q(&state, &stop));
        state.open_overlay(Overlay::Dialog(Dialogs::Help));
        assert!(
            !popup_owns_ctrl_q(&state, &quit),
            "a topmost overlay restores normal global ctrl-q handling"
        );
        state.close_overlay();
        assert!(popup_owns_ctrl_q(&state, &quit));
        state.hide_agent_popup();
        assert!(!popup_owns_ctrl_q(&state, &quit));
    }

    #[test]
    fn agent_ensure_is_single_flight_and_late_generations_cannot_claim_response() {
        let mut flights = AgentEnsureFlights::default();
        let old_key = AgentEnsureKey {
            agent: Agent::Claude,
            generation: 3,
        };
        let old = flights.claim(old_key).expect("first claim");
        assert!(
            flights.claim(old_key).is_none(),
            "same key stays single-flight"
        );

        let current = flights
            .claim(AgentEnsureKey {
                agent: Agent::Claude,
                generation: 4,
            })
            .expect("new link generation gets a distinct flight");
        assert!(
            !flights.finish(old),
            "late old response has lost its nonce claim"
        );
        assert!(flights.finish(current));
    }

    #[test]
    fn import_is_rejected_for_a_stale_first_run_context_when_state_exists() {
        assert!(first_run_import_allowed(true, false));
        assert!(!first_run_import_allowed(true, true));
        assert!(!first_run_import_allowed(false, false));
        assert!(!first_run_import_allowed(false, true));
    }
}
