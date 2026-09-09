use std::{path::Path, process::Command, sync::Arc};

use fleet_core::{
    ids::{ContextId, RepoId, WorktreeId},
    model::{Context, Repo, RepoHooks},
    paths::CreatingMarker,
    state::default_state,
};
use fleet_daemon::{
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
    services::{sessions::Sessions, worktrees::Worktrees},
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::{FakeGithub, FakeShell},
};
use serde_json::json;

#[tokio::test]
async fn worktrees_create_delete_and_restore_are_atomic() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet");
    let repo_path = create_repository(temp.path());
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    let config = Arc::new(ConfigStore::new(&home, files.clone()));
    config
        .update(json!({
            "reposDir": home.join("repos"),
            "worktreesDir": home.join("worktrees"),
            "hotPoolSize": 0,
            "hotRefreshIntervalMs": 0,
            "trash": {"retentionMs": 600000}
        }))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let state = Arc::new(StateStore::new(&home, files.clone(), Arc::new(SystemClock)));
    let repo = fixture_repo(&repo_path);
    let mut persisted = default_state();
    persisted.contexts.push(fixture_context());
    persisted.repos.push(repo.clone());
    state
        .save(persisted)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let jobs = Arc::new(JobManager::new(&home));
    let adapters = adapters_with_stubbed_github(files);
    let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
    let worktrees = Worktrees::new(config, state.clone(), jobs, &adapters, sessions);

    let (created, worktree, _post_create_job) = worktrees
        .create(
            repo.id,
            "feature".to_owned(),
            None,
            None,
            RepoHooks::default(),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(created);
    assert_eq!(worktree.branch, "feature");
    assert_eq!(worktree.base_ref, "origin/main");
    assert!(Path::new(&worktree.path).is_dir());
    assert!(
        !Path::new(&worktree.path)
            .join(".git/swarm-creating.json")
            .exists()
    );

    let (created, same, _post_create_job) = worktrees
        .create(
            RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
            "feature".to_owned(),
            None,
            None,
            RepoHooks::default(),
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!created);
    assert_eq!(same.id, worktree.id);

    let deleted = worktrees
        .delete(vec![worktree.id.clone()])
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(deleted[0].ok, "{:?}", deleted[0].reason);
    assert!(!Path::new(&worktree.path).exists());
    assert!(
        state
            .load()
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .worktrees
            .is_empty()
    );
    let entry = std::fs::read_dir(home.join("trash"))
        .unwrap_or_else(|error| panic!("{error}"))
        .next()
        .unwrap_or_else(|| panic!("trash entry"))
        .unwrap_or_else(|error| panic!("{error}"))
        .file_name()
        .to_string_lossy()
        .into_owned();

    worktrees
        .restore_trash(entry)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(Path::new(&worktree.path).is_dir());
    let restored = state.load().await.unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(restored.worktrees.len(), 1);
    assert_eq!(restored.worktrees[0].id, worktree.id);

    worktrees
        .touch_opened(worktree.id.clone())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        state
            .load()
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .worktrees[0]
            .last_opened_at
            .is_some()
    );

    let path = worktrees
        .path(WorktreeId::try_from("acme/api#feature").unwrap_or_else(|error| panic!("{error}")))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(path, worktree.path);

    let (created, pull_request, _post_create_job) = worktrees
        .create_from_pr(
            RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
            7,
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(created);
    assert_eq!(pull_request.base_ref, "pull/7/head");
    assert_eq!(pull_request.branch, "pr/7");
    assert_eq!(
        git_output(Path::new(&pull_request.path), &["branch", "--show-current"]),
        "pr/7"
    );

    let (_created, hooked, _post_create_job) = worktrees
        .create(
            RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
            "hooked".to_owned(),
            None,
            None,
            RepoHooks {
                prepare: Vec::new(),
                post_create: vec!["printf post-create; exit 7".to_owned()],
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(Path::new(&hooked.path).is_dir());
    let mut degraded = None;
    for _ in 0..100 {
        degraded = state
            .load()
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .worktrees
            .into_iter()
            .find(|item| item.id == hooked.id)
            .and_then(|item| item.degraded);
        if degraded.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let degraded = degraded.unwrap_or_else(|| panic!("failed hook should persist degraded state"));
    assert_eq!(degraded.exit_code, Some(7));
    assert!(degraded.log_path.contains("/logs/jobs/"));

    let recovered_path = home.join("worktrees/acme/api/recovered");
    run(
        temp.path(),
        &[
            "clone",
            repo_path.to_str().unwrap_or_default(),
            recovered_path.to_str().unwrap_or_default(),
        ],
    );
    run(
        &recovered_path,
        &["checkout", "-b", "recovered", "origin/main"],
    );
    let recovery_marker = CreatingMarker {
        id: "acme/api#recovered".to_owned(),
        repo_id: RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
        branch: "recovered".to_owned(),
        base_ref: "origin/main".to_owned(),
        created_at: "2026-09-04T12:00:00Z".to_owned(),
    };
    std::fs::write(
        recovered_path.join(".git/swarm-creating.json"),
        serde_json::to_vec(&recovery_marker).unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let stale_attempt = home.join("worktrees/acme/api/stale.creating-deadbeef");
    run(
        temp.path(),
        &[
            "clone",
            repo_path.to_str().unwrap_or_default(),
            stale_attempt.to_str().unwrap_or_default(),
        ],
    );

    worktrees
        .recover_startup()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let recovered = state.load().await.unwrap_or_else(|error| panic!("{error}"));
    assert!(
        recovered
            .worktrees
            .iter()
            .any(|item| item.id.as_str() == "acme/api#recovered")
    );
    assert!(!recovered_path.join(".git/swarm-creating.json").exists());
    assert!(!stale_attempt.exists());
}

fn fixture_context() -> Context {
    Context {
        id: ContextId::try_from("acme").unwrap_or_else(|error| panic!("{error}")),
        name: "Acme".to_owned(),
        owners: vec!["acme".to_owned()],
        created_at: "2026-09-04T00:00:00Z".to_owned(),
    }
}

fn fixture_repo(path: &Path) -> Repo {
    Repo {
        id: RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
        owner: "acme".to_owned(),
        name: "api".to_owned(),
        url: "unused".to_owned(),
        context_id: ContextId::try_from("acme").unwrap_or_else(|error| panic!("{error}")),
        default_branch: "main".to_owned(),
        path: path.to_string_lossy().into_owned(),
        cloned_at: "2026-09-04T00:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    }
}

fn create_repository(root: &Path) -> std::path::PathBuf {
    let remote = root.join("remote.git");
    let seed = root.join("seed");
    let base = root.join("fleet/repos/acme/api");
    run(
        root,
        &["init", "--bare", remote.to_str().unwrap_or_default()],
    );
    run(root, &["init", seed.to_str().unwrap_or_default()]);
    run(&seed, &["config", "user.email", "fleet@example.test"]);
    run(&seed, &["config", "user.name", "Fleet Test"]);
    std::fs::write(seed.join("README.md"), "fleet\n").unwrap_or_else(|error| panic!("{error}"));
    run(&seed, &["add", "README.md"]);
    run(&seed, &["commit", "-m", "initial"]);
    run(&seed, &["branch", "-M", "main"]);
    run(
        &seed,
        &[
            "remote",
            "add",
            "origin",
            remote.to_str().unwrap_or_default(),
        ],
    );
    run(&seed, &["push", "-u", "origin", "main"]);
    run(&seed, &["push", "origin", "HEAD:refs/pull/7/head"]);
    run(
        root,
        &[
            "--git-dir",
            remote.to_str().unwrap_or_default(),
            "symbolic-ref",
            "HEAD",
            "refs/heads/main",
        ],
    );
    run(
        root,
        &[
            "clone",
            remote.to_str().unwrap_or_default(),
            base.to_str().unwrap_or_default(),
        ],
    );
    base
}

fn run(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Real Git, filesystem, and shell adapters with `gh pr view` answered from a fixture so
/// pull-request creation runs its production path without network access.
fn adapters_with_stubbed_github(files: Arc<RealFiles>) -> Adapters {
    let gh = Arc::new(FakeShell::new());
    gh.when(
        |command| command.program == "gh",
        ShellResult {
            status: 0,
            stdout: r#"{"number":7,"title":"Add feature","url":"https://github.com/acme/api/pull/7","author":{"login":"octocat"},"headRefName":"pr/7","baseRefName":"main","isDraft":false,"isCrossRepository":false,"headRepository":{"name":"api","nameWithOwner":"acme/api"},"headRepositoryOwner":{"login":"acme"},"reviewDecision":null,"statusCheckRollup":[],"additions":1,"deletions":0,"labels":[],"updatedAt":"2026-09-04T00:00:00Z"}"#
                .to_owned(),
            stderr: String::new(),
        },
    );
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
