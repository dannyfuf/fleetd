use std::sync::Arc;

use fleet_core::{
    cache::PrCache,
    github::{PrChecks, PrReviewDecision, PrTab, PullRequest},
    ids::{ContextId, RepoId},
    model::{Context, Repo, RepoHooks},
    paths::FleetHome,
};
use fleet_daemon::{
    adapters::{
        clock::SystemClock,
        files::{Files, RealFiles},
        github::Github as GithubAdapter,
        shell::ShellResult,
    },
    jobs::JobManager,
    services::github::Github,
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::{FakeGithub, FakeShell},
};

async fn service(
    temp: &tempfile::TempDir,
    shell: Arc<FakeShell>,
) -> (Github, Arc<StateStore>, RepoId, ContextId) {
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
    let context = ContextId::try_from("platform").unwrap();
    let repo = RepoId::try_from("acme/api").unwrap();
    let context_for_state = context.clone();
    let repo_for_state = repo.clone();
    let home_for_state = home.clone();
    state
        .transaction(move |state| {
            state.contexts.push(Context {
                id: context_for_state.clone(),
                name: "Platform".to_owned(),
                owners: vec!["acme".to_owned()],
                created_at: "2026-09-04T00:00:00Z".to_owned(),
            });
            state.active_context_id = Some(context_for_state.clone());
            state.repos.push(Repo {
                id: repo_for_state,
                owner: "acme".to_owned(),
                name: "api".to_owned(),
                url: "git@github.com:acme/api.git".to_owned(),
                context_id: context_for_state,
                default_branch: "main".to_owned(),
                path: home_for_state
                    .join("repos/acme/api")
                    .to_string_lossy()
                    .into_owned(),
                cloned_at: "2026-09-04T00:00:00Z".to_owned(),
                hooks: RepoHooks::default(),
            });
            Ok(())
        })
        .await
        .unwrap();
    let jobs = Arc::new(JobManager::new(&home));
    let adapter: Arc<dyn GithubAdapter> = Arc::new(FakeGithub::new(shell));
    (
        Github::new(config, Arc::clone(&state), jobs, adapter, files),
        state,
        repo,
        context,
    )
}

fn pull_request(repo: RepoId) -> PullRequest {
    PullRequest {
        repo_id: repo,
        number: 42,
        title: "Fix API".to_owned(),
        url: "https://github.com/acme/api/pull/42".to_owned(),
        author: "octocat".to_owned(),
        head_ref_name: "fix-api".to_owned(),
        base_ref_name: "main".to_owned(),
        is_draft: false,
        is_cross_repository: false,
        head_repo: None,
        review_decision: PrReviewDecision::Approved,
        checks: PrChecks::Pass,
        checks_passed: Some(2),
        checks_total: Some(2),
        additions: 10,
        deletions: 2,
        labels: vec!["ready".to_owned()],
        updated_at: "2026-09-04T00:00:00Z".to_owned(),
    }
}

#[tokio::test]
async fn github_fetches_exact_tab_and_reuses_fresh_cache() {
    let temp = tempfile::tempdir().unwrap();
    let shell = Arc::new(FakeShell::new());
    shell.when(
        |command| command.program == "gh" && command.args.starts_with(&["pr".to_owned()]),
        ShellResult {
            status: 0,
            stdout: r#"[{"number":42,"title":"Fix API","url":"https://github.com/acme/api/pull/42","author":{"login":"octocat"},"headRefName":"fix-api","baseRefName":"main","isDraft":false,"isCrossRepository":false,"headRepository":{"name":"api","nameWithOwner":"acme/api"},"headRepositoryOwner":{"login":"acme"},"reviewDecision":"APPROVED","statusCheckRollup":[{"conclusion":"SUCCESS","status":"COMPLETED"}],"additions":10,"deletions":2,"labels":[{"name":"ready"}],"updatedAt":"2026-09-04T00:00:00Z"}]"#.to_owned(),
            stderr: String::new(),
        },
    );
    shell.when(
        |command| command.program == "gh" && command.args.first().is_some_and(|arg| arg == "api"),
        ShellResult {
            status: 0,
            stdout: "octocat\n".to_owned(),
            stderr: String::new(),
        },
    );
    let (github, _state, repo, context) = service(&temp, Arc::clone(&shell)).await;
    assert_eq!(github.viewer_login().await.unwrap(), "octocat");

    let slices = github
        .list_pull_requests(None, Some(context.clone()), PrTab::Mine, true)
        .await
        .unwrap();
    assert_eq!(slices.len(), 1);
    assert_eq!(slices[0].total, 1);
    assert_eq!(slices[0].prs[0].number, 42);
    assert!(slices[0].error.is_none());

    let cached = github
        .list_pull_requests(Some(repo), None, PrTab::Mine, false)
        .await
        .unwrap();
    assert_eq!(cached[0].total, 1);
    let pr_calls = shell
        .calls()
        .into_iter()
        .filter(|call| {
            matches!(
                call,
                fleet_daemon::testing::fakes::FakeShellCall::Run(command)
                    if command.args.first().is_some_and(|arg| arg == "pr")
            )
        })
        .count();
    assert_eq!(pr_calls, 1);
}

#[tokio::test]
async fn github_preserves_stale_cache_and_reports_a_concise_error() {
    let temp = tempfile::tempdir().unwrap();
    let shell = Arc::new(FakeShell::new());
    shell.when(
        |command| command.program == "gh" && command.args.starts_with(&["pr".to_owned()]),
        ShellResult {
            status: 1,
            stdout: String::new(),
            stderr: "temporary upstream failure".to_owned(),
        },
    );
    let (github, _state, repo, _context) = service(&temp, shell).await;
    let cache = PrCache {
        fetched_at: "2020-01-01T00:00:00Z".to_owned(),
        prs: vec![pull_request(repo.clone())],
    };
    let path = FleetHome::new(temp.path().join(".fleet")).pr_cache_path(&repo, PrTab::Review);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_string(&cache).unwrap()).unwrap();

    let slices = github
        .list_pull_requests(Some(repo), None, PrTab::Review, false)
        .await
        .unwrap();
    assert_eq!(slices[0].prs.len(), 1);
    assert!(
        slices[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("temporary upstream failure"))
    );
}
