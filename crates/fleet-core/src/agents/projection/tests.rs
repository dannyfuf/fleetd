use std::{collections::BTreeMap, path::PathBuf};

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use super::*;
use crate::agents::{
    AbortReason, AgentEvent, AgentKind, Attachment, Attention, AttentionKind, FileDelta,
    GateAnswer, GateId, GateKind, GateResolver, ItemId, ItemKind, ItemPatch, ItemPayloadPatch,
    ItemStatus, ModelSelection, PermissionChoice, PermissionMode, PermissionOption, PlanAnswer,
    ProviderOptionId, Question, QuestionOption, Seq, SeqEvent, SessionState, StreamKind, ThreadId,
    ToolCall, ToolDiff, ToolKind, ToolPatch, TurnId, TurnOutcome, TurnState, Usage,
};
use crate::ids::WorktreeId;

struct EventBuilder {
    seq: u64,
}

impl EventBuilder {
    fn new() -> Self {
        Self { seq: 0 }
    }

    fn next(&mut self, event: AgentEvent) -> SeqEvent {
        self.seq += 1;
        SeqEvent {
            seq: Seq(self.seq),
            at: timestamp(self.seq),
            raw: None,
            event,
        }
    }
}

fn timestamp(second: u64) -> DateTime<Utc> {
    DateTime::from_timestamp(second as i64, 0)
        .unwrap_or_else(|| panic!("test timestamp must be valid"))
}

fn thread_id(value: u128) -> ThreadId {
    ThreadId::from_uuid(Uuid::from_u128(value))
}

fn turn_id(value: u128) -> TurnId {
    TurnId::from_uuid(Uuid::from_u128(value))
}

fn item_id(value: u128) -> ItemId {
    ItemId::from_uuid(Uuid::from_u128(value))
}

fn gate_id(value: u128) -> GateId {
    GateId::from_uuid(Uuid::from_u128(value))
}

fn projection() -> ThreadProjection {
    ThreadProjection::new(
        thread_id(1),
        WorktreeId::try_from("acme/api#native-agents").unwrap_or_else(|error| panic!("{error}")),
        AgentKind::Claude,
    )
}

fn apply(
    projection: &mut ThreadProjection,
    events: &mut EventBuilder,
    event: AgentEvent,
) -> SeqEvent {
    let event = events.next(event);
    projection
        .apply(&event)
        .unwrap_or_else(|error| panic!("{error}"));
    event
}

fn session_started() -> AgentEvent {
    AgentEvent::SessionConfigured {
        provider: AgentKind::Claude,
        resume_cursor: Some("session-1".to_owned()),
        model: Some(ModelSelection {
            model: "claude-sonnet-5".to_owned(),
            effort: Some("high".to_owned()),
            provider: None,
        }),
        mode: PermissionMode::Ask,
        tools: vec!["Read".to_owned(), "Edit".to_owned()],
        commands: vec!["compact".to_owned()],
        skills: vec!["review".to_owned()],
    }
}

fn user_message(text: &str) -> ItemKind {
    ItemKind::UserMessage {
        text: text.to_owned(),
        attachments: Vec::<Attachment>::new(),
        steered: false,
    }
}

fn item_text(item: &Item) -> Option<&str> {
    match &item.kind {
        ItemKind::UserMessage { text, .. }
        | ItemKind::AssistantText { text }
        | ItemKind::Plan { text } => Some(text),
        ItemKind::Reasoning { summary, raw } => summary
            .values()
            .next()
            .or_else(|| raw.values().next())
            .map(String::as_str),
        ItemKind::Error { message } => Some(message),
        ItemKind::Tool(_) | ItemKind::Subagent { .. } => None,
    }
}

fn tool_output(item: &Item) -> Option<&str> {
    match &item.kind {
        ItemKind::Tool(call) => Some(&call.output),
        _ => None,
    }
}

fn tool_diff(item: &Item) -> Option<&ToolDiff> {
    match &item.kind {
        ItemKind::Tool(call) => call.diff.as_ref(),
        _ => None,
    }
}

fn usage(total_tokens: u64) -> Usage {
    Usage {
        input_tokens: total_tokens / 2,
        output_tokens: total_tokens / 4,
        reasoning_tokens: total_tokens / 4,
        total_tokens,
        ..Usage::default()
    }
}

fn start_turn(
    projection: &mut ThreadProjection,
    events: &mut EventBuilder,
    turn: TurnId,
    user: ItemId,
    text: &str,
) {
    apply(
        projection,
        events,
        AgentEvent::TurnStarted {
            turn,
            user_item: user,
        },
    );
    apply(
        projection,
        events,
        AgentEvent::ItemStarted {
            turn,
            item: user,
            kind: user_message(text),
            parent: None,
        },
    );
}

fn complete_turn(
    projection: &mut ThreadProjection,
    events: &mut EventBuilder,
    turn: TurnId,
) -> SeqEvent {
    apply(
        projection,
        events,
        AgentEvent::TurnSettled {
            turn,
            outcome: TurnOutcome::Completed,
            usage: usage(120),
            duration_ms: 48_000,
            files_changed: Vec::new(),
        },
    )
}

fn permission_gate() -> GateKind {
    GateKind::Permission {
        tool: ToolKind::Bash,
        title: "Run command?".to_owned(),
        payload: "cargo test".to_owned(),
        rationale: Some("Tests need approval".to_owned()),
        options: vec![PermissionOption {
            id: ProviderOptionId("once".to_owned()),
            label: PermissionChoice::AllowOnce,
        }],
    }
}

fn question_gate() -> GateKind {
    GateKind::Question {
        questions: vec![Question {
            id: "Which database?".to_owned(),
            header: "Database".to_owned(),
            prompt: "Which database?".to_owned(),
            options: vec![QuestionOption {
                id: ProviderOptionId("SQLite".to_owned()),
                label: "SQLite".to_owned(),
                description: "Local".to_owned(),
            }],
            multi_select: false,
            allows_other: true,
            is_secret: false,
            blocking: true,
        }],
    }
}

fn plan_gate() -> GateKind {
    GateKind::Plan {
        markdown: "# Plan".to_owned(),
        steps: vec!["Implement reducer".to_owned()],
    }
}

/// The provider reports the turn's wall clock, which counts the minutes a permission card
/// sat on screen waiting for a `y`. §2 reads the footer as the turn's own duration, so the
/// gate's open window is charged to the turn and taken back off again.
#[test]
fn a_turn_footer_excludes_the_time_the_turn_stood_blocked_on_the_user() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(2);
    let gate = gate_id(3);
    start_turn(&mut projection, &mut events, turn, item_id(4), "touch it");

    // The clock ticks one second per event, so the six events between the open and the
    // answer are six seconds the user spent reading the card.
    apply(
        &mut projection,
        &mut events,
        AgentEvent::GateOpened {
            gate,
            turn: Some(turn),
            kind: permission_gate(),
        },
    );
    for _ in 0..5 {
        apply(
            &mut projection,
            &mut events,
            AgentEvent::Notice("waiting".to_owned()),
        );
    }
    apply(
        &mut projection,
        &mut events,
        AgentEvent::GateResolved {
            gate,
            answer: GateAnswer::Permission {
                choice: PermissionChoice::AllowOnce,
                edited_payload: None,
            },
            by: GateResolver::User,
        },
    );
    assert_eq!(projection.turns[0].blocked_ms, 6_000);

    // `duration_ms: 48_000` is what the provider measured, gate included.
    complete_turn(&mut projection, &mut events, turn);
    let footer = projection.turns[0]
        .footer()
        .unwrap_or_else(|| panic!("a settled turn has a footer"));
    assert_eq!(
        footer.duration_ms, 42_000,
        "48s wall clock minus the 6s parked"
    );
}

/// OpenCode opens the plan gate *inside* turn settlement, so the card is still open when
/// the turn's footer freezes. Charging the reading time to that turn would move a number
/// the user has already read — to zero whenever the plan is read for longer than the turn
/// ran — which is the §5 rule the late-`TokenUsage` arm is written to keep.
#[test]
fn a_gate_opened_at_settlement_does_not_move_a_settled_turn_footer() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(2);
    let gate = gate_id(3);
    start_turn(&mut projection, &mut events, turn, item_id(4), "plan it");

    apply(
        &mut projection,
        &mut events,
        AgentEvent::GateOpened {
            gate,
            turn: Some(turn),
            kind: plan_gate(),
        },
    );
    complete_turn(&mut projection, &mut events, turn);
    let settled = projection.turns[0]
        .footer()
        .unwrap_or_else(|| panic!("a settled turn has a footer"));
    assert_eq!(settled.duration_ms, 48_000);

    // The user reads the plan for five ticks and then approves it.
    for _ in 0..5 {
        apply(
            &mut projection,
            &mut events,
            AgentEvent::Notice("reading".to_owned()),
        );
    }
    apply(
        &mut projection,
        &mut events,
        AgentEvent::GateResolved {
            gate,
            answer: GateAnswer::Plan(PlanAnswer::Approve),
            by: GateResolver::User,
        },
    );
    assert_eq!(projection.turns[0].blocked_ms, 0);
    assert_eq!(
        projection.turns[0].footer(),
        Some(settled),
        "answering a gate after the turn settled must not rewrite its footer"
    );
}

/// A second gate opening behind the first does not restart the clock, or the overlap would
/// be charged twice and the footer could go to zero.
#[test]
fn overlapping_gates_are_one_wait_not_two() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(2);
    let (first, second) = (gate_id(3), gate_id(4));
    start_turn(&mut projection, &mut events, turn, item_id(5), "two gates");

    for gate in [first, second] {
        apply(
            &mut projection,
            &mut events,
            AgentEvent::GateOpened {
                gate,
                turn: Some(turn),
                kind: permission_gate(),
            },
        );
    }
    for gate in [first, second] {
        apply(
            &mut projection,
            &mut events,
            AgentEvent::GateResolved {
                gate,
                answer: GateAnswer::Permission {
                    choice: PermissionChoice::AllowOnce,
                    edited_payload: None,
                },
                by: GateResolver::User,
            },
        );
    }
    // Opened at seq 3 and 4, closed at 5 and 6: one window of three seconds.
    assert_eq!(projection.turns[0].blocked_ms, 3_000);
}

#[test]
fn plain_text_turn_projects_text_usage_footer_title_and_summary() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(10);
    let user = item_id(11);
    let assistant = item_id(12);
    apply(&mut projection, &mut events, session_started());
    start_turn(
        &mut projection,
        &mut events,
        turn,
        user,
        "Implement a deterministic native agent projection reducer with tests",
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::ItemStarted {
            turn,
            item: assistant,
            kind: ItemKind::AssistantText {
                text: String::new(),
            },
            parent: None,
        },
    );
    for delta in ["Hello", ", world"] {
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ContentDelta {
                item: assistant,
                stream: StreamKind::AssistantText,
                delta: delta.to_owned(),
            },
        );
    }
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TokenUsage {
            turn,
            usage: usage(100),
            context_pct: 34.0,
            cost_usd: Some(0.42),
        },
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::ItemCompleted {
            item: assistant,
            status: ItemStatus::Completed,
        },
    );
    let completed = complete_turn(&mut projection, &mut events, turn);

    assert_eq!(
        projection.turn,
        TurnState::Settled(turn, TurnOutcome::Completed)
    );
    assert_eq!(projection.items[0].status, ItemStatus::Completed);
    assert_eq!(item_text(&projection.items[1]), Some("Hello, world"));
    assert_eq!(projection.title, "Implement a deterministic native agent");
    assert_eq!(projection.cumulative_usage, usage(120));
    assert_eq!(projection.cumulative_cost_usd, Some(0.42));
    assert_eq!(projection.context_pct, 34.0);
    assert_eq!(
        projection.turns[0].footer(),
        Some(TurnFooter {
            duration_ms: 48_000,
            tokens: 120,
            files_changed: 0,
            added: 0,
            removed: 0,
        })
    );
    assert_eq!(projection.last_activity, Some(completed.at));
    assert_eq!(
        projection.summary(completed.seq),
        projection.summary(completed.seq)
    );
}

#[test]
fn edit_tool_projects_replacement_patch_diff_output_and_file_totals() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(20);
    let user = item_id(21);
    let tool = item_id(22);
    apply(&mut projection, &mut events, session_started());
    start_turn(&mut projection, &mut events, turn, user, "Edit two files");
    apply(
        &mut projection,
        &mut events,
        AgentEvent::ItemStarted {
            turn,
            item: tool,
            kind: ItemKind::Tool(Box::new(ToolCall {
                kind: ToolKind::Edit,
                name: "Edit".to_owned(),
                input: json!({"path": "src/lib.rs"}),
                summary: None,
                result: None,
                output: String::new(),
                diff: None,
                exit_code: None,
                duration_ms: None,
                extra: BTreeMap::new(),
            })),
            parent: None,
        },
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::ContentDelta {
            item: tool,
            stream: StreamKind::CommandOutput,
            delta: "patching ".to_owned(),
        },
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::ContentDelta {
            item: tool,
            stream: StreamKind::CommandOutput,
            delta: "complete".to_owned(),
        },
    );
    let diff = ToolDiff {
        path: PathBuf::from("src/lib.rs"),
        added: 14,
        removed: 3,
        unified: "@@ -1 +1 @@".to_owned(),
    };
    apply(
        &mut projection,
        &mut events,
        AgentEvent::ItemUpdated {
            item: tool,
            patch: ItemPatch {
                payload: Some(ItemPayloadPatch::Tool(Box::new(ToolPatch {
                    input: Some(json!({"path": "src/lib.rs", "replace_all": true})),
                    summary: Some("src/lib.rs +14 -3".to_owned()),
                    result: Some(json!("updated")),
                    diff: Some(diff.clone()),
                    ..ToolPatch::default()
                }))),
                ..ItemPatch::default()
            },
        },
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TurnSettled {
            turn,
            outcome: TurnOutcome::Completed,
            usage: usage(250),
            duration_ms: 1_200,
            files_changed: vec![
                FileDelta {
                    path: PathBuf::from("src/lib.rs"),
                    added: 14,
                    removed: 3,
                },
                FileDelta {
                    path: PathBuf::from("src/main.rs"),
                    added: 22,
                    removed: 0,
                },
            ],
        },
    );

    let tool = projection
        .items
        .iter()
        .find(|item| item.id == tool)
        .unwrap_or_else(|| panic!("tool item must exist"));
    assert_eq!(tool.status, ItemStatus::Completed);
    assert_eq!(tool_output(tool), Some("patching complete"));
    assert_eq!(tool_diff(tool), Some(&diff));
    assert_eq!(
        projection.turns[0].footer(),
        Some(TurnFooter {
            duration_ms: 1_200,
            tokens: 250,
            files_changed: 2,
            added: 36,
            removed: 3,
        })
    );
}

#[test]
fn permission_gate_opens_mid_turn_and_resolves_independently() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(30);
    let gate = gate_id(31);
    apply(&mut projection, &mut events, session_started());
    start_turn(&mut projection, &mut events, turn, item_id(32), "Run tests");
    apply(
        &mut projection,
        &mut events,
        AgentEvent::GateOpened {
            gate,
            turn: Some(turn),
            kind: permission_gate(),
        },
    );
    assert_eq!(
        projection.attention(projection.last_seq),
        Attention::NeedsYou(AttentionKind::Permission)
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::GateResolved {
            gate,
            answer: GateAnswer::Permission {
                choice: PermissionChoice::AllowOnce,
                edited_payload: None,
            },
            by: GateResolver::User,
        },
    );
    assert!(projection.gates.is_empty());
    assert_eq!(
        projection.attention(projection.last_seq),
        Attention::Working
    );
}

#[test]
fn question_gate_remains_open_after_turn_completion() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(40);
    apply(&mut projection, &mut events, session_started());
    start_turn(
        &mut projection,
        &mut events,
        turn,
        item_id(41),
        "Ask a question",
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::GateOpened {
            gate: gate_id(42),
            turn: Some(turn),
            kind: question_gate(),
        },
    );
    complete_turn(&mut projection, &mut events, turn);

    assert_eq!(projection.gates.len(), 1);
    assert_eq!(
        projection.attention(projection.last_seq),
        Attention::NeedsYou(AttentionKind::Question)
    );
}

#[test]
fn plan_gate_has_its_own_attention_priority() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    apply(&mut projection, &mut events, session_started());
    apply(
        &mut projection,
        &mut events,
        AgentEvent::GateOpened {
            gate: gate_id(50),
            turn: None,
            kind: plan_gate(),
        },
    );
    assert_eq!(
        projection.attention(Seq::default()),
        Attention::NeedsYou(AttentionKind::Plan)
    );
}

#[test]
fn abort_closes_open_text_and_tools_without_inventing_success() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(60);
    let assistant = item_id(62);
    let tool = item_id(63);
    apply(&mut projection, &mut events, session_started());
    start_turn(
        &mut projection,
        &mut events,
        turn,
        item_id(61),
        "Interrupt this",
    );
    for (item, kind) in [
        (
            assistant,
            ItemKind::AssistantText {
                text: String::new(),
            },
        ),
        (
            tool,
            ItemKind::Tool(Box::new(ToolCall {
                kind: ToolKind::Bash,
                name: "Bash".to_owned(),
                input: json!({"command": "sleep 10"}),
                summary: None,
                result: None,
                output: String::new(),
                diff: None,
                exit_code: None,
                duration_ms: None,
                extra: BTreeMap::new(),
            })),
        ),
    ] {
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ItemStarted {
                turn,
                item,
                kind,
                parent: None,
            },
        );
    }
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TurnAborted {
            turn,
            reason: AbortReason::User,
        },
    );

    assert_eq!(
        projection.turn,
        TurnState::Settled(turn, TurnOutcome::Interrupted)
    );
    assert_eq!(projection.items[1].status, ItemStatus::Completed);
    assert_eq!(projection.items[2].status, ItemStatus::Failed);
    assert_eq!(
        projection.turns[0]
            .ended
            .as_ref()
            .map(|ended| &ended.outcome),
        Some(&TurnOutcome::Interrupted)
    );
}

#[test]
fn unexpected_exit_fails_the_active_turn_and_session() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(70);
    let tool = item_id(72);
    apply(&mut projection, &mut events, session_started());
    start_turn(
        &mut projection,
        &mut events,
        turn,
        item_id(71),
        "Start a tool",
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::ItemStarted {
            turn,
            item: tool,
            kind: ItemKind::Tool(Box::new(ToolCall {
                kind: ToolKind::Bash,
                name: "Bash".to_owned(),
                input: json!({}),
                summary: None,
                result: None,
                output: String::new(),
                diff: None,
                exit_code: None,
                duration_ms: None,
                extra: BTreeMap::new(),
            })),
            parent: None,
        },
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::SessionExited {
            code: Some(1),
            expected: false,
        },
    );

    assert_eq!(projection.session, SessionState::Error);
    assert!(matches!(
        projection.turn,
        TurnState::Settled(_, TurnOutcome::Error { .. })
    ));
    assert_eq!(projection.exit_code, Some(1));
    assert_eq!(projection.items[1].status, ItemStatus::Failed);
    assert_eq!(projection.attention(Seq(1)), Attention::Failed);
}

mod behavior;
mod validation;
