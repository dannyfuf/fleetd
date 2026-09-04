use std::{path::Path, process::Command, sync::Arc};

use fleet_core::{
    ids::{ContextId, RepoId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    state::default_state,
};
use fleet_daemon::{
    adapters::{
        clock::SystemClock,
        files::RealFiles,
        git::{Git, ShellGit},
        github::Github,
        shell::{RealShell, ShellResult},
    },
    jobs::JobManager,
    services::inspect::Inspect,
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::{FakeGithub, FakeShell},
};

#[tokio::test]
async fn inspect_reports_dirty_file_count_and_conservative_merge() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let origin = temp.path().join("origin.git");
    let base = temp.path().join("base");
    let worktree_path = temp.path().join("feature");
    run(temp.path(), &["init", "--bare", path(&origin)]);
    run(temp.path(), &["clone", path(&origin), path(&base)]);
    run(&base, &["config", "user.email", "fleet@example.com"]);
    run(&base, &["config", "user.name", "Fleet Test"]);
    run(&base, &["checkout", "-b", "main"]);
    std::fs::write(base.join("README.md"), "fleet\n").unwrap_or_else(|error| panic!("{error}"));
    run(&base, &["add", "README.md"]);
    run(&base, &["commit", "-m", "initial"]);
    run(&base, &["push", "-u", "origin", "main"]);
    run(&origin, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    run(temp.path(), &["clone", path(&origin), path(&worktree_path)]);
    run(&worktree_path, &["checkout", "-b", "feature"]);
    run(&worktree_path, &["push", "-u", "origin", "feature"]);
    std::fs::write(worktree_path.join("one.txt"), "one\n")
        .unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(worktree_path.join("two.txt"), "two\n")
        .unwrap_or_else(|error| panic!("{error}"));

    let home = temp.path().join("fleet-home");
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    let config = Arc::new(ConfigStore::new(&home, files.clone()));
    let state = Arc::new(StateStore::new(&home, files, Arc::new(SystemClock)));
    let repo_id = RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}"));
    let context_id = ContextId::try_from("acme").unwrap_or_else(|error| panic!("{error}"));
    let worktree_id =
        WorktreeId::try_from("acme/api#feature").unwrap_or_else(|error| panic!("{error}"));
    let mut persisted = default_state();
    persisted.contexts.push(Context {
        id: context_id.clone(),
        name: "Acme".to_owned(),
        owners: vec!["acme".to_owned()],
        created_at: "2026-09-04T00:00:00Z".to_owned(),
    });
    persisted.repos.push(Repo {
        id: repo_id.clone(),
        owner: "acme".to_owned(),
        name: "api".to_owned(),
        url: origin.display().to_string(),
        context_id,
        default_branch: "main".to_owned(),
        path: base.display().to_string(),
        cloned_at: "2026-09-04T00:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    });
    persisted.worktrees.push(Worktree {
        id: worktree_id,
        repo_id,
        slug: "feature".to_owned(),
        branch: "feature".to_owned(),
        base_ref: "origin/main".to_owned(),
        path: worktree_path.display().to_string(),
        session: "api/feature".to_owned(),
        host: None,
        created_at: "2026-09-04T00:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    });
    state
        .save(persisted)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let gh_shell = Arc::new(FakeShell::new());
    gh_shell.when(
        |command| command.program == "gh",
        ShellResult {
            status: 0,
            stdout: "[]".to_owned(),
            stderr: String::new(),
        },
    );
    let git: Arc<dyn Git> = Arc::new(ShellGit::new(Arc::new(RealShell)));
    let github: Arc<dyn Github> = Arc::new(FakeGithub::new(gh_shell));
    let inspect = Inspect::new(config, state, Arc::new(JobManager::new(&home)), git, github);

    let inspections = inspect
        .worktrees(Vec::new(), None, true)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(inspections.len(), 1);
    let actual = &inspections[0];
    assert_eq!(actual.dirty_files, Some(2));
    assert!(actual.dirty);
    assert!(actual.published);
    assert!(actual.merged_into_target);
    assert!(actual.merged);
    assert_eq!(actual.unique_commits, Some(0));
    assert!(actual.error.is_none());
}

fn run(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|error| panic!("could not run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn path(path: &Path) -> &str {
    path.to_str()
        .unwrap_or_else(|| panic!("non-UTF-8 test path: {}", path.display()))
}
