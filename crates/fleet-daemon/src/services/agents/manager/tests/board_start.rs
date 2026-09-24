//! Board starts paused at the provider boundary, where launch reservations protect card state.

use super::*;
use crate::{
    adapters::files::RealFiles,
    services::{
        agents::delegation,
        boards::{Automation, Boards},
        checkpoints::Checkpoints,
    },
    stores::board::BoardStore,
};
use fleet_core::{
    board::{
        Action, ActionKind, BoardDocument, CardPatch, ColumnAgentPrefs, ColumnAutomation,
        PendingRun, document_version, new_worktree_board,
    },
    ids::{CardId, StatusId},
    paths::FleetHome,
};

async fn boards(harness: &Harness) -> Arc<Boards> {
    let home = FleetHome::new(&harness.home);
    let files = Arc::new(RealFiles::new(
        home.trash_dir(),
        [home.boards_dir(), home.repos_dir(), home.worktrees_dir()],
    ));
    let adapters = Adapters::system(files.clone());
    let (delegations, _worker) = delegation::install(
        &harness.manager,
        &harness.events,
        &harness.config,
        &harness.worktrees,
    )
    .expect("the harness agent database is available");
    let store = Arc::new(BoardStore::new(home, files));
    let boards = Arc::new(Boards::new(
        Arc::clone(&store),
        Arc::clone(&harness.state),
        adapters.board_backends.clone(),
        Arc::clone(&adapters.clock),
        Arc::new(JobManager::new(&harness.home)),
        Arc::new(harness.worktrees.clone()),
        harness.events.clone(),
        Some(Automation::new(
            delegations,
            Arc::new(Checkpoints::new(Arc::clone(&adapters.shell))),
        )),
    ));
    let state = harness.state.load().await.expect("the state loads");
    let context = state.contexts.first().expect("the harness context");
    let worktree = state
        .worktrees
        .iter()
        .find(|worktree| worktree.id == harness.worktree)
        .expect("the harness worktree");
    let mut board = new_worktree_board(context, worktree, "2026-09-24T00:00:00Z");
    for status in &mut board.statuses {
        if status.id.as_str() == "todo" {
            status.automation = Some(ColumnAutomation {
                on_enter: Some(Action {
                    kind: ActionKind::Prompt,
                    instructions: "Do {key}".into(),
                    expect: "the work is done".into(),
                    agent: ColumnAgentPrefs::default(),
                    env: Vec::new(),
                }),
                on_success: None,
                advance_when_unblocked: None,
            });
        }
    }
    let card = serde_json::from_value(serde_json::json!({
        "id": "card-1",
        "boardId": board.id,
        "number": 1,
        "title": "Launching",
        "statusId": "todo",
        "createdAt": "2026-09-24T00:00:00Z",
        "updatedAt": "2026-09-24T00:00:00Z"
    }))
    .expect("a card from its wire fields");
    let board_id = board.id.clone();
    store
        .save(&BoardDocument {
            version: document_version(&board, std::slice::from_ref(&card)),
            board,
            cards: vec![card],
        })
        .expect("the board saves");
    boards.get(&board_id).await.expect("the board is indexed");
    boards
}

#[track_caller]
fn card_id() -> CardId {
    "card-1".parse().expect("a static card id")
}

#[track_caller]
fn status(id: &str) -> StatusId {
    id.parse().expect("a static status id")
}

#[tokio::test]
async fn move_archive_and_delete_refuse_while_a_card_is_launching() {
    let harness = Harness::start(full()).await;
    let boards = boards(&harness).await;
    let gate = harness.script.block_next_send();
    let starting = {
        let boards = Arc::clone(&boards);
        tokio::spawn(async move { boards.start_run(&card_id()).await })
    };
    gate.reached.cancelled().await;

    let moving = boards
        .move_card(&card_id(), &status("done"), None, false)
        .await
        .expect_err("a launching card cannot move");
    let deleting = boards
        .delete_card(&card_id())
        .await
        .expect_err("a launching card cannot be deleted");
    let archiving = boards
        .update_card(
            &card_id(),
            CardPatch {
                archived: Some(true),
                ..CardPatch::default()
            },
        )
        .await
        .expect_err("a launching card cannot be archived");

    assert!(moving.to_string().contains("is starting"), "{moving}");
    assert!(deleting.to_string().contains("is starting"), "{deleting}");
    assert!(archiving.to_string().contains("is starting"), "{archiving}");
    gate.release.cancel();
    starting
        .await
        .expect("the start task does not panic")
        .expect("the run is recorded");
    boards
        .cancel_run(&card_id())
        .await
        .expect("the live run cancels");
}

#[tokio::test]
async fn cancelling_a_launching_card_stops_the_new_delegation_without_recording_it() {
    let harness = Harness::start(full()).await;
    let boards = boards(&harness).await;
    let home = FleetHome::new(&harness.home);
    let path = std::fs::read_dir(home.boards_dir())
        .expect("the boards directory reads")
        .next()
        .expect("the board document exists")
        .expect("the board entry reads")
        .path();
    let mut document: BoardDocument =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the board document reads"))
            .expect("the board document parses");
    document.cards[0].pending_run = Some(PendingRun {
        status_id: status("todo"),
        since: "2026-09-24T00:00:00Z".into(),
    });
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&document).expect("the queued board serializes"),
    )
    .expect("the queued board writes");
    let gate = harness.script.block_next_send();
    let starting = {
        let boards = Arc::clone(&boards);
        tokio::spawn(async move { boards.start_run(&card_id()).await })
    };
    gate.reached.cancelled().await;

    let card = card_id();
    let (cancelled, ()) = tokio::join!(boards.cancel_run(&card), async {
        tokio::task::yield_now().await;
        gate.release.cancel();
    });
    let cancelled = cancelled.expect("the launching run cancels");
    let started = starting
        .await
        .expect("the start task does not panic")
        .expect("the start answers after cancellation");

    assert!(cancelled.runs.is_empty());
    assert!(cancelled.pending_run.is_none());
    assert!(started.runs.is_empty());
    assert!(started.pending_run.is_none());
    assert_eq!(
        harness
            .script
            .calls()
            .into_iter()
            .filter(|call| matches!(call, FakeCall::Stop))
            .count(),
        1,
        "the new child is stopped exactly once"
    );
}

#[tokio::test]
async fn record_run_stops_a_delegation_when_persisted_card_facts_changed() {
    let harness = Harness::start(full()).await;
    let boards = boards(&harness).await;
    let gate = harness.script.block_next_send();
    let starting = {
        let boards = Arc::clone(&boards);
        tokio::spawn(async move { boards.start_run(&card_id()).await })
    };
    gate.reached.cancelled().await;
    let home = FleetHome::new(&harness.home);
    let path = std::fs::read_dir(home.boards_dir())
        .expect("the boards directory reads")
        .next()
        .expect("the board document exists")
        .expect("the board entry reads")
        .path();
    let mut document: BoardDocument =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the board document reads"))
            .expect("the board document parses");
    document.cards[0].archived = true;
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&document).expect("the changed board serializes"),
    )
    .expect("the changed board writes");

    gate.release.cancel();
    let started = starting
        .await
        .expect("the start task does not panic")
        .expect("the abandoned start answers with the card");

    assert!(started.runs.is_empty());
    assert_eq!(
        harness
            .script
            .calls()
            .into_iter()
            .filter(|call| matches!(call, FakeCall::Stop))
            .count(),
        1,
        "the mismatched start is stopped exactly once"
    );
}
