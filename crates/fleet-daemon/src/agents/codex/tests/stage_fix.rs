//! Regressions for response/lifecycle races and malformed successful responses.

use super::*;
use crate::agents::harness::HarnessError;

#[test]
fn a_response_after_started_and_completed_does_not_reopen_the_turn() {
    let provider_turn = "01a089f2-579b-75c2-8e51-3492ec617046";
    let turn = TurnId::new();
    let mut session = CodexSession {
        root: Some("thread-1".to_owned()),
        ..CodexSession::default()
    };
    session.begin_turn(turn);

    let started = map::handle(
        &mut session,
        "turn/started",
        &json!({
            "threadId": "thread-1",
            "turn": {
                "id": provider_turn,
                "items": [],
                "itemsView": "notLoaded",
                "status": "inProgress",
            },
        }),
    );
    let settled = map::handle(
        &mut session,
        "turn/completed",
        &json!({
            "threadId": "thread-1",
            "turn": {
                "id": provider_turn,
                "items": [],
                "itemsView": "summary",
                "status": "completed",
            },
        }),
    );
    let response = session.confirm_turn_start(turn, provider_turn);

    assert_eq!(names(&started.events), ["turn_started"]);
    assert_eq!(names(&settled.events), ["turn_settled"]);
    assert!(
        !response.announce,
        "the late response emitted a second start"
    );
    assert!(!response.queued);
    assert_eq!(session.active_turn, None);
    assert_eq!(session.active_provider_turn, None);
    assert_eq!(session.pending_start, None);
}

#[tokio::test]
async fn a_successful_turn_response_without_a_string_id_is_rejected_and_rolled_back() {
    let thread = "01a089f2-5337-7470-adb1-219e71d62a35";
    let peer = MockPeer::new()
        .on(
            "initialize",
            Some(r#"{"id":__ID__,"result":{"userAgent":"fleet/0.147.0 (Linux)"}}"#),
            &[],
        )
        .on(
            "thread/start",
            Some(&format!(
                r#"{{"id":__ID__,"result":{{"thread":{{"id":"{thread}"}}}}}}"#
            )),
            &[],
        )
        .on(
            "model/list",
            Some(r#"{"id":__ID__,"result":{"data":[],"nextCursor":null}}"#),
            &[],
        )
        .on(
            "skills/list",
            Some(r#"{"id":__ID__,"result":{"data":[]}}"#),
            &[],
        )
        .on(
            "turn/start",
            Some(r#"{"id":__ID__,"result":{"turn":{"id":7}}}"#),
            &[],
        )
        .lingering()
        .build();
    let mut harness = harness(peer.command());
    harness
        .open(OpenSession {
            start: start_request(),
        })
        .await
        .unwrap_or_else(|error| panic!("open: {error}"));
    let turn = TurnId::new();
    let error = harness
        .submit(Submit {
            turn,
            input: UserInput {
                text: "hello".to_owned(),
                attachments: Vec::new(),
                item: Some(ItemId::new()),
            },
            intent: SubmitIntent::Fresh,
        })
        .await
        .expect_err("a non-string turn id must be a protocol error");

    assert!(matches!(error, HarnessError::Protocol { .. }), "{error}");
    let session = harness.session.lock().await;
    assert_eq!(session.pending_start, None);
    assert_eq!(session.active_turn, None);
    assert_eq!(session.active_provider_turn, None);
    assert!(!session.user_items.contains_key(&turn));
    drop(session);
    harness
        .shutdown(crate::agents::harness::ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
}
