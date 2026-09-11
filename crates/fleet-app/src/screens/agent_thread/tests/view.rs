//! The wiring only an entity graph can show: what a stream chunk splices, which frame a gate
//! takes the keyboard in, the `esc` cascade, scroll mode, and what a closed tab releases.

use std::{cell::RefCell, rc::Rc};

use fleet_core::agents::{
    GateAnswer, ItemStatus, PermissionChoice, SessionState, ToolKind, TurnId, TurnOutcome,
    TurnState,
};
use fleet_ui_kit::{TranscriptRowId, TranscriptRowKind};
use gpui::{AppContext as _, TestAppContext};

use super::fixtures::{
    assistant, command, permission_gate, projection, question, question_gate, running_turn,
    settled_turn, tool, user,
};
use crate::{
    bridge::BridgeCommand,
    screens::agent_thread::{AgentThreadEvent, AgentThreadView, composer::ComposerMode},
};

/// Collects every command a view emits, so a test asserts on the wire and not on a mock.
fn recorder(
    view: &gpui::Entity<AgentThreadView>,
    cx: &mut TestAppContext,
) -> Rc<RefCell<Vec<BridgeCommand>>> {
    let commands: Rc<RefCell<Vec<BridgeCommand>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&commands);
    cx.update(|cx| {
        cx.subscribe(view, move |_, event, _| {
            if let AgentThreadEvent::Command(command) = event {
                seen.borrow_mut().push(command.clone());
            }
        })
        .detach();
    });
    commands
}

#[gpui::test]
fn a_composer_submit_sends_the_text_it_reported(cx: &mut TestAppContext) {
    // `MultilineInput::submit` empties the buffer and pushes to history before the event is
    // delivered, so the owner has to consume the reported text; re-reading the composer would
    // send nothing.
    let view = cx.new(|cx| AgentThreadView::new(projection(), cx));
    let commands = recorder(&view, cx);

    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| {
            input.set_text("ship it", cx);
            input.submit(cx);
        });
    });
    cx.run_until_parked();

    let sent: Vec<String> = commands
        .borrow()
        .iter()
        .filter_map(|command| match command {
            BridgeCommand::AgentSend { input, .. } => Some(input.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, ["ship it".to_owned()]);
    view.read_with(cx, |view, cx| {
        assert!(view.input().read(cx).text().is_empty());
        // The bubble is optimistic: it is on screen in the same frame the key was pressed.
        assert_eq!(count_user_rows(view), 1);
    });
}

#[gpui::test]
fn a_message_sent_while_a_turn_runs_is_dispatched_immediately(cx: &mut TestAppContext) {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut running = projection();
    running.items = vec![prompt.clone()];
    running.turns = vec![running_turn(turn, prompt.id)];
    running.turn = TurnState::Running(turn);
    running.session = SessionState::Running;

    let view = cx.new(|cx| AgentThreadView::new(running, cx));
    let commands = recorder(&view, cx);

    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| {
            input.set_text("also fix the tests", cx);
            input.submit(cx);
        });
    });
    cx.run_until_parked();

    // There is no queue and no queued row: the steer goes out now, marked as one.
    assert!(matches!(
        commands.borrow().as_slice(),
        [BridgeCommand::AgentSend { .. }]
    ));
    view.read_with(cx, |view, _| {
        let steered = view.rows().iter().any(|row| match &row.kind {
            TranscriptRowKind::User(row) => row.steered,
            _ => false,
        });
        assert!(steered);
    });
}

#[gpui::test]
fn streaming_a_delta_rewrites_one_row_and_reuses_every_other(cx: &mut TestAppContext) {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut base = projection();
    let mut prose = assistant(turn, "wri", ItemStatus::InProgress);
    base.items = vec![
        prompt.clone(),
        tool(turn, ToolKind::Read, ItemStatus::Completed, "a.rs"),
        prose.clone(),
    ];
    base.turns = vec![running_turn(turn, prompt.id)];
    base.turn = TurnState::Running(turn);
    base.session = SessionState::Running;
    base.last_seq = fleet_core::agents::Seq(9);

    let view = cx.new(|cx| AgentThreadView::new(base.clone(), cx));
    let before = view.read_with(cx, |view, _| view.rows().to_vec());

    let mut next = base.clone();
    prose.kind = fleet_core::agents::ItemKind::AssistantText {
        text: "writing it".to_owned(),
    };
    next.items[2] = prose;
    next.last_seq = fleet_core::agents::Seq(10);
    view.update(cx, |view, cx| view.sync(&next, cx));

    let after = view.read_with(cx, |view, _| view.rows().to_vec());
    assert_eq!(before.len(), after.len(), "no row was added or removed");
    let changed: Vec<usize> = before
        .iter()
        .zip(&after)
        .enumerate()
        .filter(|(_, (before, after))| before != after)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        changed.len(),
        1,
        "a stream chunk never re-runs grouping, folding or summarization"
    );
    let splice = fleet_ui_kit::diff_rows(&before, &after).expect("one row moved");
    assert_eq!(splice.count, 1);
}

#[gpui::test]
fn a_gate_moves_the_composer_mode_in_the_same_frame(cx: &mut TestAppContext) {
    let mut base = projection();
    base.last_seq = fleet_core::agents::Seq(1);
    let view = cx.new(|cx| AgentThreadView::new(base.clone(), cx));
    view.read_with(cx, |view, _| {
        assert_eq!(view.composer_mode(), ComposerMode::Normal);
    });

    let gate = permission_gate("git push --force", &[PermissionChoice::AllowOnce]);
    let id = gate.id;
    let mut next = base.clone();
    next.gates = vec![gate];
    next.last_seq = fleet_core::agents::Seq(2);
    view.update(cx, |view, cx| view.sync(&next, cx));

    // Derived from daemon state, not from the view, which is why it lands in this frame and not
    // one frame later.
    view.read_with(cx, |view, cx| {
        assert_eq!(view.composer_mode(), ComposerMode::Approval(id));
        assert!(
            view.input().read(cx).is_read_only(),
            "the editor is disabled so there is no ambiguity about who owns `y`"
        );
        assert_eq!(view.decisions().len(), 1);
    });
}

#[gpui::test]
fn a_bare_key_answers_the_open_approval_and_leaves_a_record(cx: &mut TestAppContext) {
    let mut base = projection();
    let gate = permission_gate("git push --force", &[PermissionChoice::AllowOnce]);
    let id = gate.id;
    base.gates = vec![gate];
    base.last_seq = fleet_core::agents::Seq(1);

    let view = cx.new(|cx| AgentThreadView::new(base.clone(), cx));
    let commands = recorder(&view, cx);
    view.update(cx, |view, cx| view.decide_key("y", cx));
    cx.run_until_parked();

    assert!(matches!(
        commands.borrow().as_slice(),
        [BridgeCommand::AgentRespond { gate, answer: GateAnswer::Permission { choice: PermissionChoice::AllowOnce, .. }, .. }] if *gate == id
    ));
    // A second keystroke while the reply is in flight must not dispatch the same answer twice.
    view.update(cx, |view, cx| view.decide_key("y", cx));
    cx.run_until_parked();
    assert_eq!(commands.borrow().len(), 1);

    // The daemon closes the gate; the transcript keeps the record that makes docking safe.
    let mut next = base.clone();
    next.gates.clear();
    next.last_seq = fleet_core::agents::Seq(2);
    view.update(cx, |view, cx| view.sync(&next, cx));
    view.read_with(cx, |view, _| {
        let record = view.rows().iter().find_map(|row| match &row.kind {
            TranscriptRowKind::Gate(row) => Some(row.label.to_string()),
            _ => None,
        });
        assert_eq!(record.as_deref(), Some("allowed once"));
    });
}

#[gpui::test]
fn enter_on_an_approval_reaches_the_composer_and_never_the_gate(cx: &mut TestAppContext) {
    let mut base = projection();
    base.gates = vec![permission_gate("rm -rf /", &[PermissionChoice::AllowOnce])];
    let view = cx.new(|cx| AgentThreadView::new(base, cx));
    let commands = recorder(&view, cx);

    view.update(cx, |view, cx| view.decide_key("enter", cx));
    cx.run_until_parked();

    assert!(
        commands.borrow().is_empty(),
        "a queued Return keystroke must never approve a shell command"
    );
}

#[gpui::test]
fn edit_seeds_the_composer_and_stands_the_gates_keys_down(cx: &mut TestAppContext) {
    let mut base = projection();
    base.gates = vec![permission_gate(
        "git push --force",
        &[PermissionChoice::AllowOnce, PermissionChoice::Edit],
    )];
    let view = cx.new(|cx| AgentThreadView::new(base, cx));
    let commands = recorder(&view, cx);

    view.update(cx, |view, cx| view.decide_key("e", cx));
    cx.run_until_parked();
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input().read(cx).text(), "git push --force");
        assert!(
            view.is_composing(cx),
            "the draft may start with a y, so the gate's context stands down"
        );
        assert_eq!(view.composer_mode(), ComposerMode::Normal);
    });

    // `⏎` then allows the **corrected** invocation and only the corrected one.
    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| {
            input.set_text("git push", cx);
            input.submit(cx);
        });
    });
    cx.run_until_parked();
    assert!(matches!(
        commands.borrow().as_slice(),
        [BridgeCommand::AgentRespond {
            answer: GateAnswer::Permission {
                choice: PermissionChoice::Edit,
                edited_payload: Some(payload),
            },
            ..
        }] if payload == "git push"
    ));
}

#[gpui::test]
fn the_escape_cascade_runs_in_order(cx: &mut TestAppContext) {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut running = projection();
    running.items = vec![
        prompt.clone(),
        command(turn, ItemStatus::InProgress, "cargo test"),
    ];
    running.turns = vec![running_turn(turn, prompt.id)];
    running.turn = TurnState::Running(turn);
    running.session = SessionState::Running;

    let view = cx.new(|cx| AgentThreadView::new(running, cx));
    let commands = recorder(&view, cx);

    // Scroll mode first: a mode that freezes the tail is left again, never a one-way door.
    view.update(cx, |view, cx| view.toggle_scroll_mode(cx));
    view.update(cx, |view, cx| view.stop(cx));
    view.read_with(cx, |view, _| assert!(!view.is_scrolling()));
    assert!(
        commands.borrow().is_empty(),
        "esc left the mode, not the turn"
    );

    // Then the interrupt, exactly once however many times the key repeats.
    view.update(cx, |view, cx| view.stop(cx));
    view.update(cx, |view, cx| view.stop(cx));
    cx.run_until_parked();
    assert!(matches!(
        commands.borrow().as_slice(),
        [BridgeCommand::AgentInterrupt { .. }]
    ));
    view.read_with(cx, |view, _| assert!(view.is_stopping()));
}

#[gpui::test]
fn escape_on_an_idle_thread_interrupts_nothing(cx: &mut TestAppContext) {
    // `esc` is unbound in `AgentIdle`, so it reaches the composer and comes back as an event.
    // With nothing running, asking the daemon to interrupt earns a conflict for a key that
    // meant "never mind".
    let view = cx.new(|cx| AgentThreadView::new(projection(), cx));
    let commands = recorder(&view, cx);

    view.update(cx, |view, cx| view.stop(cx));
    cx.run_until_parked();

    assert!(commands.borrow().is_empty());
}

#[gpui::test]
fn stopping_is_held_until_the_daemon_reports_liveness_cleared(cx: &mut TestAppContext) {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut running = projection();
    running.items = vec![prompt.clone()];
    running.turns = vec![running_turn(turn, prompt.id)];
    running.turn = TurnState::Running(turn);
    running.session = SessionState::Running;
    running.last_seq = fleet_core::agents::Seq(4);

    let view = cx.new(|cx| AgentThreadView::new(running.clone(), cx));
    view.update(cx, |view, cx| view.stop(cx));
    view.read_with(cx, |view, _| assert!(view.is_stopping()));

    // A projection that still reports the turn running keeps `stopping…` on.
    let mut same = running.clone();
    same.last_seq = fleet_core::agents::Seq(5);
    view.update(cx, |view, cx| view.sync(&same, cx));
    view.read_with(cx, |view, _| assert!(view.is_stopping()));

    let mut settled = running;
    settled.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Interrupted)];
    settled.turn = TurnState::Settled(turn, TurnOutcome::Interrupted);
    settled.session = SessionState::Ready;
    settled.last_seq = fleet_core::agents::Seq(6);
    view.update(cx, |view, cx| view.sync(&settled, cx));
    view.read_with(cx, |view, _| assert!(!view.is_stopping()));
}

#[gpui::test]
fn scroll_mode_focuses_a_row_and_g_does_not_re_arm_follow(cx: &mut TestAppContext) {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut base = projection();
    base.items = vec![
        prompt.clone(),
        tool(turn, ToolKind::Read, ItemStatus::Completed, "a.rs"),
        assistant(turn, "done", ItemStatus::Completed),
    ];
    base.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Completed)];
    base.turn = TurnState::Settled(turn, TurnOutcome::Completed);

    let view = cx.new(|cx| AgentThreadView::new(base, cx));
    view.update(cx, |view, cx| view.toggle_scroll_mode(cx));
    view.read_with(cx, |view, cx| {
        assert!(view.is_scrolling());
        assert!(!view.transcript().read(cx).is_following());
    });

    // `G` is documented as "newest", not as a way out of the mode.
    view.update(cx, |view, cx| view.scroll_to_bottom(cx));
    view.read_with(cx, |view, cx| {
        assert!(view.transcript().read(cx).is_scroll_mode());
        assert!(!view.transcript().read(cx).is_following());
    });

    view.update(cx, |view, cx| view.set_scroll_mode(false, cx));
    view.read_with(cx, |view, cx| {
        assert!(view.transcript().read(cx).is_following());
        assert_eq!(view.transcript().read(cx).focused_row(), None);
    });
}

#[gpui::test]
fn toggling_a_row_reuses_every_row_it_did_not_touch(cx: &mut TestAppContext) {
    let turn = TurnId::new();
    let prompt = user(turn, "go");
    let mut base = projection();
    let long = {
        let mut entry = command(turn, ItemStatus::Completed, "cargo test --workspace");
        if let fleet_core::agents::ItemKind::Tool(call) = &mut entry.kind {
            call.output = "ok\n".repeat(20);
        }
        entry
    };
    base.items = vec![prompt.clone(), long.clone()];
    base.turns = vec![running_turn(turn, prompt.id)];

    let view = cx.new(|cx| AgentThreadView::new(base, cx));
    let before = view.read_with(cx, |view, _| view.rows().to_vec());
    let key = TranscriptRowId::Item(gpui::SharedString::from(long.id.to_string())).key();
    view.update(cx, |view, cx| view.toggle_row(key, cx));
    let after = view.read_with(cx, |view, _| view.rows().to_vec());

    assert_eq!(before.len(), after.len());
    let splice = fleet_ui_kit::diff_rows(&before, &after).expect("the row changed");
    assert_eq!(
        splice.count, 1,
        "the group header above it kept its identity"
    );
}

#[gpui::test]
fn a_question_binds_the_composer_to_the_active_answer(cx: &mut TestAppContext) {
    let mut base = projection();
    base.gates = vec![question_gate(vec![question(
        "pm",
        "which package manager?",
        &["pnpm"],
        false,
        true,
    )])];
    let view = cx.new(|cx| AgentThreadView::new(base, cx));
    let commands = recorder(&view, cx);

    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| input.set_text("bun", cx));
        // `MultilineInput::set_text` is the programmatic setter and reports no change; a real
        // keystroke goes through `edit`, which emits `Changed` and lands on this handler.
        view.on_composer_changed(cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, cx| {
        assert!(
            view.is_composing(cx),
            "a free-text answer stands the gate's keys down"
        );
    });

    view.update(cx, |view, cx| view.send(cx));
    cx.run_until_parked();
    assert!(matches!(
        commands.borrow().as_slice(),
        [BridgeCommand::AgentRespond {
            answer: GateAnswer::Question { answers },
            ..
        }] if answers == &vec![vec!["bun".to_owned()]]
    ));
}

#[gpui::test]
fn plan_mode_takes_effect_at_the_next_send_and_restores_the_base_mode(cx: &mut TestAppContext) {
    let mut base = projection();
    base.mode = fleet_core::agents::PermissionMode::FullAccess;
    let view = cx.new(|cx| AgentThreadView::new(base, cx));
    let commands = recorder(&view, cx);

    // Nothing applies mid-turn: `⇧⇥` writes to the draft and reaches the daemon on send.
    view.update(cx, |view, cx| view.toggle_plan_mode(cx));
    cx.run_until_parked();
    assert!(commands.borrow().is_empty());

    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| {
            input.set_text("plan it", cx);
            input.submit(cx);
        });
    });
    cx.run_until_parked();
    let modes: Vec<fleet_core::agents::PermissionMode> = commands
        .borrow()
        .iter()
        .filter_map(|command| match command {
            BridgeCommand::AgentSetMode { mode, .. } => Some(*mode),
            _ => None,
        })
        .collect();
    assert_eq!(modes, [fleet_core::agents::PermissionMode::Plan]);

    // Leaving plan mode restores the **base** ladder, not a hardcoded default.
    commands.borrow_mut().clear();
    view.update(cx, |view, cx| view.toggle_plan_mode(cx));
    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| {
            input.set_text("build it", cx);
            input.submit(cx);
        });
    });
    cx.run_until_parked();
    let modes: Vec<fleet_core::agents::PermissionMode> = commands
        .borrow()
        .iter()
        .filter_map(|command| match command {
            BridgeCommand::AgentSetMode { mode, .. } => Some(*mode),
            _ => None,
        })
        .collect();
    assert!(modes.is_empty(), "FullAccess is what the thread already is");
}

#[gpui::test]
fn an_optimistic_bubble_is_reconciled_by_the_projection(cx: &mut TestAppContext) {
    // This is the only view test that reaches a running turn, so it is the only one whose
    // transcript starts the working row's clock — and that reads `motion.working_tick`.
    cx.update(|cx| fleet_ui_kit::Theme::init(fleet_ui_kit::ThemeMode::Dark, cx));
    let mut base = projection();
    base.last_seq = fleet_core::agents::Seq(1);
    let view = cx.new(|cx| AgentThreadView::new(base.clone(), cx));

    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| {
            input.set_text("ship it", cx);
            input.submit(cx);
        });
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_eq!(count_user_rows(view), 1, "the bubble is on screen now");
    });

    let turn = TurnId::new();
    let mut next = base;
    let echoed = user(turn, "ship it");
    next.items = vec![echoed.clone()];
    next.turns = vec![running_turn(turn, echoed.id)];
    next.turn = TurnState::Running(turn);
    next.last_seq = fleet_core::agents::Seq(2);
    view.update(cx, |view, cx| view.sync(&next, cx));

    view.read_with(cx, |view, _| {
        assert_eq!(
            count_user_rows(view),
            1,
            "the daemon's copy replaces the bubble rather than doubling it"
        );
    });
}

fn count_user_rows(view: &AgentThreadView) -> usize {
    view.rows()
        .iter()
        .filter(|row| matches!(row.kind, TranscriptRowKind::User(_)))
        .count()
}

#[gpui::test]
fn closing_a_tab_releases_the_whole_entity_graph(cx: &mut TestAppContext) {
    let view = cx.new(|cx| AgentThreadView::new(projection(), cx));
    let transcript = view.read_with(cx, |view, _| view.transcript().downgrade());
    let input = view.read_with(cx, |view, _| view.input().downgrade());
    let weak = view.downgrade();

    drop(view);
    cx.run_until_parked();
    // Resources are retained until the end of the effect cycle, so one empty update flushes it.
    cx.update(|_| {});

    weak.assert_released();
    transcript.assert_released();
    input.assert_released();
}

#[gpui::test]
fn a_composer_that_cannot_reach_its_machine_keeps_the_draft(cx: &mut TestAppContext) {
    use crate::screens::agent_thread::ThreadHost;

    let view = cx.new(|cx| AgentThreadView::new(projection(), cx));
    let commands = recorder(&view, cx);
    view.update(cx, |view, cx| {
        view.set_host(
            Some(ThreadHost {
                name: gpui::SharedString::new_static("dev-box"),
                unreachable: true,
            }),
            cx,
        );
    });

    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| {
            input.set_text("ship it", cx);
            input.submit(cx);
        });
    });
    cx.run_until_parked();

    assert!(commands.borrow().is_empty());
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.input().read(cx).text(),
            "ship it",
            "the message survives until the link does come back"
        );
    });
}

#[gpui::test]
fn a_thread_switch_is_the_only_thing_that_resets_the_list(cx: &mut TestAppContext) {
    let view = cx.new(|cx| AgentThreadView::new(projection(), cx));
    let first = view.read_with(cx, |view, _| view.thread());

    let other = projection();
    let second = other.thread;
    view.update(cx, |view, cx| view.set_projection(other, cx));

    assert_ne!(first, second);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.thread(), second);
        assert!(view.transcript().read(cx).is_following());
    });
}

/// `[u]` is drawn from the daemon's own listing, and only for what it can actually revert.
///
/// Turn scope is the whole worktree before the turn was submitted, which is what the footer's
/// verb promises. File scope names a turn rather than the edit it covered, so it has nothing for
/// a tool row to key on and is deliberately not adopted (§13).
#[gpui::test]
fn the_turn_footer_draws_revert_only_from_a_turn_scope_checkpoint(cx: &mut TestAppContext) {
    let user_item = fleet_core::agents::ItemId::new();
    let turn = TurnId::new();
    let mut projection = projection();
    projection.turns = vec![settled_turn(turn, user_item, TurnOutcome::Completed)];
    projection.items = vec![user(turn, "ship it")];
    projection.turn = TurnState::Settled(turn, TurnOutcome::Completed);
    let view = cx.new(|cx| AgentThreadView::new(projection, cx));

    // Nothing listed: the affordance is absent rather than drawn and inert.
    assert!(!cx.update(|cx| footer_has_revert(&view, cx)));

    // A file-scope checkpoint alone changes nothing.
    view.update(cx, |view, cx| {
        view.install_checkpoints(
            &[checkpoint(
                1,
                fleet_proto::agents::CheckpointScope::File,
                turn,
            )],
            cx,
        );
    });
    assert!(!cx.update(|cx| footer_has_revert(&view, cx)));

    // A turn-scope one draws it.
    view.update(cx, |view, cx| {
        view.install_checkpoints(
            &[
                checkpoint(1, fleet_proto::agents::CheckpointScope::File, turn),
                checkpoint(2, fleet_proto::agents::CheckpointScope::Turn, turn),
            ],
            cx,
        );
    });
    assert!(cx.update(|cx| footer_has_revert(&view, cx)));

    // And `[u]` sends the id the listing named, not a turn identifier.
    let commands = recorder(&view, cx);
    view.update(cx, |view, cx| {
        let key = footer_key(view).unwrap_or_else(|| panic!("a footer row"));
        view.row_action(key, fleet_ui_kit::RowAction::Revert, cx);
    });
    cx.run_until_parked();
    let reverted: Vec<fleet_proto::agents::CheckpointId> = commands
        .borrow()
        .iter()
        .filter_map(|command| match command {
            BridgeCommand::AgentRevert { checkpoint, .. } => Some(checkpoint.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        reverted,
        vec![fleet_proto::agents::CheckpointId::from_parts(
            2,
            fleet_proto::agents::CheckpointScope::Turn,
            turn
        )]
    );
}

fn checkpoint(
    ordinal: u32,
    scope: fleet_proto::agents::CheckpointScope,
    turn: TurnId,
) -> fleet_proto::agents::TurnCheckpoint {
    fleet_proto::agents::TurnCheckpoint {
        id: fleet_proto::agents::CheckpointId::from_parts(ordinal, scope, turn),
        scope,
        turn,
        ordinal,
        at: chrono::Utc::now(),
    }
}

fn footer_key(view: &AgentThreadView) -> Option<gpui::SharedString> {
    view.rows()
        .iter()
        .find_map(|row| matches!(row.kind, TranscriptRowKind::TurnFooter(_)).then(|| row.id.key()))
}

fn footer_has_revert(view: &gpui::Entity<AgentThreadView>, cx: &gpui::App) -> bool {
    view.read(cx).rows().iter().any(|row| match &row.kind {
        TranscriptRowKind::TurnFooter(footer) => footer.revert,
        _ => false,
    })
}
