use std::{io, path::Path, process::Command, sync::Arc};

use async_trait::async_trait;
use fleet_core::{
    ids::{ContextId, RepoId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    state::default_state,
};
use fleet_daemon::{
    DaemonError, DaemonResult,
    adapters::{
        clock::SystemClock,
        files::RealFiles,
        git::{Git, ShellGit},
        github::Github,
        shell::{RealShell, ShellResult},
    },
    jobs::JobManager,
    services::{
        inspect::Inspect,
        prune::{Prune, WorktreeDeleter},
        sessions::{Sessions, TransitionLockClaim},
    },
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::{FakeFiles, FakeFilesCall, FakeGit, FakeGithub, FakeShell, FixedClock},
};

struct NoopDeleter;

#[async_trait]
impl WorktreeDeleter for NoopDeleter {
    async fn delete(
        &self,
        _id: WorktreeId,
        _lifecycle: Option<TransitionLockClaim>,
    ) -> DaemonResult<()> {
        Ok(())
    }
}

#[tokio::test]
async fn inspect_reports_dirty_file_count_and_conservative_merge() {
    let fixture = inspection_fixture().await;
    std::fs::write(fixture.worktree.join("one.txt"), "one\n")
        .unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(fixture.worktree.join("two.txt"), "two\n")
        .unwrap_or_else(|error| panic!("{error}"));

    let inspections = fixture
        .inspect
        .worktrees(Vec::new(), None, true)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(inspections.len(), 1);
    let actual = &inspections[0];
    assert_eq!(actual.dirty_files, Some(2));
    assert!(actual.dirty);
    assert!(actual.published);
    assert!(actual.merged_into_target, "{:?}", actual.warnings);
    assert!(actual.merged);
    assert_eq!(actual.unique_commits, Some(0));
    assert!(actual.error.is_none());
}

#[tokio::test]
async fn inspection_uses_live_branch() {
    let fixture = inspection_fixture().await;
    run(
        &fixture.worktree,
        &["checkout", "-b", "live", "origin/main"],
    );

    let inspection = fixture
        .inspect
        .worktrees(Vec::new(), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .pop()
        .unwrap_or_else(|| panic!("inspection should exist"));

    let head = revision(&fixture.worktree);
    assert_eq!(inspection.branch, "live");
    assert_eq!(inspection.head.as_deref(), Some(head.as_str()));
    assert!(!inspection.published);
}

#[tokio::test]
async fn unpushed_head_is_not_false_unknown() {
    let fixture = inspection_fixture().await;
    std::fs::write(fixture.worktree.join("local.txt"), "local only\n")
        .unwrap_or_else(|error| panic!("{error}"));
    run(&fixture.worktree, &["add", "local.txt"]);
    run(&fixture.worktree, &["commit", "-m", "local only"]);

    let inspection = fixture
        .inspect
        .worktrees(Vec::new(), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .pop()
        .unwrap_or_else(|| panic!("inspection should exist"));

    assert_eq!(
        inspection.unique_commits,
        Some(1),
        "{:?}",
        inspection.warnings
    );
    assert!(!inspection.merged_into_target);
}

#[tokio::test]
async fn selected_missing_worktree_is_reported_as_an_item_error() {
    let fixture = inspection_fixture().await;
    let missing =
        WorktreeId::try_from("acme/api#missing").unwrap_or_else(|error| panic!("{error}"));

    let inspections = fixture
        .inspect
        .worktrees(vec![missing.clone()], None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(inspections.len(), 1);
    assert_eq!(inspections[0].worktree_id, missing);
    assert!(
        inspections[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("not found"))
    );
}

#[tokio::test]
async fn inspect_and_prune_preserve_typed_failures() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let (inspect, jobs, files, state, _) = failing_inspect(temp.path().join("inspect"));
    fail_state_read(&files, &state);
    let inspect_error = inspect
        .worktrees(Vec::new(), None, false)
        .await
        .unwrap_err();
    assert!(matches!(inspect_error, DaemonError::Filesystem { .. }));
    assert_job_kept_filesystem_error(&jobs).await;

    let (inspect, jobs, files, state, sessions) = failing_inspect(temp.path().join("prune"));
    fail_state_read(&files, &state);
    let prune = Prune::new(Arc::clone(&jobs), inspect, sessions, Arc::new(NoopDeleter));
    let prune_error = prune
        .worktrees(false, false, false, None, None)
        .await
        .unwrap_err();
    assert!(matches!(prune_error, DaemonError::Filesystem { .. }));
    assert_job_kept_filesystem_error(&jobs).await;
}

struct InspectionFixture {
    _temp: tempfile::TempDir,
    inspect: Inspect,
    worktree: std::path::PathBuf,
}

async fn inspection_fixture() -> InspectionFixture {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let origin = temp.path().join("origin.git");
    let base = temp.path().join("base");
    let worktree = temp.path().join("feature");
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
    run(temp.path(), &["clone", path(&origin), path(&worktree)]);
    run(&worktree, &["config", "user.email", "fleet@example.com"]);
    run(&worktree, &["config", "user.name", "Fleet Test"]);
    run(&worktree, &["checkout", "-b", "feature"]);
    std::fs::write(worktree.join("feature.txt"), "feature\n")
        .unwrap_or_else(|error| panic!("{error}"));
    run(&worktree, &["add", "feature.txt"]);
    run(&worktree, &["commit", "-m", "feature"]);
    run(&worktree, &["push", "-u", "origin", "feature"]);
    run(&base, &["fetch", "origin", "feature"]);
    run(
        &base,
        &["merge", "--no-ff", "origin/feature", "-m", "merge feature"],
    );
    run(&base, &["push", "origin", "main"]);

    let home = temp.path().join("fleet-home");
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    let state = Arc::new(StateStore::new(&home, files.clone(), Arc::new(SystemClock)));
    let repo_id = RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}"));
    let context_id = ContextId::try_from("acme").unwrap_or_else(|error| panic!("{error}"));
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
        id: WorktreeId::try_from("acme/api#feature").unwrap_or_else(|error| panic!("{error}")),
        repo_id,
        slug: "feature".to_owned(),
        branch: "feature".to_owned(),
        base_ref: "origin/main".to_owned(),
        path: worktree.display().to_string(),
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
    let sessions = Sessions::new(Arc::new(ConfigStore::new(&home, files)), Arc::clone(&state));
    let inspect = Inspect::new(
        state,
        Arc::new(JobManager::new(&home)),
        git,
        github,
        sessions,
    );
    InspectionFixture {
        _temp: temp,
        inspect,
        worktree,
    }
}

fn failing_inspect(
    home: std::path::PathBuf,
) -> (
    Inspect,
    Arc<JobManager>,
    Arc<FakeFiles>,
    Arc<StateStore>,
    Sessions,
) {
    let files = Arc::new(FakeFiles::new(
        home.join("trash"),
        vec![home.join("repos"), home.join("worktrees")],
    ));
    let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
    let state = Arc::new(StateStore::new(&home, files.clone(), clock.clone()));
    let config = Arc::new(ConfigStore::new(&home, files.clone()));
    let jobs = Arc::new(JobManager::with_clock(&home, clock));
    let shell = Arc::new(FakeShell::new());
    let git: Arc<dyn Git> = Arc::new(FakeGit::new(Arc::clone(&shell)));
    let github: Arc<dyn Github> = Arc::new(FakeGithub::new(shell));
    let sessions = Sessions::new(config, Arc::clone(&state));
    (
        Inspect::new(
            Arc::clone(&state),
            Arc::clone(&jobs),
            git,
            github,
            sessions.clone(),
        ),
        jobs,
        files,
        state,
        sessions,
    )
}

fn fail_state_read(files: &FakeFiles, state: &StateStore) {
    files.insert_text(
        state.path(),
        serde_json::to_string(&default_state()).unwrap_or_else(|error| panic!("{error}")),
    );
    files.fail_next(
        FakeFilesCall::Read(state.path().to_path_buf()),
        io::ErrorKind::PermissionDenied,
    );
}

async fn assert_job_kept_filesystem_error(jobs: &JobManager) {
    let id = jobs
        .list()
        .last()
        .unwrap_or_else(|| panic!("job should be recorded"))
        .id
        .clone();
    let record = jobs
        .wait(&id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let fleet_proto::job::JobStatus::Failed { error } = record.status else {
        panic!("job should fail");
    };
    assert!(error.starts_with("filesystem operation failed"), "{error}");
}

fn revision(repo: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .unwrap_or_else(|error| panic!("could not read HEAD: {error}"));
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn run(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
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
