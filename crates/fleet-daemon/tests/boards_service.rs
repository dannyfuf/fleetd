//! Board service persistence, worktree, synchronization, and event contracts.
use std::{sync::Arc, time::Duration};

use chrono::{TimeZone, Utc};
use fleet_core::{
    board::*,
    ids::BoardId,
    model::{Context, Repo, RepoHooks},
    paths::FleetHome,
    state::default_state,
};
use fleet_daemon::{
    DaemonError,
    adapters::{
        board::{BoardBackends, LocalBackend},
        files::RealFiles,
        shell::ShellResult,
    },
    jobs::JobManager,
    server::BroadcastBus,
    services::{boards::Boards, worktrees::Worktrees},
    stores::{board::BoardStore, config::ConfigStore, state::StateStore},
    testing::fakes::{FakeBackend, FakeBackendCall, FakeGit, FakeShell, FakeShellCall, FixedClock},
};
use fleet_proto::{
    event::{BoardChangeReason, Event},
    job::{JobKind, JobStatus},
};
use tokio::sync::broadcast;

struct Fixture {
    _temp: tempfile::TempDir,
    home: FleetHome,
    store: Arc<BoardStore>,
    state: Arc<StateStore>,
    jobs: Arc<JobManager>,
    clock: Arc<FixedClock>,
    worktrees: Arc<Worktrees>,
    backend: Arc<FakeBackend>,
    shell: Arc<FakeShell>,
    events: BroadcastBus,
    receiver: broadcast::Receiver<Event>,
    boards: Boards,
}

impl Fixture {
    async fn new(caps: BackendCapabilities) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = FleetHome::new(temp.path().join("fleet"));
        let files = Arc::new(RealFiles::new(
            home.trash_dir(),
            [home.boards_dir(), home.repos_dir(), home.worktrees_dir()],
        ));
        let clock = Arc::new(FixedClock::new(
            Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).single().unwrap(),
        ));
        let state = Arc::new(StateStore::new(
            temp.path().join("fleet"),
            files.clone(),
            clock.clone(),
        ));
        let mut initial = default_state();
        initial.contexts.push(Context {
            id: "work".parse().unwrap(),
            name: "Work".into(),
            owners: vec!["acme".into()],
            created_at: "2026-09-06T12:00:00Z".into(),
        });
        let repo_path = home.repos_dir().join("acme/api");
        std::fs::create_dir_all(repo_path.join(".git")).unwrap();
        std::fs::write(repo_path.join("README.md"), "fixture\n").unwrap();
        initial.repos.push(Repo {
            id: "acme/api".parse().unwrap(),
            owner: "acme".into(),
            name: "api".into(),
            url: "unused".into(),
            context_id: "work".parse().unwrap(),
            default_branch: "main".into(),
            path: repo_path.to_string_lossy().into_owned(),
            cloned_at: "2026-09-06T12:00:00Z".into(),
            hooks: RepoHooks::default(),
        });
        state.save(initial).await.unwrap();
        let config = Arc::new(ConfigStore::new(temp.path().join("fleet"), files.clone()));
        config.update(serde_json::json!({"reposDir": home.repos_dir(), "worktreesDir": home.worktrees_dir(), "hotPoolSize": 0, "hotRefreshIntervalMs": 0})).await.unwrap();
        let jobs = Arc::new(JobManager::with_clock(
            temp.path().join("fleet"),
            clock.clone(),
        ));
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| {
                command.program == "git"
                    && command.args.first().is_some_and(|arg| arg == "show-ref")
                    && command
                        .args
                        .last()
                        .is_some_and(|arg| arg != "refs/remotes/origin/main")
            },
            ShellResult {
                status: 1,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        shell.when(
            |command| command.program == "git",
            ShellResult {
                status: 0,
                stdout: "fixture-sha\n".into(),
                stderr: String::new(),
            },
        );
        let worktrees = Arc::new(
            Worktrees::new(
                config,
                state.clone(),
                jobs.clone(),
                files.clone(),
                Arc::new(FakeGit::new(shell.clone())),
            )
            .with_shell(shell.clone()),
        );
        let store = Arc::new(BoardStore::new(home.clone(), files));
        let backend = Arc::new(FakeBackend::new("fake", caps));
        let events = BroadcastBus::default();
        let receiver = events.subscribe();
        let boards = Boards::new(
            store.clone(),
            state.clone(),
            BoardBackends::new(vec![Arc::new(LocalBackend), backend.clone()]),
            clock.clone(),
            jobs.clone(),
            worktrees.clone(),
            events.clone(),
        );
        Self {
            _temp: temp,
            home,
            store,
            state,
            jobs,
            clock,
            worktrees,
            backend,
            shell,
            events,
            receiver,
            boards,
        }
    }

    fn reopened(&self) -> Boards {
        Boards::new(
            self.store.clone(),
            self.state.clone(),
            BoardBackends::new(vec![Arc::new(LocalBackend), self.backend.clone()]),
            self.clock.clone(),
            self.jobs.clone(),
            self.worktrees.clone(),
            self.events.clone(),
        )
    }

    async fn local(&self) -> BoardView {
        self.boards.ensure(&"work".parse().unwrap()).await.unwrap()
    }

    async fn remote(&self) -> BoardView {
        self.boards
            .create(
                &"work".parse().unwrap(),
                None,
                None,
                Some(BackendRef {
                    kind: "fake".into(),
                    settings: serde_json::json!({"project":"TEST"}),
                }),
            )
            .await
            .unwrap()
    }

    async fn card(&self, board: &BoardId, title: &str) -> Card {
        self.boards
            .create_card(
                board,
                CardDraft {
                    title: title.into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
    }

    fn pull(&self, cards: Vec<RemoteCard>, cursor: Option<&str>) {
        self.backend
            .describe_responses
            .lock()
            .unwrap()
            .push_back(Ok(schema()));
        self.backend
            .pull_responses
            .lock()
            .unwrap()
            .push_back(Ok(PullResult {
                cards,
                cursor: cursor.map(str::to_owned),
                ..Default::default()
            }));
    }

    async fn sync(&self, board: &BoardId) -> fleet_proto::job::JobRecord {
        let id = self.boards.sync(board, false).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), self.jobs.wait(&id))
            .await
            .unwrap()
            .unwrap()
    }

    fn reasons(&mut self) -> Vec<BoardChangeReason> {
        let mut reasons = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            if let Event::BoardChanged { reason, .. } = event {
                reasons.push(reason);
            }
        }
        reasons
    }
}

fn schema() -> BackendSchema {
    BackendSchema {
        statuses: vec![
            RemoteStatus {
                id: "open-id".into(),
                name: "Open".into(),
                category: Some(StatusCategory::Unstarted),
            },
            RemoteStatus {
                id: "doing-id".into(),
                name: "Doing".into(),
                category: Some(StatusCategory::Started),
            },
        ],
        ..Default::default()
    }
}

fn remote(key: &str, title: &str, version: &str) -> RemoteCard {
    RemoteCard {
        key: key.into(),
        title: title.into(),
        version: Some(version.into()),
        status: schema().statuses[0].clone(),
        ..Default::default()
    }
}

fn pull_caps() -> BackendCapabilities {
    BackendCapabilities {
        pull: true,
        incremental: true,
        ..Default::default()
    }
}

fn all_caps() -> BackendCapabilities {
    BackendCapabilities {
        pull: true,
        push_updates: true,
        push_create: true,
        transitions: true,
        comments: true,
        custom_properties: true,
        incremental: true,
    }
}

fn title_patch(title: &str) -> CardPatch {
    CardPatch {
        title: Some(title.into()),
        ..Default::default()
    }
}

#[tokio::test]
async fn ensure_is_idempotent_and_missing_context_is_not_found() {
    let mut f = Fixture::new(pull_caps()).await;
    let context = "work".parse().unwrap();
    let (a, b) = tokio::join!(f.boards.ensure(&context), f.boards.ensure(&context));
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(f.reasons(), vec![BoardChangeReason::Created]);
    assert!(matches!(
        f.boards.ensure(&"missing".parse().unwrap()).await,
        Err(DaemonError::NotFound(_))
    ));
    assert!(matches!(
        f.boards.create(&context, None, None, None).await,
        Err(DaemonError::Conflict(_))
    ));
    assert_eq!(f.boards.list(Some(&context)).await.unwrap().len(), 1);
}

#[tokio::test]
async fn card_lifecycle_persists_and_rebuilds_index_after_restart() {
    let mut f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    let card = f.card(&board.id, "Fix login").await;
    assert_eq!(
        uuid::Uuid::parse_str(card.id.as_str())
            .unwrap()
            .get_version_num(),
        4
    );
    assert_eq!(card.created_at, "2026-09-06T12:00:00+00:00");
    assert_eq!(
        f.boards.get(&board.id).await.unwrap().cards,
        vec![card.clone()]
    );
    f.clock
        .set(Utc.with_ymd_and_hms(2026, 9, 6, 13, 0, 0).single().unwrap());
    let reopened = f.reopened();
    let edited = reopened
        .update_card(&card.id, title_patch("Fixed title"))
        .await
        .unwrap();
    assert_eq!(edited.updated_at, "2026-09-06T13:00:00+00:00");
    let moved = reopened
        .move_card(&card.id, &"in-progress".parse().unwrap(), Some(0))
        .await
        .unwrap();
    assert_eq!(moved.status_id.as_str(), "in-progress");
    let commented = reopened
        .add_comment(&card.id, "Ready to review".into())
        .await
        .unwrap();
    assert_eq!(
        uuid::Uuid::parse_str(&commented.comments[0].id)
            .unwrap()
            .get_version_num(),
        4
    );
    assert!(!commented.dirty);
    assert_eq!(
        f.store.load(&board.id).unwrap().unwrap().cards,
        vec![commented]
    );
    reopened.delete_card(&card.id).await.unwrap();
    assert!(f.boards.get(&board.id).await.unwrap().cards.is_empty());
    assert!(matches!(
        f.boards.update_card(&card.id, title_patch("gone")).await,
        Err(DaemonError::NotFound(_))
    ));
    assert_eq!(
        f.reasons(),
        vec![
            BoardChangeReason::Created,
            BoardChangeReason::CardChanged,
            BoardChangeReason::CardChanged,
            BoardChangeReason::CardChanged,
            BoardChangeReason::CardChanged,
            BoardChangeReason::CardChanged
        ]
    );
}

#[tokio::test]
async fn concurrent_card_creation_does_not_lose_cards_or_duplicate_numbers() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..24 {
        let boards = f.boards.clone();
        let id = board.id.clone();
        tasks.spawn(async move {
            boards
                .create_card(
                    &id,
                    CardDraft {
                        title: format!("Card {index}"),
                        ..Default::default()
                    },
                )
                .await
                .unwrap()
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    let doc = f.store.load(&board.id).unwrap().unwrap();
    assert_eq!(doc.cards.len(), 24);
    assert_eq!(doc.board.next_number, 25);
    assert_eq!(
        doc.cards
            .iter()
            .map(|card| card.number)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        24
    );
}

#[tokio::test]
async fn board_validation_is_atomic_and_backend_settings_are_checked() {
    let mut f = Fixture::new(pull_caps()).await;
    f.backend
        .validate_responses
        .lock()
        .unwrap()
        .push_back(Err(BoardError::Backend("bad settings".into())));
    assert!(
        f.boards
            .create(
                &"work".parse().unwrap(),
                None,
                None,
                Some(BackendRef {
                    kind: "fake".into(),
                    settings: serde_json::Value::Null
                })
            )
            .await
            .is_err()
    );
    assert!(f.store.list().unwrap().is_empty());
    let board = f.remote().await.board;
    let board = f
        .boards
        .update(
            &board.id,
            BoardPatch {
                labels: Some(vec![Label {
                    id: "bug".parse().unwrap(),
                    name: "Bug".into(),
                    color: None,
                }]),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .board;
    let card = f
        .boards
        .create_card(
            &board.id,
            CardDraft {
                title: "Bug".into(),
                labels: vec!["bug".parse().unwrap()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let before = f.store.load(&board.id).unwrap().unwrap();
    f.reasons();
    for patch in [
        BoardPatch {
            statuses: Some(
                board
                    .statuses
                    .into_iter()
                    .filter(|status| status.id != card.status_id)
                    .collect(),
            ),
            ..Default::default()
        },
        BoardPatch {
            labels: Some(vec![]),
            ..Default::default()
        },
        BoardPatch {
            prefix: Some("invalid".into()),
            ..Default::default()
        },
        BoardPatch {
            backend: Some(BackendRef {
                kind: "unknown".into(),
                settings: serde_json::Value::Null,
            }),
            ..Default::default()
        },
    ] {
        let error = f.boards.update(&board.id, patch).await.unwrap_err();
        assert!(matches!(error, DaemonError::Validation(_)));
        let message = error.to_string();
        assert!(
            !message.contains(card.id.as_str()),
            "a refusal names the key the user typed, not the one identifier no command ever \
             prints: {message}"
        );
        assert_eq!(f.store.load(&board.id).unwrap().unwrap(), before);
    }
    // A patch that changes nothing writes nothing and announces nothing.
    for patch in [
        BoardPatch::default(),
        BoardPatch {
            name: Some(board.name.clone()),
            prefix: Some(board.prefix.clone()),
            ..Default::default()
        },
    ] {
        f.boards.update(&board.id, patch).await.unwrap();
        assert_eq!(f.store.load(&board.id).unwrap().unwrap(), before);
        assert!(
            f.reasons().is_empty(),
            "a no-op patch must not emit a change nobody made"
        );
    }
    // A rename asks the backend nothing: `validate` is two subprocesses for a real backend, and
    // making a local edit wait on them meant a board could not even be renamed while the CLI
    // behind it was signed out of a system the edit never mentioned.
    let calls = f.backend.calls().len();
    f.boards
        .update(
            &board.id,
            BoardPatch {
                name: Some("Renamed".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.backend.calls().len(),
        calls,
        "a patch that names no backend setting must not reach the backend"
    );
    f.boards
        .update(
            &board.id,
            BoardPatch {
                name: Some(board.name.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    f.reasons();
    let before = f.store.load(&board.id).unwrap().unwrap();
    // A patch that *does* carry settings is checked, and a backend that refuses them leaves
    // the document exactly as it was.
    f.backend
        .validate_responses
        .lock()
        .unwrap()
        .push_back(Err(BoardError::Backend("offline".into())));
    assert!(
        f.boards
            .update(
                &board.id,
                BoardPatch {
                    backend: Some(BackendRef {
                        kind: "fake".into(),
                        settings: serde_json::json!({"project":"TEST","jql":"sprint"}),
                    }),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(f.store.load(&board.id).unwrap().unwrap(), before);
    assert!(f.reasons().is_empty());
    assert!(f.backend.calls().iter().any(
        |call| matches!(call, FakeBackendCall::Validate(settings) if settings["project"] == "TEST")
    ));
}

#[tokio::test]
async fn delete_clears_children_index_and_trashes_orphaned_board() {
    let mut f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    let parent = f.card(&board.id, "Parent").await;
    let child = f
        .boards
        .create_card(
            &board.id,
            CardDraft {
                title: "Child".into(),
                parent_id: Some(parent.id.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    f.boards.delete_card(&parent.id).await.unwrap();
    assert!(
        f.boards.get(&board.id).await.unwrap().cards[0]
            .parent_id
            .is_none()
    );
    f.state
        .transaction(|state| {
            state.contexts.clear();
            state.repos.clear();
            Ok(())
        })
        .await
        .unwrap();
    assert!(f.boards.list(None).await.unwrap().is_empty());
    assert!(f.boards.summaries().await.is_empty());
    f.boards.delete(&board.id).await.unwrap();
    assert!(f.store.load(&board.id).unwrap().is_none());
    assert_eq!(std::fs::read_dir(f.home.trash_dir()).unwrap().count(), 1);
    assert!(matches!(
        f.boards.add_comment(&child.id, "gone".into()).await,
        Err(DaemonError::NotFound(_))
    ));
    assert_eq!(f.reasons().last(), Some(&BoardChangeReason::Deleted));
}

#[tokio::test]
async fn worktree_creation_links_starts_and_reuses_existing_slug() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    let card = f.card(&board.id, "Fix login").await;
    assert!(matches!(
        f.boards
            .create_worktree_from_card(&card.id, None, None, None)
            .await,
        Err(DaemonError::Validation(_))
    ));
    f.boards
        .update(
            &board.id,
            BoardPatch {
                default_repo_id: Some(Some("acme/api".parse().unwrap())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let (linked, worktree, created) = tokio::time::timeout(
        Duration::from_secs(5),
        f.boards
            .create_worktree_from_card(&card.id, None, Some("origin/main".into()), None),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(created);
    assert_eq!(worktree.slug, "wor-1-fix-login");
    assert_eq!(worktree.branch, worktree.slug);
    assert_eq!(linked.worktree_id, Some(worktree.id.clone()));
    assert_eq!(linked.repo_id, Some(worktree.repo_id.clone()));
    assert_eq!(linked.status_id.as_str(), "in-progress");
    assert!(
        linked
            .activity
            .iter()
            .any(|entry| entry.kind == ActivityKind::WorktreeCreated)
    );
    assert!(f.shell.calls().iter().any(|call| matches!(call, FakeShellCall::Run(command) if command.args == ["checkout", "-b", "wor-1-fix-login", "origin/main"])));
    let calls = f.shell.calls().len();
    let (_, same, _) = f
        .boards
        .create_worktree_from_card(&card.id, None, None, None)
        .await
        .unwrap();
    assert_eq!(same.id, worktree.id);
    assert_eq!(f.shell.calls().len(), calls);
    f.state
        .transaction(|state| {
            state.worktrees.clear();
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        f.boards.get(&board.id).await.unwrap().cards[0]
            .worktree_id
            .is_none()
    );
    let edited = f
        .boards
        .update_card(&card.id, title_patch("After prune"))
        .await
        .unwrap();
    assert!(edited.worktree_id.is_none());
    let commented = f
        .boards
        .add_comment(&card.id, "Still no worktree".into())
        .await
        .unwrap();
    assert!(commented.worktree_id.is_none());
    assert!(
        f.store.load(&board.id).unwrap().unwrap().cards[0]
            .worktree_id
            .is_some()
    );
}

#[tokio::test]
async fn worktree_repo_override_and_start_setting_are_respected() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    // An unregistered repository never reaches the document, so no card can inherit it.
    assert!(matches!(
        f.boards
            .update(
                &board.id,
                BoardPatch {
                    default_repo_id: Some(Some("missing/default".parse().unwrap())),
                    ..Default::default()
                },
            )
            .await,
        Err(DaemonError::NotFound(_))
    ));
    f.boards
        .update(
            &board.id,
            BoardPatch {
                settings: Some(BoardSettings {
                    start_on_worktree: false,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        f.boards
            .create_card(
                &board.id,
                CardDraft {
                    title: "Task".into(),
                    repo_id: Some("missing/card".parse().unwrap()),
                    ..Default::default()
                },
            )
            .await,
        Err(DaemonError::NotFound(_))
    ));
    let card = f
        .boards
        .create_card(
            &board.id,
            CardDraft {
                title: "Task".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    // Neither the card nor the board names a repository.
    assert!(matches!(
        f.boards
            .create_worktree_from_card(&card.id, None, None, None)
            .await,
        Err(DaemonError::Validation(_))
    ));
    let (linked, _, _) = f
        .boards
        .create_worktree_from_card(&card.id, Some("acme/api".parse().unwrap()), None, None)
        .await
        .unwrap();
    assert_eq!(linked.status_id.as_str(), "todo");
    assert_eq!(linked.repo_id.unwrap().as_str(), "acme/api");
}

#[tokio::test]
async fn local_sync_submits_no_job_and_remote_without_pull_fails_in_job() {
    let mut f = Fixture::new(BackendCapabilities::default()).await;
    let board = f.local().await.board;
    assert!(matches!(
        f.boards.sync(&board.id, false).await,
        Err(DaemonError::Unsupported(_))
    ));
    assert!(f.jobs.list().is_empty());
    f.boards
        .update(
            &board.id,
            BoardPatch {
                backend: Some(BackendRef {
                    kind: "fake".into(),
                    settings: serde_json::Value::Null,
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let job = f.sync(&board.id).await;
    assert_eq!(job.kind, JobKind::Custom("board.sync".into()));
    assert!(matches!(job.status, JobStatus::Failed { .. }));
    assert!(
        f.boards
            .get(&board.id)
            .await
            .unwrap()
            .board
            .sync
            .last_error
            .unwrap()
            .contains("pull")
    );
    assert_eq!(f.reasons().last(), Some(&BoardChangeReason::SyncFailed));
}

#[tokio::test]
async fn sync_adopts_schema_remaps_existing_cards_and_restores_cursor() {
    let mut f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    let local = f.card(&board.id, "Local").await;
    let mut child = remote("EXT-1", "Child", "v1");
    child.parent_key = Some("EXT-2".into());
    f.pull(
        vec![child, remote("EXT-2", "Parent", "v1")],
        Some("cursor-1"),
    );
    let job = f.sync(&board.id).await;
    assert_eq!(job.status, JobStatus::Succeeded);
    let view = f.boards.get(&board.id).await.unwrap();
    assert_eq!(view.board.statuses[0].id.as_str(), "open");
    assert_eq!(
        view.cards
            .iter()
            .find(|card| card.id == local.id)
            .unwrap()
            .status_id
            .as_str(),
        "open"
    );
    let parent = view
        .cards
        .iter()
        .find(|card| card.title == "Parent")
        .unwrap();
    let child = view
        .cards
        .iter()
        .find(|card| card.title == "Child")
        .unwrap();
    assert_eq!(child.parent_id.as_ref(), Some(&parent.id));
    for card in &view.cards {
        assert_eq!(
            uuid::Uuid::parse_str(card.id.as_str())
                .unwrap()
                .get_version_num(),
            4
        );
    }
    f.pull(vec![], Some("cursor-2"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    assert!(f.backend.calls().iter().any(
        |call| matches!(call, FakeBackendCall::Pull(_, Some(cursor)) if cursor == "cursor-1")
    ));
    assert_eq!(
        f.reopened()
            .get(&board.id)
            .await
            .unwrap()
            .board
            .sync
            .cursor
            .as_deref(),
        Some("cursor-2")
    );
    assert_eq!(f.reasons().last(), Some(&BoardChangeReason::Synced));
    let log = f.jobs.tail(&job.id, 30).await.unwrap().join("\n");
    for step in [
        "describing",
        "adopting",
        "pulling",
        "reconciling",
        "saving",
        "2 pulled",
    ] {
        assert!(log.contains(step), "{log}");
    }
}

#[tokio::test]
async fn manual_conflicts_resolve_both_ways_with_labels_and_parent_lookup() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    f.pull(
        vec![
            remote("EXT-1", "Original", "v1"),
            remote("EXT-2", "Parent", "v1"),
        ],
        None,
    );
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let cards = f.boards.get(&board.id).await.unwrap().cards;
    let card = cards.iter().find(|card| card.title == "Original").unwrap();
    let parent = cards.iter().find(|card| card.title == "Parent").unwrap();
    f.boards
        .update_card(&card.id, title_patch("Local"))
        .await
        .unwrap();
    f.pull(vec![remote("EXT-1", "Remote v2", "v2")], None);
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let conflicted = f
        .boards
        .get(&board.id)
        .await
        .unwrap()
        .cards
        .into_iter()
        .find(|item| item.id == card.id)
        .unwrap();
    assert!(conflicted.conflict.is_some());
    assert_eq!(conflicted.title, "Local");
    let kept = f
        .boards
        .resolve_conflict(&card.id, ConflictResolution::KeepLocal)
        .await
        .unwrap();
    assert!(kept.conflict.is_none());
    assert!(kept.dirty);
    assert_eq!(kept.title, "Local");
    assert_eq!(kept.remote.unwrap().version.as_deref(), Some("v2"));
    let mut next = remote("EXT-1", "Remote v3", "v3");
    next.labels = vec!["New label".into()];
    next.parent_key = Some("EXT-2".into());
    f.pull(vec![next], None);
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let taken = f
        .boards
        .resolve_conflict(&card.id, ConflictResolution::TakeRemote)
        .await
        .unwrap();
    assert_eq!(taken.title, "Remote v3");
    assert_eq!(taken.parent_id.as_ref(), Some(&parent.id));
    assert!(!taken.dirty);
    assert!(taken.conflict.is_none());
    let persisted = f.store.load(&board.id).unwrap().unwrap();
    assert!(
        persisted
            .board
            .labels
            .iter()
            .any(|label| label.name == "New label" && taken.labels.contains(&label.id))
    );
    assert!(
        taken
            .activity
            .iter()
            .any(|entry| entry.kind == ActivityKind::ConflictResolved)
    );
}

#[tokio::test]
async fn push_create_update_transition_and_comments_reach_backend_and_acks_persist() {
    let f = Fixture::new(all_caps()).await;
    let board = f.remote().await.board;
    f.boards
        .update(
            &board.id,
            BoardPatch {
                settings: Some(BoardSettings {
                    push_new_cards: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let card = f.card(&board.id, "Local task").await;
    f.pull(vec![], None);
    f.backend
        .push_responses
        .lock()
        .unwrap()
        .push_back(Ok(PushResult {
            acks: vec![PushAck {
                card_id: card.id.clone(),
                key: "EXT-10".into(),
                url: Some("https://example.test/EXT-10".into()),
                version: Some("v1".into()),
                remote_updated_at: None,
                comment_ids: vec![],
            }],
            failures: vec![],
        }));
    let job = f.sync(&board.id).await;
    assert_eq!(job.status, JobStatus::Succeeded);
    let linked = f.boards.get(&board.id).await.unwrap().cards.remove(0);
    assert_eq!(linked.remote.unwrap().key, "EXT-10");
    assert!(!linked.dirty);
    assert!(
        f.jobs
            .tail(&job.id, 30)
            .await
            .unwrap()
            .join("\n")
            .contains("1 pushed")
    );
    f.boards
        .update_card(&card.id, title_patch("Edited"))
        .await
        .unwrap();
    f.boards
        .move_card(&card.id, &"doing".parse().unwrap(), None)
        .await
        .unwrap();
    let commented = f
        .boards
        .add_comment(&card.id, "Comment".into())
        .await
        .unwrap();
    let comment = &commented.comments[0].id;
    f.pull(vec![remote("EXT-10", "Local task", "v1")], None);
    f.backend
        .push_responses
        .lock()
        .unwrap()
        .push_back(Ok(PushResult {
            acks: vec![PushAck {
                card_id: card.id.clone(),
                key: "EXT-10".into(),
                url: None,
                version: Some("v2".into()),
                remote_updated_at: None,
                comment_ids: vec![(comment.clone(), "remote-comment".into())],
            }],
            failures: vec![],
        }));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let calls = f.backend.calls();
    let pushed: Vec<_> = calls
        .iter()
        .filter_map(|call| {
            if let FakeBackendCall::Push(_, _, ops) = call {
                Some(ops)
            } else {
                None
            }
        })
        .collect();
    assert!(pushed[0].contains(&PushOp::Create {
        card_id: card.id.clone()
    }));
    assert!(
        pushed[1].iter().any(
            |op| matches!(op, PushOp::Update { fields, .. } if fields.contains(&"title".into()))
        )
    );
    assert!(pushed[1].contains(&PushOp::Transition {
        card_id: card.id.clone(),
        remote_status: "doing-id".into()
    }));
    assert!(pushed[1].contains(&PushOp::AddComment {
        card_id: card.id,
        comment_id: comment.clone()
    }));
    let saved = f.store.load(&board.id).unwrap().unwrap().cards.remove(0);
    assert!(!saved.dirty);
    assert_eq!(
        saved.comments[0].remote_id.as_deref(),
        Some("remote-comment")
    );
}

/// One remote issue this board cannot import used to stop the push block from ever running:
/// every local edit, move and comment stayed queued behind it, the cursor never advanced, and
/// the error named only the key that was skipped.
#[tokio::test]
async fn an_unimportable_remote_card_is_a_warning_and_never_strands_the_local_edits() {
    let f = Fixture::new(all_caps()).await;
    let board = f.remote().await.board;
    f.pull(vec![remote("EXT-1", "Initial", "v1")], Some("cursor-1"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let card = f.boards.get(&board.id).await.unwrap().cards.remove(0);
    f.boards
        .update_card(&card.id, title_patch("Local wins"))
        .await
        .unwrap();
    // A remote card with a blank title is one `validate_card` refuses: it lands in
    // `summary.skipped` and must not take the push down with it.
    f.pull(
        vec![
            remote("EXT-1", "Initial", "v1"),
            remote("EXT-9", "   ", "v1"),
        ],
        Some("cursor-2"),
    );
    f.backend
        .push_responses
        .lock()
        .unwrap()
        .push_back(Ok(PushResult {
            acks: vec![PushAck {
                card_id: card.id.clone(),
                key: "EXT-1".into(),
                url: None,
                version: Some("v2".into()),
                remote_updated_at: None,
                comment_ids: vec![],
            }],
            failures: vec![],
        }));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let pushed: Vec<Vec<PushOp>> = f
        .backend
        .calls()
        .iter()
        .filter_map(|call| match call {
            FakeBackendCall::Push(_, _, ops) => Some(ops.clone()),
            _ => None,
        })
        .collect();
    assert!(
        pushed
            .iter()
            .flatten()
            .any(|op| matches!(op, PushOp::Update { card_id, .. } if *card_id == card.id)),
        "the local edit must have reached the backend: {pushed:?}"
    );
    let saved = f.store.load(&board.id).unwrap().unwrap();
    assert!(
        !saved
            .cards
            .iter()
            .any(|item| item.id == card.id && item.dirty)
    );
    // The skipped key is still reported, and the cursor stays behind it so the next pull
    // offers it again.
    let error = saved.board.sync.last_error.unwrap();
    assert!(error.contains("EXT-9"), "{error}");
    assert_eq!(saved.board.sync.cursor.as_deref(), Some("cursor-1"));
}

/// `delete_card` on a mirrored card removed the local row only: no tombstone, no push, and the
/// next full pull filed the issue again as a new card with none of its local history.
#[tokio::test]
async fn a_mirrored_card_cannot_be_deleted_locally_and_the_refusal_names_its_issue() {
    let f = Fixture::new(all_caps()).await;
    let board = f.remote().await.board;
    f.pull(vec![remote("EXT-1", "Initial", "v1")], Some("cursor-1"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let card = f.boards.get(&board.id).await.unwrap().cards.remove(0);
    let error = f
        .boards
        .delete_card(&card.id)
        .await
        .expect_err("a mirrored card is the backend's to delete")
        .to_string();
    assert!(error.contains("EXT-1"), "{error}");
    assert!(error.contains("archive"), "{error}");
    assert_eq!(f.boards.get(&board.id).await.unwrap().cards.len(), 1);
    // A card of this board's own is still the user's to delete.
    let local = f.card(&board.id, "Local only").await;
    f.boards.delete_card(&local.id).await.unwrap();
}

#[tokio::test]
async fn failed_push_preserves_pull_dirty_cards_and_is_retryable() {
    let mut f = Fixture::new(all_caps()).await;
    let board = f.remote().await.board;
    f.pull(vec![remote("EXT-1", "Initial", "v1")], Some("cursor-1"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let card = f.boards.get(&board.id).await.unwrap().cards.remove(0);
    f.boards
        .update_card(&card.id, title_patch("Dirty"))
        .await
        .unwrap();
    f.pull(
        vec![
            remote("EXT-1", "Initial", "v1"),
            remote("EXT-2", "Pulled before failure", "v1"),
        ],
        Some("cursor-2"),
    );
    f.backend
        .push_responses
        .lock()
        .unwrap()
        .push_back(Err(BoardError::Backend("offline".into())));
    let failed = f.sync(&board.id).await;
    assert!(matches!(failed.status, JobStatus::Failed { .. }));
    let saved = f.store.load(&board.id).unwrap().unwrap();
    assert_eq!(saved.cards.len(), 2);
    assert!(
        saved
            .cards
            .iter()
            .find(|item| item.id == card.id)
            .unwrap()
            .dirty
    );
    assert!(saved.board.sync.last_error.unwrap().contains("offline"));
    assert_eq!(saved.board.sync.cursor.as_deref(), Some("cursor-2"));
    assert_eq!(f.reasons().last(), Some(&BoardChangeReason::SyncFailed));
    f.pull(vec![], None);
    f.backend
        .pull_responses
        .lock()
        .unwrap()
        .push_back(Ok(PullResult {
            cards: vec![
                remote("EXT-1", "Initial", "v1"),
                remote("EXT-2", "Pulled before failure", "v1"),
            ],
            full: true,
            ..Default::default()
        }));
    f.backend
        .push_responses
        .lock()
        .unwrap()
        .push_back(Ok(PushResult {
            acks: vec![PushAck {
                card_id: card.id,
                key: "EXT-1".into(),
                url: None,
                version: Some("v2".into()),
                remote_updated_at: None,
                comment_ids: vec![],
            }],
            failures: vec![],
        }));
    let retried = f.jobs.retry(&failed.id).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), f.jobs.wait(&retried.id))
            .await
            .unwrap()
            .unwrap()
            .status,
        JobStatus::Succeeded
    );
    assert!(
        f.boards
            .get(&board.id)
            .await
            .unwrap()
            .board
            .sync
            .last_error
            .is_none()
    );
}

#[tokio::test]
async fn linked_cards_without_sync_metadata_prevent_wholesale_status_adoption() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    let card = f.card(&board.id, "Linked").await;
    let mut doc = f.store.load(&board.id).unwrap().unwrap();
    doc.cards[0].remote = Some(RemoteLink {
        parent_key: None,
        backend: "fake".into(),
        key: "EXT-1".into(),
        url: None,
        version: Some("v1".into()),
        synced_at: "2026-09-05T12:00:00Z".into(),
        remote_updated_at: None,
    });
    doc.cards[0].dirty = false;
    f.store.save(&doc).unwrap();
    f.pull(vec![], None);
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let saved = f.store.load(&board.id).unwrap().unwrap();
    assert_eq!(saved.board.statuses, default_statuses());
    assert_eq!(saved.cards[0].id, card.id);
    assert_eq!(saved.cards[0].status_id.as_str(), "todo");
}

#[tokio::test]
async fn partial_push_failure_keeps_acks_and_marks_the_job_failed() {
    let mut f = Fixture::new(all_caps()).await;
    let board = f.remote().await.board;
    f.boards
        .update(
            &board.id,
            BoardPatch {
                settings: Some(BoardSettings {
                    push_new_cards: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let card = f.card(&board.id, "Local").await;
    f.pull(vec![], None);
    f.backend
        .push_responses
        .lock()
        .unwrap()
        .push_back(Ok(PushResult {
            acks: vec![PushAck {
                card_id: card.id.clone(),
                key: "EXT-1".into(),
                url: None,
                version: Some("v1".into()),
                remote_updated_at: None,
                comment_ids: vec![],
            }],
            failures: vec![PushFailure {
                card_id: card.id,
                error: "comment rejected".into(),
            }],
        }));
    assert!(matches!(
        f.sync(&board.id).await.status,
        JobStatus::Failed { .. }
    ));
    let doc = f.store.load(&board.id).unwrap().unwrap();
    assert!(doc.cards[0].dirty);
    assert_eq!(doc.cards[0].remote.as_ref().unwrap().key, "EXT-1");
    assert!(
        doc.board
            .sync
            .last_error
            .unwrap()
            .contains("comment rejected")
    );
    assert_eq!(f.reasons().last(), Some(&BoardChangeReason::SyncFailed));
}

#[tokio::test]
async fn invalid_remote_data_does_not_replace_the_last_valid_document() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    let mut invalid = remote("EXT-1", "Bad date", "v1");
    invalid.due_date = Some("not-a-date".into());
    f.pull(vec![invalid], Some("invalid-cursor"));
    assert!(matches!(
        f.sync(&board.id).await.status,
        JobStatus::Failed { .. }
    ));
    let doc = f.store.load(&board.id).unwrap().unwrap();
    assert!(doc.cards.is_empty());
    assert!(doc.board.sync.cursor.is_none());
    assert!(doc.board.sync.last_error.is_some());
    assert!(f.home.board_path(&board.id).exists());
}

#[tokio::test]
async fn nonincremental_backends_never_receive_saved_cursor() {
    let f = Fixture::new(BackendCapabilities {
        pull: true,
        ..Default::default()
    })
    .await;
    let board = f.remote().await.board;
    f.pull(vec![], Some("saved"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    f.pull(vec![], None);
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    assert!(
        f.backend
            .calls()
            .iter()
            .filter_map(|call| if let FakeBackendCall::Pull(_, cursor) = call {
                Some(cursor)
            } else {
                None
            })
            .all(Option::is_none)
    );
}

#[tokio::test]
async fn invalid_card_mutations_leave_document_and_events_unchanged() {
    let mut f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    let card = f.card(&board.id, "Valid").await;
    let before = f.store.load(&board.id).unwrap().unwrap();
    f.reasons();
    assert!(
        f.boards
            .create_card(&board.id, CardDraft::default())
            .await
            .is_err()
    );
    assert!(
        f.boards
            .update_card(&card.id, title_patch(" "))
            .await
            .is_err()
    );
    assert!(
        f.boards
            .move_card(&card.id, &"unknown".parse().unwrap(), None)
            .await
            .is_err()
    );
    assert!(f.boards.add_comment(&card.id, " ".into()).await.is_err());
    assert!(
        f.boards
            .resolve_conflict(&card.id, ConflictResolution::TakeRemote)
            .await
            .is_err()
    );
    assert_eq!(f.store.load(&board.id).unwrap().unwrap(), before);
    assert!(f.reasons().is_empty());
}

#[tokio::test]
async fn describe_and_pull_errors_persist_failure_and_success_clears_it() {
    let mut f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    f.backend
        .describe_responses
        .lock()
        .unwrap()
        .push_back(Err(BoardError::Backend("describe offline".into())));
    assert!(matches!(
        f.sync(&board.id).await.status,
        JobStatus::Failed { .. }
    ));
    assert!(
        f.store
            .load(&board.id)
            .unwrap()
            .unwrap()
            .board
            .sync
            .last_error
            .unwrap()
            .contains("describe offline")
    );
    f.backend
        .describe_responses
        .lock()
        .unwrap()
        .push_back(Ok(schema()));
    f.backend
        .pull_responses
        .lock()
        .unwrap()
        .push_back(Err(BoardError::Backend("pull offline".into())));
    assert!(matches!(
        f.sync(&board.id).await.status,
        JobStatus::Failed { .. }
    ));
    let saved = f.store.load(&board.id).unwrap().unwrap();
    assert!(
        saved
            .board
            .sync
            .last_error
            .unwrap()
            .contains("pull offline")
    );
    assert_eq!(
        saved.board.statuses, board.statuses,
        "schema adoption waits for a valid pull"
    );
    assert!(saved.board.sync.last_synced_at.is_none());
    f.pull(vec![], Some("recovered"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    assert!(
        f.store
            .load(&board.id)
            .unwrap()
            .unwrap()
            .board
            .sync
            .last_error
            .is_none()
    );
    assert_eq!(
        f.reasons(),
        vec![
            BoardChangeReason::Created,
            BoardChangeReason::SyncFailed,
            BoardChangeReason::SyncFailed,
            BoardChangeReason::Synced
        ]
    );
}

#[tokio::test]
async fn full_pull_archives_missing_remote_cards_and_preserves_local_cards() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    let local = f.card(&board.id, "Local").await;
    f.pull(vec![remote("EXT-1", "Remote", "v1")], Some("cursor"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    f.backend
        .describe_responses
        .lock()
        .unwrap()
        .push_back(Ok(schema()));
    f.backend
        .pull_responses
        .lock()
        .unwrap()
        .push_back(Ok(PullResult {
            full: true,
            // A pull that reports a deletion of its own has seen the project. A wholly empty one
            // is a filter that matched nothing, and archives nothing.
            deleted_keys: vec!["EXT-9".into()],
            ..Default::default()
        }));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let doc = f.store.load(&board.id).unwrap().unwrap();
    assert!(doc.board.sync.cursor.is_none());
    assert!(
        doc.cards
            .iter()
            .find(|card| card.remote.is_some())
            .unwrap()
            .archived
    );
    assert!(
        !doc.cards
            .iter()
            .find(|card| card.id == local.id)
            .unwrap()
            .archived
    );
    assert_eq!(f.boards.summaries().await[0].card_count, 1);
}

#[tokio::test]
async fn describe_backend_is_read_only_and_existing_completed_worktree_is_reused() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    let before = f.store.load(&board.id).unwrap().unwrap();
    f.backend
        .describe_responses
        .lock()
        .unwrap()
        .push_back(Ok(schema()));
    assert_eq!(
        f.boards.describe_backend(&board.id).await.unwrap(),
        schema()
    );
    assert_eq!(f.store.load(&board.id).unwrap().unwrap(), before);
    let card = f
        .boards
        .create_card(
            &board.id,
            CardDraft {
                title: "Done".into(),
                status_id: Some("done".parse().unwrap()),
                repo_id: Some("acme/api".parse().unwrap()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let (_, worktree, _) = f
        .boards
        .create_worktree_from_card(&card.id, None, None, None)
        .await
        .unwrap();
    f.state
        .transaction(|state| {
            state.worktrees[0].branch = "existing-other-branch".into();
            Ok(())
        })
        .await
        .unwrap();
    let (card, reused, _) = f
        .boards
        .create_worktree_from_card(&card.id, None, None, None)
        .await
        .unwrap();
    assert_eq!(reused.id, worktree.id);
    assert_eq!(reused.branch, "existing-other-branch");
    assert_eq!(card.status_id.as_str(), "done");
    assert!(card.dirty);
}

#[tokio::test]
async fn a_board_only_accepts_repositories_from_its_own_context() {
    let f = Fixture::new(BackendCapabilities::default()).await;
    let board = f.local().await.board;
    f.state
        .transaction(|state| {
            state.contexts.push(Context {
                id: "personal".parse().unwrap(),
                name: "Personal".into(),
                owners: vec![],
                created_at: "2026-09-06T12:00:00Z".into(),
            });
            let mut repo = state.repos[0].clone();
            repo.id = "acme/side".parse().unwrap();
            repo.name = "side".into();
            repo.context_id = "personal".parse().unwrap();
            state.repos.push(repo);
            Ok(())
        })
        .await
        .unwrap();
    let foreign: fleet_core::ids::RepoId = "acme/side".parse().unwrap();
    // The app only ever offers this context's repositories, so a board that stored another
    // context's repo would render it as "none" and create worktrees nobody can see.
    assert!(matches!(
        f.boards
            .update(
                &board.id,
                BoardPatch {
                    default_repo_id: Some(Some(foreign.clone())),
                    ..Default::default()
                },
            )
            .await,
        Err(DaemonError::Validation(_))
    ));
    assert!(matches!(
        f.boards
            .create_card(
                &board.id,
                CardDraft {
                    title: "Task".into(),
                    repo_id: Some(foreign),
                    ..Default::default()
                },
            )
            .await,
        Err(DaemonError::Validation(_))
    ));
}

#[tokio::test]
async fn a_worktree_another_card_owns_is_never_adopted_by_slug() {
    let f = Fixture::new(BackendCapabilities::default()).await;
    let board = f.local().await.board;
    f.boards
        .update(
            &board.id,
            BoardPatch {
                settings: Some(BoardSettings {
                    branch_template: "{slug}".into(),
                    start_on_worktree: false,
                    ..Default::default()
                }),
                default_repo_id: Some(Some("acme/api".parse().unwrap())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let first = f.card(&board.id, "Same title").await;
    let second = f.card(&board.id, "Same title").await;
    let (_, worktree, created) = f
        .boards
        .create_worktree_from_card(&first.id, None, None, None)
        .await
        .unwrap();
    assert!(created);
    // Two cards sharing one worktree would open one another's session under `o`.
    assert!(matches!(
        f.boards
            .create_worktree_from_card(&second.id, None, None, None)
            .await,
        Err(DaemonError::Conflict(_))
    ));
    let cards = f.boards.get(&board.id).await.unwrap().cards;
    assert_eq!(
        cards
            .iter()
            .filter(|card| card.worktree_id.as_ref() == Some(&worktree.id))
            .count(),
        1
    );
}

#[tokio::test]
async fn invalid_incremental_import_keeps_cursor_for_retry() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    f.pull(vec![], Some("before"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    f.pull(vec![remote("EXT-1", " ", "v1")], Some("after"));
    assert!(matches!(
        f.sync(&board.id).await.status,
        JobStatus::Failed { .. }
    ));
    assert_eq!(
        f.boards
            .get(&board.id)
            .await
            .unwrap()
            .board
            .sync
            .cursor
            .as_deref(),
        Some("before")
    );
    f.pull(vec![remote("EXT-1", "Repaired", "v1")], Some("after"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    assert_eq!(
        f.boards.get(&board.id).await.unwrap().cards[0].title,
        "Repaired"
    );
}

#[tokio::test]
async fn schema_removal_and_type_changes_migrate_values_atomically() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    let property = |key: &str, kind| PropertySchema {
        key: key.into(),
        name: key.into(),
        kind,
        source: PropertySource::Backend,
        options: vec![],
        editable: true,
        show_on_card: false,
    };
    let mut first = schema();
    first.properties = vec![
        property("removed", PropertyKind::Text),
        property("changed", PropertyKind::Text),
    ];
    let mut card = remote("EXT-1", "Task", "v1");
    card.properties
        .insert("removed".into(), PropertyValue::Text("old".into()));
    card.properties
        .insert("changed".into(), PropertyValue::Text("old".into()));
    f.backend
        .describe_responses
        .lock()
        .unwrap()
        .push_back(Ok(first));
    f.backend
        .pull_responses
        .lock()
        .unwrap()
        .push_back(Ok(PullResult {
            cards: vec![card.clone()],
            ..Default::default()
        }));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let mut second = schema();
    second.properties = vec![property("changed", PropertyKind::Number)];
    card.version = Some("v2".into());
    card.properties = [("changed".into(), PropertyValue::Number(2.0))].into();
    f.backend
        .describe_responses
        .lock()
        .unwrap()
        .push_back(Ok(second));
    f.backend
        .pull_responses
        .lock()
        .unwrap()
        .push_back(Ok(PullResult {
            cards: vec![card],
            ..Default::default()
        }));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let doc = f.store.load(&board.id).unwrap().unwrap();
    assert_eq!(
        doc.cards[0].properties,
        [("changed".into(), PropertyValue::Number(2.0))].into()
    );
    validate_card(&doc.board, &doc.cards[0]).unwrap();
}

#[tokio::test]
async fn incremental_omission_fetches_baseline_and_pushes_pending_transition() {
    let f = Fixture::new(all_caps()).await;
    let board = f.remote().await.board;
    let remote = remote("EXT-1", "Task", "v1");
    f.pull(vec![remote.clone()], Some("cursor"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let card = f.boards.get(&board.id).await.unwrap().cards.remove(0);
    f.boards
        .move_card(&card.id, &"doing".parse().unwrap(), None)
        .await
        .unwrap();
    f.pull(vec![], Some("next"));
    f.backend
        .pull_responses
        .lock()
        .unwrap()
        .push_back(Ok(PullResult {
            cards: vec![remote],
            full: true,
            ..Default::default()
        }));
    f.backend
        .push_responses
        .lock()
        .unwrap()
        .push_back(Ok(PushResult {
            acks: vec![PushAck {
                card_id: card.id.clone(),
                key: "EXT-1".into(),
                url: None,
                version: Some("v2".into()),
                remote_updated_at: None,
                comment_ids: vec![],
            }],
            failures: vec![],
        }));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    assert!(f.backend.calls().iter().any(|call| matches!(call, FakeBackendCall::Push(_, _, ops) if ops.contains(&PushOp::Transition { card_id: card.id.clone(), remote_status: "doing-id".into() }))));
    assert!(!f.boards.get(&board.id).await.unwrap().cards[0].dirty);
}

#[tokio::test]
async fn linked_board_rejects_a_kind_change_but_still_takes_new_settings() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    f.pull(vec![remote("EXT-1", "Task", "v1")], Some("cursor"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let before = f.store.load(&board.id).unwrap().unwrap();
    // The links name issues on the remote this board would be leaving.
    assert!(matches!(
        f.boards
            .update(
                &board.id,
                BoardPatch {
                    backend: Some(BackendRef::default()),
                    ..Default::default()
                }
            )
            .await,
        Err(DaemonError::Conflict(_))
    ));
    assert_eq!(f.store.load(&board.id).unwrap().unwrap(), before);
    // The required setting names the remote itself, so changing it under linked cards points
    // every key at a system that never issued it — the same refusal, for the same reason.
    assert!(matches!(
        f.boards
            .update(
                &board.id,
                BoardPatch {
                    backend: Some(BackendRef {
                        kind: "fake".into(),
                        settings: serde_json::json!({"project": "other"}),
                    }),
                    ..Default::default()
                }
            )
            .await,
        Err(DaemonError::Conflict(_))
    ));
    assert_eq!(f.store.load(&board.id).unwrap().unwrap(), before);
    // Same kind, same remote, new settings: this is the whole point of `--setting` and of the
    // settings dialog, and refusing it would make both dead the moment a board syncs once.
    let view = f
        .boards
        .update(
            &board.id,
            BoardPatch {
                backend: Some(BackendRef {
                    kind: "fake".into(),
                    settings: serde_json::json!({"project":"TEST","filter":"mine"}),
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        view.board.backend.settings,
        serde_json::json!({"project":"TEST","filter":"mine"})
    );
    let after = f.store.load(&board.id).unwrap().unwrap();
    // The cursor is a window into a query this board no longer runs; the status map and the
    // read-only list still describe the same remote and must survive, or the read-only guard
    // is disarmed until the next successful sync.
    assert!(after.board.sync.cursor.is_none());
    assert_eq!(after.board.sync.status_map, before.board.sync.status_map);
    assert_eq!(
        after.board.sync.readonly_fields,
        before.board.sync.readonly_fields
    );
}

#[tokio::test]
async fn timestamp_push_refreshes_baseline_before_next_local_edit() {
    let f = Fixture::new(all_caps()).await;
    let board = f.remote().await.board;
    f.boards
        .update(
            &board.id,
            BoardPatch {
                settings: Some(BoardSettings {
                    conflict_policy: ConflictPolicy::RemoteWins,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut remote = remote("EXT-1", "Initial", "unused");
    remote.version = None;
    remote.updated_at = Some("2026-09-06T12:00:00Z".into());
    f.pull(vec![remote.clone()], None);
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let card = f.boards.get(&board.id).await.unwrap().cards.remove(0);
    f.boards
        .update_card(&card.id, title_patch("First edit"))
        .await
        .unwrap();
    f.pull(vec![remote.clone()], None);
    remote.title = "First edit".into();
    remote.updated_at = Some("2026-09-06T12:01:00Z".into());
    f.backend
        .pull_responses
        .lock()
        .unwrap()
        .push_back(Ok(PullResult {
            cards: vec![remote.clone()],
            full: true,
            ..Default::default()
        }));
    f.backend
        .push_responses
        .lock()
        .unwrap()
        .push_back(Ok(PushResult {
            acks: vec![PushAck {
                card_id: card.id.clone(),
                key: "EXT-1".into(),
                url: None,
                version: None,
                remote_updated_at: None,
                comment_ids: vec![],
            }],
            failures: vec![],
        }));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let saved = f.boards.get(&board.id).await.unwrap().cards.remove(0);
    assert_eq!(saved.remote.unwrap().remote_updated_at, remote.updated_at);
    f.boards
        .update_card(&card.id, title_patch("Second edit"))
        .await
        .unwrap();
    f.pull(vec![remote], None);
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    assert_eq!(
        f.boards.get(&board.id).await.unwrap().cards[0].title,
        "Second edit"
    );
}

#[tokio::test]
async fn archived_card_is_rejected_before_worktree_side_effects() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    let card = f.card(&board.id, "Archived").await;
    f.boards
        .update_card(
            &card.id,
            CardPatch {
                archived: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        f.boards
            .create_worktree_from_card(&card.id, Some("acme/api".parse().unwrap()), None, None)
            .await
            .is_err()
    );
    assert!(f.state.load().await.unwrap().worktrees.is_empty());
    assert!(f.jobs.list().is_empty());
    assert!(f.shell.calls().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_during_worktree_creation_still_links_and_emits() {
    let mut f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    let card = f.card(&board.id, "Detached").await;
    f.state
        .transaction(|state| {
            state.repos[0].hooks.prepare = vec!["hold".into()];
            Ok(())
        })
        .await
        .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(std::sync::Barrier::new(2));
    let signal = started.clone();
    let barrier = release.clone();
    f.shell.when(
        move |command| {
            if command.program == "sh" && command.args == ["-c", "hold"] {
                signal.notify_one();
                barrier.wait();
                true
            } else {
                false
            }
        },
        ShellResult {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
        },
    );
    f.reasons();
    let service = f.boards.clone();
    let id = card.id.clone();
    let request = tokio::spawn(async move {
        service
            .create_worktree_from_card(&id, Some("acme/api".parse().unwrap()), None, None)
            .await
    });
    let reached = tokio::time::timeout(Duration::from_secs(5), started.notified()).await;
    if reached.is_err() {
        request.abort();
        panic!("prepare hook did not start");
    }
    request.abort();
    tokio::task::spawn_blocking(move || release.wait())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(f.receiver.recv().await.unwrap(), Event::BoardChanged { board_id, reason: BoardChangeReason::CardChanged } if board_id == board.id) {
                break;
            }
        }
    })
    .await
    .unwrap();
    let linked = &f.boards.get(&board.id).await.unwrap().cards[0];
    assert_eq!(linked.status_id.as_str(), "in-progress");
    assert!(linked.worktree_id.is_some());
}

#[tokio::test]
async fn failed_post_push_refresh_blocks_edits_across_restart_until_sync_recovers() {
    let f = Fixture::new(all_caps()).await;
    let board = f.remote().await.board;
    let mut remote = remote("EXT-1", "Initial", "unused");
    remote.version = None;
    remote.updated_at = Some("2026-09-06T12:00:00Z".into());
    f.pull(vec![remote.clone()], None);
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let card = f.boards.get(&board.id).await.unwrap().cards.remove(0);
    f.boards
        .update_card(&card.id, title_patch("Pushed"))
        .await
        .unwrap();
    f.pull(vec![remote.clone()], None);
    f.backend
        .pull_responses
        .lock()
        .unwrap()
        .push_back(Err(BoardError::Backend("offline after push".into())));
    f.backend
        .push_responses
        .lock()
        .unwrap()
        .push_back(Ok(PushResult {
            acks: vec![PushAck {
                card_id: card.id.clone(),
                key: "EXT-1".into(),
                url: None,
                version: None,
                remote_updated_at: None,
                comment_ids: vec![],
            }],
            failures: vec![],
        }));
    assert!(matches!(
        f.sync(&board.id).await.status,
        JobStatus::Failed { .. }
    ));
    assert!(matches!(
        f.reopened()
            .update_card(&card.id, title_patch("Too soon"))
            .await,
        Err(DaemonError::Conflict(_))
    ));
    remote.title = "Pushed".into();
    remote.updated_at = Some("2026-09-06T12:01:00Z".into());
    f.pull(vec![remote], None);
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    assert!(
        f.boards
            .update_card(&card.id, title_patch("Next"))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn an_unreadable_document_hides_only_its_own_board() {
    let f = Fixture::new(BackendCapabilities::default()).await;
    let board = f.local().await.board;
    let card = f.card(&board.id, "Task").await;
    // A document a newer build wrote is intact, so the store reports it rather than
    // quarantining it. Sorted before `work`, it used to fail every scan of the boards
    // directory and leave a dead Board tab for every context in the home.
    std::fs::write(
        f.home.boards_dir().join("beta.json"),
        serde_json::json!({"version": 99, "board": {}, "cards": []}).to_string(),
    )
    .unwrap();
    let context: fleet_core::ids::ContextId = "work".parse().unwrap();
    assert_eq!(f.boards.ensure(&context).await.unwrap().board.id, board.id);
    assert_eq!(f.boards.list(None).await.unwrap().len(), 1);
    assert_eq!(
        f.boards
            .update_card(&card.id, title_patch("Renamed"))
            .await
            .unwrap()
            .title,
        "Renamed"
    );
    f.state
        .transaction(|state| {
            state.contexts.push(Context {
                id: "personal".parse().unwrap(),
                name: "Personal".into(),
                owners: vec![],
                created_at: "2026-09-06T12:00:00Z".into(),
            });
            Ok(())
        })
        .await
        .unwrap();
    f.boards
        .create(&"personal".parse().unwrap(), None, None, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_context_stays_deletable_when_its_board_document_is_unreadable() {
    let f = Fixture::new(BackendCapabilities::default()).await;
    let board = f.local().await.board;
    std::fs::write(
        f.home.board_path(&board.id),
        serde_json::json!({"version": 99, "board": {}, "cards": []}).to_string(),
    )
    .unwrap();
    // `DeleteContext` has already cascaded the context's repositories by the time it reaches
    // the board: refusing here leaves a context nobody can delete and repositories that are
    // already gone. The document goes to trash unread, exactly as the warning promises.
    f.boards
        .delete_for_context(&"work".parse().unwrap())
        .await
        .unwrap();
    assert!(!f.home.board_path(&board.id).exists());
    assert!(f.store.list().unwrap().is_empty());
}

#[tokio::test]
async fn taking_a_remote_parent_never_closes_a_cycle() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    f.pull(
        vec![remote("EXT-1", "One", "v1"), remote("EXT-2", "Two", "v1")],
        None,
    );
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let cards = f.boards.get(&board.id).await.unwrap().cards;
    let one = cards
        .iter()
        .find(|card| card.title == "One")
        .unwrap()
        .clone();
    let two = cards
        .iter()
        .find(|card| card.title == "Two")
        .unwrap()
        .clone();
    let mut child = remote("EXT-1", "One", "v2");
    child.parent_key = Some("EXT-2".into());
    f.pull(vec![child], None);
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    f.boards
        .update_card(&two.id, title_patch("Local two"))
        .await
        .unwrap();
    let mut parent = remote("EXT-2", "Two v2", "v2");
    parent.parent_key = Some("EXT-1".into());
    f.pull(vec![parent], None);
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let taken = f
        .boards
        .resolve_conflict(&two.id, ConflictResolution::TakeRemote)
        .await
        .unwrap();
    // `UpdateCard` refuses this edge, and the contract forbids the cycle whatever writes it.
    assert_eq!(taken.title, "Two v2");
    assert_eq!(taken.parent_id, None);
    let persisted = f.boards.get(&board.id).await.unwrap().cards;
    assert_eq!(
        persisted
            .iter()
            .find(|card| card.id == one.id)
            .unwrap()
            .parent_id
            .as_ref(),
        Some(&two.id)
    );
}

#[tokio::test]
async fn a_card_never_trades_the_live_worktree_it_already_owns() {
    let f = Fixture::new(BackendCapabilities::default()).await;
    let board = f.local().await.board;
    f.state
        .transaction(|state| {
            let mut repo = state.repos[0].clone();
            repo.id = "acme/web".parse().unwrap();
            repo.name = "web".into();
            state.repos.push(repo);
            Ok(())
        })
        .await
        .unwrap();
    let card = f.card(&board.id, "Switch").await;
    let (linked, worktree, created) = f
        .boards
        .create_worktree_from_card(&card.id, Some("acme/api".parse().unwrap()), None, None)
        .await
        .unwrap();
    assert!(created);
    assert_eq!(linked.worktree_id.as_ref(), Some(&worktree.id));
    // A second worktree in another repository strands the first: the card's link and
    // `repo_id` are rewritten, and nothing on the board can reach the live one again.
    assert!(matches!(
        f.boards
            .create_worktree_from_card(&card.id, Some("acme/web".parse().unwrap()), None, None)
            .await,
        Err(DaemonError::Conflict(_))
    ));
    let after = f.boards.get(&board.id).await.unwrap().cards;
    assert_eq!(after[0].worktree_id.as_ref(), Some(&worktree.id));
    assert_eq!(f.state.load().await.unwrap().worktrees.len(), 1);
}

#[tokio::test]
async fn a_patched_status_appends_the_card_to_its_new_column() {
    let f = Fixture::new(BackendCapabilities::default()).await;
    let board = f.local().await.board;
    let first = f.card(&board.id, "First").await;
    let second = f.card(&board.id, "Second").await;
    let done: fleet_core::ids::StatusId = "done".parse().unwrap();
    f.boards.move_card(&second.id, &done, None).await.unwrap();
    // Only `move_card` renumbers a column. A status set by a patch used to keep the position
    // it held in the column it left, interleaving it with cards it never met.
    let moved = f
        .boards
        .update_card(
            &first.id,
            CardPatch {
                status_id: Some(done.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(moved.status_id, done);
    let cards = f.boards.get(&board.id).await.unwrap().cards;
    let column = column_cards(&cards, &done);
    assert_eq!(
        column
            .iter()
            .map(|card| card.title.as_str())
            .collect::<Vec<_>>(),
        ["Second", "First"]
    );
    assert!(column[1].position > column[0].position);
}

#[tokio::test]
async fn a_board_pointing_at_an_unregistered_backend_refuses_sync_without_a_job() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    let mut doc = f.store.load(&board.id).unwrap().unwrap();
    doc.board.backend.kind = "ghost".into();
    f.store.save(&doc).unwrap();
    // `create` and `update` refuse an unregistered backend up front; sync used to answer with
    // a job id and fail minutes later under a kind the CLI reports as `unknown`.
    let error = f.boards.sync(&board.id, false).await.unwrap_err();
    assert!(matches!(error, DaemonError::Validation(_)), "{error}");
    assert!(error.to_string().contains("ghost"), "{error}");
    assert!(f.jobs.list().is_empty());
}

#[tokio::test]
async fn a_damaged_document_is_reported_instead_of_replaced_by_an_empty_board() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    f.card(&board.id, "Keep me").await;
    std::fs::write(f.home.board_path(&board.id), "not json").unwrap();
    let context = "work".parse().unwrap();
    // The scan behind `ensure` steps around the damaged file; the direct load reports it.
    assert!(matches!(
        f.boards.ensure(&context).await,
        Err(DaemonError::Validation(_))
    ));
    // That load quarantined it. Creating a board over the remains of one the user has not
    // been told about would report success for an empty board.
    assert_eq!(f.store.quarantined(&board.id).unwrap().len(), 1);
    assert!(matches!(
        f.boards.ensure(&context).await,
        Err(DaemonError::Conflict(_))
    ));
    assert!(matches!(
        f.boards.create(&context, None, None, None).await,
        Err(DaemonError::Conflict(_))
    ));
    assert!(f.boards.summaries().await.is_empty());
    // Deleting the context takes the remains with it, so a new one is not refused forever.
    f.boards.delete_for_context(&context).await.unwrap();
    assert!(f.store.quarantined(&board.id).unwrap().is_empty());
    assert_eq!(f.boards.ensure(&context).await.unwrap().cards.len(), 0);
}

#[tokio::test]
async fn an_archived_card_is_refused_by_the_patch_path_as_well_as_by_move() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    let card = f.card(&board.id, "Old news").await;
    f.boards
        .update_card(
            &card.id,
            CardPatch {
                archived: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let done: fleet_core::ids::StatusId = "done".parse().unwrap();
    assert!(
        f.boards
            .move_card(&card.id, &done, Some(0))
            .await
            .unwrap_err()
            .to_string()
            .contains("archived")
    );
    assert!(
        f.boards
            .update_card(
                &card.id,
                CardPatch {
                    status_id: Some(done),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("archived")
    );
    let stored = &f.boards.get(&board.id).await.unwrap().cards[0];
    assert!(stored.archived);
    assert_eq!(stored.status_id.as_str(), "todo");
    // Unarchiving and moving in one patch is still a move of a card that ends up in a column.
    let restored = f
        .boards
        .update_card(
            &card.id,
            CardPatch {
                archived: Some(false),
                status_id: Some("done".parse().unwrap()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!restored.archived);
    assert_eq!(restored.status_id.as_str(), "done");
}

#[tokio::test]
async fn a_card_keeps_the_repository_of_its_worktree_and_never_shows_a_deleted_one() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    let card = f.card(&board.id, "Fix login").await;
    let (linked, _, _) = tokio::time::timeout(
        Duration::from_secs(5),
        f.boards.create_worktree_from_card(
            &card.id,
            Some("acme/api".parse().unwrap()),
            Some("origin/main".into()),
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(linked.worktree_id.is_some());
    // Clearing the repository would leave `Repo: —` beside a live worktree, and every later
    // `card worktree` would refuse both the missing repository and every other one.
    assert!(matches!(
        f.boards
            .update_card(
                &card.id,
                CardPatch {
                    repo_id: Some(None),
                    ..Default::default()
                },
            )
            .await,
        Err(DaemonError::Conflict(_))
    ));
    // Pointing it at a different repository is the same wound with a name on it: the worktree
    // and the repository would then live in different repositories, `card worktree` would
    // refuse the mismatch, and clearing is refused above — the card could reach neither.
    let second = {
        let mut repo = f.state.load().await.unwrap().repos[0].clone();
        repo.id = "acme/web".parse().unwrap();
        repo.name = "web".into();
        repo
    };
    f.state
        .transaction(|state| {
            state.repos.push(second);
            Ok(())
        })
        .await
        .unwrap();
    assert!(matches!(
        f.boards
            .update_card(
                &card.id,
                CardPatch {
                    repo_id: Some(Some("acme/web".parse().unwrap())),
                    ..Default::default()
                },
            )
            .await,
        Err(DaemonError::Conflict(_))
    ));
    // The worktree's own repository is the one value that still passes.
    assert_eq!(
        f.boards
            .update_card(
                &card.id,
                CardPatch {
                    repo_id: Some(Some("acme/api".parse().unwrap())),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .repo_id,
        Some("acme/api".parse().unwrap())
    );
    f.boards
        .update(
            &board.id,
            BoardPatch {
                default_repo_id: Some(Some("acme/api".parse().unwrap())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    f.state
        .transaction(|state| {
            state.worktrees.clear();
            state.repos.clear();
            Ok(())
        })
        .await
        .unwrap();
    // A repository the state no longer has is dropped from the view exactly like a worktree.
    let view = f.boards.get(&board.id).await.unwrap();
    assert_eq!(view.cards[0].repo_id, None);
    assert_eq!(view.cards[0].worktree_id, None);
    assert_eq!(view.board.default_repo_id, None);
    // And the card asks for one instead of naming a repository nothing on the board shows.
    assert!(matches!(
        f.boards
            .create_worktree_from_card(&card.id, None, None, None)
            .await,
        Err(DaemonError::Validation(_))
    ));
}

#[tokio::test]
async fn a_move_that_changes_nothing_writes_no_document_and_no_history() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    let card = f.card(&board.id, "Stay put").await;
    let before = f.boards.get(&board.id).await.unwrap();
    f.clock
        .set(Utc.with_ymd_and_hms(2026, 9, 6, 14, 0, 0).single().unwrap());
    for _ in 0..3 {
        let moved = f
            .boards
            .move_card(&card.id, &card.status_id, Some(0))
            .await
            .unwrap();
        assert_eq!(moved.updated_at, card.updated_at);
        assert_eq!(moved.activity.len(), card.activity.len());
    }
    let after = f.boards.get(&board.id).await.unwrap();
    assert_eq!(after.board.updated_at, before.board.updated_at);
    assert_eq!(after.cards, before.cards);
}

#[tokio::test]
async fn memoized_summaries_follow_every_write_that_changes_them() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.local().await.board;
    assert_eq!(f.boards.summaries().await[0].card_count, 0);
    let card = f.card(&board.id, "One").await;
    assert_eq!(f.boards.summaries().await[0].card_count, 1);
    // Twice: the second answer comes from the memo and must say the same thing.
    assert_eq!(f.boards.summaries().await[0].card_count, 1);
    f.boards
        .update_card(&card.id, title_patch("Renamed"))
        .await
        .unwrap();
    assert_eq!(f.boards.summaries().await[0].name, board.name);
    f.boards.delete_card(&card.id).await.unwrap();
    assert_eq!(f.boards.summaries().await[0].card_count, 0);
    f.boards.delete(&board.id).await.unwrap();
    assert!(f.boards.summaries().await.is_empty());
}

/// A key the *backend* could not read is the backend's own business: it has already answered
/// with the cursor it wants kept — holding the watermark while still advancing whatever counts
/// pulls towards the next full one. Rewinding the whole opaque string on top of that undid the
/// second half, so one permanently unreadable key froze the counter and no full pull ever came
/// due again: remote deletions stopped being noticed while the window grew without bound.
#[tokio::test]
async fn a_key_the_backend_could_not_read_keeps_the_cursor_the_backend_chose() {
    let mut f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    script(
        &f,
        schema(),
        PullResult {
            cards: vec![remote("EXT-1", "Task", "v1")],
            cursor: Some("watermark|4".into()),
            failed_keys: vec!["EXT-9: field is not readable".into()],
            ..Default::default()
        },
    );
    let job = f.sync(&board.id).await;
    assert_eq!(job.status, JobStatus::Succeeded);
    let saved = f.store.load(&board.id).unwrap().unwrap();
    assert_eq!(saved.board.sync.cursor.as_deref(), Some("watermark|4"));
    // The key is still reported, on the surface every board's trouble is read from.
    let error = saved.board.sync.last_error.unwrap();
    assert!(error.contains("EXT-9"), "{error}");
    f.reasons();

    // A card *this board* could not import is the other half, and it still rewinds: nothing
    // else can offer that issue again.
    let mut invalid = remote("EXT-8", "Broken", "v1");
    invalid.due_date = Some("not-a-date".into());
    script(
        &f,
        schema(),
        pulled(
            vec![remote("EXT-1", "Edited", "v2"), invalid],
            "watermark|5",
        ),
    );
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    assert_eq!(
        f.store
            .load(&board.id)
            .unwrap()
            .unwrap()
            .board
            .sync
            .cursor
            .as_deref(),
        Some("watermark|4")
    );
}

/// A project gains a status between two syncs, and appending it put a *started* column to the
/// right of the completed one. A category the remote declared is an order the board can read.
#[tokio::test]
async fn a_status_the_pull_introduces_lands_in_its_own_category_s_place() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    let mut done = schema();
    done.statuses.push(RemoteStatus {
        id: "done-id".into(),
        name: "Done".into(),
        category: Some(StatusCategory::Completed),
    });
    script(&f, done.clone(), pulled(vec![], "cursor-0"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);

    let mut reviewing = remote("EXT-3", "In review", "v1");
    reviewing.status = RemoteStatus {
        id: "review-id".into(),
        name: "In Review".into(),
        category: Some(StatusCategory::Started),
    };
    script(&f, done, pulled(vec![reviewing], "cursor-1"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    assert_eq!(
        f.boards
            .get(&board.id)
            .await
            .unwrap()
            .board
            .statuses
            .iter()
            .map(|status| status.name.clone())
            .collect::<Vec<_>>(),
        ["Open", "Doing", "In Review", "Done"]
    );
}

/// Scripts one describe/pull pair for the next sync.
fn script(f: &Fixture, schema: BackendSchema, pull: PullResult) {
    f.backend
        .describe_responses
        .lock()
        .unwrap()
        .push_back(Ok(schema));
    f.backend.pull_responses.lock().unwrap().push_back(Ok(pull));
}

fn pulled(cards: Vec<RemoteCard>, cursor: &str) -> PullResult {
    PullResult {
        cards,
        cursor: Some(cursor.into()),
        ..Default::default()
    }
}

async fn full_sync(f: &Fixture, board: &BoardId) -> fleet_proto::job::JobRecord {
    let id = f.boards.sync(board, true).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), f.jobs.wait(&id))
        .await
        .unwrap()
        .unwrap()
}

fn cursors(f: &Fixture) -> Vec<Option<String>> {
    f.backend
        .calls()
        .iter()
        .filter_map(|call| match call {
            FakeBackendCall::Pull(_, cursor) => Some(cursor.clone()),
            _ => None,
        })
        .collect()
}

fn stored_cursor(f: &Fixture, board: &BoardId) -> Option<String> {
    f.store.load(board).unwrap().unwrap().board.sync.cursor
}

#[tokio::test]
async fn a_pulled_status_the_description_never_named_becomes_a_column() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    // A sampled `describe` never saw this one, and its name matches no category keyword: today
    // the card would land in whatever column reconcile fell back to, with "unmapped status" on
    // its activity and on the job's summary line.
    let escalated = RemoteStatus {
        id: "escalated-id".into(),
        name: "Escalated".into(),
        category: None,
    };
    let mut hot = remote("EXT-1", "Escalated task", "v1");
    hot.status = escalated;
    script(
        &f,
        schema(),
        pulled(vec![hot, remote("EXT-2", "Open task", "v1")], "cursor-1"),
    );
    let job = f.sync(&board.id).await;
    assert_eq!(job.status, JobStatus::Succeeded);
    let view = f.boards.get(&board.id).await.unwrap();
    assert_eq!(
        view.board
            .statuses
            .iter()
            .map(|status| (status.id.as_str(), status.name.as_str()))
            .collect::<Vec<_>>(),
        [
            ("open", "Open"),
            ("doing", "Doing"),
            ("escalated", "Escalated")
        ]
    );
    assert_eq!(view.board.statuses[2].category, StatusCategory::Unstarted);
    let card = view
        .cards
        .iter()
        .find(|card| card.title == "Escalated task")
        .unwrap();
    assert_eq!(card.status_id.as_str(), "escalated");
    assert!(
        !card
            .activity
            .iter()
            .any(|entry| entry.message.contains("unmapped")),
        "{:?}",
        card.activity
    );
    // The re-adoption extends the map instead of replacing it: everything `describe` named
    // still points at the column it was mapped to, and the card holding it did not move.
    let map = &view.board.sync.status_map.remote_to_local;
    assert_eq!(map["open-id"].as_str(), "open");
    assert_eq!(map["doing-id"].as_str(), "doing");
    assert_eq!(map["escalated-id"].as_str(), "escalated");
    assert_eq!(
        view.board.sync.status_map.local_to_remote[&"escalated".parse().unwrap()],
        "escalated-id"
    );
    assert_eq!(
        view.cards
            .iter()
            .find(|card| card.title == "Open task")
            .unwrap()
            .status_id
            .as_str(),
        "open"
    );
    let log = f.jobs.tail(&job.id, 40).await.unwrap().join("\n");
    assert!(log.contains("adopting newly seen remote statuses"), "{log}");
    assert!(!log.contains("escalated-id"), "{log}");
    // A second sync of the same statuses adopts nothing again and adds no second column.
    script(&f, schema(), pulled(vec![], "cursor-2"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let view = f.boards.get(&board.id).await.unwrap();
    assert_eq!(view.board.statuses.len(), 3);
}

/// A create is a write like any other: a field the backend cannot carry is dropped by its create
/// payload, acked as pushed, and then overwritten by the next pull — three typed values gone.
#[tokio::test]
async fn a_new_card_is_refused_a_field_the_backend_cannot_write() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    script(
        &f,
        BackendSchema {
            readonly_fields: vec!["priority".into(), "due_date".into()],
            ..schema()
        },
        pulled(vec![remote("EXT-1", "Task", "v1")], "cursor-1"),
    );
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let error = f
        .boards
        .create_card(
            &board.id,
            CardDraft {
                title: "Card born urgent".into(),
                priority: Priority::Urgent,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    // The same sentence `card edit --priority` already answers with.
    assert!(
        matches!(&error, DaemonError::Validation(message)
            if message == "priority is read-only on this board's backend"),
        "{error:?}"
    );
    assert!(matches!(
        f.boards
            .create_card(
                &board.id,
                CardDraft {
                    title: "Card born late".into(),
                    due_date: Some("2026-09-30".into()),
                    ..Default::default()
                },
            )
            .await,
        Err(DaemonError::Validation(_))
    ));
    // A card that sets none of them is created exactly as before.
    let card = f
        .boards
        .create_card(
            &board.id,
            CardDraft {
                title: "Plain card".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(card.title, "Plain card");
}

#[tokio::test]
async fn read_only_backend_fields_are_refused_with_the_daemon_message_intact() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    script(
        &f,
        BackendSchema {
            readonly_fields: vec!["priority".into(), "status_id".into()],
            ..schema()
        },
        pulled(vec![remote("EXT-1", "Task", "v1")], "cursor-1"),
    );
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    let view = f.boards.get(&board.id).await.unwrap();
    assert_eq!(view.board.sync.readonly_fields, ["priority", "status_id"]);
    let card = view.cards[0].clone();
    let error = f
        .boards
        .update_card(
            &card.id,
            CardPatch {
                priority: Some(Priority::Urgent),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    // The CLI and the app print this verbatim, so the message must survive the mapping whole.
    assert!(
        matches!(&error, DaemonError::Validation(message)
            if message == "priority is read-only on this board's backend"),
        "{error:?}"
    );
    // A field the backend can write is untouched by the refusal.
    assert_eq!(
        f.boards
            .update_card(&card.id, title_patch("Renamed"))
            .await
            .unwrap()
            .title,
        "Renamed"
    );
    let error = f
        .boards
        .move_card(&card.id, &"doing".parse().unwrap(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, DaemonError::Validation(message)
            if message == "status_id is read-only on this board's backend"),
        "{error:?}"
    );
    // Reordering inside the column the card already sits in is no transition and still works.
    f.boards
        .move_card(&card.id, &card.status_id, Some(0))
        .await
        .unwrap();
    let after = f.boards.get(&board.id).await.unwrap();
    assert_eq!(after.cards[0].priority, Priority::None);
    assert_eq!(after.cards[0].status_id, card.status_id);
    assert_eq!(after.cards[0].title, "Renamed");
}

#[tokio::test]
async fn deleting_a_parent_clears_children_even_when_parents_are_read_only() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    let parent = f.card(&board.id, "Parent").await;
    let child = f.card(&board.id, "Child").await;
    f.boards
        .update_card(
            &child.id,
            CardPatch {
                parent_id: Some(Some(parent.id.clone())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    script(
        &f,
        BackendSchema {
            readonly_fields: vec!["parent_id".into()],
            ..schema()
        },
        pulled(vec![], "cursor-1"),
    );
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    // A child left pointing at a card the document no longer holds fails `validate_card`, so
    // the read-only field must not be able to refuse the clearing the deletion requires.
    f.boards.delete_card(&parent.id).await.unwrap();
    let view = f.boards.get(&board.id).await.unwrap();
    assert_eq!(view.cards.len(), 1);
    assert_eq!(view.cards[0].parent_id, None);
    assert!(
        view.cards[0]
            .activity
            .iter()
            .any(|entry| entry.message.contains("parent_id")),
        "{:?}",
        view.cards[0].activity
    );
}

#[tokio::test]
async fn a_full_sync_clears_the_cursor_before_it_pulls_without_one() {
    let f = Fixture::new(pull_caps()).await;
    let board = f.remote().await.board;
    f.pull(vec![remote("EXT-1", "Task", "v1")], Some("cursor-1"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    assert_eq!(stored_cursor(&f, &board.id).as_deref(), Some("cursor-1"));
    f.pull(vec![remote("EXT-1", "Task", "v1")], Some("cursor-2"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    f.pull(vec![remote("EXT-1", "Task", "v1")], Some("cursor-3"));
    assert_eq!(full_sync(&f, &board.id).await.status, JobStatus::Succeeded);
    assert_eq!(
        cursors(&f),
        [None, Some("cursor-1".into()), None],
        "a full sync pulls without the cursor it just cleared"
    );
    assert_eq!(stored_cursor(&f, &board.id).as_deref(), Some("cursor-3"));
    // The clearing is committed before the job runs, so a full sync that fails halfway still
    // leaves the board asking for a full one — never for an increment since a lost watermark.
    f.backend
        .describe_responses
        .lock()
        .unwrap()
        .push_back(Ok(schema()));
    f.backend
        .pull_responses
        .lock()
        .unwrap()
        .push_back(Err(BoardError::Backend("remote unavailable".into())));
    assert!(matches!(
        full_sync(&f, &board.id).await.status,
        JobStatus::Failed { .. }
    ));
    assert_eq!(stored_cursor(&f, &board.id), None);
    assert!(
        f.store
            .load(&board.id)
            .unwrap()
            .unwrap()
            .board
            .sync
            .last_error
            .is_some()
    );
}

#[tokio::test]
async fn a_backend_kind_change_takes_its_settings_from_the_patch_alone() {
    let f = Fixture::new(pull_caps()).await;
    assert_eq!(
        f.boards
            .list_backends()
            .iter()
            .map(|descriptor| (descriptor.kind.as_str(), descriptor.label.as_str()))
            .collect::<Vec<_>>(),
        [("local", "Local"), ("fake", "Fake")]
    );
    let board = f.remote().await.board;
    f.pull(vec![], Some("cursor-1"));
    assert_eq!(f.sync(&board.id).await.status, JobStatus::Succeeded);
    assert_ne!(
        f.boards.get(&board.id).await.unwrap().board.sync,
        SyncState::default()
    );
    // Nothing links to the fake backend, so the kind may still change — and everything it left
    // behind goes with it: its settings, its cursor and the status map it built.
    let view = f
        .boards
        .update(
            &board.id,
            BoardPatch {
                backend: Some(BackendRef::default()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(view.board.backend, BackendRef::default());
    assert_eq!(view.board.backend.settings, serde_json::Value::Null);
    assert_eq!(view.board.sync, SyncState::default());
    // And back the other way: the new kind is described by the patch and by nothing else.
    let view = f
        .boards
        .update(
            &board.id,
            BoardPatch {
                backend: Some(BackendRef {
                    kind: "fake".into(),
                    settings: serde_json::json!({"project": "OTHER"}),
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        view.board.backend.settings,
        serde_json::json!({"project": "OTHER"})
    );
    assert!(
        f.backend.calls().iter().any(|call| matches!(call,
            FakeBackendCall::Validate(settings) if *settings == serde_json::json!({"project": "OTHER"}))),
        "the new kind validates the settings the patch named"
    );
}
