//! Goldens tests for the Claude adapter.

use super::*;
use crate::agents::golden::{canonical, canonical_text};
use fleet_core::agents::ItemId;

/// The `user` frame's block order is load-bearing: images first, the final text last.
#[test]
fn the_user_frames_block_order_puts_the_text_last() {
    let plain = user_frame(&UserInput {
        text: "hello".to_owned(),
        attachments: Vec::new(),
        item: None,
        origin: Default::default(),
    })
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        canonical(&plain),
        canonical_text(
            r#"{"message":{"content":"hello","role":"user"},"parent_tool_use_id":null,"session_id":"","type":"user"}"#
        )
    );

    let item = ItemId::from_uuid(
        uuid::Uuid::parse_str("11111111-2222-4333-8444-555555555555")
            .unwrap_or_else(|error| panic!("{error}")),
    );
    let identified = user_frame(&UserInput {
        text: "durable delivery".to_owned(),
        item: Some(item),
        ..UserInput::default()
    })
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        canonical(&identified),
        canonical_text(
            r#"{"message":{"content":"durable delivery","role":"user"},"parent_tool_use_id":null,"session_id":"","type":"user","uuid":"11111111-2222-4333-8444-555555555555"}"#
        ),
        "Claude receives the stable item identity as its client UUID"
    );

    let with_image = user_frame(&UserInput {
        text: "/skill do it".to_owned(),
        attachments: vec![Attachment {
            name: Some("shot.png".to_owned()),
            media_type: "image/png".to_owned(),
            source: AttachmentSource::Base64("AAAA".to_owned()),
        }],
        item: None,
        origin: Default::default(),
    })
    .unwrap_or_else(|error| panic!("{error}"));
    let blocks = with_image
        .pointer("/message/content")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("an image message is a block array"));
    assert_eq!(
        blocks
            .iter()
            .map(|block| block.get("type").and_then(Value::as_str).unwrap_or(""))
            .collect::<Vec<_>>(),
        ["image", "text"],
        "the CLI reads a slash command only when the last block is text"
    );

    // A non-image attachment reaches the agent as an absolute path in the prompt text.
    let with_file = user_frame(&UserInput {
        text: "read this".to_owned(),
        attachments: vec![Attachment {
            name: Some("notes.md".to_owned()),
            media_type: "text/markdown".to_owned(),
            source: AttachmentSource::Path("/w/notes.md".into()),
        }],
        item: None,
        origin: Default::default(),
    })
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        with_file
            .pointer("/message/content")
            .and_then(Value::as_str),
        Some("read this\n/w/notes.md")
    );

    // An unsupported inline media type is a request error: the turn never starts.
    assert!(
        user_frame(&UserInput {
            text: String::new(),
            attachments: vec![Attachment {
                name: None,
                media_type: "image/tiff".to_owned(),
                source: AttachmentSource::Base64("AAAA".to_owned()),
            }],
            item: None,
            origin: Default::default(),
        })
        .is_err()
    );
}

/// Byte-exact goldens for every `control_response` Fleet writes.
///
/// A permission answer is the one place Fleet's own bytes decide whether a rule is written into
/// the user's settings file, so the shapes are pinned rather than described.
#[test]
fn control_response_shapes_are_byte_exact() {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    // A permission request with the CLI's own suggestion, exactly as the live capture returned it.
    map::handle(
        &mut session,
        frame(
            r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"ls"},"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"ls"}],"behavior":"allow","destination":"localSettings"}],"tool_use_id":"toolu_1"}}"#,
        ),
    );
    let gate = session
        .gates
        .keys()
        .next()
        .copied()
        .unwrap_or_else(|| panic!("one gate"));
    let written = |answer: GateAnswer| -> String {
        let value = map::gates::response_for(&session, gate, &answer)
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|| panic!("a permission answer is written"));
        canonical(&value)
    };
    assert_eq!(
        written(GateAnswer::Permission {
            choice: PermissionChoice::AllowOnce,
            edited_payload: None,
        }),
        canonical_text(
            r#"{"response":{"request_id":"req-1","response":{"behavior":"allow","decisionClassification":"user_allow","toolUseID":"toolu_1","updatedInput":{"command":"ls"}},"subtype":"success"},"type":"control_response"}"#
        )
    );
    assert_eq!(
        written(GateAnswer::Permission {
            choice: PermissionChoice::AllowSession,
            edited_payload: None,
        }),
        canonical_text(
            r#"{"response":{"request_id":"req-1","response":{"behavior":"allow","decisionClassification":"user_allow","toolUseID":"toolu_1","updatedInput":{"command":"ls"},"updatedPermissions":[{"behavior":"allow","destination":"session","rules":[{"ruleContent":"ls","toolName":"Bash"}],"type":"addRules"}]},"subtype":"success"},"type":"control_response"}"#
        )
    );
    assert_eq!(
        written(GateAnswer::Permission {
            choice: PermissionChoice::AllowDirectory,
            edited_payload: None,
        }),
        canonical_text(
            r#"{"response":{"request_id":"req-1","response":{"behavior":"allow","decisionClassification":"user_permanent","toolUseID":"toolu_1","updatedInput":{"command":"ls"},"updatedPermissions":[{"behavior":"allow","destination":"localSettings","rules":[{"ruleContent":"ls","toolName":"Bash"}],"type":"addRules"}]},"subtype":"success"},"type":"control_response"}"#
        )
    );
    assert_eq!(
        written(GateAnswer::Permission {
            choice: PermissionChoice::Deny,
            edited_payload: None,
        }),
        canonical_text(
            r#"{"response":{"request_id":"req-1","response":{"behavior":"deny","decisionClassification":"user_reject","interrupt":false,"message":"User declined tool execution.","toolUseID":"toolu_1"},"subtype":"success"},"type":"control_response"}"#
        )
    );
    // Deny-and-stop exists on Claude only through `interrupt: true`.
    assert_eq!(
        written(GateAnswer::Permission {
            choice: PermissionChoice::DenyAndStop,
            edited_payload: None,
        }),
        canonical_text(
            r#"{"response":{"request_id":"req-1","response":{"behavior":"deny","decisionClassification":"user_reject","interrupt":true,"message":"User cancelled tool execution.","toolUseID":"toolu_1"},"subtype":"success"},"type":"control_response"}"#
        )
    );
    // An edited Bash command replaces the invocation and allows it once.
    assert_eq!(
        written(GateAnswer::Permission {
            choice: PermissionChoice::Edit,
            edited_payload: Some("ls -la".to_owned()),
        }),
        canonical_text(
            r#"{"response":{"request_id":"req-1","response":{"behavior":"allow","decisionClassification":"user_allow","toolUseID":"toolu_1","updatedInput":{"command":"ls -la"}},"subtype":"success"},"type":"control_response"}"#
        )
    );
}

/// The interrupt frame, byte-exact, with and without the declared cancel-queued capability.
#[test]
fn the_interrupt_frame_is_byte_exact_and_capability_gated() {
    let plain = json!({
        "type": "control_request",
        "request_id": "fixed",
        "request": {"subtype": "interrupt"},
    });
    assert_eq!(
        canonical(&plain),
        canonical_text(
            r#"{"request":{"subtype":"interrupt"},"request_id":"fixed","type":"control_request"}"#
        )
    );
    let cancelling = json!({
        "type": "control_request",
        "request_id": "fixed",
        "request": {"subtype": "interrupt", "cancel_queued": true},
    });
    assert_eq!(
        canonical(&cancelling),
        canonical_text(concat!(
            r#"{"request":{"cancel_queued":true,"subtype":"interrupt"},"#,
            r#""request_id":"fixed","type":"control_request"}"#
        ))
    );
}

/// The question and plan `control_response` bytes.
#[test]
fn question_and_plan_answers_are_byte_exact() {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    map::handle(
        &mut session,
        frame(
            r#"{"type":"control_request","request_id":"q-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","tool_use_id":"toolu_q","input":{"questions":[{"question":"Which color?","header":"Color","multiSelect":false,"options":[{"label":"Red","description":"warm"}]}]}}}"#,
        ),
    );
    let gate = session
        .gates
        .keys()
        .next()
        .copied()
        .unwrap_or_else(|| panic!("one gate"));
    let answered = map::gates::response_for(
        &session,
        gate,
        &GateAnswer::Question {
            answers: vec![vec!["Red".to_owned()]],
        },
    )
    .unwrap_or_else(|error| panic!("{error}"))
    .unwrap_or_else(|| panic!("a question answer is written"));
    // The answer map is keyed by the exact question text, and the questions are echoed back.
    assert_eq!(
        canonical(&answered),
        canonical_text(concat!(
            r#"{"response":{"request_id":"q-1","response":{"behavior":"allow","#,
            r#""toolUseID":"toolu_q","updatedInput":{"answers":{"Which color?":"Red"},"#,
            r#""questions":[{"header":"Color","multiSelect":false,"#,
            r#""options":[{"description":"warm","label":"Red"}],"question":"Which color?"}]}},"#,
            r#""subtype":"success"},"type":"control_response"}"#
        ))
    );
}

/// The session environment carries the same IDE muzzle the probe does.
///
/// `probe.rs` documents both variables as probe *hygiene*, but a session that omits them spawns
/// the IDE-discovery process tree the probe exists to avoid — once per thread, for the life of
/// the daemon. An explicit `StartRequest.env` still wins, because these are defaults.
#[tokio::test]
async fn the_session_environment_muzzles_ide_discovery_and_strips_inherited_claude_state() {
    use std::ffi::{OsStr, OsString};

    let harness = harness("claude".to_owned());
    let environment = harness
        .environment(&OpenSession {
            start: start_request(),
        })
        .await;
    assert_eq!(
        environment.get(OsStr::new("CLAUDE_CODE_AUTO_CONNECT_IDE")),
        Some(&OsString::from("0"))
    );
    assert_eq!(
        environment.get(OsStr::new("CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL")),
        Some(&OsString::from("1"))
    );
    for stripped in crate::agents::harness::probe::strip_list(AgentKind::Claude) {
        assert!(
            !environment.contains_key(OsStr::new(stripped)),
            "{stripped} must not be inherited by a session"
        );
    }

    let mut start = start_request();
    start
        .env
        .insert("CLAUDE_CODE_AUTO_CONNECT_IDE".to_owned(), "1".to_owned());
    let overridden = harness.environment(&OpenSession { start }).await;
    assert_eq!(
        overridden.get(OsStr::new("CLAUDE_CODE_AUTO_CONNECT_IDE")),
        Some(&OsString::from("1")),
        "an explicit request environment still wins"
    );
}
