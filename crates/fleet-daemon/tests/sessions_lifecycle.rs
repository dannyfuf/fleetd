use std::sync::Arc;

use fleet_core::{
    config::{NATIVE_LAZYGIT, WindowConfig, default_config},
    ids::{ContextId, HostId, RepoId, SessionId, TerminalId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    sessions::{Session, SessionKind, SessionState, Terminal, TerminalKind, TerminalStatus},
    state::default_state,
};
use fleet_daemon::{
    DaemonError,
    adapters::{clock::SystemClock, files::RealFiles},
    machines::Machines,
    services::{
        mirror::Mirror,
        router::{Router, Target},
        sessions::Sessions,
    },
    stores::{config::ConfigStore, state::StateStore},
    testing::FakeRemote,
};
use fleet_proto::{
    request::RequestBody,
    response::{ResponseBody, SleepResult},
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
    effective.agent_commands.claude = "/bin/sleep 30".to_owned();
    effective.agent_commands.opencode = "/bin/sleep 30".to_owned();
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
async fn ensured_terminals_receive_their_registered_ids_in_the_pty_environment() {
    if isolated_test("ensured_terminals_receive_their_registered_ids_in_the_pty_environment") {
        return;
    }
    let fixture = fixture().await;
    let mut config = fixture
        .config
        .load()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    for window in &mut config.windows {
        window.command =
            r#"printf '%s\n' "$FLEET_SESSION" "$FLEET_TERMINAL" "$FLEET_TERMINAL_ID" > "$FLEET_TERMINAL.env""#
                .to_owned();
    }
    fixture
        .config
        .save(config)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let sessions = Sessions::new(fixture.config, fixture.state);
    let created = sessions
        .ensure(Some(fixture.worktree), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let observed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let values: Option<Vec<String>> = created
                .terminals
                .iter()
                .map(|terminal| {
                    std::fs::read_to_string(
                        std::path::Path::new(&terminal.cwd).join(format!("{}.env", terminal.name)),
                    )
                    .ok()
                    .filter(|value| value.lines().count() == 3)
                })
                .collect();
            if let Some(values) = values {
                break values;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    let registered = sessions.snapshot();
    sessions
        .kill(created.id.clone())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let observed = observed.expect("login shells must write their Fleet environment");
    assert_eq!(registered[0].id, created.id);
    assert_eq!(registered[0].terminals.len(), created.terminals.len());
    assert_ne!(created.terminals[0].id, created.terminals[1].id);
    for (terminal, env) in registered[0].terminals.iter().zip(observed) {
        assert_eq!(
            env,
            format!("{}\n{}\n{}\n", created.id, terminal.name, terminal.id)
        );
    }
}

#[tokio::test]
async fn ensure_reuses_layout_and_attachment_drives_status() {
    if isolated_test("ensure_reuses_layout_and_attachment_drives_status") {
        return;
    }
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
    if isolated_test("terminal_lifecycle_preserves_identity_and_removes_last_session") {
        return;
    }
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
    assert!(sessions.snapshot().is_empty());

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
async fn concurrent_agent_ensures_create_one_named_terminal() {
    if isolated_test("concurrent_agent_ensures_create_one_named_terminal") {
        return;
    }
    let fixture = fixture().await;
    let sessions = Sessions::new(fixture.config, fixture.state);

    let (first, second) = tokio::join!(
        sessions.ensure(None, Some(fleet_core::config::Agent::Claude), false),
        sessions.ensure(None, Some(fleet_core::config::Agent::Claude), false),
    );
    let first = first.unwrap_or_else(|error| panic!("{error}"));
    let second = second.unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(first.id, second.id);

    let sessions_snapshot = sessions.snapshot();
    let agent_session = sessions_snapshot
        .iter()
        .find(|session| session.id == first.id)
        .expect("agent session must exist");
    assert_eq!(
        agent_session
            .terminals
            .iter()
            .filter(|terminal| terminal.name == "claude")
            .count(),
        1
    );

    sessions
        .kill(first.id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn remote_worktree_session_is_forwarded_and_ids_are_localized() {
    if isolated_test("remote_worktree_session_is_forwarded_and_ids_are_localized") {
        return;
    }
    let fixture = fixture().await;
    let host = HostId::try_from("devbox").unwrap_or_else(|error| panic!("{error}"));
    let machines = Arc::new(Machines::from_config(&default_config("/tmp/fleet-router")));
    let remote = Arc::new(FakeRemote::new(host.clone()));
    machines.install_endpoint(host.clone(), remote.clone());
    let router = Router::new(machines, Arc::new(Mirror::new()));
    router
        .ids
        .register_worktree(&host, fixture.worktree.clone());
    let body = RequestBody::EnsureSession {
        worktree: Some(fixture.worktree.clone()),
        agent: None,
        sleep_previous: false,
    };
    remote.push_response(Ok(ResponseBody::Session(Session {
        id: SessionId::try_from("repo/feature").unwrap_or_else(|error| panic!("{error}")),
        host: None,
        kind: SessionKind::Worktree(fixture.worktree),
        cwd: "/remote/worktrees/repo/feature".to_owned(),
        terminals: vec![Terminal {
            id: TerminalId(1),
            name: "shell".to_owned(),
            command: "zsh".to_owned(),
            cwd: "/remote/worktrees/repo/feature".to_owned(),
            shell_pid: Some(42),
            foreground_command: None,
            status: TerminalStatus::Running,
            title: None,
            keep_alive: Vec::new(),
            has_unseen_output: false,
            kind: TerminalKind::Pty,
        }],
        active_terminal: Some(TerminalId(1)),
        slept_at: None,
        kept_terminals: Vec::new(),
    })));

    assert_eq!(router.route(&body), Target::Host(host.clone()));
    let response = router
        .forward(&host, body.clone())
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let ResponseBody::Session(session) = response else {
        panic!("expected session response");
    };
    assert_eq!(session.id.as_str(), "devbox/repo/feature");
    assert_ne!(session.terminals[0].id, TerminalId(1));
    let local_session = session.id.clone();
    let local_terminal = session.terminals[0].id;

    remote.push_response(Ok(ResponseBody::Terminal(Terminal {
        id: TerminalId(2),
        name: "extra".to_owned(),
        command: "sh".to_owned(),
        cwd: "/only/on/remote".to_owned(),
        shell_pid: Some(43),
        foreground_command: None,
        status: TerminalStatus::Running,
        title: None,
        keep_alive: Vec::new(),
        has_unseen_output: false,
        kind: TerminalKind::Pty,
    })));
    router
        .forward(
            &host,
            RequestBody::NewTerminal {
                session: local_session.clone(),
                name: "extra".to_owned(),
                command: "sh".to_owned(),
                cwd: "/only/on/remote".to_owned(),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    remote.push_response(Ok(ResponseBody::Slept(SleepResult {
        kept: Vec::new(),
        closed: Vec::new(),
        session_killed: false,
    })));
    router
        .forward(
            &host,
            RequestBody::SleepSession {
                session: local_session.clone(),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    remote.push_response(Ok(ResponseBody::Ack));
    router
        .forward(
            &host,
            RequestBody::AttachTerminal {
                terminal: local_terminal,
                cols: 80,
                rows: 24,
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    remote.push_response(Ok(ResponseBody::Ack));
    router
        .forward(
            &host,
            RequestBody::KillSession {
                session: local_session,
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(
        remote.requests(),
        vec![
            body,
            RequestBody::NewTerminal {
                session: SessionId::try_from("repo/feature")
                    .unwrap_or_else(|error| panic!("{error}")),
                name: "extra".to_owned(),
                command: "sh".to_owned(),
                cwd: "/only/on/remote".to_owned(),
            },
            RequestBody::SleepSession {
                session: SessionId::try_from("repo/feature")
                    .unwrap_or_else(|error| panic!("{error}")),
            },
            RequestBody::AttachTerminal {
                terminal: TerminalId(1),
                cols: 80,
                rows: 24,
            },
            RequestBody::KillSession {
                session: SessionId::try_from("repo/feature")
                    .unwrap_or_else(|error| panic!("{error}")),
            },
        ]
    );
}

/// A `fleet://` window keeps its name and its position in the strip without a PTY.
///
/// That is the whole point of keeping it in `Session.terminals`: `ctrl-s <n>` counts positions,
/// so a native tab that lived outside the list would renumber every tab after it.
#[tokio::test]
async fn a_native_window_is_a_tab_without_a_process() {
    if isolated_test("a_native_window_is_a_tab_without_a_process") {
        return;
    }
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

#[tokio::test]
async fn restart_keeps_frame_sequences_monotonic() {
    if isolated_test("restart_keeps_frame_sequences_monotonic") {
        return;
    }
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
            if sessions.snapshot().iter().any(|session| {
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

#[tokio::test]
async fn immediate_exit_is_registered() {
    if isolated_test("immediate_exit_is_registered") {
        return;
    }
    use std::time::Duration;

    let fixture = fixture().await;
    let mut config = fixture.config.load().await.unwrap();
    config.windows = vec![WindowConfig {
        name: "immediate".into(),
        command: "exit 23".into(),
    }];
    fixture.config.save(config).await.unwrap();
    let sessions = Sessions::new(fixture.config, fixture.state);
    let session = sessions
        .ensure(Some(fixture.worktree), None, false)
        .await
        .unwrap();
    let terminal = session.terminals[0].id;

    let code = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(code) = sessions.snapshot().iter().find_map(|session| {
                session.terminals.iter().find_map(|entry| {
                    (entry.id == terminal).then_some(&entry.status).and_then(
                        |status| match status {
                            TerminalStatus::Exited { code } => Some(*code),
                            TerminalStatus::Starting | TerminalStatus::Running => None,
                        },
                    )
                })
            }) {
                break code;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("immediate exit was not applied to registered metadata");

    assert_eq!(code, Some(23));
    sessions.kill(session.id).await.unwrap();
}

#[tokio::test]
async fn new_and_restart_transitions_serialize() {
    if isolated_test("new_and_restart_transitions_serialize") {
        return;
    }

    let fixture = fixture().await;
    let sessions = Sessions::new(fixture.config, fixture.state);
    let session = sessions
        .ensure(Some(fixture.worktree), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let cwd = session.cwd.clone();
    let first = sessions.new_terminal(
        session.id.clone(),
        "raced".to_owned(),
        "/bin/sh -c 'sleep 30'".to_owned(),
        cwd.clone(),
    );
    let second = sessions.new_terminal(
        session.id.clone(),
        "raced".to_owned(),
        "/bin/sh -c 'sleep 30'".to_owned(),
        cwd,
    );

    let outcomes = tokio::join!(first, second);
    let outcomes = [outcomes.0, outcomes.1];
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Err(DaemonError::Conflict(_))))
            .count(),
        1
    );
    let registered = sessions
        .snapshot()
        .into_iter()
        .find(|entry| entry.id == session.id)
        .unwrap_or_else(|| panic!("session disappeared"));
    assert_eq!(
        registered
            .terminals
            .iter()
            .filter(|terminal| terminal.name == "raced")
            .count(),
        1
    );
    sessions
        .kill(session.id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

fn isolated_test(name: &str) -> bool {
    if std::env::var("FLEET_SESSION_TEST").as_deref() == Ok(name) {
        return false;
    }
    let home = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let output = std::process::Command::new(
        std::env::current_exe().unwrap_or_else(|error| panic!("{error}")),
    )
    .args(["--exact", name, "--nocapture"])
    .env_clear()
    .env("FLEET_SESSION_TEST", name)
    .env("FLEET_TEST_PTY_ENV_CHILD", "1")
    .env("SHELL", "/bin/sh")
    .env("HOME", home.path())
    .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
    .output()
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}
