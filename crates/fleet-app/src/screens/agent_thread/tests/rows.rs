//! The flat row projection: emission order, the fold table, grouping and the live row.

use fleet_core::agents::{
    AgentKind, CheckpointKind, CheckpointRecord, ItemStatus, NoticeRecord, PermissionChoice, Seq,
    SessionState, ToolKind, TurnId, TurnOutcome, TurnState,
};
use fleet_ui_kit::{GateOutcome, ToolRowState, TranscriptRowId, TranscriptRowKind, WorkingPhase};

use super::fixtures::{
    Locals, assistant, command, edit, item, pending, permission_gate, plan, projection, question,
    question_gate, reasoning, running_turn, settled_turn, subagent, tool, user,
};
use crate::screens::agent_thread::rows::{ResolvedGate, build_rows, fold, group, live};

/// The kind names of a built row list, for an assertion that reads as the transcript does.
fn kinds(rows: &[fleet_ui_kit::TranscriptRow]) -> Vec<&'static str> {
    rows.iter()
        .map(|row| match &row.kind {
            TranscriptRowKind::User(_) => "user",
            TranscriptRowKind::Assistant(_) => "assistant",
            TranscriptRowKind::AssistantMeta(_) => "meta",
            TranscriptRowKind::Reasoning(_) => "reasoning",
            TranscriptRowKind::Work(_) => "work",
            TranscriptRowKind::WorkLive(_) => "work-live",
            TranscriptRowKind::WorkGroup(_) => "work-group",
            TranscriptRowKind::Subagent(_) => "subagent",
            TranscriptRowKind::Diff(_) => "diff",
            TranscriptRowKind::TurnFold(_) => "fold",
            TranscriptRowKind::TurnFooter(_) => "footer",
            TranscriptRowKind::Plan(_) => "plan",
            TranscriptRowKind::Gate(_) => "gate",
            TranscriptRowKind::Checkpoint(_) => "checkpoint",
            TranscriptRowKind::Notice(_) => "notice",
            TranscriptRowKind::Error(_) => "error",
            TranscriptRowKind::Working(_) => "working",
            TranscriptRowKind::Empty(_) => "empty",
        })
        .collect()
}

#[test]
fn an_empty_idle_thread_shows_only_its_invitation() {
    let projection = projection();
    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));

    assert_eq!(kinds(&built.rows), ["empty"]);
}

#[test]
fn a_settled_turn_folds_its_work_and_ends_with_one_footer() {
    let mut projection = projection();
    let turn = TurnId::new();
    let prompt = user(turn, "fix the rounding");
    let items = vec![
        prompt.clone(),
        tool(turn, ToolKind::Read, ItemStatus::Completed, "src/lib.rs"),
        command(turn, ItemStatus::Completed, "cargo test"),
        assistant(turn, "done", ItemStatus::Completed),
    ];
    projection.items = items;
    projection.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Completed)];
    projection.turn = TurnState::Settled(turn, TurnOutcome::Completed);

    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));

    // Everything except the terminal assistant message folds, and the fold stands where the
    // folded work was — never below the closing paragraph, as if the tools ran after the answer.
    assert_eq!(
        kinds(&built.rows),
        ["user", "fold", "assistant", "meta", "footer"]
    );
    let TranscriptRowKind::TurnFold(row) = &built.rows[1].kind else {
        panic!("the second row is the fold");
    };
    assert_eq!(row.label, "worked 48s · 2 steps");
}

#[test]
fn opening_a_fold_keeps_the_number_it_just_showed() {
    let mut projection = projection();
    let turn = TurnId::new();
    let prompt = user(turn, "fix it");
    projection.items = vec![
        prompt.clone(),
        tool(turn, ToolKind::Read, ItemStatus::Completed, "a.rs"),
        tool(turn, ToolKind::Read, ItemStatus::Completed, "b.rs"),
        assistant(turn, "done", ItemStatus::Completed),
    ];
    projection.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Completed)];
    projection.turn = TurnState::Settled(turn, TurnOutcome::Completed);

    let closed = Locals::default();
    let open = Locals::default().unfold(turn);
    let before = build_rows(&closed.inputs(&projection));
    let after = build_rows(&open.inputs(&projection));

    let (TranscriptRowKind::TurnFold(closed), TranscriptRowKind::TurnFold(open)) =
        (&before.rows[1].kind, &after.rows[1].kind)
    else {
        panic!("both builds put the fold second");
    };
    assert_eq!(closed.label, open.label, "the count does not move");
    assert!(!closed.expanded);
    assert!(open.expanded);
    // Two reads collapse to one group row rather than two work rows.
    assert_eq!(
        kinds(&after.rows),
        ["user", "fold", "work-group", "assistant", "meta", "footer"]
    );
}

#[test]
fn the_fold_exemption_table_holds_one_case_per_row() {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut projection = projection();
    projection.items = vec![
        prompt.clone(),
        tool(turn, ToolKind::Read, ItemStatus::Completed, "a.rs"),
        assistant(turn, "done", ItemStatus::Completed),
    ];

    // Unsettled: nothing about the turn is history yet.
    projection.turns = vec![running_turn(turn, prompt.id)];
    let locals = Locals::default();
    let own: Vec<&_> = projection.items.iter().collect();
    assert_eq!(
        fold::derive(
            &locals.inputs(&projection),
            &projection.turns[0],
            &own,
            Some(2),
            None
        ),
        Err(fold::Exempt::Unsettled)
    );

    // The session's running turn wins over the latest recorded one.
    projection.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Completed)];
    assert_eq!(
        fold::derive(
            &locals.inputs(&projection),
            &projection.turns[0],
            &own,
            Some(2),
            Some(turn)
        ),
        Err(fold::Exempt::RunningTurnLags)
    );

    // A turn holding a streaming message never folds.
    let mut streaming = projection.clone();
    streaming.items[2] = assistant(turn, "writ", ItemStatus::InProgress);
    let own: Vec<&_> = streaming.items.iter().collect();
    assert_eq!(
        fold::derive(
            &locals.inputs(&streaming),
            &streaming.turns[0],
            &own,
            Some(2),
            None
        ),
        Err(fold::Exempt::Streaming)
    );

    // Nothing but the user message and the terminal answer: nothing to fold.
    let mut bare = projection.clone();
    bare.items = vec![
        prompt.clone(),
        assistant(turn, "done", ItemStatus::Completed),
    ];
    let own: Vec<&_> = bare.items.iter().collect();
    assert_eq!(
        fold::derive(&locals.inputs(&bare), &bare.turns[0], &own, Some(1), None),
        Err(fold::Exempt::NothingToFold)
    );
}

#[test]
fn a_failed_row_never_folds_and_a_live_subagent_never_folds() {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut projection = projection();
    let broken = command(turn, ItemStatus::Failed, "cargo build");
    let fleet = subagent(turn, ItemStatus::InProgress);
    projection.items = vec![
        prompt.clone(),
        tool(turn, ToolKind::Read, ItemStatus::Completed, "a.rs"),
        broken.clone(),
        fleet.clone(),
        assistant(turn, "done", ItemStatus::Completed),
    ];
    projection.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Completed)];
    projection.turn = TurnState::Settled(turn, TurnOutcome::Completed);

    let locals = Locals::default();
    let own: Vec<&_> = projection.items.iter().collect();
    let derived = fold::derive(
        &locals.inputs(&projection),
        &projection.turns[0],
        &own,
        Some(4),
        None,
    )
    .expect("the completed read folds");

    assert!(
        !derived.hidden.contains(&broken.id),
        "a user who scrolls back must see what broke without hunting"
    );
    assert!(
        !derived.hidden.contains(&fleet.id),
        "folding a still-running fleet makes it invisible"
    );
    let built = build_rows(&locals.inputs(&projection));
    assert!(
        kinds(&built.rows).contains(&"work"),
        "the failure stays a row"
    );
    assert!(kinds(&built.rows).contains(&"subagent"));
}

#[test]
fn the_live_row_stays_present_tense_over_a_completed_command() {
    // A completed command inside the live row still reads `running cargo`, never `ran cargo`,
    // so the row never flickers between tenses while the turn is alive.
    assert_eq!(
        live::live_status(ItemStatus::Completed, true),
        ItemStatus::InProgress
    );
    assert_eq!(
        live::live_status(ItemStatus::Completed, false),
        ItemStatus::Completed
    );
    // The three states that are news in their own right survive the collapse.
    for status in [ItemStatus::Failed, ItemStatus::Denied, ItemStatus::Stopped] {
        assert_eq!(live::live_status(status, true), status);
    }
}

#[test]
fn the_live_row_subsumes_only_the_plain_work_tail() {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut projection = projection();
    let fleet = subagent(turn, ItemStatus::InProgress);
    projection.items = vec![
        prompt.clone(),
        command(turn, ItemStatus::Completed, "cargo fmt"),
        fleet,
        command(turn, ItemStatus::Completed, "cargo test"),
        command(turn, ItemStatus::InProgress, "cargo clippy"),
    ];
    projection.turns = vec![running_turn(turn, prompt.id)];
    projection.turn = TurnState::Running(turn);
    projection.session = SessionState::Running;

    let own: Vec<&_> = projection.items.iter().collect();
    let tail = live::tail(&own);

    // The walk **breaks** on the subagent rather than skipping it, so the two commands after it
    // are the tail and the one before it is not.
    assert_eq!(tail, vec![3, 4]);
    let row = live::row(&own, &tail, true).expect("the tail has a live row");
    assert_eq!(row.label, "running cargo clippy");
    assert!(row.shimmer);

    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));
    assert_eq!(
        kinds(&built.rows),
        ["user", "work", "subagent", "work-live"],
        "one live row stands for the whole tail"
    );
    assert_eq!(built.rows[3].id, TranscriptRowId::LiveActivity);
}

#[test]
fn thinking_working_and_the_live_row_are_one_row() {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut projection = projection();
    projection.turns = vec![running_turn(turn, prompt.id)];
    projection.turn = TurnState::Running(turn);
    projection.session = SessionState::Running;

    // Nothing has started: the working row is the live row.
    projection.items = vec![prompt.clone()];
    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));
    assert_eq!(kinds(&built.rows), ["user", "working"]);
    assert_eq!(built.rows[1].id, TranscriptRowId::LiveActivity);

    // Thinking: the same id, so the handoff is a label change and not a mount.
    projection.items = vec![
        prompt.clone(),
        reasoning(turn, "hmm", ItemStatus::InProgress),
    ];
    let built = build_rows(&locals.inputs(&projection));
    assert_eq!(kinds(&built.rows), ["user", "reasoning"]);
    assert_eq!(built.rows[1].id, TranscriptRowId::LiveActivity);

    // A tool starts: still the same id.
    projection.items = vec![
        prompt,
        reasoning(turn, "hmm", ItemStatus::Completed),
        command(turn, ItemStatus::InProgress, "cargo test"),
    ];
    let built = build_rows(&locals.inputs(&projection));
    assert_eq!(*kinds(&built.rows).last().expect("a live row"), "work-live");
    assert_eq!(
        built.rows.last().map(|row| row.id.clone()),
        Some(TranscriptRowId::LiveActivity)
    );
}

#[test]
fn a_group_is_neutral_unless_its_latest_entry_failed() {
    let turn = TurnId::new();
    let mut items = [
        command(turn, ItemStatus::Failed, "cargo build"),
        command(turn, ItemStatus::Completed, "cargo fmt"),
    ];
    let run: Vec<&_> = items.iter().collect();
    let row = group::row(&run, false).expect("two commands summarize");
    assert_eq!(row.summary, "ran 2 commands");
    assert!(
        !row.latest_failed,
        "a group holding one failure and one success draws no danger mark"
    );

    items.reverse();
    let run: Vec<&_> = items.iter().collect();
    let row = group::row(&run, false).expect("two commands summarize");
    assert!(row.latest_failed, "only the latest entry failing marks it");
}

#[test]
fn a_single_edit_still_goes_through_the_summarizer() {
    let turn = TurnId::new();
    let one = [edit(turn, ItemStatus::Completed, "src/lib.rs")];
    let run: Vec<&_> = one.iter().collect();
    assert!(
        group::summarizes_alone(&run),
        "`changed 1 file` beats one file name on a settled row"
    );
    let reads = [tool(turn, ToolKind::Read, ItemStatus::Completed, "a.rs")];
    let run: Vec<&_> = reads.iter().collect();
    assert!(!group::summarizes_alone(&run), "a lone read is a plain row");
}

#[test]
fn distinct_changed_paths_are_counted_once() {
    let turn = TurnId::new();
    let items = [
        edit(turn, ItemStatus::Completed, "src/lib.rs"),
        edit(turn, ItemStatus::Completed, "src/lib.rs"),
        edit(turn, ItemStatus::Completed, "src/main.rs"),
    ];
    let run: Vec<&_> = items.iter().collect();
    assert_eq!(group::counts(&run).changed, 2);
}

#[test]
fn an_expanded_edit_row_emits_its_diff_as_its_own_row() {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let change = edit(turn, ItemStatus::Completed, "src/lib.rs");
    let mut projection = projection();
    projection.items = vec![prompt.clone(), change.clone()];
    projection.turns = vec![running_turn(turn, prompt.id)];

    let locals = Locals::default().expand(change.id);
    let built = build_rows(&locals.inputs(&projection));

    assert_eq!(kinds(&built.rows), ["user", "work-group", "work", "diff"]);
    // The work row and the diff row under it share one id, so an update merging forward does
    // not remount either of them.
    assert_eq!(built.rows[2].id, built.rows[3].id);
}

#[test]
fn a_checkpoint_and_a_notice_are_their_own_rows() {
    let mut projection = projection();
    projection.checkpoints = vec![CheckpointRecord {
        kind: CheckpointKind::CompactBoundary {
            before: 120_000,
            after: Some(30_000),
        },
        seq: Seq(2),
        after_turn: None,
    }];
    projection.notices = vec![NoticeRecord {
        text: "stop hook error occurred".to_owned(),
        seq: Seq(3),
        after_turn: None,
    }];

    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));

    assert_eq!(kinds(&built.rows), ["checkpoint", "notice"]);
    // The two share one ordinal space, and folding the discriminant into the key is what stops
    // the third notice and the third checkpoint from recycling each other's measured height.
    assert_ne!(built.rows[0].id, built.rows[1].id);
}

#[test]
fn a_backoff_is_a_notice_and_a_dead_session_is_an_error() {
    let mut projection = projection();
    projection.retrying = Some(fleet_core::agents::RetryState {
        attempt: 2,
        retry_in_ms: 4_000,
        reason: "rate limited".to_owned(),
    });
    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));
    assert_eq!(kinds(&built.rows), ["notice", "working"]);

    let mut dead = projection.clone();
    dead.retrying = None;
    dead.session = SessionState::Error;
    dead.exit_code = Some(137);
    let built = build_rows(&locals.inputs(&dead));
    assert_eq!(kinds(&built.rows), ["error"]);
}

#[test]
fn a_parked_turn_is_working_with_a_detail_line_and_never_an_error() {
    let mut projection = projection();
    projection.session = SessionState::Waiting(fleet_core::agents::WaitingReason::UsageLimit {
        window: "weekly".to_owned(),
        resets_at: super::fixtures::at(10_800),
    });
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    projection.items = vec![prompt.clone()];
    projection.turns = vec![running_turn(turn, prompt.id)];
    projection.turn = TurnState::Running(turn);

    let locals = Locals::default();
    let mut inputs = locals.inputs(&projection);
    inputs.parked = Some(gpui::SharedString::new_static("weekly limit resets in ~3h"));
    let built = build_rows(&inputs);

    assert_eq!(kinds(&built.rows), ["user", "working"]);
    let TranscriptRowKind::Working(row) = &built.rows[1].kind else {
        panic!("the live row is the working row");
    };
    assert_eq!(row.phase, WorkingPhase::Parked);
    assert_eq!(row.detail.as_deref(), Some("weekly limit resets in ~3h"));
}

#[test]
fn a_plan_promotes_its_first_heading_out_of_its_body() {
    let turn = TurnId::new();
    let prompt = user(turn, "plan it");
    let mut projection = projection();
    projection.items = vec![
        prompt.clone(),
        plan(
            turn,
            "# Fix the rounding\n\n## Summary\n\nStep one.\nStep two.",
        ),
    ];
    projection.turns = vec![running_turn(turn, prompt.id)];

    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));

    let TranscriptRowKind::Plan(row) = &built.rows[1].kind else {
        panic!("the plan is the second row");
    };
    assert_eq!(row.title, "Fix the rounding");
    // The title is removed from the body, and a redundant `## Summary` right after it goes too.
    let body = format!("{:?}", row.markdown);
    assert!(!body.contains("Fix the rounding"), "{body}");
    assert!(!body.contains("Summary"), "{body}");
}

#[test]
fn a_resolved_gate_leaves_a_record_where_it_was_asked() {
    let projection = projection();
    let gate = permission_gate("git push --force", &[PermissionChoice::AllowOnce]);
    let locals = Locals::default().resolve(ResolvedGate {
        gate: gate.clone(),
        outcome: GateOutcome::Allowed,
        answer: Some(fleet_core::agents::GateAnswer::Permission {
            choice: PermissionChoice::AllowOnce,
            edited_payload: None,
        }),
    });
    let built = build_rows(&locals.inputs(&projection));

    assert_eq!(kinds(&built.rows), ["gate"]);
    let TranscriptRowKind::Gate(row) = &built.rows[0].kind else {
        panic!("the record is a gate row");
    };
    assert_eq!(row.label, "allowed once");
    assert_eq!(row.detail, "bash: git push --force");
}

#[test]
fn a_withdrawn_gate_says_the_agent_stopped_waiting() {
    let projection = projection();
    let gate = question_gate(vec![question("pm", "which one?", &["pnpm"], false, false)]);
    let locals = Locals::default().resolve(ResolvedGate {
        gate,
        outcome: GateOutcome::Withdrawn,
        answer: None,
    });
    let built = build_rows(&locals.inputs(&projection));

    let TranscriptRowKind::Gate(row) = &built.rows[0].kind else {
        panic!("the record is a gate row");
    };
    assert_eq!(row.label, "withdrawn · the agent stopped waiting");
}

#[test]
fn an_optimistic_bubble_marks_a_steer_and_keeps_its_own_id() {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut projection = projection();
    projection.items = vec![prompt.clone()];
    projection.turns = vec![running_turn(turn, prompt.id)];
    projection.turn = TurnState::Running(turn);

    let bubble = pending("also fix the tests", true);
    let locals = Locals::default().pend(bubble.clone());
    let built = build_rows(&locals.inputs(&projection));

    assert_eq!(kinds(&built.rows), ["user", "user", "working"]);
    let TranscriptRowKind::User(row) = &built.rows[1].kind else {
        panic!("the bubble is a user row");
    };
    assert!(row.steered, "a message joining a running turn is a steer");
    assert_eq!(row.state, fleet_ui_kit::UserRowState::Sending);
    assert_eq!(
        built.rows[1].id,
        TranscriptRowId::Item(gpui::SharedString::from(bubble.id.to_string())),
        "the client-minted id is the row's identity for its whole life"
    );
}

#[test]
fn the_footer_and_the_fold_both_drop_the_time_the_turn_stood_on_a_gate() {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut projection = projection();
    projection.items = vec![
        prompt.clone(),
        tool(turn, ToolKind::Read, ItemStatus::Completed, "a.rs"),
        assistant(turn, "done", ItemStatus::Completed),
    ];
    let mut record = settled_turn(turn, prompt.id, TurnOutcome::Completed);
    // The harness reported 48s of wall clock; 26s of it was a permission card on screen.
    record.blocked_ms = 26_000;
    projection.turns = vec![record];
    projection.turn = TurnState::Settled(turn, TurnOutcome::Completed);

    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));

    let TranscriptRowKind::TurnFold(fold) = &built.rows[1].kind else {
        panic!("the fold is second");
    };
    assert_eq!(fold.label, "worked 22s · 1 step");
}

#[test]
fn an_interrupted_turn_says_you_stopped_and_raises_no_error() {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut projection = projection();
    projection.items = vec![
        prompt.clone(),
        tool(turn, ToolKind::Read, ItemStatus::Completed, "a.rs"),
        assistant(turn, "partial", ItemStatus::Completed),
    ];
    projection.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Interrupted)];
    projection.turn = TurnState::Settled(turn, TurnOutcome::Interrupted);

    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));

    let TranscriptRowKind::TurnFold(fold) = &built.rows[1].kind else {
        panic!("the fold is second");
    };
    assert_eq!(fold.label, "you stopped after 48s");
    assert!(
        !kinds(&built.rows).contains(&"error"),
        "a stop is not a failure"
    );
    let TranscriptRowKind::TurnFooter(footer) = &built.rows[4].kind else {
        panic!("the footer closes the turn");
    };
    assert_eq!(
        footer.segments.first().map(ToString::to_string).as_deref(),
        Some("stopped")
    );
}

#[test]
fn revert_is_drawn_only_where_a_checkpoint_exists() {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut projection = projection();
    projection.items = vec![
        prompt.clone(),
        assistant(turn, "done", ItemStatus::Completed),
    ];
    projection.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Completed)];
    projection.turn = TurnState::Settled(turn, TurnOutcome::Completed);

    let without = Locals::default();
    let built = build_rows(&without.inputs(&projection));
    let TranscriptRowKind::TurnFooter(footer) = &built.rows.last().expect("a footer").kind else {
        panic!("the footer closes the turn");
    };
    assert!(
        !footer.revert,
        "a drawn affordance that does nothing is worse than an absent one"
    );

    let with = Locals::default().checkpoint(turn);
    let built = build_rows(&with.inputs(&projection));
    let TranscriptRowKind::TurnFooter(footer) = &built.rows.last().expect("a footer").kind else {
        panic!("the footer closes the turn");
    };
    assert!(footer.revert);
}

#[test]
fn a_failing_command_states_its_exit_code_and_never_a_colour() {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut broken = command(turn, ItemStatus::Failed, "cargo build");
    if let fleet_core::agents::ItemKind::Tool(call) = &mut broken.kind {
        call.exit_code = Some(101);
    }
    let mut projection = projection();
    projection.items = vec![prompt.clone(), broken];
    projection.turns = vec![running_turn(turn, prompt.id)];

    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));

    let TranscriptRowKind::Work(row) = &built.rows[1].kind else {
        panic!("the command is a work row");
    };
    assert_eq!(row.state, ToolRowState::Failed);
    assert_eq!(row.result.as_deref(), Some("exit 101"));
    assert!(
        row.is_expandable(),
        "a failed row is always expandable so its label can be read in full"
    );
}

#[test]
fn a_settled_item_with_nothing_in_it_is_not_a_row() {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut projection = projection();
    projection.items = vec![
        prompt.clone(),
        // Claude opens a thinking block for a signature-only reasoning frame and settles it
        // with no delta at all; a `[⏎] show` that expands to nothing is not a row.
        reasoning(turn, "", ItemStatus::Completed),
        assistant(turn, "done", ItemStatus::Completed),
    ];
    projection.turns = vec![running_turn(turn, prompt.id)];

    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));

    assert_eq!(kinds(&built.rows), ["user", "assistant"]);
}

#[test]
fn orphan_items_are_still_visible() {
    let mut projection = projection();
    let turn = TurnId::new();
    // The reducer accepted the item before the turn record arrived; dropping it would make the
    // thread look empty for exactly as long as that race lasts.
    projection.items = vec![item(
        turn,
        fleet_core::agents::ItemKind::AssistantText {
            text: "early".to_owned(),
        },
        ItemStatus::Completed,
    )];

    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));

    assert_eq!(kinds(&built.rows), ["assistant"]);
}

#[test]
fn the_provider_word_comes_from_the_projection() {
    let mut projection = projection();
    projection.provider = AgentKind::Codex;
    projection.exit_code = Some(1);
    projection.session = SessionState::Error;

    let locals = Locals::default();
    let built = build_rows(&locals.inputs(&projection));

    let TranscriptRowKind::Error(row) = &built.rows[0].kind else {
        panic!("a dead session is an error card");
    };
    assert_eq!(row.message, "codex exited 1");
}
