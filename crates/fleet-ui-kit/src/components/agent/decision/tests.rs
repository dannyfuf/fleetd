//! The key vocabulary is the safety property, so it is tested exhaustively and without a
//! window: `spec-B` §B4.2 and §B4.3 are the source of every case here.

use super::*;

fn approval(allows_edit: bool) -> Decision {
    Decision::new(
        "gate-1",
        "claude wants to run a command",
        DecisionKind::Approval(
            ApprovalRequest::new("Bash", "git push --force origin main")
                .rationale("the branch diverged")
                .allows_edit(allows_edit),
        ),
    )
}

fn question(multi_select: bool, options: usize) -> Decision {
    let question = DecisionQuestion::new("package manager", "which package manager?")
        .options(
            (0..options)
                .map(|index| QuestionOption::new(format!("option {index}")))
                .collect(),
        )
        .multi_select(multi_select);
    Decision::new(
        "gate-2",
        "claude has a question",
        DecisionKind::Question(QuestionSet::new(vec![question])),
    )
}

fn wizard(cursor: usize) -> Decision {
    let questions = (0..3)
        .map(|index| {
            DecisionQuestion::new(format!("q{index}"), "?")
                .options(vec![QuestionOption::new("a"), QuestionOption::new("b")])
        })
        .collect();
    Decision::new(
        "gate-3",
        "claude has questions",
        DecisionKind::Question(QuestionSet::new(questions).cursor(cursor)),
    )
}

fn plan() -> Decision {
    Decision::new(
        "gate-4",
        "plan ready",
        DecisionKind::PlanReady {
            title: "fix the payroll rounding".into(),
            markdown: None,
        },
    )
}

/// The one t3code property worth keeping exactly: a queued Return keystroke must never approve
/// a shell command.
#[test]
fn enter_is_not_bound_on_an_approval() {
    let decision = approval(true);
    assert_eq!(decision.action_for_key("enter"), None);
    assert_eq!(decision.action_for_key("⏎"), None);
    assert_eq!(decision.action_for_key("return"), None);
}

#[test]
fn an_approval_answers_its_own_five_keys() {
    let decision = approval(true);
    assert_eq!(
        decision.action_for_key("y"),
        Some(DecisionAction::AllowOnce)
    );
    assert_eq!(
        decision.action_for_key("a"),
        Some(DecisionAction::AllowSession)
    );
    assert_eq!(decision.action_for_key("n"), Some(DecisionAction::Deny));
    assert_eq!(decision.action_for_key("e"), Some(DecisionAction::Edit));
    assert_eq!(
        decision.action_for_key("esc"),
        Some(DecisionAction::DenyAndStop)
    );
    assert_eq!(
        decision.action_for_key("escape"),
        Some(DecisionAction::DenyAndStop)
    );
    assert_eq!(decision.action_for_key("q"), None);
}

/// Codex accepts no amended invocation, so `[e]` is unbound *and* absent — never a listed key
/// that does nothing.
#[test]
fn edit_is_neither_bound_nor_drawn_when_the_harness_cannot_honour_it() {
    let decision = approval(false);
    assert_eq!(decision.action_for_key("e"), None);
    assert!(
        !decision
            .options()
            .iter()
            .any(|option| option.action == DecisionAction::Edit)
    );
    assert!(
        decision
            .key_hints()
            .pairs()
            .iter()
            .all(|(key, _)| key.as_ref() != "e")
    );
}

/// The word "always" never appears on a command or file-change approval, and no scope wider
/// than the session is offered in v1.
#[test]
fn no_approval_option_promises_more_than_a_session() {
    for option in approval(true).options() {
        let label = option.label.to_lowercase();
        assert!(!label.contains("always"), "{label:?} says always");
        assert!(!label.contains("director"), "{label:?} offers a directory");
        assert!(!label.contains("project"), "{label:?} offers a project");
    }
}

#[test]
fn question_digits_are_bounded_by_the_options_that_exist() {
    let decision = question(false, 2);
    assert_eq!(
        decision.action_for_key("1"),
        Some(DecisionAction::Choose(0))
    );
    assert_eq!(
        decision.action_for_key("2"),
        Some(DecisionAction::Choose(1))
    );
    // Ignored, not stored: there is no third label for it to mean.
    assert_eq!(decision.action_for_key("3"), None);
    assert_eq!(decision.action_for_key("0"), None);
}

#[test]
fn the_free_text_row_is_one_of_the_numbered_options() {
    let question = DecisionQuestion::new("scope", "which?")
        .options(vec![QuestionOption::new("a"), QuestionOption::new("b")])
        .allow_other(true);
    assert_eq!(question.option_count(), 3);
    let decision = Decision::new(
        "g",
        "t",
        DecisionKind::Question(QuestionSet::new(vec![question])),
    );
    assert_eq!(
        decision.action_for_key("3"),
        Some(DecisionAction::Choose(2))
    );
}

#[test]
fn a_question_never_advertises_a_row_past_the_last_bound_digit() {
    let question = DecisionQuestion::new("scope", "which?")
        .options(
            (0..9)
                .map(|index| QuestionOption::new(format!("o{index}")))
                .collect(),
        )
        .allow_other(true);
    assert_eq!(question.option_count(), MAX_QUESTION_OPTIONS);
}

#[test]
fn space_toggles_only_a_multi_select_question() {
    assert_eq!(
        question(true, 3).action_for_key("space"),
        Some(DecisionAction::Toggle)
    );
    assert_eq!(question(false, 3).action_for_key("space"), None);
}

#[test]
fn the_wizard_advances_and_only_offers_previous_once_there_is_one() {
    let first = wizard(0);
    assert_eq!(first.action_for_key("p"), None);
    let labels: Vec<_> = first
        .options()
        .into_iter()
        .map(|option| option.label)
        .collect();
    assert!(labels.iter().any(|label| label == "next"));

    let second = wizard(1);
    assert_eq!(second.action_for_key("p"), Some(DecisionAction::Previous));

    let last = wizard(2);
    assert!(
        last.options()
            .into_iter()
            .any(|option| option.label == "answer")
    );
}

#[test]
fn a_ready_plan_offers_only_implement_and_refine() {
    let decision = plan();
    assert_eq!(
        decision.action_for_key("y"),
        Some(DecisionAction::Implement)
    );
    assert_eq!(decision.action_for_key("n"), Some(DecisionAction::Refine));
    assert_eq!(decision.action_for_key("a"), None);
    assert_eq!(decision.options().len(), 2);
}

/// While a reply is in flight every option is disabled, so no key may fire twice.
#[test]
fn nothing_is_claimed_while_a_reply_is_in_flight() {
    let decision = approval(true).answering(true);
    for key in ["y", "a", "n", "e", "esc"] {
        assert_eq!(decision.action_for_key(key), None, "{key} still fired");
    }
}

/// Strict priority, then creation order: an approval outranks a question, which outranks a
/// plan-ready reminder, and the older of two approvals is the head.
#[test]
fn the_head_of_the_queue_is_the_highest_priority_and_then_the_oldest() {
    let pending = vec![plan(), question(false, 2), approval(true)];
    assert_eq!(
        Decision::head(&pending).map(|head| head.id.clone()),
        Some("gate-1".into())
    );

    let older = approval(true);
    let mut newer = approval(true);
    newer.id = "gate-1b".into();
    let pending = vec![older, newer];
    assert_eq!(
        Decision::head(&pending).map(|head| head.id.clone()),
        Some("gate-1".into())
    );
    assert!(Decision::head(&[]).is_none());
}

/// The status bar mirrors the drawer from this one source, so the two can never disagree.
#[test]
fn the_hints_and_the_keys_come_from_the_same_source() {
    let decision = approval(true);
    for (key, _) in decision.key_hints().pairs() {
        assert!(
            decision.action_for_key(&key).is_some(),
            "{key} is drawn but answers nothing"
        );
    }
}

#[test]
fn an_option_description_that_repeats_the_label_is_dropped() {
    let option = QuestionOption::new("pnpm").description("pnpm");
    assert_eq!(option.description, None);
    let option = QuestionOption::new("pnpm").description("fast, disk-efficient");
    assert_eq!(option.description.as_deref(), Some("fast, disk-efficient"));
}

#[test]
fn a_custom_answer_suppresses_the_checkmarks() {
    let mut set = QuestionSet::new(vec![
        DecisionQuestion::new("q", "?").options(vec![QuestionOption::new("a")]),
    ]);
    set.selected = vec![vec![0]];
    assert!(set.is_selected(0, 0));
    set.custom = true;
    assert!(!set.is_selected(0, 0));
}
