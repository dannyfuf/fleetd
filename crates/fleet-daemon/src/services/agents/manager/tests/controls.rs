//! Controls and checkpoints: what a change to a running session costs, and what is captured
//! before a turn runs.
//!
//! Both subjects are here because both are answered by the *adapter* and acted on by the
//! manager, which is the seam `NATIVE-AGENTS.md` §3.1 draws: `apply_runtime` reports what a
//! change would cost and only the manager knows whether a turn is running, and a checkpoint is
//! taken at the one moment the manager owns — the turn it is about to submit.

use std::sync::atomic::Ordering;

use fleet_core::agents::{
    AgentEvent, ItemId, ModelSelection, PermissionMode, SeqEvent, SessionState, TurnId, TurnState,
    UserInput,
};
use fleet_proto::event::Event;

use super::{FakeCall, Harness, drain, full, restarting_controls};

/// The control tier §7 describes: a change whose cost is a restart happens at a turn boundary,
/// and is **refused** while a turn runs rather than killing it to make a picker truthful.
#[tokio::test]
async fn a_restarting_control_change_waits_for_a_turn_boundary() {
    let harness = Harness::start(restarting_controls()).await;
    harness
        .script
        .restarts_on_control
        .store(true, Ordering::SeqCst);
    let thread = harness.create(None).await.thread;

    // No turn running: the adapter reports a restart and the manager performs it, bracketed by
    // the optimistic `Starting` §7.1 asks for and the `Ready` that says it landed.
    let mut events = harness.events.subscribe();
    harness
        .manager
        .set_mode(thread, PermissionMode::AcceptEdits)
        .await
        .expect("a mode change at a turn boundary restarts");
    let states = drain(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            Event::Agent {
                event:
                    SeqEvent {
                        event: AgentEvent::SessionStateChanged(state),
                        ..
                    },
                ..
            } => Some(state),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        vec![SessionState::Starting, SessionState::Ready],
        "the tab reads starting… before the round trip, and ready when it lands"
    );
    assert!(
        harness
            .script
            .calls()
            .contains(&FakeCall::Restart(/* resume */ true)),
        "the restart reuses the resume cursor: {:?}",
        harness.script.calls()
    );
    assert_eq!(
        harness.projection(thread).await.mode,
        PermissionMode::AcceptEdits
    );

    // A turn running: the same change is refused, and nothing is restarted under it.
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;
    let restarts = harness
        .script
        .calls()
        .iter()
        .filter(|call| matches!(call, FakeCall::Restart(_)))
        .count();
    let error = harness
        .manager
        .set_model(
            thread,
            ModelSelection {
                model: "claude-opus-5".to_owned(),
                effort: None,
                provider: None,
            },
        )
        .await
        .expect_err("a restarting change mid-turn is refused, not performed");
    assert_eq!(error.kind, fleet_proto::error::ErrorKind::Conflict);
    assert!(
        error.message.contains("turn boundary"),
        "the refusal says why: {}",
        error.message
    );
    assert_eq!(
        harness
            .script
            .calls()
            .iter()
            .filter(|call| matches!(call, FakeCall::Restart(_)))
            .count(),
        restarts,
        "the running turn's process was not replaced under it"
    );
    // And the refusal did not record the change as if it had landed.
    assert_eq!(harness.projection(thread).await.model, None);
}

/// §5's checkpoint rule, from the manager's side: a turn that is *starting* is checkpointed, a
/// steer is not, and a capture that fails never refuses the turn.
///
/// A second checkpoint on a steer would make `[u] revert turn` restore the middle of the turn
/// rather than its start, which is the opposite of what the footer's verb says.
#[tokio::test]
async fn a_starting_turn_is_checkpointed_and_a_steer_is_not() {
    let harness = Harness::start(full()).await;
    let shell = std::sync::Arc::new(super::RecordingShell::default());
    harness
        .manager
        .set_checkpoints(crate::services::checkpoints::Checkpoints::new(
            shell.clone(),
        ));
    let thread = harness.create(None).await.thread;

    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "ship it".to_owned(),
                attachments: Vec::new(),
                item: None,
            },
        )
        .await
        .expect("the turn is accepted even though its checkpoint failed");
    let probes = shell
        .git_calls()
        .iter()
        .filter(|args| args.first().map(String::as_str) == Some("rev-parse"))
        .count();
    assert_eq!(probes, 1, "a starting turn asks for one checkpoint");

    // The harness announces the turn the manager minted, exactly as an adapter does, so the
    // second send is a real steer rather than a second fresh turn.
    let turn = harness
        .script
        .calls()
        .into_iter()
        .find_map(|call| match call {
            FakeCall::Send(turn, _) => Some(turn),
            _ => None,
        })
        .expect("the send reached the provider");
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;
    harness
        .manager
        .send(
            thread,
            UserInput {
                text: "also update the docs".to_owned(),
                attachments: Vec::new(),
                item: None,
            },
        )
        .await
        .expect("steer");
    assert_eq!(
        shell
            .git_calls()
            .iter()
            .filter(|args| args.first().map(String::as_str) == Some("rev-parse"))
            .count(),
        probes,
        "a steer takes no second checkpoint: {:?}",
        shell.git_calls()
    );
}

#[tokio::test]
async fn a_mode_change_is_a_durable_event_every_mirror_sees() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let mut events = harness.events.subscribe();
    harness
        .manager
        .set_mode(thread, PermissionMode::AcceptEdits)
        .await
        .expect("set mode");

    let broadcast = drain(&mut events);
    assert!(
        broadcast.iter().any(|event| matches!(
            event,
            Event::Agent {
                event: SeqEvent {
                    event: AgentEvent::MetadataChanged {
                        mode: Some(PermissionMode::AcceptEdits),
                        ..
                    },
                    ..
                },
                ..
            }
        )),
        "a mode change reaches clients as an event: {broadcast:?}"
    );
    assert_eq!(
        harness.projection(thread).await.mode,
        PermissionMode::AcceptEdits
    );
}
