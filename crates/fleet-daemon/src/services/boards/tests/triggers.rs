//! The card trigger sites: every path that can change a satisfaction re-evaluates the board.
//!
//! Nothing here needs a provider. A run is seeded the way a daemon that had already started one
//! would leave it — a live `CardRun` row on the document — because that row is exactly what the
//! refusals and the live index read; the delivery tests hand the hook the terminal delegation
//! that row names, which is the one thing the store cannot be asked for here.

use super::*;
use crate::services::agents::delegation::RunDeliveryHook;
use fleet_core::agents::{
    AgentKind, Delegation, DelegationCaller, DelegationStatus, DeliveryState,
};

/// A live delegation and its thread, as a seeded run row names them.
const LIVE_RUN: &str = "11111111-1111-4111-8111-111111111111";
const LIVE_THREAD: &str = "22222222-2222-4222-8222-222222222222";

/// A board whose `todo` column runs a prompt on everything that enters it.
///
/// `backlog` releases a card into `todo` once nothing blocks it, so one document exercises both
/// engine rules; every other column is inert, which is where a test parks a card it has moved.
async fn automated_board(services: &Services, max_live_runs: Option<u32>) -> Board {
    let view = services
        .boards
        .ensure(&"work".parse().unwrap())
        .await
        .unwrap();
    let mut board = view.board;
    board.settings.max_live_runs = max_live_runs;
    for status in &mut board.statuses {
        status.automation = match status.id.as_str() {
            "todo" => Some(ColumnAutomation {
                on_enter: Some(Action {
                    kind: ActionKind::Prompt,
                    instructions: "Do {key}".into(),
                    expect: "the tests pass".into(),
                    agent: ColumnAgentPrefs::default(),
                    env: Vec::new(),
                }),
                on_success: Some("done".parse().unwrap()),
                advance_when_unblocked: None,
            }),
            "backlog" => Some(ColumnAutomation {
                on_enter: None,
                on_success: None,
                advance_when_unblocked: Some("todo".parse().unwrap()),
            }),
            _ => None,
        };
    }
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

/// One card on disk, plus whatever run, link or pending marker the test needs on top.
fn seeded_card(board: &Board, number: u32, status: &str, extra: serde_json::Value) -> Card {
    let mut value = serde_json::json!({
        "id": format!("card-{number}"),
        "boardId": board.id,
        "number": number,
        "title": format!("Task {number}"),
        "statusId": status,
        "createdAt": "2026-09-21T00:00:00Z",
        "updatedAt": "2026-09-21T00:00:00Z",
    });
    if let Some(target) = value.as_object_mut()
        && let Some(extra) = extra.as_object()
    {
        for (key, item) in extra {
            target.insert(key.clone(), item.clone());
        }
    }
    serde_json::from_value(value).unwrap()
}

/// A run row that has not ended: what a card that is working carries.
fn live_run(status: &str) -> serde_json::Value {
    serde_json::json!({
        "runs": [{
            "id": LIVE_RUN,
            "threadId": LIVE_THREAD,
            "statusId": status,
            "action": { "kind": "prompt" },
            "provider": "claude",
            "startedAt": "2026-09-21T00:00:00Z",
        }]
    })
}

fn card_id(number: u32) -> CardId {
    format!("card-{number}").parse().unwrap()
}

fn status_id(id: &str) -> StatusId {
    id.parse().unwrap()
}

/// A card created straight into a running column starts that column's action.
///
/// The gesture is irrelevant: the column is what runs, so a create lands the same run a move
/// into the column would.
#[tokio::test]
async fn a_card_created_in_a_running_column_starts_its_action() {
    let (_temp, services, _receiver) = fixture().await;
    let board = automated_board(&services, None).await;

    let card = services
        .boards
        .create_card(
            &board.id,
            CardDraft {
                title: "First".into(),
                status_id: Some(status_id("todo")),
                ..CardDraft::default()
            },
        )
        .await
        .unwrap();

    let started: Vec<&Activity> = card
        .activity
        .iter()
        .filter(|entry| entry.kind == ActivityKind::RunStarted)
        .collect();
    assert_eq!(started.len(), 1, "{:?}", card.activity);
    assert_eq!(started[0].message, "Run started");
    assert!(card.pending_run.is_none());
}

/// An edit that cannot change a satisfaction evaluates nothing.
///
/// The card is sitting in the running column with no run of its own, so anything that seeded the
/// engine would start one — renaming a card must not.
#[tokio::test]
async fn an_edit_that_changes_neither_column_nor_archive_starts_nothing() {
    let (_temp, services, _receiver) = fixture().await;
    let board = automated_board(&services, None).await;
    write_document(
        &services,
        &board,
        vec![seeded_card(&board, 1, "todo", serde_json::json!({}))],
    );

    let card = services
        .boards
        .update_card(
            &card_id(1),
            CardPatch {
                title: Some("Renamed".into()),
                ..CardPatch::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(card.title, "Renamed");
    assert!(
        !card
            .activity
            .iter()
            .any(|entry| entry.kind == ActivityKind::RunStarted),
        "{:?}",
        card.activity
    );
}

/// A card moved into a running column starts it; a board already at its ceiling parks it.
#[tokio::test]
async fn a_move_into_a_running_column_starts_it_unless_the_board_is_full() {
    let (_temp, services, _receiver) = fixture().await;
    let board = automated_board(&services, None).await;
    write_document(
        &services,
        &board,
        vec![seeded_card(&board, 1, "backlog", serde_json::json!({}))],
    );

    let card = services
        .boards
        .move_card(&card_id(1), &status_id("todo"), None, false)
        .await
        .unwrap();
    assert!(
        card.activity
            .iter()
            .any(|entry| entry.kind == ActivityKind::RunStarted)
    );
    assert!(card.pending_run.is_none());

    // The same move on a board whose one slot is taken parks the card instead, and says nothing:
    // a card waiting for a slot has had nothing happen to it yet.
    let (_temp, services, _receiver) = fixture().await;
    let board = automated_board(&services, Some(1)).await;
    write_document(
        &services,
        &board,
        vec![
            seeded_card(&board, 1, "todo", live_run("todo")),
            seeded_card(&board, 2, "backlog", serde_json::json!({})),
        ],
    );

    let card = services
        .boards
        .move_card(&card_id(2), &status_id("todo"), None, false)
        .await
        .unwrap();
    assert_eq!(
        card.pending_run.as_ref().map(|pending| &pending.status_id),
        Some(&status_id("todo"))
    );
    assert!(
        !card
            .activity
            .iter()
            .any(|entry| entry.kind == ActivityKind::RunStarted)
    );
}

/// Moving a working card is refused, and the flag is the way past it.
#[tokio::test]
async fn moving_a_working_card_needs_the_cancel_flag() {
    let (_temp, services, _receiver) = fixture().await;
    let board = automated_board(&services, None).await;
    let card = seeded_card(&board, 1, "todo", live_run("todo"));
    let key = card.display_key(&board);
    write_document(&services, &board, vec![card]);

    let error = services
        .boards
        .move_card(&card_id(1), &status_id("in-progress"), None, false)
        .await
        .unwrap_err();
    match error {
        DaemonError::Conflict(message) => assert_eq!(
            message,
            format!("{key} is working; pass --cancel-run to move it")
        ),
        other => panic!("expected a conflict, got {other:?}"),
    }

    // With the flag the move stops refusing and cancels first. The cancellation itself belongs
    // to `Boards::cancel_run`, so all this asserts is that the refusal is gone — the run that is
    // really cancelled and then moved is the phase's end-to-end suite.
    let refused = services
        .boards
        .move_card(&card_id(1), &status_id("in-progress"), None, true)
        .await
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(
        !refused.contains("pass --cancel-run"),
        "the flag still refused: {refused}"
    );
}

/// A move clears a run the card was owed, and says nothing about it.
#[tokio::test]
async fn a_move_clears_a_pending_run_silently() {
    let (_temp, services, _receiver) = fixture().await;
    let board = automated_board(&services, None).await;
    write_document(
        &services,
        &board,
        vec![seeded_card(
            &board,
            1,
            "todo",
            serde_json::json!({
                "pendingRun": { "statusId": "todo", "since": "2026-09-21T00:00:00Z" }
            }),
        )],
    );

    let card = services
        .boards
        .move_card(&card_id(1), &status_id("in-progress"), None, false)
        .await
        .unwrap();

    assert!(card.pending_run.is_none());
    assert_eq!(
        card.activity.last().map(|entry| entry.kind),
        Some(ActivityKind::Moved),
        "{:?}",
        card.activity
    );
}

/// Archiving a working card is refused: the run would have no column left to report into.
#[tokio::test]
async fn archiving_a_working_card_is_refused() {
    let (_temp, services, _receiver) = fixture().await;
    let board = automated_board(&services, None).await;
    let card = seeded_card(&board, 1, "todo", live_run("todo"));
    let key = card.display_key(&board);
    write_document(&services, &board, vec![card]);

    let error = services
        .boards
        .update_card(
            &card_id(1),
            CardPatch {
                archived: Some(true),
                ..CardPatch::default()
            },
        )
        .await
        .unwrap_err();

    match error {
        DaemonError::Conflict(message) => {
            assert_eq!(message, format!("{key} is working; cancel the run first"));
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
}

/// Deleting a working card is refused with the same sentence archiving uses.
#[tokio::test]
async fn deleting_a_working_card_is_refused() {
    let (_temp, services, _receiver) = fixture().await;
    let board = automated_board(&services, None).await;
    let card = seeded_card(&board, 1, "todo", live_run("todo"));
    let key = card.display_key(&board);
    write_document(&services, &board, vec![card]);

    let error = services.boards.delete_card(&card_id(1)).await.unwrap_err();

    match error {
        DaemonError::Conflict(message) => {
            assert_eq!(message, format!("{key} is working; cancel the run first"));
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert!(services.boards.get(&board.id).await.unwrap().cards.len() == 1);
}

/// Deleting a blocker drops the link it held and tells every card it freed.
#[tokio::test]
async fn deleting_a_blocker_drops_its_links_in_the_same_write() {
    let (_temp, services, _receiver) = fixture().await;
    let board = automated_board(&services, None).await;
    let blocker = seeded_card(&board, 1, "in-progress", serde_json::json!({}));
    let key = blocker.display_key(&board);
    write_document(
        &services,
        &board,
        vec![
            blocker,
            seeded_card(
                &board,
                2,
                "backlog",
                serde_json::json!({ "blockedBy": ["card-1"] }),
            ),
        ],
    );

    services.boards.delete_card(&card_id(1)).await.unwrap();

    let view = services.boards.get(&board.id).await.unwrap();
    assert_eq!(view.cards.len(), 1);
    assert!(view.cards[0].blocked_by.is_empty());
    let activity = &view.cards[0].activity;
    assert!(
        activity
            .iter()
            .any(|entry| entry.message == format!("Unblocked: {key} was deleted")),
        "{activity:?}"
    );
    // Freed in a routing column, the card advances at once (§11.7 rule 0).
    assert!(
        activity
            .iter()
            .any(|entry| entry.kind == ActivityKind::AutoMoved
                && entry.message == "Moved to Todo: nothing blocks it"),
        "{activity:?}"
    );
}

/// A link to a card this board does not hold is refused before anything is written.
#[tokio::test]
async fn a_link_to_a_card_this_board_does_not_hold_is_refused() {
    let (_temp, services, _receiver) = fixture().await;
    let board = automated_board(&services, None).await;

    let error = services
        .boards
        .create_card(
            &board.id,
            CardDraft {
                title: "First".into(),
                blocked_by: vec![card_id(9)],
                ..CardDraft::default()
            },
        )
        .await
        .unwrap_err();

    assert!(
        error.to_string().contains("card-9 is not on this board"),
        "{error}"
    );
    assert!(
        services
            .boards
            .get(&board.id)
            .await
            .unwrap()
            .cards
            .is_empty()
    );
}

/// The same board in another context, for the one rule that is per board rather than per daemon.
async fn second_automated_board(services: &Services, max_live_runs: Option<u32>) -> Board {
    let mut state = services.boards.state_store.load().await.unwrap();
    state.contexts.push(Context {
        id: "other".parse().unwrap(),
        name: "Other".into(),
        owners: vec![],
        created_at: "now".into(),
    });
    services.boards.state_store.save(state).await.unwrap();
    let view = services
        .boards
        .ensure(&"other".parse().unwrap())
        .await
        .unwrap();
    let mut board = view.board;
    board.settings.max_live_runs = max_live_runs;
    for status in &mut board.statuses {
        status.automation = (status.id.as_str() == "todo").then(|| ColumnAutomation {
            on_enter: Some(Action {
                kind: ActionKind::Prompt,
                instructions: "Do {key}".into(),
                expect: "the tests pass".into(),
                agent: ColumnAgentPrefs::default(),
                env: Vec::new(),
            }),
            on_success: None,
            advance_when_unblocked: None,
        });
    }
    write_document(services, &board, Vec::new());
    board
}

/// One terminal delegation for a card, as the delivery hook is handed it.
fn delivered(board: &Board, status: DelegationStatus) -> Delegation {
    Delegation {
        id: LIVE_RUN.parse().unwrap(),
        caller: DelegationCaller::Card {
            board: board.id.clone(),
            card: card_id(1),
        },
        caller_turn: None,
        caller_item: None,
        child: LIVE_THREAD.parse().unwrap(),
        provider: AgentKind::Claude,
        depth: 1,
        brief: "Do card-1".into(),
        expectation: "the tests pass".into(),
        eager: false,
        status,
        status_payload: None,
        result: None,
        nudges: 0,
        recoveries: 0,
        delivery: DeliveryState::Pending,
        created: chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        finished: None,
        headline: None,
        usage: None,
    }
}

/// A run that ends where it started leaves the card there; it does not start the column again.
///
/// The card is still sitting in the column whose `on_enter` fired, with no live run and no
/// reservation, so rule 1 on its own would start a second run the moment the first one ended —
/// and a third when that one ended. `docs/BOARD.md` §11.7: nothing fires for a card merely
/// sitting in a column, and every terminal outcome but a routed success moves nothing.
#[tokio::test]
async fn a_terminal_run_does_not_start_the_column_it_ended_in() {
    for status in [
        DelegationStatus::Failed,
        DelegationStatus::Cancelled,
        DelegationStatus::Incomplete,
        // A success whose column routes nowhere is the same card standing in the same place.
        DelegationStatus::Succeeded,
    ] {
        let (_temp, services, _receiver) = fixture().await;
        let mut board = automated_board(&services, None).await;
        for column in &mut board.statuses {
            if column.id.as_str() == "todo"
                && let Some(automation) = column.automation.as_mut()
            {
                automation.on_success = None;
            }
        }
        write_document(
            &services,
            &board,
            vec![seeded_card(&board, 1, "todo", live_run("todo"))],
        );

        services
            .boards
            .on_run_delivered(&board.id, &card_id(1), &delivered(&board, status))
            .await
            .unwrap();

        let view = services.boards.get(&board.id).await.unwrap();
        let card = &view.cards[0];
        assert_eq!(card.runs.len(), 1, "{status:?} started another run");
        assert!(
            card.runs[0].outcome.is_some(),
            "the outcome was not written"
        );
        assert!(card.pending_run.is_none(), "{status:?} parked the card");
        assert!(
            !card
                .activity
                .iter()
                .any(|entry| entry.kind == ActivityKind::RunStarted),
            "{status:?}: {:?}",
            card.activity
        );
    }
}

/// The half of the delivery that must keep working: a success the column routes on still moves
/// the card, and the column it lands in still runs.
#[tokio::test]
async fn a_routed_success_moves_the_card_and_the_next_column_starts_it() {
    let (_temp, services, _receiver) = fixture().await;
    let mut board = automated_board(&services, None).await;
    // `todo` routes into `in-progress`, which runs an action of its own: the chain a workflow is.
    for column in &mut board.statuses {
        match column.id.as_str() {
            "todo" => {
                if let Some(automation) = column.automation.as_mut() {
                    automation.on_success = Some(status_id("in-progress"));
                }
            }
            "in-progress" => {
                column.automation = Some(ColumnAutomation {
                    on_enter: Some(Action {
                        kind: ActionKind::Prompt,
                        instructions: "Review {key}".into(),
                        expect: "the review lands".into(),
                        agent: ColumnAgentPrefs::default(),
                        env: Vec::new(),
                    }),
                    on_success: None,
                    advance_when_unblocked: None,
                });
            }
            _ => {}
        }
    }
    write_document(
        &services,
        &board,
        vec![seeded_card(&board, 1, "todo", live_run("todo"))],
    );

    services
        .boards
        .on_run_delivered(
            &board.id,
            &card_id(1),
            &delivered(&board, DelegationStatus::Succeeded),
        )
        .await
        .unwrap();

    let view = services.boards.get(&board.id).await.unwrap();
    let card = &view.cards[0];
    assert_eq!(card.status_id, status_id("in-progress"));
    assert_eq!(
        card.activity
            .iter()
            .filter(|entry| entry.kind == ActivityKind::RunStarted)
            .count(),
        1,
        "{:?}",
        card.activity
    );
}

/// One board's reservation is not spent against another board's ceiling.
///
/// The set is held for the whole round trip to the provider, which is seconds; counted daemon
/// wide, a card reserved on one board would park a card on a board with nothing running at all,
/// and only a delegation *somewhere* going terminal would ever release it.
#[tokio::test]
async fn a_reservation_on_one_board_leaves_another_boards_slot_free() {
    let (_temp, services, _receiver) = fixture().await;
    let first = automated_board(&services, Some(1)).await;
    write_document(
        &services,
        &first,
        vec![seeded_card(&first, 1, "todo", serde_json::json!({}))],
    );
    let second = second_automated_board(&services, Some(1)).await;
    write_document(
        &services,
        &second,
        vec![seeded_card(&second, 2, "todo", serde_json::json!({}))],
    );

    // The reservation the first board takes is still held: nothing has recorded its run.
    let mut first_doc = services.boards.load(&first.id).unwrap();
    let first_plan = services
        .boards
        .evaluate_with_reservation(&mut first_doc, &[card_id(1)], "2026-09-21T00:00:00Z")
        .await
        .unwrap();
    assert_eq!(first_plan.starts.len(), 1);

    let mut second_doc = services.boards.load(&second.id).unwrap();
    let second_plan = services
        .boards
        .evaluate_with_reservation(&mut second_doc, &[card_id(2)], "2026-09-21T00:00:00Z")
        .await
        .unwrap();

    assert_eq!(
        second_plan.starts.len(),
        1,
        "another board's reservation spent this board's only slot"
    );
    assert!(second_plan.queued.is_empty());
    assert!(second_doc.cards[0].pending_run.is_none());
}

/// `card cancel` on a card that is only *owed* a run drops the slot it was waiting for.
#[tokio::test]
async fn cancelling_a_waiting_card_drops_the_run_it_was_owed() {
    let (_temp, services, _receiver) = fixture().await;
    let board = automated_board(&services, Some(1)).await;
    write_document(
        &services,
        &board,
        vec![
            seeded_card(&board, 1, "todo", live_run("todo")),
            seeded_card(
                &board,
                2,
                "todo",
                serde_json::json!({
                    "pendingRun": { "statusId": "todo", "since": "2026-09-21T00:00:00Z" }
                }),
            ),
        ],
    );

    let card = services.boards.cancel_run(&card_id(2)).await.unwrap();

    assert!(card.pending_run.is_none());
    assert_eq!(
        card.activity.last().map(|entry| entry.message.as_str()),
        Some("Run canceled: it was still waiting for a slot"),
        "{:?}",
        card.activity
    );
    assert!(card.runs.is_empty(), "nothing had started to end");
}

/// A card with neither a live run nor an owed one still refuses in the daemon's own words.
#[tokio::test]
async fn cancelling_a_card_with_nothing_running_is_refused() {
    let (_temp, services, _receiver) = fixture().await;
    let board = automated_board(&services, None).await;
    let card = seeded_card(&board, 1, "backlog", serde_json::json!({}));
    let key = card.display_key(&board);
    write_document(&services, &board, vec![card]);

    let error = services.boards.cancel_run(&card_id(1)).await.unwrap_err();

    match error {
        DaemonError::NotFound(message) => assert_eq!(message, format!("{key} has no live run")),
        other => panic!("expected a not-found, got {other:?}"),
    }
}
