use super::*;
use crate::board::{PullRequestRef, validate_board};

const NOW: &str = "2026-09-22T12:00:00Z";

fn context(id: &str) -> Context {
    Context {
        id: ContextId::try_from(id).unwrap(),
        name: "Fleet".into(),
        owners: vec![],
        created_at: NOW.into(),
    }
}

fn card(pull_request: Option<PullRequestRef>) -> Card {
    let mut card: Card = serde_json::from_value(serde_json::json!({
        "id": "card-1", "boardId": "work", "number": 3, "title": "Fix login",
        "statusId": "pending", "createdAt": NOW, "updatedAt": NOW
    }))
    .unwrap();
    card.pull_request = pull_request;
    card
}

fn pull_request() -> PullRequestRef {
    PullRequestRef {
        repo: "acme/api".parse().unwrap(),
        number: 42,
        url: "https://github.com/acme/api/pull/42".into(),
    }
}

fn status<'a>(board: &'a Board, id: &str) -> &'a Status {
    board
        .statuses
        .iter()
        .find(|status| status.id.as_str() == id)
        .unwrap()
}

#[test]
fn the_reviews_preset_routes_forward_and_validates() {
    let board = new_reviews_board(&context("work"), NOW);
    validate_board(&board).unwrap();
    let ids: Vec<&str> = board.statuses.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        ["pending", "reviewing", "reviewed", "published", "dismissed"]
    );
    let names: Vec<&str> = board.statuses.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Pending review",
            "Reviewing",
            "Reviewed",
            "Review published",
            "Dismissed"
        ]
    );
    let categories: Vec<StatusCategory> = board.statuses.iter().map(|s| s.category).collect();
    assert_eq!(
        categories,
        [
            StatusCategory::Unstarted,
            StatusCategory::Started,
            StatusCategory::Started,
            StatusCategory::Completed,
            StatusCategory::Canceled,
        ]
    );

    let pending = status(&board, "pending").automation.as_ref().unwrap();
    assert!(pending.on_enter.is_none());
    assert!(pending.on_success.is_none());
    assert_eq!(
        pending
            .advance_when_unblocked
            .as_ref()
            .map(StatusId::as_str),
        Some("reviewing")
    );

    let reviewing = status(&board, "reviewing").automation.as_ref().unwrap();
    let action = reviewing.on_enter.as_ref().unwrap();
    assert_eq!(action.kind, ActionKind::Prompt);
    assert_eq!(action.instructions, PRESET_INSTRUCTIONS_REVIEW_PR);
    assert_eq!(action.expect, PRESET_EXPECT_REVIEW_PR);
    assert_eq!(action.agent, ColumnAgentPrefs::default());
    assert_eq!(
        reviewing.on_success.as_ref().map(StatusId::as_str),
        Some("reviewed")
    );
    assert!(reviewing.advance_when_unblocked.is_none());

    assert!(status(&board, "reviewed").automation.is_none());

    let published = status(&board, "published").automation.as_ref().unwrap();
    let action = published.on_enter.as_ref().unwrap();
    assert_eq!(action.kind, ActionKind::Prompt);
    assert_eq!(action.instructions, PRESET_INSTRUCTIONS_PUBLISH_REVIEW);
    assert_eq!(action.expect, PRESET_EXPECT_PUBLISH_REVIEW);
    assert_eq!(action.agent, ColumnAgentPrefs::default());
    assert!(published.on_success.is_none());
    assert!(published.advance_when_unblocked.is_none());

    assert!(status(&board, "dismissed").automation.is_none());
}

#[test]
fn a_reviews_board_id_is_derived_from_its_context() {
    assert_eq!(
        reviews_board_id(&ContextId::try_from("work").unwrap()).as_str(),
        "reviews-work"
    );
    // 70 characters: `reviews-` plus this is cut at 64 bytes, landing on a `-` that is trimmed.
    let long = format!("{}-{}", "a".repeat(55), "b".repeat(14));
    assert_eq!(long.len(), 70);
    let id = reviews_board_id(&ContextId::try_from(long.as_str()).unwrap());
    assert_eq!(id.as_str(), format!("reviews-{}", "a".repeat(55)));
    assert!(id.as_str().len() <= BOARD_ID_MAX_LEN);
    assert!(!id.as_str().ends_with('-'));
}

#[test]
fn a_new_reviews_board_runs_in_card_worktrees_two_at_a_time() {
    let board = new_reviews_board(&context("work"), NOW);
    assert_eq!(board.id.as_str(), "reviews-work");
    assert_eq!(board.context_id.as_str(), "work");
    assert_eq!(board.worktree_id, None);
    assert_eq!(board.name, "Reviews");
    assert_eq!(board.prefix, "REV");
    assert_eq!(board.kind, BoardKind::Reviews);
    assert_eq!(board.settings.run_location, RunLocation::CardWorktree);
    assert_eq!(board.settings.max_live_runs, Some(2));
    assert_eq!(board.settings.max_live_runs(), 2);
    assert!(!board.settings.start_on_worktree);
    let labels: Vec<(&str, &str)> = board
        .labels
        .iter()
        .map(|label| (label.id.as_str(), label.name.as_str()))
        .collect();
    assert_eq!(labels, [("github", "github"), ("chat", "chat")]);
    assert_ne!(board.labels[0].color, board.labels[1].color);
    assert!(board.labels.iter().all(|label| label.color.is_some()));
    assert_eq!(board.created_at, NOW);
    assert_eq!(board.updated_at, NOW);
}

#[test]
fn a_card_template_substitutes_the_pull_request() {
    let rendered = render_card_template(
        "{key} {title}: review {pr_url} ({pr_repo}#{pr_number})",
        "REV-3",
        &card(Some(pull_request())),
    );
    assert_eq!(
        rendered,
        "REV-3 Fix login: review https://github.com/acme/api/pull/42 (acme/api#42)"
    );
}

#[test]
fn pull_request_placeholders_are_left_alone_without_one() {
    let text = "{key} {title} {pr_url} {pr_repo} {pr_number} {other}";
    assert_eq!(
        render_card_template(text, "FLE-3", &card(None)),
        "FLE-3 Fix login {pr_url} {pr_repo} {pr_number} {other}"
    );
    assert_eq!(
        render_card_template(text, "FLE-3", &card(None)),
        render_template(text, "FLE-3", "Fix login")
    );
}
