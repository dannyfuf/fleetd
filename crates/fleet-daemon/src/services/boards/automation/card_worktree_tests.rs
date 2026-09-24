//! Card-worktree runs: the start step that creates or adopts a card's pull-request worktree, links
//! it, and records it on the run.
//!
//! The world is real Git — an origin that carries `refs/pull/7/head` and `refs/pull/8/head`, and a
//! clone of it registered as `acme/api` — with `gh pr view` answered from a fixture, so worktree
//! creation runs its production path without the network. No provider ever starts: the Claude
//! binary is configured as a command line that cannot be parsed, so every run that reaches the
//! delegation service is refused there, before a token is minted or a process spawned, and is
//! recorded as a failed start that still names its worktree.

use std::{path::Path, process::Command, sync::Arc};

use fleet_core::{
    board::{
        Action, ActionKind, ActivityKind, Board, BoardDocument, Card, ColumnAgentPrefs,
        ColumnAutomation, RunLocation,
    },
    ids::{CardId, RepoId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    paths::FleetHome,
    state::default_state,
};

use super::{RunRow, card_request, on_enter};
use crate::{
    adapters::{
        Adapters,
        board::BoardBackends,
        clock::{Clock, SystemClock},
        files::RealFiles,
        git::ShellGit,
        process::RealProcess,
        shell::{RealShell, Shell, ShellResult},
    },
    jobs::JobManager,
    server::BroadcastBus,
    services::Services,
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::{FakeGithub, FakeShell},
};

/// The pull request every test card reviews unless it says otherwise.
const FIRST_PULL: u64 = 7;
/// A second pull request of the same repository, for two cards at once.
const SECOND_PULL: u64 = 8;

/// A private daemon's services over a real repository, and the directory that holds both.
struct World {
    services: Services,
    _root: tempfile::TempDir,
}

impl World {
    async fn new() -> Self {
        let root = tempfile::tempdir().expect("a temporary directory");
        let home = root.path().join("fleet");
        let repo_path = create_repository(root.path(), &home);
        let fleet_home = FleetHome::new(&home);
        let files = Arc::new(RealFiles::new(
            fleet_home.trash_dir(),
            [
                fleet_home.boards_dir(),
                home.join("repos"),
                home.join("worktrees"),
            ],
        ));
        let config = Arc::new(ConfigStore::new(&home, files.clone()));
        config
            .update(serde_json::json!({
                "reposDir": home.join("repos"),
                "worktreesDir": home.join("worktrees"),
                "hotPoolSize": 0,
                "hotRefreshIntervalMs": 0,
                // Unparsable on purpose: the delegation service refuses it before anything runs.
                "agentBinaries": {"claude": "'unterminated", "codex": "'unterminated"},
            }))
            .await
            .expect("the test configuration is valid");
        let state = Arc::new(StateStore::new(&home, files.clone(), Arc::new(SystemClock)));
        let mut initial = default_state();
        initial.contexts.push(Context {
            id: "work".parse().expect("a static context slug"),
            name: "Work".into(),
            owners: vec![],
            created_at: "now".into(),
        });
        initial.repos.push(Repo {
            id: repo_id(),
            owner: "acme".into(),
            name: "api".into(),
            url: "unused".into(),
            context_id: "work".parse().expect("a static context slug"),
            default_branch: "main".into(),
            path: repo_path.to_string_lossy().into_owned(),
            cloned_at: "now".into(),
            hooks: RepoHooks::default(),
        });
        state.save(initial).await.expect("the initial state saves");
        let services = Services::build(
            home.clone(),
            config,
            state,
            Arc::new(JobManager::new(&home)),
            adapters(files),
            BroadcastBus::default(),
        );
        Self {
            services,
            _root: root,
        }
    }

    /// The context board, turned into a card-worktree board whose `todo` column runs a prompt.
    async fn board(&self, max_live_runs: Option<u32>, cards: Vec<Card>) -> Board {
        let view = self
            .services
            .boards
            .ensure(&"work".parse().expect("a static context slug"))
            .await
            .expect("the context board");
        let mut board = view.board;
        board.settings.run_location = RunLocation::CardWorktree;
        board.settings.max_live_runs = max_live_runs;
        for status in &mut board.statuses {
            if status.id.as_str() == "todo" {
                status.automation = Some(ColumnAutomation {
                    on_enter: Some(Action {
                        kind: ActionKind::Prompt,
                        instructions: "Review {key}".into(),
                        expect: "a review".into(),
                        agent: ColumnAgentPrefs::default(),
                        env: Vec::new(),
                    }),
                    on_success: None,
                    advance_when_unblocked: None,
                });
            }
        }
        self.write(&board, cards);
        board
    }

    fn write(&self, board: &Board, cards: Vec<Card>) {
        let doc = BoardDocument {
            version: fleet_core::board::document_version(board, &cards),
            board: board.clone(),
            cards,
        };
        self.services
            .boards
            .store
            .save(&doc)
            .expect("the board document saves");
    }

    fn document(&self, board: &Board) -> BoardDocument {
        self.services
            .boards
            .load(&board.id)
            .expect("the board document loads")
    }

    /// The seed checkout the origin's pull-request heads are pushed from.
    fn seed(&self) -> std::path::PathBuf {
        self._root.path().join("seed")
    }

    async fn worktrees(&self) -> Vec<Worktree> {
        self.services
            .state
            .load()
            .await
            .expect("the state loads")
            .worktrees
    }
}

/// Real Git, filesystem and shell, with `gh pr view <n>` answered for pull requests 7 and 8.
fn adapters(files: Arc<RealFiles>) -> Adapters {
    let gh = Arc::new(FakeShell::new());
    for number in [FIRST_PULL, SECOND_PULL] {
        gh.when(
            move |command| {
                command.program == "gh" && command.args.get(2) == Some(&number.to_string())
            },
            ShellResult {
                status: 0,
                stdout: serde_json::json!({
                    "number": number, "title": format!("Change {number}"),
                    "url": format!("https://github.com/acme/api/pull/{number}"),
                    "author": {"login": "octocat"}, "headRefName": format!("review-{number}"),
                    "baseRefName": "main", "isDraft": false, "isCrossRepository": false,
                    "headRepository": {"name": "api", "nameWithOwner": "acme/api"},
                    "headRepositoryOwner": {"login": "acme"}, "reviewDecision": null,
                    "statusCheckRollup": [], "additions": 1, "deletions": 0, "labels": [],
                    "updatedAt": "2026-09-22T00:00:00Z",
                })
                .to_string(),
                stderr: String::new(),
            },
        );
    }
    let shell: Arc<dyn Shell> = Arc::new(RealShell);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    Adapters {
        board_backends: BoardBackends::system(Arc::clone(&shell), Arc::clone(&clock)),
        git: Arc::new(ShellGit::new(Arc::clone(&shell))),
        github: Arc::new(FakeGithub::new(gh)),
        process: Arc::new(RealProcess::new(Arc::clone(&shell))),
        files,
        shell,
        clock,
    }
}

/// An origin with `main` and two pull-request heads, cloned where `acme/api` is registered.
fn create_repository(root: &Path, home: &Path) -> std::path::PathBuf {
    let remote = root.join("remote.git");
    let seed = root.join("seed");
    let base = home.join("repos/acme/api");
    let remote_arg = remote.to_string_lossy().into_owned();
    git(root, &["init", "--bare", &remote_arg]);
    git(root, &["init", &seed.to_string_lossy()]);
    git(&seed, &["config", "user.email", "fleet@example.test"]);
    git(&seed, &["config", "user.name", "Fleet Test"]);
    std::fs::write(seed.join("README.md"), "fleet\n").expect("the seed file writes");
    git(&seed, &["add", "README.md"]);
    git(&seed, &["commit", "-m", "initial"]);
    git(&seed, &["branch", "-M", "main"]);
    git(&seed, &["remote", "add", "origin", &remote_arg]);
    git(&seed, &["push", "-u", "origin", "main"]);
    for number in [FIRST_PULL, SECOND_PULL] {
        git(
            &seed,
            &["push", "origin", &format!("HEAD:refs/pull/{number}/head")],
        );
    }
    git(
        root,
        &[
            "--git-dir",
            &remote_arg,
            "symbolic-ref",
            "HEAD",
            "refs/heads/main",
        ],
    );
    git(root, &["clone", &remote_arg, &base.to_string_lossy()]);
    base
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repo_id() -> RepoId {
    "acme/api".parse().expect("a static repository id")
}

fn card_id(number: u32) -> CardId {
    format!("card-{number}").parse().expect("a static card id")
}

fn pull_worktree(number: u64) -> WorktreeId {
    format!("acme/api#review-{number}")
        .parse()
        .expect("a static worktree id")
}

/// A card standing in the running column, carrying whatever the test adds on top.
fn card(board_id: &str, number: u32, extra: serde_json::Value) -> Card {
    let mut value = serde_json::json!({
        "id": format!("card-{number}"),
        "boardId": board_id,
        "number": number,
        "title": format!("Review {number}"),
        "statusId": "todo",
        "createdAt": "2026-09-22T00:00:00Z",
        "updatedAt": "2026-09-22T00:00:00Z",
    });
    if let (Some(target), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
        for (key, item) in extra {
            target.insert(key.clone(), item.clone());
        }
    }
    serde_json::from_value(value).expect("a card from its wire fields")
}

/// The `pullRequest` field of a card reviewing `repo#number`.
fn reviewing(repo: &str, number: u64) -> serde_json::Value {
    serde_json::json!({
        "pullRequest": {
            "repo": repo,
            "number": number,
            "url": format!("https://github.com/{repo}/pull/{number}"),
        }
    })
}

/// The board id the context board gets, which the seeded cards must carry.
async fn board_id(world: &World) -> String {
    world
        .services
        .boards
        .ensure(&"work".parse().expect("a static context slug"))
        .await
        .expect("the context board")
        .board
        .id
        .to_string()
}

fn linked(card: &Card) -> Vec<&str> {
    card.activity
        .iter()
        .filter(|entry| entry.kind == ActivityKind::WorktreeCreated)
        .map(|entry| entry.message.as_str())
        .collect()
}

#[tokio::test]
async fn a_review_card_gets_its_pull_request_worktree_on_first_run() {
    let world = World::new().await;
    let id = board_id(&world).await;
    let board = world
        .board(None, vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))])
        .await;

    let started = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the start is recorded");

    let worktree = pull_worktree(FIRST_PULL);
    assert_eq!(started.worktree_id.as_ref(), Some(&worktree));
    assert_eq!(started.repo_id.as_ref(), Some(&repo_id()));
    assert_eq!(
        linked(&started),
        vec![format!("Linked worktree {worktree}")]
    );
    let created = world.worktrees().await;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].id, worktree);
    assert!(Path::new(&created[0].path).is_dir());
    assert_eq!(started.runs.len(), 1);
    assert_eq!(started.runs[0].worktree_id.as_ref(), Some(&worktree));

    // The request the start builds from that document runs in the card's worktree, not the
    // board's, which a context board does not even have.
    let doc = world.document(&board);
    let stored = &doc.cards[0];
    let action = on_enter(&doc.board, &stored.status_id).expect("the column runs an action");
    let key = stored.display_key(&doc.board);
    let request = card_request(
        &doc.board,
        stored,
        action,
        &key,
        &RunRow {
            status_id: stored.status_id.clone(),
            action: ActionKind::Prompt,
            provider: fleet_core::agents::AgentKind::Claude,
            model: None,
            effort: None,
        },
        fleet_core::agents::PermissionMode::default(),
    )
    .expect("a linked card builds its request");
    assert_eq!(request.worktree, worktree);
}

#[tokio::test]
async fn a_second_run_reuses_the_card_worktree() {
    let world = World::new().await;
    let id = board_id(&world).await;
    world
        .board(None, vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))])
        .await;

    world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the first start is recorded");
    let again = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the second start is recorded");

    let worktree = pull_worktree(FIRST_PULL);
    assert_eq!(linked(&again).len(), 1, "{:?}", again.activity);
    assert_eq!(world.worktrees().await.len(), 1);
    assert_eq!(again.runs.len(), 2);
    assert!(
        again
            .runs
            .iter()
            .all(|run| run.worktree_id.as_ref() == Some(&worktree))
    );
}

#[tokio::test]
async fn a_card_without_a_pull_request_or_worktree_records_the_refusal() {
    let world = World::new().await;
    let id = board_id(&world).await;
    let board = world
        .board(None, vec![card(&id, 1, serde_json::json!({}))])
        .await;

    let refused = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the refusal is recorded");

    let key = refused.display_key(&board);
    assert_eq!(refused.runs.len(), 1);
    assert!(refused.runs[0].failed_to_start());
    assert_eq!(
        refused.runs[0].detail.as_deref(),
        Some(
            format!(
                "validation failed: invalid automation: {key} has no worktree to run in; link a pull request or create its worktree first"
            )
            .as_str()
        )
    );
    assert_eq!(refused.runs[0].worktree_id, None);
    assert!(refused.worktree_id.is_none());
    assert!(world.worktrees().await.is_empty());
}

#[tokio::test]
async fn a_pull_request_in_an_unknown_repository_records_the_refusal() {
    let world = World::new().await;
    let id = board_id(&world).await;
    world
        .board(None, vec![card(&id, 1, reviewing("other/lib", 3))])
        .await;

    let refused = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the refusal is recorded");

    assert_eq!(refused.runs.len(), 1);
    assert_eq!(
        refused.runs[0].detail.as_deref(),
        Some(
            "validation failed: invalid automation: other/lib is not a Fleet repository; clone it into this context first"
        )
    );
    assert!(refused.worktree_id.is_none());
    assert!(world.worktrees().await.is_empty());
}

#[tokio::test]
async fn a_card_deleted_while_its_worktree_is_created_is_refused() {
    let world = World::new().await;
    let id = board_id(&world).await;
    let board = world
        .board(None, vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))])
        .await;
    let made: Worktree = serde_json::from_value(serde_json::json!({
        "id": pull_worktree(FIRST_PULL), "repoId": "acme/api", "slug": "review-7",
        "branch": "review-7", "baseRef": "pull/7/head", "path": "/tmp/acme-api-review-7",
        "session": "api/review-7", "createdAt": "now"
    }))
    .expect("a worktree from its wire fields");

    let boards = Arc::clone(&world.services.boards);
    let refused = world
        .services
        .boards
        .ensure_pull_request_worktree_with(&board.id, &card_id(1), move |repo, number| {
            async move {
                assert_eq!((repo, number), (repo_id(), FIRST_PULL));
                // The gate is dropped while the worktree is made, so the card can go meanwhile.
                boards
                    .delete_card(&card_id(1))
                    .await
                    .expect("the card deletes while its worktree is made");
                Ok((true, made))
            }
        })
        .await;

    match refused {
        Err(crate::DaemonError::Conflict(message)) => assert!(
            message.starts_with(&format!(
                "worktree {} was created, but its card is gone:",
                pull_worktree(FIRST_PULL)
            )),
            "{message}"
        ),
        other => panic!("expected the orphaned worktree to be named, got {other:?}"),
    }
    assert!(world.document(&board).cards.is_empty());
}

#[tokio::test]
async fn two_cards_run_at_once_in_two_worktrees() {
    let world = World::new().await;
    let id = board_id(&world).await;
    world
        .board(
            Some(2),
            vec![
                card(&id, 1, reviewing("acme/api", FIRST_PULL)),
                card(&id, 2, reviewing("acme/api", SECOND_PULL)),
            ],
        )
        .await;

    let (one, two) = (card_id(1), card_id(2));
    let (first, second) = tokio::join!(
        world.services.boards.start_run(&one),
        world.services.boards.start_run(&two),
    );
    let first = first.expect("the first start is recorded");
    let second = second.expect("the second start is recorded");

    assert_eq!(first.worktree_id.as_ref(), Some(&pull_worktree(FIRST_PULL)));
    assert_eq!(
        second.worktree_id.as_ref(),
        Some(&pull_worktree(SECOND_PULL))
    );
    assert_eq!(
        first.runs[0].worktree_id.as_ref(),
        Some(&pull_worktree(FIRST_PULL))
    );
    assert_eq!(
        second.runs[0].worktree_id.as_ref(),
        Some(&pull_worktree(SECOND_PULL))
    );
    let mut ids: Vec<WorktreeId> = world
        .worktrees()
        .await
        .into_iter()
        .map(|worktree| worktree.id)
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![pull_worktree(FIRST_PULL), pull_worktree(SECOND_PULL)]
    );
}

/// A board that runs every card in its own worktree records that one on each run, and never
/// creates a worktree for a card.
#[tokio::test]
async fn a_card_run_records_its_worktree() {
    let world = World::new().await;
    let id = board_id(&world).await;
    let mut board = world
        .board(None, vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))])
        .await;
    let (_created, worktree, _post_create_job) = world
        .services
        .worktrees
        .create(
            repo_id(),
            "feature".into(),
            None,
            None,
            RepoHooks::default(),
        )
        .await
        .expect("the board's worktree is created");
    board.settings.run_location = RunLocation::BoardWorktree;
    board.worktree_id = Some(worktree.id.clone());
    world.write(
        &board,
        vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))],
    );

    let ran = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the start is recorded");

    assert_eq!(ran.runs.len(), 1);
    assert_eq!(ran.runs[0].worktree_id.as_ref(), Some(&worktree.id));
    assert!(ran.worktree_id.is_none());
    assert!(linked(&ran).is_empty());
    assert_eq!(world.worktrees().await.len(), 1);
}

/// Pushes a new commit onto the origin's `refs/pull/<number>/head`, as a pull request's author
/// pushing after the review started.
fn push_to_pull(world: &World, number: u64, file: &str) -> String {
    let seed = world.seed();
    std::fs::write(seed.join(file), format!("{file}\n")).expect("the change writes");
    git(&seed, &["add", file]);
    git(&seed, &["commit", "-m", file]);
    git(
        &seed,
        &[
            "push",
            "-f",
            "origin",
            &format!("HEAD:refs/pull/{number}/head"),
        ],
    );
    head_of(&seed)
}

fn head_of(path: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// The path of the worktree the first pull request's card runs in.
async fn pull_path(world: &World) -> std::path::PathBuf {
    let id = pull_worktree(FIRST_PULL);
    let worktree = world
        .worktrees()
        .await
        .into_iter()
        .find(|worktree| worktree.id == id)
        .expect("the card's worktree exists");
    std::path::PathBuf::from(worktree.path)
}

/// What the local-changes refusal records on the run.
fn local_changes_refusal() -> String {
    "validation failed: invalid automation: The worktree for acme/api#7 has local changes; commit or discard them before the review runs.".to_owned()
}

#[tokio::test]
async fn a_second_run_fast_forwards_the_worktree_to_the_new_head() {
    let world = World::new().await;
    let id = board_id(&world).await;
    world
        .board(None, vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))])
        .await;
    world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the first start is recorded");
    let pushed = push_to_pull(&world, FIRST_PULL, "second.txt");

    let again = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the second start is recorded");

    assert_eq!(head_of(&pull_path(&world).await), pushed);
    assert_ne!(
        again.runs[1].detail.as_deref(),
        Some(local_changes_refusal().as_str())
    );
}

#[tokio::test]
async fn a_worktree_already_at_the_head_is_left_alone() {
    let world = World::new().await;
    let id = board_id(&world).await;
    world
        .board(None, vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))])
        .await;
    world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the first start is recorded");
    let path = pull_path(&world).await;
    let before = head_of(&path);
    // Uncommitted work on a checkout that is already current is not in the way of anything.
    std::fs::write(path.join("notes.txt"), "mine\n").expect("the local file writes");

    let again = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the second start is recorded");

    assert_eq!(head_of(&path), before);
    assert!(path.join("notes.txt").is_file());
    assert_ne!(
        again.runs[1].detail.as_deref(),
        Some(local_changes_refusal().as_str())
    );
}

#[tokio::test]
async fn a_dirty_worktree_refuses_the_run_and_is_left_as_it_was() {
    let world = World::new().await;
    let id = board_id(&world).await;
    world
        .board(None, vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))])
        .await;
    world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the first start is recorded");
    let path = pull_path(&world).await;
    let before = head_of(&path);
    std::fs::write(path.join("README.md"), "edited\n").expect("the local edit writes");
    push_to_pull(&world, FIRST_PULL, "second.txt");

    let refused = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the refusal is recorded");

    assert!(refused.runs[1].failed_to_start());
    assert_eq!(
        refused.runs[1].detail.as_deref(),
        Some(local_changes_refusal().as_str())
    );
    assert_eq!(head_of(&path), before);
    assert_eq!(
        std::fs::read_to_string(path.join("README.md")).expect("the edit is still there"),
        "edited\n"
    );
}

#[tokio::test]
async fn a_diverged_worktree_refuses_the_run() {
    let world = World::new().await;
    let id = board_id(&world).await;
    world
        .board(None, vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))])
        .await;
    world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the first start is recorded");
    let path = pull_path(&world).await;
    std::fs::write(path.join("local.txt"), "local\n").expect("the local file writes");
    git(&path, &["add", "local.txt"]);
    git(
        &path,
        &[
            "-c",
            "user.email=fleet@example.test",
            "-c",
            "user.name=Fleet Test",
            "commit",
            "-m",
            "local",
        ],
    );
    let local = head_of(&path);
    push_to_pull(&world, FIRST_PULL, "second.txt");

    let refused = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the refusal is recorded");

    assert_eq!(
        refused.runs[1].detail.as_deref(),
        Some(local_changes_refusal().as_str())
    );
    assert_eq!(head_of(&path), local);
}

/// Rewrites the origin's `refs/pull/<number>/head` to a commit that does not descend from the
/// old head, as a pull request's author rebasing or amending and force-pushing.
fn force_push_to_pull(world: &World, number: u64) -> String {
    let seed = world.seed();
    git(
        &seed,
        &["commit", "--amend", "--allow-empty", "-m", "rebased"],
    );
    git(
        &seed,
        &[
            "push",
            "-f",
            "origin",
            &format!("HEAD:refs/pull/{number}/head"),
        ],
    );
    head_of(&seed)
}

#[tokio::test]
async fn a_force_pushed_head_resets_a_clean_untouched_worktree() {
    let world = World::new().await;
    let id = board_id(&world).await;
    world
        .board(None, vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))])
        .await;
    world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the first start is recorded");
    let path = pull_path(&world).await;
    // A run's own leftovers are untracked, and no checkout touches them.
    std::fs::write(path.join("coverage.out"), "left\n").expect("the leftover writes");
    let rebased = force_push_to_pull(&world, FIRST_PULL);

    let again = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the second start is recorded");

    assert_eq!(head_of(&path), rebased);
    assert!(path.join("coverage.out").is_file());
    assert_ne!(
        again.runs[1].detail.as_deref(),
        Some(local_changes_refusal().as_str())
    );
}

/// A refused follow leaves the record of what Fleet placed alone: once the user discards the
/// change that refused it, the force-pushed head is followed.
#[tokio::test]
async fn a_refused_follow_is_followed_once_the_worktree_is_clean() {
    let world = World::new().await;
    let id = board_id(&world).await;
    world
        .board(None, vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))])
        .await;
    world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the first start is recorded");
    let path = pull_path(&world).await;
    std::fs::write(path.join("README.md"), "edited\n").expect("the local edit writes");
    let rebased = force_push_to_pull(&world, FIRST_PULL);
    let refused = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the refusal is recorded");
    assert_eq!(
        refused.runs[1].detail.as_deref(),
        Some(local_changes_refusal().as_str())
    );
    git(&path, &["checkout", "--", "README.md"]);

    world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the third start is recorded");

    assert_eq!(head_of(&path), rebased);
}

/// The board's second running column — Review published on a Reviews board — works on the head
/// the first one reviewed, even after the author pushed again.
#[tokio::test]
async fn a_later_column_run_leaves_the_worktree_at_the_reviewed_head() {
    let world = World::new().await;
    let id = board_id(&world).await;
    let mut board = world
        .board(None, vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))])
        .await;
    world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the review start is recorded");
    let path = pull_path(&world).await;
    let reviewed = head_of(&path);
    for status in &mut board.statuses {
        if status.id.as_str() == "done" {
            status.automation = Some(ColumnAutomation {
                on_enter: Some(Action {
                    kind: ActionKind::Prompt,
                    instructions: "Publish {key}".into(),
                    expect: "a published review".into(),
                    agent: ColumnAgentPrefs::default(),
                    env: Vec::new(),
                }),
                on_success: None,
                advance_when_unblocked: None,
            });
        }
    }
    let mut cards = world.document(&board).cards;
    cards[0].status_id = "done".parse().expect("a static status id");
    world.write(&board, cards);
    push_to_pull(&world, FIRST_PULL, "second.txt");

    let published = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the publish start is recorded");

    assert_eq!(head_of(&path), reviewed);
    assert_ne!(
        published.runs.last().and_then(|run| run.detail.as_deref()),
        Some(local_changes_refusal().as_str())
    );
}

#[tokio::test]
async fn an_adopted_stale_worktree_is_brought_to_the_head_or_refused() {
    let world = World::new().await;
    let id = board_id(&world).await;
    world
        .board(
            None,
            vec![
                card(&id, 1, reviewing("acme/api", FIRST_PULL)),
                card(&id, 2, reviewing("acme/api", SECOND_PULL)),
            ],
        )
        .await;
    // Both worktrees exist before either card runs, as the old Review tab left them.
    for number in [FIRST_PULL, SECOND_PULL] {
        world
            .services
            .worktrees
            .create_from_pr(repo_id(), number)
            .await
            .expect("the pull request's worktree is created");
    }
    let paths = world.worktrees().await;
    let path_of = |number| {
        let id = pull_worktree(number);
        std::path::PathBuf::from(
            &paths
                .iter()
                .find(|worktree| worktree.id == id)
                .expect("the worktree exists")
                .path,
        )
    };
    let (clean, dirty) = (path_of(FIRST_PULL), path_of(SECOND_PULL));
    std::fs::write(dirty.join("README.md"), "mine\n").expect("the local edit writes");
    let dirty_before = head_of(&dirty);
    let pushed = push_to_pull(&world, FIRST_PULL, "second.txt");
    push_to_pull(&world, SECOND_PULL, "third.txt");

    let followed = world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the adopting start is recorded");
    let refused = world
        .services
        .boards
        .start_run(&card_id(2))
        .await
        .expect("the refusal is recorded");

    assert_eq!(head_of(&clean), pushed);
    assert_eq!(followed.worktree_id, Some(pull_worktree(FIRST_PULL)));
    assert_eq!(
        refused.runs[0].detail.as_deref(),
        Some(
            "validation failed: invalid automation: The worktree for acme/api#8 has local changes; commit or discard them before the review runs."
        )
    );
    assert_eq!(head_of(&dirty), dirty_before);
    assert!(refused.worktree_id.is_none());
}

/// The card worktree verb on a pull request's card makes that pull request's checkout, not a
/// slug branch off the base.
#[tokio::test]
async fn the_worktree_verb_checks_out_a_cards_pull_request() {
    let world = World::new().await;
    let id = board_id(&world).await;
    let mut waiting = card(&id, 1, reviewing("acme/api", FIRST_PULL));
    waiting.status_id = "backlog".parse().expect("a static status id");
    world.board(None, vec![waiting]).await;
    let pushed = push_to_pull(&world, FIRST_PULL, "second.txt");

    let (card, worktree, created) = world
        .services
        .boards
        .create_worktree_from_card(&card_id(1), None, None, None)
        .await
        .expect("the verb links the pull request's worktree");

    assert!(created);
    assert_eq!(worktree.id, pull_worktree(FIRST_PULL));
    assert_eq!(card.worktree_id, Some(pull_worktree(FIRST_PULL)));
    assert_eq!(head_of(Path::new(&worktree.path)), pushed);
}

/// A start that fails before any delegation exists frees its slot at once: the cards waiting
/// behind it get their (here, equally failed) starts without an unrelated delegation ending.
#[tokio::test]
async fn a_failed_start_hands_its_slot_to_the_next_card() {
    let world = World::new().await;
    let id = board_id(&world).await;
    let waiting = |number| {
        let mut card = card(&id, number, reviewing("other/lib", u64::from(number)));
        card.status_id = "backlog".parse().expect("a static status id");
        card
    };
    let board = world
        .board(Some(1), vec![waiting(1), waiting(2), waiting(3)])
        .await;
    let todo: fleet_core::ids::StatusId = "todo".parse().expect("a static status id");
    for number in 1..=3 {
        world
            .services
            .boards
            .move_card(&card_id(number), &todo, None, false)
            .await
            .expect("the card moves into the running column");
    }

    world.services.boards.background_starts_settled().await;

    let doc = world.document(&board);
    for card in &doc.cards {
        assert_eq!(card.runs.len(), 1, "{} got no start", card.id);
        assert!(card.runs[0].failed_to_start());
        assert!(card.pending_run.is_none(), "{} still waits", card.id);
    }
}

/// A card that re-enters a routing column while a start still holds it is evaluated again once
/// that start is abandoned, instead of standing there with nothing to move it.
#[tokio::test]
async fn an_abandoned_start_evaluates_its_card_again() {
    let world = World::new().await;
    let id = board_id(&world).await;
    let mut routed = card(&id, 1, reviewing("other/lib", 3));
    routed.status_id = "backlog".parse().expect("a static status id");
    let mut board = world.board(None, vec![routed.clone()]).await;
    for status in &mut board.statuses {
        if status.id.as_str() == "backlog" {
            status.automation = Some(ColumnAutomation {
                on_enter: None,
                on_success: None,
                advance_when_unblocked: Some("todo".parse().expect("a static status id")),
            });
        }
    }
    world.write(&board, vec![routed]);
    // The state a start in flight leaves: the card is reserved, and its entry into the routing
    // column was refused for that reason.
    let automation = world
        .services
        .boards
        .automation()
        .expect("the test daemon runs automation");
    automation
        .in_flight
        .lock()
        .await
        .entry(board.id.clone())
        .or_default()
        .insert(card_id(1));

    world
        .services
        .boards
        .start_for_card(&board.id, &card_id(1))
        .await
        .expect("the abandoned start answers");
    world.services.boards.background_starts_settled().await;

    let doc = world.document(&board);
    assert_eq!(doc.cards[0].status_id.as_str(), "todo");
    assert_eq!(doc.cards[0].runs.len(), 1, "the re-evaluation started it");
}

/// Two runs ending together free two slots. The first went to the oldest waiting card, whose
/// start is still in flight and whose marker is still set; the second must go to the card behind
/// it, not to that one again.
#[tokio::test]
async fn a_second_freed_slot_passes_over_a_card_already_handed_one() {
    let world = World::new().await;
    let id = board_id(&world).await;
    let waiting = |number: u32, since: &str| {
        let mut card = card(&id, number, reviewing("other/lib", u64::from(number)));
        card.pending_run = Some(fleet_core::board::PendingRun {
            status_id: "todo".parse().expect("a static status id"),
            since: since.into(),
        });
        card
    };
    let board = world
        .board(
            Some(2),
            vec![
                waiting(1, "2026-09-22T00:00:01Z"),
                waiting(2, "2026-09-22T00:00:02Z"),
            ],
        )
        .await;
    // The first freed slot's hand-off: card 1 is reserved, its start not yet recorded.
    let automation = world
        .services
        .boards
        .automation()
        .expect("the test daemon runs automation");
    automation
        .in_flight
        .lock()
        .await
        .entry(board.id.clone())
        .or_default()
        .insert(card_id(1));

    world
        .services
        .boards
        .release_slot(&board.id)
        .await
        .expect("the second slot is handed on");
    world.services.boards.background_starts_settled().await;

    let doc = world.document(&board);
    let runs = |number| {
        doc.cards
            .iter()
            .find(|card| card.id == card_id(number))
            .map_or(0, |card| card.runs.len())
    };
    assert_eq!(runs(2), 1, "the card behind the reserved one got the slot");
    assert_eq!(runs(1), 0, "the reserved card's own start is not repeated");
}

/// The same pull request on two boards is reviewed on one of them: the second board's card is
/// refused the worktree the first one already links.
#[tokio::test]
async fn a_pull_request_worktree_is_linked_to_one_card_across_boards() {
    let world = World::new().await;
    let id = board_id(&world).await;
    let first = world
        .board(None, vec![card(&id, 1, reviewing("acme/api", FIRST_PULL))])
        .await;
    world
        .services
        .boards
        .start_run(&card_id(1))
        .await
        .expect("the first board's start is recorded");

    let mut second = first.clone();
    second.id = "work-reviews".parse().expect("a static board id");
    let mut other = card(second.id.as_str(), 2, reviewing("acme/api", FIRST_PULL));
    other.id = card_id(2);
    world.write(&second, vec![other]);
    world
        .services
        .boards
        .get(&second.id)
        .await
        .expect("the second board reads");

    let refused = world
        .services
        .boards
        .start_run(&card_id(2))
        .await
        .expect("the refusal is recorded");

    assert!(refused.runs[0].failed_to_start());
    assert_eq!(
        refused.runs[0].detail.as_deref(),
        Some(
            format!(
                "conflict: worktree {} already belongs to another card",
                pull_worktree(FIRST_PULL)
            )
            .as_str()
        )
    );
    assert!(refused.worktree_id.is_none());
}
