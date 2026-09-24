//! The Reviews scope: what entering it, a `BoardChanged` under it and an old daemon ask for.
//!
//! The recording bridge answers nothing, so each test asserts the requests a gesture handed
//! over and applies the view by hand where the daemon's answer would have landed.

use std::time::Instant;

use fleet_core::{
    board::{BoardKind, BoardView, new_board},
    ids::ContextId,
    model::Context,
};
use fleet_proto::{
    event::{BoardChangeReason, Event},
    request::RequestBody,
    response::BOARD_REVIEWS_CAPABILITY,
};
use gpui::{AppContext as _, Entity, TestAppContext};

use super::{enter_reviews_scope, refresh_after_daemon_change};
use crate::{
    bridge::{Bridge, RecordedRequests},
    state::{AppState, BoardScope, REVIEW_BOARDS_UNSUPPORTED},
};

fn context() -> Context {
    Context {
        id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
        name: "Fleet".into(),
        owners: Vec::new(),
        created_at: "2026-09-06T12:00:00Z".into(),
    }
}

/// The context's Reviews board, as `EnsureReviewsBoard` would answer it.
fn reviews_view(context: &Context) -> BoardView {
    let mut board = new_board(context, "2026-09-06T12:00:00Z");
    board.kind = BoardKind::Reviews;
    BoardView {
        board,
        cards: Vec::new(),
        live_runs: Vec::new(),
    }
}

/// The app with nothing loaded, on a daemon that serves Reviews boards when `reviews` is set.
fn setup(reviews: bool, cx: &mut TestAppContext) -> (Entity<AppState>, Bridge, RecordedRequests) {
    let state = cx.new(|_| {
        let mut state = AppState::new("/tmp/fleet-board-reviews-scope", Instant::now());
        if reviews {
            state
                .daemon_capabilities
                .insert(BOARD_REVIEWS_CAPABILITY.to_owned());
        }
        state
    });
    let (bridge, requests) = Bridge::recording();
    (state, bridge, requests)
}

/// The board requests among everything the gesture asked for; the once-per-connection
/// `ListBoardBackends` is not what these tests are about.
fn board_requests(requests: &RecordedRequests) -> Vec<RequestBody> {
    requests
        .take()
        .into_iter()
        .filter(|body| !matches!(body, RequestBody::ListBoardBackends {}))
        .collect()
}

fn ensure_reviews(context: &ContextId) -> RequestBody {
    RequestBody::EnsureReviewsBoard {
        context_id: context.clone(),
    }
}

#[gpui::test]
fn entering_the_reviews_scope_sends_ensure_reviews_board(cx: &mut TestAppContext) {
    let (state, bridge, requests) = setup(true, cx);
    let context = context();

    let entered = cx.update(|cx| enter_reviews_scope(context.id.clone(), &state, &bridge, cx));
    cx.run_until_parked();

    assert!(entered);
    assert_eq!(board_requests(&requests), [ensure_reviews(&context.id)]);
    assert_eq!(
        state.read_with(cx, |app, _| app.board.scope.clone()),
        Some(BoardScope::Reviews(context.id.clone()))
    );

    // The Review tab re-enters on every notify: the scope it already holds asks nothing more.
    cx.update(|cx| enter_reviews_scope(context.id.clone(), &state, &bridge, cx));
    cx.run_until_parked();
    assert_eq!(board_requests(&requests), []);
}

#[gpui::test]
fn a_board_changed_for_the_reviews_board_re_requests_it(cx: &mut TestAppContext) {
    let (state, bridge, requests) = setup(true, cx);
    let context = context();
    cx.update(|cx| enter_reviews_scope(context.id.clone(), &state, &bridge, cx));
    cx.run_until_parked();
    requests.take();

    let view = reviews_view(&context);
    let board_id = view.board.id.clone();
    state.update(cx, |app, _| {
        // The recording bridge never answers; this is the answer landing.
        app.board.loading = false;
        app.apply_board_view(view);
    });
    assert!(
        state.read_with(cx, |app, _| app.board().is_some()),
        "the Reviews scope admits the context's Reviews board"
    );

    state.update(cx, |app, _| {
        app.apply_daemon_event(
            Event::BoardChanged {
                board_id,
                reason: BoardChangeReason::CardChanged,
            },
            Instant::now(),
        );
    });
    cx.update(|cx| refresh_after_daemon_change(&state, &bridge, cx));
    cx.run_until_parked();

    assert_eq!(board_requests(&requests), [ensure_reviews(&context.id)]);
}

#[gpui::test]
fn a_daemon_without_review_boards_shows_the_sentence_and_sends_nothing(cx: &mut TestAppContext) {
    let (state, bridge, requests) = setup(false, cx);
    let context = context();

    let entered = cx.update(|cx| enter_reviews_scope(context.id.clone(), &state, &bridge, cx));
    cx.run_until_parked();

    assert!(!entered);
    assert_eq!(
        requests.take(),
        [],
        "a refused scope asks the daemon nothing"
    );
    state.read_with(cx, |app, _| {
        assert_eq!(
            app.toasts
                .last()
                .unwrap_or_else(|| panic!("no toast"))
                .toast
                .text
                .as_ref(),
            REVIEW_BOARDS_UNSUPPORTED,
            "the sentence carries its own remedy"
        );
        assert_ne!(
            app.board.scope,
            Some(BoardScope::Reviews(context.id.clone())),
            "a refused scope is not entered"
        );
    });
}

#[gpui::test]
fn the_context_scope_and_the_reviews_scope_never_take_each_others_board(cx: &mut TestAppContext) {
    let (state, bridge, _requests) = setup(true, cx);
    let context = context();
    let tasks = BoardView {
        board: new_board(&context, "2026-09-06T12:00:00Z"),
        cards: Vec::new(),
        live_runs: Vec::new(),
    };

    cx.update(|cx| enter_reviews_scope(context.id.clone(), &state, &bridge, cx));
    state.update(cx, |app, _| app.apply_board_view(tasks.clone()));
    assert!(
        state.read_with(cx, |app, _| app.board().is_none()),
        "the context's task board is not its Reviews board, though both name the context"
    );

    state.update(cx, |app, _| {
        app.board.scope = Some(BoardScope::Context(context.id.clone()));
        app.apply_board_view(reviews_view(&context));
    });
    assert!(
        state.read_with(cx, |app, _| app.board().is_none()),
        "and the Reviews board never lands in the Hub's slot"
    );
    state.update(cx, |app, _| app.apply_board_view(tasks));
    assert!(state.read_with(cx, |app, _| app.board().is_some()));
}

/// A lasting `ListSchedules` failure (an unreadable `schedules.json`) must not turn the
/// observation that re-enters the scope on every notify into a request loop.
#[gpui::test]
fn a_failed_schedules_load_is_not_asked_again_on_every_notify(cx: &mut TestAppContext) {
    let (state, bridge, requests) = setup(true, cx);
    let context = context();
    let view = reviews_view(&context);
    let board_id = view.board.id.clone();
    state.update(cx, |app, _| {
        app.daemon_capabilities
            .insert(fleet_proto::response::SCHEDULES_CAPABILITY.to_owned());
    });
    cx.update(|cx| enter_reviews_scope(context.id.clone(), &state, &bridge, cx));
    cx.run_until_parked();
    state.update(cx, |app, _| {
        app.board.loading = false;
        app.apply_board_view(view);
    });
    requests.take();

    let schedule_loads = |requests: &RecordedRequests| {
        requests
            .take()
            .into_iter()
            .filter(|body| matches!(body, RequestBody::ListSchedules { .. }))
            .count()
    };
    cx.update(|cx| enter_reviews_scope(context.id.clone(), &state, &bridge, cx));
    cx.run_until_parked();
    assert_eq!(
        schedule_loads(&requests),
        1,
        "the shown board is asked once"
    );

    // The daemon's answer is a failure, which notifies; the observation re-enters the scope.
    state.update(cx, |app, cx| {
        app.schedules
            .load_failed(&board_id, "schedules.json is unreadable".to_owned());
        cx.notify();
    });
    for _ in 0..3 {
        cx.update(|cx| enter_reviews_scope(context.id.clone(), &state, &bridge, cx));
        cx.run_until_parked();
    }
    assert_eq!(
        schedule_loads(&requests),
        0,
        "a failure is retried per activation, never per notify"
    );
}
