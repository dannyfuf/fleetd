use super::*;
use crate::{
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    services::Services,
    stores::config::ConfigStore,
};
use fleet_core::{model::Context, paths::FleetHome, state::default_state};

use super::worktree::ARCHIVED_CARD;

mod refusals;
mod triggers;

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
async fn worktree_board_materialization_waits_for_the_worktree_lifecycle_claim() {
    let (_temp, services, _receiver) = fixture().await;
    let worktree: WorktreeId = "acme/api#feature".parse().unwrap();
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

    let lifecycle = services.worktrees.claim_lifecycle(worktree.clone()).await;
    let boards = Arc::clone(&services.boards);
    let request_worktree = worktree.clone();
    let request = tokio::spawn(async move { boards.ensure_for_worktree(&request_worktree).await });
    tokio::task::yield_now().await;
    assert!(
        !request.is_finished(),
        "board creation cannot pass an in-flight delete or restore"
    );

    drop(lifecycle);
    let view = request.await.unwrap().unwrap();
    assert_eq!(view.board.worktree_id.as_ref(), Some(&worktree));
}

#[tokio::test]
async fn differently_based_boards_reserve_colliding_suffixes_through_save() {
    let (_temp, services, _receiver) = fixture().await;
    let first: WorktreeId = "acme/api#feature".parse().unwrap();
    let second: WorktreeId = "acme/api#feature-2".parse().unwrap();
    services
        .state
        .transaction({
            let first = first.clone();
            let second = second.clone();
            move |state| {
                state.repos.push(
                    serde_json::from_value(serde_json::json!({
                        "id": "acme/api", "owner": "acme", "name": "api",
                        "url": "https://example.invalid/acme/api.git", "contextId": "work",
                        "defaultBranch": "main", "path": "/tmp/acme-api", "clonedAt": "now"
                    }))
                    .unwrap(),
                );
                for (id, slug) in [(first, "feature"), (second, "feature-2")] {
                    state.worktrees.push(
                        serde_json::from_value(serde_json::json!({
                            "id": id, "repoId": "acme/api", "slug": slug,
                            "branch": slug, "baseRef": "main", "path": format!("/tmp/{slug}"),
                            "session": format!("api/{slug}"), "createdAt": "now"
                        }))
                        .unwrap(),
                    );
                }
                Ok(())
            }
        })
        .await
        .unwrap();
    let mut blocker = services
        .boards
        .ensure(&"work".parse().unwrap())
        .await
        .unwrap();
    blocker.board.id = "wt-acme-api-feature".parse().unwrap();
    services
        .boards
        .store
        .save(&BoardDocument {
            version: BOARD_DOCUMENT_VERSION,
            board: blocker.board,
            cards: Vec::new(),
        })
        .unwrap();

    let allocation = Arc::clone(&services.boards.allocation).lock_owned().await;
    let boards = Arc::clone(&services.boards);
    let first_request = tokio::spawn({
        let boards = Arc::clone(&boards);
        let first = first.clone();
        async move { boards.ensure_for_worktree(&first).await }
    });
    let second_request = tokio::spawn(async move { boards.ensure_for_worktree(&second).await });
    tokio::task::yield_now().await;
    assert!(!first_request.is_finished());
    assert!(!second_request.is_finished());

    drop(allocation);
    let (first_view, second_view) = tokio::join!(first_request, second_request);
    let first_view = first_view.unwrap().unwrap();
    let second_view = second_view.unwrap().unwrap();
    assert_ne!(first_view.board.id, second_view.board.id);
    assert_eq!(first_view.board.worktree_id.as_ref(), Some(&first));
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

/// Publishes `worktrees` as the inventory host `host` owns, the way an observed snapshot does.
fn mirror_worktrees(services: &Services, host: &str, worktrees: Vec<serde_json::Value>) {
    let snapshot = serde_json::from_value(serde_json::json!({
        "generatedAt": "now",
        "contexts": [], "repos": [], "clones": [], "worktrees": worktrees,
        "activeContext": null, "sessions": [], "statuses": [], "jobs": [],
        "daemon": {
            "version": "fleetd test", "pid": 1, "startedAt": "now", "home": "/tmp/remote"
        },
    }))
    .unwrap();
    services.mirror.apply(&host.parse().unwrap(), snapshot);
}

/// One worktree record as a remote host publishes it in its own snapshot.
fn remote_worktree(id: &WorktreeId) -> serde_json::Value {
    serde_json::json!({
        "id": id, "repoId": id.repo(), "slug": id.slug(), "branch": id.slug(),
        "baseRef": "main", "path": format!("/remote/{}", id.slug()),
        "session": format!("api/{}", id.slug()), "createdAt": "now"
    })
}

/// Registers `acme/api` in the `work` context, as a local clone of the mirrored repository.
async fn clone_repo_locally(services: &Services) {
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
}

/// Writes the document an older build left here for a worktree another host owns.
///
/// A board for a hosted worktree can no longer be created through this service, so the stale
/// document every retirement starts from is written straight to the store.
async fn stale_hosted_board(
    services: &Services,
    worktree: &WorktreeId,
    cards: usize,
) -> BoardDocument {
    let state = services.state.load().await.unwrap();
    let context = state
        .contexts
        .iter()
        .find(|context| context.id.as_str() == "work")
        .unwrap()
        .clone();
    let record: fleet_core::model::Worktree =
        serde_json::from_value(remote_worktree(worktree)).unwrap();
    let board = new_worktree_board(&context, &record, "now");
    let cards: Vec<Card> = (1..=cards)
        .map(|number| {
            serde_json::from_value(serde_json::json!({
                "id": format!("card-{number}"), "boardId": board.id, "number": number,
                "title": format!("Task {number}"), "statusId": "todo",
                "createdAt": "now", "updatedAt": "now"
            }))
            .unwrap()
        })
        .collect();
    // Stamped the way the store stamps it: a document nobody automated is written at 1, and the
    // caller compares this value against the file the store wrote.
    let doc = BoardDocument {
        version: fleet_core::board::document_version(&board, &cards),
        board,
        cards,
    };
    services.boards.store.save(&doc).unwrap();
    doc
}

/// `ctrl-s b` in a Workspace on a worktree another host owns is that host's board to serve.
///
/// The worktree is never in this daemon's state — the mirror is the only place it exists — and a
/// board scoped to it lives on the daemon that owns the worktree, so this service refuses to
/// invent a local one and the router sends the request on (`docs/BOARD.md` §4).
#[tokio::test]
async fn a_board_for_a_worktree_another_host_owns_is_not_found_here() {
    let (_temp, services, _receiver) = fixture().await;
    clone_repo_locally(&services).await;
    let worktree: WorktreeId = "acme/api#feature".parse().unwrap();
    mirror_worktrees(&services, "dev-box", vec![remote_worktree(&worktree)]);

    let error = services
        .boards
        .ensure_for_worktree(&worktree)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DaemonError::NotFound(message) if message == "worktree acme/api#feature"),
        "{error:?}"
    );

    // Explicit creation answers the same way: the repository being cloned here as well changes
    // nothing about who owns the worktree.
    let error = services
        .boards
        .create_for_worktree(&worktree, Some("Feature plan".into()), None, None)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DaemonError::NotFound(message) if message == "worktree acme/api#feature"),
        "{error:?}"
    );
    assert!(services.boards.store.list().unwrap().is_empty());
}

/// A local worktree board is unaffected: the daemon that publishes the worktree serves it.
#[tokio::test]
async fn a_board_for_a_worktree_this_daemon_published_is_still_created_locally() {
    let (_temp, services, _receiver) = fixture().await;
    clone_repo_locally(&services).await;
    let worktree: WorktreeId = "acme/api#local".parse().unwrap();
    let record = remote_worktree(&worktree);
    services
        .state
        .transaction(move |state| {
            state
                .worktrees
                .push(serde_json::from_value(record.clone()).unwrap());
            Ok(())
        })
        .await
        .unwrap();

    let view = services
        .boards
        .ensure_for_worktree(&worktree)
        .await
        .unwrap();

    assert_eq!(view.board.worktree_id.as_ref(), Some(&worktree));
    assert_eq!(view.board.context_id.as_str(), "work");
    assert_eq!(
        services
            .boards
            .summaries()
            .await
            .iter()
            .filter_map(|summary| summary.worktree_id.clone())
            .collect::<Vec<_>>(),
        vec![worktree]
    );
}

/// A document left behind for a hosted worktree is not published as one of this daemon's boards.
#[tokio::test]
async fn a_stale_document_for_a_worktree_another_host_owns_is_skipped_by_list_and_summaries() {
    let (_temp, services, _receiver) = fixture().await;
    clone_repo_locally(&services).await;
    let worktree: WorktreeId = "acme/api#feature".parse().unwrap();
    mirror_worktrees(&services, "dev-box", vec![remote_worktree(&worktree)]);
    let doc = stale_hosted_board(&services, &worktree, 1).await;

    assert!(services.boards.list(None).await.unwrap().is_empty());
    assert!(services.boards.summaries().await.is_empty());
    // The file is still readable by id, which is what retirement needs to report its cards.
    assert_eq!(
        services
            .boards
            .get(&doc.board.id)
            .await
            .unwrap()
            .cards
            .len(),
        1
    );
}

/// A card may link a worktree another host owns, and that link is not a stale one.
#[tokio::test]
async fn a_card_link_to_a_worktree_another_host_owns_survives_a_board_read() {
    let (_temp, services, _receiver) = fixture().await;
    clone_repo_locally(&services).await;
    let worktree: WorktreeId = "acme/api#feature".parse().unwrap();
    mirror_worktrees(&services, "dev-box", vec![remote_worktree(&worktree)]);
    let view = services
        .boards
        .ensure(&"work".parse().unwrap())
        .await
        .unwrap();
    let card: Card = serde_json::from_value(serde_json::json!({
        "id": "card-1", "boardId": view.board.id, "number": 1, "title": "Task",
        "statusId": "todo", "worktreeId": worktree, "repoId": "acme/api",
        "createdAt": "now", "updatedAt": "now"
    }))
    .unwrap();
    let doc = BoardDocument {
        version: BOARD_DOCUMENT_VERSION,
        board: view.board,
        cards: vec![card],
    };
    services.boards.store.save(&doc).unwrap();

    assert_eq!(
        services.boards.get(&doc.board.id).await.unwrap().cards[0]
            .worktree_id
            .as_ref(),
        Some(&worktree)
    );
    // Clearing the repository of a card that owns a live worktree is refused for a hosted
    // worktree exactly as for a local one: the link is the same field either way.
    assert!(matches!(
        services
            .boards
            .update_card(
                &"card-1".parse().unwrap(),
                CardPatch {
                    repo_id: Some(None),
                    ..CardPatch::default()
                },
            )
            .await,
        Err(DaemonError::Conflict(_))
    ));

    // Once the worktree leaves the mirror — deleted on its owning host — the link is stale and
    // the view drops it, as it drops a link to a deleted local worktree.
    mirror_worktrees(&services, "dev-box", Vec::new());
    assert!(
        services.boards.get(&doc.board.id).await.unwrap().cards[0]
            .worktree_id
            .is_none()
    );
}

/// Moving a repository rehomes the boards of the worktrees this daemon published, and no others.
#[tokio::test]
async fn moving_a_repository_leaves_a_hosted_worktrees_document_where_it_is() {
    let (_temp, services, _receiver) = fixture().await;
    clone_repo_locally(&services).await;
    services
        .state
        .transaction(|state| {
            state.contexts.push(Context {
                id: "next".parse().unwrap(),
                name: "Next".into(),
                owners: vec![],
                created_at: "now".into(),
            });
            Ok(())
        })
        .await
        .unwrap();
    let worktree: WorktreeId = "acme/api#feature".parse().unwrap();
    mirror_worktrees(&services, "dev-box", vec![remote_worktree(&worktree)]);
    let doc = stale_hosted_board(&services, &worktree, 0).await;

    services
        .dispatch(fleet_proto::request::RequestBody::MoveRepoToContext {
            repo: "acme/api".parse().unwrap(),
            context: "next".parse().unwrap(),
        })
        .await
        .unwrap();

    // Nothing rehomes a document this daemon does not publish: the owning host holds the board
    // whose context matters, and this one is only waiting to be retired.
    assert_eq!(
        services
            .boards
            .store
            .peek(&doc.board.id)
            .unwrap()
            .unwrap()
            .board
            .context_id
            .as_str(),
        "work"
    );
}

/// Retiring a board no document names is not an error and reports nothing.
#[tokio::test]
async fn retiring_the_board_of_a_hosted_worktree_reports_nothing_when_there_is_none() {
    let (_temp, services, _receiver) = fixture().await;
    clone_repo_locally(&services).await;
    let worktree: WorktreeId = "acme/api#feature".parse().unwrap();
    mirror_worktrees(&services, "dev-box", vec![remote_worktree(&worktree)]);

    assert_eq!(
        services
            .boards
            .retire_hosted_worktree_board(&worktree, &"dev-box".parse().unwrap())
            .await
            .unwrap(),
        None
    );
    assert!(services.boards.store.list().unwrap().is_empty());
}

/// The empty document an older build created here is moved to the trash, not deleted outright.
#[tokio::test]
async fn retiring_an_empty_board_for_a_hosted_worktree_trashes_its_document() {
    let (temp, services, _receiver) = fixture().await;
    clone_repo_locally(&services).await;
    let worktree: WorktreeId = "acme/api#feature".parse().unwrap();
    mirror_worktrees(&services, "dev-box", vec![remote_worktree(&worktree)]);
    let doc = stale_hosted_board(&services, &worktree, 0).await;
    let home = FleetHome::new(temp.path());

    let retired = services
        .boards
        .retire_hosted_worktree_board(&worktree, &"dev-box".parse().unwrap())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(retired.board, doc.board.id);
    assert_eq!(retired.cards, 0);
    assert!(retired.path.starts_with(home.trash_dir()));
    assert!(retired.path.exists());
    assert!(!home.board_path(&doc.board.id).exists());
    assert!(services.boards.store.list().unwrap().is_empty());
    assert!(services.boards.list(None).await.unwrap().is_empty());

    // Retirement is idempotent: the forwarded request that triggers it runs on every refresh.
    assert_eq!(
        services
            .boards
            .retire_hosted_worktree_board(&worktree, &"dev-box".parse().unwrap())
            .await
            .unwrap(),
        None
    );
}

/// Cards are never merged into the host's board, so the trashed file is the only copy left.
#[tokio::test]
async fn retiring_a_board_for_a_hosted_worktree_keeps_its_cards_in_the_trashed_document() {
    let (_temp, services, _receiver) = fixture().await;
    clone_repo_locally(&services).await;
    let worktree: WorktreeId = "acme/api#feature".parse().unwrap();
    mirror_worktrees(&services, "dev-box", vec![remote_worktree(&worktree)]);
    let doc = stale_hosted_board(&services, &worktree, 2).await;

    let retired = services
        .boards
        .retire_hosted_worktree_board(&worktree, &"dev-box".parse().unwrap())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(retired.cards, 2);
    let trashed: BoardDocument =
        serde_json::from_str(&std::fs::read_to_string(&retired.path).unwrap()).unwrap();
    assert_eq!(trashed, doc);
    assert_eq!(
        trashed
            .cards
            .iter()
            .map(|card| card.id.to_string())
            .collect::<Vec<_>>(),
        vec!["card-1".to_owned(), "card-2".to_owned()]
    );
    // The card index no longer routes those ids to a board that is gone.
    assert!(matches!(
        services
            .boards
            .update_card(
                &"card-1".parse().unwrap(),
                CardPatch {
                    title: Some("Renamed".into()),
                    ..CardPatch::default()
                },
            )
            .await,
        Err(DaemonError::NotFound(message)) if message == "card card-1"
    ));
}
