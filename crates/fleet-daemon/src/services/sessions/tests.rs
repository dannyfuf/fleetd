use super::*;

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
