//! Prepared worktree creation, recovery, publication, and deletion.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use fleet_core::{
    config::Config,
    ids::{JobId, RepoId, SessionId, WorktreeId},
    model::{Degraded, Repo, RepoHooks, Worktree},
    paths::{CreatingMarker, creating_marker_path, hot_marker_path, uuid_attempt_path},
    slug::slugify,
    validate::{validate_branch, validate_slug},
};
use fleet_proto::{
    job::{JobKind, JobRecord},
    response::WorktreeDeleteResult,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    DaemonError, DaemonResult,
    adapters::{
        Adapters,
        files::Files,
        git::Git,
        github::Github,
        process::pid_is_alive,
        shell::{Shell, ShellCommand},
    },
    error::remote_unsupported,
    jobs::{JobCtx, JobManager, JobPolicy},
    stores::{config::ConfigStore, state::StateStore},
};

use super::{
    awaited::{JobDelivery, copy_error},
    branch_refspec, check_cancelled, discard_path, hooks,
    pool::{CancellableCopy, Pool},
    repo_worktrees_dir,
    sessions::{Sessions, TransitionLockClaim},
};

mod creation;
mod post_create;
mod publication;
mod recovery;
mod trash;

const TRASH_MARKER_FILE: &str = "fleet-trash.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostCreateIntent {
    worktree: Worktree,
    hooks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    status_path: PathBuf,
    log_path: PathBuf,
    intent_path: PathBuf,
}

/// Worktree service owning prepared-copy claim, publication, and deletion orchestration.
#[derive(Clone)]
pub struct Worktrees {
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    jobs: Arc<JobManager>,
    files: Arc<dyn Files>,
    git: Arc<dyn Git>,
    shell: Arc<dyn Shell>,
    github: Arc<dyn Github>,
    sessions: Sessions,
    pool: Pool,
    trash_jobs: Arc<Mutex<HashMap<String, JobId>>>,
    startup_ready: Arc<AtomicBool>,
    startup_notify: Arc<tokio::sync::Notify>,
}

impl Worktrees {
    /// Creates the worktree service and schedules startup intent recovery. Creation drives
    /// files, Git, the shell, and GitHub, so it takes the shared adapter bundle.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        adapters: &Adapters,
        sessions: Sessions,
    ) -> Self {
        let service = Self {
            pool: Pool::new(
                Arc::clone(&config),
                Arc::clone(&state),
                Arc::clone(&jobs),
                Arc::clone(&adapters.git),
                Arc::clone(&adapters.files),
                Arc::clone(&adapters.shell),
            ),
            config,
            state,
            jobs,
            files: Arc::clone(&adapters.files),
            git: Arc::clone(&adapters.git),
            shell: Arc::clone(&adapters.shell),
            github: Arc::clone(&adapters.github),
            sessions,
            trash_jobs: Arc::new(Mutex::new(HashMap::new())),
            startup_ready: Arc::new(AtomicBool::new(false)),
            startup_notify: Arc::new(tokio::sync::Notify::new()),
        };
        service.schedule_startup_recovery();
        service
    }

    /// Hard-kills the session associated with a worktree.
    pub async fn kill(&self, id: WorktreeId) -> DaemonResult<()> {
        let state = self.state.load().await?;
        let worktree = state
            .worktrees
            .iter()
            .find(|worktree| worktree.id == id)
            .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
        if worktree.host.is_some() {
            return Err(remote_unsupported());
        }
        self.kill_session(&worktree.session).await
    }

    /// Kills a worktree's session, treating an already-gone session as success.
    async fn kill_session(&self, session: &str) -> DaemonResult<()> {
        let session = SessionId::try_from(session)
            .map_err(|error| DaemonError::Validation(error.to_string()))?;
        match self.sessions.kill(session).await {
            Ok(()) | Err(DaemonError::NotFound(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Updates `lastOpenedAt` transactionally without changing worktree identity.
    pub async fn touch_opened(&self, id: WorktreeId) -> DaemonResult<()> {
        let now = Utc::now().to_rfc3339();
        self.state
            .transaction(move |state| {
                let worktree = state
                    .worktrees
                    .iter_mut()
                    .find(|worktree| worktree.id == id)
                    .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
                worktree.last_opened_at = Some(now);
                Ok(())
            })
            .await
    }

    /// Resolves the absolute path of a local worktree and rejects remote mirrors.
    pub async fn path(&self, id: WorktreeId) -> DaemonResult<String> {
        let state = self.state.load().await?;
        let worktree = state
            .worktrees
            .into_iter()
            .find(|worktree| worktree.id == id)
            .ok_or_else(|| DaemonError::NotFound(format!("worktree {id}")))?;
        if worktree.host.is_some() {
            return Err(remote_unsupported());
        }
        let path = PathBuf::from(&worktree.path);
        if !path.is_absolute() {
            return Err(DaemonError::Validation(format!(
                "worktree {id} has a non-absolute path"
            )));
        }
        Ok(path.to_string_lossy().into_owned())
    }

    async fn repo_and_config(&self, id: &RepoId) -> DaemonResult<(Repo, Config)> {
        let config = self.config.load().await?;
        let state = self.state.load().await?;
        let repo = state
            .repos
            .into_iter()
            .find(|repo| &repo.id == id)
            .ok_or_else(|| DaemonError::NotFound(format!("repository {id}")))?;
        Ok((repo, config))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrashMarker {
    original_path: String,
    worktree: Worktree,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<String>,
}

struct AttemptGuard {
    files: Arc<dyn Files>,
    path: PathBuf,
    coordination: Arc<tokio::sync::Mutex<()>>,
    context: JobCtx,
    armed: bool,
}

impl AttemptGuard {
    fn new(files: Arc<dyn Files>, path: PathBuf, context: &JobCtx) -> Self {
        Self {
            files,
            path,
            coordination: Arc::new(tokio::sync::Mutex::new(())),
            context: context.clone(),
            armed: true,
        }
    }

    fn coordination(&self) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(&self.coordination)
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let files = Arc::clone(&self.files);
        let path = self.path.clone();
        let coordination = Arc::clone(&self.coordination);
        self.context.track_cleanup(async move {
            let _guard = coordination.lock().await;
            let _ignored = discard_path(files.as_ref(), &path);
        });
    }
}

fn trash_marker_path(root: &Path) -> PathBuf {
    root.join(".git").join(TRASH_MARKER_FILE)
}

fn validate_restore_marker(marker: &TrashMarker, config: &Config) -> DaemonResult<()> {
    if marker.worktree.host.is_some() {
        return Err(remote_unsupported());
    }
    let expected = repo_worktrees_dir(config, &marker.worktree.repo_id).join(&marker.worktree.slug);
    if Path::new(&marker.original_path) != expected || marker.worktree.path != marker.original_path
    {
        return Err(DaemonError::Validation(
            "trash entry does not contain a canonical worktree path".to_owned(),
        ));
    }
    Ok(())
}

fn validate_trash_entry(entry: &str) -> DaemonResult<()> {
    if entry.is_empty()
        || matches!(entry, "." | "..")
        || Path::new(entry).file_name().and_then(|name| name.to_str()) != Some(entry)
    {
        return Err(DaemonError::Validation(
            "trash entry must be one directory name".to_owned(),
        ));
    }
    Ok(())
}

fn worktree_id(repo: &RepoId, slug: &str) -> DaemonResult<WorktreeId> {
    WorktreeId::try_from(format!("{repo}#{slug}"))
        .map_err(|error| DaemonError::Validation(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    use fleet_core::{ids::ContextId, model::Context, state::default_state};
    use fleet_proto::job::JobStatus;

    use super::post_create::{RECOVERED_RUNNER_TIMEOUT, runner_pid_path};

    use crate::{
        adapters::{
            clock::SystemClock,
            files::RealFiles,
            git::ShellGit,
            github::GhCli,
            shell::{Shell, ShellResult},
        },
        testing::fakes::{FakeFiles, FakeFilesCall, FakeProcess, FakeShell, FakeShellCall},
    };

    struct Fixture {
        _temp: tempfile::TempDir,
        home: PathBuf,
        files: Arc<FakeFiles>,
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        jobs: Arc<JobManager>,
        shell: Arc<FakeShell>,
        service: Worktrees,
        repo: Repo,
    }

    struct BlockingFiles {
        inner: RealFiles,
        started: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl Files for BlockingFiles {
        fn read_text(&self, path: &Path) -> DaemonResult<String> {
            self.inner.read_text(path)
        }

        fn create_dir_all(&self, path: &Path) -> DaemonResult<()> {
            self.inner.create_dir_all(path)
        }

        fn clone_dir(&self, source: &Path, destination: &Path) -> DaemonResult<()> {
            if let Some(started) = self
                .started
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                let _ignored = started.send(());
                self.release
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv()
                    .map_err(|error| DaemonError::Join(error.to_string()))?;
            }
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

    async fn fixture() -> Fixture {
        let temp = tempfile::tempdir().expect("temporary home");
        let home = temp.path().join("fleet");
        let files = Arc::new(FakeFiles::new(
            home.join("trash"),
            vec![home.join("worktrees"), home.join("repos")],
        ));
        let files_boundary: Arc<dyn Files> = files.clone();
        let config = Arc::new(ConfigStore::new(&home, files_boundary.clone()));
        config
            .update(serde_json::json!({
                "reposDir": home.join("repos"),
                "worktreesDir": home.join("worktrees"),
                "hotPoolSize": 0,
                "hotRefreshIntervalMs": 0,
                "trash": {"retentionMs": 600000}
            }))
            .await
            .expect("test config");
        let state = Arc::new(StateStore::new(
            &home,
            files_boundary.clone(),
            Arc::new(SystemClock),
        ));
        let repo = Repo {
            id: RepoId::try_from("acme/api").expect("repo id"),
            owner: "acme".to_owned(),
            name: "api".to_owned(),
            url: "https://example.invalid/acme/api".to_owned(),
            context_id: ContextId::try_from("acme").expect("context id"),
            default_branch: "main".to_owned(),
            path: home.join("repos/acme/api").to_string_lossy().into_owned(),
            cloned_at: "2026-09-04T00:00:00Z".to_owned(),
            hooks: RepoHooks::default(),
        };
        let mut persisted = default_state();
        persisted.contexts.push(Context {
            id: ContextId::try_from("acme").expect("context id"),
            name: "Acme".to_owned(),
            owners: vec!["acme".to_owned()],
            created_at: "2026-09-04T00:00:00Z".to_owned(),
        });
        persisted.repos.push(repo.clone());
        state.save(persisted).await.expect("test state");
        let shell = Arc::new(FakeShell::new());
        shell.when(
            |command| command.program == "sh",
            ShellResult {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let shell_boundary: Arc<dyn Shell> = shell.clone();
        let clock: Arc<dyn crate::adapters::clock::Clock> =
            Arc::new(crate::adapters::clock::SystemClock);
        let adapters = Adapters {
            board_backends: crate::adapters::board::BoardBackends::system(
                shell_boundary.clone(),
                Arc::clone(&clock),
            ),
            git: Arc::new(ShellGit::new(shell_boundary.clone())),
            github: Arc::new(GhCli::new(shell_boundary.clone())),
            process: Arc::new(FakeProcess::default()),
            files: files_boundary,
            shell: shell_boundary,
            clock,
        };
        let jobs = Arc::new(JobManager::new(&home));
        let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
        let service = Worktrees {
            pool: Pool::new(
                Arc::clone(&config),
                Arc::clone(&state),
                Arc::clone(&jobs),
                Arc::clone(&adapters.git),
                Arc::clone(&adapters.files),
                Arc::clone(&adapters.shell),
            ),
            config: Arc::clone(&config),
            state: Arc::clone(&state),
            jobs: Arc::clone(&jobs),
            files: Arc::clone(&adapters.files),
            git: Arc::clone(&adapters.git),
            shell: Arc::clone(&adapters.shell),
            github: Arc::clone(&adapters.github),
            sessions,
            trash_jobs: Arc::new(Mutex::new(HashMap::new())),
            startup_ready: Arc::new(AtomicBool::new(true)),
            startup_notify: Arc::new(tokio::sync::Notify::new()),
        };
        Fixture {
            _temp: temp,
            home,
            files,
            config,
            state,
            jobs,
            shell,
            service,
            repo,
        }
    }

    #[tokio::test]
    async fn cancelled_create_cleans_attempt_before_terminal_status() {
        let temp = tempfile::tempdir().expect("temporary home");
        let source = temp.path().join("repos/acme/api");
        let attempt = temp.path().join("worktrees/acme/api/feature.creating-test");
        std::fs::create_dir_all(&source).expect("source directory");
        std::fs::write(source.join("README.md"), "fleet\n").expect("source file");
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let files: Arc<dyn Files> = Arc::new(BlockingFiles {
            inner: RealFiles::new(
                temp.path().join("trash"),
                [temp.path().join("worktrees"), temp.path().join("repos")],
            ),
            started: Mutex::new(Some(started_tx)),
            release: Mutex::new(release_rx),
        });
        let manager = JobManager::new(temp.path());
        manager.set_cancellation_grace(Duration::ZERO);
        let operation_files = Arc::clone(&files);
        let operation_source = source.clone();
        let operation_attempt = attempt.clone();
        let id = manager.submit(
            JobKind::CreateWorktree,
            "acme/api#feature",
            "Create acme/api#feature",
            true,
            false,
            move |context| {
                let files = Arc::clone(&operation_files);
                let source = operation_source.clone();
                let attempt = operation_attempt.clone();
                async move {
                    let attempt_guard =
                        AttemptGuard::new(Arc::clone(&files), attempt.clone(), &context);
                    let coordination = attempt_guard.coordination().lock_owned().await;
                    let copy = CancellableCopy::start_coordinated(
                        files,
                        source,
                        attempt,
                        None,
                        coordination,
                        Some(context),
                    );
                    let _attempt_guard = attempt_guard;
                    let _copy = copy;
                    std::future::pending::<DaemonResult<()>>().await
                }
            },
        );
        tokio::task::spawn_blocking(move || started_rx.recv())
            .await
            .expect("copy-start waiter")
            .expect("copy starts");

        manager.cancel(&id).expect("cancel create");
        release_tx.send(()).expect("release copy");
        let record = manager
            .wait(&id)
            .await
            .expect("create reaches terminal state");

        assert_eq!(record.status, JobStatus::Cancelled);
        assert!(!attempt.exists());
    }

    fn worktree(fixture: &Fixture, slug: &str) -> Worktree {
        Worktree {
            id: worktree_id(&fixture.repo.id, slug).expect("worktree id"),
            repo_id: fixture.repo.id.clone(),
            slug: slug.to_owned(),
            branch: slug.to_owned(),
            base_ref: "origin/main".to_owned(),
            path: fixture
                .home
                .join("worktrees/acme/api")
                .join(slug)
                .to_string_lossy()
                .into_owned(),
            session: format!("api/{slug}"),
            host: None,
            created_at: "2026-09-04T00:00:00Z".to_owned(),
            last_opened_at: None,
            degraded: None,
        }
    }

    #[test]
    fn rejects_paths_as_trash_entry_names() {
        assert!(validate_trash_entry("123-feature").is_ok());
        assert!(validate_trash_entry("../123-feature").is_err());
        assert!(validate_trash_entry(".").is_err());
    }

    #[tokio::test]
    async fn all_worktree_transitions_serialize() {
        let fixture = fixture().await;
        let worktree = worktree(&fixture, "restore-race");
        let marker = TrashMarker {
            original_path: worktree.path.clone(),
            worktree: worktree.clone(),
            expires_at: Some("2026-09-05T00:00:00Z".to_owned()),
        };
        let first_trash = fixture.home.join("trash/restore-race-first");
        let second_trash = fixture.home.join("trash/restore-race-second");
        let marker_text = serde_json::to_string(&marker).expect("marker");
        fixture
            .files
            .insert_text(trash_marker_path(&first_trash), marker_text.clone());
        fixture
            .files
            .insert_text(trash_marker_path(&second_trash), marker_text);
        fixture
            .files
            .create_dir_all(Path::new(&worktree.path).parent().expect("worktree parent"))
            .expect("worktree root");

        let (first, second) = tokio::join!(
            fixture
                .service
                .restore_trash("restore-race-first".to_owned()),
            fixture
                .service
                .restore_trash("restore-race-second".to_owned()),
        );

        first.expect("first restore commits");
        assert!(
            matches!(second, Err(DaemonError::Conflict(ref message)) if message.starts_with("restore destination already exists:")),
            "the second transition must revalidate after the first commits: {second:?}"
        );
        assert!(fixture.files.exists(Path::new(&worktree.path)));
        assert!(fixture.files.exists(&second_trash));
        assert_eq!(
            fixture
                .state
                .load()
                .await
                .expect("state")
                .worktrees
                .iter()
                .filter(|item| item.id == worktree.id)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn worktree_lifecycle_claims_serialize() {
        let fixture = fixture().await;
        let id = worktree_id(&fixture.repo.id, "feature").expect("worktree id");
        let first = fixture
            .service
            .sessions
            .claim_worktree_lifecycle(id.clone())
            .await;
        let mut second = Box::pin(fixture.service.sessions.claim_worktree_lifecycle(id));
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);

        assert!(matches!(
            std::future::Future::poll(second.as_mut(), &mut context),
            std::task::Poll::Pending
        ));
        drop(first);
        second.await;
    }

    #[tokio::test]
    async fn restore_survives_marker_cleanup_failure() {
        let fixture = fixture().await;
        let worktree = worktree(&fixture, "restore");
        let trash = fixture.home.join("trash/restore-entry");
        let marker = TrashMarker {
            original_path: worktree.path.clone(),
            worktree: worktree.clone(),
            expires_at: Some("2026-09-05T00:00:00Z".to_owned()),
        };
        fixture.files.insert_text(
            trash_marker_path(&trash),
            serde_json::to_string(&marker).expect("marker"),
        );
        fixture
            .files
            .create_dir_all(Path::new(&worktree.path).parent().expect("worktree parent"))
            .expect("worktree root");
        let destination_marker = trash_marker_path(Path::new(&worktree.path));
        fixture.files.fail_next(
            FakeFilesCall::Remove(destination_marker.clone()),
            std::io::ErrorKind::PermissionDenied,
        );

        fixture
            .service
            .restore_trash("restore-entry".to_owned())
            .await
            .expect("committed restore must succeed");

        assert!(fixture.files.exists(Path::new(&worktree.path)));
        assert!(fixture.files.exists(&destination_marker));
        assert!(
            fixture
                .state
                .load()
                .await
                .expect("state")
                .worktrees
                .iter()
                .any(|item| item.id == worktree.id)
        );
    }

    #[tokio::test]
    async fn trash_expiry_survives_restart() {
        let fixture = fixture().await;
        let worktree = worktree(&fixture, "expired");
        let trash = fixture.home.join("trash/expired-entry");
        let marker = TrashMarker {
            original_path: worktree.path.clone(),
            worktree,
            expires_at: Some("2000-01-01T00:00:00Z".to_owned()),
        };
        fixture.files.insert_text(
            trash_marker_path(&trash),
            serde_json::to_string(&marker).expect("marker"),
        );
        let config = fixture.config.load().await.expect("config");

        fixture
            .service
            .reconcile_trash_expiries(&config)
            .expect("reconcile expiry");
        let job = fixture
            .service
            .trash_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get("expired-entry")
            .cloned()
            .expect("cleanup job");
        let record = tokio::time::timeout(Duration::from_secs(2), fixture.jobs.wait(&job))
            .await
            .expect("cleanup deadline")
            .expect("cleanup completion");

        assert!(matches!(record.status, JobStatus::Succeeded));
        assert!(!fixture.files.exists(&trash));
    }

    #[tokio::test]
    async fn post_create_crash_is_recoverable() {
        let fixture = fixture().await;
        let worktree = worktree(&fixture, "pending-hooks");
        let mut state = fixture.state.load().await.expect("state");
        state.worktrees.push(worktree.clone());
        fixture.state.save(state).await.expect("state save");
        let intent_path = fixture.home.join("cache/post-create/pending.json");
        let intent = PostCreateIntent {
            worktree,
            hooks: vec!["true".to_owned()],
            pid: None,
            status_path: fixture.home.join("jobs/pending.post-create.status"),
            log_path: fixture.home.join("jobs/pending.log"),
            intent_path: intent_path.clone(),
        };
        fixture.files.insert_text(
            &intent_path,
            serde_json::to_string(&intent).expect("intent"),
        );
        let mut updates = fixture.jobs.subscribe();

        fixture
            .service
            .recover_post_create_intents()
            .await
            .expect("recover intents");
        let job = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let record = updates.recv().await.expect("job update");
                if matches!(record.kind, JobKind::PostCreateHooks) {
                    break record.id;
                }
            }
        })
        .await
        .expect("hook job deadline");
        let _record = tokio::time::timeout(Duration::from_secs(2), fixture.jobs.wait(&job))
            .await
            .expect("hook completion deadline")
            .expect("hook job completion");

        assert!(
            fixture
                .shell
                .calls()
                .iter()
                .any(|call| matches!(call, FakeShellCall::Detached { .. }))
        );
        assert!(!fixture.files.exists(&intent_path));
        let recovered = fixture.state.load().await.expect("state");
        assert_eq!(
            recovered
                .worktrees
                .iter()
                .find(|item| item.id == intent.worktree.id)
                .and_then(|item| item.degraded.as_ref())
                .map(|degraded| degraded.step.as_str()),
            Some("detached hook runner did not record completion")
        );
    }

    #[tokio::test]
    async fn interrupted_post_create_launch_is_not_relaunched() {
        let fixture = fixture().await;
        let worktree = worktree(&fixture, "interrupted-hooks");
        let mut state = fixture.state.load().await.expect("state");
        state.worktrees.push(worktree.clone());
        fixture.state.save(state).await.expect("state save");
        let intent = PostCreateIntent {
            worktree,
            hooks: vec!["true".to_owned()],
            pid: None,
            status_path: fixture.home.join("jobs/interrupted.post-create.status"),
            log_path: fixture.home.join("jobs/interrupted.log"),
            intent_path: fixture.home.join("cache/post-create/interrupted.json"),
        };
        // The runner published its own identifier and the daemon died before the intent
        // recorded one, so the user's hooks are already running. `u32::MAX` is outside the
        // identifier range, so the reconciliation observes an exited runner rather than
        // waiting on a live one.
        let pid_path = runner_pid_path(&intent.status_path);
        fixture
            .files
            .insert_text(&pid_path, format!("{}\n", u32::MAX));
        fixture.files.insert_text(
            &intent.intent_path,
            serde_json::to_string(&intent).expect("intent"),
        );
        let mut updates = fixture.jobs.subscribe();

        fixture
            .service
            .recover_post_create_intents()
            .await
            .expect("recover intents");
        let job = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let record = updates.recv().await.expect("job update");
                if matches!(record.kind, JobKind::PostCreateHooks) {
                    break record.id;
                }
            }
        })
        .await
        .expect("hook job deadline");
        let _record = tokio::time::timeout(Duration::from_secs(2), fixture.jobs.wait(&job))
            .await
            .expect("hook completion deadline")
            .expect("hook job completion");

        assert!(
            !fixture
                .shell
                .calls()
                .iter()
                .any(|call| matches!(call, FakeShellCall::Detached { .. })),
            "a runner that already started must never have its hooks run a second time"
        );
        assert!(!fixture.files.exists(&pid_path));
        assert!(!fixture.files.exists(&intent.intent_path));
    }

    #[tokio::test(start_paused = true)]
    async fn recovered_post_create_stops_waiting_on_a_foreign_pid() {
        let fixture = fixture().await;
        let worktree = worktree(&fixture, "foreign-pid-hooks");
        let mut state = fixture.state.load().await.expect("state");
        state.worktrees.push(worktree.clone());
        fixture.state.save(state).await.expect("state save");
        let intent = PostCreateIntent {
            worktree: worktree.clone(),
            // Process identifier 1 is always alive and is never ours: after a reboot the
            // identifier a previous boot recorded can belong to any process at all.
            pid: Some(1),
            hooks: vec!["true".to_owned()],
            status_path: fixture.home.join("jobs/foreign.post-create.status"),
            log_path: fixture.home.join("jobs/foreign.log"),
            intent_path: fixture.home.join("cache/post-create/foreign.json"),
        };
        fixture.files.insert_text(
            &intent.intent_path,
            serde_json::to_string(&intent).expect("intent"),
        );
        let mut updates = fixture.jobs.subscribe();

        fixture
            .service
            .recover_post_create_intents()
            .await
            .expect("recover intents");
        let job = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let record = updates.recv().await.expect("job update");
                if matches!(record.kind, JobKind::PostCreateHooks) {
                    break record.id;
                }
            }
        })
        .await
        .expect("hook job deadline");
        // The clock is paused, so this deadline outlasts the bound the wait must honour
        // without the test spending it.
        let _record = tokio::time::timeout(RECOVERED_RUNNER_TIMEOUT * 4, fixture.jobs.wait(&job))
            .await
            .expect("waiting on a recovered runner must be bounded")
            .expect("hook job completion");

        let recovered = fixture.state.load().await.expect("state");
        assert_eq!(
            recovered
                .worktrees
                .iter()
                .find(|item| item.id == worktree.id)
                .and_then(|item| item.degraded.as_ref())
                .map(|degraded| degraded.step.as_str()),
            Some("detached hook runner did not record completion")
        );
        assert!(!fixture.files.exists(&intent.intent_path));
    }

    #[tokio::test]
    async fn successful_retry_clears_degradation() {
        let fixture = fixture().await;
        let mut worktree = worktree(&fixture, "recovered-hooks");
        worktree.degraded = Some(Degraded {
            kind: "post_create_hooks".to_owned(),
            step: "hook 1: false".to_owned(),
            exit_code: Some(1),
            at: "2026-09-04T00:00:00Z".to_owned(),
            log_path: "/old.log".to_owned(),
        });
        let mut state = fixture.state.load().await.expect("state");
        state.worktrees.push(worktree.clone());
        fixture.state.save(state).await.expect("state save");
        let intent = PostCreateIntent {
            worktree: worktree.clone(),
            hooks: vec!["true".to_owned()],
            pid: None,
            status_path: fixture.home.join("jobs/retry.post-create.status"),
            log_path: fixture.home.join("jobs/retry.log"),
            intent_path: fixture.home.join("cache/post-create/retry.json"),
        };
        fixture.files.insert_text(&intent.status_path, "0 0\n");
        fixture.files.insert_text(
            &intent.intent_path,
            serde_json::to_string(&intent).expect("intent"),
        );

        fixture
            .service
            .finish_detached_post_create(intent)
            .await
            .expect("successful retry");

        let recovered = fixture.state.load().await.expect("state");
        assert!(
            recovered
                .worktrees
                .iter()
                .find(|item| item.id == worktree.id)
                .expect("worktree")
                .degraded
                .is_none()
        );

        fixture
            .state
            .transaction({
                let id = worktree.id.clone();
                move |state| {
                    state.worktrees.retain(|item| item.id != id);
                    Ok(())
                }
            })
            .await
            .expect("delete worktree while hooks finish");
        let deleted_intent = PostCreateIntent {
            worktree,
            hooks: vec!["true".to_owned()],
            pid: None,
            status_path: fixture.home.join("jobs/deleted.post-create.status"),
            log_path: fixture.home.join("jobs/deleted.log"),
            intent_path: fixture.home.join("cache/post-create/deleted.json"),
        };
        fixture
            .files
            .insert_text(&deleted_intent.status_path, "0 0\n");
        fixture.files.insert_text(
            &deleted_intent.intent_path,
            serde_json::to_string(&deleted_intent).expect("intent"),
        );

        fixture
            .service
            .finish_detached_post_create(deleted_intent.clone())
            .await
            .expect("a deleted worktree has no degradation to clear");

        assert!(!fixture.files.exists(&deleted_intent.status_path));
        assert!(!fixture.files.exists(&deleted_intent.intent_path));
    }

    #[tokio::test]
    async fn observation_failure_never_deletes() {
        let fixture = fixture().await;
        let attempt = fixture
            .home
            .join("worktrees/acme/api/uncertain.creating-attempt");
        let marker = CreatingMarker {
            id: "acme/api#uncertain".to_owned(),
            repo_id: fixture.repo.id.clone(),
            branch: "uncertain".to_owned(),
            base_ref: "origin/main".to_owned(),
            created_at: "2026-09-04T00:00:00Z".to_owned(),
        };
        fixture.files.insert_text(
            creating_marker_path(&attempt),
            serde_json::to_string(&marker).expect("marker"),
        );
        let config = fixture.config.load().await.expect("config");

        fixture
            .service
            .recover_attempt(&config, &fixture.repo, "uncertain", &attempt)
            .await
            .expect("observation failure is quarantined");

        assert!(!fixture.files.exists(&attempt));
        assert!(
            fixture
                .files
                .exists(&fixture.home.join("trash/0-uncertain.creating-attempt"))
        );
        let quarantined = fixture.home.join("trash/0-uncertain.creating-attempt");
        let marker: TrashMarker = serde_json::from_str(
            &fixture
                .files
                .read_text(&trash_marker_path(&quarantined))
                .expect("quarantine marker"),
        )
        .expect("valid quarantine marker");
        assert_eq!(marker.worktree.id.as_str(), "acme/api#uncertain");
        assert!(marker.expires_at.is_some());
        assert!(
            fixture
                .service
                .trash_jobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key("0-uncertain.creating-attempt")
        );
        assert!(!fixture
            .files
            .calls()
            .iter()
            .any(|call| matches!(call, FakeFilesCall::Remove(path) if path.ends_with("uncertain.creating-attempt"))));
    }

    #[tokio::test]
    async fn startup_recovery_survives_one_unobservable_worktree() {
        let fixture = fixture().await;
        let root = fixture.home.join("worktrees/acme/api");
        for slug in ["broken", "good"] {
            let marker = CreatingMarker {
                id: format!("acme/api#{slug}"),
                repo_id: fixture.repo.id.clone(),
                branch: slug.to_owned(),
                base_ref: "origin/main".to_owned(),
                created_at: "2026-09-04T00:00:00Z".to_owned(),
            };
            fixture.files.insert_text(
                creating_marker_path(root.join(slug)),
                serde_json::to_string(&marker).expect("marker"),
            );
        }
        // Only the second worktree answers `git branch --show-current`; the first leaves the
        // fake unmatched, which is the transient observation failure a scan must survive.
        let observable = root.join("good");
        fixture.shell.when(
            move |command| command.cwd.as_deref() == Some(observable.as_path()),
            ShellResult {
                status: 0,
                stdout: "good\n".to_owned(),
                stderr: String::new(),
            },
        );
        let intent_path = fixture.home.join("cache/post-create/orphan.json");
        let intent = PostCreateIntent {
            worktree: worktree(&fixture, "orphan"),
            hooks: vec!["true".to_owned()],
            pid: None,
            status_path: fixture.home.join("jobs/orphan.post-create.status"),
            log_path: fixture.home.join("jobs/orphan.log"),
            intent_path: intent_path.clone(),
        };
        fixture.files.insert_text(
            &intent_path,
            serde_json::to_string(&intent).expect("intent"),
        );

        fixture
            .service
            .recover_startup()
            .await
            .expect("an unobservable worktree never aborts the scan");

        let state = fixture.state.load().await.expect("state");
        assert!(state.worktrees.iter().any(|item| item.slug == "good"));
        assert!(!state.worktrees.iter().any(|item| item.slug == "broken"));
        // The scan reaches its last step, so a detached hook intent is still reconciled.
        assert!(!fixture.files.exists(&intent_path));
    }
}
