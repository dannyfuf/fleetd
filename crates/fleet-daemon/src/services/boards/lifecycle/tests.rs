//! A context's Reviews board, and where a board may automate.
//!
//! Every test drives `Boards` through a composed `Services` over a temporary home: the board
//! documents and the state file are the whole world, and nothing here starts a run.

use super::*;
use crate::{
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    services::Services,
    stores::config::ConfigStore,
};
use fleet_core::{paths::FleetHome, state::default_state};
use fleet_proto::request::RequestBody;

/// The one context every test's home starts with.
const CONTEXT: &str = "work";

/// A composed daemon over a temporary home holding the `work` context and nothing else.
///
/// Shared with the other board modules' tests that need a whole service rather than a pure
/// function (`automation::resume`).
pub(in crate::services::boards) async fn fixture() -> (tempfile::TempDir, Services) {
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
        id: CONTEXT.parse().unwrap(),
        name: "Work".into(),
        owners: vec![],
        created_at: "now".into(),
    });
    state.save(initial).await.unwrap();
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

/// Rewrites a stored board so it runs each card in the card's own worktree.
pub(in crate::services::boards) fn run_in_card_worktrees(services: &Services, id: &BoardId) {
    let mut doc = services.boards.load(id).unwrap();
    doc.board.settings.run_location = RunLocation::CardWorktree;
    services.boards.store.save(&doc).unwrap();
}

fn context() -> ContextId {
    CONTEXT.parse().unwrap()
}

/// The same columns, with the first one running a prompt on entry.
fn with_action(statuses: &[Status]) -> BoardPatch {
    let mut statuses = statuses.to_vec();
    statuses[0].automation = Some(ColumnAutomation {
        on_enter: Some(Action {
            kind: ActionKind::Prompt,
            instructions: "Do the card.".into(),
            expect: String::new(),
            agent: ColumnAgentPrefs::default(),
            env: Vec::new(),
        }),
        on_success: None,
        advance_when_unblocked: None,
    });
    BoardPatch {
        statuses: Some(statuses),
        ..Default::default()
    }
}

#[track_caller]
fn assert_validation(error: &DaemonError, expected: &str) {
    match error {
        DaemonError::Validation(message) => assert_eq!(message, expected),
        other => panic!("expected a validation refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn ensuring_a_reviews_board_twice_returns_one_board() {
    let (_temp, services) = fixture().await;
    let context = context();
    let (first, second) = tokio::join!(
        services.boards.ensure_reviews(&context),
        services.boards.ensure_reviews(&context),
    );
    let first = first.unwrap();
    assert_eq!(first, second.unwrap());
    assert_eq!(
        first,
        services.boards.ensure_reviews(&context).await.unwrap()
    );
    assert_eq!(first.board.id, reviews_board_id(&context));
    assert_eq!(services.boards.list(Some(&context)).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_reviews_board_uses_the_reviews_preset() {
    let (_temp, services) = fixture().await;
    let view = services.boards.ensure_reviews(&context()).await.unwrap();
    let board = &view.board;
    assert_eq!(board.kind, BoardKind::Reviews);
    assert_eq!(board.name, "Reviews");
    assert_eq!(board.prefix, "REV");
    assert_eq!(board.statuses, reviews_preset());
    assert_eq!(board.context_id, context());
    assert_eq!(board.worktree_id, None);
    assert_eq!(board.settings.run_location, RunLocation::CardWorktree);
    assert_eq!(board.settings.max_live_runs, Some(2));
    assert!(!board.settings.start_on_worktree);
    // What the store holds, not only what the call answered.
    assert_eq!(services.boards.load(&board.id).unwrap().board, *board);
}

#[tokio::test]
async fn a_reviews_board_is_never_the_context_board() {
    let (_temp, services) = fixture().await;
    let reviews = services.boards.ensure_reviews(&context()).await.unwrap();
    let tasks = services.boards.ensure(&context()).await.unwrap();
    assert_ne!(tasks.board.id, reviews.board.id);
    assert!(tasks.board.kind.is_tasks());
    assert_eq!(
        services.boards.context_board(&context()).unwrap(),
        Some(tasks.board.id.clone())
    );
    // Asking again resolves the same two boards, in either order.
    assert_eq!(services.boards.ensure(&context()).await.unwrap(), tasks);
    assert_eq!(
        services.boards.ensure_reviews(&context()).await.unwrap(),
        reviews
    );
    assert_eq!(
        services.boards.list(Some(&context())).await.unwrap().len(),
        2
    );
}

#[tokio::test]
async fn deleting_a_context_deletes_its_reviews_board() {
    let (_temp, services) = fixture().await;
    let tasks = services.boards.ensure(&context()).await.unwrap();
    let reviews = services.boards.ensure_reviews(&context()).await.unwrap();
    services
        .dispatch(RequestBody::DeleteContext { id: context() })
        .await
        .unwrap();
    for id in [&tasks.board.id, &reviews.board.id] {
        assert!(services.boards.store.load(id).unwrap().is_none(), "{id}");
    }
}

#[tokio::test]
async fn ensuring_a_reviews_board_for_an_unknown_context_is_not_found() {
    let (_temp, services) = fixture().await;
    let error = services
        .boards
        .ensure_reviews(&"missing".parse().unwrap())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DaemonError::NotFound(what) if what == "context missing"),
        "{error:?}"
    );
    assert!(services.boards.store.list().unwrap().is_empty());
}

#[tokio::test]
async fn a_context_board_that_runs_in_card_worktrees_accepts_an_action_column() {
    let (_temp, services) = fixture().await;
    let view = services.boards.ensure(&context()).await.unwrap();
    run_in_card_worktrees(&services, &view.board.id);
    let patched = services
        .boards
        .update(&view.board.id, with_action(&view.board.statuses))
        .await
        .unwrap();
    assert!(patched.board.statuses[0].automation.is_some());
}

#[tokio::test]
async fn a_context_board_that_runs_in_the_board_worktree_still_refuses() {
    let (_temp, services) = fixture().await;
    let view = services.boards.ensure(&context()).await.unwrap();
    let error = services
        .boards
        .update(&view.board.id, with_action(&view.board.statuses))
        .await
        .unwrap_err();
    assert_validation(
        &error,
        "invalid automation: automation is available on worktree boards only",
    );
}

#[tokio::test]
async fn a_jira_board_still_refuses_automation() {
    let (_temp, services) = fixture().await;
    let view = services.boards.ensure(&context()).await.unwrap();
    // Straight onto the document, as the worktree-board refusal test does: the refusal is about
    // where the cards come from, and asking `acli` would only make the test need one.
    let mut doc = services.boards.load(&view.board.id).unwrap();
    doc.board.settings.run_location = RunLocation::CardWorktree;
    doc.board.backend = BackendRef {
        kind: "jira".into(),
        settings: serde_json::json!({ "project": "SP" }),
    };
    services.boards.store.save(&doc).unwrap();
    let error = services
        .boards
        .update(&view.board.id, with_action(&doc.board.statuses))
        .await
        .unwrap_err();
    assert_validation(
        &error,
        "invalid automation: automation is available on local boards only",
    );
}

#[tokio::test]
async fn moving_an_automated_context_board_back_to_its_own_worktree_is_refused() {
    let (_temp, services) = fixture().await;
    let view = services.boards.ensure(&context()).await.unwrap();
    run_in_card_worktrees(&services, &view.board.id);
    let automated = services
        .boards
        .update(&view.board.id, with_action(&view.board.statuses))
        .await
        .unwrap();
    let mut settings = automated.board.settings.clone();
    settings.run_location = RunLocation::BoardWorktree;
    let error = services
        .boards
        .update(
            &view.board.id,
            BoardPatch {
                settings: Some(settings),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_validation(
        &error,
        "invalid automation: automation is available on worktree boards only",
    );
    // A patch that leaves automation and the run location alone still goes through.
    let renamed = services
        .boards
        .update(
            &view.board.id,
            BoardPatch {
                name: Some("Renamed".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(renamed.board.name, "Renamed");
}
