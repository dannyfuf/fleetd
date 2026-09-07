use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use fleet_core::{
    ids::{ContextId, RepoId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    state::default_state,
};
use fleet_daemon::{
    DaemonResult,
    adapters::shell::{DetachedProcess, LineCallback, Shell, ShellCommand, ShellResult},
    jobs::JobManager,
    services::{
        inspect::Inspect,
        prune::{Prune, WorktreeDeleter},
        sessions::{Sessions, TransitionLockClaim},
    },
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::{FakeFiles, FakeGit, FakeGithub, FakeShell, FixedClock},
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct RecordingDeleter {
    deleted: Mutex<Vec<WorktreeId>>,
    pause: Option<(Arc<tokio::sync::Barrier>, Arc<tokio::sync::Barrier>)>,
}

#[async_trait]
impl WorktreeDeleter for RecordingDeleter {
    async fn delete(
        &self,
        id: WorktreeId,
        lifecycle: Option<TransitionLockClaim>,
    ) -> DaemonResult<()> {
        assert!(
            lifecycle.is_some(),
            "prune must transfer its lifecycle claim"
        );
        self.deleted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(id);
        if let Some((entered, release)) = &self.pause {
            entered.wait().await;
            release.wait().await;
        }
        Ok(())
    }
}

struct DirtyOnSecondStatus {
    inner: FakeShell,
    status_calls: AtomicUsize,
}

#[async_trait]
impl Shell for DirtyOnSecondStatus {
    async fn run(&self, command: ShellCommand) -> DaemonResult<ShellResult> {
        if command.program == "git"
            && command
                .args
                .first()
                .is_some_and(|argument| argument == "status")
        {
            let call = self.status_calls.fetch_add(1, Ordering::SeqCst);
            return Ok(success(if call == 0 { "" } else { "?? changed.txt" }));
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
async fn commit_never_expands_reviewed_set() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet");
    let files = Arc::new(FakeFiles::new(
        home.join("trash"),
        vec![home.join("repos"), home.join("worktrees")],
    ));
    let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
    let state = Arc::new(StateStore::new(&home, files.clone(), clock.clone()));
    let jobs = Arc::new(JobManager::with_clock(&home, clock));
    let config = Arc::new(ConfigStore::new(&home, files.clone()));
    let repo_id = RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}"));
    let context_id = ContextId::try_from("acme").unwrap_or_else(|error| panic!("{error}"));
    let worktree_id =
        WorktreeId::try_from("acme/api#done").unwrap_or_else(|error| panic!("{error}"));
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
        url: "git@github.com:acme/api.git".to_owned(),
        context_id,
        default_branch: "main".to_owned(),
        path: home.join("repos/acme/api").display().to_string(),
        cloned_at: "2026-09-04T00:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    });
    persisted.worktrees.push(Worktree {
        id: worktree_id.clone(),
        repo_id: repo_id.clone(),
        slug: "done".to_owned(),
        branch: "done".to_owned(),
        base_ref: "origin/main".to_owned(),
        path: home.join("worktrees/acme/api/done").display().to_string(),
        session: "api/done".to_owned(),
        host: None,
        created_at: "2026-09-04T00:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    });
    persisted.worktrees.push(Worktree {
        id: WorktreeId::try_from("acme/api#newly-eligible")
            .unwrap_or_else(|error| panic!("{error}")),
        repo_id,
        slug: "newly-eligible".to_owned(),
        branch: "newly-eligible".to_owned(),
        base_ref: "origin/main".to_owned(),
        path: home
            .join("worktrees/acme/api/newly-eligible")
            .display()
            .to_string(),
        session: "api/newly-eligible".to_owned(),
        host: None,
        created_at: "2026-09-04T00:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    });
    state
        .save(persisted)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let shell = Arc::new(FakeShell::new());
    shell.when(|command| command.program == "gh", success("[]"));
    shell.when(
        |command| command.args == ["branch", "--show-current"],
        success("done"),
    );
    shell.when(
        |command| {
            command
                .args
                .starts_with(&["rev-parse".to_owned(), "--verify".to_owned()])
                && command
                    .args
                    .last()
                    .is_some_and(|argument| argument == "HEAD")
        },
        success("abc"),
    );
    shell.when(
        |command| {
            command
                .args
                .starts_with(&["rev-parse".to_owned(), "--verify".to_owned()])
                && command
                    .args
                    .last()
                    .is_some_and(|argument| argument == "origin/main")
        },
        success("def"),
    );
    shell.when(
        |command| {
            command
                .args
                .first()
                .is_some_and(|argument| argument == "status")
        },
        success(""),
    );
    shell.when(
        |command| {
            command
                .args
                .first()
                .is_some_and(|argument| argument == "for-each-ref")
        },
        success("origin/done\0"),
    );
    shell.when(
        |command| command.args.contains(&"--left-right".to_owned()),
        success("0 0"),
    );
    shell.when(
        |command| command.args.contains(&"--count".to_owned()),
        success("0"),
    );
    shell.when(|command| command.program == "git", success(""));
    let git = Arc::new(FakeGit::new(shell.clone()));
    let github = Arc::new(FakeGithub::new(shell));
    let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
    let inspect = Inspect::new(state, jobs.clone(), git, github, sessions.clone());
    let entered = Arc::new(tokio::sync::Barrier::new(2));
    let release = Arc::new(tokio::sync::Barrier::new(2));
    let deleter = Arc::new(RecordingDeleter {
        deleted: Mutex::default(),
        pause: Some((Arc::clone(&entered), Arc::clone(&release))),
    });
    let prune = Prune::new(jobs, inspect, sessions.clone(), deleter.clone());
    let reviewed = worktree_id.clone();
    let pruning = tokio::spawn(async move {
        prune
            .worktrees(false, false, false, None, Some(vec![reviewed]))
            .await
    });
    entered.wait().await;
    let mut competing = tokio::spawn({
        let sessions = sessions.clone();
        let worktree_id = worktree_id.clone();
        async move { sessions.ensure(Some(worktree_id), None, false).await }
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut competing)
            .await
            .is_err(),
        "competing lifecycle transition entered during prune deletion"
    );
    release.wait().await;
    let result = pruning
        .await
        .unwrap_or_else(|error| panic!("prune task: {error}"))
        .unwrap_or_else(|error| panic!("{error}"));
    let _competing = tokio::time::timeout(std::time::Duration::from_secs(2), competing)
        .await
        .unwrap_or_else(|_| panic!("competing transition stayed blocked"))
        .unwrap_or_else(|error| panic!("competing task: {error}"));
    assert_eq!(result.deleted, vec![worktree_id.clone()]);
    assert!(result.skipped.is_empty());
    assert_eq!(
        deleter
            .deleted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &[worktree_id]
    );
}

#[tokio::test]
async fn newly_dirty_candidate_is_not_deleted() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet");
    let files = Arc::new(FakeFiles::new(
        home.join("trash"),
        vec![home.join("repos"), home.join("worktrees")],
    ));
    let clock = Arc::new(FixedClock::new(chrono::Utc::now()));
    let state = Arc::new(StateStore::new(&home, files.clone(), clock.clone()));
    let jobs = Arc::new(JobManager::with_clock(&home, clock));
    let config = Arc::new(ConfigStore::new(&home, files.clone()));
    let repo_id = RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}"));
    let context_id = ContextId::try_from("acme").unwrap_or_else(|error| panic!("{error}"));
    let worktree_id =
        WorktreeId::try_from("acme/api#done").unwrap_or_else(|error| panic!("{error}"));
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
        url: "git@github.com:acme/api.git".to_owned(),
        context_id,
        default_branch: "main".to_owned(),
        path: home.join("repos/acme/api").display().to_string(),
        cloned_at: "2026-09-04T00:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    });
    persisted.worktrees.push(Worktree {
        id: worktree_id.clone(),
        repo_id,
        slug: "done".to_owned(),
        branch: "done".to_owned(),
        base_ref: "origin/main".to_owned(),
        path: home.join("worktrees/acme/api/done").display().to_string(),
        session: "api/done".to_owned(),
        host: None,
        created_at: "2026-09-04T00:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    });
    state
        .save(persisted)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let inner = FakeShell::new();
    inner.when(|command| command.program == "gh", success("[]"));
    inner.when(
        |command| command.args == ["branch", "--show-current"],
        success("done"),
    );
    inner.when(
        |command| {
            command
                .args
                .starts_with(&["rev-parse".to_owned(), "--verify".to_owned()])
                && command
                    .args
                    .last()
                    .is_some_and(|argument| argument == "HEAD")
        },
        success("abc"),
    );
    inner.when(
        |command| {
            command
                .args
                .starts_with(&["rev-parse".to_owned(), "--verify".to_owned()])
                && command
                    .args
                    .last()
                    .is_some_and(|argument| argument == "origin/main")
        },
        success("def"),
    );
    inner.when(
        |command| {
            command
                .args
                .first()
                .is_some_and(|argument| argument == "for-each-ref")
        },
        success("origin/done\0"),
    );
    inner.when(
        |command| command.args.contains(&"--left-right".to_owned()),
        success("0 0"),
    );
    inner.when(
        |command| command.args.contains(&"--count".to_owned()),
        success("0"),
    );
    inner.when(|command| command.program == "git", success(""));
    let shell = Arc::new(DirtyOnSecondStatus {
        inner,
        status_calls: AtomicUsize::new(0),
    });
    let git = Arc::new(fleet_daemon::adapters::git::ShellGit::new(Arc::clone(
        &shell,
    )));
    let github = Arc::new(fleet_daemon::adapters::github::GhCli::new(Arc::clone(
        &shell,
    )));
    let sessions = Sessions::new(Arc::clone(&config), Arc::clone(&state));
    let inspect = Inspect::new(state, Arc::clone(&jobs), git, github, sessions.clone());
    let deleter = Arc::new(RecordingDeleter::default());
    let prune = Prune::new(jobs, inspect, sessions, deleter.clone());

    let result = prune
        .worktrees(false, false, false, None, None)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(result.deleted.is_empty());
    assert_eq!(result.skipped.len(), 1);
    assert!(result.skipped[0].dirty);
    assert_eq!(result.skipped[0].reason, "uncommitted changes");
    assert!(
        deleter
            .deleted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    assert_eq!(shell.status_calls.load(Ordering::SeqCst), 2);
}

fn success(stdout: &str) -> fleet_daemon::adapters::shell::ShellResult {
    fleet_daemon::adapters::shell::ShellResult {
        status: 0,
        stdout: stdout.to_owned(),
        stderr: String::new(),
    }
}
