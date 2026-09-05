use std::sync::Arc;

use fleet_core::{
    config::WindowConfig,
    ids::{ContextId, RepoId, SessionId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    sessions::{SessionState, TerminalStatus},
    state::default_state,
};
use fleet_daemon::{
    DaemonError,
    adapters::{clock::SystemClock, files::RealFiles},
    services::sessions::Sessions,
    stores::{config::ConfigStore, state::StateStore},
};

struct Fixture {
    _temp: tempfile::TempDir,
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    worktree: WorktreeId,
}

async fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet");
    let repos = home.join("repos");
    let worktree_path = home.join("worktrees/repo/feature");
    std::fs::create_dir_all(&repos).unwrap_or_else(|error| panic!("{error}"));
    std::fs::create_dir_all(&worktree_path).unwrap_or_else(|error| panic!("{error}"));
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [repos.clone(), home.join("worktrees")],
    ));
    let config = Arc::new(ConfigStore::new(&home, files.clone()));
    let mut effective = config
        .load()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    effective.windows = vec![
        WindowConfig {
            name: "one".to_owned(),
            command: "/bin/sh -c 'sleep 30'".to_owned(),
        },
        WindowConfig {
            name: "two".to_owned(),
            command: "/bin/sh -c 'sleep 30'".to_owned(),
        },
    ];
    config
        .save(effective)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let state = Arc::new(StateStore::new(&home, files, Arc::new(SystemClock)));
    let context_id = ContextId::try_from("team").unwrap_or_else(|error| panic!("{error}"));
    let repo_id = RepoId::try_from("owner/repo").unwrap_or_else(|error| panic!("{error}"));
    let worktree_id =
        WorktreeId::try_from("owner/repo#feature").unwrap_or_else(|error| panic!("{error}"));
    let mut persisted = default_state();
    persisted.contexts.push(Context {
        id: context_id.clone(),
        name: "Team".to_owned(),
        owners: vec!["owner".to_owned()],
        created_at: "2026-09-04T00:00:00Z".to_owned(),
    });
    persisted.repos.push(Repo {
        id: repo_id.clone(),
        owner: "owner".to_owned(),
        name: "repo".to_owned(),
        url: "https://example.invalid/owner/repo".to_owned(),
        context_id,
        default_branch: "main".to_owned(),
        path: repos.join("owner/repo").to_string_lossy().into_owned(),
        cloned_at: "2026-09-04T00:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    });
    persisted.worktrees.push(Worktree {
        id: worktree_id.clone(),
        repo_id,
        slug: "feature".to_owned(),
        branch: "feature".to_owned(),
        base_ref: "main".to_owned(),
        path: worktree_path.to_string_lossy().into_owned(),
        session: "repo/feature".to_owned(),
        host: None,
        created_at: "2026-09-04T00:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    });
    state
        .save(persisted)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    Fixture {
        _temp: temp,
        config,
        state,
        worktree: worktree_id,
    }
}

#[tokio::test]
async fn ensure_reuses_layout_and_attachment_drives_status() {
    let fixture = fixture().await;
    let sessions = Sessions::new(fixture.config, fixture.state);
    let created = sessions
        .ensure(Some(fixture.worktree.clone()), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        created
            .terminals
            .iter()
            .map(|terminal| terminal.name.as_str())
            .collect::<Vec<_>>(),
        vec!["one", "two"]
    );
    assert!(
        created
            .terminals
            .iter()
            .all(|terminal| terminal.shell_pid.is_some())
    );

    let reused = sessions
        .ensure(Some(fixture.worktree.clone()), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(reused.terminals, created.terminals);

    let terminal = created.terminals[0].id;
    let mut frames = sessions.subscribe_frames();
    sessions
        .attach(terminal, 91, 27)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), frames.recv())
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(frame.full);
    assert_eq!((frame.cols, frame.rows), (91, 27));
    assert_eq!(
        sessions
            .refresh_statuses(None)
            .await
            .unwrap_or_else(|error| panic!("{error}"))[0]
            .session,
        SessionState::Attached
    );
    sessions
        .detach(terminal)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        sessions
            .refresh_statuses(None)
            .await
            .unwrap_or_else(|error| panic!("{error}"))[0]
            .session,
        SessionState::Detached
    );
    sessions
        .close_terminal(terminal)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let repaired = sessions
        .ensure(Some(fixture.worktree), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        repaired
            .terminals
            .iter()
            .map(|terminal| terminal.name.as_str())
            .collect::<Vec<_>>(),
        vec!["one", "two"]
    );
    sessions
        .kill(SessionId::try_from("repo/feature").unwrap_or_else(|error| panic!("{error}")))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn terminal_lifecycle_preserves_identity_and_removes_last_session() {
    let fixture = fixture().await;
    let sessions = Sessions::new(fixture.config, fixture.state);
    let session = sessions
        .ensure(Some(fixture.worktree), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let first = session.terminals[0].id;
    let renamed = sessions
        .rename_terminal(first, "main".to_owned())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(renamed.name, "main");
    assert!(matches!(renamed.status, TerminalStatus::Running));
    sessions
        .close_terminal(first)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    sessions
        .close_terminal(session.terminals[1].id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        sessions
            .list()
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .is_empty()
    );

    let agent = sessions
        .ensure(None, Some(fleet_core::config::Agent::Claude), false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(agent.id.as_str(), "swarm-agent-claude");
    assert_eq!(agent.terminals[0].name, "claude");
    sessions
        .kill(agent.id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn remote_worktrees_are_rejected_with_stable_message() {
    let fixture = fixture().await;
    let mut state = fixture
        .state
        .load()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    state.worktrees[0].host =
        Some(fleet_core::ids::HostId::try_from("devbox").unwrap_or_else(|error| panic!("{error}")));
    fixture
        .state
        .save(state)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let sessions = Sessions::new(fixture.config, fixture.state);
    let error = sessions
        .ensure(Some(fixture.worktree), None, false)
        .await
        .expect_err("remote ensure must fail");
    assert!(
        matches!(error, DaemonError::Unsupported(message) if message == "remote hosts are not supported yet")
    );
}

#[tokio::test]
async fn restart_keeps_frame_sequences_monotonic() {
    use std::time::Duration;
    let fixture = fixture().await;
    let mut config = fixture.config.load().await.unwrap();
    config.windows = vec![WindowConfig {
        name: "short-lived".into(),
        command: "/bin/cat".into(),
    }];
    fixture.config.save(config).await.unwrap();
    let sessions = Sessions::new(fixture.config, fixture.state);
    let mut frames = sessions.subscribe_frames();
    let session = sessions
        .ensure(Some(fixture.worktree), None, false)
        .await
        .unwrap();
    let terminal = session.terminals[0].id;
    // Attachment frames also participate in the sequence tracked by Sessions.
    for _ in 0..3 {
        sessions.attach(terminal, 80, 24).await.unwrap();
    }
    // End this exact child independently of login-shell startup files and prompt readiness.
    assert!(
        std::process::Command::new("/bin/kill")
            .args([
                "-KILL",
                &session.terminals[0].shell_pid.unwrap().to_string()
            ])
            .status()
            .unwrap()
            .success()
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if sessions.list().await.unwrap().iter().any(|session| {
                session.terminals.iter().any(|entry| {
                    entry.id == terminal && matches!(entry.status, TerminalStatus::Exited { .. })
                })
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("terminal did not exit");
    // Exited is forwarded after all frames from the old host.
    let mut last_sequence = 0;
    while let Ok(frame) = frames.try_recv() {
        assert_eq!(frame.terminal, terminal);
        last_sequence = last_sequence.max(frame.seq);
    }
    assert!(last_sequence >= 3);
    sessions.restart_terminal(terminal).await.unwrap();
    sessions.request_full_frame(terminal).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), frames.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.terminal, terminal);
    assert!(first.full);
    assert!(
        first.seq > last_sequence,
        "restart frame {} must follow {}",
        first.seq,
        last_sequence
    );
    sessions.kill(session.id).await.unwrap();
}
