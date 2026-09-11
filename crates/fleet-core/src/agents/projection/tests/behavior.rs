use super::*;
use crate::agents::ToolCall;

/// BH-3: an unmeasured window is not a report of zero occupancy.
///
/// On an OpenCode resume the first usage frame of the turn arrives before the
/// `message.updated` that re-learns the model, so the mapper answers 0.0 for "window
/// unknown". Writing it through blanked `context 34%` mid-turn.
#[test]
fn an_unmeasured_context_window_leaves_the_last_known_percentage_standing() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    apply(&mut projection, &mut events, session_started());
    let turn = turn_id(150);
    start_turn(&mut projection, &mut events, turn, item_id(151), "Resume");
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TokenUsage {
            turn,
            usage: usage(10),
            context_pct: 19.5,
            cost_usd: None,
        },
    );
    assert_eq!(projection.context_pct, 19.5);
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TokenUsage {
            turn,
            usage: usage(20),
            context_pct: 0.0,
            cost_usd: None,
        },
    );
    assert_eq!(
        projection.context_pct, 19.5,
        "an unmeasured window blanked the metadata row"
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TokenUsage {
            turn,
            usage: usage(30),
            context_pct: 1.9,
            cost_usd: None,
        },
    );
    assert_eq!(projection.context_pct, 1.9);
}

#[test]
fn a_new_session_starts_a_new_cost_but_a_missing_one_does_not_clear_it() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    apply(&mut projection, &mut events, session_started());
    let turn = turn_id(140);
    start_turn(&mut projection, &mut events, turn, item_id(141), "Cost");
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TokenUsage {
            turn,
            usage: usage(10),
            context_pct: 12.0,
            cost_usd: Some(0.42),
        },
    );
    assert_eq!(projection.cumulative_cost_usd, Some(0.42));
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TokenUsage {
            turn,
            usage: usage(20),
            context_pct: 13.0,
            cost_usd: None,
        },
    );
    assert_eq!(projection.cumulative_cost_usd, Some(0.42));

    apply(&mut projection, &mut events, session_started());
    assert_eq!(projection.cumulative_cost_usd, None);
}

#[test]
fn replay_is_deterministic_and_summary_is_stable() {
    let turn = turn_id(110);
    let assistant = item_id(112);
    let mut builder = EventBuilder::new();
    let sequence = vec![
        builder.next(session_started()),
        builder.next(AgentEvent::TurnStarted {
            turn,
            user_item: item_id(111),
        }),
        builder.next(AgentEvent::ItemStarted {
            turn,
            item: item_id(111),
            kind: user_message("Deterministic replay"),
            parent: None,
        }),
        builder.next(AgentEvent::ItemStarted {
            turn,
            item: assistant,
            kind: ItemKind::Reasoning {
                summary: BTreeMap::new(),
                raw: BTreeMap::new(),
            },
            parent: None,
        }),
        builder.next(AgentEvent::ContentDelta {
            item: assistant,
            stream: StreamKind::ReasoningSummary { part: 0 },
            delta: "reasoning".to_owned(),
        }),
        builder.next(AgentEvent::ItemCompleted {
            item: assistant,
            status: ItemStatus::Completed,
        }),
        builder.next(AgentEvent::TurnSettled {
            turn,
            outcome: TurnOutcome::Completed,
            usage: usage(64),
            duration_ms: 700,
            files_changed: Vec::new(),
        }),
    ];
    let mut left = projection();
    let mut right = projection();
    for event in &sequence {
        left.apply(event).unwrap_or_else(|error| panic!("{error}"));
        right.apply(event).unwrap_or_else(|error| panic!("{error}"));
    }

    assert_eq!(left, right);
    let summary = left.summary(Seq(1));
    assert_eq!(summary, left.summary(Seq(1)));
    assert_eq!(summary, right.summary(Seq(1)));
    assert_eq!(item_text(&left.items[1]), Some("reasoning"));
}

#[test]
fn projections_without_attention_cursors_remain_wire_compatible() {
    let projection = projection();
    let mut value = serde_json::to_value(&projection)
        .unwrap_or_else(|error| panic!("projection must serialize: {error}"));
    let object = value
        .as_object_mut()
        .unwrap_or_else(|| panic!("projection must serialize as an object"));
    object.remove("lastCompletedSeq");
    object.remove("lastNonterminalSeq");

    let restored: ThreadProjection = serde_json::from_value(value)
        .unwrap_or_else(|error| panic!("legacy projection must deserialize: {error}"));
    assert_eq!(restored, projection);
}

#[test]
fn checkpoints_are_recorded_where_the_transcript_shows_them() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(150);
    apply(&mut projection, &mut events, session_started());
    apply(
        &mut projection,
        &mut events,
        AgentEvent::Compacted(crate::agents::CheckpointKind::Resumed { age_ms: 7_200_000 }),
    );
    start_turn(&mut projection, &mut events, turn, item_id(151), "Go on");
    apply(
        &mut projection,
        &mut events,
        AgentEvent::Compacted(crate::agents::CheckpointKind::CompactBoundary {
            before: 84_000,
            after: Some(12_000),
        }),
    );

    assert_eq!(
        projection
            .checkpoints
            .iter()
            .map(|checkpoint| checkpoint.after_turn)
            .collect::<Vec<_>>(),
        [None, Some(turn)]
    );
}

#[test]
fn an_error_result_fails_the_turn_so_the_tab_counts_it_as_failed() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(130);
    apply(&mut projection, &mut events, session_started());
    start_turn(&mut projection, &mut events, turn, item_id(131), "Ask");
    let completed = apply(
        &mut projection,
        &mut events,
        AgentEvent::TurnSettled {
            turn,
            outcome: TurnOutcome::Error {
                message: Some("Authentication Failed".to_owned()),
            },
            usage: usage(10),
            duration_ms: 900,
            files_changed: Vec::new(),
        },
    );

    assert!(matches!(
        projection.turn,
        TurnState::Settled(_, TurnOutcome::Error { .. })
    ));
    assert_eq!(
        projection.turns[0]
            .ended
            .as_ref()
            .map(|ended| ended.outcome.clone()),
        Some(TurnOutcome::Error {
            message: Some("Authentication Failed".to_owned()),
        })
    );
    // §3 rule 7: failure outranks fresh completion, seen or unseen. A turn that ended in an
    // error is not a result waiting to be read.
    assert_eq!(
        projection.attention(Seq(completed.seq.0 - 1)),
        Attention::Failed
    );
    assert_eq!(projection.attention(completed.seq), Attention::Failed);
}

#[test]
fn an_unseen_completion_never_shadows_a_dead_or_a_working_session() {
    // A finished turn nobody has read, followed by a killed provider: §3 rule 7 puts work
    // and failure ahead of fresh completion, so the tab says `failed`, not `needs you`.
    let mut dead = projection();
    let mut events = EventBuilder::new();
    apply(&mut dead, &mut events, session_started());
    start_turn(&mut dead, &mut events, turn_id(200), item_id(201), "Go");
    let completed = complete_turn(&mut dead, &mut events, turn_id(200));
    assert_eq!(
        dead.attention(Seq(completed.seq.0 - 1)),
        Attention::NeedsYou(AttentionKind::Finished)
    );
    apply(
        &mut dead,
        &mut events,
        AgentEvent::SessionExited {
            code: None,
            expected: false,
        },
    );
    assert_eq!(dead.attention(Seq(completed.seq.0 - 1)), Attention::Failed);

    // The same unread completion, followed by a new turn: the tab says `working`.
    let mut busy = projection();
    let mut events = EventBuilder::new();
    apply(&mut busy, &mut events, session_started());
    start_turn(&mut busy, &mut events, turn_id(202), item_id(203), "Go");
    let completed = complete_turn(&mut busy, &mut events, turn_id(202));
    start_turn(&mut busy, &mut events, turn_id(204), item_id(205), "More");
    assert_eq!(busy.attention(Seq(completed.seq.0 - 1)), Attention::Working);
}

#[test]
fn a_rejected_event_leaves_every_field_untouched_without_cloning() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(140);
    let tool = item_id(141);
    apply(&mut projection, &mut events, session_started());
    start_turn(&mut projection, &mut events, turn, item_id(142), "Work");
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
    let before = projection.clone();

    let next_seq = projection.last_seq.next();
    let orphan_parent = SeqEvent {
        seq: next_seq,
        at: timestamp(next_seq.0),
        raw: None,
        event: AgentEvent::ItemStarted {
            turn,
            item: item_id(143),
            kind: ItemKind::AssistantText {
                text: String::new(),
            },
            parent: Some(item_id(144)),
        },
    };
    assert_eq!(
        projection.apply(&orphan_parent),
        Err(ProjectionError::UnknownItem(item_id(144)))
    );
    assert_eq!(projection, before);

    let unknown_gate = SeqEvent {
        event: AgentEvent::GateResolved {
            gate: gate_id(145),
            answer: GateAnswer::Plan(PlanAnswer::Approve),
            by: GateResolver::User,
        },
        ..orphan_parent.clone()
    };
    assert_eq!(
        projection.apply(&unknown_gate),
        Err(ProjectionError::UnknownGate(gate_id(145)))
    );
    assert_eq!(projection, before);

    let unknown_turn_usage = SeqEvent {
        event: AgentEvent::TokenUsage {
            turn: turn_id(146),
            usage: usage(10),
            context_pct: 1.0,
            cost_usd: None,
        },
        ..orphan_parent.clone()
    };
    assert_eq!(
        projection.apply(&unknown_turn_usage),
        Err(ProjectionError::WrongTurn(turn_id(146)))
    );
    assert_eq!(projection, before);
}

#[test]
fn gate_resolution_and_plan_answers_do_not_mutate_unrelated_turn_state() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let gate = gate_id(120);
    apply(
        &mut projection,
        &mut events,
        AgentEvent::GateOpened {
            gate,
            turn: None,
            kind: plan_gate(),
        },
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::GateResolved {
            gate,
            answer: GateAnswer::Plan(PlanAnswer::Approve),
            by: GateResolver::Auto,
        },
    );
    assert_eq!(projection.turn, TurnState::None);
    assert!(projection.gates.is_empty());
}
