use fleet_core::agents::{DelegationStatus, DeliveryState, ResultSource};
use fleet_proto::response::ResponseBody;

use crate::services::agents::store::OutboxAction;

use super::{
    super::CompleteRequest,
    run::{Harness, request, started},
};

#[track_caller]
fn completed(response: ResponseBody) -> fleet_core::agents::Delegation {
    match response {
        ResponseBody::Delegation(delegation) => delegation,
        other => panic!("expected Delegation, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn complete_then_settle_succeeds_with_the_reported_result() {
    let harness = Harness::start().await;
    let (caller, _) = harness.running_caller().await;
    let (delegation, _) = started(
        harness
            .run(request(caller))
            .await
            .expect("start held delegation"),
    );
    let token = harness.token(&delegation).await;

    let reported = completed(
        harness
            .service()
            .complete(CompleteRequest {
                delegation: delegation.id,
                child: delegation.child,
                token,
                result: "verified report".to_owned(),
                blocked: false,
            })
            .await
            .expect("complete delegation"),
    );
    assert_eq!(reported.status, DelegationStatus::Running);

    harness
        .send(delegation.child, "finish after reporting")
        .await;
    let succeeded = harness
        .wait_for_delegation(delegation.id, |current| {
            current.status == DelegationStatus::Succeeded
        })
        .await;

    assert_eq!(
        succeeded.result.as_ref().map(|result| result.source),
        Some(ResultSource::Reported)
    );
    assert_eq!(
        succeeded.result.as_ref().map(|result| result.text.as_str()),
        Some("verified report")
    );
    harness
        .wait_for_outbox(delegation.id, OutboxAction::Deliver)
        .await;
}

#[tokio::test(start_paused = true)]
async fn settlement_without_complete_nudges_twice_then_delivers_incomplete_last_text() {
    let harness = Harness::start().await;
    let (caller, _) = harness.running_caller().await;
    let mut run = request(caller);
    run.brief = "finish without reporting".to_owned();
    let (delegation, _) = started(harness.run(run).await.expect("start unreported delegation"));

    harness
        .wait_for_outbox(delegation.id, OutboxAction::Nudge)
        .await;
    // SessionConfigured queued an older Mirror for the same caller. One pass performs that row;
    // the next performs the nudge, matching the worker's one-row-per-caller rule.
    harness.drain().await;
    harness.drain().await;
    harness
        .wait_for_delegation(delegation.id, |current| current.nudges == 1)
        .await;

    harness
        .wait_for_outbox(delegation.id, OutboxAction::Nudge)
        .await;
    harness.drain().await;
    harness
        .wait_for_delegation(delegation.id, |current| current.nudges == 2)
        .await;

    harness
        .wait_for_outbox(delegation.id, OutboxAction::Deliver)
        .await;
    let incomplete = harness
        .wait_for_delegation(delegation.id, |current| {
            current.status == DelegationStatus::Incomplete
        })
        .await;

    let result = incomplete
        .result
        .expect("incomplete delegation has fallback text");
    assert_eq!(result.source, ResultSource::LastAssistantText);
    assert_eq!(result.text, "scripted answer");
}

#[tokio::test(start_paused = true)]
async fn a_failed_outcome_delivers_a_failed_delegation() {
    let harness = Harness::start().await;
    let (caller, _) = harness.running_caller().await;
    let mut run = request(caller);
    run.brief = "fail-outcome".to_owned();
    run.eager = true;
    let (delegation, _) = started(harness.run(run).await.expect("start failing delegation"));

    harness
        .wait_for_outbox(delegation.id, OutboxAction::Deliver)
        .await;
    let failed = harness
        .wait_for_delegation(delegation.id, |current| {
            current.status == DelegationStatus::Failed
        })
        .await;
    assert!(failed.status_payload.is_some());

    harness.drain().await;
    harness.drain().await;
    let delivered = harness
        .wait_for_delegation(delegation.id, |current| {
            matches!(current.delivery, DeliveryState::Delivered { .. })
        })
        .await;
    assert_eq!(delivered.status, DelegationStatus::Failed);
}

#[tokio::test(start_paused = true)]
async fn stopping_the_child_delivers_cancelled() {
    let harness = Harness::start().await;
    let (caller, _) = harness.running_caller().await;
    let mut run = request(caller);
    run.eager = true;
    let (delegation, _) = started(harness.run(run).await.expect("start held delegation"));
    harness
        .wait_for_delegation(delegation.id, |current| {
            current.status == DelegationStatus::Running
        })
        .await;

    harness.stop(delegation.child).await;
    harness
        .wait_for_outbox(delegation.id, OutboxAction::Deliver)
        .await;
    let cancelled = harness
        .wait_for_delegation(delegation.id, |current| {
            current.status == DelegationStatus::Cancelled
        })
        .await;
    assert_eq!(cancelled.status, DelegationStatus::Cancelled);

    harness.drain().await;
    harness.drain().await;
    let delivered = harness
        .wait_for_delegation(delegation.id, |current| {
            matches!(current.delivery, DeliveryState::Delivered { .. })
        })
        .await;
    assert_eq!(delivered.status, DelegationStatus::Cancelled);
}
