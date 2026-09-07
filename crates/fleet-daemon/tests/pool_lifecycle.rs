use std::{path::Path, process::Command, sync::Arc};

use fleet_core::{
    ids::{ContextId, RepoId},
    model::{Context, Repo, RepoHooks},
    state::default_state,
};
use fleet_daemon::{
    adapters::{clock::SystemClock, files::RealFiles, git::ShellGit, shell::RealShell},
    jobs::JobManager,
    services::pool::Pool,
    stores::{config::ConfigStore, state::StateStore},
};
use serde_json::json;

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
