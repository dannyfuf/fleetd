//! Dispatch of the review-board and schedule requests (BOARD §4, §12.5): one request each, sent
//! through `Services::dispatch` exactly as a client's frame is, against a temporary home.

use std::sync::Arc;

use fleet_core::{
    board::{BoardKind, CardDraft, PullRequestRef, UpsertOutcome},
    ids::{BoardId, ContextId},
    model::Context,
    paths::FleetHome,
    schedule::{Cadence, ScheduleDraft, SchedulePatch},
    state::default_state,
};
use fleet_proto::{request::RequestBody, response::ResponseBody};

use crate::{
    DaemonError,
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    jobs::JobManager,
    server::broadcast::BroadcastBus,
    services::Services,
    stores::{config::ConfigStore, state::StateStore},
};

/// A composed daemon over a temporary home holding the `work` context and nothing else.
async fn fixture() -> (tempfile::TempDir, Services) {
    let temp = tempfile::tempdir().expect("temp home");
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
        id: context(),
        name: "Work".into(),
        owners: vec![],
        created_at: "now".into(),
    });
    state.save(initial).await.expect("seed state");
    let services = Services::build(
        temp.path().to_path_buf(),
        Arc::new(ConfigStore::new(temp.path(), files.clone())),
        state,
        Arc::new(JobManager::new(temp.path())),
        Adapters::system(files),
        BroadcastBus::default(),
    );
    (temp, services)
}

fn context() -> ContextId {
    "work".parse().expect("context id")
}

async fn reviews_board(services: &Services) -> BoardId {
    let answer = services
        .dispatch(RequestBody::EnsureReviewsBoard {
            context_id: context(),
        })
        .await
        .expect("ensure the reviews board");
    let ResponseBody::Board(view) = answer else {
        panic!("EnsureReviewsBoard answers a board, not {answer:?}");
    };
    assert_eq!(view.board.kind, BoardKind::Reviews);
    view.board.id
}

fn pull_request_draft(number: u64) -> CardDraft {
    CardDraft {
        title: format!("Review #{number}"),
        pull_request: Some(PullRequestRef {
            repo: "acme/api".parse().expect("repo id"),
            number,
            url: format!("https://github.com/acme/api/pull/{number}"),
        }),
        ..CardDraft::default()
    }
}

fn schedule_draft(board: &BoardId) -> ScheduleDraft {
    ScheduleDraft {
        board_id: board.clone(),
        name: "GitHub reviews".into(),
        prompt: "List my review requests.".into(),
        cadence: Cadence::Every { minutes: 15 },
        agent: None,
        // Disabled, so the loop — which these tests never start anyway — would leave it alone.
        enabled: Some(false),
        timeout_minutes: None,
    }
}

#[tokio::test]
async fn ensure_reviews_board_dispatches_to_one_board_per_context() {
    let (_temp, services) = fixture().await;
    let first = reviews_board(&services).await;
    let second = reviews_board(&services).await;
    assert_eq!(first, second);
}

#[tokio::test]
async fn upsert_pull_request_card_dispatches_and_answers_the_outcome() {
    let (_temp, services) = fixture().await;
    let board = reviews_board(&services).await;
    let mut outcomes = Vec::new();
    for _ in 0..2 {
        let answer = services
            .dispatch(RequestBody::UpsertPullRequestCard {
                board_id: board.clone(),
                draft: pull_request_draft(7),
                requested_at: None,
            })
            .await
            .expect("upsert");
        let ResponseBody::CardUpsert { card, outcome } = answer else {
            panic!("UpsertPullRequestCard answers CardUpsert, not {answer:?}");
        };
        assert_eq!(card.pull_request.map(|pull| pull.number), Some(7));
        outcomes.push(outcome);
    }
    assert_eq!(outcomes, [UpsertOutcome::Created, UpsertOutcome::Existing]);
}

#[tokio::test]
async fn every_schedule_request_dispatches_to_the_schedules_service() {
    let (_temp, services) = fixture().await;
    let board = reviews_board(&services).await;

    let ResponseBody::Schedule(created) = services
        .dispatch(RequestBody::CreateSchedule {
            draft: schedule_draft(&board),
        })
        .await
        .expect("create")
    else {
        panic!("CreateSchedule answers a schedule");
    };
    assert_eq!(created.board_id, board);

    let ResponseBody::Schedules(listed) = services
        .dispatch(RequestBody::ListSchedules {
            board_id: Some(board.clone()),
        })
        .await
        .expect("list")
    else {
        panic!("ListSchedules answers schedules");
    };
    assert_eq!(
        listed
            .iter()
            .map(|schedule| &schedule.id)
            .collect::<Vec<_>>(),
        [&created.id]
    );

    let ResponseBody::Schedule(updated) = services
        .dispatch(RequestBody::UpdateSchedule {
            id: created.id.clone(),
            patch: SchedulePatch {
                name: Some("Renamed".into()),
                ..SchedulePatch::default()
            },
        })
        .await
        .expect("update")
    else {
        panic!("UpdateSchedule answers a schedule");
    };
    assert_eq!(updated.name, "Renamed");

    let answer = services
        .dispatch(RequestBody::DeleteSchedule {
            id: created.id.clone(),
        })
        .await
        .expect("delete");
    assert!(matches!(answer, ResponseBody::Ack), "{answer:?}");

    let error = services
        .dispatch(RequestBody::RunScheduleNow { id: created.id })
        .await
        .expect_err("a deleted schedule cannot run");
    assert!(matches!(error, DaemonError::NotFound(_)), "{error:?}");
}

#[tokio::test]
async fn deleting_a_board_deletes_its_schedules_through_dispatch() {
    let (_temp, services) = fixture().await;
    let board = reviews_board(&services).await;
    services
        .dispatch(RequestBody::CreateSchedule {
            draft: schedule_draft(&board),
        })
        .await
        .expect("create");

    services
        .dispatch(RequestBody::DeleteBoard {
            board_id: board.clone(),
        })
        .await
        .expect("delete the board");

    let ResponseBody::Schedules(left) = services
        .dispatch(RequestBody::ListSchedules { board_id: None })
        .await
        .expect("list")
    else {
        panic!("ListSchedules answers schedules");
    };
    assert!(left.is_empty(), "{left:?}");
}
