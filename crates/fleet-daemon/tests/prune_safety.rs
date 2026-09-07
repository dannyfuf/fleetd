use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use fleet_core::{
    ids::{ContextId, RepoId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    state::default_state,
};
use fleet_daemon::{
    DaemonResult,
    jobs::JobManager,
    services::{
        inspect::Inspect,
        prune::{Prune, WorktreeDeleter},
        sessions::Sessions,
    },
    stores::{config::ConfigStore, state::StateStore},
    testing::fakes::{FakeFiles, FakeGit, FakeGithub, FakeShell, FixedClock},
};

#[derive(Default)]
struct RecordingDeleter(Mutex<Vec<WorktreeId>>);

#[async_trait]
impl WorktreeDeleter for RecordingDeleter {
    async fn delete(&self, id: WorktreeId) -> DaemonResult<()> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(id);
        Ok(())
    }
}

#[tokio::test]
async fn prune_deletes_eligible_worktrees_through_the_deletion_seam() {
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

    let shell = Arc::new(FakeShell::new());
    shell.when(|command| command.program == "gh", success("[]"));
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
    let inspect = Inspect::new(state, jobs.clone(), git, github, sessions);
    let deleter = Arc::new(RecordingDeleter::default());
    let prune = Prune::new(jobs, inspect, deleter.clone());

    let result = prune
        .worktrees(false, false, false, None)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(result.deleted, vec![worktree_id.clone()]);
    assert!(result.skipped.is_empty());
    assert_eq!(
        deleter
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &[worktree_id]
    );
}

fn success(stdout: &str) -> fleet_daemon::adapters::shell::ShellResult {
    fleet_daemon::adapters::shell::ShellResult {
        status: 0,
        stdout: stdout.to_owned(),
        stderr: String::new(),
    }
}
