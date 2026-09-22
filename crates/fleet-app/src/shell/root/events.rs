use super::Shell;
use crate::{
    bridge::{Bridge, BridgeEvent},
    presentation::EventDamage,
    state::{AppState, DaemonLink},
};
use fleet_client::MirrorOutcome;
use fleet_core::agents::{Applied, SeqEvent, ThreadId};
use fleet_core::ids::TerminalId;
use fleet_proto::request::RequestBody;
use gpui::{App, Context, Entity, Task};
use std::{
    collections::{HashMap, HashSet},
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
    nudged: bool,
    agent_text: HashMap<ThreadId, Vec<Applied>>,
    terminals: HashSet<TerminalId>,
    recover: HashSet<TerminalId>,
}

impl BatchDamage {
    fn apply(&mut self, state: &mut AppState, event: BridgeEvent, now: Instant) {
        let event = match event {
            BridgeEvent::Agent { thread, event } => {
                self.apply_agent(state, thread, &event, now);
                return;
            }
            BridgeEvent::Daemon(event) => match *event {
                fleet_proto::event::Event::Agent { thread, event } => {
                    self.apply_agent(state, thread, &event, now);
                    return;
                }
                event => BridgeEvent::Daemon(Box::new(event)),
            },
            event => event,
        };
        if matches!(event, BridgeEvent::Nudge) {
            self.nudged = true;
            return;
        }
        let damage = match &event {
            BridgeEvent::Daemon(event) => Some(crate::presentation::event_damage(event)),
            _ => None,
        };
        let terminal = damage.and_then(|damage| damage.terminal);
        let lagged = matches!(event, BridgeEvent::EventsLagged { .. });
        let terminal_only = damage.is_some_and(is_terminal_only);
        let connected_snapshot_revision = match &event {
            BridgeEvent::Connected(snapshot) => Some(snapshot.revision),
            _ => None,
        };
        let applied_snapshot_revision = match &event {
            BridgeEvent::Daemon(event) => match event.as_ref() {
                fleet_proto::event::Event::SnapshotChanged(snapshot) => Some(snapshot.revision),
                _ => None,
            },
            _ => None,
        };
        state.apply_bridge_event(event, now);
        if let Some(revision) = connected_snapshot_revision {
            state.harness.settle().connected();
            state.harness.settle().applied(revision);
        } else if let Some(revision) = applied_snapshot_revision {
            state.harness.settle().applied(revision);
        }
        self.state |= !terminal_only;
        if let Some(terminal) = terminal {
            self.terminals.insert(terminal);
            // A mirror that cannot accept a delta needs a full frame, whether it lost one or
            // was born from one: a grid created by a diff is unprimed, never desynced, so an
            // edge from synced to desynced would never fire for it. `recover` is a per-batch
            // set, so this asks once per batch and asks again if the request is lost.
            if state
                .grids
                .get(&terminal)
                .is_some_and(|grid| !grid.primed || grid.desynced)
            {
                self.recover.insert(terminal);
            }
        }
        // Lag is the only event whose recovery concerns every grid. Ordinary frames inspect
        // only their own mirror, preserving each ordered delta before issuing recovery.
        if lagged {
            self.recover.extend(state.grids.keys().copied());
        }
    }

    fn apply_agent(
        &mut self,
        state: &mut AppState,
        thread: ThreadId,
        event: &SeqEvent,
        now: Instant,
    ) {
        match state.apply_agent_event(thread, event) {
            MirrorOutcome::Applied(applied @ Applied::Text { .. }) => {
                self.agent_text.entry(thread).or_default().push(applied);
            }
            MirrorOutcome::Applied(Applied::Structural)
            | MirrorOutcome::Duplicate { .. }
            | MirrorOutcome::Gap { .. }
            | MirrorOutcome::Rejected { .. } => {
                self.state = true;
                state.notify_agent_attention(now);
            }
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
        } else if fleet_ui_kit::harness::is_recording()
            && (damage.nudged || damage.affects_visible_terminal(state))
        {
            // Plain PTY output deliberately does not wake AppState's chrome observers — that is
            // the optimisation the branch above exists for. The harness is the one observer that
            // needs it anyway: `await terminal.text ~= "…"` waits on exactly this signal, and
            // without it a scenario waits for the next unrelated notification instead of for the
            // output it asked about. `is_recording` is a thread-local `bool` that is false in
            // every launch that did not ask for harness mode, so production pays one branch.
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
                    if !damage.agent_text.is_empty() {
                        shell
                            .workspace
                            .sync_agent_text(&shell.state, &damage.agent_text, cx);
                    }
                    if damage.state {
                        shell.reconcile_agent_session(cx);
                    }
                    // A board the *daemon* changed — a card run recorded, an `on_success` move,
                    // a released blocker — reaches the mirror as a `BoardChanged` that only
                    // marks it stale. The Hub's tab reloads itself from its own observation of
                    // `AppState`; the Workspace's pane has nothing that would, so the batch
                    // that carried the event is what claims the load.
                    crate::screens::board::refresh_after_daemon_change(
                        &shell.state,
                        &shell.bridge,
                        cx,
                    );
                    tracing::trace!(
                        events = count,
                        recovery = damage.recover.len(),
                        agent_text = damage.agent_text.values().map(Vec::len).sum::<usize>(),
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
                    window
                        .update(cx, |_, window, cx| {
                            shell
                                .update(cx, |shell, cx| {
                                    shell.synchronize_surfaces(window, cx);
                                    cx.notify();
                                })
                                .ok();
                        })
                        .ok();
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
    use crate::screens::agent_thread::AgentThreadView;
    use fleet_core::{
        agents::{
            AgentEvent, AgentKind, ItemId, ItemKind, ItemStatus, Seq, ThreadProjection, TurnId,
        },
        ids::WorktreeId,
    };
    use fleet_proto::{
        event::Event,
        snapshot::{DaemonInfo, Snapshot},
        terminal::{CursorShape, CursorState, FrameUpdate, TerminalModes, ViewportInfo},
    };
    use gpui::AppContext;
    use std::{rc::Rc, sync::Arc};

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

    fn empty_snapshot() -> Snapshot {
        Snapshot {
            boards: Vec::new(),
            generated_at: String::new(),
            revision: None,
            contexts: Vec::new(),
            repos: Vec::new(),
            clones: Vec::new(),
            worktrees: Vec::new(),
            active_context: None,
            sessions: Vec::new(),
            agent_threads: Vec::new(),
            statuses: Vec::new(),
            pools: Vec::new(),
            hosts: Vec::new(),
            jobs: Vec::new(),
            daemon: DaemonInfo {
                version: "test".to_owned(),
                pid: 1,
                started_at: String::new(),
                home: String::new(),
            },
        }
    }

    fn streaming_projection() -> (ThreadProjection, ItemId) {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let item = ItemId::new();
        let worktree =
            WorktreeId::try_from("fleet/app#streaming").unwrap_or_else(|error| panic!("{error}"));
        let mut projection = ThreadProjection::new(thread, worktree, AgentKind::Claude);
        for event in [
            SeqEvent {
                seq: Seq(1),
                at: chrono::DateTime::UNIX_EPOCH,
                raw: None,
                event: AgentEvent::TurnStarted {
                    turn,
                    user_item: ItemId::new(),
                },
            },
            SeqEvent {
                seq: Seq(2),
                at: chrono::DateTime::UNIX_EPOCH,
                raw: None,
                event: AgentEvent::ItemStarted {
                    turn,
                    item,
                    kind: ItemKind::AssistantText {
                        text: "a".to_owned(),
                    },
                    parent: None,
                },
            },
        ] {
            projection
                .apply(&event)
                .unwrap_or_else(|error| panic!("{error}"));
        }
        (projection, item)
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
    fn a_delta_for_a_terminal_with_no_mirror_asks_for_a_full_frame() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        let mut batch = BatchDamage::default();
        batch.apply(&mut state, frame(1, 7, false), now);
        assert!(!state.grids[&TerminalId(1)].primed);
        assert_eq!(batch.recover, HashSet::from([TerminalId(1)]));
        batch.apply(&mut state, frame(1, 8, false), now);
        assert_eq!(batch.recover.len(), 1, "one request, not one per delta");
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

    #[gpui::test]
    fn one_hundred_text_deltas_skip_app_state_and_patch_one_row_each(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(fleet_ui_kit::Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (projection, item) = streaming_projection();
        let thread = projection.thread;
        let view = cx.new(|cx| AgentThreadView::new(projection.clone(), cx));
        let state = cx.new(|_| {
            let mut state = AppState::new("/tmp/fleet-agent-text", Instant::now());
            state.agents.install_snapshot(projection, &[]);
            state
        });
        let notifications = Rc::new(std::cell::Cell::new(0));
        let count = Rc::clone(&notifications);
        let _subscription =
            cx.update(|cx| cx.observe(&state, move |_, _| count.set(count.get() + 1)));
        let events = (0..100).map(|offset| BridgeEvent::Agent {
            thread,
            event: SeqEvent {
                seq: Seq(3 + offset),
                at: chrono::DateTime::UNIX_EPOCH,
                raw: None,
                event: AgentEvent::ContentDelta {
                    item,
                    stream: fleet_core::agents::StreamKind::AssistantText,
                    delta: "x".to_owned(),
                },
            },
        });
        let damage = cx.update(|cx| apply_batch(&state, events, cx));
        cx.run_until_parked();
        assert!(!damage.state);
        assert_eq!(notifications.get(), 0);
        assert_eq!(damage.agent_text[&thread].len(), 100);

        state.update(cx, |app, cx| {
            let projection = app
                .agents
                .projection(thread)
                .unwrap_or_else(|| panic!("streaming projection must remain installed"));
            view.update(cx, |view, cx| {
                view.sync_batch(projection, &damage.agent_text[&thread], cx)
            });
        });
        cx.run_until_parked();
        assert_eq!(notifications.get(), 0);
        assert_eq!(view.read_with(cx, |view, _| view.patched_rows()), 100);
    }

    #[gpui::test]
    fn a_structural_agent_event_still_notifies_app_state(cx: &mut gpui::TestAppContext) {
        let (projection, item) = streaming_projection();
        let thread = projection.thread;
        let state = cx.new(|_| {
            let mut state = AppState::new("/tmp/fleet-agent-structural", Instant::now());
            state.agents.install_snapshot(projection, &[]);
            state
        });
        let notifications = Rc::new(std::cell::Cell::new(0));
        let count = Rc::clone(&notifications);
        let _subscription =
            cx.update(|cx| cx.observe(&state, move |_, _| count.set(count.get() + 1)));

        let damage = cx.update(|cx| {
            apply_batch(
                &state,
                [BridgeEvent::Agent {
                    thread,
                    event: SeqEvent {
                        seq: Seq(3),
                        at: chrono::DateTime::UNIX_EPOCH,
                        raw: None,
                        event: AgentEvent::ItemCompleted {
                            item,
                            status: ItemStatus::Completed,
                        },
                    },
                }],
                cx,
            )
        });
        cx.run_until_parked();
        assert!(damage.state);
        assert!(damage.agent_text.is_empty());
        assert_eq!(notifications.get(), 1);
    }

    #[gpui::test]
    fn a_nudge_notifies_only_while_the_harness_is_recording(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new("/tmp/fleet", Instant::now()));
        let notifications = Rc::new(std::cell::Cell::new(0));
        let count = Rc::clone(&notifications);
        let _subscription =
            cx.update(|cx| cx.observe(&state, move |_, _| count.set(count.get() + 1)));

        fleet_ui_kit::harness::set_recording(false);
        let production_damage = cx.update(|cx| apply_batch(&state, [BridgeEvent::Nudge], cx));
        cx.run_until_parked();
        assert!(production_damage.nudged);
        assert!(!production_damage.state);
        assert!(production_damage.terminals.is_empty());
        assert!(production_damage.recover.is_empty());
        assert_eq!(notifications.get(), 0);

        fleet_ui_kit::harness::set_recording(true);
        let recorded_damage = cx.update(|cx| apply_batch(&state, [BridgeEvent::Nudge], cx));
        fleet_ui_kit::harness::set_recording(false);
        cx.run_until_parked();
        assert!(recorded_damage.nudged);
        assert!(!recorded_damage.state);
        assert_eq!(notifications.get(), 1);
    }

    #[gpui::test]
    fn snapshot_changed_settles_only_mutations_covered_by_its_revision(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| AppState::new("/tmp/fleet", Instant::now()));
        let settle = cx.update(|cx| Arc::clone(state.read(cx).harness.settle()));
        settle.begin(Some(3));
        let pending = settle.begin(Some(5));
        assert_eq!(settle.pending(), 2);

        let mut snapshot = empty_snapshot();
        snapshot.revision = Some(4);

        let damage = cx.update(|cx| {
            apply_batch(
                &state,
                [BridgeEvent::Daemon(Box::new(Event::SnapshotChanged(
                    snapshot,
                )))],
                cx,
            )
        });

        assert!(damage.state);
        assert_eq!(settle.pending(), 1);
        assert!(!settle.expire(pending));
        assert_eq!(settle.pending(), 1);

        let mut snapshot = empty_snapshot();
        snapshot.revision = Some(5);
        cx.update(|cx| {
            apply_batch(
                &state,
                [BridgeEvent::Daemon(Box::new(Event::SnapshotChanged(
                    snapshot,
                )))],
                cx,
            )
        });
        assert_eq!(settle.pending(), 0);
    }

    #[test]
    fn connected_settles_pending_mutations() {
        let now = Instant::now();
        let mut state = AppState::new("/tmp/fleet", now);
        state.harness.settle().begin(Some(99));
        let pending = state.harness.settle().begin(Some(100));
        assert!(!state.harness.settle().expire(pending));
        let mut damage = BatchDamage::default();
        let mut snapshot = empty_snapshot();
        snapshot.revision = Some(2);

        damage.apply(&mut state, BridgeEvent::Connected(Box::new(snapshot)), now);

        assert!(damage.state);
        assert_eq!(state.harness.settle().pending(), 0);
        state.harness.settle().begin(Some(2));
        assert_eq!(state.harness.settle().pending(), 0);
    }
}
