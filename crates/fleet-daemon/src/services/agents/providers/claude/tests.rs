use fleet_core::agents::{
    AbortReason, AgentEvent, AgentKind, GateAnswer, GateKind, ItemKind, ModelSelection,
    PermissionChoice, PermissionMode, PlanAnswer, StartRequest, ThreadId, TurnId, TurnOutcome,
    UserInput,
};
use serde_json::{Value, json};

use super::{
    AgentProvider, ClaudeProvider, interrupt_message, launch_args, map::ClaudeMapper, user_message,
    wire::parse_line,
};

const BASIC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agents/claude/claude-basic.ndjson"
));
const PERMISSION: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agents/claude/claude-permission-stdout.ndjson"
));
/// A turn whose Bash command the settings do **not** allowlist, so the CLI asks.
///
/// `claude-permission-stdout.ndjson` was captured with the command already allowed, so it holds
/// no `can_use_tool` frame at all and pins nothing about the permission mapping. This fixture's
/// control request is the frame documented in `docs/research/harness-protocols.md` (the
/// installed CLI's own shape, `default_to_no` and a `localSettings` suggestion included), so the
/// payload, the option ids and the answer's destination are asserted against real wire bytes
/// until a live capture with a denied command replaces it.
const CAN_USE_TOOL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agents/claude/claude-can-use-tool.ndjson"
));

fn replay(fixture: &str) -> Vec<AgentEvent> {
    let mut mapper = ClaudeMapper::default();
    let mut events = Vec::new();
    let mut started = false;
    for line in fixture.lines() {
        let message = parse_line(line).expect("fixture line parses");
        let is_init = matches!(
            &message,
            super::wire::ClaudeMessage::System(system) if system.subtype == "init"
        );
        events.extend(mapper.handle(message).expect("fixture maps").events);
        if is_init && !started {
            events.push(
                mapper
                    .begin_turn(TurnId::new())
                    .expect("turn starts")
                    .expect("new turn emits"),
            );
            started = true;
        }
    }
    events
}

fn event_name(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::SessionStarted { .. } => "session_started",
        AgentEvent::MetadataChanged { .. } => "metadata_changed",
        AgentEvent::SessionStateChanged(_) => "session_state_changed",
        AgentEvent::SessionExited { .. } => "session_exited",
        AgentEvent::TurnStarted { .. } => "turn_started",
        AgentEvent::TurnCompleted { .. } => "turn_completed",
        AgentEvent::TurnAborted { .. } => "turn_aborted",
        AgentEvent::ItemStarted { .. } => "item_started",
        AgentEvent::ContentDelta { .. } => "content_delta",
        AgentEvent::ItemUpdated { .. } => "item_updated",
        AgentEvent::ItemCompleted { .. } => "item_completed",
        AgentEvent::GateOpened { .. } => "gate_opened",
        AgentEvent::GateResolved { .. } => "gate_resolved",
        AgentEvent::TokenUsage { .. } => "token_usage",
        AgentEvent::Checkpoint(_) => "checkpoint",
        AgentEvent::Retrying { .. } => "retrying",
        AgentEvent::RuntimeError { .. } => "runtime_error",
        AgentEvent::Notice(_) => "notice",
    }
}

#[test]
fn basic_fixture_maps_exact_event_sequence_and_usage() {
    let events = replay(BASIC);
    assert_eq!(
        events.iter().map(event_name).collect::<Vec<_>>(),
        [
            "session_started",
            "session_state_changed",
            "turn_started",
            "item_started",
            "content_delta",
            "content_delta",
            "item_completed",
            "token_usage",
            "turn_completed",
        ]
    );
    assert!(matches!(
        &events[3],
        AgentEvent::ItemStarted {
            kind: ItemKind::AssistantText,
            ..
        }
    ));
    let AgentEvent::TurnCompleted {
        outcome,
        usage,
        duration_ms,
        files_changed,
        ..
    } = events.last().expect("completion")
    else {
        panic!("last event was not completion");
    };
    let AgentEvent::TokenUsage {
        context_pct,
        cost_usd,
        ..
    } = &events[events.len() - 2]
    else {
        panic!("result did not publish cost and context");
    };
    assert_eq!(*cost_usd, Some(0.431_029_5));
    assert!((*context_pct - 3.145_5).abs() < 0.001, "{context_pct}");
    assert_eq!(outcome, &TurnOutcome::Completed);
    assert_eq!(*duration_ms, 2_822);
    assert_eq!(usage.input_tokens, 2);
    assert_eq!(usage.output_tokens, 4);
    assert_eq!(usage.cache_read_tokens, 10_038);
    assert_eq!(usage.cache_write_tokens, 21_415);
    assert!(files_changed.is_empty());
}

#[test]
fn permission_fixture_maps_tool_lifecycle_and_completion() {
    let events = replay(PERMISSION);
    let tools = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::ItemStarted {
                    kind: ItemKind::Tool { name, .. },
                    ..
                } if name == "Bash"
            )
        })
        .count();
    assert_eq!(tools, 1);
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ItemCompleted {
            status: fleet_core::agents::ItemStatus::Done,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Completed,
            usage,
            duration_ms: 15_476,
            ..
        } if usage.output_tokens == 115
    )));
}

#[test]
fn a_live_permission_request_carries_the_command_and_answers_in_session_scope() {
    let mut mapper = ClaudeMapper::default();
    let mut gate = None;
    for line in CAN_USE_TOOL.lines() {
        let output = mapper
            .handle(parse_line(line).expect("fixture line parses"))
            .expect("fixture maps");
        for event in output.events {
            if let AgentEvent::GateOpened { gate: id, kind, .. } = event {
                gate = Some((id, kind));
            }
        }
        if gate.is_some() && mapper.active_turn().is_none() {
            mapper.begin_turn(TurnId::new()).expect("turn starts");
        }
    }
    let (gate, kind) = gate.expect("the fixture opens a permission gate");
    let GateKind::Permission {
        title,
        payload,
        rationale,
        options,
        ..
    } = kind
    else {
        panic!("expected a permission gate");
    };
    // The protected invocation, never the model's prose description.
    assert_eq!(payload, "curl -s https://example.com/fixture-9182");
    assert_eq!(title, "Claude wants to run a command");
    assert_eq!(rationale.as_deref(), Some("Command needs approval"));
    // `default_to_no` leads with deny, and no option id leaks a raw suggestion payload.
    assert_eq!(options[0].label, PermissionChoice::Deny);
    assert_eq!(
        options
            .iter()
            .map(|option| option.id.0.as_str())
            .collect::<Vec<_>>(),
        [
            "deny",
            "allow_once",
            "allow_session",
            "deny_and_stop",
            "edit"
        ]
    );

    let response = mapper
        .response_for(
            gate,
            &GateAnswer::Permission {
                choice: PermissionChoice::AllowSession,
                edited_payload: None,
            },
        )
        .expect("session answer");
    // The card says "for this session"; the wire must not write the user's settings file.
    assert_eq!(
        response["response"]["response"]["updatedPermissions"],
        json!([{
            "type":"addRules",
            "rules":[{"toolName":"Bash","ruleContent":"curl -s https://example.com/fixture-9182"}],
            "behavior":"allow",
            "destination":"session"
        }])
    );

    let edited = mapper
        .response_for(
            gate,
            &GateAnswer::Permission {
                choice: PermissionChoice::AllowOnce,
                edited_payload: Some("curl -s https://example.com/other".to_owned()),
            },
        )
        .expect("edited answer");
    assert_eq!(
        edited["response"]["response"]["updatedInput"]["command"],
        "curl -s https://example.com/other"
    );
}

#[test]
fn a_cancelled_control_request_closes_its_gate() {
    let mut mapper = ClaudeMapper::default();
    let (gate, _) = open_gate(
        &mut mapper,
        json!({
            "type":"control_request","request_id":"perm-cancel",
            "request":{"subtype":"can_use_tool","tool_name":"Bash",
            "input":{"command":"echo fixture"},"tool_use_id":"toolu_c"}
        }),
    );
    let cancelled = mapper
        .handle(
            parse_line(r#"{"type":"control_cancel_request","request_id":"perm-cancel"}"#)
                .expect("cancel parses"),
        )
        .expect("cancel maps");
    assert!(matches!(
        cancelled.events.as_slice(),
        [AgentEvent::GateResolved {
            gate: resolved,
            by: fleet_core::agents::GateResolver::ProviderClosed,
            ..
        }] if *resolved == gate
    ));
    assert!(cancelled.writes.is_empty());
    assert!(
        mapper
            .response_for(gate, &GateAnswer::Plan(PlanAnswer::Approve))
            .is_err()
    );
}

#[test]
fn a_steer_supersede_result_keeps_the_turn_open_until_the_real_result() {
    let mut mapper = ClaudeMapper::default();
    let turn = TurnId::new();
    mapper.begin_turn(turn).expect("turn starts");
    assert!(
        mapper
            .begin_turn(turn)
            .expect("steering the same turn is accepted")
            .is_none()
    );
    mapper.record_steer(turn);

    let superseded = mapper
        .handle(
            parse_line(
                r#"{"type":"result","subtype":"success","is_error":false,"terminal_reason":"aborted_streaming","duration_ms":4000,"usage":{"output_tokens":9},"queued_turn_count":1}"#,
            )
            .expect("supersede result parses"),
        )
        .expect("supersede maps");
    assert!(matches!(
        superseded.events.as_slice(),
        [AgentEvent::TokenUsage { turn: usage, .. }] if *usage == turn
    ));
    assert_eq!(mapper.active_turn(), Some(turn));

    let settled = mapper
        .handle(
            parse_line(
                r#"{"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed","duration_ms":9000,"usage":{"output_tokens":40},"queued_turn_count":0}"#,
            )
            .expect("final result parses"),
        )
        .expect("final result maps");
    assert!(matches!(
        settled.events.last(),
        Some(AgentEvent::TurnCompleted {
            turn: completed,
            outcome: TurnOutcome::Completed,
            ..
        }) if *completed == turn
    ));
    assert_eq!(mapper.active_turn(), None);
}

#[test]
fn a_subagent_stream_never_overwrites_the_main_agents_blocks() {
    let mut mapper = ClaudeMapper::default();
    mapper.begin_turn(TurnId::new()).expect("turn starts");
    mapper
        .handle(
            parse_line(
                r#"{"type":"assistant","message":{"id":"msg_task","content":[{"type":"tool_use","id":"toolu_task","name":"Task","input":{"description":"explore"}}]}}"#,
            )
            .expect("task tool parses"),
        )
        .expect("task tool maps");
    let main = mapper
        .handle(
            parse_line(
                r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},"parent_tool_use_id":null}"#,
            )
            .expect("main block parses"),
        )
        .expect("main block maps");
    let AgentEvent::ItemStarted {
        item: main_item, ..
    } = main.events.first().expect("main text row")
    else {
        panic!("expected the main agent's text row");
    };
    mapper
        .handle(
            parse_line(
                r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},"parent_tool_use_id":"toolu_task"}"#,
            )
            .expect("subagent block parses"),
        )
        .expect("subagent block maps");
    mapper
        .handle(
            parse_line(
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"main"}},"parent_tool_use_id":null}"#,
            )
            .expect("main delta parses"),
        )
        .expect("main delta maps");

    // The main agent's frame must complete the main agent's block, not the subagent's.
    let completed = mapper
        .handle(
            parse_line(
                r#"{"type":"assistant","message":{"id":"msg_main","content":[{"type":"text","text":"main"}]},"parent_tool_use_id":null}"#,
            )
            .expect("main assistant frame parses"),
        )
        .expect("main assistant frame maps");
    assert!(
        completed.events.iter().any(
            |event| matches!(event, AgentEvent::ItemCompleted { item, .. } if item == main_item)
        )
    );
}

#[test]
fn a_live_background_task_outlives_the_turn_that_started_it() {
    let mut mapper = ClaudeMapper::default();
    mapper.begin_turn(TurnId::new()).expect("turn starts");
    let started = mapper
        .handle(
            parse_line(
                r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"bg_1","task_type":"explore","description":"survey the repo"}]}"#,
            )
            .expect("background task frame parses"),
        )
        .expect("background task maps");
    let AgentEvent::ItemStarted { item: task, .. } = started
        .events
        .into_iter()
        .find(|event| matches!(event, AgentEvent::ItemStarted { .. }))
        .expect("background task row")
    else {
        panic!("expected a background task row");
    };

    // §3.3 lists "background tasks alive" as its own `Working` source, and
    // `background_tasks_changed` owns their lifetime: the turn terminal must not close one.
    let settled = mapper
        .handle(
            parse_line(
                r#"{"type":"result","subtype":"success","is_error":false,"terminal_reason":"completed","duration_ms":1200,"usage":{"output_tokens":7},"queued_turn_count":0}"#,
            )
            .expect("result parses"),
        )
        .expect("result maps");
    assert!(
        !settled
            .events
            .iter()
            .any(|event| matches!(event, AgentEvent::ItemCompleted { item, .. } if *item == task)),
        "the turn terminal closed a live background task",
    );

    // The list it drops out of is what closes it.
    let ended = mapper
        .handle(
            parse_line(r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#)
                .expect("empty task list parses"),
        )
        .expect("empty task list maps");
    assert!(
        ended
            .events
            .iter()
            .any(|event| matches!(event, AgentEvent::ItemCompleted { item, .. } if *item == task)),
    );
}

#[test]
fn a_permission_with_nothing_to_grant_never_offers_a_session_scope() {
    let mut mapper = ClaudeMapper::default();
    let (_, kind) = open_gate(
        &mut mapper,
        json!({
            "type": "control_request",
            "request_id": "perm-empty",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Read",
                "input": {"file_path": "/tmp/x"},
                "tool_use_id": "toolu_empty"
            }
        }),
    );
    let GateKind::Permission { options, .. } = kind else {
        panic!("permission gate");
    };
    // §3.2: the copy spells out the effective scope, and with no suggestion to echo an
    // `allow for this session` would grant exactly what `allow once` does.
    assert!(
        !options
            .iter()
            .any(|option| option.label == PermissionChoice::AllowSession),
        "a session grant was offered with no rule to install",
    );
    assert!(
        options
            .iter()
            .any(|option| option.label == PermissionChoice::AllowOnce)
    );
}

fn open_gate(mapper: &mut ClaudeMapper, request: Value) -> (fleet_core::agents::GateId, GateKind) {
    let output = mapper
        .handle(parse_line(&request.to_string()).expect("control request parses"))
        .expect("control request maps");
    let AgentEvent::GateOpened { gate, kind, .. } = output.events.into_iter().next().expect("gate")
    else {
        panic!("expected gate");
    };
    (gate, kind)
}

#[test]
fn bash_permission_allow_response_preserves_input_and_suggestions() {
    let mut mapper = ClaudeMapper::default();
    let request = json!({
        "type": "control_request",
        "request_id": "perm-1",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "input": {"command": "echo fixture"},
            "permission_suggestions": [{"type":"addRules","destination":"session"}],
            "decision_reason": "\u{1b}[31mNeeds approval\u{1b}[0m",
            "tool_use_id": "toolu_1"
        }
    });
    let (gate, kind) = open_gate(&mut mapper, request);
    let GateKind::Permission {
        rationale, options, ..
    } = kind
    else {
        panic!("permission gate");
    };
    assert_eq!(rationale.as_deref(), Some("Needs approval"));
    assert!(
        options
            .iter()
            .any(|option| option.label == PermissionChoice::Edit)
    );
    let response = mapper
        .response_for(
            gate,
            &GateAnswer::Permission {
                choice: PermissionChoice::AllowSession,
                edited_payload: None,
            },
        )
        .expect("response");
    assert_eq!(
        response,
        json!({
            "type":"control_response",
            "response":{
                "subtype":"success",
                "request_id":"perm-1",
                "response":{
                    "behavior":"allow",
                    "updatedInput":{"command":"echo fixture"},
                    "updatedPermissions":[{"type":"addRules","destination":"session"}],
                    "toolUseID":"toolu_1",
                    "decisionClassification":"user_permanent"
                }
            }
        })
    );
}

#[test]
fn ask_user_question_answer_bytes_key_by_exact_question() {
    let mut mapper = ClaudeMapper::default();
    let questions = json!([{
        "question":"Which database?",
        "header":"Database",
        "options":[{"label":"SQLite","description":"Local"}],
        "multiSelect":false
    }]);
    let (gate, kind) = open_gate(
        &mut mapper,
        json!({
            "type":"control_request","request_id":"q-1",
            "request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion",
            "input":{"questions":questions.clone()},"tool_use_id":"toolu_q"}
        }),
    );
    assert!(matches!(kind, GateKind::Question { .. }));
    let response = mapper
        .response_for(
            gate,
            &GateAnswer::Question {
                answers: vec![vec!["SQLite".to_owned()]],
            },
        )
        .expect("question response");
    assert_eq!(
        response["response"]["response"]["updatedInput"],
        json!({"questions":questions,"answers":{"Which database?":"SQLite"}})
    );
}

#[test]
fn exit_plan_mode_approve_and_changes_bytes() {
    let mut mapper = ClaudeMapper::default();
    let (gate, kind) = open_gate(
        &mut mapper,
        json!({
            "type":"control_request","request_id":"plan-1",
            "request":{"subtype":"can_use_tool","tool_name":"ExitPlanMode",
            "input":{"plan":"- inspect\n- change"},"tool_use_id":"toolu_p"}
        }),
    );
    assert!(matches!(
        kind,
        GateKind::Plan { ref steps, .. } if steps == &["inspect", "change"]
    ));
    let approve = mapper
        .response_for(gate, &GateAnswer::Plan(PlanAnswer::Approve))
        .expect("approve");
    assert_eq!(approve["response"]["response"]["behavior"], "allow");
    assert_eq!(
        approve["response"]["response"]["updatedInput"],
        json!({"plan":"- inspect\n- change"})
    );
    let changes = mapper
        .response_for(
            gate,
            &GateAnswer::Plan(PlanAnswer::AskForChanges {
                note: "cover rollback".to_owned(),
            }),
        )
        .expect("changes");
    assert_eq!(
        changes["response"]["response"],
        json!({
            "behavior":"deny","message":"cover rollback","interrupt":false,
            "toolUseID":"toolu_p"
        })
    );
}

#[test]
fn interrupt_result_aborts_and_eof_mid_turn_fails_session() {
    let mut mapper = ClaudeMapper::default();
    let turn = TurnId::new();
    mapper.begin_turn(turn).expect("turn");
    mapper.mark_interrupted(turn).expect("interrupt");
    let output = mapper
        .handle(
            parse_line(
                r#"{"type":"result","subtype":"success","is_error":true,"terminal_reason":"aborted_streaming","duration_ms":1,"usage":{}}"#,
            )
            .expect("result parses"),
        )
        .expect("result maps");
    assert_eq!(
        output.events.last(),
        Some(&AgentEvent::TurnAborted {
            turn,
            reason: AbortReason::User
        })
    );

    let mut mapper = ClaudeMapper::default();
    mapper.begin_turn(TurnId::new()).expect("turn");
    assert!(matches!(
        mapper.process_exit(None, false).as_slice(),
        [
            AgentEvent::SessionExited {
                code: None,
                expected: false
            },
            AgentEvent::RuntimeError { fatal: true, .. }
        ]
    ));
}

#[test]
fn send_and_interrupt_wire_values_match_protocol() {
    let sent = user_message(UserInput {
        text: "hello".to_owned(),
        attachments: Vec::new(),
    })
    .expect("user message");
    assert_eq!(sent["type"], "user");
    assert_eq!(sent["message"], json!({"role":"user","content":"hello"}));
    assert_eq!(sent["parent_tool_use_id"], Value::Null);
    assert_eq!(sent["priority"], "now");
    assert_eq!(sent["shouldQuery"], true);

    let interrupt = interrupt_message("i-1".to_owned());
    assert_eq!(interrupt["request"]["cancel_queued"], true);
}

#[test]
fn launch_argv_matches_fresh_resume_and_fork_protocol() {
    let session =
        uuid::Uuid::parse_str("11111111-1111-4111-8111-111111111111").expect("fixed session uuid");
    let mut request = StartRequest {
        thread: ThreadId::new(),
        worktree_path: "/tmp/fixture".into(),
        provider: AgentKind::Claude,
        model: Some(ModelSelection {
            model: "claude-sonnet-5".to_owned(),
            effort: None,
            provider: None,
        }),
        mode: PermissionMode::Plan,
        resume_cursor: None,
        title: None,
    };
    assert_eq!(
        launch_args(&request, session, false),
        [
            "-p",
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            "plan",
            "--model",
            "claude-sonnet-5",
            "--session-id=11111111-1111-4111-8111-111111111111",
        ]
    );
    request.resume_cursor = Some("resume-me".to_owned());
    let resumed = launch_args(&request, session, true);
    assert_eq!(
        &resumed[resumed.len() - 2..],
        ["--resume=resume-me", "--fork-session"]
    );
}

#[test]
fn edit_tool_result_derives_diff_and_file_counts() {
    let mut mapper = ClaudeMapper::default();
    mapper.begin_turn(TurnId::new()).expect("turn");
    let started = parse_line(
        r#"{"type":"assistant","message":{"id":"msg_edit","content":[{"type":"tool_use","id":"toolu_edit","name":"Edit","input":{"file_path":"src/lib.rs","old_string":"old","new_string":"new\nline"}}]}}"#,
    )
    .expect("tool parses");
    mapper.handle(started).expect("tool starts");
    let finished = parse_line(
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_edit","content":"updated","is_error":false}]},"tool_use_result":{"filePath":"src/lib.rs","oldString":"old","newString":"new\nline"}}"#,
    )
    .expect("result parses");
    let output = mapper.handle(finished).expect("result maps");
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::ItemUpdated { patch, .. }
            if matches!(&patch.diff, Some(diff) if diff.added == 2 && diff.removed == 1)
                && patch.summary.as_deref() == Some("src/lib.rs +2 −1")
    )));
    let completed = mapper
        .handle(
            parse_line(
                r#"{"type":"result","subtype":"success","terminal_reason":"completed","usage":{}}"#,
            )
            .expect("completion parses"),
        )
        .expect("completion maps");
    assert!(matches!(
        completed.events.last(),
        Some(AgentEvent::TurnCompleted { files_changed, usage, .. })
            if files_changed.len() == 1
                && files_changed[0].added == 2
                && files_changed[0].removed == 1
                && usage.tool_uses == 1
    ));
}

/// BH2: a `Write` has no `structuredPatch`, so the synthetic diff is what the row renders.
#[test]
fn a_synthetic_write_diff_carries_a_hunk_header_a_parser_accepts() {
    let mut mapper = ClaudeMapper::default();
    mapper.begin_turn(TurnId::new()).expect("turn");
    mapper
        .handle(
            parse_line(
                r#"{"type":"assistant","message":{"id":"msg_w","content":[{"type":"tool_use","id":"toolu_w","name":"Write","input":{"file_path":"/private/tmp/fixture.txt","content":"ok"}}]}}"#,
            )
            .expect("tool parses"),
        )
        .expect("tool starts");
    let output = mapper
        .handle(
            parse_line(
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_w","content":"written","is_error":false}]},"tool_use_result":{"filePath":"/private/tmp/fixture.txt"}}"#,
            )
            .expect("result parses"),
        )
        .expect("result maps");
    let unified = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ItemUpdated { patch, .. } => {
                patch.diff.as_ref().map(|diff| diff.unified.clone())
            }
            _ => None,
        })
        .expect("the write produced a diff");

    // `fleet_lazygit::diff_view::hunk_header` and `fleet_git::parse::diff` both need ranges: a
    // bare `@@` parses as a file with zero hunks and renders an empty inline diff (BH2).
    let header = unified
        .lines()
        .find(|line| line.starts_with("@@"))
        .expect("the diff has a hunk header");
    assert_eq!(header, "@@ -0,0 +1,1 @@");
    // `a//abs/path` is not a path any of those readers strips back to something meaningful.
    assert!(!unified.contains("a//"), "{unified}");
    assert!(
        unified.starts_with("--- a/private/tmp/fixture.txt\n"),
        "{unified}"
    );
}

/// BH3/C5: an unrecognised frame is a tracing diagnostic, never a transcript notice.
#[test]
fn unrecognised_claude_frames_never_reach_the_transcript() {
    let mut mapper = ClaudeMapper::default();
    mapper.begin_turn(TurnId::new()).expect("turn");
    for line in [
        r#"{"type":"system","subtype":"a_subtype_from_the_future"}"#,
        r#"{"type":"system","subtype":"session_state_changed","state":"levitating"}"#,
        r#"{"type":"assistant","message":{"id":"msg_x","content":[{"type":"holograph"}]}}"#,
        r#"{"type":"stream_event","event":{"type":"teleport"}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_missing","content":"x"}]}}"#,
    ] {
        let output = mapper
            .handle(parse_line(line).expect("frame parses"))
            .expect("frame maps");
        assert!(
            !output
                .events
                .iter()
                .any(|event| matches!(event, AgentEvent::Notice(_))),
            "{line} produced a notice"
        );
    }
}

/// BH4: the CLI's notice copy names a keystroke Fleet does not implement.
#[test]
fn a_notice_drops_the_clis_own_terminal_affordance() {
    let mut mapper = ClaudeMapper::default();
    let output = mapper
        .handle(
            parse_line(
                r#"{"type":"system","subtype":"notification","text":"Stop hook error occurred · ctrl+o to see"}"#,
            )
            .expect("notification parses"),
        )
        .expect("notification maps");
    assert_eq!(
        output.events,
        vec![AgentEvent::Notice("Stop hook error occurred".to_owned())]
    );

    // A notice that is *only* an affordance has nothing left to say.
    let empty = mapper
        .handle(
            parse_line(r#"{"type":"system","subtype":"notification","text":"ctrl+o to see"}"#)
                .expect("notification parses"),
        )
        .expect("notification maps");
    assert!(empty.events.is_empty());
}

/// BH5: §2 gives every tool row one line, and §4.1 makes the request itself the gate.
#[test]
fn gate_shaped_tools_summarise_in_one_line_instead_of_dumping_their_payload() {
    let mut mapper = ClaudeMapper::default();
    mapper.begin_turn(TurnId::new()).expect("turn");
    let output = mapper
        .handle(
            parse_line(
                r#"{"type":"assistant","message":{"id":"msg_q","content":[{"type":"tool_use","id":"toolu_q","name":"AskUserQuestion","input":{"questions":[{"header":"Pick a database","question":"Which one?","options":[{"label":"sqlite"},{"label":"postgres"}]}]}}]}}"#,
            )
            .expect("tool parses"),
        )
        .expect("tool maps");
    let summary = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ItemUpdated { patch, .. } => patch.summary.clone(),
            _ => None,
        })
        .expect("the tool opened a row");
    assert_eq!(summary, "Pick a database");
    assert!(!summary.contains('{'), "the payload leaked into the row");
}

/// C8: 2.1.263 may omit `input.plan`; the card is unreadable if the mapper gives up there.
#[test]
fn exit_plan_mode_recovers_the_plan_from_the_tool_use_block() {
    let mut mapper = ClaudeMapper::default();
    mapper.begin_turn(TurnId::new()).expect("turn");
    mapper
        .handle(
            parse_line(
                r#"{"type":"assistant","message":{"id":"msg_p","content":[{"type":"tool_use","id":"toolu_plan","name":"ExitPlanMode","input":{"plan":"Steps\n- one\n- two"}}]}}"#,
            )
            .expect("tool parses"),
        )
        .expect("tool maps");
    let (_, kind) = open_gate(
        &mut mapper,
        json!({
            "type": "control_request",
            "request_id": "req_plan",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "ExitPlanMode",
                "tool_use_id": "toolu_plan",
                "input": {},
            },
        }),
    );
    let GateKind::Plan { markdown, steps } = kind else {
        panic!("expected a plan gate");
    };
    assert!(markdown.contains("- one"), "{markdown}");
    assert_eq!(steps.len(), 2);
}

/// C4: `modelUsage` is cumulative across the process and carries pipeline subcalls.
#[test]
fn context_percentage_measures_the_session_model_not_the_first_key() {
    let mut mapper = ClaudeMapper::default();
    mapper
        .handle(
            parse_line(
                r#"{"type":"system","subtype":"init","model":"claude-sonnet-5","tools":[],"slash_commands":[]}"#,
            )
            .expect("init parses"),
        )
        .expect("init maps");
    mapper.begin_turn(TurnId::new()).expect("turn");
    let output = mapper
        .handle(
            parse_line(
                r#"{"type":"result","subtype":"success","terminal_reason":"completed","usage":{"input_tokens":20000},"modelUsage":{"claude-haiku-4":{"inputTokens":40000,"contextWindow":50000},"claude-sonnet-5":{"inputTokens":20000,"contextWindow":200000}}}"#,
            )
            .expect("result parses"),
        )
        .expect("result maps");
    let context = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::TokenUsage { context_pct, .. } => Some(*context_pct),
            _ => None,
        })
        .expect("the result reported usage");
    // The numerator is this turn's own occupancy (BH1); only the *window* comes from
    // `modelUsage`, and the haiku subcall sorts first, so its 50k window would have said 40%.
    assert!((context - 10.0).abs() < 0.01, "{context}");
}

/// BH1: a killed child leaves the mapper holding a turn nothing can ever answer.
#[test]
fn an_unexpected_exit_releases_the_turn_so_the_thread_can_be_resumed() {
    let mut mapper = ClaudeMapper::default();
    let turn = TurnId::new();
    mapper.begin_turn(turn).expect("turn starts");
    assert_eq!(mapper.active_turn(), Some(turn));
    let events = mapper.process_exit(Some(137), false);
    assert!(matches!(
        events.first(),
        Some(AgentEvent::SessionExited {
            code: Some(137),
            expected: false
        })
    ));
    assert_eq!(mapper.active_turn(), None);
    // A fresh send must not be rejected with "turn … is active" by a mapper whose child is gone.
    assert!(
        mapper
            .begin_turn(TurnId::new())
            .expect("a new turn starts")
            .is_some()
    );
}

#[test]
fn background_task_list_never_closes_a_foreground_subagent() {
    let mut mapper = ClaudeMapper::default();
    mapper.begin_turn(TurnId::new()).expect("turn");
    let started = mapper
        .handle(
            parse_line(
                r#"{"type":"system","subtype":"task_started","task_id":"task_fg","description":"find callers","subagent_type":"explore"}"#,
            )
            .expect("task_started parses"),
        )
        .expect("task_started maps");
    let AgentEvent::ItemStarted {
        item: foreground, ..
    } = started.events.first().expect("subagent row")
    else {
        panic!("expected a subagent row");
    };
    let replaced = mapper
        .handle(
            parse_line(
                r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"task_bg","task_type":"bash","description":"serve"}]}"#,
            )
            .expect("background list parses"),
        )
        .expect("background list maps");
    let AgentEvent::ItemStarted {
        item: background, ..
    } = replaced.events.first().expect("background row")
    else {
        panic!("expected a background row");
    };
    assert!(!replaced.events.iter().any(
        |event| matches!(event, AgentEvent::ItemCompleted { item, .. } if item == foreground)
    ));
    let emptied = mapper
        .handle(
            parse_line(r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#)
                .expect("empty background list parses"),
        )
        .expect("empty background list maps");
    assert_eq!(
        emptied.events,
        [AgentEvent::ItemCompleted {
            item: *background,
            status: fleet_core::agents::ItemStatus::Done
        }]
    );
}

#[tokio::test]
async fn fake_process_uses_production_reader_mapper_and_writer() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().expect("temporary fake Claude directory");
    let script = directory.path().join("fake-claude");
    let capture = directory.path().join("stdin.ndjson");
    std::fs::write(
        &script,
        r#"#!/bin/sh
for argument in "$@"; do
  if [ "$argument" = "--version" ]; then
    echo "2.1.263 (Claude Code)"
    exit 0
  fi
done
fixture="$1"
capture="$2"
IFS= read -r line || exit 1
printf '%s\n' "$line" > "$capture"
cat "$fixture"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$capture"
done
"#,
    )
    .expect("write fake Claude");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
        .expect("make fake Claude executable");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agents/claude/claude-basic.ndjson");
    let command = format!(
        "{} {} {}",
        shell_words::quote(&script.to_string_lossy()),
        shell_words::quote(&fixture.to_string_lossy()),
        shell_words::quote(&capture.to_string_lossy())
    );
    let mut provider = ClaudeProvider::new(command);
    let mut events = provider.events();
    provider
        .start(StartRequest {
            thread: ThreadId::new(),
            worktree_path: directory.path().to_path_buf(),
            provider: AgentKind::Claude,
            model: None,
            mode: PermissionMode::Ask,
            resume_cursor: None,
            title: None,
        })
        .await
        .expect("start fake Claude");
    let turn = TurnId::new();
    provider
        .send(
            turn,
            UserInput {
                text: "hello from fake process".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("write prompt");
    let mut saw_completion = false;
    for _ in 0..32 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .expect("fake Claude event deadline")
            .expect("fake Claude event channel");
        if matches!(event.event, AgentEvent::TurnCompleted { turn: completed, .. } if completed == turn)
        {
            saw_completion = true;
            break;
        }
    }
    assert!(saw_completion);
    let captured = std::fs::read_to_string(&capture).expect("captured stdin");
    let first: Value =
        serde_json::from_str(captured.lines().next().expect("prompt line")).expect("prompt JSON");
    assert_eq!(first["message"]["content"], "hello from fake process");
    provider.stop().await.expect("stop fake Claude");
}

/// BH-2: §3.2 makes `payload` "the invocation itself" — for a write that is the path *and* the
/// bytes, or the card asks the user to allow a change it never showed them.
#[test]
fn a_write_gate_shows_the_change_and_not_only_its_destination() {
    let mut mapper = ClaudeMapper::default();
    let (_, kind) = open_gate(
        &mut mapper,
        json!({
            "type": "control_request",
            "request_id": "perm-write",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Write",
                "tool_use_id": "toolu_w",
                "input": {
                    "file_path": "/private/tmp/repo/fixture.txt",
                    "content": "one\ntwo\n"
                }
            }
        }),
    );
    let GateKind::Permission { payload, .. } = kind else {
        panic!("expected a permission gate");
    };
    assert!(
        payload.starts_with("/private/tmp/repo/fixture.txt\n"),
        "{payload}"
    );
    assert!(payload.contains("+one"), "{payload}");
    assert!(payload.contains("+two"), "{payload}");
    // The hunk header and the `--- a/… +++ b/…` pair are for a diff parser, not for a reader.
    assert!(!payload.contains("@@"), "{payload}");

    // An `Edit` states both sides of the replacement.
    let (_, kind) = open_gate(
        &mut mapper,
        json!({
            "type": "control_request",
            "request_id": "perm-edit",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Edit",
                "tool_use_id": "toolu_e",
                "input": {
                    "file_path": "/private/tmp/repo/lib.rs",
                    "old_string": "half_down",
                    "new_string": "half_up"
                }
            }
        }),
    );
    let GateKind::Permission { payload, .. } = kind else {
        panic!("expected a permission gate");
    };
    assert!(payload.contains("-half_down"), "{payload}");
    assert!(payload.contains("+half_up"), "{payload}");
}

/// BH-1: `context N%` is occupancy, not the running sum `modelUsage` accumulates.
#[test]
fn context_percentage_does_not_climb_with_every_turn() {
    let mut mapper = ClaudeMapper::default();
    mapper
        .handle(
            parse_line(
                r#"{"type":"system","subtype":"init","model":"claude-sonnet-5","tools":[],"slash_commands":[]}"#,
            )
            .expect("init parses"),
        )
        .expect("init maps");
    let percent = |mapper: &mut ClaudeMapper, line: &str| {
        mapper.begin_turn(TurnId::new()).expect("turn");
        mapper
            .handle(parse_line(line).expect("result parses"))
            .expect("result maps")
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::TokenUsage { context_pct, .. } => Some(*context_pct),
                _ => None,
            })
            .expect("the result reported usage")
    };
    // Two turns of a conversation that never exceeds 20k of a 200k window: `modelUsage` sums
    // to 30k across the process, but the second turn is still holding 20k.
    let first = percent(
        &mut mapper,
        r#"{"type":"result","subtype":"success","terminal_reason":"completed","usage":{"input_tokens":10000},"modelUsage":{"claude-sonnet-5":{"inputTokens":10000,"contextWindow":200000}}}"#,
    );
    let second = percent(
        &mut mapper,
        r#"{"type":"result","subtype":"success","terminal_reason":"completed","usage":{"input_tokens":8000,"cache_read_input_tokens":12000},"modelUsage":{"claude-sonnet-5":{"inputTokens":30000,"contextWindow":200000}}}"#,
    );
    assert!((first - 5.0).abs() < 0.01, "{first}");
    assert!((second - 10.0).abs() < 0.01, "{second}");
}
