//! The docked decision drawer: the priority ladder, the key vocabulary, and answer resolution.

use fleet_core::agents::{
    AgentKind, GateAnswer, ItemStatus, PermissionChoice, PermissionMode, TurnId, TurnOutcome,
    TurnState,
};
use fleet_ui_kit::{Decision, DecisionAction, DecisionKind, SOMETHING_ELSE};

use super::fixtures::{
    assistant, permission_gate, plan, plan_gate, projection, question, question_gate, settled_turn,
    user,
};
use crate::screens::agent_thread::decisions::{
    self, PLAN_IMPLEMENTATION_PROMPT_PREFIX, QuestionWizard, Routed, decision_context,
};

#[test]
fn enter_is_not_bound_on_an_approval() {
    let gate = permission_gate(
        "git push --force origin main",
        &[PermissionChoice::AllowOnce],
    );
    let decision = decisions::decision_for(&gate, AgentKind::Claude, &QuestionWizard::default());

    // The one property worth keeping exactly: a queued Return keystroke must never approve a
    // shell command.
    assert_eq!(decision.action_for_key("enter"), None);
    assert_eq!(decision.action_for_key("⏎"), None);
    assert_eq!(
        decision.action_for_key("y"),
        Some(DecisionAction::AllowOnce)
    );
    assert_eq!(
        decision.action_for_key("escape"),
        Some(DecisionAction::DenyAndStop)
    );
}

#[test]
fn an_approval_advertises_only_the_scopes_the_harness_can_honour() {
    // Claude offers an amended invocation; the session grant is only advertised when the
    // adapter mapped one, so `[a]` can never promise a scope that silently degrades.
    let claude = permission_gate(
        "rm -rf build",
        &[
            PermissionChoice::AllowOnce,
            PermissionChoice::AllowSession,
            PermissionChoice::Edit,
        ],
    );
    let decision = decisions::decision_for(&claude, AgentKind::Claude, &QuestionWizard::default());
    let keys: Vec<String> = decision
        .options()
        .into_iter()
        .map(|option| option.key.to_string())
        .collect();
    assert_eq!(keys, ["y", "a", "n", "e", "esc"]);
    assert_eq!(
        decision
            .options()
            .into_iter()
            .find(|option| option.key == "a")
            .map(|option| option.label.to_string()),
        Some("allow for this session".to_owned())
    );

    // Codex accepts no amended command, so `[e]` is absent rather than drawn and refused.
    let codex = permission_gate(
        "rm -rf build",
        &[PermissionChoice::AllowOnce, PermissionChoice::AllowSession],
    );
    let decision = decisions::decision_for(&codex, AgentKind::Codex, &QuestionWizard::default());
    let keys: Vec<String> = decision
        .options()
        .into_iter()
        .map(|option| option.key.to_string())
        .collect();
    assert_eq!(keys, ["y", "a", "n", "esc"]);
    assert_eq!(decision.action_for_key("e"), None);
}

#[test]
fn the_word_always_never_appears_on_a_command_approval() {
    for provider in [AgentKind::Claude, AgentKind::Codex] {
        let gate = permission_gate(
            "git push",
            &[PermissionChoice::AllowOnce, PermissionChoice::AllowSession],
        );
        let decision = decisions::decision_for(&gate, provider, &QuestionWizard::default());
        for option in decision.options() {
            assert!(
                !option.label.to_lowercase().contains("always"),
                "{provider:?} advertised `{}`",
                option.label
            );
        }
    }
}

#[test]
fn a_widened_grant_narrows_to_what_the_adapter_mapped() {
    // A harness that advertises no wider scope gets the narrow one rather than a promise the
    // wire cannot carry.
    let gate = permission_gate("git push", &[PermissionChoice::AllowOnce]);
    let mut wizard = QuestionWizard::default();
    let routed = decisions::route(&gate, &DecisionAction::AllowSession, &mut wizard, "");
    assert_eq!(
        routed,
        Routed::Answer(GateAnswer::Permission {
            choice: PermissionChoice::AllowOnce,
            edited_payload: None,
        })
    );
}

#[test]
fn edit_opens_the_composer_seeded_with_the_invocation() {
    let gate = permission_gate("git push --force", &[PermissionChoice::Edit]);
    let mut wizard = QuestionWizard::default();
    let routed = decisions::route(&gate, &DecisionAction::Edit, &mut wizard, "");

    // `[e]` *opens* the composer rather than answering at once, so the correction can start
    // with a `y` and contain spaces — and the seed is the invocation, never prose about it.
    assert_eq!(routed, Routed::Compose("git push --force".to_owned()));
}

#[test]
fn a_digit_the_question_does_not_offer_is_ignored() {
    let gate = question_gate(vec![question(
        "pm",
        "which package manager?",
        &["pnpm", "npm"],
        false,
        false,
    )]);
    let decision = decisions::decision_for(&gate, AgentKind::Claude, &QuestionWizard::default());

    assert_eq!(
        decision.action_for_key("2"),
        Some(DecisionAction::Choose(1))
    );
    // A `3` on a two-option question is ignored, never stored as an answer no label matches.
    assert_eq!(decision.action_for_key("3"), None);

    let mut wizard = QuestionWizard::new(1);
    assert_eq!(
        decisions::route(&gate, &DecisionAction::Choose(5), &mut wizard, ""),
        Routed::None
    );
}

#[test]
fn a_single_select_question_answers_with_the_option_label() {
    let gate = question_gate(vec![question(
        "pm",
        "which package manager?",
        &["pnpm", "npm"],
        false,
        false,
    )]);
    let mut wizard = QuestionWizard::new(1);

    assert_eq!(
        decisions::route(&gate, &DecisionAction::Choose(0), &mut wizard, ""),
        Routed::Local
    );
    assert_eq!(
        decisions::route(&gate, &DecisionAction::Answer, &mut wizard, ""),
        Routed::Answer(GateAnswer::Question {
            answers: vec![vec!["pnpm".to_owned()]],
        })
    );
}

#[test]
fn a_multi_select_question_returns_the_array_and_never_auto_advances() {
    let gate = question_gate(vec![question(
        "targets",
        "which targets?",
        &["lib", "bin", "tests"],
        true,
        false,
    )]);
    let mut wizard = QuestionWizard::new(1);

    decisions::route(&gate, &DecisionAction::Choose(0), &mut wizard, "");
    decisions::route(&gate, &DecisionAction::Choose(2), &mut wizard, "");
    assert_eq!(wizard.cursor(), 0, "multi-select waits for ⏎");
    assert_eq!(
        decisions::route(&gate, &DecisionAction::Answer, &mut wizard, ""),
        Routed::Answer(GateAnswer::Question {
            answers: vec![vec!["lib".to_owned(), "tests".to_owned()]],
        })
    );
}

#[test]
fn a_custom_answer_wins_over_a_selection_and_clears_it() {
    let gate = question_gate(vec![question(
        "pm",
        "which package manager?",
        &["pnpm"],
        false,
        true,
    )]);
    let mut wizard = QuestionWizard::new(1);
    decisions::route(&gate, &DecisionAction::Choose(0), &mut wizard, "");

    // Typing and selecting are mutually exclusive, enforced in the model: setting a non-empty
    // custom answer clears the selection.
    assert_eq!(
        decisions::route(&gate, &DecisionAction::Answer, &mut wizard, "bun"),
        Routed::Answer(GateAnswer::Question {
            answers: vec![vec!["bun".to_owned()]],
        })
    );
}

#[test]
fn whitespace_only_free_text_is_not_an_answer() {
    let gate = question_gate(vec![question("pm", "which one?", &["pnpm"], false, true)]);
    let mut wizard = QuestionWizard::new(1);

    // Nothing chosen and nothing but spaces typed: the drawer stays open rather than sending an
    // empty array the user was shown as a choice.
    assert_eq!(
        decisions::route(&gate, &DecisionAction::Answer, &mut wizard, "   "),
        Routed::Local
    );
}

#[test]
fn a_wizard_advances_on_enter_and_keeps_what_p_walks_back_to() {
    let gate = question_gate(vec![
        question("one", "first?", &["a", "b"], false, false),
        question("two", "second?", &["c", "d"], false, false),
    ]);
    let mut wizard = QuestionWizard::new(2);

    decisions::route(&gate, &DecisionAction::Choose(0), &mut wizard, "");
    assert_eq!(wizard.cursor(), 1, "single-select walks the wizard itself");
    decisions::route(&gate, &DecisionAction::Choose(1), &mut wizard, "");
    // Advance needs only the current question answered; submit needs all of them.
    assert_eq!(
        decisions::route(&gate, &DecisionAction::Previous, &mut wizard, ""),
        Routed::Local
    );
    assert_eq!(wizard.cursor(), 0);
    assert_eq!(
        decisions::route(&gate, &DecisionAction::Answer, &mut wizard, ""),
        Routed::Local,
        "⏎ on a non-final question advances rather than submitting"
    );
    assert_eq!(
        decisions::route(&gate, &DecisionAction::Answer, &mut wizard, ""),
        Routed::Answer(GateAnswer::Question {
            answers: vec![vec!["a".to_owned()], vec!["d".to_owned()]],
        }),
        "pressing p never lost a given answer"
    );
}

#[test]
fn a_question_that_accepts_free_text_offers_the_row_and_a_digit_for_it() {
    let gate = question_gate(vec![question("pm", "which one?", &["pnpm"], false, true)]);
    let decision = decisions::decision_for(&gate, AgentKind::Claude, &QuestionWizard::default());
    let DecisionKind::Question(set) = &decision.kind else {
        panic!("a question gate is a question decision");
    };
    let labels: Vec<String> = set
        .active()
        .expect("one question")
        .options
        .iter()
        .map(|option| option.label.to_string())
        .collect();
    assert_eq!(labels, ["pnpm".to_owned(), SOMETHING_ELSE.to_owned()]);
    assert_eq!(
        decision.action_for_key("2"),
        Some(DecisionAction::Choose(1))
    );
}

#[test]
fn the_priority_ladder_is_approval_then_question_then_plan() {
    let mut projection = projection();
    projection.gates = vec![
        plan_gate("# Plan\n\nsteps"),
        question_gate(vec![question("q", "which?", &["a"], false, false)]),
        permission_gate("git push", &[PermissionChoice::AllowOnce]),
    ];
    let pending = decisions::decisions(&projection, &QuestionWizard::default(), None);

    assert_eq!(pending.len(), 3);
    let head = Decision::head(&pending).expect("a head");
    assert!(matches!(head.kind, DecisionKind::Approval(_)));
    // Only the head is actionable; the rest are a `1/N` counter.
    assert_eq!(head.queued, 3);
}

#[test]
fn a_reply_in_flight_claims_no_key_at_all() {
    let mut projection = projection();
    let gate = permission_gate("git push", &[PermissionChoice::AllowOnce]);
    let id = gate.id;
    projection.gates = vec![gate];
    let pending = decisions::decisions(&projection, &QuestionWizard::default(), Some(id));
    let head = Decision::head(&pending).expect("a head");

    assert!(head.answering);
    assert_eq!(head.action_for_key("y"), None);
}

#[test]
fn a_gate_names_the_decision_context_that_owns_the_keys() {
    assert_eq!(
        decision_context(&permission_gate("x", &[PermissionChoice::AllowOnce])),
        "AgentPermission"
    );
    assert_eq!(
        decision_context(&question_gate(vec![question(
            "q",
            "which?",
            &["a"],
            false,
            false
        )])),
        "AgentQuestion"
    );
    assert_eq!(decision_context(&plan_gate("# p")), "AgentPlan");
}

#[test]
fn a_plan_the_harness_produced_as_an_item_gets_the_composer_verbs() {
    let turn = TurnId::new();
    let prompt = user(turn, "plan it");
    let mut projection = projection();
    projection.mode = PermissionMode::Plan;
    projection.items = vec![prompt.clone(), plan(turn, "# Fix rounding\n\nstep one")];
    projection.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Completed)];
    projection.turn = TurnState::Settled(turn, TurnOutcome::Completed);

    let (_, markdown) = decisions::plan_item(&projection).expect("the plan is actionable");
    let pending = decisions::decisions(&projection, &QuestionWizard::default(), None);
    let head = Decision::head(&pending).expect("the drawer shows the reminder");
    assert!(matches!(head.kind, DecisionKind::PlanReady { .. }));

    // Empty composer implements; a non-empty one refines, in plan mode, with the user's text.
    assert_eq!(
        decisions::route_plan_item(&DecisionAction::Implement, &markdown, ""),
        Routed::Send {
            text: format!("{PLAN_IMPLEMENTATION_PROMPT_PREFIX}{markdown}"),
            plan_mode: false,
        }
    );
    assert_eq!(
        decisions::route_plan_item(&DecisionAction::Implement, &markdown, "narrow it"),
        Routed::Send {
            text: "narrow it".to_owned(),
            plan_mode: true,
        }
    );
    assert_eq!(
        decisions::route_plan_item(&DecisionAction::Refine, &markdown, ""),
        Routed::Compose(String::new()),
        "[n] opens the composer rather than answering at once"
    );
}

#[test]
fn a_plan_a_later_message_answered_is_no_longer_actionable() {
    let turn = TurnId::new();
    let prompt = user(turn, "plan it");
    let mut projection = projection();
    projection.mode = PermissionMode::Plan;
    projection.items = vec![
        prompt.clone(),
        plan(turn, "# Fix rounding\n\nstep one"),
        // Rejection is implicit: type something else.
        user(turn, "do something else"),
        assistant(turn, "ok", ItemStatus::Completed),
    ];
    projection.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Completed)];
    projection.turn = TurnState::Settled(turn, TurnOutcome::Completed);

    assert!(decisions::plan_item(&projection).is_none());
}

#[test]
fn a_plan_outside_plan_mode_draws_no_reminder() {
    let turn = TurnId::new();
    let prompt = user(turn, "plan it");
    let mut projection = projection();
    projection.mode = PermissionMode::Ask;
    projection.items = vec![prompt.clone(), plan(turn, "# Fix rounding")];
    projection.turns = vec![settled_turn(turn, prompt.id, TurnOutcome::Completed)];

    assert!(decisions::plan_item(&projection).is_none());
}

#[test]
fn a_plan_gate_implements_on_the_wire_and_refines_through_the_composer() {
    let gate = plan_gate("# Fix rounding\n\nstep one");
    let mut wizard = QuestionWizard::default();

    assert_eq!(
        decisions::route(&gate, &DecisionAction::Implement, &mut wizard, ""),
        Routed::Answer(GateAnswer::Plan(fleet_core::agents::PlanAnswer::Approve))
    );
    assert_eq!(
        decisions::route(&gate, &DecisionAction::Refine, &mut wizard, ""),
        Routed::Compose(String::new())
    );
    assert_eq!(
        decisions::route(&gate, &DecisionAction::Refine, &mut wizard, "smaller steps"),
        Routed::Answer(GateAnswer::Plan(
            fleet_core::agents::PlanAnswer::AskForChanges {
                note: "smaller steps".to_owned(),
            }
        ))
    );
}

#[test]
fn a_free_text_draft_stands_the_gates_keys_down() {
    let mut wizard = QuestionWizard::new(1);
    assert!(!wizard.is_composing());
    wizard.set_custom("bun");
    assert!(
        wizard.is_composing(),
        "the answer may start with a y, and space is one of the drawer's keys"
    );
    assert_eq!(wizard.custom(), "bun");
    wizard.set_custom("   ");
    assert!(!wizard.is_composing(), "whitespace is not a draft");
}
