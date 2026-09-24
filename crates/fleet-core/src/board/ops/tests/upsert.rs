//! Idempotent pull request card upsert tests (FEA-6).

use super::*;

const EARLIER: &str = "2026-09-06T11:00:00Z";
const EVEN_LATER: &str = "2026-09-06T14:00:00Z";

fn pull_request(number: u64) -> PullRequestRef {
    PullRequestRef {
        repo: "acme/api".parse().unwrap(),
        number,
        url: format!("https://github.com/acme/api/pull/{number}"),
    }
}

fn draft(number: u64) -> CardDraft {
    CardDraft {
        title: format!("Review acme/api#{number}"),
        pull_request: Some(pull_request(number)),
        ..CardDraft::default()
    }
}

fn upsert(
    board: &mut Board,
    cards: &mut Vec<Card>,
    id: &str,
    requested_at: Option<&str>,
    now: &str,
) -> Result<(UpsertOutcome, usize), BoardError> {
    upsert_pull_request_card(
        board,
        cards,
        id.parse().unwrap(),
        draft(7),
        requested_at,
        now,
    )
}

/// A board holding one card for `acme/api#7`, moved to `status` at `NOW`.
fn board_with_card_in(status: &str) -> (Board, Vec<Card>) {
    let mut board = board();
    let mut cards = Vec::new();
    upsert(&mut board, &mut cards, "a", None, NOW).unwrap();
    move_card(
        &board,
        &mut cards,
        &"a".parse().unwrap(),
        &status.parse().unwrap(),
        None,
        NOW,
    )
    .unwrap();
    (board, cards)
}

#[test]
fn upserting_a_new_pull_request_creates_a_card() {
    let mut board = board();
    let mut cards = vec![create(&mut board, &[], "x")];
    let (outcome, index) = upsert(&mut board, &mut cards, "a", Some(NOW), NOW).unwrap();
    assert_eq!((outcome, index), (UpsertOutcome::Created, 1));
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[1].pull_request, Some(pull_request(7)));
    assert_eq!(cards[1].status_id.as_str(), "todo");
    assert_eq!(cards[1].activity[0].kind, ActivityKind::Created);
}

#[test]
fn upserting_a_known_pull_request_returns_it_unchanged() {
    let mut board = board();
    let mut cards = Vec::new();
    upsert(&mut board, &mut cards, "a", None, NOW).unwrap();
    let (board_before, cards_before) = (board.clone(), cards.clone());
    let outcome = upsert(&mut board, &mut cards, "b", Some(LATER), LATER).unwrap();
    assert_eq!(outcome, (UpsertOutcome::Existing, 0));
    assert_eq!((board, cards), (board_before, cards_before));
}

#[test]
fn a_completed_card_reopens_when_the_request_is_newer() {
    let (mut board, mut cards) = board_with_card_in("done");
    let outcome = upsert(&mut board, &mut cards, "b", Some(LATER), EVEN_LATER).unwrap();
    assert_eq!(outcome, (UpsertOutcome::Reopened, 0));
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].status_id.as_str(), "todo");
    let last = cards[0].activity.last().unwrap();
    assert_eq!(
        (last.kind, last.message.as_str(), last.at.as_str()),
        (ActivityKind::Updated, "Review re-requested", EVEN_LATER)
    );
}

#[test]
fn a_completed_card_stays_when_the_request_is_older() {
    let (mut board, mut cards) = board_with_card_in("done");
    let before = cards.clone();
    let outcome = upsert(&mut board, &mut cards, "b", Some(EARLIER), LATER).unwrap();
    assert_eq!(outcome, (UpsertOutcome::Existing, 0));
    assert_eq!(cards, before);
}

#[test]
fn a_completed_card_stays_without_a_request_time() {
    let (mut board, mut cards) = board_with_card_in("done");
    let before = cards.clone();
    let outcome = upsert(&mut board, &mut cards, "b", None, LATER).unwrap();
    assert_eq!(outcome, (UpsertOutcome::Existing, 0));
    assert_eq!(cards, before);
}

#[test]
fn a_dismissed_card_is_never_reopened() {
    let (mut board, mut cards) = board_with_card_in("canceled");
    cards[0].archived = true;
    let before = cards.clone();
    let outcome = upsert(&mut board, &mut cards, "b", Some(EVEN_LATER), EVEN_LATER).unwrap();
    assert_eq!(outcome, (UpsertOutcome::Existing, 0));
    assert_eq!(cards, before);
}

#[test]
fn an_archived_card_reopens_unarchived() {
    let (mut board, mut cards) = board_with_card_in("done");
    cards[0].archived = true;
    let mut draft = draft(7);
    draft.status_id = Some("backlog".parse().unwrap());
    let outcome = upsert_pull_request_card(
        &mut board,
        &mut cards,
        "b".parse().unwrap(),
        draft,
        Some(LATER),
        LATER,
    )
    .unwrap();
    assert_eq!(outcome, (UpsertOutcome::Reopened, 0));
    assert!(!cards[0].archived);
    assert_eq!(cards[0].status_id.as_str(), "backlog");
}

#[test]
fn a_board_refuses_two_cards_for_one_pull_request() {
    let mut board = board();
    let mut cards = Vec::new();
    upsert(&mut board, &mut cards, "a", None, NOW).unwrap();
    assert_eq!(validate_pull_requests(&board, &cards), Ok(()));
    let mut twin = create_card(&mut board, &cards, "b".parse().unwrap(), draft(7), NOW).unwrap();
    // Archived cards still hold their pull request.
    twin.archived = true;
    cards.push(twin);
    assert_eq!(
        validate_pull_requests(&board, &cards),
        Err(BoardError::Invalid {
            field: "pull_request".into(),
            reason: "acme/api#7 is already on this board as FLE-1".into(),
        })
    );
}

#[test]
fn an_unparsable_request_time_is_refused() {
    let mut board = board();
    let mut cards = Vec::new();
    let refusal = || BoardError::Invalid {
        field: "requested_at".into(),
        reason: "must be an RFC 3339 time".into(),
    };
    assert_eq!(
        upsert(&mut board, &mut cards, "a", Some("yesterday"), NOW),
        Err(refusal())
    );
    assert!(cards.is_empty());
    let (mut board, mut cards) = board_with_card_in("done");
    assert_eq!(
        upsert(&mut board, &mut cards, "b", Some("2026-09-06 13:00"), LATER),
        Err(refusal())
    );
}

#[test]
fn upserting_a_draft_without_a_pull_request_is_refused() {
    let mut board = board();
    let mut cards = Vec::new();
    let refusal = upsert_pull_request_card(
        &mut board,
        &mut cards,
        "a".parse().unwrap(),
        CardDraft {
            title: "Fix login".into(),
            ..CardDraft::default()
        },
        None,
        NOW,
    );
    assert_eq!(
        refusal,
        Err(BoardError::Invalid {
            field: "pull_request".into(),
            reason: "a pull request card needs a pull request".into(),
        })
    );
}

#[test]
fn upserting_a_pull_request_with_a_mismatched_url_is_refused() {
    let mut board = board();
    let mut cards = Vec::new();
    let mut draft = draft(7);
    draft.pull_request.as_mut().unwrap().url = "https://github.com/acme/api/pull/8".into();

    let refusal = upsert_pull_request_card(
        &mut board,
        &mut cards,
        "a".parse().unwrap(),
        draft,
        None,
        NOW,
    );

    assert!(matches!(
        refusal,
        Err(BoardError::Invalid { field, .. }) if field == "pull_request"
    ));
    assert!(cards.is_empty());
}

#[test]
fn upserting_pull_request_number_zero_is_refused() {
    let mut board = board();
    let mut cards = Vec::new();

    let refusal = upsert_pull_request_card(
        &mut board,
        &mut cards,
        "a".parse().unwrap(),
        draft(0),
        None,
        NOW,
    );

    assert!(matches!(
        refusal,
        Err(BoardError::Invalid { field, .. }) if field == "pull_request"
    ));
    assert!(cards.is_empty());
}

/// GitHub names are case-insensitive: a link spelt `Acme/API` finds the card `acme/api` holds,
/// and a board never holds both.
#[test]
fn a_pull_request_spelt_in_another_case_is_the_same_one() {
    let mut board = board();
    let mut cards = Vec::new();
    upsert(&mut board, &mut cards, "a", None, NOW).unwrap();
    let mut shouted = draft(7);
    shouted.pull_request =
        Some(PullRequestRef::parse("https://github.com/Acme/API/pull/7").unwrap());
    let outcome = upsert_pull_request_card(
        &mut board,
        &mut cards,
        "b".parse().unwrap(),
        shouted.clone(),
        None,
        LATER,
    )
    .unwrap();
    assert_eq!(outcome, (UpsertOutcome::Existing, 0));
    assert_eq!(cards.len(), 1);

    let twin = create_card(&mut board, &cards, "c".parse().unwrap(), shouted, NOW).unwrap();
    cards.push(twin);
    assert_eq!(
        validate_pull_requests(&board, &cards),
        Err(BoardError::Invalid {
            field: "pull_request".into(),
            reason: "Acme/API#7 is already on this board as FLE-1".into(),
        })
    );
}
