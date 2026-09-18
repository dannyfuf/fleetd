//! The wiring only an entity graph can show: what a stream chunk splices, which frame a gate
//! takes the keyboard in, the `esc` cascade, scroll mode, and what a closed tab releases.

use std::{cell::RefCell, rc::Rc, time::Duration};

use fleet_core::agents::{
    AgentKind, Applied, GateAnswer, ItemStatus, ModelDescriptor, ModelSelection, PermissionChoice,
    ReasoningEffortDescriptor, SessionState, StreamKind, ToolKind, TurnId, TurnOutcome, TurnState,
};
use fleet_ui_kit::{TranscriptRowId, TranscriptRowKind};
use gpui::{AppContext as _, EntityInputHandler as _, TestAppContext};

use super::fixtures::{
    assistant, command, edit, permission_gate, projection, question, question_gate, running_turn,
    settled_turn, tool, user,
};
use crate::{
    bridge::BridgeCommand,
    screens::agent_thread::{
        AgentThreadEvent, AgentThreadView, composer::ComposerMode, picker::PickerKind,
    },
};

#[gpui::test]
fn codex_effort_picker_uses_discovered_descriptions_and_empty_catalogues_draw_nothing(
    cx: &mut TestAppContext,
) {
    let mut codex = projection();
    codex.provider = AgentKind::Codex;
    codex.model = Some(ModelSelection {
        model: "gpt-5.6-sol".to_owned(),
        effort: Some("high".to_owned()),
        provider: None,
    });
    codex.models = vec![ModelDescriptor {
        id: "gpt-5.6-sol".to_owned(),
        display_name: "GPT-5.6 Sol".to_owned(),
        efforts: vec![ReasoningEffortDescriptor {
            id: "xhigh".to_owned(),
            description: "Deep reasoning".to_owned(),
        }],
        default_effort: Some("high".to_owned()),
    }];
    let described = cx.new(|cx| AgentThreadView::new(codex, cx));
    described.update(cx, |view, cx| view.open_picker(PickerKind::Traits, cx));
    described.read_with(cx, |view, _| {
        let labels: Vec<&str> = view
            .picker
            .as_ref()
            .into_iter()
            .flat_map(|picker| picker.matches())
            .map(|candidate| candidate.label.as_str())
            .collect();
        assert_eq!(labels, ["xhigh · Deep reasoning"]);
    });

    let mut pick_default = projection();
    pick_default.provider = AgentKind::Codex;
    pick_default.model = Some(ModelSelection {
        model: "older-model".to_owned(),
        effort: Some("medium".to_owned()),
        provider: None,
    });
    pick_default.models = described.read_with(cx, |view, _| view.projection.models.clone());
    let pick_default = cx.new(|cx| AgentThreadView::new(pick_default, cx));
    pick_default.update(cx, |view, cx| {
        view.open_picker(PickerKind::Models, cx);
        view.send(cx);
    });
    pick_default.read_with(cx, |view, _| {
        let picked = view
            .controls
            .model()
            .unwrap_or_else(|| panic!("the discovered model should be selected"));
        assert_eq!(picked.model, "gpt-5.6-sol");
        assert_eq!(picked.effort.as_deref(), Some("high"));
    });

    let empty = cx.new(|cx| AgentThreadView::new(projection(), cx));
    empty.update(cx, |view, cx| view.open_picker(PickerKind::Traits, cx));
    empty.read_with(cx, |view, _| assert!(view.picker.is_none()));
}

#[gpui::test]
fn claude_effort_picker_uses_the_discovered_catalogue(cx: &mut TestAppContext) {
    let mut claude = projection();
    claude.provider = AgentKind::Claude;
    claude.model = Some(ModelSelection {
        model: "fable[1m]".to_owned(),
        effort: Some("high".to_owned()),
        provider: None,
    });
    claude.models = vec![ModelDescriptor {
        id: "fable[1m]".to_owned(),
        display_name: "Fable".to_owned(),
        efforts: ["low", "medium", "high", "xhigh", "max"]
            .into_iter()
            .map(|id| ReasoningEffortDescriptor {
                id: id.to_owned(),
                description: String::new(),
            })
            .collect(),
        default_effort: None,
    }];
    let view = cx.new(|cx| AgentThreadView::new(claude, cx));
    view.update(cx, |view, cx| view.open_picker(PickerKind::Traits, cx));
    view.read_with(cx, |view, _| {
        let labels = view
            .picker
            .as_ref()
            .into_iter()
            .flat_map(|picker| picker.matches())
            .map(|candidate| candidate.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["low", "medium", "high", "xhigh", "max"]);
    });
}

#[gpui::test]
fn claude_default_selector_keeps_the_effort_picker_populated_after_init(cx: &mut TestAppContext) {
    let mut claude = projection();
    claude.provider = AgentKind::Claude;
    claude.model = Some(ModelSelection {
        model: "default".to_owned(),
        effort: None,
        provider: None,
    });
    claude.models = vec![ModelDescriptor {
        id: "default".to_owned(),
        display_name: "Default (recommended)".to_owned(),
        efforts: ["low", "medium", "high", "xhigh", "max"]
            .into_iter()
            .map(|id| ReasoningEffortDescriptor {
                id: id.to_owned(),
                description: String::new(),
            })
            .collect(),
        default_effort: None,
    }];

    let view = cx.new(|cx| AgentThreadView::new(claude, cx));
    view.update(cx, |view, cx| view.open_picker(PickerKind::Traits, cx));
    view.read_with(cx, |view, _| {
        let labels = view
            .picker
            .as_ref()
            .into_iter()
            .flat_map(|picker| picker.matches())
            .map(|candidate| candidate.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["low", "medium", "high", "xhigh", "max"]);
    });
}

#[gpui::test]
fn access_picker_follows_harness_order_and_plan_uses_interaction_state(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let view = cx.new(|cx| AgentThreadView::new(projection(), cx));
    let commands = recorder(&view, cx);
    view.update(cx, |view, cx| {
        view.set_modes(AgentKind::Claude.supported_modes().to_vec());
        view.open_picker(PickerKind::Access, cx);
    });
    view.read_with(cx, |view, _| {
        let labels = view
            .picker
            .as_ref()
            .into_iter()
            .flat_map(|picker| picker.matches())
            .map(|candidate| candidate.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [
                "accepts edits",
                "plans before editing",
                "auto-approves safe actions",
                "denies unlisted tools",
                "full access",
            ]
        );
    });

    view.update(cx, |view, cx| {
        view.pick_access("plans before editing", cx);
        let input = view.input().clone();
        input.update(cx, |input, cx| {
            input.set_text("plan it", cx);
            input.submit(cx);
        });
    });
    cx.run_until_parked();
    assert!(commands.borrow().iter().any(|command| matches!(
        command,
        BridgeCommand::AgentSetMode {
            mode: fleet_core::agents::PermissionMode::Plan,
            ..
        }
    )));
}

#[gpui::test]
fn decision_observable_comes_from_the_prepared_drawer_and_joined_item(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let turn = TurnId::new();
    let mut changed = edit(turn, ItemStatus::InProgress, "README.md");
    if let fleet_core::agents::ItemKind::Tool(call) = &mut changed.kind {
        call.input = serde_json::json!({"paths": ["README.md"]});
    }
    let mut gate = permission_gate("apply README.md", &[PermissionChoice::AllowOnce]);
    if let fleet_core::agents::GateKind::Permission { item, .. } = &mut gate.kind {
        *item = Some(changed.id);
    }
    let mut projection = projection();
    projection.provider = AgentKind::Codex;
    projection.items = vec![changed];
    projection.gates = vec![gate];

    let view = cx.new(|cx| AgentThreadView::new(projection, cx));
    view.read_with(cx, |view, _| {
        let decision = view
            .decision_observable()
            .unwrap_or_else(|| panic!("the prepared decision should be observable"));
        assert_eq!(decision.kind, "permission");
        assert_eq!(decision.paths, ["README.md"]);
        assert!(decision.has_diff);
        assert!(decision.title.contains("codex"));
    });
}

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
fn send_actions_do_not_submit_an_open_ime_composition(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let (view, visual) = cx.add_window_view(|_, cx| AgentThreadView::new(projection(), cx));

    visual.update(|window, cx| {
        let input = view.read(cx).input().clone();
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "漢", Some(1..1), window, cx);
        });
        view.update(cx, |view, cx| {
            view.send(cx);
            view.send_background(cx);
        });
    });
    cx.run_until_parked();

    view.read_with(cx, |view, cx| {
        assert_eq!(view.input().read(cx).text(), "漢");
        assert!(view.input().read(cx).is_composing());
        assert_eq!(count_user_rows(view), 0);
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
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        cx.set_reduce_motion(true);
    });
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
    let applied = Applied::Text {
        item: next.items[2].id,
        stream: StreamKind::AssistantText,
        appended: 3..10,
    };
    view.update(cx, |view, cx| view.sync(&next, &applied, cx));

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
fn reveal_buffer_advances_on_the_theme_tick_and_finishes_within_one_horizon(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
    let turn = TurnId::new();
    let prose = assistant(turn, "a", ItemStatus::InProgress);
    let item = prose.id;
    let mut base = projection();
    base.items = vec![prose];
    base.last_seq = fleet_core::agents::Seq(1);
    let view = cx.new(|cx| AgentThreadView::new(base.clone(), cx));

    let mut next = base;
    next.items[0].kind = fleet_core::agents::ItemKind::AssistantText {
        text: "abcdefghijklmn".to_owned(),
    };
    next.last_seq = fleet_core::agents::Seq(2);
    let applied = Applied::Text {
        item,
        stream: StreamKind::AssistantText,
        appended: 1..14,
    };
    view.update(cx, |view, cx| view.sync(&next, &applied, cx));
    view.read_with(cx, |view, _| {
        let TranscriptRowKind::Assistant(row) = &view.rows()[0].kind else {
            panic!("the stream must remain an assistant row");
        };
        assert_eq!(*row.markdown, fleet_ui_kit::parse_markdown_document("a"));
    });

    cx.executor().advance_clock(Duration::from_millis(16));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        let TranscriptRowKind::Assistant(row) = &view.rows()[0].kind else {
            panic!("the stream must remain an assistant row");
        };
        assert_eq!(*row.markdown, fleet_ui_kit::parse_markdown_document("abc"));
    });

    for _ in 0..12 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
    }
    view.read_with(cx, |view, _| {
        let TranscriptRowKind::Assistant(row) = &view.rows()[0].kind else {
            panic!("the stream must remain an assistant row");
        };
        assert_eq!(
            *row.markdown,
            fleet_ui_kit::parse_markdown_document("abcdefghijklmn")
        );
    });
}

#[gpui::test]
fn streaming_command_output_patches_its_row_without_reallocating_the_row_slice(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        cx.set_reduce_motion(true);
    });
    let turn = TurnId::new();
    let mut command = command(turn, ItemStatus::InProgress, "cargo test");
    if let fleet_core::agents::ItemKind::Tool(call) = &mut command.kind {
        call.output = "running crate tests".to_owned();
    }
    let mut base = projection();
    base.items = vec![command.clone()];
    base.last_seq = fleet_core::agents::Seq(20);

    let view = cx.new(|cx| AgentThreadView::new(base.clone(), cx));
    let before = view.read_with(cx, |view, _| view.rows().as_ptr());

    let mut next = base;
    if let fleet_core::agents::ItemKind::Tool(call) = &mut command.kind {
        call.output.push_str("\nall green");
    }
    next.items[0] = command;
    next.last_seq = fleet_core::agents::Seq(21);
    let applied = Applied::Text {
        item: next.items[0].id,
        stream: StreamKind::CommandOutput,
        appended: 19..29,
    };
    view.update(cx, |view, cx| view.sync(&next, &applied, cx));

    view.read_with(cx, |view, _| {
        assert_eq!(view.rows().as_ptr(), before);
        let TranscriptRowKind::Work(row) = &view.rows()[0].kind else {
            panic!("the command remains one work row");
        };
        assert!(
            row.body
                .as_ref()
                .is_some_and(|body| body.contains("all green"))
        );
    });
}

#[gpui::test]
fn command_output_hidden_by_the_live_row_skips_a_structural_rebuild(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(fleet_ui_kit::Theme::dark());
        cx.set_reduce_motion(true);
    });
    let turn = TurnId::new();
    let prompt = user(turn, "run it");
    let mut running = command(turn, ItemStatus::InProgress, "cargo test");
    let mut base = projection();
    base.items = vec![prompt.clone(), running.clone()];
    base.turns = vec![running_turn(turn, prompt.id)];
    base.turn = TurnState::Running(turn);
    base.session = SessionState::Running;
    base.last_seq = fleet_core::agents::Seq(30);

    let view = cx.new(|cx| AgentThreadView::new(base.clone(), cx));
    let before = view.read_with(cx, |view, _| view.rows().as_ptr());

    if let fleet_core::agents::ItemKind::Tool(call) = &mut running.kind {
        call.output = "one more crate passed".to_owned();
    }
    let mut next = base;
    next.items[1] = running;
    next.last_seq = fleet_core::agents::Seq(31);
    let applied = Applied::Text {
        item: next.items[1].id,
        stream: StreamKind::CommandOutput,
        appended: 0..21,
    };
    view.update(cx, |view, cx| view.sync(&next, &applied, cx));

    view.read_with(cx, |view, _| {
        assert_eq!(view.rows().as_ptr(), before);
        assert!(matches!(
            &view.rows()[1].kind,
            TranscriptRowKind::WorkLive(_)
        ));
    });
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
    view.update(cx, |view, cx| view.sync(&next, &Applied::Structural, cx));

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
    view.update(cx, |view, cx| view.sync(&next, &Applied::Structural, cx));
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
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
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
    view.update(cx, |view, cx| view.sync(&same, &Applied::Structural, cx));
    view.read_with(cx, |view, _| assert!(view.is_stopping()));

    let mut settled = running;
    settled.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Interrupted)];
    settled.turn = TurnState::Settled(turn, TurnOutcome::Interrupted);
    settled.session = SessionState::Ready;
    settled.last_seq = fleet_core::agents::Seq(6);
    view.update(cx, |view, cx| view.sync(&settled, &Applied::Structural, cx));
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
    cx.update(|cx| cx.set_global(fleet_ui_kit::Theme::dark()));
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
    view.update(cx, |view, cx| view.sync(&next, &Applied::Structural, cx));

    view.read_with(cx, |view, _| {
        assert_eq!(
            count_user_rows(view),
            1,
            "the daemon's copy replaces the bubble rather than doubling it"
        );
    });
}

/// The join is by id: a pasted code block after a blank line is stripped from the bubble's
/// display text, and a text join never matched the daemon's full copy — the bubble stayed
/// `sending`, the thread kept `working`, and every later send was refused as unacknowledged.
#[gpui::test]
fn an_optimistic_bubble_is_reconciled_by_id_even_when_its_display_text_differs(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| fleet_ui_kit::Theme::init(fleet_ui_kit::ThemeMode::Dark, cx));
    let mut base = projection();
    base.last_seq = fleet_core::agents::Seq(1);
    let view = cx.new(|cx| AgentThreadView::new(base.clone(), cx));
    let commands = recorder(&view, cx);
    let text = "fix this\n\n```rust\nfn a() {}\n```";

    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| {
            input.set_text(text, cx);
            input.submit(cx);
        });
    });
    cx.run_until_parked();
    let sent = commands
        .borrow()
        .iter()
        .find_map(|command| match command {
            BridgeCommand::AgentSend { input, .. } => Some(input.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the composer dispatched a send"));
    assert_eq!(sent.text, text, "the daemon receives the whole message");
    let item = sent
        .item
        .unwrap_or_else(|| panic!("the send carries the client-minted id"));
    view.read_with(cx, |view, _| {
        assert_eq!(count_user_rows(view), 1);
        assert!(view.is_working(), "an in-flight send is work");
    });

    let turn = TurnId::new();
    let mut next = base;
    let mut echoed = user(turn, text);
    echoed.id = item;
    next.items = vec![echoed];
    next.turns = vec![running_turn(turn, item)];
    next.turn = TurnState::Running(turn);
    next.last_seq = fleet_core::agents::Seq(2);
    view.update(cx, |view, cx| view.sync(&next, &Applied::Structural, cx));

    view.read_with(cx, |view, _| {
        assert_eq!(
            count_user_rows(view),
            1,
            "the daemon's copy replaces the bubble by id rather than doubling it"
        );
        assert_eq!(view.pending_in_flight(), 0);
    });
}

/// A refused send marks its bubble failed and frees the composer: the thread stops reading
/// `working`, the reason is said once, and the next send goes out instead of being refused as
/// unacknowledged.
#[gpui::test]
fn a_refused_send_fails_its_bubble_and_frees_the_composer(cx: &mut TestAppContext) {
    cx.update(|cx| fleet_ui_kit::Theme::init(fleet_ui_kit::ThemeMode::Dark, cx));
    let view = cx.new(|cx| AgentThreadView::new(projection(), cx));
    let commands = recorder(&view, cx);
    let notices: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&notices);
    cx.update(|cx| {
        cx.subscribe(&view, move |_, event, _| {
            if let AgentThreadEvent::Notice(text) = event {
                seen.borrow_mut().push(text.to_string());
            }
        })
        .detach();
    });

    let submit = |text: &str, cx: &mut TestAppContext| {
        view.update(cx, |view, cx| {
            let input = view.input().clone();
            input.update(cx, |input, cx| {
                input.set_text(text, cx);
                input.submit(cx);
            });
        });
        cx.run_until_parked();
    };
    submit("first", cx);
    let item = commands
        .borrow()
        .iter()
        .find_map(|command| match command {
            BridgeCommand::AgentSend { input, .. } => input.item,
            _ => None,
        })
        .unwrap_or_else(|| panic!("the send carries the client-minted id"));

    // Before the failure lands, a second send is refused as unacknowledged and the draft is kept.
    submit("second", cx);
    assert_eq!(sends(&commands.borrow()), ["first".to_owned()]);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input().read(cx).text(), "second");
    });

    view.update(cx, |view, cx| {
        view.send_failed(item, "agent thread is not live", cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(!view.is_working(), "a failed send is not work");
        assert_eq!(view.pending_in_flight(), 0);
        let failed = view
            .rows()
            .iter()
            .filter(|row| {
                matches!(
                    &row.kind,
                    TranscriptRowKind::User(user) if user.state == fleet_ui_kit::UserRowState::Failed
                )
            })
            .count();
        assert_eq!(failed, 1, "the bubble stays on screen, drawn as failed");
    });
    assert!(
        notices
            .borrow()
            .iter()
            .any(|notice| notice.contains("agent thread is not live")),
        "{notices:?}"
    );

    // The composer still holds the refused draft; `⏎` now sends it.
    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| input.submit(cx));
    });
    cx.run_until_parked();
    assert_eq!(
        sends(&commands.borrow()),
        ["first".to_owned(), "second".to_owned()]
    );
}

fn sends(commands: &[BridgeCommand]) -> Vec<String> {
    commands
        .iter()
        .filter_map(|command| match command {
            BridgeCommand::AgentSend { input, .. } => Some(input.text.clone()),
            _ => None,
        })
        .collect()
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

/// Collects every `AgentThreadEvent` a view emits, including the ones that are not commands.
fn event_recorder(
    view: &gpui::Entity<AgentThreadView>,
    cx: &mut TestAppContext,
) -> Rc<RefCell<Vec<AgentThreadEvent>>> {
    let events: Rc<RefCell<Vec<AgentThreadEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&events);
    cx.update(|cx| {
        cx.subscribe(view, move |_, event: &AgentThreadEvent, _| {
            seen.borrow_mut().push(event.clone());
        })
        .detach();
    });
    events
}

/// The `/` rows a view offers, in order.
fn command_rows(view: &gpui::Entity<AgentThreadView>, cx: &mut TestAppContext) -> Vec<String> {
    view.update(cx, |view, cx| view.open_picker(PickerKind::Commands, cx));
    view.read_with(cx, |view, _| {
        view.picker
            .as_ref()
            .into_iter()
            .flat_map(|picker| picker.matches())
            .map(|candidate| candidate.label.to_string())
            .collect()
    })
}

/// `/login` and `/logout` are offered where they do something, and nowhere else.
#[gpui::test]
fn the_account_commands_are_listed_for_codex_and_not_for_claude(cx: &mut TestAppContext) {
    let mut codex = projection();
    codex.provider = AgentKind::Codex;
    let codex = cx.new(|cx| AgentThreadView::new(codex, cx));
    let rows = command_rows(&codex, cx);
    assert_eq!(
        rows,
        ["model", "plan", "default", "compact", "login", "logout"]
    );

    // Claude publishes no account surface, and DESIGN-SYSTEM §4 does not list a command that
    // would answer "unsupported".
    let claude = cx.new(|cx| AgentThreadView::new(projection(), cx));
    let rows = command_rows(&claude, cx);
    assert_eq!(rows, ["model", "plan", "default", "compact"]);
}

/// Accepting `/login` asks the workspace for the sign-in and deletes its own trigger text.
#[gpui::test]
fn accepting_login_requests_the_sign_in_and_clears_the_trigger(cx: &mut TestAppContext) {
    let mut codex = projection();
    codex.provider = AgentKind::Codex;
    let view = cx.new(|cx| AgentThreadView::new(codex, cx));
    let events = event_recorder(&view, cx);

    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| input.set_text("/login", cx));
        view.open_picker(PickerKind::Commands, cx);
        // `⏎` on the highlighted row, which is the one the query narrowed to first.
        view.send(cx);
    });
    cx.run_until_parked();

    assert!(
        events
            .borrow()
            .iter()
            .any(|event| matches!(event, AgentThreadEvent::AccountLogin)),
        "{:?}",
        events.borrow()
    );
    view.read_with(cx, |view, cx| {
        assert!(
            view.input().read(cx).text().is_empty(),
            "a built-in sends nothing, so its trigger text goes with it"
        );
    });
}

/// Accepting `/logout` dispatches the mutation and deletes its own trigger text.
#[gpui::test]
fn accepting_logout_dispatches_the_mutation_and_clears_the_trigger(cx: &mut TestAppContext) {
    let mut codex = projection();
    codex.provider = AgentKind::Codex;
    let thread = codex.thread;
    let view = cx.new(|cx| AgentThreadView::new(codex, cx));
    let commands = recorder(&view, cx);

    view.update(cx, |view, cx| {
        let input = view.input().clone();
        input.update(cx, |input, cx| input.set_text("/logout", cx));
        view.open_picker(PickerKind::Commands, cx);
        view.send(cx);
    });
    cx.run_until_parked();

    assert!(
        commands.borrow().iter().any(|command| matches!(
            command,
            BridgeCommand::AgentAccountLogout { thread: target } if *target == thread
        )),
        "{:?}",
        commands.borrow()
    );
    view.read_with(cx, |view, cx| {
        assert!(view.input().read(cx).text().is_empty());
    });
}
