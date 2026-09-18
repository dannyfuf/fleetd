use fleet_core::agents::{DeliveryState, MessageOrigin, SessionState, StopCause, TurnState};
use fleet_proto::request::RequestBody;

use super::{super::worker::drain, worker::Harness};

fn open(thread: fleet_core::agents::ThreadId) -> RequestBody {
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

#[tokio::test(start_paused = true)]
async fn stopped_caller_without_a_cursor_becomes_undeliverable() {
    let harness = Harness::start().await;
    harness.omit_resume_cursor();
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
