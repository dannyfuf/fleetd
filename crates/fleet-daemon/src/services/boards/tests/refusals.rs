//! The daemon's automation refusals — the ones needing neither a provider nor a delegation.
//!
//! Every test here drives `Boards` alone: the board document is the whole world, and the three
//! run verbs are asked only what a daemon without a native-agent database answers.

use super::*;
use crate::services::boards::automation::NO_DELEGATION_SERVICE;
use fleet_core::agents::{AgentKind, DelegationId};

const WORKTREE: &str = "acme/api#feature";
const NOW: &str = "2026-09-21T00:00:00Z";

/// Publishes the repo and the worktree a worktree board is scoped to.
async fn publish_worktree(services: &Services) -> WorktreeId {
    let worktree: WorktreeId = WORKTREE.parse().unwrap();
    services
        .state
        .transaction({
            let worktree = worktree.clone();
            move |state| {
                state.repos.push(
                    serde_json::from_value(serde_json::json!({
                        "id": "acme/api", "owner": "acme", "name": "api",
                        "url": "https://example.invalid/acme/api.git", "contextId": "work",
                        "defaultBranch": "main", "path": "/tmp/acme-api", "clonedAt": "now"
                    }))
                    .unwrap(),
                );
                state.worktrees.push(
                    serde_json::from_value(serde_json::json!({
                        "id": worktree, "repoId": "acme/api", "slug": "feature",
                        "branch": "feature", "baseRef": "main", "path": "/tmp/acme-api-feature",
                        "session": "api/feature", "createdAt": "now"
                    }))
                    .unwrap(),
                );
                Ok(())
            }
        })
        .await
        .unwrap();
    worktree
}

/// The worktree board this daemon serves, before anything has opted into automation.
async fn worktree_board(services: &Services) -> BoardView {
    let worktree = publish_worktree(services).await;
    services
        .boards
        .ensure_for_worktree(&worktree)
        .await
        .unwrap()
}

/// The same columns, with the first one running a prompt on entry.
fn with_action(statuses: &[Status]) -> Vec<Status> {
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
    statuses
}

fn columns(statuses: Vec<Status>) -> BoardPatch {
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
async fn automation_is_refused_on_a_board_no_worktree_owns() {
    let (_temp, services, _events) = fixture().await;
    let view = services
        .boards
        .ensure(&"work".parse().unwrap())
        .await
        .unwrap();
    let error = services
        .boards
        .update(&view.board.id, columns(with_action(&view.board.statuses)))
        .await
        .unwrap_err();
    assert_validation(
        &error,
        "invalid automation: automation is available on worktree boards only",
    );
}

#[tokio::test]
async fn automation_is_refused_on_a_board_a_remote_backend_owns() {
    let (_temp, services, _events) = fixture().await;
    let view = worktree_board(&services).await;
    // Straight onto the document: the refusal is about where this board's cards come from, not
    // about whether that system answers, and asking `acli` would only make the test need one.
    let mut doc = services.boards.load(&view.board.id).unwrap();
    doc.board.backend = BackendRef {
        kind: "jira".into(),
        settings: serde_json::json!({ "project": "SP" }),
    };
    services.boards.store.save(&doc).unwrap();
    let error = services
        .boards
        .update(&view.board.id, columns(with_action(&doc.board.statuses)))
        .await
        .unwrap_err();
    assert_validation(
        &error,
        "invalid automation: automation is available on local boards only",
    );
}

#[tokio::test]
async fn automation_is_refused_on_a_worktree_another_host_owns() {
    let (_temp, services, _events) = fixture().await;
    let view = worktree_board(&services).await;
    // The tree this board is scoped to is adopted by a host. Local state gives it up — only the
    // daemon that owns a worktree may publish it — and it comes back through the mirror, which
    // is where the owner's name now lives.
    services
        .state
        .transaction(|state| {
            state.worktrees.clear();
            Ok(())
        })
        .await
        .unwrap();
    let worktree: WorktreeId = WORKTREE.parse().unwrap();
    mirror_worktrees(&services, "dev-box", vec![remote_worktree(&worktree)]);

    let error = services
        .boards
        .update(&view.board.id, columns(with_action(&view.board.statuses)))
        .await
        .unwrap_err();
    assert_validation(
        &error,
        "invalid automation: automation is unavailable on a worktree owned by host dev-box",
    );
    // The board is still a board: only the patch that asks for automation is refused, so a tree
    // a host adopted afterwards can still be renamed — and can still give its automation up.
    let renamed = services
        .boards
        .update(
            &view.board.id,
            BoardPatch {
                name: Some("Adopted".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(renamed.board.name, "Adopted");
}

#[tokio::test]
async fn a_live_run_ceiling_outside_its_range_is_refused_with_the_shared_sentence() {
    let (_temp, services, _events) = fixture().await;
    let view = worktree_board(&services).await;
    let mut settings = view.board.settings.clone();
    settings.max_live_runs = Some(MAX_LIVE_RUNS_PER_BOARD + 1);
    let error = services
        .boards
        .update(
            &view.board.id,
            BoardPatch {
                settings: Some(settings.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_validation(&error, "invalid max_live_runs: must be between 1 and 8");
    settings.max_live_runs = Some(MAX_LIVE_RUNS_PER_BOARD);
    assert_eq!(
        services
            .boards
            .update(
                &view.board.id,
                BoardPatch {
                    settings: Some(settings),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .board
            .settings
            .max_live_runs(),
        MAX_LIVE_RUNS_PER_BOARD
    );
}

#[tokio::test]
async fn a_column_with_live_runs_cannot_be_removed_under_them() {
    let (_temp, services, _events) = fixture().await;
    let view = worktree_board(&services).await;
    let doing = view.board.statuses[0].id.clone();
    let elsewhere = view.board.statuses[1].id.clone();
    let card = services
        .boards
        .create_card(
            &view.board.id,
            CardDraft {
                title: "Ship it".into(),
                status_id: Some(elsewhere),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    // A run records the column it started in, and the card has since moved on: the run is what
    // keeps that column alive, not the cards standing in it.
    let mut doc = services.boards.load(&view.board.id).unwrap();
    doc.cards[0].runs.push(CardRun {
        id: DelegationId::new(),
        thread_id: None,
        status_id: doing.clone(),
        action: ActionKind::Prompt,
        provider: AgentKind::Claude,
        model: None,
        effort: None,
        started_at: NOW.into(),
        ended_at: None,
        outcome: None,
        detail: None,
        report_comment_id: None,
        files_changed: 0,
        cost_usd: None,
        tokens: None,
        worktree_id: None,
    });
    assert_eq!(doc.cards[0].id, card.id);
    services.boards.store.save(&doc).unwrap();

    let remaining: Vec<Status> = view
        .board
        .statuses
        .iter()
        .filter(|status| status.id != doing)
        .cloned()
        .collect();
    let error = services
        .boards
        .update(&view.board.id, columns(remaining))
        .await
        .unwrap_err();
    match &error {
        DaemonError::Conflict(message) => {
            assert_eq!(message, "column has 1 live runs; cancel them first");
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn a_column_that_stops_running_an_action_releases_the_cards_parked_for_it() {
    let (_temp, services, _events) = fixture().await;
    let view = worktree_board(&services).await;
    let parked = view.board.statuses[0].clone();
    services
        .boards
        .create_card(
            &view.board.id,
            CardDraft {
                title: "Waiting for a slot".into(),
                status_id: Some(parked.id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    services
        .boards
        .update(&view.board.id, columns(with_action(&view.board.statuses)))
        .await
        .unwrap();
    // The park a full board writes, put on directly: what is under test is the column losing
    // its action, not the ceiling that parked the card.
    let mut doc = services.boards.load(&view.board.id).unwrap();
    doc.cards[0].pending_run = Some(PendingRun {
        status_id: parked.id.clone(),
        since: NOW.into(),
    });
    services.boards.store.save(&doc).unwrap();

    let view = services
        .boards
        .update(&view.board.id, columns(view.board.statuses.clone()))
        .await
        .unwrap();
    assert_eq!(view.cards[0].pending_run, None);
    let last = view.cards[0].activity.last().unwrap();
    assert_eq!(last.kind, ActivityKind::Updated);
    assert_eq!(
        last.message,
        format!("Run canceled: {} no longer runs an action", parked.name)
    );
}

#[tokio::test]
async fn the_run_verbs_refuse_on_a_daemon_with_no_native_agent_database() {
    let (temp, services, _events) = fixture().await;
    let boards = Boards::new(
        Arc::new(crate::stores::board::BoardStore::new(
            FleetHome::new(temp.path()),
            Arc::clone(&services.adapters.files),
        )),
        Arc::clone(&services.state),
        services.adapters.board_backends.clone(),
        Arc::clone(&services.adapters.clock),
        Arc::clone(&services.jobs),
        Arc::new(services.worktrees.clone()),
        services.events.clone(),
        None,
    );
    assert!(boards.automation().is_none());
    let card = new_card_id().unwrap();
    let refusals = [
        boards.start_run(&card).await.unwrap_err(),
        boards.cancel_run(&card).await.unwrap_err(),
        boards.wait_run(&card, 0).await.unwrap_err(),
    ];
    for error in &refusals {
        match error {
            DaemonError::Unsupported(message) => assert_eq!(message, NO_DELEGATION_SERVICE),
            other => panic!("expected an unsupported refusal, got {other:?}"),
        }
    }
    assert_eq!(
        NO_DELEGATION_SERVICE,
        "the native-agent database is unavailable, so board automation is refused"
    );
}
