//! Codex adapter tests: the captured wire trace, the byte-exact outbound goldens, the three
//! privacy tests, and a mock peer for the process-level behaviour.

use crate::agents::golden::{canonical, canonical_text};
use std::time::Duration;

use fleet_core::agents::{
    AgentEvent, AgentKind, ApprovalPolicy, GateAnswer, ItemId, ItemStatus, ModelSelection,
    PermissionChoice, PermissionMode, SandboxPolicy, StartRequest, ThreadId, TurnId, TurnOutcome,
    UserInput,
};
use semver::Version;
use serde_json::{Value, json};

use super::{
    CodexHarness, approvals, envelope, map, methods, params::user_input, session::CodexSession,
    user_agent_version,
};
use crate::agents::harness::{
    Harness, HarnessConfig, OpenSession, Submit, SubmitIntent,
    capture::{SECRETS, assert_no_secret, logged},
    mockpeer::MockPeer,
    probe::Probed,
};

/// The real capture, taken from `codex-cli 0.147.0` driving one ordinary turn.
const TURN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agents/codex/codex-turn.ndjson"
));

/// The real capture of a turn that failed upstream: `idle` → `error` → `turn/completed{failed}`.
const FAILED_TURN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agents/codex/codex-failed-turn.ndjson"
));

fn start_request() -> StartRequest {
    StartRequest {
        thread: ThreadId::new(),
        worktree_path: std::env::temp_dir(),
        provider: AgentKind::Codex,
        model: Some(ModelSelection {
            model: "gpt-5.1-codex".to_owned(),
            effort: Some("medium".to_owned()),
            provider: None,
        }),
        mode: PermissionMode::Ask,
        resume_cursor: None,
        fork: false,
        env: std::collections::BTreeMap::new(),
        sandbox: SandboxPolicy::ReadOnly,
        approval_policy: ApprovalPolicy::Untrusted,
        permission_profile: None,
        title: None,
    }
}

fn harness(command: String) -> CodexHarness {
    CodexHarness::new(
        HarnessConfig {
            command,
            client_version: "0.1.0".to_owned(),
            ..HarnessConfig::default()
        },
        Probed {
            kind: AgentKind::Codex,
            version: Version::new(0, 147, 0),
            reported: "codex-cli 0.147.0".to_owned(),
        },
    )
}

/// Replays one captured stream through the mapper, returning the events in order.
fn replay(fixture: &str) -> Vec<AgentEvent> {
    let mut session = CodexSession::default();
    let mut events = Vec::new();
    let mut submitted: Option<TurnId> = None;
    for line in fixture.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let inbound = value.get("_dir").and_then(Value::as_str) == Some("in");
        if !inbound {
            // Fleet's own outbound frame: a `turn/start` is where the caller's turn id is minted.
            if value.get("method").and_then(Value::as_str) == Some("turn/start") {
                submitted = Some(TurnId::new());
            }
            continue;
        }
        // Only the `turn/start` response is consumed here: `open` adopts the thread from its
        // own response, and letting `thread/started` do it in the replay is what exercises the
        // notification path as well.
        if value.pointer("/result/thread/id").is_some() {
            continue;
        }
        if let Some(turn) = value.pointer("/result/turn/id").and_then(Value::as_str) {
            let minted = submitted.unwrap_or_default();
            session.alias_turn(turn, minted);
            session.begin_turn(minted);
            session.adopt_turn(minted, turn);
            continue;
        }
        let Some(envelope::Inbound::Notification { method, params, .. }) =
            envelope::classify(value)
        else {
            continue;
        };
        // The capture was taken without the suppression capability, so the replay applies the
        // same filter the real connection asks the server for.
        if methods::OPT_OUT_NOTIFICATION_METHODS.contains(&method.as_str()) {
            continue;
        }
        events.extend(map::handle(&mut session, &method, &params).events);
    }
    events
}

fn names(events: &[AgentEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            AgentEvent::SessionConfigured { .. } => "session_configured",
            AgentEvent::MetadataChanged { .. } => "metadata_changed",
            AgentEvent::SessionStateChanged(_) => "session_state_changed",
            AgentEvent::SessionActivity { .. } => "session_activity",
            AgentEvent::SessionExited { .. } => "session_exited",
            AgentEvent::TurnStarted { .. } => "turn_started",
            AgentEvent::TurnSettled { .. } => "turn_settled",
            AgentEvent::TurnAborted { .. } => "turn_aborted",
            AgentEvent::TurnDiff { .. } => "turn_diff",
            AgentEvent::PlanSteps { .. } => "plan_steps",
            AgentEvent::ItemStarted { .. } => "item_started",
            AgentEvent::ContentDelta { .. } => "content_delta",
            AgentEvent::ItemUpdated { .. } => "item_updated",
            AgentEvent::ItemCompleted { .. } => "item_completed",
            AgentEvent::GateOpened { .. } => "gate_opened",
            AgentEvent::GateResolved { .. } => "gate_resolved",
            AgentEvent::GateWithdrawn { .. } => "gate_withdrawn",
            AgentEvent::PlanProposed { .. } => "plan_proposed",
            AgentEvent::TokenUsage { .. } => "token_usage",
            AgentEvent::RateLimits { .. } => "rate_limits",
            AgentEvent::Compacted(_) => "compacted",
            AgentEvent::Retrying { .. } => "retrying",
            AgentEvent::ModelRerouted { .. } => "model_rerouted",
            AgentEvent::RuntimeError { .. } => "runtime_error",
            AgentEvent::Notice(_) => "notice",
            AgentEvent::Unknown { .. } => "unknown",
        })
        .collect()
}

/// The captured turn, mapped. The transcript is built from `item/*` and nothing else.
#[test]
fn the_captured_turn_maps_to_the_transcript_the_items_describe() {
    let events = replay(TURN);
    let mapped = names(&events);
    assert_eq!(
        mapped.first().copied(),
        Some("session_configured"),
        "{mapped:?}"
    );
    assert_eq!(mapped.last().copied(), Some("turn_settled"), "{mapped:?}");
    // Four items: the echoed user message, two assistant messages and one command execution.
    let started = mapped
        .iter()
        .filter(|name| **name == "item_started")
        .count();
    assert_eq!(started, 4, "{mapped:?}");
    let deltas = mapped
        .iter()
        .filter(|name| **name == "content_delta")
        .count();
    assert_eq!(deltas, 14, "every streamed token reaches the transcript");
    // `turn/completed` reported ONE item for this turn; the transcript must not be rebuilt
    // from it.
    let AgentEvent::TurnSettled { outcome, .. } =
        events.last().unwrap_or_else(|| panic!("a settlement"))
    else {
        panic!("the turn must settle");
    };
    assert_eq!(outcome, &TurnOutcome::Completed);

    // The command row's kind column comes from `commandActions`, so it is a Read and not a shell
    // wrapper.
    let tool = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ItemStarted {
                kind: fleet_core::agents::ItemKind::Tool(tool),
                ..
            } => Some(tool.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the capture ran one command"));
    assert_eq!(tool.kind, fleet_core::agents::ToolKind::Read);
    assert_eq!(tool.summary.as_deref(), Some("note.txt"));
    assert!(
        tool.input
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| command.contains("bash -lc")),
        "the raw command is kept as input, and only as input"
    );
}

/// The captured failure: `error` is **not** terminal, and `turn/completed` is.
#[test]
fn the_captured_failure_keeps_the_error_and_the_settlement_apart() {
    let events = replay(FAILED_TURN);
    let mapped = names(&events);
    let error = mapped.iter().position(|name| *name == "runtime_error");
    let settled = mapped
        .iter()
        .position(|name| *name == "turn_settled")
        .unwrap_or_else(|| panic!("the turn must settle: {mapped:?}"));
    assert!(
        error.is_some_and(|error| error < settled),
        "the error arrives first and settles nothing: {mapped:?}"
    );
    let AgentEvent::TurnSettled { outcome, .. } = &events[settled] else {
        panic!("expected a settlement");
    };
    let TurnOutcome::Error { message } = outcome else {
        panic!("the captured turn failed: {outcome:?}");
    };
    // `serverOverloaded` earns its own actionable sentence rather than the raw string.
    assert_eq!(
        message.as_deref(),
        Some("The selected model is at capacity — try another model.")
    );
}

/// Byte-exact goldens for every outbound frame Fleet writes.
mod wire;
/// The mock peer: a real child process, scripted from the captured responses.
fn peer() -> crate::agents::harness::mockpeer::BuiltPeer {
    let thread = "01a089f2-5337-7470-adb1-219e71d62a35";
    let turn = "01a089f2-579b-75c2-8e51-3492ec617046";
    MockPeer::new()
        .on(
            "initialize",
            Some(
                r#"{"id":__ID__,"result":{"userAgent":"fleet/0.147.0 (Linux)","codexHome":"/home/df/.codex","platformFamily":"unix","platformOs":"linux"}}"#,
            ),
            &[],
        )
        .on(
            "thread/start",
            Some(&format!(
                r#"{{"id":__ID__,"result":{{"thread":{{"id":"{thread}","model":"gpt-5.1-codex"}}}}}}"#
            )),
            &[&format!(
                r#"{{"method":"thread/status/changed","params":{{"threadId":"{thread}","status":{{"type":"idle"}}}},"emittedAtMs":1}}"#
            )],
        )
        .on(
            "turn/start",
            Some(&format!(
                r#"{{"id":__ID__,"result":{{"turn":{{"id":"{turn}","items":[],"itemsView":"notLoaded","status":"inProgress"}}}}}}"#
            )),
            &[
                &format!(
                    r#"{{"method":"item/started","params":{{"threadId":"{thread}","turnId":"{turn}","startedAtMs":1,"item":{{"type":"agentMessage","id":"msg_1","text":""}}}},"emittedAtMs":2}}"#
                ),
                &format!(
                    r#"{{"method":"item/agentMessage/delta","params":{{"threadId":"{thread}","turnId":"{turn}","itemId":"msg_1","delta":"hi"}},"emittedAtMs":3}}"#
                ),
                &format!(
                    r#"{{"method":"item/completed","params":{{"threadId":"{thread}","turnId":"{turn}","completedAtMs":4,"item":{{"type":"agentMessage","id":"msg_1","text":"hi"}}}},"emittedAtMs":4}}"#
                ),
                &format!(
                    r#"{{"method":"turn/completed","params":{{"threadId":"{thread}","turn":{{"id":"{turn}","items":[],"itemsView":"summary","status":"completed","durationMs":12}}}},"emittedAtMs":5}}"#
                ),
            ],
        )
        .lingering()
        .build()
}

/// Open, submit and settle against a spawned mock peer, with no Codex on the machine.
#[tokio::test]
async fn a_scripted_peer_drives_the_handshake_a_turn_and_its_settlement() {
    let peer = peer();
    let mut harness = harness(peer.command());
    let mut events = harness.events();
    harness
        .open(OpenSession {
            start: start_request(),
        })
        .await
        .unwrap_or_else(|error| panic!("open: {error}"));

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
    assert_eq!(submitted.turn, turn, "the caller's own turn id survives");
    assert!(!submitted.queued);

    // Drain until the settlement, which is the only thing that ends a turn.
    let mut seen = Vec::new();
    let settled = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            let terminal = matches!(event.event, AgentEvent::TurnSettled { .. });
            seen.push(event);
            if terminal {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or_else(|_| panic!("the settlement never arrived: {seen:#?}"));
    assert!(settled);
    let mapped = names(
        &seen
            .iter()
            .map(|event| event.event.clone())
            .collect::<Vec<_>>(),
    );
    assert!(mapped.contains(&"turn_started"), "{mapped:?}");
    assert!(mapped.contains(&"content_delta"), "{mapped:?}");
    assert_eq!(mapped.last().copied(), Some("turn_settled"), "{mapped:?}");
    // Codex stamps every notification with its own clock, and Fleet keeps it.
    assert!(
        seen.iter().any(|event| event.emitted_at.is_some()),
        "emittedAtMs must survive the mapping"
    );

    // The frames Fleet wrote, in order, byte-for-byte from the peer's own record.
    let written = peer.wait_for_frames(4, Duration::from_secs(5)).await;
    let written_methods = written
        .iter()
        .filter_map(|line| {
            serde_json::from_str::<Value>(line).ok().and_then(|value| {
                value
                    .get("method")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        written_methods,
        ["initialize", "initialized", "thread/start", "turn/start"],
        "{written:?}"
    );
    assert!(
        !written.iter().any(|line| line.contains("jsonrpc")),
        "there is no jsonrpc field in either direction: {written:?}"
    );
    harness
        .shutdown(crate::agents::harness::ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
}

/// A stray line on stdout is counted and never kills the session.
#[tokio::test]
async fn a_stray_stdout_line_never_kills_the_session() {
    let thread = "01a089f2-5337-7470-adb1-219e71d62a35";
    let peer = MockPeer::new()
        // Something else on the machine wrote to this stdout. It is not an envelope.
        .greeting(&["fleetd: starting codex", "{\"not\":\"an envelope\"}"])
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
        .lingering()
        .build();
    let mut harness = harness(peer.command());
    harness
        .open(OpenSession {
            start: start_request(),
        })
        .await
        .unwrap_or_else(|error| panic!("a stray line must not stop the handshake: {error}"));
    assert!(
        harness.unroutable_lines() >= 2,
        "both stray lines are counted"
    );
    harness
        .shutdown(crate::agents::harness::ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
}

/// A second message while a turn runs is a **steer**, decided by the adapter and not the caller.
#[tokio::test]
async fn a_second_submit_steers_the_running_turn() {
    let thread = "01a089f2-5337-7470-adb1-219e71d62a35";
    let turn = "01a089f2-579b-75c2-8e51-3492ec617046";
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
            "turn/start",
            Some(&format!(
                r#"{{"id":__ID__,"result":{{"turn":{{"id":"{turn}","items":[],"status":"inProgress"}}}}}}"#
            )),
            &[],
        )
        .on("turn/steer", Some(r#"{"id":__ID__,"result":{}}"#), &[])
        .lingering()
        .build();
    let mut harness = harness(peer.command());
    harness
        .open(OpenSession {
            start: start_request(),
        })
        .await
        .unwrap_or_else(|error| panic!("open: {error}"));
    let first = TurnId::new();
    let message = |text: &str| UserInput {
        text: text.to_owned(),
        attachments: Vec::new(),
        item: None,
    };
    let started = harness
        .submit(Submit {
            turn: first,
            input: message("start"),
            intent: SubmitIntent::Fresh,
        })
        .await
        .unwrap_or_else(|error| panic!("submit: {error}"));
    assert!(!started.queued);
    // The caller says `Fresh` again — it has no business knowing the wire form — and the adapter
    // steers, because a turn is running.
    let steered = harness
        .submit(Submit {
            turn: TurnId::new(),
            input: message("actually, do this"),
            intent: SubmitIntent::Fresh,
        })
        .await
        .unwrap_or_else(|error| panic!("steer: {error}"));
    assert!(steered.queued, "a steer joins the running turn");
    assert_eq!(
        steered.turn, first,
        "and it lands in that turn, not a new one"
    );
    let written = peer.wait_for_frames(4, Duration::from_secs(5)).await;
    let steer = written
        .iter()
        .find(|line| line.contains("turn/steer"))
        .unwrap_or_else(|| panic!("a steer was written: {written:?}"));
    // `expectedTurnId` is a compare-and-swap against the turn that is active now.
    assert!(
        steer.contains(&format!(r#""expectedTurnId":"{turn}""#)),
        "{steer}"
    );
    harness
        .shutdown(crate::agents::harness::ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
}

/// Stop must not hang on a child that never answers, and a child that ignores SIGTERM is killed.
#[tokio::test]
async fn stop_finishes_against_a_wedged_child() {
    let peer = MockPeer::new().ignoring_sigterm().build();
    let mut harness = harness(peer.command());
    let mut events = harness.events();
    // The handshake cannot complete against a peer that answers nothing, and that is a handshake
    // failure with the terminal fallback rather than a hang.
    let error = tokio::time::timeout(
        Duration::from_secs(20),
        harness.open(OpenSession {
            start: start_request(),
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("open must not hang forever"))
    .err()
    .unwrap_or_else(|| panic!("a peer that answers nothing cannot hand back a session"));
    assert!(
        error.to_string().contains("handshake") || error.to_string().contains("timed out"),
        "{error}"
    );
    // Teardown still completes, and the child is killed rather than left behind.
    tokio::time::timeout(
        Duration::from_secs(20),
        harness.shutdown(crate::agents::harness::ShutdownReason::User),
    )
    .await
    .unwrap_or_else(|_| panic!("shutdown must not hang"))
    .unwrap_or_else(|error| panic!("shutdown: {error}"));
    drop(events.try_recv());
}

/// The suppression list only names methods this Codex build actually publishes.
#[test]
fn every_suppressed_method_is_a_real_notification() {
    for method in methods::OPT_OUT_NOTIFICATION_METHODS {
        assert!(
            methods::SERVER_NOTIFICATION_METHODS.contains(&method),
            "{method} is not a Codex notification, so suppressing it is a rejected capability"
        );
    }
    for method in methods::MAPPED_NOTIFICATION_METHODS {
        assert!(
            methods::SERVER_NOTIFICATION_METHODS.contains(&method),
            "{method} is mapped but is not a Codex notification"
        );
        assert!(
            !methods::OPT_OUT_NOTIFICATION_METHODS.contains(&method),
            "{method} is both mapped and suppressed"
        );
    }
    for method in methods::IMPLEMENTED_SERVER_REQUESTS {
        assert!(
            methods::SERVER_REQUEST_METHODS.contains(&method),
            "{method} is answered but is not a Codex server request"
        );
    }
}

/// A settlement for a turn Fleet is not running is dropped, and one for the running turn lands.
#[test]
fn a_stale_settlement_never_ends_the_running_turn() {
    let mut session = CodexSession {
        root: Some("t".to_owned()),
        ..CodexSession::default()
    };
    let running = "01a089f2-579b-75c2-8e51-3492ec617046";
    let stale = "01a089f2-0000-7000-8000-000000000000";
    let turn = session.turn_for(running);
    session.begin_turn(turn);
    session.adopt_turn(turn, running);
    let completed = |turn_id: &str| {
        json!({
            "threadId": "t",
            "turn": {
                "id": turn_id,
                "items": [],
                "itemsView": "summary",
                "status": "completed",
            },
        })
    };
    assert!(
        map::handle(&mut session, "turn/completed", &completed(stale))
            .events
            .is_empty(),
        "a stale settlement settles nothing"
    );
    assert_eq!(session.active_turn, Some(turn));
    let landed = map::handle(&mut session, "turn/completed", &completed(running));
    assert_eq!(names(&landed.events), ["turn_settled"]);
    assert!(session.active_turn.is_none());
}

/// An idle status never settles a turn, however tempting it is.
#[test]
fn an_idle_status_is_a_hint_and_never_a_settlement() {
    let mut session = CodexSession {
        root: Some("t".to_owned()),
        ..CodexSession::default()
    };
    let turn = session.turn_for("01a089f2-579b-75c2-8e51-3492ec617046");
    session.begin_turn(turn);
    session.adopt_turn(turn, "01a089f2-579b-75c2-8e51-3492ec617046");
    let output = map::handle(
        &mut session,
        "thread/status/changed",
        &json!({"threadId": "t", "status": {"type": "idle"}}),
    );
    assert_eq!(names(&output.events), ["session_state_changed"]);
    assert_eq!(session.active_turn, Some(turn), "the turn is still running");
}

/// A withdrawn gate closes with no answer written.
#[test]
fn a_withdrawn_request_closes_its_gate_and_answers_nothing() {
    let mut session = CodexSession {
        root: Some("t".to_owned()),
        ..CodexSession::default()
    };
    let outcome = approvals::handle(
        &mut session,
        "item/fileChange/requestApproval",
        &json!("req-1"),
        &json!({
            "threadId": "t",
            "turnId": "01a089f2-579b-75c2-8e51-3492ec617046",
            "itemId": "patch-1",
            "startedAtMs": 1,
        }),
    );
    assert!(outcome.immediate.is_none() && outcome.error.is_none());
    assert_eq!(session.gates.len(), 1);
    let output = map::handle(
        &mut session,
        "serverRequest/resolved",
        &json!({"threadId": "t", "requestId": "req-1"}),
    );
    assert_eq!(names(&output.events), ["gate_withdrawn"]);
    assert!(session.gates.is_empty());
    // A second withdrawal is a no-op rather than a second event.
    assert!(
        map::handle(
            &mut session,
            "serverRequest/resolved",
            &json!({"threadId": "t", "requestId": "req-1"}),
        )
        .events
        .is_empty()
    );
}

/// An item Fleet answered with a decision Codex offered writes exactly that decision.
#[test]
fn an_offered_decision_is_written_verbatim() {
    let mut session = CodexSession {
        root: Some("t".to_owned()),
        ..CodexSession::default()
    };
    approvals::handle(
        &mut session,
        "item/commandExecution/requestApproval",
        &json!(11),
        &json!({
            "threadId": "t",
            "turnId": "01a089f2-579b-75c2-8e51-3492ec617046",
            "itemId": "exec-1",
            "startedAtMs": 1,
            "command": "ls",
            "availableDecisions": ["accept", "acceptForSession", "decline", "cancel"],
        }),
    );
    let (gate, pending) = session
        .gates
        .iter()
        .next()
        .map(|(gate, pending)| (*gate, pending.clone()))
        .unwrap_or_else(|| panic!("one gate"));
    assert_eq!(pending.item.as_deref(), Some("exec-1"));
    assert_eq!(pending.thread, "t");
    for (choice, expected) in [
        (PermissionChoice::AllowOnce, "accept"),
        (PermissionChoice::AllowSession, "acceptForSession"),
        (PermissionChoice::Deny, "decline"),
        (PermissionChoice::DenyAndStop, "cancel"),
    ] {
        assert_eq!(
            approvals::answer(
                &pending,
                gate,
                &GateAnswer::Permission {
                    choice,
                    edited_payload: None,
                },
            )
            .unwrap_or_else(|error| panic!("{error}")),
            json!({"decision": expected})
        );
    }
}

/// An item that completes without ever starting still becomes a row.
#[test]
fn a_completed_item_fleet_never_saw_start_still_becomes_a_row() {
    let mut session = CodexSession {
        root: Some("t".to_owned()),
        ..CodexSession::default()
    };
    let turn = session.turn_for("01a089f2-579b-75c2-8e51-3492ec617046");
    session.begin_turn(turn);
    session.adopt_turn(turn, "01a089f2-579b-75c2-8e51-3492ec617046");
    let output = map::handle(
        &mut session,
        "item/completed",
        &json!({
            "threadId": "t",
            "turnId": "01a089f2-579b-75c2-8e51-3492ec617046",
            "completedAtMs": 2,
            "item": {
                "type": "commandExecution",
                "id": "exec-9",
                "command": "/usr/bin/bash -lc ls",
                "commandActions": [],
                "cwd": "/w",
                "status": "declined",
            },
        }),
    );
    assert_eq!(names(&output.events), ["item_started", "item_completed"]);
    // Declined is denied, not failed: a refused tool is not an error.
    assert!(matches!(
        output.events.last(),
        Some(AgentEvent::ItemCompleted {
            status: ItemStatus::Denied,
            ..
        })
    ));
}
