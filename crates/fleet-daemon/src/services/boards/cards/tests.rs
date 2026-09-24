//! `upsert_pull_request_card`: one card per pull request, written under the board gate, and
//! seeded like any created card so the column it lands in decides what runs.
//!
//! The board is written straight to the store with the Reviews preset, so these tests depend on
//! neither `ensure_reviews` nor a provider: a start the evaluation decided shows as the
//! `RunStarted` entry it writes on the card, whatever the start itself then does.

use super::*;
use crate::{
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    services::Services,
    stores::config::ConfigStore,
};
use fleet_core::{model::Context, paths::FleetHome, state::default_state};

async fn fixture() -> (
    tempfile::TempDir,
    Services,
    tokio::sync::broadcast::Receiver<Event>,
) {
    let temp = tempfile::tempdir().unwrap();
    let home = FleetHome::new(temp.path());
    let files = Arc::new(RealFiles::new(
        home.trash_dir(),
        [home.boards_dir(), home.repos_dir(), home.worktrees_dir()],
    ));
    let state = Arc::new(StateStore::new(
        temp.path(),
        files.clone(),
        Arc::new(SystemClock),
    ));
    let mut initial = default_state();
    initial.contexts.push(Context {
        id: "work".parse().unwrap(),
        name: "Work".into(),
        owners: vec![],
        created_at: "now".into(),
    });
    state.save(initial).await.unwrap();
    let events = BroadcastBus::default();
    let receiver = events.subscribe();
    let services = Services::build(
        temp.path().to_path_buf(),
        Arc::new(ConfigStore::new(temp.path(), files.clone())),
        state,
        Arc::new(JobManager::new(temp.path())),
        Adapters::system(files),
        events,
    );
    (temp, services, receiver)
}

/// A Reviews board in the `work` context, with the preset columns and their automation.
async fn reviews_board(services: &Services) -> Board {
    let view = services
        .boards
        .ensure(&"work".parse().unwrap())
        .await
        .unwrap();
    let mut board = view.board;
    board.kind = BoardKind::Reviews;
    board.settings.run_location = RunLocation::CardWorktree;
    board.statuses = reviews_preset();
    write_document(services, &board, Vec::new());
    board
}

/// Writes a document straight to the store, the way a daemon that had already run leaves it.
fn write_document(services: &Services, board: &Board, cards: Vec<Card>) {
    let doc = BoardDocument {
        version: fleet_core::board::document_version(board, &cards),
        board: board.clone(),
        cards,
    };
    services.boards.store.save(&doc).unwrap();
}

fn stored(services: &Services, board: &Board) -> BoardDocument {
    services.boards.store.load(&board.id).unwrap().unwrap()
}

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

fn started(card: &Card) -> usize {
    card.activity
        .iter()
        .filter(|entry| entry.kind == ActivityKind::RunStarted)
        .count()
}

#[tokio::test]
async fn upserting_a_pull_request_card_twice_creates_one() {
    let (_temp, services, _receiver) = fixture().await;
    let board = reviews_board(&services).await;

    let (first, created) = services
        .boards
        .upsert_pull_request_card(&board.id, draft(7), None)
        .await
        .unwrap();
    let (second, existing) = services
        .boards
        .upsert_pull_request_card(&board.id, draft(7), None)
        .await
        .unwrap();

    assert_eq!(created, UpsertOutcome::Created);
    assert_eq!(existing, UpsertOutcome::Existing);
    assert_eq!(first.id, second.id);
    let doc = stored(&services, &board);
    assert_eq!(doc.cards.len(), 1);
    assert_eq!(doc.cards[0].pull_request, Some(pull_request(7)));
}

/// Rule 0 moves a card created in *Pending review* on to *Reviewing*, whose action starts.
#[tokio::test]
async fn a_card_upserted_into_pending_review_starts_reviewing() {
    let (_temp, services, _receiver) = fixture().await;
    let board = reviews_board(&services).await;

    let (card, outcome) = services
        .boards
        .upsert_pull_request_card(&board.id, draft(7), None)
        .await
        .unwrap();

    assert_eq!(outcome, UpsertOutcome::Created);
    assert_eq!(card.status_id.as_str(), "reviewing");
    assert_eq!(started(&card), 1, "{:?}", card.activity);
    assert!(card.pending_run.is_none());
}

/// A pull request the board already tracks changes nothing: no save and no event.
#[tokio::test]
async fn an_existing_upsert_writes_nothing() {
    let (_temp, services, mut receiver) = fixture().await;
    let board = reviews_board(&services).await;
    services
        .boards
        .upsert_pull_request_card(&board.id, draft(7), None)
        .await
        .unwrap();
    // The first upsert's start lands after it answered; the comparison starts from there.
    services.boards.background_starts_settled().await;
    let before = stored(&services, &board);
    while receiver.try_recv().is_ok() {}

    let (card, outcome) = services
        .boards
        .upsert_pull_request_card(&board.id, draft(7), None)
        .await
        .unwrap();

    assert_eq!(outcome, UpsertOutcome::Existing);
    let after = stored(&services, &board);
    assert_eq!(after.board.updated_at, before.board.updated_at);
    assert_eq!(after, before);
    assert_eq!(card, before.cards[0]);
    assert!(receiver.try_recv().is_err());
}

/// A review requested again after the card was published reopens it, and the column it lands
/// in starts again.
#[tokio::test]
async fn a_reopened_card_starts_again() {
    let (_temp, services, _receiver) = fixture().await;
    let board = reviews_board(&services).await;
    let mut published: Card = serde_json::from_value(serde_json::json!({
        "id": "card-1",
        "boardId": board.id,
        "number": 1,
        "title": "Review acme/api#7",
        "statusId": "published",
        "createdAt": "2026-09-21T00:00:00Z",
        "updatedAt": "2026-09-21T00:00:00Z",
    }))
    .unwrap();
    published.pull_request = Some(pull_request(7));
    let mut board = board;
    board.next_number = 2;
    write_document(&services, &board, vec![published]);

    let (card, outcome) = services
        .boards
        .upsert_pull_request_card(&board.id, draft(7), Some("2026-09-22T00:00:00Z".to_owned()))
        .await
        .unwrap();

    assert_eq!(outcome, UpsertOutcome::Reopened);
    assert_eq!(card.id.as_str(), "card-1");
    assert_eq!(card.status_id.as_str(), "reviewing");
    assert!(
        card.activity
            .iter()
            .any(|entry| entry.kind == ActivityKind::Updated
                && entry.message == "Review re-requested"),
        "{:?}",
        card.activity
    );
    assert_eq!(started(&card), 1, "{:?}", card.activity);
    let doc = stored(&services, &board);
    assert_eq!(doc.cards.len(), 1);
    assert_ne!(doc.board.updated_at, board.updated_at);
}

/// Two schedule runs upserting the same pull request at once create one card between them.
#[tokio::test]
async fn concurrent_upserts_create_one_card() {
    let (_temp, services, _receiver) = fixture().await;
    let board = reviews_board(&services).await;

    let (a, b) = tokio::join!(
        services
            .boards
            .upsert_pull_request_card(&board.id, draft(7), None),
        services
            .boards
            .upsert_pull_request_card(&board.id, draft(7), None),
    );
    let (a, b) = (a.unwrap(), b.unwrap());

    let created = [a.1, b.1]
        .iter()
        .filter(|outcome| **outcome == UpsertOutcome::Created)
        .count();
    assert_eq!(created, 1, "{:?} {:?}", a.1, b.1);
    assert_eq!(a.0.id, b.0.id);
    assert_eq!(stored(&services, &board).cards.len(), 1);
}

/// A Reviews board takes a pull request from any context, and links its repository only when
/// Fleet knows it.
#[tokio::test]
async fn an_upsert_links_the_repository_only_when_fleet_knows_it() {
    let (_temp, services, _receiver) = fixture().await;
    let board = reviews_board(&services).await;
    services
        .state
        .transaction(|state| {
            state.contexts.push(Context {
                id: "elsewhere".parse().unwrap(),
                name: "Elsewhere".into(),
                owners: vec![],
                created_at: "now".into(),
            });
            state.repos.push(
                serde_json::from_value(serde_json::json!({
                    "id": "acme/api", "owner": "acme", "name": "api",
                    "url": "https://example.invalid/acme/api.git", "contextId": "elsewhere",
                    "defaultBranch": "main", "path": "/tmp/acme-api", "clonedAt": "now"
                }))
                .unwrap(),
            );
            Ok(())
        })
        .await
        .unwrap();

    let (known, _) = services
        .boards
        .upsert_pull_request_card(&board.id, draft(7), None)
        .await
        .unwrap();
    let mut unknown = draft(8);
    let pull_request = PullRequestRef {
        repo: "other/lib".parse().unwrap(),
        number: 8,
        url: "https://github.com/other/lib/pull/8".into(),
    };
    unknown.pull_request = Some(pull_request);
    unknown.repo_id = Some("other/lib".parse().unwrap());
    let (unknown, outcome) = services
        .boards
        .upsert_pull_request_card(&board.id, unknown, None)
        .await
        .unwrap();

    assert_eq!(known.repo_id, Some("acme/api".parse().unwrap()));
    assert_eq!(outcome, UpsertOutcome::Created);
    assert_eq!(unknown.repo_id, None);
}

/// A Reviews card names its pull request's repository whatever context that repository is in,
/// and every later view keeps showing it.
#[tokio::test]
async fn a_cross_context_repository_survives_every_view() {
    let (_temp, services, _receiver) = fixture().await;
    let board = reviews_board(&services).await;
    services
        .state
        .transaction(|state| {
            state.contexts.push(Context {
                id: "elsewhere".parse().unwrap(),
                name: "Elsewhere".into(),
                owners: vec![],
                created_at: "now".into(),
            });
            state.repos.push(
                serde_json::from_value(serde_json::json!({
                    "id": "acme/api", "owner": "acme", "name": "api",
                    "url": "https://example.invalid/acme/api.git", "contextId": "elsewhere",
                    "defaultBranch": "main", "path": "/tmp/acme-api", "clonedAt": "now"
                }))
                .unwrap(),
            );
            Ok(())
        })
        .await
        .unwrap();
    let (card, _) = services
        .boards
        .upsert_pull_request_card(&board.id, draft(7), None)
        .await
        .unwrap();

    let view = services.boards.get(&board.id).await.unwrap();
    let commented = services
        .boards
        .add_comment(&card.id, "a note".into())
        .await
        .unwrap();

    let repo: RepoId = "acme/api".parse().unwrap();
    assert_eq!(view.cards[0].repo_id.as_ref(), Some(&repo));
    assert_eq!(commented.repo_id.as_ref(), Some(&repo));
}

/// On a card-worktree board the upsert answers once its save lands: the start, which may first
/// fetch the pull request into a new worktree, is recorded after the answer.
#[tokio::test]
async fn a_card_worktree_start_lands_after_the_upsert_answers() {
    let (_temp, services, _receiver) = fixture().await;
    let board = reviews_board(&services).await;

    let (card, outcome) = services
        .boards
        .upsert_pull_request_card(&board.id, draft(7), None)
        .await
        .unwrap();

    assert_eq!(outcome, UpsertOutcome::Created);
    assert_eq!(started(&card), 1, "{:?}", card.activity);
    assert!(card.runs.is_empty(), "{:?}", card.runs);
    services.boards.background_starts_settled().await;
    let doc = stored(&services, &board);
    // No Fleet repository holds acme/api here, so the start is recorded as the C5 refusal.
    assert_eq!(doc.cards[0].runs.len(), 1, "{:?}", doc.cards[0].runs);
    assert!(doc.cards[0].runs[0].failed_to_start());
}

/// A review requested again while the card's run is still going changes nothing: reopening it
/// would move the card out from under its run.
#[tokio::test]
async fn a_working_card_is_not_reopened_under_its_run() {
    let (_temp, services, mut receiver) = fixture().await;
    let board = reviews_board(&services).await;
    let mut publishing: Card = serde_json::from_value(serde_json::json!({
        "id": "card-1",
        "boardId": board.id,
        "number": 1,
        "title": "Review acme/api#7",
        "statusId": "published",
        "createdAt": "2026-09-21T00:00:00Z",
        "updatedAt": "2026-09-21T00:00:00Z",
        "runs": [{
            "id": "11111111-1111-4111-8111-111111111111",
            "statusId": "published",
            "action": {"kind": "prompt"},
            "provider": "claude",
            "startedAt": "2026-09-21T00:00:00Z"
        }],
    }))
    .unwrap();
    publishing.pull_request = Some(pull_request(7));
    let mut board = board;
    board.next_number = 2;
    write_document(&services, &board, vec![publishing]);
    let before = stored(&services, &board);
    while receiver.try_recv().is_ok() {}

    let (card, outcome) = services
        .boards
        .upsert_pull_request_card(&board.id, draft(7), Some("2026-09-22T00:00:00Z".to_owned()))
        .await
        .unwrap();

    assert_eq!(outcome, UpsertOutcome::Existing);
    assert_eq!(card.status_id.as_str(), "published");
    assert_eq!(stored(&services, &board), before);
    assert!(receiver.try_recv().is_err());
}

/// A reference spelt in another case finds the card, and links the repository under the id
/// Fleet registered it with.
#[tokio::test]
async fn a_pull_request_in_another_case_is_the_same_card() {
    let (_temp, services, _receiver) = fixture().await;
    let board = reviews_board(&services).await;
    services
        .boards
        .upsert_pull_request_card(&board.id, draft(7), None)
        .await
        .unwrap();
    let mut shouted = draft(7);
    shouted.pull_request =
        Some(PullRequestRef::parse("https://github.com/Acme/API/pull/7").unwrap());

    let (_card, outcome) = services
        .boards
        .upsert_pull_request_card(&board.id, shouted, None)
        .await
        .unwrap();

    assert_eq!(outcome, UpsertOutcome::Existing);
    assert_eq!(stored(&services, &board).cards.len(), 1);
}
