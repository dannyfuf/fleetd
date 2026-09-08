use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use fleet_core::{
    ids::RepoId,
    model::{CloneJob, CloneStatus, Repo, RepoHooks},
};
use fleet_daemon::{
    DaemonError, DaemonResult,
    adapters::{
        Adapters,
        board::BoardBackends,
        clock::{Clock, SystemClock},
        files::{Files, RealFiles},
        git::{Git, ShellGit},
        github::Github,
        process::{Process, RealProcess},
        shell::{DetachedProcess, LineCallback, RealShell, Shell, ShellCommand, ShellResult},
    },
    jobs::JobManager,
    services::{Services, contexts::Contexts, repos::Repos},
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::{FakeGithub, FakeProcess, FakeShell},
};
use fleet_proto::{
    job::{JobKind, JobStatus},
    request::RequestBody,
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct FailingCloneShell {
    attempts: Mutex<Vec<PathBuf>>,
}

impl FailingCloneShell {
    fn attempts(&self) -> Vec<PathBuf> {
        self.attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl Shell for FailingCloneShell {
    async fn run(&self, _command: ShellCommand) -> DaemonResult<ShellResult> {
        Ok(ShellResult {
            status: 127,
            stdout: String::new(),
            stderr: "unexpected command".to_owned(),
        })
    }

    async fn run_detached(
        &self,
        command: ShellCommand,
        _log_path: &Path,
    ) -> DaemonResult<DetachedProcess> {
        let staging =
            command.args.last().map(PathBuf::from).ok_or_else(|| {
                DaemonError::Validation("clone destination is missing".to_owned())
            })?;
        std::fs::create_dir_all(&staging).map_err(|error| DaemonError::fs(&staging, error))?;
        std::fs::write(staging.join("partial"), "partial clone")
            .map_err(|error| DaemonError::fs(&staging, error))?;
        self.attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(staging);
        Err(DaemonError::Git("synthetic clone failure".to_owned()))
    }

    async fn run_streaming(
        &self,
        _command: ShellCommand,
        _cancel: CancellationToken,
        _on_line: LineCallback,
    ) -> DaemonResult<ShellResult> {
        Err(DaemonError::Shell(
            "unexpected streaming command".to_owned(),
        ))
    }
}

struct CleanupFailingFiles {
    inner: RealFiles,
    fail_cleanup: AtomicBool,
}

impl CleanupFailingFiles {
    fn new(inner: RealFiles) -> Self {
        Self {
            inner,
            fail_cleanup: AtomicBool::new(true),
        }
    }
}

impl Files for CleanupFailingFiles {
    fn read_text(&self, path: &Path) -> DaemonResult<String> {
        self.inner.read_text(path)
    }

    fn create_dir_all(&self, path: &Path) -> DaemonResult<()> {
        self.inner.create_dir_all(path)
    }

    fn clone_dir(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
        self.inner.clone_dir(source, destination)
    }

    fn atomic_write_text(&self, path: &Path, text: &str) -> DaemonResult<()> {
        self.inner.atomic_write_text(path, text)
    }

    fn rename(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
        self.inner.rename(source, destination)
    }

    fn trash(&self, path: &Path) -> DaemonResult<PathBuf> {
        self.inner.trash(path)
    }

    fn remove_detached(&self, path: &Path) -> DaemonResult<()> {
        if self.fail_cleanup.swap(false, Ordering::SeqCst) {
            return Err(DaemonError::fs(
                path,
                io::Error::other("synthetic cleanup failure"),
            ));
        }
        self.inner.remove_detached(path)
    }

    fn remove_file(&self, path: &Path) -> DaemonResult<()> {
        self.inner.remove_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }

    fn list(&self, path: &Path) -> DaemonResult<Vec<PathBuf>> {
        self.inner.list(path)
    }

    fn guard_strict_descendant(&self, path: &Path) -> DaemonResult<()> {
        self.inner.guard_strict_descendant(path)
    }

    fn set_removable_roots(&self, roots: Vec<PathBuf>) {
        self.inner.set_removable_roots(roots);
    }
}

struct PublicationFiles {
    inner: RealFiles,
    state_path: PathBuf,
    final_path: PathBuf,
    fail_state_after_publish: AtomicBool,
    fail_marker_removal: AtomicBool,
    publication_observed: AtomicBool,
    publication_renamed: AtomicBool,
}

impl PublicationFiles {
    fn new(
        inner: RealFiles,
        state_path: PathBuf,
        final_path: PathBuf,
        fail_state_after_publish: bool,
        fail_marker_removal: bool,
    ) -> Self {
        Self {
            inner,
            state_path,
            final_path,
            fail_state_after_publish: AtomicBool::new(fail_state_after_publish),
            fail_marker_removal: AtomicBool::new(fail_marker_removal),
            publication_observed: AtomicBool::new(false),
            publication_renamed: AtomicBool::new(false),
        }
    }

    fn publication_observed(&self) -> bool {
        self.publication_observed.load(Ordering::SeqCst)
    }

    fn marker_path(&self) -> PathBuf {
        self.final_path
            .join(".git")
            .join(".fleet-clone-publish.json")
    }
}

impl Files for PublicationFiles {
    fn read_text(&self, path: &Path) -> DaemonResult<String> {
        self.inner.read_text(path)
    }

    fn create_dir_all(&self, path: &Path) -> DaemonResult<()> {
        self.inner.create_dir_all(path)
    }

    fn clone_dir(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
        self.inner.clone_dir(source, destination)
    }

    fn atomic_write_text(&self, path: &Path, text: &str) -> DaemonResult<()> {
        if path == self.state_path
            && self.publication_renamed.load(Ordering::SeqCst)
            && self.fail_state_after_publish.swap(false, Ordering::SeqCst)
        {
            return Err(DaemonError::fs(
                path,
                io::Error::other("synthetic publication state-save failure"),
            ));
        }
        self.inner.atomic_write_text(path, text)
    }

    fn rename(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
        if destination == self.final_path {
            let marker = source.join(".git").join(".fleet-clone-publish.json");
            if !marker.exists() {
                return Err(DaemonError::Validation(format!(
                    "clone publication marker was absent before rename: {}",
                    marker.display()
                )));
            }
            self.publication_observed.store(true, Ordering::SeqCst);
            self.inner.rename(source, destination)?;
            self.publication_renamed.store(true, Ordering::SeqCst);
            return Ok(());
        }
        self.inner.rename(source, destination)
    }

    fn trash(&self, path: &Path) -> DaemonResult<PathBuf> {
        self.inner.trash(path)
    }

    fn remove_detached(&self, path: &Path) -> DaemonResult<()> {
        self.inner.remove_detached(path)
    }

    fn remove_file(&self, path: &Path) -> DaemonResult<()> {
        if path == self.marker_path() && self.fail_marker_removal.swap(false, Ordering::SeqCst) {
            return Err(DaemonError::fs(
                path,
                io::Error::other("synthetic marker removal failure"),
            ));
        }
        self.inner.remove_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }

    fn list(&self, path: &Path) -> DaemonResult<Vec<PathBuf>> {
        self.inner.list(path)
    }

    fn guard_strict_descendant(&self, path: &Path) -> DaemonResult<()> {
        self.inner.guard_strict_descendant(path)
    }

    fn set_removable_roots(&self, roots: Vec<PathBuf>) {
        self.inner.set_removable_roots(roots);
    }
}

fn stores(temp: &tempfile::TempDir) -> (Arc<ConfigStore>, Arc<StateStore>, Arc<dyn Files>) {
    let home = temp.path().canonicalize().unwrap().join(".fleet");
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
    tokio::time::timeout(Duration::from_secs(30), jobs.wait(id))
        .await
        .expect("job did not finish")
        .expect("job remains retained")
        .status
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
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let adapters = Adapters {
        board_backends: BoardBackends::system(Arc::clone(&real_shell), Arc::clone(&clock)),
        git: Arc::new(ShellGit::new(Arc::clone(&real_shell))),
        github: Arc::new(FakeGithub::new(shell)),
        process: Arc::new(RealProcess::new(Arc::clone(&real_shell))),
        files,
        shell: real_shell,
        clock,
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

#[tokio::test]
async fn launch_crash_is_recoverable() {
    let temp = tempfile::tempdir().unwrap();
    let (config, state, files) = stores(&temp);
    let context = Contexts::new(Arc::clone(&state))
        .create("Recovery".to_owned(), vec![])
        .await
        .unwrap();
    let effective = config.load().await.unwrap();
    let final_path = PathBuf::from(&effective.repos_dir).join("acme/api");
    std::fs::create_dir_all(final_path.parent().unwrap()).unwrap();
    init_repository(&final_path);

    let repo_id = RepoId::try_from("acme/api").unwrap();
    let repo = Repo {
        id: repo_id.clone(),
        owner: "acme".to_owned(),
        name: "api".to_owned(),
        url: "git@example.test:acme/api.git".to_owned(),
        context_id: context.id.clone(),
        default_branch: "main".to_owned(),
        path: final_path.to_string_lossy().into_owned(),
        cloned_at: "2026-09-06T00:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    };
    let clone = CloneJob {
        id: repo_id.clone(),
        owner: repo.owner.clone(),
        name: repo.name.clone(),
        url: repo.url.clone(),
        context_id: context.id,
        default_branch: repo.default_branch.clone(),
        path: repo.path.clone(),
        staging_path: final_path
            .parent()
            .unwrap()
            .join("api.staging-interrupted")
            .to_string_lossy()
            .into_owned(),
        log_path: temp.path().join("clone.log").to_string_lossy().into_owned(),
        pid: None,
        started_at: "2026-09-06T00:00:00Z".to_owned(),
        status: CloneStatus::Starting,
        error: None,
    };
    state
        .transaction(move |state| {
            state.clones.push(clone);
            Ok(())
        })
        .await
        .unwrap();
    let pid_file = temp.path().join("clone.log.pid");
    std::fs::write(&pid_file, "999999\n").unwrap();
    let marker_path = final_path.join(".git").join(".fleet-clone-publish.json");
    let mut marker = serde_json::to_string_pretty(&serde_json::json!({ "repo": repo })).unwrap();
    marker.push('\n');
    files.atomic_write_text(&marker_path, &marker).unwrap();

    let jobs = Arc::new(JobManager::new(temp.path().join(".fleet")));
    let real_shell: Arc<dyn Shell> = Arc::new(RealShell);
    let git: Arc<dyn Git> = Arc::new(ShellGit::new(real_shell));
    let github: Arc<dyn Github> = Arc::new(FakeGithub::new(Arc::new(FakeShell::new())));
    let process: Arc<dyn Process> = Arc::new(FakeProcess::default());
    let repos = Repos::new(
        config,
        Arc::clone(&state),
        Arc::clone(&jobs),
        git,
        github,
        files,
        process,
    );

    repos.reconcile_startup().await.unwrap();
    let job = jobs
        .list()
        .into_iter()
        .find(|job| job.kind == JobKind::Clone)
        .unwrap();
    assert_eq!(wait_for_job(&jobs, &job.id).await, JobStatus::Succeeded);

    let recovered = state.load().await.unwrap();
    assert!(recovered.clones.is_empty());
    assert_eq!(recovered.repos, [repo]);
    assert!(!marker_path.exists());
    assert!(!pid_file.exists());
}

#[tokio::test]
async fn restart_reconciles_clone_publication() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    init_repository(&source);
    let home = temp.path().canonicalize().unwrap().join(".fleet");
    let state_path = home.join("state.json");
    let final_path = home.join("repos/acme/api");
    let publication_files = Arc::new(PublicationFiles::new(
        RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ),
        state_path,
        final_path.clone(),
        true,
        false,
    ));
    let files: Arc<dyn Files> = publication_files.clone();
    let config = Arc::new(ConfigStore::new(&home, Arc::clone(&files)));
    let state = Arc::new(StateStore::new(
        &home,
        Arc::clone(&files),
        Arc::new(SystemClock),
    ));
    let context = Contexts::new(Arc::clone(&state))
        .create("Publication recovery".to_owned(), vec![])
        .await
        .unwrap();
    let jobs = Arc::new(JobManager::new(&home));
    let real_shell: Arc<dyn Shell> = Arc::new(RealShell);
    let repos = Repos::new(
        config.clone(),
        Arc::clone(&state),
        Arc::clone(&jobs),
        Arc::new(ShellGit::new(Arc::clone(&real_shell))),
        Arc::new(FakeGithub::new(Arc::new(FakeShell::new()))),
        Arc::clone(&files),
        Arc::new(FakeProcess::default()),
    );

    let submitted = repos
        .clone_repo(
            "acme".to_owned(),
            "api".to_owned(),
            source.to_string_lossy().into_owned(),
            context.id,
            None,
        )
        .await
        .unwrap();
    let failed = wait_for_job(&jobs, &submitted.id).await;
    assert!(matches!(
        failed,
        JobStatus::Failed { ref error }
            if error.contains("published repository preserved")
                && error.contains(&final_path.to_string_lossy().into_owned())
    ));
    assert!(publication_files.publication_observed());
    let marker_path = publication_files.marker_path();
    assert!(marker_path.exists());
    assert!(!final_path.join(".fleet-clone-publish.json").exists());
    let interrupted = state.load().await.unwrap();
    assert_eq!(interrupted.clones.len(), 1);
    assert_eq!(interrupted.clones[0].status, CloneStatus::Failed);
    assert!(
        interrupted.clones[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains(&final_path.to_string_lossy().into_owned()))
    );
    assert!(interrupted.repos.is_empty());

    let restarted_jobs = Arc::new(JobManager::new(&home));
    let restarted = Repos::new(
        config,
        Arc::clone(&state),
        Arc::clone(&restarted_jobs),
        Arc::new(ShellGit::new(real_shell)),
        Arc::new(FakeGithub::new(Arc::new(FakeShell::new()))),
        files,
        Arc::new(FakeProcess::default()),
    );
    restarted.reconcile_startup().await.unwrap();
    let recovery = restarted_jobs
        .list()
        .into_iter()
        .find(|job| job.kind == JobKind::Clone)
        .unwrap();
    assert_eq!(
        wait_for_job(&restarted_jobs, &recovery.id).await,
        JobStatus::Succeeded
    );

    let recovered = state.load().await.unwrap();
    assert!(recovered.clones.is_empty());
    assert_eq!(recovered.repos.len(), 1);
    assert_eq!(recovered.repos[0].id, RepoId::try_from("acme/api").unwrap());
    assert!(!marker_path.exists());
}

#[tokio::test]
async fn destination_conflict_reports_preserved_staging() {
    let temp = tempfile::tempdir().unwrap();
    let (config, state, files) = stores(&temp);
    let context = Contexts::new(Arc::clone(&state))
        .create("Conflict recovery".to_owned(), vec![])
        .await
        .unwrap();
    let effective = config.load().await.unwrap();
    let final_path = PathBuf::from(&effective.repos_dir).join("acme/api");
    let staging = final_path.parent().unwrap().join("api.staging-preserved");
    init_repository(&final_path);
    init_repository(&staging);
    let repo_id = RepoId::try_from("acme/api").unwrap();
    let clone = CloneJob {
        id: repo_id.clone(),
        owner: "acme".to_owned(),
        name: "api".to_owned(),
        url: "git@example.test:acme/api.git".to_owned(),
        context_id: context.id,
        default_branch: "main".to_owned(),
        path: final_path.to_string_lossy().into_owned(),
        staging_path: staging.to_string_lossy().into_owned(),
        log_path: temp.path().join("clone.log").to_string_lossy().into_owned(),
        pid: Some(999_999),
        started_at: "2026-09-06T00:00:00Z".to_owned(),
        status: CloneStatus::Cloning,
        error: None,
    };
    state
        .transaction(move |state| {
            state.clones.push(clone);
            Ok(())
        })
        .await
        .unwrap();
    let jobs = Arc::new(JobManager::new(temp.path().join(".fleet")));
    let real_shell: Arc<dyn Shell> = Arc::new(RealShell);
    let repos = Repos::new(
        config,
        Arc::clone(&state),
        Arc::clone(&jobs),
        Arc::new(ShellGit::new(real_shell)),
        Arc::new(FakeGithub::new(Arc::new(FakeShell::new()))),
        files,
        Arc::new(FakeProcess::default()),
    );

    repos.reconcile_startup().await.unwrap();
    let job = jobs
        .list()
        .into_iter()
        .find(|job| job.kind == JobKind::Clone)
        .unwrap();
    assert!(matches!(
        wait_for_job(&jobs, &job.id).await,
        JobStatus::Failed { .. }
    ));
    let clone = state
        .load()
        .await
        .unwrap()
        .clones
        .into_iter()
        .find(|clone| clone.id == repo_id)
        .unwrap();
    let error = clone.error.unwrap();
    assert!(error.contains(&final_path.to_string_lossy().into_owned()));
    assert!(error.contains(&staging.to_string_lossy().into_owned()));
    assert!(staging.exists());
}

#[tokio::test]
async fn clone_marker_removal_failure_is_reported() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    init_repository(&source);
    let home = temp.path().canonicalize().unwrap().join(".fleet");
    let final_path = home.join("repos/acme/api");
    let publication_files = Arc::new(PublicationFiles::new(
        RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ),
        home.join("state.json"),
        final_path.clone(),
        false,
        true,
    ));
    let files: Arc<dyn Files> = publication_files.clone();
    let config = Arc::new(ConfigStore::new(&home, Arc::clone(&files)));
    let state = Arc::new(StateStore::new(
        &home,
        Arc::clone(&files),
        Arc::new(SystemClock),
    ));
    let context = Contexts::new(Arc::clone(&state))
        .create("Marker cleanup".to_owned(), vec![])
        .await
        .unwrap();
    let jobs = Arc::new(JobManager::new(&home));
    let real_shell: Arc<dyn Shell> = Arc::new(RealShell);
    let repos = Repos::new(
        config,
        Arc::clone(&state),
        Arc::clone(&jobs),
        Arc::new(ShellGit::new(real_shell)),
        Arc::new(FakeGithub::new(Arc::new(FakeShell::new()))),
        files,
        Arc::new(FakeProcess::default()),
    );

    let submitted = repos
        .clone_repo(
            "acme".to_owned(),
            "api".to_owned(),
            source.to_string_lossy().into_owned(),
            context.id,
            None,
        )
        .await
        .unwrap();
    let status = wait_for_job(&jobs, &submitted.id).await;
    assert!(matches!(
        status,
        JobStatus::Failed { ref error } if error.contains("synthetic marker removal failure")
    ));
    assert!(publication_files.publication_observed());
    assert!(publication_files.marker_path().exists());
    let persisted = state.load().await.unwrap();
    assert!(persisted.clones.is_empty());
    assert_eq!(persisted.repos.len(), 1);
}

#[tokio::test]
async fn retry_never_races_prior_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap().join(".fleet");
    let files: Arc<dyn Files> = Arc::new(CleanupFailingFiles::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    )));
    let config = Arc::new(ConfigStore::new(&home, Arc::clone(&files)));
    let state = Arc::new(StateStore::new(
        &home,
        Arc::clone(&files),
        Arc::new(SystemClock),
    ));
    let context = Contexts::new(Arc::clone(&state))
        .create("Retries".to_owned(), vec![])
        .await
        .unwrap();
    let jobs = Arc::new(JobManager::new(&home));
    let clone_shell = Arc::new(FailingCloneShell::default());
    let git: Arc<dyn Git> = Arc::new(ShellGit::new(Arc::clone(&clone_shell)));
    let github: Arc<dyn Github> = Arc::new(FakeGithub::new(Arc::new(FakeShell::new())));
    let process: Arc<dyn Process> = Arc::new(FakeProcess::default());
    let repos = Repos::new(
        config,
        Arc::clone(&state),
        Arc::clone(&jobs),
        git,
        github,
        files,
        process,
    );

    let first = repos
        .clone_repo(
            "acme".to_owned(),
            "api".to_owned(),
            "git@example.test:acme/api.git".to_owned(),
            context.id,
            None,
        )
        .await
        .unwrap();
    let first_status = wait_for_job(&jobs, &first.id).await;
    assert!(matches!(
        first_status,
        JobStatus::Failed { ref error }
            if error.contains("cleanup failed") && error.contains("synthetic cleanup failure")
    ));

    let retry = jobs.retry(&first.id).unwrap();
    let retry_status = wait_for_job(&jobs, &retry.id).await;
    assert!(matches!(retry_status, JobStatus::Failed { .. }));
    let attempts = clone_shell.attempts();
    assert_eq!(attempts.len(), 2);
    assert_ne!(attempts[0], attempts[1]);
    assert!(
        attempts[0].exists(),
        "failed cleanup remains isolated from retries"
    );
    assert!(
        !attempts[1].exists(),
        "successful cleanup claims its exact attempt: {}; status: {retry_status:?}",
        attempts[1].display()
    );
    let clone = state
        .load()
        .await
        .unwrap()
        .clones
        .into_iter()
        .find(|clone| clone.id == RepoId::try_from("acme/api").unwrap())
        .unwrap();
    assert_eq!(Path::new(&clone.staging_path), attempts[1]);
}
