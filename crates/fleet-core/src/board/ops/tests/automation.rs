use super::*;
use crate::{
    agents::{AgentKind, DelegationId, ThreadId},
    board::defaults::{
        PRESET_EXPECT_IMPLEMENT, PRESET_EXPECT_REVIEW, PRESET_INSTRUCTIONS_IMPLEMENT,
        PRESET_REVIEW_SKILL, apply_workflow_preset, default_statuses, render_template,
        workflow_preset,
    },
    board::model::PendingRun,
};

/// A terminal run on `card`, ended at `ended_at`.
fn ended(card: &mut Card, outcome: RunOutcome, ended_at: &str) {
    card.runs.push(CardRun {
        id: DelegationId::new(),
        thread_id: Some(ThreadId::new()),
        status_id: card.status_id.clone(),
        action: ActionKind::Prompt,
        provider: AgentKind::Claude,
        model: None,
        effort: None,
        started_at: NOW.into(),
        ended_at: Some(ended_at.into()),
        outcome: Some(outcome),
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
        worktree_id: None,
    });
}

#[test]
fn blocks_is_the_reverse_of_blocked_by() {
    let mut board = board();
    let first = create(&mut board, &[], "a");
    let mut second = create(&mut board, std::slice::from_ref(&first), "b");
    let mut third = create(&mut board, &[first.clone(), second.clone()], "c");
    second.blocked_by = vec![first.id.clone()];
    third.blocked_by = vec![first.id.clone()];
    let cards = vec![first.clone(), second.clone(), third.clone()];

    assert_eq!(
        blocks(&cards, &first.id)
            .iter()
            .map(|card| card.id.clone())
            .collect::<Vec<_>>(),
        vec![second.id.clone(), third.id]
    );
    assert!(blocks(&cards, &second.id).is_empty());
}

#[test]
fn only_a_completed_blocker_satisfies_and_a_canceled_one_warns() {
    let mut board = board();
    let mut blocker = create(&mut board, &[], "a");
    let mut card = create(&mut board, std::slice::from_ref(&blocker), "b");
    card.blocked_by = vec![blocker.id.clone()];

    let cards = vec![blocker.clone(), card.clone()];
    assert!(!is_satisfied(&board, &cards, &blocker.id));
    assert_eq!(
        blocked(&board, &cards, &card),
        Some(Blocked {
            unsatisfied: 1,
            tone: BlockedTone::Muted,
        })
    );

    blocker.status_id = "canceled".parse().unwrap();
    let cards = vec![blocker.clone(), card.clone()];
    assert!(!is_satisfied(&board, &cards, &blocker.id));
    assert_eq!(
        blocked(&board, &cards, &card),
        Some(Blocked {
            unsatisfied: 1,
            tone: BlockedTone::Warning,
        })
    );

    blocker.status_id = "done".parse().unwrap();
    let cards = vec![blocker.clone(), card.clone()];
    assert!(is_satisfied(&board, &cards, &blocker.id));
    assert_eq!(blocked(&board, &cards, &card), None);
}

#[test]
fn an_archived_blocker_warns_even_from_an_open_column() {
    let mut board = board();
    let mut blocker = create(&mut board, &[], "a");
    let mut card = create(&mut board, std::slice::from_ref(&blocker), "b");
    card.blocked_by = vec![blocker.id.clone()];
    blocker.archived = true;
    let cards = vec![blocker, card.clone()];

    assert_eq!(
        blocked(&board, &cards, &card),
        Some(Blocked {
            unsatisfied: 1,
            tone: BlockedTone::Warning,
        })
    );
}

#[test]
fn a_card_with_no_links_is_never_blocked() {
    let mut board = board();
    let card = create(&mut board, &[], "a");

    assert_eq!(blocked(&board, std::slice::from_ref(&card), &card), None);
}

#[test]
fn a_failed_run_wants_a_human_until_one_moves_the_card_by_hand() {
    let mut board = board();
    let mut card = create(&mut board, &[], "a");
    ended(&mut card, RunOutcome::Failed, NOW);

    assert!(attention(&card, LATER));

    push_activity(&mut card, ActivityKind::AutoMoved, None, "Moved", LATER);
    assert!(attention(&card, LATER));

    push_activity(&mut card, ActivityKind::Moved, None, "Moved", LATER);
    assert!(!attention(&card, LATER));
}

#[test]
fn a_succeeded_run_never_wants_a_human() {
    let mut board = board();
    let mut card = create(&mut board, &[], "a");
    ended(&mut card, RunOutcome::Succeeded, NOW);

    assert!(!attention(&card, LATER));
}

#[test]
fn a_pending_run_wants_a_human_only_once_it_has_waited_a_minute() {
    let mut board = board();
    let mut card = create(&mut board, &[], "a");
    // Parked in its own column: a queued card never raises attention.
    card.status_id = "in-progress".parse().unwrap();
    card.pending_run = Some(PendingRun {
        status_id: "in-progress".parse().unwrap(),
        since: NOW.into(),
    });

    assert!(!attention(&card, "2026-09-06T12:00:59Z"));
    assert!(attention(&card, "2026-09-06T12:01:00Z"));
}

#[test]
fn the_latest_run_is_the_last_one_recorded() {
    let mut board = board();
    let mut card = create(&mut board, &[], "a");

    assert_eq!(latest_run(&card), None);

    ended(&mut card, RunOutcome::Failed, NOW);
    ended(&mut card, RunOutcome::Succeeded, LATER);

    assert_eq!(
        latest_run(&card).and_then(|run| run.outcome),
        Some(RunOutcome::Succeeded)
    );
}

#[test]
fn the_preset_pipeline_routes_forward_and_validates() {
    let mut board = board();
    board.statuses = workflow_preset();

    assert_eq!(
        board
            .statuses
            .iter()
            .map(|status| status.name.as_str())
            .collect::<Vec<_>>(),
        [
            "Backlog",
            "Todo",
            "Ready",
            "In Progress",
            "In review",
            "Done",
            "Canceled"
        ]
    );
    let ready = &board.statuses[2].automation.as_ref().unwrap();
    assert_eq!(
        ready
            .advance_when_unblocked
            .as_ref()
            .map(ToString::to_string),
        Some("in-progress".to_owned())
    );
    let implement = board.statuses[3].automation.as_ref().unwrap();
    let action = implement.on_enter.as_ref().unwrap();
    assert_eq!(action.kind, ActionKind::Prompt);
    assert_eq!(action.instructions, PRESET_INSTRUCTIONS_IMPLEMENT);
    assert_eq!(action.expect, PRESET_EXPECT_IMPLEMENT);
    assert_eq!(
        implement.on_success.as_ref().map(ToString::to_string),
        Some("in-review".to_owned())
    );
    let review = board.statuses[4].automation.as_ref().unwrap();
    assert_eq!(
        review.on_enter.as_ref().map(|action| action.kind.clone()),
        Some(ActionKind::Skill {
            name: PRESET_REVIEW_SKILL.into(),
            args: String::new(),
        })
    );
    assert_eq!(
        review.on_enter.as_ref().map(|action| action.expect.clone()),
        Some(PRESET_EXPECT_REVIEW.to_owned())
    );
    assert_eq!(board.settings.max_live_runs, None);
    assert_eq!(validate_board(&board), Ok(()));
}

#[test]
fn applying_the_preset_adds_only_the_missing_columns_and_keeps_the_shipped_names() {
    let mut board = board();
    let before = board.statuses.clone();

    assert!(apply_workflow_preset(&mut board));

    assert_eq!(
        board
            .statuses
            .iter()
            .map(|status| status.id.to_string())
            .collect::<Vec<_>>(),
        [
            "backlog",
            "todo",
            "ready",
            "in-progress",
            "in-review",
            "done",
            "canceled"
        ]
    );
    for existing in &before {
        assert_eq!(
            board
                .statuses
                .iter()
                .find(|status| status.id == existing.id),
            Some(existing),
            "{} was rewritten",
            existing.id
        );
    }
    assert_eq!(board.statuses[3].name, "In Progress");
}

#[test]
fn applying_the_preset_twice_changes_nothing_the_second_time() {
    let mut board = board();
    apply_workflow_preset(&mut board);
    let once = board.statuses.clone();

    assert!(!apply_workflow_preset(&mut board));

    assert_eq!(board.statuses, once);
}

#[test]
fn the_preset_never_rewrites_a_column_a_board_already_automated() {
    let mut board = board();
    board.statuses = default_statuses();
    board.statuses[2].name = "Doing".into();
    board.statuses[2].automation = Some(ColumnAutomation {
        on_success: Some("done".parse().unwrap()),
        ..ColumnAutomation::default()
    });
    let mine = board.statuses[2].clone();

    assert!(apply_workflow_preset(&mut board));

    assert_eq!(board.statuses[3], mine);
}

#[test]
fn a_template_substitutes_the_key_and_title_and_leaves_other_braces_alone() {
    assert_eq!(
        render_template("Do {key} — {title}. Keep {braces}.", "FLT-7", "Fix login"),
        "Do FLT-7 — Fix login. Keep {braces}."
    );
    assert_eq!(render_template("", "FLT-7", "Fix login"), "");
}
