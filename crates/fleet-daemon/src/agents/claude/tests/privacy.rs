//! Privacy tests for the Claude adapter.

use super::*;
use fleet_core::agents::ItemId;

/// Privacy test 1 of 3: a decode failure carries a fingerprint and no payload, anywhere.
#[test]
fn a_decode_failure_never_reaches_a_log_line() {
    let line = format!(
        "{{\"type\":\"control_request\",\"request\":{{\"input\":{{\"token\":\"{}\"}}}}}}",
        SECRETS[0]
    );
    let (parsed, logs) = logged(|| parse_frame(&line));
    let ParsedFrame::Undecodable { fingerprint, .. } = parsed else {
        panic!("the frame must not decode");
    };
    assert_no_secret(&format!("{fingerprint:?}"), "the fingerprint");
    assert_no_secret(&logs, "parsing a frame");
}

/// Privacy test 2 of 3: an unknown frame's event and log carry the type name only.
#[test]
fn an_unknown_frame_never_carries_its_payload() {
    let line = json!({"type": "holograph", "prompt": SECRETS[1], "key": SECRETS[0]}).to_string();
    let mut session = ClaudeSession::default();
    let (output, logs) = logged(|| map::handle(&mut session, frame(&line)));
    assert_no_secret(&logs, "the unknown-frame warning");
    assert_no_secret(&format!("{:?}", output.events), "the degraded event");
}

/// Privacy test 3 of 3: an error a caller sees names the operation, never the payload.
#[test]
fn an_error_that_crosses_the_boundary_names_no_payload() {
    let mut session = ClaudeSession::default();
    let turn = TurnId::new();
    session
        .begin_turn(turn, ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    // Answering a gate that does not exist.
    let error = map::gates::response_for(
        &session,
        fleet_core::agents::GateId::new(),
        &GateAnswer::Permission {
            choice: PermissionChoice::AllowOnce,
            edited_payload: Some(SECRETS[3].to_owned()),
        },
    )
    .err()
    .unwrap_or_else(|| panic!("an unknown gate is refused"));
    assert_no_secret(&error.to_string(), "a gate error");
    assert_no_secret(&format!("{error:?}"), "a gate error's debug form");
    // A submit that cannot be encoded.
    let oversized = UserInput {
        text: "x".repeat(200_000),
        attachments: Vec::new(),
        item: None,
    };
    let error = user_frame(&oversized)
        .err()
        .unwrap_or_else(|| panic!("an oversized message is refused"));
    assert!(!error.to_string().contains("xxxxxxxx"), "{error}");
}
