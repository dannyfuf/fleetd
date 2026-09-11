//! Peer tests for the Claude adapter.

use super::*;

/// The mock peer: a real child that greets with `system/init` and answers one turn.
#[tokio::test]
async fn a_scripted_peer_drives_init_a_turn_and_its_result() {
    let init = json!({
        "type": "system",
        "subtype": "init",
        "session_id": "6b8fc1c4-2f4e-4c4a-9f1a-6b7f0e2c1d3e",
        "capabilities": ["interrupt_receipt_v1", "interrupt_cancel_queued_v1", "msg_lifecycle_v1"],
        "model": "claude-haiku-4-5-20251001",
        "permissionMode": "default",
        "tools": ["Read", "Bash"],
        "slash_commands": ["compact"],
        "skills": [],
        "claude_code_version": "2.1.266",
    })
    .to_string();
    let result = json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "terminal_reason": "completed",
        "duration_ms": 12,
        "session_id": "6b8fc1c4-2f4e-4c4a-9f1a-6b7f0e2c1d3e",
        "usage": {"input_tokens": 2, "output_tokens": 4},
        "modelUsage": {"claude-haiku-4-5-20251001": {"contextWindow": 200000}},
    })
    .to_string();
    // A frame with neither `method` nor `subtype` is the `user` line Fleet writes.
    let peer = MockPeer::new()
        .greeting(&[&init])
        .on("", Some(&result), &[])
        .lingering()
        .build();

    let mut harness = harness(peer.command());
    let mut events = harness.events();
    let opened = harness
        .open(OpenSession {
            start: start_request(),
        })
        .await
        .unwrap_or_else(|error| panic!("open: {error}"));
    assert_eq!(
        opened.resume_cursor.as_deref(),
        Some("6b8fc1c4-2f4e-4c4a-9f1a-6b7f0e2c1d3e"),
        "the cursor is durable before any turn runs"
    );
    // The capability gate is read from `system/init`, never from a version compare.
    assert!(matches!(
        harness.capabilities().interrupt,
        fleet_core::agents::InterruptSupport::Receipted {
            cancel_queued: true
        }
    ));

    let turn = TurnId::new();
    let submitted = harness
        .submit(Submit {
            turn,
            input: UserInput {
                text: "hello".to_owned(),
                attachments: Vec::new(),
                item: None,
            },
            intent: SubmitIntent::Fresh,
        })
        .await
        .unwrap_or_else(|error| panic!("submit: {error}"));
    assert_eq!(submitted.turn, turn);
    assert!(!submitted.queued);

    let mut seen = Vec::new();
    let settled = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            let terminal = matches!(event.event, AgentEvent::TurnSettled { .. });
            seen.push(event.event);
            if terminal {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or_else(|_| panic!("the result never settled the turn: {:?}", names(&seen)));
    assert!(settled);
    let mapped = names(&seen);
    assert!(mapped.contains(&"turn_started"), "{mapped:?}");
    assert_eq!(mapped.last().copied(), Some("turn_settled"));

    let written = peer.wait_for_frames(1, Duration::from_secs(5)).await;
    let first = written
        .first()
        .unwrap_or_else(|| panic!("Fleet wrote a frame"));
    assert!(first.contains(r#""type":"user""#), "{first}");
    harness
        .shutdown(ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
}

/// Stop against a child that never answers still finishes, and the child is killed.
#[tokio::test]
async fn stop_finishes_against_a_child_that_ignores_sigterm() {
    let peer = MockPeer::new().ignoring_sigterm().build();
    let mut harness = harness(peer.command());
    let events = harness.events();
    harness
        .open(OpenSession {
            start: start_request(),
        })
        .await
        .unwrap_or_else(|error| panic!("a silent child is still a live session: {error}"));
    let stopped = tokio::time::timeout(
        Duration::from_secs(20),
        harness.shutdown(ShutdownReason::User),
    )
    .await
    .unwrap_or_else(|_| panic!("shutdown must not hang on a wedged child"));
    stopped.unwrap_or_else(|error| panic!("shutdown: {error}"));
    drop(events);
}
