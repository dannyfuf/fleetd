use super::Shell;
use crate::{
    bridge::{Bridge, BridgeEvent},
    presentation::EventDamage,
    state::{AppState, DaemonLink},
};
use fleet_core::ids::TerminalId;
use fleet_proto::request::RequestBody;
use gpui::{App, Context, Entity, Task};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

const TICK: Duration = Duration::from_millis(250);
const EVENT_BATCH_LIMIT: usize = 128;

/// Whether an event touches one terminal mirror and nothing an observer of `AppState` reads.
fn is_terminal_only(damage: EventDamage) -> bool {
    damage.terminal.is_some()
        && damage.watch.is_none()
        && !(damage.domain
            || damage.jobs
            || damage.sessions
            || damage.chrome
            || damage.notifications
            || damage.connection
            || damage.watch_structure)
}

#[derive(Default)]
struct BatchDamage {
    state: bool,
    terminals: HashSet<TerminalId>,
    recover: HashSet<TerminalId>,
}

impl BatchDamage {
    fn apply(&mut self, state: &mut AppState, event: BridgeEvent, now: Instant) {
        let damage = match &event {
            BridgeEvent::Daemon(event) => Some(crate::presentation::event_damage(event)),
            _ => None,
        };
        let terminal = damage.and_then(|damage| damage.terminal);
        let was_synced =
            terminal.is_some_and(|id| state.grids.get(&id).is_some_and(|grid| !grid.desynced));
        let lagged = matches!(event, BridgeEvent::EventsLagged { .. });
        let terminal_only = damage.is_some_and(is_terminal_only);
        state.apply_bridge_event(event, now);
        self.state |= !terminal_only;
        if let Some(terminal) = terminal {
            self.terminals.insert(terminal);
            if was_synced && state.grids.get(&terminal).is_some_and(|grid| grid.desynced) {
                self.recover.insert(terminal);
            }
        }
        // Lag is the only event whose recovery concerns every grid. Ordinary frames inspect
        // only their own mirror, preserving each ordered delta before issuing recovery.
        if lagged {
            self.recover.extend(state.grids.keys().copied());
        }
    }

    fn affects_visible_terminal(&self, state: &AppState) -> bool {
        if state.doctor.is_none()
            && matches!(
                state.daemon,
                DaemonLink::Starting | DaemonLink::Failed { .. }
            )
        {
            return false;
        }
        let workspace = (state.doctor.is_none() && !state.is_first_run())
            .then(|| {
                state
                    .active_session()
                    .and_then(|session| session.active_terminal)
            })
            .flatten();
        let popup = state
            .agent_popup_session()
            .and_then(|session| session.terminals.first())
            .map(|terminal| terminal.id);
        workspace
            .into_iter()
            .chain(popup)
            .any(|terminal| self.terminals.contains(&terminal))
    }
}

fn apply_batch(
    state: &Entity<AppState>,
    events: impl IntoIterator<Item = BridgeEvent>,
    cx: &mut App,
) -> BatchDamage {
    let mut damage = BatchDamage::default();
    // One instant for the whole burst: every event in it was produced before this dispatch.
    let now = Instant::now();
    state.update(cx, |state, cx| {
        for event in events {
            damage.apply(state, event, now);
        }
        if damage.state {
            cx.notify();
        }
    });
    damage
}

impl Shell {
    pub(super) fn spawn_event_loop(bridge: &Bridge, cx: &mut Context<Self>) -> Task<()> {
        let events = bridge.events();
        cx.spawn(async move |shell, cx| {
            while let Ok(first) = events.recv().await {
                let mut batch = Vec::with_capacity(EVENT_BATCH_LIMIT);
                batch.push(first);
                while batch.len() < EVENT_BATCH_LIMIT {
                    let Ok(event) = events.try_recv() else { break };
                    batch.push(event);
                }
                let count = batch.len();
                let updated = shell.update(cx, |shell, cx| {
                    let damage = apply_batch(&shell.state, batch, cx);
                    for terminal in &damage.recover {
                        shell.bridge.send(RequestBody::RequestFullFrame {
                            terminal: *terminal,
                        });
                    }
                    let terminal_update =
                        !damage.state && damage.affects_visible_terminal(shell.state.read(cx));
                    if damage.state {
                        shell.reconcile_agent_session(cx);
                    }
                    tracing::trace!(
                        events = count,
                        recovery = damage.recover.len(),
                        state_notify = damage.state,
                        "applied bridge batch"
                    );
                    terminal_update.then_some(shell.window).flatten()
                });
                let Ok(window) = updated else {
                    return;
                };
                if let Some(window) = window {
                    // Release the shell lease before entering its window. Plain terminal output
                    // updates prepared surfaces without waking AppState/chrome observers.
                    let _ = window.update(cx, |_, window, cx| {
                        let _ = shell.update(cx, |shell, cx| {
                            shell.synchronize_surfaces(window, cx);
                            cx.notify();
                        });
                    });
                }
                // A busy producer cannot keep the UI executor inside an always-ready receive loop.
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
    }

    pub(super) fn spawn_ticker(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |shell, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let ticked = shell.update(cx, |shell, cx| {
                    shell.state.update(cx, |state, cx| {
                        if state.tick(Instant::now()) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_proto::{
        event::Event,
        terminal::{CursorShape, CursorState, FrameUpdate, TerminalModes, ViewportInfo},
    };
    use gpui::AppContext;
    use std::rc::Rc;

    fn frame(terminal: u64, seq: u64, full: bool) -> BridgeEvent {
        BridgeEvent::Daemon(Box::new(Event::TerminalFrame(FrameUpdate {
            terminal: TerminalId(terminal),
            seq,
            cols: 4,
            rows: 2,
            full,
            shift: None,
            rows_changed: Vec::new(),
            cursor: CursorState {
                row: 0,
                col: (seq % 4) as u16,
                visible: true,
                shape: CursorShape::Block,
            },
            viewport: ViewportInfo {
                scrollback_len: 0,
                offset: 0,
                history_epoch: 0,
            },
            modes: TerminalModes::default(),
            title: None,
        })))
    }

    #[test]
    fn ordered_frames_recover_only_the_grid_that_lost_a_delta() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        let mut batch = BatchDamage::default();
        batch.apply(&mut state, frame(1, 1, true), now);
        batch.apply(&mut state, frame(2, 1, true), now);
        batch.apply(&mut state, frame(1, 3, false), now);
        batch.apply(&mut state, frame(2, 2, false), now);
        assert_eq!(batch.recover, HashSet::from([TerminalId(1)]));
        assert!(state.grids[&TerminalId(1)].desynced);
        assert!(!state.grids[&TerminalId(2)].desynced);
        assert_eq!(state.grids[&TerminalId(2)].cursor.col, 2);
        batch.apply(&mut state, frame(1, 4, true), now);
        batch.apply(&mut state, frame(1, 5, false), now);
        assert!(!state.grids[&TerminalId(1)].desynced);
        assert_eq!(state.grids[&TerminalId(1)].cursor.col, 1);
        assert!(
            !batch.state,
            "terminal deltas leave chrome and list observers clean"
        );
        assert!(!batch.affects_visible_terminal(&state));
    }

    #[test]
    fn broadcast_lag_requests_each_mirror_once_even_when_already_desynced() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        let mut batch = BatchDamage::default();
        batch.apply(&mut state, frame(1, 1, true), now);
        batch.apply(&mut state, frame(2, 1, true), now);
        batch.apply(&mut state, frame(1, 3, false), now);
        batch.apply(&mut state, BridgeEvent::EventsLagged { dropped: 2 }, now);
        assert_eq!(batch.recover, HashSet::from([TerminalId(1), TerminalId(2)]));
        assert!(batch.state);
        assert!(state.grids.values().all(|grid| grid.desynced));
    }

    #[test]
    fn terminal_title_invalidates_the_state_projection() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        let mut batch = BatchDamage::default();
        batch.apply(&mut state, frame(1, 1, true), now);
        batch.apply(
            &mut state,
            BridgeEvent::Daemon(Box::new(Event::TerminalTitle {
                terminal: TerminalId(1),
                title: "vim".into(),
            })),
            now,
        );
        assert!(batch.state);
        assert_eq!(state.grids[&TerminalId(1)].title.as_deref(), Some("vim"));
        assert!(batch.recover.is_empty());
    }

    #[gpui::test]
    fn one_state_notification_per_mixed_burst_and_none_for_plain_frames(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| AppState::new("/tmp/fleet", Instant::now()));
        let notifications = Rc::new(std::cell::Cell::new(0));
        let count = Rc::clone(&notifications);
        let _subscription =
            cx.update(|cx| cx.observe(&state, move |_, _| count.set(count.get() + 1)));
        cx.update(|cx| {
            apply_batch(&state, [frame(1, 1, true), frame(1, 2, false)], cx);
        });
        cx.run_until_parked();
        assert_eq!(notifications.get(), 0);
        cx.update(|cx| {
            apply_batch(
                &state,
                [
                    frame(1, 3, false),
                    BridgeEvent::Daemon(Box::new(Event::TerminalTitle {
                        terminal: TerminalId(1),
                        title: "vim".into(),
                    })),
                    BridgeEvent::EventsLagged { dropped: 1 },
                ],
                cx,
            );
        });
        cx.run_until_parked();
        assert_eq!(notifications.get(), 1);
    }
}
