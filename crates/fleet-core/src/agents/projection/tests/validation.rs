use super::*;

#[test]
fn out_of_order_unknown_targets_and_wrong_turns_are_rejected_atomically() {
    let mut projection = projection();
    let original = projection.clone();
    let skipped = SeqEvent {
        seq: Seq(2),
        at: timestamp(2),
        raw: None,
        event: session_started(),
    };
    assert_eq!(
        projection.apply(&skipped),
        Err(ProjectionError::OutOfOrder {
            expected: Seq(1),
            got: Seq(2),
        })
    );
    assert_eq!(projection, original);

    let mut events = EventBuilder::new();
    let turn = turn_id(80);
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TurnStarted {
            turn,
            user_item: item_id(81),
        },
    );
    let before = projection.clone();
    let second_turn = turn_id(82);
    let second_start = events.next(AgentEvent::TurnStarted {
        turn: second_turn,
        user_item: item_id(83),
    });
    assert_eq!(
        projection.apply(&second_start),
        Err(ProjectionError::WrongTurn(second_turn))
    );
    assert_eq!(projection, before);

    let wrong_terminal = SeqEvent {
        event: AgentEvent::TurnSettled {
            turn: second_turn,
            outcome: TurnOutcome::Completed,
            usage: Usage::default(),
            duration_ms: 0,
            files_changed: Vec::new(),
        },
        ..second_start.clone()
    };
    assert_eq!(
        projection.apply(&wrong_terminal),
        Err(ProjectionError::WrongTurn(second_turn))
    );
    assert_eq!(projection, before);

    let missing_item = SeqEvent {
        event: AgentEvent::ContentDelta {
            item: item_id(84),
            stream: StreamKind::AssistantText,
            delta: "lost".to_owned(),
        },
        ..second_start
    };
    assert_eq!(
        projection.apply(&missing_item),
        Err(ProjectionError::UnknownItem(item_id(84)))
    );
    assert_eq!(projection, before);
}

#[test]
fn attention_follows_every_priority_row_and_seen_transition() {
    let mut permission = projection();
    permission.gates.push(OpenGate {
        id: gate_id(90),
        turn: None,
        kind: permission_gate(),
        opened_seq: Seq(1),
        blocked_since: None,
    });
    permission.gates.push(OpenGate {
        id: gate_id(91),
        turn: None,
        kind: question_gate(),
        opened_seq: Seq(2),
        blocked_since: None,
    });
    assert_eq!(
        permission.attention(Seq::default()),
        Attention::NeedsYou(AttentionKind::Permission)
    );

    permission.gates.remove(0);
    assert_eq!(
        permission.attention(Seq::default()),
        Attention::NeedsYou(AttentionKind::Question)
    );
    permission.gates[0].kind = plan_gate();
    assert_eq!(
        permission.attention(Seq::default()),
        Attention::NeedsYou(AttentionKind::Plan)
    );

    let mut finished = projection();
    let mut events = EventBuilder::new();
    apply(&mut finished, &mut events, session_started());
    start_turn(
        &mut finished,
        &mut events,
        turn_id(92),
        item_id(93),
        "Finish",
    );
    let completed = complete_turn(&mut finished, &mut events, turn_id(92));
    assert_eq!(
        finished.attention(Seq(completed.seq.0 - 1)),
        Attention::NeedsYou(AttentionKind::Finished)
    );
    assert_eq!(finished.attention(completed.seq), Attention::Idle);
    // A thread nobody has opened yet still raises the amber dot when its turn finishes.
    assert_eq!(
        finished.attention(Seq::default()),
        Attention::NeedsYou(AttentionKind::Finished)
    );
    apply(
        &mut finished,
        &mut events,
        AgentEvent::Notice("post-completion notice".to_owned()),
    );
    assert_eq!(finished.attention(completed.seq), Attention::Unread);
    assert_eq!(
        finished.attention(Seq(completed.seq.0 - 1)),
        Attention::NeedsYou(AttentionKind::Finished)
    );

    let mut failed = projection();
    failed.session = SessionState::Error;
    assert_eq!(failed.attention(Seq(1)), Attention::Failed);

    let mut working = projection();
    working.session = SessionState::Running;
    assert_eq!(working.attention(Seq(1)), Attention::Working);
    working.session = SessionState::Ready;
    working.background_tasks.push(item_id(94));
    assert_eq!(working.attention(Seq(1)), Attention::Working);
    working.background_tasks.clear();
    working.retrying = Some(RetryState {
        attempt: 1,
        retry_in_ms: 10,
        reason: "retry".to_owned(),
    });
    assert_eq!(working.attention(Seq(1)), Attention::Working);

    let mut unread = projection();
    let mut events = EventBuilder::new();
    apply(&mut unread, &mut events, session_started());
    let seen = unread.last_seq;
    apply(
        &mut unread,
        &mut events,
        AgentEvent::Notice("new output".to_owned()),
    );
    assert_eq!(unread.attention(seen), Attention::Unread);
    assert_eq!(unread.attention(unread.last_seq), Attention::Idle);
    assert_eq!(unread.attention(Seq::default()), Attention::Idle);
}

#[test]
fn steering_adds_a_user_message_to_the_active_turn_and_queue_has_no_event() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(100);
    apply(&mut projection, &mut events, session_started());
    start_turn(
        &mut projection,
        &mut events,
        turn,
        item_id(101),
        "First prompt",
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::ItemStarted {
            turn,
            item: item_id(102),
            kind: user_message("Steer the active prompt"),
            parent: None,
        },
    );
    assert_eq!(projection.items.len(), 2);
    assert!(projection.items.iter().all(|item| item.turn == turn));
    assert_eq!(projection.turn, TurnState::Running(turn));

    let before_queue = projection.clone();
    // Queued messages live only in app state and deliberately produce no AgentEvent.
    assert_eq!(projection, before_queue);
}

#[test]
fn all_observability_events_reduce_without_hidden_side_effects() {
    let mut projection = projection();
    projection.title = "Provider title".to_owned();
    let mut events = EventBuilder::new();
    apply(&mut projection, &mut events, session_started());
    assert_eq!(projection.title, "Provider title");
    apply(
        &mut projection,
        &mut events,
        AgentEvent::Compacted(crate::agents::CheckpointKind::Resumed { age_ms: 50 }),
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::Retrying {
            attempt: 2,
            retry_in_ms: 500,
            reason: "overloaded".to_owned(),
        },
    );
    assert_eq!(projection.attention(Seq(1)), Attention::Working);
    apply(
        &mut projection,
        &mut events,
        AgentEvent::RuntimeError {
            fatal: false,
            message: "recovered".to_owned(),
        },
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::SessionStateChanged(SessionState::Ready),
    );
    assert!(projection.retrying.is_none());
    apply(
        &mut projection,
        &mut events,
        AgentEvent::SessionExited {
            code: Some(0),
            expected: true,
        },
    );
    assert_eq!(projection.session, SessionState::Stopped);
    assert_eq!(projection.exit_code, Some(0));
}

#[test]
fn subagent_items_track_background_work_until_the_item_settles() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    let turn = turn_id(103);
    let subagent = item_id(104);
    apply(&mut projection, &mut events, session_started());
    start_turn(
        &mut projection,
        &mut events,
        turn,
        item_id(105),
        "Delegate research",
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::ItemStarted {
            turn,
            item: subagent,
            kind: ItemKind::Subagent {
                name: "explore".to_owned(),
                description: "Find callers".to_owned(),
                result: None,
            },
            parent: None,
        },
    );
    assert_eq!(projection.background_tasks, vec![subagent]);
    apply(
        &mut projection,
        &mut events,
        AgentEvent::ItemUpdated {
            item: subagent,
            patch: ItemPatch {
                status: Some(ItemStatus::Completed),
                ..ItemPatch::default()
            },
        },
    );
    assert!(projection.background_tasks.is_empty());
}

#[test]
fn usage_accumulates_by_turn_while_cost_and_context_use_latest_observation() {
    let mut projection = projection();
    let mut events = EventBuilder::new();
    apply(&mut projection, &mut events, session_started());

    let first = turn_id(105);
    start_turn(
        &mut projection,
        &mut events,
        first,
        item_id(106),
        "First turn",
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TurnSettled {
            turn: first,
            outcome: TurnOutcome::Completed,
            usage: usage(100),
            duration_ms: 100,
            files_changed: Vec::new(),
        },
    );

    let second = turn_id(107);
    start_turn(
        &mut projection,
        &mut events,
        second,
        item_id(108),
        "Second turn",
    );
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TokenUsage {
            turn: second,
            usage: usage(25),
            context_pct: 61.5,
            cost_usd: Some(1.25),
        },
    );
    assert_eq!(projection.cumulative_usage.total_tokens, 125);
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TurnAborted {
            turn: second,
            reason: AbortReason::User,
        },
    );
    assert_eq!(
        projection.turns[1].footer().map(|footer| footer.tokens),
        Some(25)
    );
    assert_eq!(projection.cumulative_usage.total_tokens, 125);
    apply(
        &mut projection,
        &mut events,
        AgentEvent::TokenUsage {
            turn: second,
            usage: usage(30),
            context_pct: 62.0,
            cost_usd: None,
        },
    );
    // §5: "turn metadata is withheld until the turn completes, so the footer never moves
    // under the reader". A late observation against a settled turn — Claude publishes one
    // for `last_turn` whenever a `result` arrives with no active turn — is a session fact,
    // never a rewrite of the footer that turn already froze.
    assert_eq!(
        projection.turns[1].footer().map(|footer| footer.tokens),
        Some(25)
    );
    assert_eq!(projection.cumulative_usage.total_tokens, 125);
    assert_eq!(projection.context_pct, 62.0);
    // A frame that omits the cost is not a report of zero: the metadata row keeps the last
    // cost the process actually reported (§4.1, "take the latest, never sum").
    assert_eq!(projection.cumulative_cost_usd, Some(1.25));
}
