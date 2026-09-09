use std::collections::{HashMap, HashSet};

use chrono::{TimeZone, Utc};
use fleet_core::{
    agents::{
        AgentKind, Attention, AttentionKind, FileDelta, GateAnswer, GateId, GateKind, Item, ItemId,
        ItemKind, ItemStatus, ModelSelection, OpenGate, PermissionChoice, PermissionMode,
        PermissionOption, ProviderOptionId, Question, QuestionOption, Seq, ThreadId,
        ThreadProjection, ToolKind, TurnEnd, TurnId, TurnOutcome, TurnRecord, Usage,
    },
    ids::WorktreeId,
};
use fleet_ui_kit::{DecisionCardKind, ToolRowState, TranscriptRow};

use super::{
    decisions::{self, DecisionKey, QuestionSelection},
    presentation::{
        TabBadge, header_word, key_hint_set, metadata_left, metadata_right, tab_badge, tab_title,
    },
    rows::{RowInputs, build_rows},
};

fn worktree() -> WorktreeId {
    "acme/payroll#feat-x"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

fn projection() -> ThreadProjection {
    ThreadProjection::new(ThreadId::new(), worktree(), AgentKind::Claude)
}

fn item(turn: TurnId, kind: ItemKind, status: ItemStatus) -> Item {
    Item {
        id: ItemId::new(),
        turn,
        parent: None,
        kind,
        status,
        text: None,
        summary: None,
        result: None,
        output: None,
        diff: None,
        children: Vec::new(),
        started: Utc.timestamp_opt(1_780_000_000, 0).unwrap(),
        ended: None,
    }
}

fn tool(turn: TurnId, kind: ToolKind, status: ItemStatus, summary: &str) -> Item {
    let mut item = item(
        turn,
        ItemKind::Tool {
            kind,
            name: "Bash".to_owned(),
            input: serde_json::json!({}),
        },
        status,
    );
    item.summary = Some(summary.to_owned());
    item
}

fn completed_turn(id: TurnId, user_item: ItemId, outcome: TurnOutcome) -> TurnRecord {
    TurnRecord {
        id,
        user_item,
        started_at: Utc.timestamp_opt(1_780_000_000, 0).unwrap(),
        blocked_ms: 0,
        ended: Some(TurnEnd {
            outcome,
            usage: Usage {
                total_tokens: 12_400,
                ..Usage::default()
            },
            duration_ms: 48_000,
            files_changed: vec![FileDelta {
                path: "src/lib.rs".into(),
                added: 36,
                removed: 3,
            }],
        }),
    }
}

fn rows_of(projection: &ThreadProjection, queued: &[String]) -> Vec<TranscriptRow> {
    build_rows(&RowInputs {
        projection,
        expanded: &HashSet::new(),
        unfolded: &HashSet::new(),
        queued,
        cards: &[],
    })
    .rows
}

#[test]
fn an_empty_thread_shows_only_its_empty_state() {
    let rows = rows_of(&projection(), &[]);
    assert!(matches!(
        rows.as_slice(),
        [TranscriptRow::EmptyState { message }] if message.contains("claude")
    ));
}

#[test]
fn a_completed_turn_folds_its_successful_work_and_ends_with_one_footer() {
    let mut projection = projection();
    let turn = TurnId::new();
    let user = item(
        turn,
        ItemKind::UserMessage {
            text: "fix the rounding".to_owned(),
            attachments: Vec::new(),
        },
        ItemStatus::Done,
    );
    let mut assistant = item(turn, ItemKind::AssistantText, ItemStatus::Done);
    assistant.text = Some("done".to_owned());
    let ok = tool(turn, ToolKind::Bash, ItemStatus::Done, "cargo test");
    let ok2 = tool(turn, ToolKind::Read, ItemStatus::Done, "src/lib.rs");
    let failed = tool(turn, ToolKind::Bash, ItemStatus::Error, "cargo bench");
    projection.turns = vec![completed_turn(turn, user.id, TurnOutcome::Completed)];
    projection.items = vec![user, assistant, ok, ok2, failed];

    let rows = rows_of(&projection, &[]);
    // §2: only successful rows fold; a failed one stays exposed beside the fold.
    let fold = rows
        .iter()
        .find_map(|row| match row {
            TranscriptRow::WorkedFold { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a settled turn folds its successful work"));
    assert_eq!(fold, "worked 48s \u{b7} 2 tool calls");
    assert!(rows.iter().any(
        |row| matches!(row, TranscriptRow::ToolRow { row, .. } if row.state == ToolRowState::Error)
    ));
    let footer = match rows.last() {
        Some(TranscriptRow::TurnFooter { text }) => text.to_string(),
        other => panic!("a settled turn ends with its footer, got {other:?}"),
    };
    // DESIGN-SYSTEM §6.6: the keycaps belong to the row, so they are not in the string too.
    assert_eq!(
        footer,
        "48s \u{b7} 12.4k tokens \u{b7} 1 file changed +36 \u{2212}3"
    );
}

#[test]
fn the_footer_and_the_fold_both_drop_the_time_the_turn_stood_on_a_gate() {
    let mut projection = projection();
    let turn = TurnId::new();
    let user = item(
        turn,
        ItemKind::UserMessage {
            text: "write the file".to_owned(),
            attachments: Vec::new(),
        },
        ItemStatus::Done,
    );
    let ok = tool(turn, ToolKind::Write, ItemStatus::Done, "smoke.txt");
    let mut record = completed_turn(turn, user.id, TurnOutcome::Completed);
    // The smoke round's numbers: the provider measured 33.9 s of wall clock, of which 28.6 s
    // was the permission card sitting on screen.
    record.blocked_ms = 28_631;
    if let Some(end) = record.ended.as_mut() {
        end.duration_ms = 33_911;
    }
    projection.turns = vec![record];
    projection.items = vec![user, ok];

    let rows = rows_of(&projection, &[]);
    let fold = rows.iter().find_map(|row| match row {
        TranscriptRow::WorkedFold { text, .. } => Some(text.to_string()),
        _ => None,
    });
    assert_eq!(fold.as_deref(), Some("worked 5.3s \u{b7} 1 tool call"));
    assert!(matches!(
        rows.last(),
        Some(TranscriptRow::TurnFooter { text })
            if text == "5.3s \u{b7} 12.4k tokens \u{b7} 1 file changed +36 \u{2212}3"
    ));
}

#[test]
fn an_open_fold_keeps_saying_what_it_folds() {
    let mut projection = projection();
    let turn = TurnId::new();
    let user = item(
        turn,
        ItemKind::UserMessage {
            text: "fix the rounding".to_owned(),
            attachments: Vec::new(),
        },
        ItemStatus::Done,
    );
    let ok = tool(turn, ToolKind::Bash, ItemStatus::Done, "cargo test");
    let ok2 = tool(turn, ToolKind::Read, ItemStatus::Done, "src/lib.rs");
    projection.turns = vec![completed_turn(turn, user.id, TurnOutcome::Completed)];
    projection.items = vec![user, ok, ok2];

    let unfolded = HashSet::from([turn]);
    let rows = build_rows(&RowInputs {
        projection: &projection,
        expanded: &HashSet::new(),
        unfolded: &unfolded,
        queued: &[],
        cards: &[],
    })
    .rows;
    // §2: only the `[⏎]` label changes between show and hide.
    assert!(matches!(
        rows.iter().find_map(|row| match row {
            TranscriptRow::WorkedFold { text, expanded } => Some((text.to_string(), *expanded)),
            _ => None,
        }),
        Some((text, true)) if text == "worked 48s \u{b7} 2 tool calls"
    ));
}

#[test]
fn a_reasoning_row_reads_as_a_duration_and_keeps_its_text_for_the_body() {
    let mut projection = projection();
    let turn = TurnId::new();
    let mut thinking = item(turn, ItemKind::Thinking, ItemStatus::Done);
    thinking.text = Some("the spec says half-up; the reducer truncates".to_owned());
    thinking.ended = Some(Utc.timestamp_opt(1_780_000_006, 0).unwrap());
    projection.items = vec![thinking];

    let rows = rows_of(&projection, &[]);
    assert!(matches!(
        rows.first(),
        Some(TranscriptRow::Thinking {
            text,
            duration_ms: 6_000,
            ..
        }) if text.starts_with("the spec")
    ));
}

#[test]
fn compaction_and_resume_boundaries_are_their_own_rows() {
    let mut projection = projection();
    let turn = TurnId::new();
    let user = item(
        turn,
        ItemKind::UserMessage {
            text: "go".to_owned(),
            attachments: Vec::new(),
        },
        ItemStatus::Done,
    );
    projection.turns = vec![completed_turn(turn, user.id, TurnOutcome::Completed)];
    projection.items = vec![user];
    projection.checkpoints = vec![
        fleet_core::agents::CheckpointRecord {
            kind: fleet_core::agents::CheckpointKind::Resumed { age_ms: 7_200_000 },
            seq: Seq(1),
            after_turn: None,
        },
        fleet_core::agents::CheckpointRecord {
            kind: fleet_core::agents::CheckpointKind::CompactBoundary {
                before: 84_000,
                after: Some(12_000),
            },
            seq: Seq(9),
            after_turn: Some(turn),
        },
    ];

    let rows = rows_of(&projection, &[]);
    let lines = rows
        .iter()
        .filter_map(|row| match row {
            TranscriptRow::CheckpointLine { text } => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        [
            "session resumed \u{b7} 2h ago",
            "context compacted \u{b7} 84k \u{2192} 12k tokens"
        ]
    );
    assert!(matches!(
        rows.first(),
        Some(TranscriptRow::CheckpointLine { .. })
    ));
}

#[test]
fn viewing_a_plan_answers_nothing_and_a_provider_without_edit_offers_no_e() {
    let plan = OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Plan {
            markdown: "# Plan\n\n- inspect".to_owned(),
            steps: vec!["inspect".to_owned()],
        },
        opened_seq: Seq(2),
        blocked_since: None,
    };
    assert_eq!(
        decisions::answer_for(
            &plan,
            DecisionKey::ViewPlan,
            &mut QuestionSelection::default(),
            None
        ),
        None,
        "the plan card expands locally; the gate stays open"
    );
    let expanded = decisions::card(
        &plan,
        AgentKind::Claude,
        &QuestionSelection::default(),
        true,
    );
    assert!(expanded.expanded);

    let opencode = OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Permission {
            tool: ToolKind::Bash,
            title: "run a command".to_owned(),
            payload: "rm -rf build".to_owned(),
            rationale: None,
            options: vec![PermissionOption {
                id: ProviderOptionId("once".to_owned()),
                label: PermissionChoice::AllowOnce,
            }],
        },
        opened_seq: Seq(3),
        blocked_since: None,
    };
    let card = decisions::card(
        &opencode,
        AgentKind::OpenCode,
        &QuestionSelection::default(),
        false,
    );
    assert!(
        !card.actions.iter().any(|action| action.key == "e"),
        "a provider that cannot carry an edited command does not offer one"
    );
}

#[test]
fn a_permission_card_states_its_target_once() {
    let write = |rationale: Option<&str>| OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Permission {
            tool: ToolKind::Write,
            title: "Claude wants to use Write".to_owned(),
            payload: "/tmp/work/smoke-7a8f27.txt\n+ok".to_owned(),
            rationale: rationale.map(ToOwned::to_owned),
            options: Vec::new(),
        },
        opened_seq: Seq(5),
        blocked_since: None,
    };
    let rationale_of = |gate: &OpenGate| match decisions::card(
        gate,
        AgentKind::Claude,
        &QuestionSelection::default(),
        false,
    )
    .kind
    {
        fleet_ui_kit::DecisionCardKind::Permission { rationale, .. } => rationale,
        other => panic!("a permission gate builds a permission card, got {other:?}"),
    };
    // §2's card is title + payload + keys: the model's `description` earns its own line only
    // by adding something the two already-shown strings do not say (r2-04 repeated the file
    // name three times).
    assert_eq!(rationale_of(&write(Some("smoke-7a8f27.txt"))), None);
    assert_eq!(rationale_of(&write(Some("  "))), None);
    assert_eq!(rationale_of(&write(Some("Claude wants to use"))), None);
    assert_eq!(
        rationale_of(&write(Some("record the smoke marker"))).as_deref(),
        Some("record the smoke marker")
    );
}

#[test]
fn an_interrupted_turn_says_stopped_and_raises_no_error_card() {
    let mut projection = projection();
    let turn = TurnId::new();
    let user = item(
        turn,
        ItemKind::UserMessage {
            text: "go".to_owned(),
            attachments: Vec::new(),
        },
        ItemStatus::Done,
    );
    projection.turns = vec![completed_turn(turn, user.id, TurnOutcome::Interrupted)];
    projection.items = vec![user];

    let rows = rows_of(&projection, &[]);
    assert!(
        !rows
            .iter()
            .any(|row| matches!(row, TranscriptRow::ErrorCard { .. })),
        "§11: an interrupted turn has a footer, never an error card"
    );
    assert!(matches!(
        rows.last(),
        Some(TranscriptRow::TurnFooter { text }) if text.starts_with("stopped \u{b7} 48s")
    ));
}

#[test]
fn queued_messages_precede_the_decision_card_and_the_card_is_last() {
    let mut projection = projection();
    let gate = OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Permission {
            tool: ToolKind::Bash,
            title: "run a command".to_owned(),
            payload: "rm -rf build".to_owned(),
            rationale: None,
            options: Vec::new(),
        },
        opened_seq: Seq(4),
        blocked_since: None,
    };
    projection.gates = vec![gate.clone()];
    let card = decisions::card(
        &gate,
        AgentKind::Claude,
        &QuestionSelection::default(),
        false,
    );
    let rows = build_rows(&RowInputs {
        projection: &projection,
        expanded: &HashSet::new(),
        unfolded: &HashSet::new(),
        queued: &["later".to_owned()],
        cards: &[card],
    })
    .rows;
    assert!(matches!(
        rows.as_slice(),
        [
            TranscriptRow::QueuedMessage { .. },
            TranscriptRow::DecisionCard(_)
        ]
    ));
}

#[test]
fn a_retrying_provider_raises_an_error_card() {
    let mut projection = projection();
    projection.retrying = Some(fleet_core::agents::RetryState {
        attempt: 2,
        retry_in_ms: 3_000,
        reason: "rate limit".to_owned(),
    });
    let rows = rows_of(&projection, &[]);
    assert!(matches!(
        rows.first(),
        Some(TranscriptRow::ErrorCard { message, retrying: true }) if message.contains("attempt 2")
    ));
}

#[test]
fn a_user_block_carries_the_attachments_the_message_was_sent_with() {
    use fleet_core::agents::{Attachment, AttachmentSource};

    let mut projection = projection();
    let turn = TurnId::new();
    projection.items.push(item(
        turn,
        ItemKind::UserMessage {
            text: "look at these".to_owned(),
            attachments: vec![
                Attachment {
                    name: Some("spec.md".to_owned()),
                    media_type: "text/markdown".to_owned(),
                    source: AttachmentSource::Base64("Zm9v".to_owned()),
                },
                Attachment {
                    name: None,
                    media_type: "text/x-rust".to_owned(),
                    source: AttachmentSource::Path("/repo/crates/core/src/lib.rs".into()),
                },
                Attachment {
                    name: None,
                    media_type: "image/png".to_owned(),
                    source: AttachmentSource::Url("https://example.test/a.png".to_owned()),
                },
            ],
        },
        ItemStatus::Done,
    ));
    let rows = rows_of(&projection, &[]);
    let Some(TranscriptRow::UserBlock { attachments, .. }) = rows.first() else {
        panic!("expected a user block, got {rows:?}");
    };
    // The provider's name wins; a path falls back to its file name and a URL to its media type,
    // because neither a data URI nor an absolute path fits on a 22 px pill.
    assert_eq!(
        attachments.as_slice(),
        ["spec.md", "lib.rs", "image/png"].map(gpui::SharedString::from)
    );
}

#[test]
fn the_tab_badge_follows_the_attention_table() {
    assert_eq!(
        tab_badge(Attention::Working, None),
        TabBadge::Spinner,
        "working is the gray spinner"
    );
    for kind in [
        AttentionKind::Permission,
        AttentionKind::Question,
        AttentionKind::Plan,
        AttentionKind::Finished,
    ] {
        assert_eq!(
            tab_badge(Attention::NeedsYou(kind), None),
            TabBadge::NeedsYou
        );
    }
    assert_eq!(tab_badge(Attention::Unread, None), TabBadge::Unread);
    assert_eq!(
        tab_badge(Attention::Failed, Some(1)),
        TabBadge::Exited(Some(1))
    );
    assert_eq!(tab_badge(Attention::Idle, None), TabBadge::None);

    assert_eq!(header_word(Attention::Working), "working");
    assert_eq!(
        header_word(Attention::NeedsYou(AttentionKind::Plan)),
        "needs you"
    );
    assert_eq!(header_word(Attention::Failed), "failed");
    // §3.3: `unread` has no header word of its own; the header stays idle.
    assert_eq!(header_word(Attention::Unread), "idle");
}

#[test]
fn the_metadata_row_spells_out_the_mode_for_both_providers() {
    let mut projection = projection();
    projection.model = Some(ModelSelection {
        model: "claude-sonnet-5".to_owned(),
        effort: Some("high".to_owned()),
        provider: None,
    });
    projection.context_pct = 34.0;
    projection.cumulative_cost_usd = Some(0.42);
    assert_eq!(
        metadata_left(&projection),
        "agent mode \u{b7} claude-sonnet-5 \u{b7} high \u{b7} asks before edits"
    );
    assert!(metadata_right(&projection).starts_with("context 34% \u{b7} $0.42"));

    projection.provider = AgentKind::OpenCode;
    assert!(metadata_left(&projection).starts_with("build agent"));
    projection.mode = PermissionMode::Plan;
    assert!(metadata_left(&projection).starts_with("plan agent"));
}

/// S7: OpenCode reports no cost, and dropping the element gave the two providers two different
/// rows. §2 fixes one shape for both, so the slot stays and says "not reported".
#[test]
fn the_metadata_row_keeps_its_shape_when_a_provider_reports_no_cost() {
    let mut projection = projection();
    projection.context_pct = 2.0;
    projection.turns.push(TurnRecord {
        id: TurnId::new(),
        user_item: ItemId::new(),
        started_at: Utc.timestamp_opt(1_780_000_000, 0).unwrap(),
        blocked_ms: 0,
        ended: None,
    });
    projection.last_activity = Some(Utc.timestamp_opt(1_780_000_003, 400_000_000).unwrap());

    projection.cumulative_cost_usd = Some(0.42);
    assert_eq!(
        metadata_right(&projection),
        "context 2% \u{b7} $0.42 \u{b7} 3.4s"
    );

    projection.cumulative_cost_usd = None;
    assert_eq!(
        metadata_right(&projection),
        "context 2% \u{b7} \u{2014} \u{b7} 3.4s",
        "\u{a7}2 fixes the right side at three elements for both providers"
    );

    // An empty thread still says nothing rather than a lone em dash.
    let empty = ThreadProjection::new(ThreadId::new(), worktree(), AgentKind::Claude);
    assert_eq!(metadata_right(&empty), String::new());
}

#[test]
fn a_permission_card_offers_the_widest_scope_the_provider_can_map() {
    let directory = OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Permission {
            tool: ToolKind::Bash,
            title: "run a command".to_owned(),
            payload: "ls".to_owned(),
            rationale: None,
            options: vec![PermissionOption {
                id: ProviderOptionId("always".to_owned()),
                label: PermissionChoice::AllowDirectory,
            }],
        },
        opened_seq: Seq(1),
        blocked_since: None,
    };
    let card = decisions::card(
        &directory,
        AgentKind::OpenCode,
        &QuestionSelection::default(),
        false,
    );
    assert!(matches!(card.kind, DecisionCardKind::Permission { .. }));
    assert!(
        card.actions
            .iter()
            .any(|option| option.label == "allow for this directory"),
        "§4.2: OpenCode stores `always` per directory, so the copy says so"
    );

    let mut selection = QuestionSelection::default();
    assert_eq!(
        decisions::answer_for(&directory, DecisionKey::AllowSession, &mut selection, None),
        Some(GateAnswer::Permission {
            choice: PermissionChoice::AllowDirectory,
            edited_payload: None,
        })
    );
    // A provider that offers no wider scope can never be widened by `a`.
    let narrow = OpenGate {
        kind: GateKind::Permission {
            tool: ToolKind::Bash,
            title: "run a command".to_owned(),
            payload: "ls".to_owned(),
            rationale: None,
            options: Vec::new(),
        },
        ..directory.clone()
    };
    assert_eq!(
        decisions::answer_for(&narrow, DecisionKey::AllowSession, &mut selection, None),
        Some(GateAnswer::Permission {
            choice: PermissionChoice::AllowOnce,
            edited_payload: None,
        })
    );
}

#[test]
fn question_keys_select_before_they_answer() {
    let gate = OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Question {
            questions: vec![Question {
                text: "Which crates?".to_owned(),
                header: "scope".to_owned(),
                options: vec![
                    QuestionOption {
                        label: "core".to_owned(),
                        description: String::new(),
                    },
                    QuestionOption {
                        label: "app".to_owned(),
                        description: String::new(),
                    },
                ],
                multi_select: true,
                allow_other: false,
            }],
        },
        opened_seq: Seq(2),
        blocked_since: None,
    };
    let mut selection = QuestionSelection::new(1);
    assert_eq!(
        decisions::answer_for(&gate, DecisionKey::Answer, &mut selection, None),
        None,
        "§9: `⏎` cannot answer a question nothing has been chosen for"
    );
    assert_eq!(
        decisions::answer_for(&gate, DecisionKey::Choose(0), &mut selection, None),
        None,
        "a multi-select choice only toggles"
    );
    assert_eq!(
        decisions::answer_for(&gate, DecisionKey::Choose(1), &mut selection, None),
        None
    );
    assert_eq!(
        decisions::answer_for(&gate, DecisionKey::Answer, &mut selection, None),
        Some(GateAnswer::Question {
            answers: vec![vec!["core".to_owned(), "app".to_owned()]],
        })
    );
}

#[test]
fn a_plan_card_routes_y_and_n_and_never_answers_a_permission_key() {
    let gate = OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Plan {
            markdown: "# plan".to_owned(),
            steps: vec!["extract".to_owned()],
        },
        opened_seq: Seq(3),
        blocked_since: None,
    };
    let mut selection = QuestionSelection::default();
    assert_eq!(
        decisions::answer_for(&gate, DecisionKey::ApprovePlan, &mut selection, None),
        Some(GateAnswer::Plan(fleet_core::agents::PlanAnswer::Approve))
    );
    assert_eq!(
        decisions::answer_for(
            &gate,
            DecisionKey::AskChanges,
            &mut selection,
            Some("smaller steps".to_owned())
        ),
        Some(GateAnswer::Plan(
            fleet_core::agents::PlanAnswer::AskForChanges {
                note: "smaller steps".to_owned()
            }
        ))
    );
    assert_eq!(
        decisions::answer_for(&gate, DecisionKey::AllowOnce, &mut selection, None),
        None,
        "a permission key must never resolve a plan gate"
    );
    assert_eq!(decisions::decision_context(&gate), "AgentPlan");
}

#[test]
fn the_other_option_answers_with_the_text_the_composer_carries() {
    let gate = OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Question {
            questions: vec![Question {
                text: "Which crates?".to_owned(),
                header: "scope".to_owned(),
                options: vec![QuestionOption {
                    label: "core".to_owned(),
                    description: String::new(),
                }],
                multi_select: false,
                allow_other: true,
            }],
        },
        opened_seq: Seq(2),
        blocked_since: None,
    };

    // §3.2: the "other" option is free text keyed by the question. Choosing it with nothing
    // typed is not an answer — the card stays open rather than sending an empty array the user
    // was shown as their choice.
    let mut selection = QuestionSelection::new(1);
    decisions::answer_for(&gate, DecisionKey::Choose(1), &mut selection, None);
    assert_eq!(
        decisions::answer_for(&gate, DecisionKey::Answer, &mut selection, None),
        None
    );
    assert_eq!(
        decisions::answer_for(
            &gate,
            DecisionKey::Answer,
            &mut selection,
            Some("neither, split the crate".to_owned())
        ),
        Some(GateAnswer::Question {
            answers: vec![vec!["neither, split the crate".to_owned()]],
        })
    );
}

/// UX-1: a subagent's text is drawn inside its tool row, so it owns no top-level row.
#[test]
fn a_nested_item_never_consumes_a_top_level_streaming_row() {
    let mut projection = projection();
    let turn = TurnId::new();
    let user = item(
        turn,
        ItemKind::UserMessage {
            text: "go".to_owned(),
            attachments: Vec::new(),
        },
        ItemStatus::Done,
    );
    let tool = tool(turn, ToolKind::Agent, ItemStatus::Running, "explore");
    let mut nested = item(turn, ItemKind::AssistantText, ItemStatus::Running);
    nested.parent = Some(tool.id);
    nested.text = Some("subagent prose".to_owned());
    let mut top = item(turn, ItemKind::AssistantText, ItemStatus::Running);
    top.text = Some("main prose".to_owned());
    let top_id = top.id;
    projection.turns.push(TurnRecord {
        id: turn,
        user_item: user.id,
        started_at: Utc.timestamp_opt(1_780_000_000, 0).unwrap(),
        blocked_ms: 0,
        ended: None,
    });
    projection.items = vec![user, tool, nested, top];

    let rows = rows_of(&projection, &[]);
    let index = super::index_streaming_rows(&projection, &rows);
    let position = index
        .get(&top_id)
        .copied()
        .expect("the top-level paragraph has a row");
    assert!(
        matches!(
            rows.get(position),
            Some(TranscriptRow::AssistantText { .. })
        ),
        "the streaming index paired the wrong row"
    );
    assert_eq!(index.len(), 1, "only top-level items own a row");
}

/// UX-4: `1`-`4` are bound unconditionally; only the digits the question has may be stored.
#[test]
fn a_digit_the_question_does_not_offer_never_wedges_the_card() {
    let gate = OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Question {
            questions: vec![Question {
                text: "Which crates?".to_owned(),
                header: "scope".to_owned(),
                options: vec![
                    QuestionOption {
                        label: "core".to_owned(),
                        description: String::new(),
                    },
                    QuestionOption {
                        label: "app".to_owned(),
                        description: String::new(),
                    },
                ],
                multi_select: false,
                allow_other: false,
            }],
        },
        opened_seq: Seq(2),
        blocked_since: None,
    };
    let mut selection = QuestionSelection::new(1);
    decisions::answer_for(&gate, DecisionKey::Choose(2), &mut selection, None);
    assert!(
        selection.selected(0).is_empty(),
        "a `3` on a two-option question stored an answer no label matches"
    );
    // The card is still answerable afterwards.
    decisions::answer_for(&gate, DecisionKey::Choose(1), &mut selection, None);
    assert_eq!(
        decisions::answer_for(&gate, DecisionKey::Answer, &mut selection, None),
        Some(GateAnswer::Question {
            answers: vec![vec!["app".to_owned()]],
        })
    );
}

/// UX-3: the chosen options travel with the card, or the keys change invisible state.
#[test]
fn a_question_card_carries_the_selection_the_keys_made() {
    let gate = OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Question {
            questions: vec![Question {
                text: "Which crates?".to_owned(),
                header: "scope".to_owned(),
                options: vec![
                    QuestionOption {
                        label: "core".to_owned(),
                        description: String::new(),
                    },
                    QuestionOption {
                        label: "app".to_owned(),
                        description: String::new(),
                    },
                ],
                multi_select: true,
                allow_other: false,
            }],
        },
        opened_seq: Seq(2),
        blocked_since: None,
    };
    let mut selection = QuestionSelection::new(1);
    decisions::answer_for(&gate, DecisionKey::Choose(1), &mut selection, None);
    let card = decisions::card(&gate, AgentKind::Claude, &selection, false);
    assert_eq!(card.selected, vec![vec![1]]);
}

#[test]
fn the_status_hints_change_with_the_open_card() {
    let idle = key_hint_set(false);
    assert_eq!(idle.first(), Some(&("\u{23ce}", "send")));
    let working = key_hint_set(true);
    assert_eq!(working.first(), Some(&("esc", "stop")));
    // §9: scroll mode and the terminal fallback have no other surface, so both rows carry them.
    for set in [idle, working] {
        assert!(set.contains(&("^s [", "scroll")), "{set:?}");
        assert!(set.contains(&("^s F", "terminal")), "{set:?}");
    }
}

#[test]
fn the_status_bar_mirrors_the_open_card_including_its_scope() {
    fn labels(gate: &OpenGate, provider: AgentKind) -> Vec<(String, String)> {
        // `decision_hints` renders exactly this list, so asserting the card asserts the row the
        // status bar draws from it.
        decisions::card(gate, provider, &QuestionSelection::default(), false)
            .actions
            .iter()
            .map(|option| (option.key.to_string(), option.label.to_string()))
            .collect()
    }

    let permission = OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Permission {
            tool: ToolKind::Bash,
            title: "run a command".to_owned(),
            payload: "cargo test".to_owned(),
            rationale: None,
            options: vec![PermissionOption {
                id: ProviderOptionId("allow_session".to_owned()),
                label: PermissionChoice::AllowSession,
            }],
        },
        opened_seq: Seq(2),
        blocked_since: None,
    };
    assert!(
        labels(&permission, AgentKind::Claude)
            .contains(&("a".to_owned(), "allow for this session".to_owned())),
        "the mirrored row spells out the scope `a` grants"
    );

    let question = OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Question {
            questions: vec![Question {
                text: "Which fixture?".to_owned(),
                header: "fixture".to_owned(),
                options: vec![
                    QuestionOption {
                        label: "first".to_owned(),
                        description: String::new(),
                    },
                    QuestionOption {
                        label: "second".to_owned(),
                        description: String::new(),
                    },
                ],
                multi_select: false,
                allow_other: false,
            }],
        },
        opened_seq: Seq(3),
        blocked_since: None,
    };
    let keys: Vec<String> = labels(&question, AgentKind::Claude)
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    // DESIGN-SYSTEM §4: a two-option single-select question never lists `3`, `4` or `space`.
    assert!(keys.contains(&"1\u{2013}2".to_owned()), "{keys:?}");
    assert!(!keys.contains(&"space".to_owned()), "{keys:?}");
}

#[test]
fn an_untitled_thread_is_named_by_its_provider_alone() {
    let mut summary = projection().summary(Seq::default());
    assert_eq!(tab_title(&summary), "claude");
    summary.title = "rounding fix".to_owned();
    assert_eq!(tab_title(&summary), "claude \u{2014} rounding fix");
}

#[test]
fn a_gate_names_the_decision_context_that_owns_the_keys() {
    for (kind, context) in [
        (
            GateKind::Permission {
                tool: ToolKind::Bash,
                title: String::new(),
                payload: String::new(),
                rationale: None,
                options: Vec::new(),
            },
            "AgentPermission",
        ),
        (
            GateKind::Question {
                questions: Vec::new(),
            },
            "AgentQuestion",
        ),
        (
            GateKind::Plan {
                markdown: String::new(),
                steps: Vec::new(),
            },
            "AgentPlan",
        ),
    ] {
        let gate = OpenGate {
            id: GateId::new(),
            turn: None,
            kind,
            opened_seq: Seq(1),
            blocked_since: None,
        };
        assert_eq!(decisions::decision_context(&gate), context);
    }
}

/// UX-3: the fold stands in for the work, so it belongs where the work was — not below the
/// answer the model wrote after it.
#[test]
fn the_worked_fold_stands_where_the_folded_work_was() {
    let mut projection = projection();
    let turn = TurnId::new();
    let user = item(
        turn,
        ItemKind::UserMessage {
            text: "fix the rounding".to_owned(),
            attachments: Vec::new(),
        },
        ItemStatus::Done,
    );
    let ok = tool(turn, ToolKind::Bash, ItemStatus::Done, "cargo test");
    let mut closing = item(turn, ItemKind::AssistantText, ItemStatus::Done);
    closing.text = Some("all green".to_owned());
    projection.turns = vec![completed_turn(turn, user.id, TurnOutcome::Completed)];
    projection.items = vec![user, ok, closing];

    let rows = rows_of(&projection, &[]);
    let fold = rows
        .iter()
        .position(|row| matches!(row, TranscriptRow::WorkedFold { .. }))
        .unwrap_or_else(|| panic!("a settled turn folds its successful work"));
    let answer = rows
        .iter()
        .position(|row| matches!(row, TranscriptRow::AssistantText { .. }))
        .unwrap_or_else(|| panic!("the closing paragraph is a row"));
    assert!(
        fold < answer,
        "the fold landed below the answer the tools ran for: {rows:?}"
    );
}

/// UX-3: the anchors still address the rows they were built for after the fold is spliced in.
#[test]
fn splicing_the_fold_keeps_every_row_anchor_on_its_own_row() {
    let mut projection = projection();
    let turn = TurnId::new();
    let user = item(
        turn,
        ItemKind::UserMessage {
            text: "go".to_owned(),
            attachments: Vec::new(),
        },
        ItemStatus::Done,
    );
    let ok = tool(turn, ToolKind::Bash, ItemStatus::Done, "cargo test");
    let mut thinking = item(turn, ItemKind::Thinking, ItemStatus::Done);
    thinking.text = Some("the reducer truncates".to_owned());
    thinking.ended = Some(Utc.timestamp_opt(1_780_000_006, 0).unwrap());
    let thinking_id = thinking.id;
    projection.turns = vec![completed_turn(turn, user.id, TurnOutcome::Completed)];
    projection.items = vec![user, ok, thinking];

    let built = build_rows(&RowInputs {
        projection: &projection,
        expanded: &HashSet::new(),
        unfolded: &HashSet::new(),
        queued: &[],
        cards: &[],
    });
    let fold = built
        .anchors
        .folds
        .keys()
        .copied()
        .next()
        .unwrap_or_else(|| panic!("the fold is anchored"));
    assert!(matches!(
        built.rows.get(fold),
        Some(TranscriptRow::WorkedFold { .. })
    ));
    let reasoning = built
        .anchors
        .items
        .iter()
        .find(|(_, item)| **item == thinking_id)
        .map(|(index, _)| *index)
        .unwrap_or_else(|| panic!("the reasoning row is anchored"));
    assert!(matches!(
        built.rows.get(reasoning),
        Some(TranscriptRow::Thinking { .. })
    ));
}

/// BH-4: Claude opens a thinking block for a signature-only frame and settles it with no text.
#[test]
fn a_settled_item_with_no_text_is_not_a_row() {
    let mut projection = projection();
    let turn = TurnId::new();
    let empty = item(turn, ItemKind::Thinking, ItemStatus::Done);
    let mut blank_prose = item(turn, ItemKind::AssistantText, ItemStatus::Done);
    blank_prose.text = Some("   ".to_owned());
    // A block still streaming has not settled, so it keeps its row.
    let running = item(turn, ItemKind::Thinking, ItemStatus::Running);
    projection.items = vec![empty, blank_prose, running];

    let rows = rows_of(&projection, &[]);
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row, TranscriptRow::Thinking { .. }))
            .count(),
        1,
        "an empty settled fold expanded to nothing: {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|row| matches!(row, TranscriptRow::AssistantText { .. })),
        "{rows:?}"
    );
}

/// UX-1: a `Notice` or a `Checkpoint` alone must not be mistaken for a text delta.
#[test]
fn a_row_bearing_change_is_never_a_streaming_only_change() {
    use fleet_core::agents::{CheckpointKind, CheckpointRecord, NoticeRecord};

    let current = projection();
    let mut with_notice = current.clone();
    with_notice.notices.push(NoticeRecord {
        text: "compacting the transcript".to_owned(),
        seq: Seq(2),
        after_turn: None,
    });
    assert!(
        !super::streaming_only_change(&current, &with_notice, &HashMap::new()),
        "a notice row would never have appeared"
    );

    let mut with_checkpoint = current.clone();
    with_checkpoint.checkpoints.push(CheckpointRecord {
        kind: CheckpointKind::Resumed { age_ms: 1_000 },
        seq: Seq(2),
        after_turn: None,
    });
    assert!(!super::streaming_only_change(
        &current,
        &with_checkpoint,
        &HashMap::new()
    ));

    let mut with_task = current.clone();
    with_task.background_tasks = vec![ItemId::new()];
    assert!(!super::streaming_only_change(
        &current,
        &with_task,
        &HashMap::new()
    ));

    // A resolved gate replaced by a new one in the same frame keeps the count at one.
    let permission = |payload: &str| OpenGate {
        id: GateId::new(),
        turn: None,
        kind: GateKind::Permission {
            tool: ToolKind::Bash,
            title: "run a command".to_owned(),
            payload: payload.to_owned(),
            rationale: None,
            options: Vec::new(),
        },
        opened_seq: Seq(2),
        blocked_since: None,
    };
    let mut first = current.clone();
    first.gates.push(permission("ls"));
    let mut second = current.clone();
    second.gates.push(permission("rm -rf target"));
    assert!(!super::streaming_only_change(
        &first,
        &second,
        &HashMap::new()
    ));
}

/// UX-4: DESIGN-SYSTEM §4 gives a list under a text field both directions.
#[test]
fn the_completion_picker_moves_both_ways() {
    use super::picker::{Picker, PickerKind};

    let mut picker = Picker::new(
        PickerKind::Commands,
        vec!["init".to_owned(), "review".to_owned(), "ship".to_owned()],
    );
    assert_eq!(picker.highlight(), 0);
    picker.advance();
    assert_eq!(picker.highlight(), 1);
    picker.retreat();
    assert_eq!(picker.highlight(), 0);
    // Both directions wrap, so neither key is a dead end at an edge.
    picker.retreat();
    assert_eq!(picker.highlight(), 2);
    picker.advance();
    assert_eq!(picker.highlight(), 0);

    let mut empty = Picker::new(PickerKind::Commands, Vec::new());
    empty.retreat();
    assert_eq!(empty.highlight(), 0);
}

/// UX-2: `MultilineInput::submit` empties the buffer before the event is delivered, so the
/// owner has to consume the reported text (DESIGN-SYSTEM §6.6). `enter` is unbound in
/// `Agent > AgentDecision > AgentPermission` and `Agent > AgentNativeScroll`, where the key
/// reaches the composer and this is the only path the draft survives.
#[gpui::test]
fn a_composer_submit_sends_the_text_it_reported(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    use crate::bridge::BridgeCommand;
    use gpui::AppContext as _;

    use super::{AgentThreadEvent, AgentThreadView};

    let view = cx.new(|cx| AgentThreadView::new(projection(), cx));
    let sent: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&sent);
    cx.update(|cx| {
        cx.subscribe(&view, move |_, event, _| {
            if let AgentThreadEvent::Command(BridgeCommand::AgentSend { input, .. }) = event {
                seen.borrow_mut().push(input.text.clone());
            }
        })
        .detach();
    });

    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| {
            input.set_text("ship it", cx);
            input.submit(cx);
        });
    });
    cx.run_until_parked();

    assert_eq!(sent.borrow().as_slice(), ["ship it".to_owned()]);
    view.read_with(cx, |view, cx| {
        assert!(view.input().read(cx).text().is_empty());
    });
}

/// S4: `esc` is unbound in `AgentIdle`, so it reaches the composer and comes back as
/// `MultilineInputEvent::Escape`. With nothing running, asking the daemon to interrupt earned a
/// sticky `conflict: agent thread … has no active turn` for a key that meant "never mind".
#[gpui::test]
fn escape_on_an_idle_thread_interrupts_nothing(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    use crate::bridge::BridgeCommand;
    use gpui::AppContext as _;

    use super::{AgentThreadEvent, AgentThreadView};

    let mut idle = projection();
    idle.session = fleet_core::agents::SessionState::Ready;
    let view = cx.new(|cx| AgentThreadView::new(idle, cx));
    let interrupts: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let seen = Rc::clone(&interrupts);
    cx.update(|cx| {
        cx.subscribe(&view, move |_, event, _| {
            if matches!(
                event,
                AgentThreadEvent::Command(BridgeCommand::AgentInterrupt { .. })
            ) {
                *seen.borrow_mut() += 1;
            }
        })
        .detach();
    });

    view.update(cx, AgentThreadView::stop);
    cx.run_until_parked();
    assert_eq!(*interrupts.borrow(), 0);

    // A turn that really is running is still interruptible.
    view.update(cx, |view, cx| {
        let mut next = view.projection().clone();
        next.turn = fleet_core::agents::TurnState::Running(TurnId::new());
        view.set_projection(next, cx);
    });
    view.update(cx, AgentThreadView::stop);
    cx.run_until_parked();
    assert_eq!(*interrupts.borrow(), 1);
}

/// UX-10: `⇧⇥` is a round trip; a thread on `full access` must not come back as `ask`.
#[gpui::test]
fn leaving_plan_mode_restores_the_mode_it_was_entered_from(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    use crate::bridge::BridgeCommand;
    use gpui::AppContext as _;

    use super::{AgentThreadEvent, AgentThreadView};

    let mut projection = projection();
    projection.mode = PermissionMode::FullAccess;
    let view = cx.new(|cx| AgentThreadView::new(projection, cx));
    let modes: Rc<RefCell<Vec<PermissionMode>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&modes);
    cx.update(|cx| {
        cx.subscribe(&view, move |_, event, _| {
            if let AgentThreadEvent::Command(BridgeCommand::AgentSetMode { mode, .. }) = event {
                seen.borrow_mut().push(*mode);
            }
        })
        .detach();
    });

    view.update(cx, AgentThreadView::toggle_plan_mode);
    cx.run_until_parked();
    // The daemon echoes the mode back through the projection, which is what the row reads.
    view.update(cx, |view, cx| {
        let mut next = view.projection().clone();
        next.mode = PermissionMode::Plan;
        view.set_projection(next, cx);
    });
    view.update(cx, AgentThreadView::toggle_plan_mode);
    cx.run_until_parked();

    assert_eq!(
        modes.borrow().as_slice(),
        [PermissionMode::Plan, PermissionMode::FullAccess],
        "leaving plan mode downgraded the thread instead of restoring it"
    );
}

/// P3-T04: a thread whose machine is unreachable refuses `⏎` *before* the send, keeps the
/// draft the composer reported, and says which machine it is waiting for.
#[gpui::test]
fn an_unreachable_host_stands_the_composer_down_and_keeps_the_draft(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    use crate::bridge::BridgeCommand;
    use gpui::AppContext as _;

    use super::{AgentThreadEvent, AgentThreadView, ThreadHost};

    let view = cx.new(|cx| AgentThreadView::new(projection(), cx));
    let sent: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let notices: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let (seen, heard) = (Rc::clone(&sent), Rc::clone(&notices));
    cx.update(|cx| {
        cx.subscribe(&view, move |_, event, _| match event {
            AgentThreadEvent::Command(BridgeCommand::AgentSend { input, .. }) => {
                seen.borrow_mut().push(input.text.clone());
            }
            AgentThreadEvent::Notice(text) => heard.borrow_mut().push(text.to_string()),
            _ => {}
        })
        .detach();
    });

    view.update(cx, |view, cx| {
        view.set_host(
            Some(ThreadHost {
                name: "dev-box".into(),
                unreachable: true,
            }),
            cx,
        );
        let input = view.input().clone();
        input.update(cx, |input, cx| {
            input.set_text("ship it", cx);
            input.submit(cx);
        });
    });
    cx.run_until_parked();

    view.read_with(cx, |view, cx| {
        assert!(view.is_unreachable());
        assert!(view.input().read(cx).is_read_only());
        assert_eq!(
            view.host().map(|host| host.name.to_string()),
            Some("dev-box".to_owned()),
            "the badge names the machine the thread runs on"
        );
        assert_eq!(
            view.input().read(cx).text(),
            "ship it",
            "the draft the composer reported survives the refusal"
        );
    });
    assert!(
        sent.borrow().is_empty(),
        "nothing may be dispatched to a machine the daemon has no link to"
    );
    assert!(
        notices.borrow().is_empty(),
        "a disabled composer must not process submit at all"
    );

    // The link comes back: the same key sends, with no further intervention.
    view.update(cx, |view, cx| {
        view.set_host(
            Some(ThreadHost {
                name: "dev-box".into(),
                unreachable: false,
            }),
            cx,
        );
        view.send(cx);
    });
    cx.run_until_parked();

    assert_eq!(sent.borrow().as_slice(), ["ship it".to_owned()]);
    view.read_with(cx, |view, _| assert!(!view.is_unreachable()));
}

/// A local thread carries no badge at all, and its composer keeps its ordinary invitation.
#[gpui::test]
fn a_local_thread_has_no_host_badge(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;

    use super::{AgentThreadView, ThreadHost};

    let view = cx.new(|cx| AgentThreadView::new(projection(), cx));
    view.update(cx, |view, cx| view.set_host(None, cx));
    view.read_with(cx, |view, _| {
        assert!(view.host().is_none());
        assert!(!view.is_unreachable());
    });

    // A reachable remote thread is badged but never stood down.
    view.update(cx, |view, cx| {
        view.set_host(
            Some(ThreadHost {
                name: "dev-box".into(),
                unreachable: false,
            }),
            cx,
        );
    });
    view.read_with(cx, |view, _| {
        assert!(view.host().is_some());
        assert!(!view.is_unreachable());
    });
}

/// The three unreachable sentences all name the host and never repeat one another.
#[test]
fn the_unreachable_copy_names_the_machine_in_every_place_it_appears() {
    use super::presentation::{unreachable_hint, unreachable_notice, unreachable_placeholder};

    let placeholder = unreachable_placeholder("dev-box");
    let hint = unreachable_hint("dev-box");
    let notice = unreachable_notice("dev-box");
    for line in [&placeholder, &hint, &notice] {
        assert!(line.contains("dev-box"), "{line}");
        assert!(line.contains("unreachable"), "{line}");
    }
    assert_ne!(placeholder, hint);
    assert_ne!(hint, notice);
}
