//! Byte-exact outbound-frame and privacy regression tests.

use super::*;
use crate::agents::codex::{params, session};

#[test]
fn outbound_frames_are_byte_exact() {
    let harness = harness("codex".to_owned());
    let initialize = serde_json::to_value(envelope::OutboundRequest {
        id: 1,
        method: "initialize",
        params: Some(harness.initialize_params(false)),
    })
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        canonical(&initialize),
        canonical_text(
            r#"{"id":1,"method":"initialize","params":{"capabilities":{"experimentalApi":true,"mcpServerOpenaiFormElicitation":false,"requestAttestation":false},"clientInfo":{"name":"fleet","title":"Fleet","version":"0.1.0"}}}"#
        )
    );
    // The suppression list rides on the capabilities and nowhere else.
    let suppressed = harness.initialize_params(true);
    assert_eq!(
        suppressed
            .pointer("/capabilities/optOutNotificationMethods")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(methods::OPT_OUT_NOTIFICATION_METHODS.len())
    );

    let start = harness.start_params(
        &OpenSession {
            start: StartRequest {
                worktree_path: std::path::PathBuf::from("/w"),
                ..start_request()
            },
        },
        &session::TurnControls::from_mode(PermissionMode::Ask),
    );
    assert_eq!(
        canonical(&start),
        canonical_text(
            r#"{"approvalPolicy":"untrusted","approvalsReviewer":"user","cwd":"/w","model":"gpt-5.1-codex","sandbox":"read-only"}"#
        )
    );

    let item = ItemId::from_uuid(
        uuid::Uuid::parse_str("11111111-2222-4333-8444-555555555555")
            .unwrap_or_else(|error| panic!("{error}")),
    );
    let turn = harness.turn_params(
        "01a089f2-5337-7470-adb1-219e71d62a35",
        &UserInput {
            text: "hello".to_owned(),
            attachments: Vec::new(),
            item: None,
        },
        item,
        &session::TurnControls {
            model: Some("gpt-5.1-codex".to_owned()),
            effort: Some("high".to_owned()),
            ..session::TurnControls::from_mode(PermissionMode::AcceptEdits)
        },
    );
    assert_eq!(
        canonical(&turn),
        canonical_text(
            r#"{"approvalPolicy":"on-request","approvalsReviewer":"user","clientUserMessageId":"11111111-2222-4333-8444-555555555555","effort":"high","input":[{"text":"hello","type":"text"}],"model":"gpt-5.1-codex","sandboxPolicy":{"type":"workspaceWrite"},"threadId":"01a089f2-5337-7470-adb1-219e71d62a35"}"#
        )
    );
}

/// The remaining outbound frames, byte-exact: steer, interrupt, settings and compaction.
#[test]
fn every_other_outbound_frame_is_byte_exact() {
    let harness = harness("codex".to_owned());
    let thread = "01a089f2-5337-7470-adb1-219e71d62a35";
    let turn = "01a089f2-579b-75c2-8e51-3492ec617046";
    let item = ItemId::from_uuid(
        uuid::Uuid::parse_str("11111111-2222-4333-8444-555555555555")
            .unwrap_or_else(|error| panic!("{error}")),
    );
    let steer = harness.steer_params(
        thread,
        &UserInput {
            text: "actually, do this".to_owned(),
            attachments: Vec::new(),
            item: None,
        },
        item,
        turn,
    );
    assert_eq!(
        canonical(&steer),
        canonical_text(concat!(
            r#"{"clientUserMessageId":"11111111-2222-4333-8444-555555555555","#,
            r#""expectedTurnId":"01a089f2-579b-75c2-8e51-3492ec617046","#,
            r#""input":[{"text":"actually, do this","type":"text"}],"#,
            r#""threadId":"01a089f2-5337-7470-adb1-219e71d62a35"}"#
        ))
    );
    assert_eq!(
        canonical(&params::interrupt_params(thread, turn)),
        canonical_text(concat!(
            r#"{"threadId":"01a089f2-5337-7470-adb1-219e71d62a35","#,
            r#""turnId":"01a089f2-579b-75c2-8e51-3492ec617046"}"#
        ))
    );
    assert_eq!(
        canonical(&params::compact_params(thread)),
        canonical_text(r#"{"threadId":"01a089f2-5337-7470-adb1-219e71d62a35"}"#)
    );
    let settings = params::settings_params(
        thread,
        &session::TurnControls {
            model: Some("gpt-5.1-codex".to_owned()),
            ..session::TurnControls::from_mode(PermissionMode::FullAccess)
        },
    );
    assert_eq!(
        canonical(&settings),
        canonical_text(concat!(
            r#"{"approvalPolicy":"never","approvalsReviewer":"user","model":"gpt-5.1-codex","#,
            r#""sandboxPolicy":{"type":"dangerFullAccess"},"#,
            r#""threadId":"01a089f2-5337-7470-adb1-219e71d62a35"}"#
        ))
    );
}

/// A skill and a mention are typed parts, never `@path` interpolated into prose.
#[test]
fn structured_input_parts_stay_structured() {
    let input = UserInput {
        text: "look at this".to_owned(),
        attachments: vec![fleet_core::agents::Attachment {
            name: Some("notes.md".to_owned()),
            media_type: "text/markdown".to_owned(),
            source: fleet_core::agents::AttachmentSource::Path("/w/notes.md".into()),
        }],
        item: None,
    };
    assert_eq!(
        user_input(&input),
        json!([
            {"type": "text", "text": "look at this"},
            {"type": "mention", "name": "notes.md", "path": "/w/notes.md"},
        ])
    );
}

#[test]
fn the_version_comes_from_the_user_agent_because_there_is_no_protocol_version() {
    assert_eq!(
        user_agent_version("fleet/0.147.0 (Linux Unknown; x86_64) xterm-256color"),
        Some(Version::new(0, 147, 0))
    );
    assert_eq!(user_agent_version("nonsense"), None);
}

/// Privacy test 1 of 3: an undecodable notification names its method and nothing else.
#[test]
fn a_decode_failure_never_carries_the_payload() {
    let mut session = CodexSession {
        root: Some("t".to_owned()),
        ..CodexSession::default()
    };
    let params = json!({
        "threadId": "t",
        "turn": {"id": 42, "secret": SECRETS[0]},
    });
    let (output, logs) = logged(|| map::handle(&mut session, "turn/completed", &params));
    match output.events.first() {
        Some(AgentEvent::Unknown { method }) => assert_eq!(method, "turn/completed"),
        other => panic!("expected a degraded event, got {other:?}"),
    }
    assert_no_secret(&logs, "the Codex decode warning");
    assert!(
        logs.contains("fingerprint="),
        "the log must be structural: {logs}"
    );
}

/// Privacy test 2 of 3: an unroutable line is counted and never echoed.
#[test]
fn an_unroutable_line_is_counted_and_never_echoed() {
    let line = format!("{{\"stray\":\"{}\"}}", SECRETS[3]);
    let value: Value = serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(envelope::classify(value), None);
    let (_, logs) = logged(|| {
        tracing::warn!(
            target: "fleet::agents::codex",
            unroutable = 1,
            "a Codex stdout line was not an envelope"
        );
    });
    assert_no_secret(&logs, "the unroutable-line warning");
}

/// Privacy test 3 of 3: a secret answer never reaches an event, a log line or an error.
#[test]
fn a_secret_answer_never_leaves_the_wire_response() {
    let mut session = CodexSession {
        root: Some("t".to_owned()),
        ..CodexSession::default()
    };
    let params = json!({
        "threadId": "t",
        "turnId": "01a089f2-579b-75c2-8e51-3492ec617046",
        "itemId": "tool-1",
        "isBlocking": true,
        "questions": [{
            "id": "q1",
            "header": "Token",
            "question": "Paste the deploy token",
            "isSecret": true,
            "options": null,
        }],
    });
    let (outcome, logs) = logged(|| {
        approvals::handle(
            &mut session,
            "item/tool/requestUserInput",
            &json!(7),
            &params,
        )
    });
    assert_no_secret(&logs, "the question gate's log");
    let Some(AgentEvent::GateOpened { gate, .. }) = outcome.events.first() else {
        panic!("expected a question gate");
    };
    let pending = session
        .gates
        .get(gate)
        .unwrap_or_else(|| panic!("pending"))
        .clone();
    let (answered, logs) = logged(|| {
        approvals::answer(
            &pending,
            *gate,
            &GateAnswer::Question {
                answers: vec![vec![SECRETS[0].to_owned()]],
            },
        )
    });
    let answered = answered.unwrap_or_else(|error| panic!("{error}"));
    // The secret is in the wire response — that is where it belongs — and nowhere else.
    assert!(
        serde_json::to_string(&answered)
            .unwrap_or_default()
            .contains(SECRETS[0])
    );
    assert_no_secret(&logs, "answering a secret question");
    assert_no_secret(&format!("{:?}", outcome.events), "the gate event");
}
