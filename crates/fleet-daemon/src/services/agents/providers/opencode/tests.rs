use fleet_core::agents::{
    AgentEvent, ItemKind, ItemStatus, PermissionMode, StreamKind, TurnId, TurnOutcome, UserInput,
};
use serde_json::{Value, json};

use super::{
    AgentProvider, OpenCodeProvider, ResumeCursor, decode_cursor, encode_cursor, finished,
};
use crate::services::agents::providers::opencode::{
    http::{permission_reply_body, question_reply_body},
    map::Mapper,
    sse::{SseParser, WireEvent},
};

const SESSION: &str = "ses_f84367efaffepzjRB2cvputgcY";

fn wire(kind: &str, properties: Value) -> WireEvent {
    WireEvent {
        id: format!("test-{kind}"),
        kind: kind.to_owned(),
        properties,
    }
}

fn mapper_with_turn() -> (Mapper, TurnId) {
    let turn = TurnId::new();
    let mut mapper = Mapper::new(SESSION.to_owned(), PermissionMode::Ask);
    mapper
        .admit(
            turn,
            UserInput {
                text: "test".to_owned(),
                attachments: Vec::new(),
            },
            "build".to_owned(),
        )
        .expect("admit test turn");
    (mapper, turn)
}

#[test]
fn captured_sse_maps_error_before_authoritative_idle_completion() {
    let (mut mapper, turn) = mapper_with_turn();
    let fixture = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/agents/opencode/opencode-events.ndjson"
    ));
    let mut parser = SseParser::default();
    let mut wire_events = Vec::new();
    let mut diagnostics = Vec::new();
    for chunk in fixture.chunks(17) {
        let batch = parser.push(chunk);
        wire_events.extend(batch.events);
        diagnostics.extend(batch.diagnostics);
    }
    let batch = parser.finish();
    wire_events.extend(batch.events);
    diagnostics.extend(batch.diagnostics);
    assert_eq!(wire_events.len(), 9);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let mut events = Vec::new();
    for wire in wire_events {
        events.extend(mapper.handle(wire).events);
    }
    assert_eq!(
        events.iter().map(event_name).collect::<Vec<_>>(),
        // The title is projected metadata, not a transcript notice, and the second `busy` says
        // nothing new: an unchanged session state costs no sequence (§6).
        vec![
            "session_state_changed",
            "metadata_changed",
            "runtime_error",
            "turn_completed",
            "session_state_changed",
        ]
    );
    let runtime_error = events
        .iter()
        .position(|event| matches!(event, AgentEvent::RuntimeError { fatal: false, .. }))
        .expect("fixture runtime error");
    let completion = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::TurnCompleted {
                    turn: completed,
                    outcome: TurnOutcome::Error { message: Some(message) },
                    ..
                } if *completed == turn && message.contains("Model not found")
            )
        })
        .expect("fixture error completion");
    assert!(runtime_error < completion);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::TurnCompleted { .. }))
            .count(),
        1,
        "session.idle after session.status idle must be idempotent"
    );
}

#[test]
fn one_undecodable_payload_never_costs_the_events_beside_it() {
    let mut parser = SseParser::default();
    let batch = parser.push(
        concat!(
            "data: {\"id\":\"a\",\"type\":\"server.connected\",\"properties\":{}}\n\n",
            "data: {not json}\n\n",
            "data: {\"id\":\"b\",\"type\":\"session.idle\",\"properties\":{}}\n\n",
        )
        .as_bytes(),
    );
    assert_eq!(
        batch
            .events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        ["server.connected", "session.idle"],
        "a malformed payload must not take the idle that settles the turn with it"
    );
    assert_eq!(batch.diagnostics.len(), 1);
}

#[test]
fn retiring_a_provider_ends_the_event_loop_that_outlived_it() {
    let mut provider = OpenCodeProvider::default();
    // The manager takes the receiver, and the spawned loop keeps a clone of the sender.
    let stream = provider.events();
    let loop_sink = provider.events_tx.clone();
    let loop_stopping = std::sync::Arc::clone(&provider.stopping);
    assert!(!finished(&loop_stopping, &loop_sink));

    // §3 leaves no runtime behind a finished session, and a `SessionExited` retires the
    // provider by dropping its slot — `stop()` is never called on that path.
    drop(provider);
    assert!(
        finished(&loop_stopping, &loop_sink),
        "a dropped provider left its event loop reconnecting to a dead port",
    );
    drop(stream);
}

#[test]
fn an_event_that_never_reaches_a_blank_line_cannot_grow_the_parser() {
    let mut parser = SseParser::default();
    // Every line is a well-formed `data:` line, and none of them is ever followed by the blank
    // separator that would flush the event: the byte budget alone never sees them.
    let mut diagnostics = Vec::new();
    let line = format!("data: {}\n", "x".repeat(64 * 1024));
    for _ in 0..192 {
        let batch = parser.push(line.as_bytes());
        assert!(batch.events.is_empty());
        diagnostics.extend(batch.diagnostics);
    }
    assert!(
        !diagnostics.is_empty(),
        "an event with no separator grew the parser without bound",
    );

    // The parser is usable again: the next complete event still decodes.
    let batch = parser.push(
        "\ndata: {\"id\":\"a\",\"type\":\"server.connected\",\"properties\":{}}\n\n".as_bytes(),
    );
    assert_eq!(
        batch
            .events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        ["server.connected"],
    );
}

#[test]
fn a_text_delta_that_beats_its_tool_frame_still_renders_a_tool_row() {
    let (mut mapper, _) = mapper_with_turn();
    // §4.2: the tool frame owns the id's kind. A delta that arrives first may only guess.
    let early = mapper.handle(wire(
        "message.part.delta",
        json!({
            "sessionID": SESSION,
            "partID": "prt_tool",
            "messageID": "msg_1",
            "field": "text",
            "delta": "running",
        }),
    ));
    let AgentEvent::ItemStarted { item: guessed, .. } = early
        .events
        .iter()
        .find(|event| matches!(event, AgentEvent::ItemStarted { .. }))
        .expect("the speculative row")
    else {
        panic!("expected a speculative row");
    };

    let tool = mapper.handle(wire(
        "message.part.updated",
        json!({
            "sessionID": SESSION,
            "part": {
                "id": "prt_tool",
                "messageID": "msg_1",
                "type": "tool",
                "tool": "bash",
                "state": {"status": "running", "input": {"command": "ls"}},
            },
        }),
    ));
    assert!(
        tool.events.iter().any(|event| matches!(
            event,
            AgentEvent::ItemStarted {
                item,
                kind: ItemKind::Tool { .. },
                ..
            } if item == guessed
        )),
        "the tool frame never re-keyed the row a delta had opened as prose",
    );
}

#[test]
fn a_settled_part_takes_no_more_deltas() {
    let (mut mapper, _) = mapper_with_turn();
    mapper.handle(wire(
        "message.part.updated",
        json!({
            "sessionID": SESSION,
            "part": {
                "id": "prt_done",
                "messageID": "msg_1",
                "type": "tool",
                "tool": "bash",
                "state": {"status": "completed", "input": {"command": "ls"}, "output": "ok"},
            },
        }),
    ));
    // §3 rule 3: the row is settled, so a late delta cannot append to it.
    let late = mapper.handle(wire(
        "message.part.delta",
        json!({
            "sessionID": SESSION,
            "partID": "prt_done",
            "messageID": "msg_1",
            "field": "text",
            "delta": "trailing",
        }),
    ));
    assert!(late.events.is_empty(), "{:?}", late.events);
}

#[test]
fn a_user_abort_settles_as_aborted_even_when_no_assistant_error_arrives() {
    let (mut mapper, turn) = mapper_with_turn();
    // A turn cancelled while the provider is in a retry backoff never produces an assistant
    // message, so `MessageAbortedError` never arrives. The local abort is the authority.
    mapper.mark_abort(turn).expect("mark local abort");
    let settled = mapper.handle(wire(
        "session.status",
        json!({"sessionID": SESSION, "status": {"type": "idle"}}),
    ));
    assert!(settled.events.iter().any(|event| matches!(
        event,
        AgentEvent::TurnAborted {
            turn: aborted,
            reason: fleet_core::agents::AbortReason::User,
        } if *aborted == turn
    )));
    assert!(
        !settled
            .events
            .iter()
            .any(|event| matches!(event, AgentEvent::TurnCompleted { .. }))
    );
}

#[test]
fn a_retry_backoff_does_not_append_an_event_per_tick() {
    let (mut mapper, _) = mapper_with_turn();
    let mut events = Vec::new();
    for next in [1_000_u64, 900, 800] {
        events.extend(
            mapper
                .handle(wire(
                    "session.status",
                    json!({
                        "sessionID": SESSION,
                        "status": {"type": "retry", "attempt": 1, "next": next, "message": "overloaded"}
                    }),
                ))
                .events,
        );
    }
    events.extend(
        mapper
            .handle(wire(
                "session.status",
                json!({
                    "sessionID": SESSION,
                    "status": {"type": "retry", "attempt": 2, "next": 700, "message": "overloaded"}
                }),
            ))
            .events,
    );
    assert_eq!(
        events.iter().map(event_name).collect::<Vec<_>>(),
        ["session_state_changed", "other", "other"],
        "only a new attempt re-reports the backoff, and `running` is said once"
    );
}

fn event_name(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::MetadataChanged { .. } => "metadata_changed",
        AgentEvent::SessionStateChanged(_) => "session_state_changed",
        AgentEvent::Notice(_) => "notice",
        AgentEvent::RuntimeError { .. } => "runtime_error",
        AgentEvent::TurnCompleted { .. } => "turn_completed",
        AgentEvent::TurnAborted { .. } => "turn_aborted",
        _ => "other",
    }
}

#[test]
fn cumulative_parts_replace_delta_state_without_duplication() {
    let (mut mapper, _) = mapper_with_turn();
    let delta = mapper.handle(wire(
        "message.part.delta",
        json!({
            "sessionID": SESSION,
            "messageID": "msg-a",
            "partID": "part-a",
            "field": "text",
            "delta": "hel"
        }),
    ));
    assert!(delta.events.iter().any(|event| matches!(
        event,
        AgentEvent::ContentDelta {
            stream: StreamKind::AssistantText,
            delta,
            ..
        } if delta == "hel"
    )));

    let replacement = wire(
        "message.part.updated",
        json!({
            "sessionID": SESSION,
            "part": {
                "id": "part-a",
                "sessionID": SESSION,
                "messageID": "msg-a",
                "type": "text",
                "text": "hello"
            }
        }),
    );
    let first = mapper.handle(replacement.clone());
    let duplicate = mapper.handle(replacement);
    assert_eq!(mapper.part_text("part-a"), Some("hello"));
    assert_eq!(
        first
            .events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ItemUpdated { .. }))
            .count(),
        1
    );
    assert!(duplicate.events.is_empty());
}

#[test]
fn tool_part_upserts_four_states_once() {
    let (mut mapper, _) = mapper_with_turn();
    let states = [
        json!({ "status": "pending", "input": {"command": "echo hi"}, "raw": "" }),
        json!({ "status": "running", "input": {"command": "echo hi"}, "title": "echo hi", "time": {"start": 1} }),
        json!({ "status": "running", "input": {"command": "echo hi"}, "title": "echo hi", "metadata": {"progress": 1}, "time": {"start": 1} }),
        json!({ "status": "completed", "input": {"command": "echo hi"}, "title": "echo hi", "output": "hi", "metadata": {}, "time": {"start": 1, "end": 2} }),
    ];
    let mut events = Vec::new();
    for state in states {
        events.extend(
            mapper
                .handle(wire(
                    "message.part.updated",
                    json!({
                        "sessionID": SESSION,
                        "part": {
                            "id": "part-tool",
                            "sessionID": SESSION,
                            "messageID": "msg-tool",
                            "type": "tool",
                            "callID": "call-tool",
                            "tool": "bash",
                            "state": state
                        }
                    }),
                ))
                .events,
        );
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ItemStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                AgentEvent::ItemCompleted {
                    status: ItemStatus::Done,
                    ..
                }
            ))
            .count(),
        1
    );
}

#[test]
fn step_finish_updates_usage_but_never_completes_turn() {
    let (mut mapper, turn) = mapper_with_turn();
    let mapped = mapper.handle(wire(
        "message.part.updated",
        json!({
            "sessionID": SESSION,
            "part": {
                "id": "part-step",
                "sessionID": SESSION,
                "messageID": "msg-step",
                "type": "step-finish",
                "reason": "tool-calls",
                "cost": 0.25,
                "tokens": {"input": 10, "output": 5, "reasoning": 2, "cache": {"read": 3, "write": 1}}
            }
        }),
    ));
    assert!(mapped.events.iter().any(|event| matches!(
        event,
        AgentEvent::TokenUsage { turn: seen, .. } if *seen == turn
    )));
    assert!(!mapped.events.iter().any(|event| matches!(
        event,
        AgentEvent::TurnCompleted { .. } | AgentEvent::TurnAborted { .. }
    )));
    assert!(mapper.is_running());
}

#[test]
fn permission_and_question_reply_bodies_match_wire_contract() {
    assert_eq!(permission_reply_body("once"), json!({ "reply": "once" }));
    assert_eq!(
        question_reply_body(&[
            vec!["SQLite".to_owned()],
            vec!["A".to_owned(), "B".to_owned()]
        ]),
        json!({ "answers": [["SQLite"], ["A", "B"]] })
    );
}

#[test]
fn local_abort_with_message_aborted_error_settles_as_turn_aborted() {
    let (mut mapper, turn) = mapper_with_turn();
    mapper.mark_abort(turn).expect("mark local abort");
    mapper.handle(wire(
        "message.updated",
        json!({
            "sessionID": SESSION,
            "info": {
                "id": "msg-assistant",
                "sessionID": SESSION,
                "role": "assistant",
                "time": {"created": 1, "completed": 2},
                "error": {"name": "MessageAbortedError", "data": {"message": "aborted"}},
                "cost": 0,
                "tokens": {"input": 1, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}}
            }
        }),
    ));
    let settled = mapper.handle(wire(
        "session.status",
        json!({"sessionID": SESSION, "status": {"type": "idle"}}),
    ));
    assert!(settled.events.iter().any(|event| matches!(
        event,
        AgentEvent::TurnAborted { turn: aborted, .. } if *aborted == turn
    )));
    assert!(
        !settled
            .events
            .iter()
            .any(|event| matches!(event, AgentEvent::TurnCompleted { .. }))
    );
}

/// BH7: §11 gives an interrupted turn its footer and no error card.
#[test]
fn an_abort_never_synthesises_a_runtime_error_the_reducer_would_drop() {
    let (mut mapper, turn) = mapper_with_turn();
    mapper.mark_abort(turn).expect("mark local abort");
    let mapped = mapper.handle(wire(
        "message.updated",
        json!({
            "sessionID": SESSION,
            "info": {
                "id": "msg-assistant",
                "sessionID": SESSION,
                "role": "assistant",
                "time": {"created": 1, "completed": 2},
                "error": {"name": "MessageAbortedError", "data": {"message": "Aborted"}},
                "cost": 0,
                "tokens": {"input": 1, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}}
            }
        }),
    ));
    assert!(
        !mapped
            .events
            .iter()
            .any(|event| matches!(event, AgentEvent::RuntimeError { .. })),
        "{:?}",
        mapped.events
    );
    // A real failure still reports one.
    let (mut mapper, _) = mapper_with_turn();
    let mapped = mapper.handle(wire(
        "session.error",
        json!({"sessionID": SESSION, "error": {"name": "ProviderAuthError", "data": {"message": "no key"}}}),
    ));
    assert!(mapped.events.iter().any(|event| matches!(
        event,
        AgentEvent::RuntimeError { fatal: false, message } if message == "no key"
    )));
}

/// BH8: the first usage frame of a session is all zeros and would print `context 0%`.
#[test]
fn an_empty_usage_frame_costs_no_sequence() {
    let (mut mapper, _) = mapper_with_turn();
    let mapped = mapper.handle(wire(
        "message.updated",
        json!({
            "sessionID": SESSION,
            "info": {
                "id": "msg-assistant",
                "sessionID": SESSION,
                "role": "assistant",
                "time": {"created": 1},
                "cost": 0,
                "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}}
            }
        }),
    ));
    assert!(
        !mapped
            .events
            .iter()
            .any(|event| matches!(event, AgentEvent::TokenUsage { .. })),
        "{:?}",
        mapped.events
    );
}

/// BH6: `New session - <timestamp>` is the server's placeholder, not a title §10 can use.
#[test]
fn the_auto_generated_session_title_never_replaces_the_first_message_title() {
    let (mut mapper, _) = mapper_with_turn();
    let placeholder = mapper.handle(wire(
        "session.updated",
        json!({"info": {"id": SESSION, "title": "New session - 2026-09-07T19:31:32.360Z"}}),
    ));
    assert!(placeholder.events.is_empty(), "{:?}", placeholder.events);

    let renamed = mapper.handle(wire(
        "session.updated",
        json!({"info": {"id": SESSION, "title": "Fix the diff header"}}),
    ));
    assert!(renamed.events.iter().any(|event| matches!(
        event,
        AgentEvent::MetadataChanged { title: Some(title), .. } if title == "Fix the diff header"
    )));
}

/// C6: §2's turn footer is `… 2 files changed +36 −3`, not `0 files changed`.
#[test]
fn patch_parts_accumulate_into_the_turns_file_counts() {
    let (mut mapper, turn) = mapper_with_turn();
    mapper.handle(wire(
        "message.part.updated",
        json!({
            "sessionID": SESSION,
            "part": {
                "id": "prt-tool",
                "messageID": "msg-assistant",
                "sessionID": SESSION,
                "type": "tool",
                "callID": "call-1",
                "tool": "edit",
                "state": {"status": "completed", "input": {"filePath": "src/lib.rs"}, "title": "src/lib.rs"}
            }
        }),
    ));
    mapper.handle(wire(
        "message.part.updated",
        json!({
            "sessionID": SESSION,
            "part": {
                "id": "prt-patch",
                "messageID": "msg-assistant",
                "sessionID": SESSION,
                "type": "patch",
                "files": ["src/lib.rs"],
                "unified": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,2 @@\n one\n+two\n"
            }
        }),
    ));
    let settled = mapper.handle(wire(
        "session.status",
        json!({"sessionID": SESSION, "status": {"type": "idle"}}),
    ));
    let files = settled
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::TurnCompleted {
                turn: completed,
                files_changed,
                ..
            } if *completed == turn => Some(files_changed.clone()),
            _ => None,
        })
        .expect("the turn completed");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].added, 1);
    assert_eq!(files[0].removed, 0);
}

/// C7: a turn has to be admitted before the request that produces its events.
#[test]
fn a_withdrawn_admission_releases_the_turn_it_announced() {
    let (mut mapper, turn) = mapper_with_turn();
    assert_eq!(mapper.active_turn(), Some(turn));
    assert!(mapper.withdraw(turn));
    assert_eq!(mapper.active_turn(), None);
    // A steer that joined someone else's turn never takes that turn away.
    let (mut mapper, turn) = mapper_with_turn();
    assert!(!mapper.withdraw(TurnId::new()));
    assert_eq!(mapper.active_turn(), Some(turn));
}

#[test]
fn cursor_round_trips_all_routing_coordinates() {
    let cursor = ResumeCursor {
        base_url: "http://127.0.0.1:4597".to_owned(),
        directory: "/tmp/worktree".to_owned(),
        session_id: "ses_test".to_owned(),
    };
    let encoded = encode_cursor(&cursor).expect("encode cursor");
    assert_eq!(decode_cursor(&encoded).expect("decode cursor"), cursor);
}

/// R1: every `*.replied` / `*.rejected` frame spells the request id `requestID`, not `id`
/// (harness-protocols.md:644-651). Reading `id` alone never matched, so a permission answered
/// in the OpenCode TUI kept its card pinned at the top of `NeedsYou` here forever.
#[test]
fn a_gate_answered_elsewhere_is_closed_by_its_reply_event() {
    for (asked, replied) in [
        ("permission.asked", "permission.replied"),
        ("permission.v2.asked", "permission.v2.replied"),
        ("question.asked", "question.replied"),
        ("question.v2.asked", "question.v2.rejected"),
    ] {
        let (mut mapper, _) = mapper_with_turn();
        let request = format!("req-{replied}");
        let opened = mapper.handle(wire(
            asked,
            json!({
                "sessionID": SESSION,
                "id": request,
                "permission": "edit",
                "patterns": ["src/lib.rs"],
                "questions": [{"header":"scope","question":"which?","options":["a","b"]}],
            }),
        ));
        let gate = opened
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::GateOpened { gate, .. } => Some(*gate),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{asked} opened no gate"));

        let closed = mapper.handle(wire(
            replied,
            json!({ "sessionID": SESSION, "requestID": request }),
        ));
        assert!(
            matches!(
                closed.events.as_slice(),
                [AgentEvent::GateResolved {
                    gate: resolved,
                    by: fleet_core::agents::GateResolver::ProviderClosed,
                    ..
                }] if *resolved == gate
            ),
            "{replied} left the card open: {:?}",
            closed.events
        );
        // A second reply for a gate that is already gone costs nothing.
        assert!(
            mapper
                .handle(wire(
                    replied,
                    json!({ "sessionID": SESSION, "requestID": request }),
                ))
                .events
                .is_empty()
        );
    }
}

/// R2: a status snapshot read before a prompt was admitted must not settle that prompt.
///
/// `reconcile_status` reads `GET /session/:id` and only then awaits `reconcile_messages`, so a
/// turn admitted in that window would be settled the moment it started — after which every
/// later part, todo and status frame for the real work is dropped (§3.3 rule 2).
#[test]
fn a_stale_status_snapshot_never_settles_a_turn_admitted_after_it_was_read() {
    let (mut mapper, _) = mapper_with_turn();
    let idle = json!({ "type": "idle" });
    // The snapshot is taken here…
    let observed_at = mapper.admissions();
    // …and a new turn is admitted before it is applied.
    mapper.handle(wire("session.idle", json!({ "sessionID": SESSION })));
    let steered = TurnId::new();
    mapper
        .admit(
            steered,
            UserInput {
                text: "and now this".to_owned(),
                attachments: Vec::new(),
            },
            "build".to_owned(),
        )
        .expect("the steered turn is admitted");

    let stale = mapper.settle_from_status_map(Some(&idle), observed_at);
    assert!(
        stale.events.is_empty(),
        "a status read from before the admission settled the turn it never saw: {:?}",
        stale.events
    );
    assert_eq!(mapper.active_turn(), Some(steered));

    // A snapshot taken after the admission is authoritative for it.
    let observed_at = mapper.admissions();
    let settled = mapper.settle_from_status_map(Some(&idle), observed_at);
    assert!(
        settled.events.iter().any(
            |event| matches!(event, AgentEvent::TurnCompleted { turn, .. } if *turn == steered)
        ),
        "{:?}",
        settled.events
    );
}

/// BH-5: §3.2's `commands` is the slash-command list, and `GET /agent` answers *modes*.
///
/// The live server reported `commands: ["build","plan"]`, so the composer's `/` picker offered
/// two entries that splice a literal `/build` into the prompt and none of the real commands.
#[tokio::test]
async fn the_slash_commands_come_from_the_command_endpoint_not_the_agent_list() {
    use std::{
        path::Path,
        sync::{Arc, Mutex},
    };

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::{http::HttpClient, session_commands};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback listener");
    let addr = listener.local_addr().expect("listener address");
    let requested: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requested);
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("one request");
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            match socket.read(&mut byte).await {
                Ok(0) | Err(_) => break,
                Ok(_) => head.push(byte[0]),
            }
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(head.lines().next().unwrap_or_default().to_owned());
        // `GET /agent` would have answered `build`/`plan`; this is `GET /command`.
        let body = r#"[{"name":"init","description":"seed the repo"},{"name":"review"}]"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ignored = socket.write_all(response.as_bytes()).await;
        let _ignored = socket.shutdown().await;
    });

    let http = HttpClient::new(&format!("http://{addr}"), Path::new("/tmp/repo"), None)
        .expect("build the client");
    let commands = session_commands(&http).await;
    server.await.expect("the fixture server finishes");

    assert_eq!(commands, vec!["init".to_owned(), "review".to_owned()]);
    let requested = requested
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        requested[0].starts_with("GET /command"),
        "the picker was filled from {}",
        requested[0]
    );
}

/// BH9 again, on the other path: `start` reports the mode the server will actually honour, so a
/// later switch must too. A server whose `permission` config allows everything never opens a
/// gate, and a row that answers `asks before edits` for it states a scope that is false.
#[tokio::test]
async fn set_mode_reports_the_mode_the_server_will_actually_honour() {
    use std::path::Path;

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::http::HttpClient;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback listener");
    let addr = listener.local_addr().expect("listener address");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("one request");
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            match socket.read(&mut byte).await {
                Ok(0) | Err(_) => break,
                Ok(_) => head.push(byte[0]),
            }
        }
        // `GET /config`: this server allows every action, so no gate will ever open.
        let body = r#"{"permission":"allow"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ignored = socket.write_all(response.as_bytes()).await;
        let _ignored = socket.shutdown().await;
    });

    let mut provider = OpenCodeProvider::new("opencode");
    provider.http = Some(
        HttpClient::new(&format!("http://{addr}"), Path::new("/tmp/repo"), None)
            .expect("build the client"),
    );
    provider.mode = PermissionMode::FullAccess;

    let reported = provider
        .set_mode(PermissionMode::Ask)
        .await
        .expect("set the mode");

    // Asserted before the fixture is joined: a `set_mode` that never asks the server would
    // otherwise leave this waiting on a connection that is never made.
    assert_eq!(reported, PermissionMode::FullAccess);
    assert_eq!(provider.mode, PermissionMode::FullAccess);
    server.await.expect("the fixture server finishes");
}
