//! Mapping tests for the Claude adapter.

use super::*;
use fleet_core::agents::ItemId;

/// The basic capture: one turn, one assistant block, one settlement.
#[test]
fn the_basic_capture_maps_to_one_turn_and_one_settlement() {
    let (_, events) = replay(BASIC);
    let mapped = names(&events);
    assert_eq!(mapped.first().copied(), Some("session_configured"));
    assert_eq!(mapped.last().copied(), Some("turn_settled"));
    assert!(mapped.contains(&"content_delta"));
    // The capture carries a `rate_limit_event`, which is now mapped rather than ignored.
    assert!(mapped.contains(&"rate_limits"), "{mapped:?}");
    let AgentEvent::TurnSettled {
        outcome,
        usage,
        duration_ms,
        ..
    } = events.last().unwrap_or_else(|| panic!("a settlement"))
    else {
        panic!("the turn must settle");
    };
    assert_eq!(outcome, &TurnOutcome::Completed);
    assert_eq!(*duration_ms, 2_822);
    assert_eq!(usage.cache_read_tokens, 10_038);
}

/// The permission capture: a tool's whole lifecycle, from block start to its result.
#[test]
fn the_permission_capture_maps_a_tool_lifecycle() {
    let (_, events) = replay(PERMISSION);
    let tools = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::ItemStarted {
                    kind: ItemKind::Tool(_),
                    ..
                }
            )
        })
        .count();
    assert!(tools >= 1, "{:?}", names(&events));
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ItemCompleted {
                status: ItemStatus::Completed,
                ..
            }
        )),
        "the tool result closes its row"
    );
}

/// The live `can_use_tool`: the card's payload, its options, and the session-scope rewrite.
#[test]
fn a_live_permission_request_carries_the_command_and_rewrites_the_scope() {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    let mut opened = None;
    for line in CAN_USE_TOOL.lines() {
        let output = map::handle(&mut session, frame(line));
        for event in output.events {
            if let AgentEvent::GateOpened { gate, kind, .. } = event {
                opened = Some((gate, kind));
            }
        }
    }
    let (gate, kind) = opened.unwrap_or_else(|| panic!("the capture opens a permission gate"));
    let GateKind::Permission {
        payload, options, ..
    } = kind
    else {
        panic!("expected a permission gate");
    };
    // The payload is the invocation, never the model's prose about it.
    assert!(payload.contains("curl"), "{payload}");
    let ids = options
        .iter()
        .map(|option| option.id.0.as_str())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"allow_once"), "{ids:?}");
    assert!(
        ids.contains(&"allow_session") && ids.contains(&"allow_persistent"),
        "all three scopes are offered: {ids:?}"
    );

    // The session answer rewrites `destination`, which the capture returned as `localSettings`.
    let response = map::gates::response_for(
        &session,
        gate,
        &GateAnswer::Permission {
            choice: PermissionChoice::AllowSession,
            edited_payload: None,
        },
    )
    .unwrap_or_else(|error| panic!("{error}"))
    .unwrap_or_else(|| panic!("a permission answer is written to the wire"));
    let updated = response
        .pointer("/response/response/updatedPermissions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a session answer carries rescoped permissions"));
    assert!(
        updated.iter().all(
            |suggestion| suggestion.get("destination").and_then(Value::as_str) == Some("session")
        ),
        "a session choice must never write a permanent rule: {updated:?}"
    );

    // The persistent answer keeps the destination the CLI offered, and says so in its own option.
    let persistent = map::gates::response_for(
        &session,
        gate,
        &GateAnswer::Permission {
            choice: PermissionChoice::AllowDirectory,
            edited_payload: None,
        },
    )
    .unwrap_or_else(|error| panic!("{error}"))
    .unwrap_or_else(|| panic!("written"));
    let updated = persistent
        .pointer("/response/response/updatedPermissions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("carried"));
    assert!(
        updated.iter().any(
            |suggestion| suggestion.get("destination").and_then(Value::as_str)
                == Some("localSettings")
        ),
        "the persistent choice keeps the CLI's own destination: {updated:?}"
    );
}

/// The live `AskUserQuestion`: answers are keyed by the **exact question text**.
#[test]
fn a_question_answer_is_keyed_by_the_exact_question_text() {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    let mut opened = None;
    for line in QUESTION.lines() {
        for event in map::handle(&mut session, frame(line)).events {
            if let AgentEvent::GateOpened { gate, kind, .. } = event {
                opened = Some((gate, kind));
            }
        }
    }
    let (gate, kind) = opened.unwrap_or_else(|| panic!("the capture asks a question"));
    let GateKind::Question { questions } = kind else {
        panic!("expected a question gate");
    };
    assert!(!questions.is_empty());
    let first = questions
        .first()
        .unwrap_or_else(|| panic!("at least one question"));
    // Claude's identity for a question *is* its text, and the option identity is its label.
    assert_eq!(first.id, first.prompt);
    assert!(first.blocking, "Claude has no non-blocking question");
    assert!(!first.is_secret, "Claude has no secret answers");
    let answers = questions
        .iter()
        .map(|question| {
            vec![
                question
                    .options
                    .first()
                    .map(|option| option.label.clone())
                    .unwrap_or_default(),
            ]
        })
        .collect::<Vec<_>>();
    let response = map::gates::response_for(&session, gate, &GateAnswer::Question { answers })
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("written"));
    let map = response
        .pointer("/response/response/updatedInput/answers")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("the answer map"));
    assert!(
        map.contains_key(&first.prompt),
        "the CLI looks answers up by question text: {map:?}"
    );
}

/// `ExitPlanMode` is denied the moment it arrives, in every mode, and becomes a plan decision.
#[test]
fn a_plan_request_is_denied_immediately_and_becomes_a_decision() {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    let output = map::handle(
        &mut session,
        frame(
            r##"{"type":"control_request","request_id":"plan-1","request":{"subtype":"can_use_tool","tool_name":"ExitPlanMode","input":{"plan":"# Plan\n- one\n- two"}}}"##,
        ),
    );
    assert_eq!(names(&output.events), ["plan_proposed"]);
    let AgentEvent::PlanProposed { gate, steps, .. } = &output.events[0] else {
        panic!("expected a plan");
    };
    assert_eq!(steps, &["one".to_owned(), "two".to_owned()]);
    // The wire is answered immediately, with the stop-and-wait instruction.
    let write = output.writes.first().unwrap_or_else(|| panic!("a denial"));
    assert_eq!(
        write
            .pointer("/response/response/behavior")
            .and_then(Value::as_str),
        Some("deny")
    );
    assert!(
        write
            .pointer("/response/response/message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("Stop here and wait")),
        "{write}"
    );
    // Answering the plan writes nothing more: the wire is already settled.
    assert!(
        map::gates::response_for(&session, *gate, &GateAnswer::Plan(PlanAnswer::Approve))
            .unwrap_or_else(|error| panic!("{error}"))
            .is_none()
    );
}

/// A withdrawn control request closes its card with no reply.
#[test]
fn a_cancelled_control_request_closes_its_gate() {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    map::handle(
        &mut session,
        frame(
            r#"{"type":"control_request","request_id":"perm-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}"#,
        ),
    );
    let output = map::handle(
        &mut session,
        frame(r#"{"type":"control_cancel_request","request_id":"perm-1"}"#),
    );
    assert_eq!(names(&output.events), ["gate_resolved"]);
    assert!(
        output.writes.is_empty(),
        "a cancel is answered with nothing"
    );
    let AgentEvent::GateResolved { by, .. } = &output.events[0] else {
        panic!("expected a resolution");
    };
    assert_eq!(by, &GateResolver::ProviderClosed);
}

/// A reused content-block index within one turn mints a **new** item id.
#[test]
fn a_reused_block_index_mints_a_new_item() {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    let mut items = Vec::new();
    for line in [
        r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text"}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"first"}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text"}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"second"}}}"#,
    ] {
        for event in map::handle(&mut session, frame(line)).events {
            if let AgentEvent::ItemStarted { item, .. } = event {
                items.push(item);
            }
        }
    }
    assert_eq!(items.len(), 2);
    assert_ne!(items[0], items[1], "two rows, two ids");
}

/// `signature_delta` is dropped: appending it to the thinking body is a visible corruption bug.
#[test]
fn a_signature_delta_never_becomes_display_text() {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    map::handle(
        &mut session,
        frame(
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}}"#,
        ),
    );
    let thinking = map::handle(
        &mut session,
        frame(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"pondering"}}}"#,
        ),
    );
    assert_eq!(names(&thinking.events), ["content_delta"]);
    let signature = map::handle(
        &mut session,
        frame(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"AAAA"}}}"#,
        ),
    );
    assert!(
        signature.events.is_empty(),
        "{:?}",
        names(&signature.events)
    );
}

/// Tool input deltas republish only when the **parsed** document changes.
#[test]
fn tool_input_deltas_republish_only_on_a_real_change() {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    map::handle(
        &mut session,
        frame(
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"Bash","input":{}}}}"#,
        ),
    );
    let mut updates = 0;
    // Six partial fragments, one complete document: one update, not six.
    for fragment in [r#"{"comm"#, r#"and":"#, r#""ls "#, r#"-la"}"#] {
        let line = json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": fragment},
            }
        })
        .to_string();
        updates += map::handle(&mut session, frame(&line))
            .events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ItemUpdated { .. }))
            .count();
    }
    assert_eq!(
        updates, 1,
        "one event per parsed document, not one per token"
    );
    // `content_block_stop` re-parses the same document and must not republish it.
    let stop = map::handle(
        &mut session,
        frame(r#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#),
    );
    assert!(stop.events.is_empty(), "{:?}", names(&stop.events));
}

/// Subagent narration is dropped; subagent tool blocks are not.
#[test]
fn subagent_narration_is_dropped_and_its_tools_are_kept() {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    map::handle(
        &mut session,
        frame(
            r#"{"type":"assistant","message":{"id":"msg_task","content":[{"type":"tool_use","id":"toolu_task","name":"Task","input":{"description":"explore"}}]}}"#,
        ),
    );
    let narration = map::handle(
        &mut session,
        frame(
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text"}},"parent_tool_use_id":"toolu_task"}"#,
        ),
    );
    assert!(
        narration.events.is_empty(),
        "a subagent's prose never interleaves into the chat"
    );
    let tool = map::handle(
        &mut session,
        frame(
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_child","name":"Read","input":{}}},"parent_tool_use_id":"toolu_task"}"#,
        ),
    );
    assert!(
        tool.events.iter().any(|event| matches!(
            event,
            AgentEvent::ItemStarted {
                parent: Some(_),
                ..
            }
        )),
        "a subagent's tool row is kept, attributed to its task: {:?}",
        names(&tool.events)
    );
}

/// A rejected rate-limit window parks the turn: a **state**, not a warning row.
#[test]
fn a_rejected_window_parks_the_turn_as_a_waiting_state() {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    let rejected = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1789029000,"rateLimitType":"five_hour","overageStatus":"rejected","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":1.0,"resetsAt":1789029000}}}}"#;
    let output = map::handle(&mut session, frame(rejected));
    assert_eq!(
        names(&output.events),
        ["rate_limits", "session_state_changed"]
    );
    let AgentEvent::SessionStateChanged(SessionState::Waiting(reason)) = &output.events[1] else {
        panic!("a parked turn is a state: {:?}", output.events);
    };
    let fleet_core::agents::WaitingReason::UsageLimit { window, resets_at } = reason;
    assert_eq!(window, "five_hour");
    assert_eq!(resets_at.timestamp(), 1_789_029_000);

    // The same window re-firing as the wait shrinks is not a second announcement.
    let again = map::handle(&mut session, frame(rejected));
    assert_eq!(names(&again.events), ["rate_limits"]);

    // A window with headroom stays quiet, even when the overage is refused.
    let allowed = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1789029000,"rateLimitType":"five_hour","overageStatus":"rejected"}}"#;
    assert_eq!(
        names(&map::handle(&mut session, frame(allowed)).events),
        ["rate_limits"]
    );
}

/// `system/permission_denied` must render: dropping it makes a refusal look like a hang.
#[test]
fn a_silent_denial_still_becomes_a_denied_row() {
    let mut session = ClaudeSession::default();
    session
        .begin_turn(TurnId::new(), ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    let output = map::handle(
        &mut session,
        frame(
            r#"{"type":"system","subtype":"permission_denied","tool_name":"Bash","tool_use_id":"toolu_never_seen","decision_reason":"This command requires approval","message":"This command requires approval"}"#,
        ),
    );
    let mapped = names(&output.events);
    assert!(mapped.contains(&"item_started"), "{mapped:?}");
    assert!(matches!(
        output.events.last(),
        Some(AgentEvent::ItemCompleted {
            status: ItemStatus::Denied,
            ..
        })
    ));
}

/// The known-noise list is data: firing all of it plus four loud frames yields four notices.
#[test]
fn the_known_noise_list_stays_out_of_the_transcript() {
    let mut session = ClaudeSession {
        initialized: true,
        ..ClaudeSession::default()
    };
    let mut notices = 0;
    for subtype in crate::agents::claude::frames::SILENT_SYSTEM_SUBTYPES {
        let line = json!({"type": "system", "subtype": subtype}).to_string();
        let output = map::handle(&mut session, frame(&line));
        assert!(
            output.events.is_empty(),
            "{subtype} must stay out of the transcript: {:?}",
            names(&output.events)
        );
    }
    for line in [
        json!({"type": "system", "subtype": "notification", "priority": "high", "text": "Disk is full"}),
        json!({"type": "system", "subtype": "informational", "level": "warning", "content": "Slow filesystem"}),
        json!({"type": "system", "subtype": "model_refusal_fallback", "content": "Falling back"}),
        json!({"type": "system", "subtype": "mirror_error", "message": "mirror failed"}),
    ] {
        let output = map::handle(&mut session, frame(&line.to_string()));
        notices += output
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    AgentEvent::Notice(_) | AgentEvent::RuntimeError { .. }
                )
            })
            .count();
    }
    assert_eq!(notices, 4);
    // A low-priority notification is CLI chrome and says nothing.
    let quiet =
        json!({"type": "system", "subtype": "notification", "priority": "low", "text": "hi"});
    assert!(
        map::handle(&mut session, frame(&quiet.to_string()))
            .events
            .is_empty()
    );
}

/// An unrecognised frame is counted and named, never silent and never a red row.
#[test]
fn an_unknown_frame_is_named_and_counted() {
    let mut session = ClaudeSession::default();
    let output = map::handle(&mut session, frame(r#"{"type":"holograph","payload":1}"#));
    assert_eq!(names(&output.events), ["unknown"]);
    let AgentEvent::Unknown { method } = &output.events[0] else {
        panic!("expected a named unknown");
    };
    assert_eq!(method, "holograph");
}

/// A `/compact` turn that settles without a boundary frame gets a synthesised one.
#[test]
fn a_compact_turn_without_a_boundary_synthesises_one() {
    let mut session = ClaudeSession::default();
    let turn = TurnId::new();
    session
        .begin_turn(turn, ItemId::new())
        .unwrap_or_else(|error| panic!("{error}"));
    session.compacting = Some(turn);
    let output = map::handle(
        &mut session,
        frame(
            r#"{"type":"result","subtype":"success","terminal_reason":"completed","duration_ms":10}"#,
        ),
    );
    let mapped = names(&output.events);
    assert!(mapped.contains(&"compacted"), "{mapped:?}");
    assert!(session.compacting.is_none());
}

/// The whole 2.1.266 capture, mapped: 84 frames, one turn, one settlement, nothing unknown.
#[test]
fn the_live_capture_maps_without_a_single_unknown_frame() {
    let (session, events) = replay(LIVE_TURN);
    let mapped = names(&events);
    assert!(
        !mapped.contains(&"unknown"),
        "every frame in the ground-truth capture has a mapping: {mapped:?}"
    );
    assert_eq!(mapped.first().copied(), Some("session_configured"));
    assert_eq!(mapped.last().copied(), Some("turn_settled"));
    // `system/status` and `system/thinking_tokens` are the truthful spinner sub-labels.
    assert!(
        mapped
            .iter()
            .filter(|name| **name == "session_activity")
            .count()
            >= 4,
        "{mapped:?}"
    );
    // The capture's thinking block becomes a reasoning row, and its `signature_delta` does not
    // become text.
    let reasoning = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::ItemStarted {
                    kind: ItemKind::Reasoning { .. },
                    ..
                }
            )
        })
        .count();
    assert!(reasoning >= 1, "{mapped:?}");
    let signatures = events.iter().any(
        |event| matches!(event, AgentEvent::ContentDelta { delta, .. } if delta.contains("Ev")),
    );
    assert!(!signatures, "a signature must never reach the transcript");
    // The turn settled, so nothing is left open and the cursor is the one `system/init` named.
    assert!(session.active_turn().is_none());
    assert_eq!(session.cursor.as_deref().map(str::len), Some(36));
}

/// The whole 2.1.266 capture of a gated Bash command: the card opens mid-turn and the row settles.
#[test]
fn the_live_permission_capture_opens_a_card_inside_a_running_turn() {
    let (session, events) = replay(LIVE_PERMISSION);
    let mapped = names(&events);
    assert!(!mapped.contains(&"unknown"), "{mapped:?}");
    let opened = mapped
        .iter()
        .position(|name| *name == "gate_opened")
        .unwrap_or_else(|| panic!("the capture asks for permission: {mapped:?}"));
    let settled = mapped
        .iter()
        .position(|name| *name == "turn_settled")
        .unwrap_or_else(|| panic!("the capture settles: {mapped:?}"));
    assert!(opened < settled, "the card opens while the turn runs");
    // The gate stays open until it is answered: a settlement never closes one.
    assert_eq!(session.gates.len(), 1, "gates are independent of turns");
    // Its payload is the command the user is approving.
    let AgentEvent::GateOpened { kind, .. } = &events[opened] else {
        panic!("expected a gate");
    };
    let GateKind::Permission { payload, tool, .. } = kind else {
        panic!("expected a permission gate");
    };
    assert_eq!(tool, &fleet_core::agents::ToolKind::Bash);
    assert!(!payload.trim().is_empty(), "the card must show the command");
}
