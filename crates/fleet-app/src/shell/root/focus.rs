use super::Shell;
use crate::{
    keymap,
    state::{AppState, DaemonLink, Screen},
};
use gpui::{
    Action, AnyElement, Context, DispatchPhase, Div, Entity, IntoElement, KeyDownEvent, Keystroke,
    MouseDownEvent, MouseEvent, MouseExitEvent, MouseMoveEvent, MousePressureEvent, MouseUpEvent,
    PinchEvent, PlatformInput, ScrollWheelEvent, Window, canvas, deferred, div, prelude::*,
};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

/// Maximum input retained while GPUI is painting a new keyboard-focus owner.
pub(super) const STALE_KEY_CAPACITY: usize = 64;
/// Paint after Fleet's deferred surfaces so their capture bookkeeping runs before this gate.
pub(super) const POINTER_GATE_PRIORITY: usize = usize::MAX;

/// The authoritative focus-owner generation, keyboard replay marker, and queued stale input.
#[derive(Debug)]
pub(super) struct FocusOwnerKeys {
    pub(super) live_owner: Vec<&'static str>,
    pub(super) live_generation: u64,
    pub(super) replayed_generation: u64,
    pub(super) queued: VecDeque<KeyDownEvent>,
    /// The interceptor saw a stale key; the root capture listener still needs to retain its
    /// complete `KeyDownEvent`, which the interceptor API does not expose.
    pub(super) awaiting_capture: bool,
}

impl FocusOwnerKeys {
    pub(super) fn new(live_owner: Vec<&'static str>) -> Self {
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
    pub(super) fn sync_owner(&mut self, live_owner: impl AsRef<[&'static str]>) -> (u64, bool) {
        let live_owner = live_owner.as_ref();
        let mut changed = false;
        if self.live_owner != live_owner {
            self.live_owner.clear();
            self.live_owner.extend_from_slice(live_owner);
            self.live_generation = self.live_generation.wrapping_add(1);
            changed = true;
        }
        (self.live_generation, changed)
    }

    pub(super) fn is_stale(&self) -> bool {
        self.live_generation != self.replayed_generation
    }

    pub(super) fn should_queue(&self) -> bool {
        self.is_stale() || !self.queued.is_empty()
    }

    pub(super) fn await_capture(&mut self) {
        self.awaiting_capture = true;
    }

    /// Queues the complete stale-frame event from the root capture listener.
    pub(super) fn capture(&mut self, event: KeyDownEvent) -> Option<KeyDownEvent> {
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
    pub(super) fn finish_render(&mut self, generation: u64) -> Vec<KeyDownEvent> {
        if self.live_generation != generation {
            return Vec::new();
        }
        self.replayed_generation = generation;
        self.queued.drain(..).collect()
    }

    /// Whether an event resolved against this painted frame may reach its pointer handlers.
    pub(super) fn rejects_pointer(
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
pub(super) fn take_live_prefix_action(
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

/// Doctor shadows the base screen whenever no overlay is open.
pub(super) fn focus_owner(state: &AppState) -> Vec<&'static str> {
    if state.doctor.is_some() && state.overlay.is_none() {
        vec!["Daemon", "Doctor"]
    } else {
        state.context_chain()
    }
}

/// The popup consumes plain ctrl-q before the global quit binding.
pub(super) fn popup_owns_ctrl_q(state: &AppState, keystroke: &Keystroke) -> bool {
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

/// Installs the two application-wide input gates the generation queue depends on: the observer
/// that makes every authoritative owner transition dirty the window, and the keystroke
/// interceptor that queues input arriving ahead of the painted dispatch tree.
pub(super) fn install_input_gates(
    state: &Entity<AppState>,
    bridge: &crate::bridge::Bridge,
    agent_input: crate::screens::agent_popup::AgentPopupInput,
    focus_owner_keys: &Rc<RefCell<FocusOwnerKeys>>,
    cx: &mut Context<Shell>,
) -> Vec<gpui::Subscription> {
    let observed_keys = Rc::clone(focus_owner_keys);
    // GPUI redraws dirty windows synchronously before keyboard dispatch; the generation queue
    // is the safety net for any transition that somehow outruns that repaint.
    let owner_observer = cx.observe(state, move |_, state, cx| {
        observed_keys
            .borrow_mut()
            .sync_owner(focus_owner(state.read(cx)));
        cx.notify();
    });

    // GPUI resolves key bindings and raw key listeners against the last rendered dispatch tree.
    // Queue every key received while the authoritative focus owner is ahead of that tree; the
    // next-frame callback in `prepare_key_replay` replays it through normal dispatch, in order.
    let key_state = state.clone();
    let key_bridge = bridge.clone();
    let intercepted_keys = Rc::clone(focus_owner_keys);
    let interceptor = cx.intercept_keystrokes(move |event, window, cx| {
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
            // `intercept_keystrokes` exposes only a Keystroke. Stopping here still enters GPUI's
            // raw capture path, where the root retains the full KeyDownEvent.
            keys.await_capture();
            drop(keys);
            window.refresh();
            cx.stop_propagation();
            return;
        }
        drop(keys);

        // This deliberately remains independent of the generation gate: even if a future caller
        // bypasses or accidentally marks the queue current, popup ctrl-q can only execute Hide
        // and can never fall through to Fleet's global Quit binding.
        if popup_ctrl_q {
            agent_input.hide(&key_state, &key_bridge, cx);
            cx.stop_propagation();
        }
    });

    vec![owner_observer, interceptor]
}

impl Shell {
    /// Drains one completed root render's queued keystrokes while the Shell is leased.
    pub(super) fn drain_stale_keys_after_render(
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
    pub(super) fn pointer_gate(
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
    pub(super) fn after_pointer_handlers(root: Div, pointer_gate: AnyElement) -> Div {
        root.child(deferred(pointer_gate).with_priority(POINTER_GATE_PRIORITY))
    }

    /// Wraps `child` in one div per key context, outermost first.
    ///
    /// The focused element is `child` itself — every screen and overlay tracks the shell's
    /// focus handle — so the dispatch path reads `Fleet > Hub > Worktrees > <screen>` and both
    /// the shell's listeners (above) and the screen's own (at the focus node) are on it.
    pub(super) fn contexts(chain: &[&'static str], child: AnyElement) -> AnyElement {
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

    /// Overlay context wrappers must be absolute: in-flow wrappers would consume the frame's
    /// flex column and move its persistent bars.
    pub(super) fn overlay_contexts(chain: &[&'static str], child: AnyElement) -> AnyElement {
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
}

impl Shell {
    pub(super) fn observe_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window = Some(window.window_handle());
        self._subscriptions
            .push(cx.observe_in(&self.state, window, |shell, _, window, cx| {
                shell.synchronize_surfaces(window, cx);
            }));
        self._subscriptions
            .push(
                cx.observe_global_in::<fleet_ui_kit::Theme>(window, |shell, window, cx| {
                    shell.synchronize_surfaces(window, cx);
                    cx.notify();
                }),
            );
        self._subscriptions
            .push(cx.observe_window_bounds(window, |shell, window, cx| {
                shell.synchronize_surfaces(window, cx);
                cx.notify();
            }));
        self.synchronize_surfaces(window, cx);
    }

    pub(super) fn synchronize_surfaces(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Daemon attachments are set-valued: the base releases before the popup claims.
        self.workspace
            .synchronize(&self.state, &self.bridge, window, cx);
        self.agent_popup.synchronize(&self.state, &self.bridge, cx);
        self.reconcile_focus(window, cx);
    }

    fn reconcile_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let target = match focus_target(state) {
            FocusTarget::Native if !self.workspace.pane_owns_keyboard() => FocusTarget::Body,
            target => target,
        };
        focus_surface(
            target,
            &self.body_focus,
            &self.overlay_focus,
            &self.agent_focus,
            window,
            cx,
        );
    }
}

#[derive(Clone, Copy)]
enum FocusTarget {
    Body,
    Overlay,
    Agent,
    Native,
    /// A native agent tab, whose composer takes the keyboard itself.
    AgentThread,
}

fn focus_target(state: &AppState) -> FocusTarget {
    let splash = state.doctor.is_none()
        && matches!(
            state.daemon,
            DaemonLink::Starting | DaemonLink::Failed { .. }
        );
    if state.overlay.is_some() {
        FocusTarget::Overlay
    } else if !splash && state.agent_popup.is_some() {
        FocusTarget::Agent
    } else if !splash
        && state.doctor.is_none()
        && !state.is_first_run()
        && matches!(state.screen, Screen::Workspace { .. })
        && state.active_agent_thread().is_some()
    {
        // APP-CONTRACTS §3: an agent tab's keys reach Fleet's own composer, which the thread
        // view focuses. Taking focus back for the shell body here would undo that on every
        // delta the thread receives.
        FocusTarget::AgentThread
    } else if !splash
        && state.doctor.is_none()
        && !state.is_first_run()
        && matches!(state.screen, Screen::Workspace { .. })
        && state.active_terminal_is_native()
    {
        FocusTarget::Native
    } else {
        FocusTarget::Body
    }
}

fn focus_surface(
    target: FocusTarget,
    body: &gpui::FocusHandle,
    overlay: &gpui::FocusHandle,
    agent: &gpui::FocusHandle,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let wanted = match target {
        FocusTarget::Body => body,
        FocusTarget::Overlay => overlay,
        FocusTarget::Agent => agent,
        // Native panes and agent tabs restore their own descendant focus on activation.
        FocusTarget::Native | FocusTarget::AgentThread => return,
    };
    if !wanted.is_focused(window) {
        window.focus(wanted, cx);
    }
}

impl Shell {
    pub(super) fn prepare_key_replay(
        &mut self,
        chain: &[&'static str],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> u64 {
        let (generation, owner_changed) = self.focus_owner_keys.borrow_mut().sync_owner(chain);
        if owner_changed {
            window.refresh();
        }
        if self.focus_owner_keys.borrow().should_queue() {
            let shell = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                let Ok((queued, owner_changed)) = shell.update(cx, |shell, cx| {
                    shell.drain_stale_keys_after_render(generation, cx)
                }) else {
                    return;
                };
                if owner_changed {
                    window.refresh();
                }
                // Release the Shell lease before dispatch; an owner-changing replay makes later
                // events re-enter the bounded FIFO through the application interceptor.
                for event in queued {
                    window.dispatch_event(PlatformInput::KeyDown(event), cx);
                }
            });
        }
        generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{FocusHandle, Render, Subscription, TestAppContext};
    use std::time::Instant;

    struct FocusFixture {
        state: Entity<AppState>,
        body: FocusHandle,
        overlay: FocusHandle,
        agent: FocusHandle,
        _subscription: Subscription,
    }

    impl Render for FocusFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(div().track_focus(&self.body))
                .child(div().track_focus(&self.overlay))
                .child(div().track_focus(&self.agent))
        }
    }

    #[gpui::test]
    fn state_observation_restores_focus_for_overlays_and_popup(cx: &mut TestAppContext) {
        let window = cx.add_window(|window, cx: &mut Context<FocusFixture>| {
            let state = cx.new(|_| AppState::new("/tmp/fleet", Instant::now()));
            let body = cx.focus_handle();
            let overlay = cx.focus_handle();
            let agent = cx.focus_handle();
            let subscription = cx.observe_in(&state, window, |view, state, window, cx| {
                focus_surface(
                    focus_target(state.read(cx)),
                    &view.body,
                    &view.overlay,
                    &view.agent,
                    window,
                    cx,
                );
            });
            focus_surface(FocusTarget::Body, &body, &overlay, &agent, window, cx);
            FocusFixture {
                state,
                body,
                overlay,
                agent,
                _subscription: subscription,
            }
        });
        window
            .update(cx, |view, _, cx| {
                view.state.update(cx, |state, cx| {
                    state.daemon = DaemonLink::Connected;
                    state.open_overlay(crate::state::Overlay::Jobs);
                    cx.notify();
                });
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, window, cx| {
                assert!(view.overlay.is_focused(window));
                view.state.update(cx, |state, cx| {
                    state.close_overlay();
                    state.toggle_agent_popup(fleet_core::config::Agent::Claude, None);
                    cx.notify();
                });
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, window, cx| {
                assert!(view.agent.is_focused(window));
                view.state.update(cx, |state, cx| {
                    state.hide_agent_popup();
                    cx.notify();
                });
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, window, _| assert!(view.body.is_focused(window)))
            .unwrap();
    }
}
