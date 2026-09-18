use fleet_core::agents::ThreadId;
use fleet_proto::error::ErrorKind;

use super::{
    super::limits::{MAX_DEPTH, MAX_LIVE_CHILDREN_PER_CALLER, MAX_LIVE_DELEGATIONS},
    run::{Harness, request},
};

#[tokio::test(start_paused = true)]
async fn a_depth_three_caller_is_refused_with_the_depth_limit() {
    let harness = Harness::start().await;
    let (caller, _) = harness.running_caller().await;
    harness
        .insert_live(ThreadId::new(), caller, MAX_DEPTH)
        .await;

    let error = harness
        .run(request(caller))
        .await
        .expect_err("a depth-three caller cannot create a child");

    assert_eq!(error.kind, ErrorKind::Conflict);
    assert!(error.message.contains("depth-limit rule"));
    assert!(error.message.contains(&MAX_DEPTH.to_string()));
}

#[tokio::test(start_paused = true)]
async fn a_fifth_live_child_is_refused_with_the_per_caller_limit() {
    let harness = Harness::start().await;
    let (caller, _) = harness.running_caller().await;
    for _ in 0..MAX_LIVE_CHILDREN_PER_CALLER {
        harness.insert_live(caller, ThreadId::new(), 1).await;
    }

    let error = harness
        .run(request(caller))
        .await
        .expect_err("a fifth live child is refused");

    assert_eq!(error.kind, ErrorKind::Conflict);
    assert!(error.message.contains("live-child-limit rule"));
    assert!(
        error
            .message
            .contains(&MAX_LIVE_CHILDREN_PER_CALLER.to_string())
    );
}

#[tokio::test(start_paused = true)]
async fn a_ninth_live_delegation_is_refused_with_the_daemon_limit() {
    let harness = Harness::start().await;
    for _ in 0..MAX_LIVE_DELEGATIONS {
        harness
            .insert_live(ThreadId::new(), ThreadId::new(), 1)
            .await;
    }
    let (caller, _) = harness.running_caller().await;

    let error = harness
        .run(request(caller))
        .await
        .expect_err("a ninth live delegation is refused");

    assert_eq!(error.kind, ErrorKind::Conflict);
    assert!(error.message.contains("daemon-live-limit rule"));
    assert!(error.message.contains(&MAX_LIVE_DELEGATIONS.to_string()));
}

#[tokio::test(start_paused = true)]
async fn concurrent_runs_cannot_both_take_the_last_child_slot() {
    let harness = Harness::start().await;
    let (caller, _) = harness.running_caller().await;
    for _ in 1..MAX_LIVE_CHILDREN_PER_CALLER {
        harness.insert_live(caller, ThreadId::new(), 1).await;
    }

    let (left, right) = harness.run_pair(request(caller), request(caller)).await;
    let outcomes = [left, right];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    let error = outcomes
        .into_iter()
        .find_map(Result::err)
        .expect("one concurrent run is refused");
    assert_eq!(error.kind, ErrorKind::Conflict);
    assert!(error.message.contains("live-child-limit rule"));
}
