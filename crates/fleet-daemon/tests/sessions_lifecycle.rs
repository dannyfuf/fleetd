use std::sync::Arc;

use fleet_core::{
    config::{NATIVE_LAZYGIT, WindowConfig},
    ids::{ContextId, RepoId, SessionId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    sessions::{SessionState, TerminalKind, TerminalStatus},
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

/// A `fleet://` window keeps its name and its position in the strip without a PTY.
///
/// That is the whole point of keeping it in `Session.terminals`: `ctrl-s <n>` counts positions,
/// so a native tab that lived outside the list would renumber every tab after it.
#[tokio::test]
async fn a_native_window_is_a_tab_without_a_process() {
    let fixture = fixture().await;
    let mut effective = fixture
        .config
        .load()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    effective.windows.insert(
        1,
        WindowConfig {
            name: "lg".to_owned(),
            command: NATIVE_LAZYGIT.to_owned(),
        },
    );
    fixture
        .config
        .save(effective)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    let sessions = Sessions::new(Arc::clone(&fixture.config), Arc::clone(&fixture.state));
    let session = sessions
        .ensure(Some(fixture.worktree.clone()), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(
        session
            .terminals
            .iter()
            .map(|terminal| (terminal.name.as_str(), terminal.kind))
            .collect::<Vec<_>>(),
        vec![
            ("one", TerminalKind::Pty),
            ("lg", TerminalKind::Native),
            ("two", TerminalKind::Pty),
        ],
        "the configured order is the tab order, native or not"
    );

    let native = session.terminals[1].clone();
    assert!(native.is_native());
    assert_eq!(native.shell_pid, None, "there is no process behind it");
    assert!(matches!(native.status, TerminalStatus::Running));

    // `ctrl-s 2` reaches it, and selecting it is an ordinary selection.
    let selected = sessions
        .select_terminal(session.id.clone(), native.id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(selected.active_terminal, Some(native.id));

    // Everything that needs a PTY says so instead of reaching one that does not exist.
    for outcome in [
        sessions.attach(native.id, 80, 24).await,
        sessions.resize(native.id, 100, 30).await,
        sessions.request_full_frame(native.id).await,
        sessions.paste(native.id, "x".to_owned()).await,
    ] {
        assert!(
            matches!(outcome, Err(DaemonError::NotFound(_))),
            "a native terminal has no host: {outcome:?}"
        );
    }
    assert!(
        matches!(
            sessions.restart_terminal(native.id).await,
            Err(DaemonError::Conflict(_))
        ),
        "there is nothing to restart"
    );

    // A native tab must not be able to report the session as attached on its own.
    assert_eq!(
        sessions
            .refresh_statuses(None)
            .await
            .unwrap_or_else(|error| panic!("{error}"))[0]
            .session,
        SessionState::Detached
    );

    // Repair after a close puts it back in the same place.
    sessions
        .close_terminal(native.id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let repaired = sessions
        .ensure(Some(fixture.worktree.clone()), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        repaired
            .terminals
            .iter()
            .map(|terminal| terminal.name.as_str())
            .collect::<Vec<_>>(),
        vec!["one", "lg", "two"]
    );
    assert!(repaired.terminals[1].is_native());

    sessions
        .kill(SessionId::try_from("repo/feature").unwrap_or_else(|error| panic!("{error}")))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}
