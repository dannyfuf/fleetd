use super::*;

use crate::adapters::{clock::SystemClock, files::RealFiles};

#[tokio::test]
async fn proxied_ensure_uses_executing_daemon_layout_and_only_degrades_lazygit() {
    let temp = tempfile::tempdir().expect("temp home");
    let home = temp.path();
    let repos = home.join("repos");
    let worktrees = home.join("worktrees");
    let worktree_path = worktrees.join("owner/repo/feature");
    std::fs::create_dir_all(&repos).expect("repos directory");
    std::fs::create_dir_all(&worktree_path).expect("worktree directory");
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [repos.clone(), worktrees],
    ));
    let config = Arc::new(ConfigStore::new(home, files.clone()));
    let mut effective = config.load().await.expect("config");
    effective.agent_commands.claude = "/bin/sleep 30".to_owned();
    effective.windows = vec![
        fleet_core::config::WindowConfig {
            name: "lg".to_owned(),
            command: fleet_core::config::NATIVE_LAZYGIT.to_owned(),
        },
        fleet_core::config::WindowConfig {
            name: "agent".to_owned(),
            command: "{agent}".to_owned(),
        },
    ];
    config.save(effective).await.expect("save config");

    let state = Arc::new(StateStore::new(home, files, Arc::new(SystemClock)));
    let context: fleet_core::ids::ContextId = "team".parse().expect("context");
    let repo: RepoId = "owner/repo".parse().expect("repo");
    let worktree: WorktreeId = "owner/repo#feature".parse().expect("worktree");
    let mut persisted = fleet_core::state::default_state();
    persisted.contexts.push(fleet_core::model::Context {
        id: context.clone(),
        name: "Team".to_owned(),
        owners: vec!["owner".to_owned()],
        created_at: "2026-09-08T00:00:00Z".to_owned(),
    });
    persisted.repos.push(fleet_core::model::Repo {
        id: repo.clone(),
        owner: "owner".to_owned(),
        name: "repo".to_owned(),
        url: "https://example.invalid/owner/repo".to_owned(),
        context_id: context,
        default_branch: "main".to_owned(),
        path: repos.join("owner/repo").display().to_string(),
        cloned_at: "2026-09-08T00:00:00Z".to_owned(),
        hooks: fleet_core::model::RepoHooks::default(),
    });
    persisted.worktrees.push(fleet_core::model::Worktree {
        id: worktree.clone(),
        repo_id: repo,
        slug: "feature".to_owned(),
        branch: "feature".to_owned(),
        base_ref: "main".to_owned(),
        path: worktree_path.display().to_string(),
        session: "repo/feature".to_owned(),
        host: None,
        created_at: "2026-09-08T00:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    });
    state.save(persisted).await.expect("save state");
    let sessions = Sessions::new(config, state);

    let ensured = sessions
        .ensure_proxied(Some(worktree.clone()), None, false)
        .await
        .expect("proxied worktree session");
    assert_eq!(
        ensured
            .terminals
            .iter()
            .map(|terminal| (
                terminal.name.as_str(),
                terminal.command.as_str(),
                terminal.kind
            ))
            .collect::<Vec<_>>(),
        vec![
            ("lg", "lazygit", TerminalKind::Pty),
            ("agent", "/bin/sleep 30", TerminalKind::Pty),
        ]
    );

    let agent = sessions
        .ensure_proxied(Some(worktree), Some(Agent::Claude), false)
        .await
        .expect("proxied agent session");
    assert_eq!(agent.terminals.len(), 1);
    assert_eq!(agent.terminals[0].command, "/bin/sleep 30");

    sessions.kill(ensured.id).await.expect("kill worktree");
    sessions.kill(agent.id).await.expect("kill agent");
}

#[tokio::test(flavor = "current_thread")]
async fn attach_timeout_does_not_block_executor() {
    use std::{sync::atomic::AtomicBool, time::Duration};

    let executor_progressed = Arc::new(AtomicBool::new(false));
    let progress = Arc::clone(&executor_progressed);
    let progressed_before_attach_completed = Arc::new(AtomicBool::new(false));
    let observed_progress = Arc::clone(&progressed_before_attach_completed);
    let host = TerminalHost::spawn(TerminalHostOptions {
        terminal: TerminalId(9),
        pty: PtyOptions::command(
            "/bin/cat",
            std::iter::empty::<&str>(),
            PathBuf::from("/tmp"),
            80,
            24,
        ),
        scrollback_bytes: 1024,
        initial_command: None,
        starting_sequence: 1,
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let (attached, ()) = tokio::join!(
        async {
            let result = host.attach(81, 24, deadline).await;
            observed_progress.store(
                executor_progressed.load(std::sync::atomic::Ordering::Acquire),
                std::sync::atomic::Ordering::Release,
            );
            result
        },
        async move {
            progress.store(true, std::sync::atomic::Ordering::Release);
        },
    );
    let elapsed = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or_else(Instant::now);
    let expired = host.attach(82, 24, elapsed).await;
    host.kill().unwrap();
    host.join().unwrap();

    assert!(attached.is_ok());
    assert!(executor_progressed.load(std::sync::atomic::Ordering::Acquire));
    assert!(
        progressed_before_attach_completed.load(std::sync::atomic::Ordering::Acquire),
        "TerminalHost::attach blocked the current-thread executor"
    );
    assert!(
        matches!(expired, Err(fleet_term::HostError::AttachFailed(message)) if message == "attachment deadline elapsed")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn await_attach_timeout_bounds_pending_future() {
    use std::{future::pending, time::Duration};

    let deadline = tokio::time::Instant::now() + Duration::from_millis(20);
    let result = host_bridge::await_attach(
        TerminalId(9),
        deadline,
        pending::<Result<FrameUpdate, fleet_term::HostError>>(),
    )
    .await;

    assert!(
        matches!(result, Err(DaemonError::Timeout(message)) if message == "terminal 9 attachment")
    );
}

#[tokio::test]
async fn ensure_lock_entry_is_pruned_after_its_session_is_gone() {
    let (frames, _receiver) = broadcast::channel(1);
    let runtime = Arc::new(SessionRuntime::new(frames));
    let session = agent_session_id(Agent::Claude).expect("valid fixed agent session id");
    runtime
        .registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .sessions
        .insert(
            session.clone(),
            Session {
                id: session.clone(),
                host: None,
                kind: SessionKind::Agent(Agent::Claude),
                cwd: "/tmp".to_owned(),
                terminals: Vec::new(),
                active_terminal: None,
                slept_at: None,
                kept_terminals: Vec::new(),
            },
        );

    let claim = runtime.claim_ensure_lock(session.clone()).await;
    assert!(runtime.kill_if_present(&session));
    assert!(
        runtime
            .ensure_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&session),
        "an in-flight claim keeps serialization intact while it unwinds"
    );

    drop(claim);
    assert!(
        !runtime
            .ensure_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&session),
        "the final claim must not leave a historical session id behind"
    );
}

#[tokio::test]
async fn cancelled_ensure_lock_waiter_prunes_the_expired_entry() {
    let (frames, _receiver) = broadcast::channel(1);
    let runtime = Arc::new(SessionRuntime::new(frames));
    let session = agent_session_id(Agent::Claude).expect("valid fixed agent session id");
    let owner = runtime.claim_ensure_lock(session.clone()).await;

    let waiting_runtime = Arc::clone(&runtime);
    let waiting_session = session.clone();
    let mut waiter =
        Box::pin(async move { waiting_runtime.claim_ensure_lock(waiting_session).await });
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    assert!(matches!(
        std::future::Future::poll(waiter.as_mut(), &mut context),
        std::task::Poll::Pending
    ));

    // Wake the queued waiter, then cancel it before it can be polled into an acquired claim.
    // The owner observes the waiter's Arc and cannot prune; the waiter's pre-await guard must.
    drop(owner);
    assert!(
        runtime
            .ensure_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&session)
    );
    drop(waiter);
    assert!(
        !runtime
            .ensure_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&session)
    );
}

#[tokio::test]
async fn unchanged_process_observation_does_not_publish_again() {
    let (frames, _) = broadcast::channel(8);
    let runtime = SessionRuntime::new(frames);
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    runtime.register_events(events);
    let session_id = SessionId::try_from("agent/session").unwrap_or_else(|error| panic!("{error}"));
    let terminal_id = TerminalId(1);
    {
        let mut registry = runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry
            .terminal_sessions
            .insert(terminal_id, session_id.clone());
        registry.sessions.insert(
            session_id.clone(),
            Session {
                id: session_id,
                host: None,
                kind: SessionKind::Agent(Agent::Claude),
                cwd: "/tmp".to_owned(),
                terminals: vec![Terminal {
                    id: terminal_id,
                    name: "agent".to_owned(),
                    command: "claude".to_owned(),
                    cwd: "/tmp".to_owned(),
                    shell_pid: None,
                    foreground_command: None,
                    status: TerminalStatus::Running,
                    title: None,
                    keep_alive: Vec::new(),
                    has_unseen_output: false,
                    agent_attention: None,
                    kind: TerminalKind::Native,
                }],
                active_terminal: Some(terminal_id),
                slept_at: None,
                kept_terminals: Vec::new(),
            },
        );
    }

    runtime.update_observation(
        terminal_id,
        Some("claude".to_owned()),
        vec!["claude".to_owned()],
        Some("claude".to_owned()),
    );
    assert!(matches!(
        receiver.recv().await,
        Ok(Event::SessionChanged(_))
    ));
    runtime.update_observation(
        terminal_id,
        Some("claude".to_owned()),
        vec!["claude".to_owned()],
        Some("claude".to_owned()),
    );
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn terminal_transition_claims_serialize() {
    let (frames, _) = broadcast::channel(1);
    let runtime = Arc::new(SessionRuntime::new(frames));
    let session = agent_session_id(Agent::Claude).expect("valid fixed agent session id");
    let first = runtime.claim_terminal_transition(session.clone()).await;
    let mut second = Box::pin(runtime.claim_terminal_transition(session));
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);

    assert!(matches!(
        std::future::Future::poll(second.as_mut(), &mut context),
        std::task::Poll::Pending
    ));
    drop(first);
    second.await;
}

#[test]
fn viewport_frames_do_not_mark_output() {
    let session_id = agent_session_id(Agent::Claude).expect("valid fixed agent session id");
    let terminal = TerminalId(1);
    let mut registry = Registry::default();
    registry
        .terminal_sessions
        .insert(terminal, session_id.clone());
    registry.sessions.insert(
        session_id.clone(),
        Session {
            id: session_id,
            host: None,
            kind: SessionKind::Agent(Agent::Claude),
            cwd: "/tmp".to_owned(),
            terminals: vec![Terminal {
                id: terminal,
                name: "background".to_owned(),
                command: "claude".to_owned(),
                cwd: "/tmp".to_owned(),
                shell_pid: None,
                foreground_command: None,
                status: TerminalStatus::Running,
                title: None,
                keep_alive: Vec::new(),
                has_unseen_output: false,
                agent_attention: None,
                kind: TerminalKind::Native,
            }],
            active_terminal: None,
            slept_at: None,
            kept_terminals: Vec::new(),
        },
    );

    assert_eq!(
        host_bridge::record_frame_activity(&mut registry, terminal, 0),
        None
    );
    assert!(!registry.sessions.values().next().unwrap().terminals[0].has_unseen_output);
    assert!(host_bridge::record_frame_activity(&mut registry, terminal, 4).is_some());
    registry.sessions.values_mut().next().unwrap().terminals[0].has_unseen_output = false;
    assert_eq!(
        host_bridge::record_frame_activity(&mut registry, terminal, 4),
        None
    );
    assert!(!registry.sessions.values().next().unwrap().terminals[0].has_unseen_output);
}
