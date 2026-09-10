use std::{
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use fleet_core::{
    ids::{ContextId, RepoId},
    model::{Context, Repo, RepoHooks},
    state::default_state,
};
use fleet_daemon::{
    DaemonResult,
    adapters::{
        clock::SystemClock,
        files::RealFiles,
        git::ShellGit,
        shell::{DetachedProcess, LineCallback, RealShell, Shell, ShellCommand, ShellResult},
    },
    jobs::JobManager,
    services::pool::Pool,
    stores::{config::ConfigStore, state::StateStore},
};
use fleet_proto::job::JobKind;
use serde_json::json;
use tokio::sync::Barrier;
use tokio_util::sync::CancellationToken;

struct BlockingFirstFetch {
    inner: RealShell,
    fetches: AtomicUsize,
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl BlockingFirstFetch {
    fn new() -> Self {
        Self {
            inner: RealShell,
            fetches: AtomicUsize::new(0),
            entered: Arc::new(Barrier::new(2)),
            release: Arc::new(Barrier::new(2)),
        }
    }
}

#[async_trait]
impl Shell for BlockingFirstFetch {
    async fn run(&self, command: ShellCommand) -> DaemonResult<ShellResult> {
        if command.program == "git"
            && command.args == ["fetch", "--prune", "origin"]
            && self.fetches.fetch_add(1, Ordering::SeqCst) == 0
        {
            self.entered.wait().await;
            self.release.wait().await;
        }
        self.inner.run(command).await
    }

    async fn run_detached(
        &self,
        command: ShellCommand,
        log_path: &Path,
    ) -> DaemonResult<DetachedProcess> {
        self.inner.run_detached(command, log_path).await
    }

    async fn run_streaming(
        &self,
        command: ShellCommand,
        cancel: CancellationToken,
        on_line: LineCallback,
    ) -> DaemonResult<ShellResult> {
        self.inner.run_streaming(command, cancel, on_line).await
    }
}

#[tokio::test]
async fn pool_build_claim_refill_and_snapshot_status() {
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
            "hotPoolSize": 2,
            "hotRefreshIntervalMs": 0
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
    let shell = Arc::new(RealShell);
    let git = Arc::new(ShellGit::new(Arc::clone(&shell)));
    let pool = Pool::new(config.clone(), state, jobs, git, files, shell);

    pool.prepare(repo.id.clone(), false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let root = home.join("worktrees/acme/api");
    assert!(root.join(".hot/.git/swarm-hot.json").is_file());
    assert!(root.join(".hot.1/.git/swarm-hot.json").is_file());

    let claimed = pool
        .claim(repo.id.clone())
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("prepared copy should be ready"));
    assert!(claimed.join(".git/swarm-hot.json").is_file());
    assert!(!root.join(".hot").exists());

    pool.prepare(repo.id.clone(), false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(root.join(".hot/.git/swarm-hot.json").is_file());
    assert!(root.join(".hot.1/.git/swarm-hot.json").is_file());

    let before = std::fs::read_to_string(root.join(".hot/.git/swarm-hot.json"))
        .unwrap_or_else(|error| panic!("{error}"));
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    pool.prepare(
        RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
        true,
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    let after = std::fs::read_to_string(root.join(".hot/.git/swarm-hot.json"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_ne!(before, after);

    config
        .update(json!({"hotPoolSize": 1}))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    pool.prepare(
        RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
        false,
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(root.join(".hot").is_dir());
    assert!(!root.join(".hot.1").exists());
}

#[tokio::test]
async fn force_runs_forced_successor() {
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
            "hotPoolSize": 1,
            "hotRefreshIntervalMs": 0
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
    let shell = Arc::new(BlockingFirstFetch::new());
    let shell_adapter: Arc<dyn Shell> = shell.clone();
    let pool = Pool::new(
        config,
        state,
        Arc::clone(&jobs),
        Arc::new(ShellGit::new(Arc::clone(&shell_adapter))),
        files,
        shell_adapter,
    );

    let ordinary = {
        let pool = pool.clone();
        let repo = repo.id.clone();
        tokio::spawn(async move { pool.prepare(repo, false).await })
    };
    shell.entered.wait().await;
    let forced = {
        let pool = pool.clone();
        let repo = repo.id.clone();
        tokio::spawn(async move { pool.prepare(repo, true).await })
    };
    let mut forced_submitted = false;
    for _ in 0..100 {
        forced_submitted = jobs
            .list()
            .iter()
            .any(|job| job.kind == JobKind::PoolRefresh);
        if forced_submitted {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(forced_submitted, "forced successor was not submitted");
    shell.release.wait().await;

    ordinary
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|error| panic!("{error}"));
    forced
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(shell.fetches.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_slot_is_reused_inside_its_freshness_window_and_rebuilt_past_it() {
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
            "hotPoolSize": 1,
            "hotRefreshIntervalMs": 0
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
    let shell = Arc::new(RealShell);
    let git = Arc::new(ShellGit::new(Arc::clone(&shell)));
    let pool = Pool::new(config.clone(), state, jobs, git, files, shell);

    pool.prepare(repo.id.clone(), false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let marker = home.join("worktrees/acme/api/.hot/.git/swarm-hot.json");
    let built = std::fs::read_to_string(&marker).unwrap_or_else(|error| panic!("{error}"));

    // Inside the default sixty-second window an unforced prepare reuses the slot untouched.
    pool.prepare(repo.id.clone(), false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_else(|error| panic!("{error}")),
        built,
        "a slot inside its freshness window must not be rebuilt"
    );

    // Past the window the same unforced prepare rebuilds the slot and rewrites its marker.
    config
        .update(json!({"hotFreshnessMs": 0}))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    pool.prepare(repo.id.clone(), false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_ne!(
        std::fs::read_to_string(&marker).unwrap_or_else(|error| panic!("{error}")),
        built,
        "a slot past its freshness window must be rebuilt without force"
    );
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
