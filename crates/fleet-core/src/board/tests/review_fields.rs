//! Review-field serialisation and version-3 stamping tests (FEA-1).

use super::*;

fn run(board: &Board) -> CardRun {
    CardRun {
        id: DelegationId::new(),
        thread_id: None,
        status_id: "in-progress".parse().unwrap(),
        action: ActionKind::Prompt,
        provider: AgentKind::Claude,
        model: None,
        effort: None,
        started_at: board.created_at.clone(),
        ended_at: None,
        outcome: None,
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
        worktree_id: None,
    }
}

fn pull_request() -> PullRequestRef {
    PullRequestRef::parse("acme/api#12").unwrap()
}

/// Serialises, parses back and re-serialises a document, asserting nothing moved on the way.
fn stamp_after_round_trip(board: Board, cards: Vec<Card>) -> u32 {
    let version = document_version(&board, &cards);
    let document = BoardDocument {
        version,
        board,
        cards,
    };
    let text = serde_json::to_string(&document).unwrap();
    let back: BoardDocument = serde_json::from_str(&text).unwrap();
    assert_eq!(back, document);
    assert_eq!(serde_json::to_string(&back).unwrap(), text);
    document_version(&back.board, &back.cards)
}

#[test]
fn a_board_that_uses_no_review_field_keeps_its_version() {
    let board = board();
    let plain = card(&board);
    assert_eq!(
        stamp_after_round_trip(board.clone(), vec![plain.clone()]),
        1
    );

    let mut ran = plain;
    ran.runs = vec![run(&board)];
    assert_eq!(stamp_after_round_trip(board.clone(), vec![ran.clone()]), 2);

    let mut automated = board;
    automated.settings.max_live_runs = Some(2);
    assert_eq!(stamp_after_round_trip(automated, vec![ran]), 2);
}

#[test]
fn a_reviews_board_is_written_at_version_3() {
    let mut board = board();
    board.kind = BoardKind::Reviews;
    let cards = vec![card(&board)];

    assert_eq!(stamp_after_round_trip(board, cards), 3);
    assert_eq!(BOARD_DOCUMENT_VERSION, 3);
}

#[test]
fn a_card_worktree_board_is_written_at_version_3() {
    let mut board = board();
    board.settings.run_location = RunLocation::CardWorktree;
    let cards = vec![card(&board)];

    assert_eq!(stamp_after_round_trip(board, cards), BOARD_DOCUMENT_VERSION);
}

#[test]
fn a_card_with_a_pull_request_is_written_at_version_3() {
    let board = board();
    let mut reviewed = card(&board);
    reviewed.pull_request = Some(pull_request());

    assert_eq!(
        stamp_after_round_trip(board, vec![reviewed]),
        BOARD_DOCUMENT_VERSION
    );
}

#[test]
fn a_run_that_records_its_worktree_is_written_at_version_3() {
    let board = board();
    let mut ran = card(&board);
    let mut recorded = run(&board);
    recorded.worktree_id = Some(WorktreeId::try_from("acme/api#pr-12").unwrap());
    ran.runs = vec![recorded];

    assert_eq!(
        stamp_after_round_trip(board, vec![ran]),
        BOARD_DOCUMENT_VERSION
    );
}

#[test]
fn a_board_without_review_fields_serialises_byte_for_byte_as_before() {
    let board = board();
    let mut plain = card(&board);
    plain.runs = vec![run(&board)];

    let board_json = serde_json::to_value(&board).unwrap();
    assert!(board_json.get("kind").is_none());
    assert!(board_json["settings"].get("runLocation").is_none());

    let card_json = serde_json::to_value(&plain).unwrap();
    assert!(card_json.get("pullRequest").is_none());
    assert!(card_json["runs"][0].get("worktreeId").is_none());

    let draft_json = serde_json::to_value(CardDraft::default()).unwrap();
    assert!(draft_json.get("pullRequest").is_none());
}

#[test]
fn the_review_fields_serialise_under_their_contract_names() {
    let mut board = board();
    board.kind = BoardKind::Reviews;
    board.settings.run_location = RunLocation::CardWorktree;
    let mut reviewed = card(&board);
    reviewed.pull_request = Some(pull_request());

    let board_json = serde_json::to_value(&board).unwrap();
    assert_eq!(board_json["kind"], "reviews");
    assert_eq!(board_json["settings"]["runLocation"], "card_worktree");
    assert_eq!(
        serde_json::to_value(&reviewed).unwrap()["pullRequest"],
        serde_json::json!({
            "repo": "acme/api",
            "number": 12,
            "url": "https://github.com/acme/api/pull/12"
        })
    );
    round_trip(&board);
    round_trip(&reviewed);
}
