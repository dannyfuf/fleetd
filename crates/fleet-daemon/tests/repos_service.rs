use std::{path::Path, process::Command, sync::Arc, time::Duration};

use fleet_core::model::RepoHooks;
use fleet_daemon::{
    adapters::{
        Adapters,
        clock::SystemClock,
        files::{Files, RealFiles},
        git::{Git, ShellGit},
        github::Github,
        process::{Process, RealProcess},
        shell::{RealShell, Shell, ShellResult},
    },
    jobs::JobManager,
    services::{Services, contexts::Contexts, repos::Repos},
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::{FakeGithub, FakeShell},
};
use fleet_proto::{
    job::{JobKind, JobStatus},
    request::RequestBody,
};

fn stores(temp: &tempfile::TempDir) -> (Arc<ConfigStore>, Arc<StateStore>, Arc<dyn Files>) {
    let home = temp.path().join(".fleet");
    let files: Arc<dyn Files> = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    let config = Arc::new(ConfigStore::new(&home, Arc::clone(&files)));
    let state = Arc::new(StateStore::new(
        &home,
        Arc::clone(&files),
        Arc::new(SystemClock),
    ));
    (config, state, files)
}

fn init_repository(path: &Path) {
    let status = Command::new("git")
        .args(["init", "-b", "main"])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::write(path.join("README.md"), "fixture\n").unwrap();
    for args in [
        vec!["config", "user.email", "fleet@example.test"],
        vec!["config", "user.name", "Fleet Test"],
        vec!["add", "README.md"],
        vec!["commit", "-m", "fixture"],
    ] {
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }
}

async fn wait_for_job(jobs: &JobManager, id: &fleet_core::ids::JobId) -> JobStatus {
    for _ in 0..400 {
        if let Some(job) = jobs.list().into_iter().find(|job| &job.id == id)
            && !matches!(
                job.status,
                JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling
            )
        {
            return job.status;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("job did not finish");
}

#[tokio::test]
async fn repos_clone_reconciles_then_moves_updates_hooks_and_deletes() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    init_repository(&source);
    let (config, state, files) = stores(&temp);
    let contexts = Contexts::new(Arc::clone(&state));
    let first = contexts
        .create("First".to_owned(), vec!["acme".to_owned()])
        .await
        .unwrap();
    let second = contexts.create("Second".to_owned(), vec![]).await.unwrap();
    let jobs = Arc::new(JobManager::new(temp.path().join(".fleet")));
    let shell = Arc::new(FakeShell::new());
    let real_shell: Arc<dyn Shell> = Arc::new(RealShell);
    let adapters = Adapters {
        git: Arc::new(ShellGit::new(Arc::clone(&real_shell))),
        github: Arc::new(FakeGithub::new(shell)),
        process: Arc::new(RealProcess::new(Arc::clone(&real_shell))),
        files,
        shell: real_shell,
    };
    let services = Services::new(
        temp.path().join(".fleet"),
        config,
        Arc::clone(&state),
        Arc::clone(&jobs),
        adapters,
    );
    let repos = &services.repos;

    let failed = repos
        .clone_repo(
            "acme".to_owned(),
            "missing".to_owned(),
            temp.path()
                .join("does-not-exist")
                .to_string_lossy()
                .into_owned(),
            first.id.clone(),
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        wait_for_job(&jobs, &failed.id).await,
        JobStatus::Failed { .. }
    ));
    let failed_repo = fleet_core::ids::RepoId::try_from("acme/missing").unwrap();
    let failed_clone = state
        .load()
        .await
        .unwrap()
        .clones
        .into_iter()
        .find(|clone| clone.id == failed_repo)
        .unwrap();
    assert!(failed_clone.error.is_some());
    assert!(!failed_clone.log_path.is_empty());
    repos.dismiss_clone(failed_repo).await.unwrap();

    let job = repos
        .clone_repo(
            "acme".to_owned(),
            "api".to_owned(),
            source.to_string_lossy().into_owned(),
            first.id,
            None,
        )
        .await
        .unwrap();
    assert_eq!(job.kind, JobKind::Clone);
    assert_eq!(wait_for_job(&jobs, &job.id).await, JobStatus::Succeeded);
    let snapshot = state.load().await.unwrap();
    assert!(snapshot.clones.is_empty());
    assert_eq!(snapshot.repos[0].default_branch, "main");

    let hooks = RepoHooks {
        prepare: vec!["bundle install".to_owned()],
        post_create: vec!["bin/setup".to_owned()],
    };
    let repo_id = snapshot.repos[0].id.clone();
    let updated = repos
        .set_hooks(repo_id.clone(), hooks.clone())
        .await
        .unwrap();
    assert_eq!(updated.hooks, hooks);
    let moved = repos
        .move_to_context(repo_id.clone(), second.id)
        .await
        .unwrap();
    assert_eq!(moved.context_id.as_str(), "second");

    services
        .dispatch(RequestBody::DeleteRepo { repo: repo_id })
        .await
        .unwrap();
    assert!(state.load().await.unwrap().repos.is_empty());
}

#[tokio::test]
async fn repos_discovery_is_cached_and_search_is_tokenized() {
    let temp = tempfile::tempdir().unwrap();
    let (config, state, files) = stores(&temp);
    let jobs = Arc::new(JobManager::new(temp.path().join(".fleet")));
    let shell = Arc::new(FakeShell::new());
    shell.when(
        |command| command.program == "gh" && command.args.starts_with(&["repo".to_owned()]),
        ShellResult {
            status: 0,
            stdout: r#"[{"name":"api","owner":{"login":"acme"},"nameWithOwner":"acme/api","description":"payments service","sshUrl":"git@github.com:acme/api.git","isPrivate":true,"updatedAt":"2026-09-04T00:00:00Z","defaultBranchRef":{"name":"main"}},{"name":"web","owner":{"login":"acme"},"nameWithOwner":"acme/web","description":"frontend","sshUrl":"git@github.com:acme/web.git","isPrivate":false,"updatedAt":"2026-09-03T00:00:00Z","defaultBranchRef":{"name":"main"}}]"#.to_owned(),
            stderr: String::new(),
        },
    );
    let github: Arc<dyn Github> = Arc::new(FakeGithub::new(Arc::clone(&shell)));
    let git_shell = Arc::new(FakeShell::new());
    let git: Arc<dyn Git> = Arc::new(fleet_daemon::testing::fakes::FakeGit::new(Arc::clone(
        &git_shell,
    )));
    let process: Arc<dyn Process> = Arc::new(RealProcess::new(git_shell));
    let repos = Repos::new(
        config,
        state,
        Arc::clone(&jobs),
        git,
        github,
        files,
        process,
    );

    let cache = repos.list_remote("acme".to_owned(), true).await.unwrap();
    assert_eq!(cache.repos.len(), 2);
    let matched = repos
        .search_remote("acme".to_owned(), "payments api".to_owned())
        .await
        .unwrap();
    assert_eq!(matched.repos.len(), 1);
    assert_eq!(matched.repos[0].full_name, "acme/api");
    assert_eq!(
        shell
            .calls()
            .into_iter()
            .filter(|call| matches!(call, fleet_daemon::testing::fakes::FakeShellCall::Run(_)))
            .count(),
        1
    );
}
