use fleet_core::agents::{
    Delegation, DeliveryState, MessageOrigin, SessionState, StopCause, ThreadId, TurnState,
};
use fleet_proto::{request::RequestBody, response::ResponseBody};

use crate::services::agents::store::OutboxAction;

use super::{super::worker::drain, worker::Harness};

fn one(response: ResponseBody) -> Delegation {
    match response {
        ResponseBody::Delegation(delegation) => delegation,
        other => panic!("expected one delegation, got {other:?}"),
    }
}

fn open(thread: ThreadId) -> RequestBody {
    RequestBody::AgentThreadOpen {
        thread,
        from_seq: None,
        after_seq: None,
        turn_limit: None,
        before_cursor: None,
        request_sync_marker: false,
    }
}

#[tokio::test(start_paused = true)]
async fn idle_delivery_starts_a_caller_turn_with_delegation_origin() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(1, false, true).await;

    drain(&harness.service).await.expect("drain delivery");
    harness.wait_for_delegation_origins(caller, 1).await;

    assert!(
        harness
            .origins(caller)
            .await
            .contains(&MessageOrigin::Delegation {
                id: delegations[0].id
            })
    );
}

#[tokio::test(start_paused = true)]
async fn a_second_wake_cannot_resubmit_delivery_before_origin_commit() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(1, false, true).await;

    drain(&harness.service).await.expect("first delivery drain");
    drain(&harness.service)
        .await
        .expect("second drain inside origin commit window");
    tokio::time::resume();
    harness.wait_for_delegation_origins(caller, 1).await;

    assert_eq!(
        harness
            .origins(caller)
            .await
            .into_iter()
            .filter(|origin| matches!(origin, fleet_core::agents::MessageOrigin::Delegation { id } if *id == delegations[0].id))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn running_then_idle_waits_for_the_callers_settle() {
    let harness = Harness::start().await;
    let (caller, _) = harness.caller_with_delegations(1, false, false).await;

    drain(&harness.service).await.expect("drain while running");
    assert!(
        harness
            .origins(caller)
            .await
            .iter()
            .all(MessageOrigin::is_user)
    );

    tokio::time::resume();
    harness.send(caller, "finish setup turn").await;
    harness
        .wait_for(caller, |projection| {
            matches!(projection.turn, TurnState::Settled(_, _))
        })
        .await;
    tokio::time::pause();
    drain(&harness.service).await.expect("drain after settle");
    harness.wait_for_delegation_origins(caller, 1).await;
}

#[tokio::test(start_paused = true)]
async fn eager_delivery_steers_the_running_turn() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(1, true, false).await;

    drain(&harness.service).await.expect("drain eager delivery");
    harness.wait_for_delegation_origins(caller, 1).await;

    assert!(
        harness
            .origins(caller)
            .await
            .contains(&MessageOrigin::Delegation {
                id: delegations[0].id
            })
    );
}

#[tokio::test(start_paused = true)]
async fn provider_exit_delivery_resumes_and_sends() {
    let harness = Harness::start().await;
    let (caller, _) = harness.caller_with_delegations(1, false, true).await;
    tokio::time::resume();
    harness.send(caller, "exit-now").await;
    harness
        .wait_for(caller, |projection| {
            projection.session == SessionState::Error
        })
        .await;
    harness
        .wait_for_stop_cause(caller, StopCause::ProviderExit)
        .await;

    drain(&harness.service)
        .await
        .expect("drain resume delivery");
    harness.wait_for_delegation_origins(caller, 1).await;
    tokio::time::pause();
}

#[tokio::test(start_paused = true)]
async fn user_stop_holds_delivery_until_session_configured() {
    let harness = Harness::start().await;
    let (caller, _) = harness.caller_with_delegations(1, false, true).await;
    tokio::time::resume();
    harness.manager.stop(caller).await.expect("stop caller");
    harness
        .wait_for(caller, |projection| {
            projection.session == SessionState::Stopped
        })
        .await;
    harness.wait_for_stop_cause(caller, StopCause::User).await;
    tokio::time::pause();

    drain(&harness.service).await.expect("drain stopped caller");
    assert!(
        harness
            .origins(caller)
            .await
            .iter()
            .all(MessageOrigin::is_user)
    );
    assert_eq!(
        harness
            .store
            .delegation_outbox()
            .await
            .expect("open rows")
            .len(),
        1
    );

    tokio::time::resume();
    harness
        .manager
        .open(&open(caller))
        .await
        .expect("opening the caller resumes its session");
    harness
        .wait_for(caller, |projection| {
            projection.session == SessionState::Ready
        })
        .await;
    tokio::time::pause();

    drain(&harness.service)
        .await
        .expect("drain after session configured");
    harness.wait_for_delegation_origins(caller, 1).await;
}

/// Both shipped adapters mint a cursor at start, so this forces the legacy cursorless record.
#[tokio::test(start_paused = true)]
async fn stopped_caller_without_a_cursor_becomes_undeliverable() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(1, false, true).await;
    tokio::time::resume();
    harness.send(caller, "exit-now").await;
    harness
        .wait_for(caller, |projection| {
            projection.session == SessionState::Error
        })
        .await;
    harness
        .wait_for_stop_cause(caller, StopCause::ProviderExit)
        .await;
    harness
        .manager
        .clear_resume_cursor(caller)
        .await
        .expect("clear the caller's durable resume cursor");
    tokio::time::pause();

    drain(&harness.service)
        .await
        .expect("drain no-cursor delivery");

    let stored = harness
        .store
        .delegation(delegations[0].id)
        .await
        .expect("read delegation")
        .expect("delegation exists");
    assert!(matches!(
        stored.delivery,
        DeliveryState::Undeliverable { .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn two_children_finishing_together_deliver_as_two_turns() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(2, false, true).await;

    drain(&harness.service).await.expect("drain first child");
    harness.wait_for_delegation_origins(caller, 1).await;
    harness
        .wait_for(caller, |projection| {
            matches!(projection.turn, TurnState::Settled(_, _))
        })
        .await;
    drain(&harness.service).await.expect("drain second child");
    harness.wait_for_delegation_origins(caller, 2).await;

    let origins = harness.origins(caller).await;
    for delegation in delegations {
        assert!(origins.contains(&MessageOrigin::Delegation { id: delegation.id }));
    }
}

/// The reported pain point: an orchestrator that waited on eight children was sent all eight
/// results again as user messages once its turn settled. A caller's own `wait` hands the result
/// over, so nothing is left to inject.
#[tokio::test(start_paused = true)]
async fn a_callers_own_wait_consumes_the_result_instead_of_delivering_it() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(1, false, true).await;

    let answered = one(harness
        .service
        .wait(delegations[0].id, 0, Some(caller))
        .await
        .expect("wait on a terminal child"));
    assert_eq!(answered.delivery, DeliveryState::Consumed);

    drain(&harness.service).await.expect("drain after consume");

    assert!(
        harness
            .origins(caller)
            .await
            .iter()
            .all(MessageOrigin::is_user),
        "a consumed result is never injected as a delegation-origin message",
    );
    assert!(
        harness
            .store
            .delegation_outbox()
            .await
            .expect("open rows")
            .is_empty(),
        "consuming the result closes its delivery row",
    );
}

/// The other half of the rule: only the delegation's own caller consumes. Anyone else — including
/// a `wait` from a shell that knows no session — leaves the delivery exactly as it was, so the
/// caller still receives its child's result.
#[tokio::test(start_paused = true)]
async fn a_wait_from_anyone_but_the_caller_still_delivers_the_result() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(1, false, true).await;

    for waiter in [None, Some(ThreadId::new())] {
        let answered = one(harness
            .service
            .wait(delegations[0].id, 0, waiter)
            .await
            .expect("wait on a terminal child"));
        assert_eq!(answered.delivery, DeliveryState::Pending, "{waiter:?}");
    }

    drain(&harness.service).await.expect("drain delivery");
    harness.wait_for_delegation_origins(caller, 1).await;

    assert!(
        harness
            .origins(caller)
            .await
            .contains(&MessageOrigin::Delegation {
                id: delegations[0].id
            })
    );
}

/// `consume` closes the delivery row in its own transaction, so the worker only meets a consumed
/// delegation when a drain read the outbox before that commit landed. It must patch the caller's
/// item, close the row, and send nothing.
#[tokio::test(start_paused = true)]
async fn the_worker_closes_a_consumed_delivery_without_injecting_it() {
    let harness = Harness::start().await;
    let (caller, delegations) = harness.caller_with_delegations(1, false, true).await;

    one(harness
        .service
        .wait(delegations[0].id, 0, Some(caller))
        .await
        .expect("wait on a terminal child"));
    // Re-open the delivery row the way a pass that raced the consume transaction would see it.
    harness
        .enqueue(delegations[0].id, OutboxAction::Deliver)
        .await;

    drain(&harness.service)
        .await
        .expect("drain a consumed delivery");

    assert!(
        harness
            .origins(caller)
            .await
            .iter()
            .all(MessageOrigin::is_user),
    );
    assert!(
        harness
            .store
            .delegation_outbox()
            .await
            .expect("open rows")
            .is_empty(),
    );
}
