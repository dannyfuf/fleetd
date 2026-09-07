//! Board events and payloads cross the real socket into the app's state and view models.

mod common;

use std::time::{Duration, Instant};

use fleet_app::{
    state::AppState,
    views::{board_card_detail, board_screen},
};
use fleet_client::Client;
use fleet_core::board::{CardDraft, CardPatch, Priority};
use fleet_proto::event::{BoardChangeReason, Event};

#[tokio::test]
async fn board_mutations_refresh_the_app_through_default_subscriptions() {
    let daemon = common::Daemon::start("board-flow")
        .expect("build fleet-daemon before running the board socket integration test");
    let writer = daemon.connect().await;
    // A separate client retains Client::connect's default subscriptions, as the app does.
    let observer = Client::connect(daemon.home()).await.unwrap();
    let context = writer.create_context("Board test", vec![]).await.unwrap();
    writer
        .set_active_context(Some(context.id.clone()))
        .await
        .unwrap();
    let view = writer.ensure_board(context.id).await.unwrap();
    let board_id = view.board.id.clone();
    let mut app = AppState::new(daemon.home(), Instant::now());
    app.apply_snapshot(observer.get_snapshot().await.unwrap(), Instant::now());
    app.apply_board_view(view);
    assert_eq!(board_screen::counts(app.board().unwrap(), ""), (0, 0));
    let mut events = observer.events();

    let card = writer
        .create_card(
            board_id.clone(),
            CardDraft {
                title: "Integration card".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if matches!(&event, Event::BoardChanged { board_id: changed, reason: BoardChangeReason::CardChanged } if changed == &board_id) {
                break event;
            }
        }
    }).await.expect("BoardChanged must pass server filtering and default client subscriptions");
    app.apply_daemon_event(event, Instant::now());
    assert!(app.board_stale);
    app.apply_board_view(observer.get_board(board_id.clone()).await.unwrap());
    assert!(!app.board_stale);
    assert_eq!(
        board_screen::counts(app.board().unwrap(), "integration"),
        (1, 1)
    );

    let updated = writer
        .update_card(
            card.id.clone(),
            CardPatch {
                title: Some("Edited title".into()),
                description: Some("## Description\n\n**Ready** for review".into()),
                priority: Some(Priority::High),
                assignee: Some(Some("Ada".into())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    app.apply_card(updated.clone());
    assert_eq!(app.board().unwrap().cards, vec![updated.clone()]);
    assert!(
        !board_card_detail::property_rows(
            &app.board().unwrap().board,
            &app.board().unwrap().cards,
            &updated,
            0
        )
        .is_empty()
    );
    let moved = writer
        .move_card(card.id.clone(), "in-progress".parse().unwrap(), Some(0))
        .await
        .unwrap();
    app.apply_card(moved.clone());
    assert_eq!(
        board_screen::visible_cards(app.board().unwrap(), &moved.status_id, "Ada").len(),
        1
    );
    let commented = writer
        .add_card_comment(card.id.clone(), "Reviewed".into())
        .await
        .unwrap();
    assert_eq!(commented.comments[0].body, "Reviewed");
    assert!(!commented.dirty);
    let snapshot = observer.get_snapshot().await.unwrap();
    assert_eq!(snapshot.boards[0].card_count, 1);
    assert_eq!(snapshot.boards[0].open_count, 1);

    writer.delete_card(card.id).await.unwrap();
    app.apply_board_view(observer.get_board(board_id.clone()).await.unwrap());
    assert_eq!(board_screen::counts(app.board().unwrap(), ""), (0, 0));
    writer.delete_board(board_id.clone()).await.unwrap();
    assert!(observer.get_board(board_id).await.is_err());
    writer.daemon_shutdown(true).await.unwrap();
}
