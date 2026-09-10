use super::*;
use crate::{
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    services::Services,
    stores::config::ConfigStore,
};
use fleet_core::{model::Context, paths::FleetHome, state::default_state};

use super::worktree::ARCHIVED_CARD;

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
#[tokio::test]
async fn ensure_is_idempotent_persists_and_emits_once() {
    let (_temp, services, mut receiver) = fixture().await;
    let id = "work".parse().unwrap();
    let (a, b) = tokio::join!(services.boards.ensure(&id), services.boards.ensure(&id));
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(
        receiver.recv().await.unwrap(),
        Event::BoardChanged {
            board_id: a.board.id.clone(),
            reason: BoardChangeReason::Created
        }
    );
    assert!(receiver.try_recv().is_err());
    assert_eq!(services.boards.get(&a.board.id).await.unwrap(), a);
    assert_eq!(services.snapshot().await.unwrap().boards.len(), 1);
    assert!(
        services
            .boards
            .ensure(&"missing".parse().unwrap())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn stale_worktree_links_are_only_cleared_in_views_and_orphans_are_hidden() {
    let (_temp, services, _receiver) = fixture().await;
    let view = services
        .boards
        .ensure(&"work".parse().unwrap())
        .await
        .unwrap();
    let card: Card = serde_json::from_value(serde_json::json!({ "id":"card-1", "boardId":view.board.id, "number":1, "title":"Task", "statusId":"todo", "worktreeId":"acme/api#missing", "createdAt":"now", "updatedAt":"now" })).unwrap();
    let doc = BoardDocument {
        version: BOARD_DOCUMENT_VERSION,
        board: view.board,
        cards: vec![card],
    };
    services.boards.store.save(&doc).unwrap();
    assert!(
        services.boards.get(&doc.board.id).await.unwrap().cards[0]
            .worktree_id
            .is_none()
    );
    assert!(
        services
            .boards
            .store
            .load(&doc.board.id)
            .unwrap()
            .unwrap()
            .cards[0]
            .worktree_id
            .is_some()
    );
    services
        .state
        .transaction(|state| {
            state.contexts.retain(|c| c.id.as_str() != "work");
            Ok(())
        })
        .await
        .unwrap();
    assert!(services.boards.list(None).await.unwrap().is_empty());
    assert!(services.boards.summaries().await.is_empty());
}
#[tokio::test]
async fn a_renamed_card_reuses_the_worktree_it_is_already_linked_to() {
    let (_temp, services, _receiver) = fixture().await;
    let view = services
        .boards
        .ensure(&"work".parse().unwrap())
        .await
        .unwrap();
    let repo_id: RepoId = "acme/api".parse().unwrap();
    let card = services
        .boards
        .create_card(
            &view.board.id,
            CardDraft {
                title: "Third".into(),
                ..CardDraft::default()
            },
        )
        .await
        .unwrap();
    let slug = worktree_slug(&view.board, &card);
    let worktree_id = format!("{repo_id}#{slug}");
    let seed = (worktree_id.clone(), slug.clone());
    services
        .state
        .transaction(move |state| {
            let (worktree_id, slug) = seed.clone();
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
                    "id": worktree_id, "repoId": "acme/api", "slug": slug,
                    "branch": slug, "baseRef": "main", "path": "/tmp/acme-api-wt",
                    "session": "s", "createdAt": "now"
                }))
                .unwrap(),
            );
            Ok(())
        })
        .await
        .unwrap();
    let (card, worktree, created) = services
        .boards
        .create_worktree_from_card(&card.id, Some(repo_id), None, None)
        .await
        .unwrap();
    // The worktree already existed, so this request adopted it rather than creating one.
    assert!(!created);
    assert_eq!(worktree.id.as_str(), worktree_id);
    assert_eq!(card.worktree_id.as_ref().unwrap().as_str(), worktree_id);
    services
        .boards
        .update_card(
            &card.id,
            CardPatch {
                title: Some("Third renamed".into()),
                ..CardPatch::default()
            },
        )
        .await
        .unwrap();
    // The title-derived slug moved, but the stored link is what identifies the worktree.
    let (card, worktree, created) = services
        .boards
        .create_worktree_from_card(&card.id, None, None, None)
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(worktree.id.as_str(), worktree_id);
    assert_eq!(card.worktree_id.as_ref().unwrap().as_str(), worktree_id);
    assert_eq!(services.state.load().await.unwrap().worktrees.len(), 1);
}

#[tokio::test]
async fn deleting_a_context_deletes_the_board_it_owned() {
    let (_temp, services, _receiver) = fixture().await;
    let context: ContextId = "work".parse().unwrap();
    let view = services.boards.ensure(&context).await.unwrap();
    services
        .boards
        .create_card(
            &view.board.id,
            CardDraft {
                title: "Secret".into(),
                ..CardDraft::default()
            },
        )
        .await
        .unwrap();
    services
        .dispatch(fleet_proto::request::RequestBody::DeleteContext {
            id: context.clone(),
        })
        .await
        .unwrap();
    // The document is gone, so a later context deriving the same id cannot adopt its cards.
    assert!(
        services
            .boards
            .store
            .load(&view.board.id)
            .unwrap()
            .is_none()
    );
    services
        .dispatch(fleet_proto::request::RequestBody::CreateContext {
            name: "Work".into(),
            owners: vec![],
        })
        .await
        .unwrap();
    assert!(
        services
            .boards
            .ensure(&context)
            .await
            .unwrap()
            .cards
            .is_empty()
    );
}

#[tokio::test]
async fn parents_must_exist_on_the_same_board_and_never_close_a_cycle() {
    let (_temp, services, _receiver) = fixture().await;
    let view = services
        .boards
        .ensure(&"work".parse().unwrap())
        .await
        .unwrap();
    let parent = services
        .boards
        .create_card(
            &view.board.id,
            CardDraft {
                title: "Parent".into(),
                ..CardDraft::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        services
            .boards
            .create_card(
                &view.board.id,
                CardDraft {
                    title: "Orphan".into(),
                    parent_id: Some("11111111-1111-4111-8111-111111111111".parse().unwrap()),
                    ..CardDraft::default()
                },
            )
            .await,
        Err(DaemonError::NotFound(_))
    ));
    let child = services
        .boards
        .create_card(
            &view.board.id,
            CardDraft {
                title: "Child".into(),
                parent_id: Some(parent.id.clone()),
                ..CardDraft::default()
            },
        )
        .await
        .unwrap();
    for (card, proposed) in [(&parent.id, &parent.id), (&parent.id, &child.id)] {
        assert!(matches!(
            services
                .boards
                .update_card(
                    card,
                    CardPatch {
                        parent_id: Some(Some(proposed.clone())),
                        ..CardPatch::default()
                    },
                )
                .await,
            Err(DaemonError::Validation(_))
        ));
    }
}

#[tokio::test]
async fn unregistered_repositories_never_reach_a_board_or_a_card() {
    let (_temp, services, _receiver) = fixture().await;
    let view = services
        .boards
        .ensure(&"work".parse().unwrap())
        .await
        .unwrap();
    let missing: RepoId = "missing/repo".parse().unwrap();
    assert!(
        services
            .boards
            .update(
                &view.board.id,
                BoardPatch {
                    default_repo_id: Some(Some(missing.clone())),
                    ..BoardPatch::default()
                },
            )
            .await
            .is_err()
    );
    assert!(
        services
            .boards
            .create_card(
                &view.board.id,
                CardDraft {
                    title: "Task".into(),
                    repo_id: Some(missing.clone()),
                    ..CardDraft::default()
                },
            )
            .await
            .is_err()
    );
    let card = services
        .boards
        .create_card(
            &view.board.id,
            CardDraft {
                title: "Task".into(),
                ..CardDraft::default()
            },
        )
        .await
        .unwrap();
    assert!(
        services
            .boards
            .update_card(
                &card.id,
                CardPatch {
                    repo_id: Some(Some(missing)),
                    ..CardPatch::default()
                },
            )
            .await
            .is_err()
    );
    // A refused reference is never persisted, so `card worktree` cannot inherit it.
    let after = services.boards.get(&view.board.id).await.unwrap();
    assert_eq!(after.board.default_repo_id, None);
    assert_eq!(after.cards.len(), 1);
    assert_eq!(after.cards[0].repo_id, None);
}

#[tokio::test]
async fn damaged_board_does_not_break_snapshot() {
    let (temp, services, _receiver) = fixture().await;
    let view = services
        .boards
        .ensure(&"work".parse().unwrap())
        .await
        .unwrap();
    std::fs::write(
        FleetHome::new(temp.path()).board_path(&view.board.id),
        "broken",
    )
    .unwrap();
    assert!(services.snapshot().await.unwrap().boards.is_empty());
}

#[tokio::test]
async fn archiving_a_card_during_the_clone_names_the_created_worktree() {
    let (_temp, services, _receiver) = fixture().await;
    let view = services
        .boards
        .ensure(&"work".parse().unwrap())
        .await
        .unwrap();
    let repo_id: RepoId = "acme/api".parse().unwrap();
    let card = services
        .boards
        .create_card(
            &view.board.id,
            CardDraft {
                title: "Archived mid-clone".into(),
                ..CardDraft::default()
            },
        )
        .await
        .unwrap();
    let slug = worktree_slug(&view.board, &card);
    let worktree_id = format!("{repo_id}#{slug}");
    services
        .state
        .transaction(|state| {
            state.repos.push(
                serde_json::from_value(serde_json::json!({
                    "id": "acme/api", "owner": "acme", "name": "api",
                    "url": "https://example.invalid/acme/api.git", "contextId": "work",
                    "defaultBranch": "main", "path": "/tmp/acme-api", "clonedAt": "now"
                }))
                .unwrap(),
            );
            Ok(())
        })
        .await
        .unwrap();
    let made: Worktree = serde_json::from_value(serde_json::json!({
        "id": worktree_id, "repoId": "acme/api", "slug": slug,
        "branch": slug, "baseRef": "main", "path": "/tmp/acme-api-wt",
        "session": "s", "createdAt": "now"
    }))
    .unwrap();
    let boards = services.boards.clone();
    let archived_card = card.id.clone();
    // The board guard is dropped for the clone, so the card can be archived while its
    // worktree is being made.
    let error = services
        .boards
        .create_worktree_from_card_routed(
            &card.id,
            Some(repo_id),
            None,
            None,
            move |_repo, _slug, _branch, _base, _host| async move {
                boards
                    .update_card(
                        &archived_card,
                        CardPatch {
                            archived: Some(true),
                            ..CardPatch::default()
                        },
                    )
                    .await?;
                Ok((true, made))
            },
        )
        .await
        .unwrap_err();

    // The refusal must name the orphan, exactly as the card-gone branch beside it does:
    // nothing on the board references the worktree, so `fleet list` is its only trace.
    let DaemonError::Conflict(message) = error else {
        panic!("archiving during the clone is a conflict: {error:?}");
    };
    assert!(
        message.contains(&worktree_id),
        "a refusal for a worktree that was created must name it: {message}"
    );
    // Refusing before anything is created keeps the plain wording.
    assert!(matches!(
        services
            .boards
            .create_worktree_from_card(&card.id, None, None, None)
            .await,
        Err(DaemonError::Conflict(message)) if message == ARCHIVED_CARD
    ));
}
