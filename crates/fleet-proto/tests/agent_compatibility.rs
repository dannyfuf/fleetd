//! Byte-exact wire goldens for the whole native-agent protocol family.
//!
//! `rust-ipc-protocol` Rule 9 asks for one golden per wire shape, and the agent family shipped
//! with **zero** — only round-trip assertions, which pass just as happily after a field is
//! renamed or a case is changed on both sides at once. These are the debt payment, and they cover
//! three things the round trips cannot:
//!
//! 1. **Every request, response, and event in the family**, so the windowed open cannot silently
//!    change the shape a version-7 peer receives.
//! 2. **One `SeqEvent` golden per [`AgentEvent`] variant.** That set protects the *persisted log*,
//!    not just the wire: `agent_events.payload` is this exact serialization, so a rename here is a
//!    transcript that no longer decodes on the next daemon start.
//! 3. **Legacy-peer fixtures**, proving a payload written before the window fields, before Codex,
//!    and before the sequenced log still decodes.

mod support;

#[path = "agent_compatibility/legacy.rs"]
mod legacy;

use chrono::{DateTime, Utc};
use fleet_core::{
    agents::{
        AbortReason, AgentEvent, AgentKind, AgentThreadSummary, Attention, AttentionKind,
        CheckpointKind, FileDelta, GateAnswer, GateId, GateKind, GateResolver, Item, ItemId,
        ItemKind, ItemPatch, ItemPayloadPatch, ItemStatus, ModelSelection, PermissionChoice,
        PermissionMode, PlanAnswer, ProviderOptionId, Question, QuestionOption, Seq, SeqEvent,
        SessionState, StreamKind, ThreadId, ThreadProjection, ToolCall, ToolKind, TurnId,
        TurnOutcome, TurnState, Usage, UserInput,
    },
    ids::WorktreeId,
};
use fleet_proto::{
    agents::{
        AgentRevertReport, AgentSessionView, AgentThreadWindow, CheckpointId, CheckpointScope,
        TranscriptPage, TranscriptWindow, TurnCheckpoint,
    },
    event::Event,
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
};
use support::assert_frame;

/// Every `AgentEvent` tag this build implements, in declaration order.
///
/// The list is duplicated from `fleet_core::agents::AgentEvent` on purpose: it is the tripwire
/// that says a variant was added without a golden, and it can only do that by being written down
/// twice. Both copies move in the same commit.
const AGENT_EVENT_TAGS: &[&str] = &[
    "session_configured",
    "metadata_changed",
    "session_state_changed",
    "session_activity",
    "session_exited",
    "turn_started",
    "turn_settled",
    "turn_aborted",
    "turn_diff",
    "plan_steps",
    "item_started",
    "content_delta",
    "item_updated",
    "item_completed",
    "gate_opened",
    "gate_resolved",
    "gate_withdrawn",
    "plan_proposed",
    "token_usage",
    "rate_limits",
    "compacted",
    "retrying",
    "model_rerouted",
    "runtime_error",
    "notice",
    "unknown",
];

#[test]
fn agent_request_wire_goldens() {
    for (request, golden) in request_goldens() {
        assert_frame(request, golden);
    }
}

#[test]
fn agent_response_wire_goldens() {
    for (response, golden) in response_goldens() {
        assert_frame(response, golden);
    }
}

#[test]
fn agent_event_wire_goldens() {
    for (event, golden) in event_goldens() {
        assert_frame(event, golden);
    }
}

#[test]
fn every_agent_event_variant_has_a_persisted_payload_golden() {
    let goldens = seq_event_goldens();
    let tags = goldens
        .iter()
        .map(|(event, _)| {
            serde_json::to_value(&event.event)
                .unwrap_or_else(|error| panic!("{error}"))
                .get("type")
                .and_then(|tag| tag.as_str())
                .unwrap_or_else(|| panic!("an adjacently tagged payload always has a type"))
                .to_owned()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        tags, AGENT_EVENT_TAGS,
        "every AgentEvent variant needs a byte-exact SeqEvent golden, in declaration order"
    );
    for (event, golden) in goldens {
        assert_frame(event, golden);
    }
}

/// Fixed identifiers, so a golden is a byte fixture and never a snapshot of a random UUID.
fn thread() -> ThreadId {
    "11111111-2222-4333-8444-555555555555"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

fn turn() -> TurnId {
    "aaaaaaaa-2222-4333-8444-555555555555"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

fn item() -> ItemId {
    "bbbbbbbb-2222-4333-8444-555555555555"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

fn gate() -> GateId {
    "cccccccc-2222-4333-8444-555555555555"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

/// The third checkpoint of [`turn`], which is what a `[u] revert turn` would address.
fn checkpoint() -> CheckpointId {
    CheckpointId::from_parts(3, CheckpointScope::Turn, turn())
}

fn worktree() -> WorktreeId {
    WorktreeId::try_from("acme/api#native-agents").unwrap_or_else(|error| panic!("{error}"))
}

fn at() -> DateTime<Utc> {
    "2026-09-07T12:00:00Z"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

fn model() -> ModelSelection {
    ModelSelection {
        model: "gpt-5-codex".to_owned(),
        effort: Some("high".to_owned()),
        provider: None,
    }
}

fn summary() -> AgentThreadSummary {
    AgentThreadSummary {
        thread: thread(),
        worktree: worktree(),
        host: None,
        provider: AgentKind::Codex,
        title: "Codex".to_owned(),
        attention: Attention::NeedsYou(AttentionKind::Permission),
        session: SessionState::Ready,
        turn: TurnState::Running(turn()),
        last_seq: Seq(12),
        last_activity: Some(at()),
        last_completed_seq: Some(Seq(9)),
        last_nonterminal_seq: Some(Seq(11)),
        exit_code: None,
    }
}

fn window_item() -> Item {
    Item {
        id: item(),
        turn: turn(),
        parent: None,
        kind: ItemKind::AssistantText {
            text: "done".to_owned(),
        },
        status: ItemStatus::Completed,
        children: Vec::new(),
        started: at(),
        ended: Some(at()),
    }
}

fn tool_call() -> ToolCall {
    ToolCall {
        kind: ToolKind::Bash,
        name: "shell".to_owned(),
        input: serde_json::json!({"command": "cargo test"}),
        summary: Some("cargo test".to_owned()),
        result: None,
        output: "ok".to_owned(),
        diff: None,
        exit_code: Some(0),
        duration_ms: Some(1_200),
        extra: std::collections::BTreeMap::new(),
    }
}

fn seq(seq: u64, raw: Option<&str>, event: AgentEvent) -> SeqEvent {
    SeqEvent {
        seq: Seq(seq),
        at: at(),
        raw: raw.map(str::to_owned),
        event,
    }
}

fn request_goldens() -> Vec<(Request, &'static str)> {
    let thread = thread();
    vec![
        (
            Request {
                id: 1,
                body: RequestBody::AgentThreadList,
            },
            r#"{"id":1,"body":{"type":"agent_thread_list"}}"#,
        ),
        (
            Request {
                id: 2,
                body: RequestBody::AgentThreadCreate {
                    worktree: worktree(),
                    provider: AgentKind::Codex,
                    model: Some(model()),
                    mode: PermissionMode::Ask,
                    resume_cursor: Some("thread-1".to_owned()),
                    title: Some("native agents".to_owned()),
                },
            },
            r#"{"id":2,"body":{"type":"agent_thread_create","worktree":"acme/api#native-agents","provider":"codex","model":{"model":"gpt-5-codex","effort":"high"},"mode":"ask","resume_cursor":"thread-1","title":"native agents"}}"#,
        ),
        (
            Request {
                id: 3,
                body: RequestBody::AgentThreadOpen {
                    thread,
                    from_seq: Some(Seq(41)),
                    after_seq: None,
                    turn_limit: None,
                    before_cursor: None,
                    request_sync_marker: false,
                },
            },
            r#"{"id":3,"body":{"type":"agent_thread_open","thread":"11111111-2222-4333-8444-555555555555","from_seq":41}}"#,
        ),
        (
            Request {
                id: 4,
                body: RequestBody::AgentThreadOpen {
                    thread,
                    from_seq: None,
                    after_seq: None,
                    turn_limit: Some(10),
                    before_cursor: None,
                    request_sync_marker: true,
                },
            },
            r#"{"id":4,"body":{"type":"agent_thread_open","thread":"11111111-2222-4333-8444-555555555555","from_seq":null,"turn_limit":10,"request_sync_marker":true}}"#,
        ),
        (
            Request {
                id: 5,
                body: RequestBody::AgentThreadOpen {
                    thread,
                    from_seq: None,
                    after_seq: Some(Seq(41)),
                    turn_limit: Some(25),
                    before_cursor: Some("fat.1.11111111-2222-4333-8444-555555555555.90".to_owned()),
                    request_sync_marker: true,
                },
            },
            r#"{"id":5,"body":{"type":"agent_thread_open","thread":"11111111-2222-4333-8444-555555555555","from_seq":null,"after_seq":41,"turn_limit":25,"before_cursor":"fat.1.11111111-2222-4333-8444-555555555555.90","request_sync_marker":true}}"#,
        ),
        (
            Request {
                id: 6,
                body: RequestBody::AgentItemBody {
                    thread,
                    item: item(),
                    stream: StreamKind::CommandOutput,
                    offset: 262_144,
                    limit: 262_144,
                },
            },
            r#"{"id":6,"body":{"type":"agent_item_body","thread":"11111111-2222-4333-8444-555555555555","item":"bbbbbbbb-2222-4333-8444-555555555555","stream":{"type":"command_output"},"offset":262144,"limit":262144}}"#,
        ),
        (
            Request {
                id: 7,
                body: RequestBody::AgentThreadClose { thread },
            },
            r#"{"id":7,"body":{"type":"agent_thread_close","thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 8,
                body: RequestBody::AgentSend {
                    thread,
                    input: UserInput {
                        text: "inspect the failing test".to_owned(),
                        attachments: Vec::new(),
                        item: None,
                    },
                },
            },
            r#"{"id":8,"body":{"type":"agent_send","thread":"11111111-2222-4333-8444-555555555555","input":{"text":"inspect the failing test","attachments":[]}}}"#,
        ),
        (
            Request {
                id: 9,
                body: RequestBody::AgentInterrupt { thread },
            },
            r#"{"id":9,"body":{"type":"agent_interrupt","thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 10,
                body: RequestBody::AgentRespond {
                    thread,
                    gate: gate(),
                    answer: GateAnswer::Permission {
                        choice: PermissionChoice::AllowSession,
                        edited_payload: None,
                    },
                },
            },
            r#"{"id":10,"body":{"type":"agent_respond","thread":"11111111-2222-4333-8444-555555555555","gate":"cccccccc-2222-4333-8444-555555555555","answer":{"type":"permission","data":{"choice":"allow_session"}}}}"#,
        ),
        (
            Request {
                id: 11,
                body: RequestBody::AgentSetMode {
                    thread,
                    mode: PermissionMode::Plan,
                },
            },
            r#"{"id":11,"body":{"type":"agent_set_mode","thread":"11111111-2222-4333-8444-555555555555","mode":"plan"}}"#,
        ),
        (
            Request {
                id: 12,
                body: RequestBody::AgentSetModel {
                    thread,
                    model: model(),
                },
            },
            r#"{"id":12,"body":{"type":"agent_set_model","thread":"11111111-2222-4333-8444-555555555555","model":{"model":"gpt-5-codex","effort":"high"}}}"#,
        ),
        (
            Request {
                id: 13,
                body: RequestBody::AgentMarkSeen {
                    thread,
                    seq: Seq(12),
                },
            },
            r#"{"id":13,"body":{"type":"agent_mark_seen","thread":"11111111-2222-4333-8444-555555555555","seq":12}}"#,
        ),
        (
            Request {
                id: 14,
                body: RequestBody::AgentStop { thread },
            },
            r#"{"id":14,"body":{"type":"agent_stop","thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 15,
                body: RequestBody::AgentCheckpoints { thread },
            },
            r#"{"id":15,"body":{"type":"agent_checkpoints","thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Request {
                id: 16,
                body: RequestBody::AgentRevert {
                    thread,
                    checkpoint: checkpoint(),
                },
            },
            r#"{"id":16,"body":{"type":"agent_revert","thread":"11111111-2222-4333-8444-555555555555","checkpoint":"00003-turn-aaaaaaaa-2222-4333-8444-555555555555"}}"#,
        ),
    ]
}

fn response_goldens() -> Vec<(Response, &'static str)> {
    let projection = ThreadProjection::new(thread(), worktree(), AgentKind::Codex);
    vec![
        (
            Response {
                id: 1,
                result: Ok(ResponseBody::AgentThreads(vec![summary()])),
            },
            r#"{"id":1,"result":{"Ok":{"type":"agent_threads","data":[{"thread":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","attention":{"type":"needs_you","data":"permission"},"session":{"type":"ready"},"turn":{"type":"running","data":"aaaaaaaa-2222-4333-8444-555555555555"},"lastSeq":12,"lastActivity":"2026-09-07T12:00:00Z","lastCompletedSeq":9,"lastNonterminalSeq":11,"exitCode":null}]}}}"#,
        ),
        (
            Response {
                id: 2,
                result: Ok(ResponseBody::AgentThreadCreated(summary())),
            },
            r#"{"id":2,"result":{"Ok":{"type":"agent_thread_created","data":{"thread":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","attention":{"type":"needs_you","data":"permission"},"session":{"type":"ready"},"turn":{"type":"running","data":"aaaaaaaa-2222-4333-8444-555555555555"},"lastSeq":12,"lastActivity":"2026-09-07T12:00:00Z","lastCompletedSeq":9,"lastNonterminalSeq":11,"exitCode":null}}}}"#,
        ),
        (
            Response {
                id: 3,
                result: Ok(ResponseBody::AgentThreadSnapshot {
                    projection,
                    events_after: vec![seq(1, None, AgentEvent::Notice("ready".to_owned()))],
                }),
            },
            r#"{"id":3,"result":{"Ok":{"type":"agent_thread_snapshot","data":{"projection":{"thread":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","session":{"type":"starting"},"turn":{"type":"none"},"gates":[],"items":[],"turns":[],"backgroundTasks":[],"lastSeq":0,"lastActivity":null,"cumulativeUsage":{"inputTokens":0,"outputTokens":0,"reasoningTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":0,"webSearchRequests":0,"toolUses":0},"cumulativeCostUsd":null,"contextPct":0.0,"model":null,"mode":"ask","exitCode":null,"retrying":null},"events_after":[{"seq":1,"at":"2026-09-07T12:00:00Z","event":{"type":"notice","data":"ready"}}]}}}}"#,
        ),
        (
            Response {
                id: 4,
                result: Ok(ResponseBody::AgentThreadWindow(Box::new(
                    AgentThreadWindow {
                        summary: summary(),
                        session: AgentSessionView {
                            model: Some(model()),
                            mode: PermissionMode::Ask,
                            tools: vec!["shell".to_owned()],
                            commands: vec!["/review".to_owned()],
                            ..AgentSessionView::default()
                        },
                        window: TranscriptWindow {
                            turns: Vec::new(),
                            items: vec![window_item()],
                            checkpoints: Vec::new(),
                            notices: Vec::new(),
                            gates: Vec::new(),
                            elided: vec![item()],
                        },
                        page: Some(TranscriptPage {
                            before_cursor: Some(
                                "fat.1.11111111-2222-4333-8444-555555555555.90".to_owned(),
                            ),
                            has_more: true,
                            thread_seq: Seq(12),
                        }),
                        head_seq: Seq(12),
                        projected_seq: Seq(12),
                        events_after: Vec::new(),
                        synchronized: true,
                    },
                ))),
            },
            r#"{"id":4,"result":{"Ok":{"type":"agent_thread_window","data":{"summary":{"thread":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","attention":{"type":"needs_you","data":"permission"},"session":{"type":"ready"},"turn":{"type":"running","data":"aaaaaaaa-2222-4333-8444-555555555555"},"lastSeq":12,"lastActivity":"2026-09-07T12:00:00Z","lastCompletedSeq":9,"lastNonterminalSeq":11,"exitCode":null},"session":{"model":{"model":"gpt-5-codex","effort":"high"},"mode":"ask","tools":["shell"],"commands":["/review"]},"window":{"items":[{"id":"bbbbbbbb-2222-4333-8444-555555555555","turn":"aaaaaaaa-2222-4333-8444-555555555555","kind":{"type":"assistant_text","data":{"text":"done"}},"status":"completed","children":[],"started":"2026-09-07T12:00:00Z","ended":"2026-09-07T12:00:00Z"}],"elided":["bbbbbbbb-2222-4333-8444-555555555555"]},"page":{"beforeCursor":"fat.1.11111111-2222-4333-8444-555555555555.90","hasMore":true,"threadSeq":12},"headSeq":12,"projectedSeq":12,"synchronized":true}}}}"#,
        ),
        (
            Response {
                id: 5,
                result: Ok(ResponseBody::AgentItemBodyChunk {
                    thread: thread(),
                    item: item(),
                    stream: StreamKind::CommandOutput,
                    offset: 262_144,
                    total: 1_048_576,
                    text: "…tail".to_owned(),
                }),
            },
            r#"{"id":5,"result":{"Ok":{"type":"agent_item_body_chunk","data":{"thread":"11111111-2222-4333-8444-555555555555","item":"bbbbbbbb-2222-4333-8444-555555555555","stream":{"type":"command_output"},"offset":262144,"total":1048576,"text":"…tail"}}}}"#,
        ),
        (
            Response {
                id: 6,
                result: Ok(ResponseBody::AgentAck),
            },
            r#"{"id":6,"result":{"Ok":{"type":"agent_ack"}}}"#,
        ),
        (
            Response {
                id: 7,
                result: Ok(ResponseBody::AgentCheckpoints(vec![TurnCheckpoint {
                    id: checkpoint(),
                    scope: CheckpointScope::Turn,
                    turn: turn(),
                    ordinal: 3,
                    at: at(),
                }])),
            },
            r#"{"id":7,"result":{"Ok":{"type":"agent_checkpoints","data":[{"id":"00003-turn-aaaaaaaa-2222-4333-8444-555555555555","scope":"turn","turn":"aaaaaaaa-2222-4333-8444-555555555555","ordinal":3,"at":"2026-09-07T12:00:00Z"}]}}}"#,
        ),
        (
            Response {
                id: 8,
                result: Ok(ResponseBody::AgentReverted(AgentRevertReport {
                    thread: thread(),
                    checkpoint: checkpoint(),
                    restored: 2,
                    deleted: 1,
                    paths: vec!["src/lib.rs".to_owned(), "src/new.rs".to_owned()],
                })),
            },
            r#"{"id":8,"result":{"Ok":{"type":"agent_reverted","data":{"thread":"11111111-2222-4333-8444-555555555555","checkpoint":"00003-turn-aaaaaaaa-2222-4333-8444-555555555555","restored":2,"deleted":1,"paths":["src/lib.rs","src/new.rs"]}}}}"#,
        ),
    ]
}

fn event_goldens() -> Vec<(Event, &'static str)> {
    vec![
        (
            Event::Agent {
                thread: thread(),
                event: seq(
                    12,
                    Some("system.init"),
                    AgentEvent::Notice("ready".to_owned()),
                ),
            },
            r#"{"type":"agent","data":{"thread":"11111111-2222-4333-8444-555555555555","event":{"seq":12,"at":"2026-09-07T12:00:00Z","raw":"system.init","event":{"type":"notice","data":"ready"}}}}"#,
        ),
        (
            Event::AgentSummary(summary()),
            r#"{"type":"agent_summary","data":{"thread":"11111111-2222-4333-8444-555555555555","worktree":"acme/api#native-agents","provider":"codex","title":"Codex","attention":{"type":"needs_you","data":"permission"},"session":{"type":"ready"},"turn":{"type":"running","data":"aaaaaaaa-2222-4333-8444-555555555555"},"lastSeq":12,"lastActivity":"2026-09-07T12:00:00Z","lastCompletedSeq":9,"lastNonterminalSeq":11,"exitCode":null}}"#,
        ),
        (
            Event::AgentResync {
                thread: thread(),
                from_seq: Seq(41),
            },
            r#"{"type":"agent_resync","data":{"thread":"11111111-2222-4333-8444-555555555555","from_seq":41}}"#,
        ),
        (
            Event::AgentSynchronized { thread: thread() },
            r#"{"type":"agent_synchronized","data":{"thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
        (
            Event::AgentWindow { thread: thread() },
            r#"{"type":"agent_window","data":{"thread":"11111111-2222-4333-8444-555555555555"}}"#,
        ),
    ]
}

fn seq_event_goldens() -> Vec<(SeqEvent, &'static str)> {
    vec![
        (
            seq(
                1,
                Some("system.init"),
                AgentEvent::SessionConfigured {
                    provider: AgentKind::Codex,
                    resume_cursor: Some("thread-1".to_owned()),
                    model: Some(model()),
                    mode: PermissionMode::Ask,
                    tools: vec!["shell".to_owned()],
                    commands: vec!["/review".to_owned()],
                    skills: Vec::new(),
                },
            ),
            r#"{"seq":1,"at":"2026-09-07T12:00:00Z","raw":"system.init","event":{"type":"session_configured","data":{"provider":"codex","resume_cursor":"thread-1","model":{"model":"gpt-5-codex","effort":"high"},"mode":"ask","tools":["shell"],"commands":["/review"],"skills":[]}}}"#,
        ),
        (
            seq(
                2,
                None,
                AgentEvent::MetadataChanged {
                    title: Some("protocol goldens".to_owned()),
                    mode: Some(PermissionMode::Plan),
                    model: None,
                },
            ),
            r#"{"seq":2,"at":"2026-09-07T12:00:00Z","event":{"type":"metadata_changed","data":{"title":"protocol goldens","mode":"plan"}}}"#,
        ),
        (
            seq(
                3,
                None,
                AgentEvent::SessionStateChanged(SessionState::Ready),
            ),
            r#"{"seq":3,"at":"2026-09-07T12:00:00Z","event":{"type":"session_state_changed","data":{"type":"ready"}}}"#,
        ),
        (
            seq(
                4,
                None,
                AgentEvent::SessionActivity {
                    phase: "compacting".to_owned(),
                },
            ),
            r#"{"seq":4,"at":"2026-09-07T12:00:00Z","event":{"type":"session_activity","data":{"phase":"compacting"}}}"#,
        ),
        (
            seq(
                5,
                None,
                AgentEvent::SessionExited {
                    code: Some(0),
                    expected: true,
                },
            ),
            r#"{"seq":5,"at":"2026-09-07T12:00:00Z","event":{"type":"session_exited","data":{"code":0,"expected":true}}}"#,
        ),
        (
            seq(
                6,
                None,
                AgentEvent::TurnStarted {
                    turn: turn(),
                    user_item: item(),
                },
            ),
            r#"{"seq":6,"at":"2026-09-07T12:00:00Z","event":{"type":"turn_started","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","user_item":"bbbbbbbb-2222-4333-8444-555555555555"}}}"#,
        ),
        (
            seq(
                7,
                None,
                AgentEvent::TurnSettled {
                    turn: turn(),
                    outcome: TurnOutcome::Completed,
                    usage: Usage {
                        input_tokens: 100,
                        output_tokens: 20,
                        ..Usage::default()
                    },
                    duration_ms: 4_200,
                    files_changed: vec![FileDelta {
                        path: std::path::PathBuf::from("crates/fleet-proto/src/agents.rs"),
                        added: 12,
                        removed: 3,
                    }],
                },
            ),
            r#"{"seq":7,"at":"2026-09-07T12:00:00Z","event":{"type":"turn_settled","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","outcome":{"type":"completed"},"usage":{"inputTokens":100,"outputTokens":20,"reasoningTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":0,"webSearchRequests":0,"toolUses":0},"duration_ms":4200,"files_changed":[{"path":"crates/fleet-proto/src/agents.rs","added":12,"removed":3}]}}}"#,
        ),
        (
            seq(
                8,
                None,
                AgentEvent::TurnAborted {
                    turn: turn(),
                    reason: AbortReason::User,
                },
            ),
            r#"{"seq":8,"at":"2026-09-07T12:00:00Z","event":{"type":"turn_aborted","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","reason":{"type":"user"}}}}"#,
        ),
        (
            seq(
                9,
                None,
                AgentEvent::TurnDiff {
                    turn: turn(),
                    unified: "--- a\n+++ b\n".to_owned(),
                    files_changed: Vec::new(),
                },
            ),
            r#"{"seq":9,"at":"2026-09-07T12:00:00Z","event":{"type":"turn_diff","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","unified":"--- a\n+++ b\n","files_changed":[]}}}"#,
        ),
        (
            seq(
                10,
                None,
                AgentEvent::PlanSteps {
                    turn: turn(),
                    steps: vec!["read the spec".to_owned()],
                },
            ),
            r#"{"seq":10,"at":"2026-09-07T12:00:00Z","event":{"type":"plan_steps","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","steps":["read the spec"]}}}"#,
        ),
        (
            seq(
                11,
                None,
                AgentEvent::ItemStarted {
                    turn: turn(),
                    item: item(),
                    kind: ItemKind::Tool(Box::new(tool_call())),
                    parent: None,
                },
            ),
            r#"{"seq":11,"at":"2026-09-07T12:00:00Z","event":{"type":"item_started","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","item":"bbbbbbbb-2222-4333-8444-555555555555","kind":{"type":"tool","data":{"kind":{"type":"bash"},"name":"shell","input":{"command":"cargo test"},"summary":"cargo test","output":"ok","exit_code":0,"duration_ms":1200}},"parent":null}}}"#,
        ),
        (
            seq(
                12,
                None,
                AgentEvent::ContentDelta {
                    item: item(),
                    stream: StreamKind::ReasoningSummary { part: 0 },
                    delta: "thinking".to_owned(),
                },
            ),
            r#"{"seq":12,"at":"2026-09-07T12:00:00Z","event":{"type":"content_delta","data":{"item":"bbbbbbbb-2222-4333-8444-555555555555","stream":{"type":"reasoning_summary","data":{"part":0}},"delta":"thinking"}}}"#,
        ),
        (
            seq(
                13,
                None,
                AgentEvent::ItemUpdated {
                    item: item(),
                    patch: ItemPatch {
                        payload: Some(ItemPayloadPatch::AssistantText {
                            text: "done".to_owned(),
                        }),
                        status: Some(ItemStatus::Completed),
                    },
                },
            ),
            r#"{"seq":13,"at":"2026-09-07T12:00:00Z","event":{"type":"item_updated","data":{"item":"bbbbbbbb-2222-4333-8444-555555555555","patch":{"payload":{"type":"assistant_text","data":{"text":"done"}},"status":"completed"}}}}"#,
        ),
        (
            seq(
                14,
                None,
                AgentEvent::ItemCompleted {
                    item: item(),
                    status: ItemStatus::Completed,
                },
            ),
            r#"{"seq":14,"at":"2026-09-07T12:00:00Z","event":{"type":"item_completed","data":{"item":"bbbbbbbb-2222-4333-8444-555555555555","status":"completed"}}}"#,
        ),
        (
            seq(
                15,
                None,
                AgentEvent::GateOpened {
                    gate: gate(),
                    turn: Some(turn()),
                    kind: GateKind::Question {
                        questions: vec![Question {
                            id: "store".to_owned(),
                            header: "Storage".to_owned(),
                            prompt: "Which store?".to_owned(),
                            options: vec![QuestionOption {
                                id: ProviderOptionId("sqlite".to_owned()),
                                label: "SQLite".to_owned(),
                                description: "One file per daemon".to_owned(),
                            }],
                            multi_select: false,
                            allows_other: true,
                            is_secret: false,
                            blocking: true,
                        }],
                    },
                },
            ),
            r#"{"seq":15,"at":"2026-09-07T12:00:00Z","event":{"type":"gate_opened","data":{"gate":"cccccccc-2222-4333-8444-555555555555","turn":"aaaaaaaa-2222-4333-8444-555555555555","kind":{"type":"question","data":{"questions":[{"id":"store","header":"Storage","prompt":"Which store?","options":[{"id":"sqlite","label":"SQLite","description":"One file per daemon"}],"multiSelect":false,"allowsOther":true,"isSecret":false,"blocking":true}]}}}}}"#,
        ),
        (
            seq(
                16,
                None,
                AgentEvent::GateResolved {
                    gate: gate(),
                    answer: GateAnswer::Plan(PlanAnswer::Approve),
                    by: GateResolver::User,
                },
            ),
            r#"{"seq":16,"at":"2026-09-07T12:00:00Z","event":{"type":"gate_resolved","data":{"gate":"cccccccc-2222-4333-8444-555555555555","answer":{"type":"plan","data":{"type":"approve"}},"by":"user"}}}"#,
        ),
        (
            seq(17, None, AgentEvent::GateWithdrawn { gate: gate() }),
            r#"{"seq":17,"at":"2026-09-07T12:00:00Z","event":{"type":"gate_withdrawn","data":{"gate":"cccccccc-2222-4333-8444-555555555555"}}}"#,
        ),
        (
            seq(
                18,
                None,
                AgentEvent::PlanProposed {
                    gate: gate(),
                    turn: turn(),
                    markdown: "Plan\n- add the goldens\n".to_owned(),
                    steps: vec!["add the goldens".to_owned()],
                },
            ),
            r#"{"seq":18,"at":"2026-09-07T12:00:00Z","event":{"type":"plan_proposed","data":{"gate":"cccccccc-2222-4333-8444-555555555555","turn":"aaaaaaaa-2222-4333-8444-555555555555","markdown":"Plan\n- add the goldens\n","steps":["add the goldens"]}}}"#,
        ),
        (
            seq(
                19,
                None,
                AgentEvent::TokenUsage {
                    turn: turn(),
                    usage: Usage {
                        input_tokens: 100,
                        output_tokens: 20,
                        ..Usage::default()
                    },
                    context_pct: 12.5,
                    cost_usd: Some(0.25),
                },
            ),
            r#"{"seq":19,"at":"2026-09-07T12:00:00Z","event":{"type":"token_usage","data":{"turn":"aaaaaaaa-2222-4333-8444-555555555555","usage":{"inputTokens":100,"outputTokens":20,"reasoningTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":0,"webSearchRequests":0,"toolUses":0},"context_pct":12.5,"cost_usd":0.25}}}"#,
        ),
        (
            seq(
                20,
                None,
                AgentEvent::RateLimits {
                    limits: serde_json::json!({"resetsAt": "2026-09-07T13:00:00Z", "used": 42}),
                },
            ),
            r#"{"seq":20,"at":"2026-09-07T12:00:00Z","event":{"type":"rate_limits","data":{"limits":{"resetsAt":"2026-09-07T13:00:00Z","used":42}}}}"#,
        ),
        (
            seq(
                21,
                None,
                AgentEvent::Compacted(CheckpointKind::CompactBoundary {
                    before: 180_000,
                    after: Some(40_000),
                }),
            ),
            r#"{"seq":21,"at":"2026-09-07T12:00:00Z","event":{"type":"compacted","data":{"type":"compact_boundary","data":{"before":180000,"after":40000}}}}"#,
        ),
        (
            seq(
                22,
                None,
                AgentEvent::Retrying {
                    attempt: 2,
                    retry_in_ms: 500,
                    reason: "overloaded".to_owned(),
                },
            ),
            r#"{"seq":22,"at":"2026-09-07T12:00:00Z","event":{"type":"retrying","data":{"attempt":2,"retry_in_ms":500,"reason":"overloaded"}}}"#,
        ),
        (
            seq(
                23,
                None,
                AgentEvent::ModelRerouted {
                    from: "gpt-5-codex".to_owned(),
                    to: "gpt-5".to_owned(),
                    reason: "capacity".to_owned(),
                },
            ),
            r#"{"seq":23,"at":"2026-09-07T12:00:00Z","event":{"type":"model_rerouted","data":{"from":"gpt-5-codex","to":"gpt-5","reason":"capacity"}}}"#,
        ),
        (
            seq(
                24,
                None,
                AgentEvent::RuntimeError {
                    fatal: false,
                    message: "stream reset".to_owned(),
                },
            ),
            r#"{"seq":24,"at":"2026-09-07T12:00:00Z","event":{"type":"runtime_error","data":{"fatal":false,"message":"stream reset"}}}"#,
        ),
        (
            seq(25, None, AgentEvent::Notice("deprecated flag".to_owned())),
            r#"{"seq":25,"at":"2026-09-07T12:00:00Z","event":{"type":"notice","data":"deprecated flag"}}"#,
        ),
        (
            seq(
                26,
                Some("codex/unknownMethod"),
                AgentEvent::Unknown {
                    method: "codex/unknownMethod".to_owned(),
                },
            ),
            r#"{"seq":26,"at":"2026-09-07T12:00:00Z","raw":"codex/unknownMethod","event":{"type":"unknown","data":{"method":"codex/unknownMethod"}}}"#,
        ),
    ]
}
