//! Restart behaviour: what a new manager over the same database owes the threads it finds.
//!
//! The two properties under test are the ones the SQLite store changed. A start replays nothing,
//! so the thread list is answered from the database and no log is read; and a thread the previous
//! run left claiming a provider is settled **as appended events** by the background repair pass,
//! which each test here awaits rather than races.

use super::*;

#[tokio::test]
async fn restart_recovery_never_advances_memory_past_a_log_it_could_not_write() {
    // The database path's parent is a *file*, so the store cannot be opened and every append
    // fails. §6 makes the log what the reducer wrote before the event became visible: a
    // projection that ran ahead of a failed append would make the next real append leave a hole
    // the log's unique index refuses, taking the tail of the transcript with it.
    let temp = tempfile::tempdir().expect("tempdir");
    let home = FleetHome::new(temp.path());
    std::fs::write(home.agents_path(), b"not a directory").expect("occupy the store root");
    let manager = AgentSessionManager::new_with_factory(
        home.agents_db_path(),
        BroadcastBus::new(16),
        empty_worktrees(temp.path()),
        None,
        Arc::new(|_, _, _| Err(anyhow::anyhow!("no provider in this test"))),
    );

    let thread = ThreadId::new();
    let worktree = WorktreeId::try_from("acme/api#feature").expect("worktree");
    let created = Utc::now();
    let mut record = AgentThreadRecord {
        thread,
        worktree: worktree.clone(),
        provider: AgentKind::Claude,
        title: "Claude".to_owned(),
        created,
        last_activity: created,
        resume_cursor: None,
        model: None,
        mode: PermissionMode::Ask,
        last_outcome: None,
    };
    let mut projection = ThreadProjection::new(thread, worktree, AgentKind::Claude);
    let gate = GateId::new();
    projection
        .apply(&SeqEvent {
            seq: Seq(1),
            at: created,
            raw: None,
            event: AgentEvent::GateOpened {
                gate,
                turn: None,
                kind: GateKind::Plan {
                    markdown: "plan".to_owned(),
                    steps: Vec::new(),
                },
            },
        })
        .expect("open a gate");
    projection.session = SessionState::Running;

    super::hydrate::recover_orphan(&manager.inner, &mut record, &mut projection).await;

    assert_eq!(
        projection.last_seq,
        Seq(1),
        "the projection advanced past an event the log never received",
    );
    assert!(
        !projection.gates.is_empty(),
        "the gate was settled in memory only",
    );
}

#[tokio::test]
async fn restart_settles_a_ready_thread_and_lets_open_resume_it() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(Some("cursor-ready".to_owned())).await.thread;
    harness
        .script
        .emit(AgentEvent::SessionStateChanged(SessionState::Ready))
        .await;
    harness
        .settle(thread, "a ready session", |projection| {
            projection.session == SessionState::Ready
        })
        .await;

    // The restart killed the child: a thread still claiming a live session is an orphan, and
    // one the log leaves `Ready` is exactly as orphaned as one it leaves `Running` (§6).
    let restarted = harness.restart().await;
    let summary = restarted
        .summaries()
        .await
        .into_iter()
        .find(|summary| summary.thread == thread)
        .expect("the thread survives a restart");
    assert_eq!(summary.session, SessionState::Stopped);
    assert_eq!(harness.script.starts(), 1, "restart never reattaches");

    restarted
        .open(thread, None)
        .await
        .expect("open resumes a recovered thread");
    assert_eq!(harness.script.starts(), 2, "open starts a resumed provider");
    restarted
        .send(
            thread,
            UserInput {
                text: "still usable".to_owned(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("a recovered thread accepts a send");
}

#[tokio::test]
async fn restart_fails_orphaned_threads_without_a_resume_cursor() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(None).await.thread;
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;

    let restarted = harness.restart().await;
    let summary = restarted
        .summaries()
        .await
        .into_iter()
        .find(|summary| summary.thread == thread)
        .expect("the thread survives a restart");
    assert_eq!(summary.session, SessionState::Error);
    assert_eq!(summary.turn, TurnState::Failed(turn));
    assert_eq!(summary.attention, Attention::Failed);
    assert_eq!(harness.script.starts(), 1, "restart never reattaches");
}

#[tokio::test]
async fn restart_stops_resumable_threads_and_resumes_them_on_open() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(Some("cursor-1".to_owned())).await.thread;
    let turn = TurnId::new();
    harness
        .script
        .emit(AgentEvent::TurnStarted {
            turn,
            user_item: ItemId::new(),
        })
        .await;
    harness
        .settle(thread, "a running turn", |projection| {
            projection.turn == TurnState::Running(turn)
        })
        .await;

    let restarted = harness.restart().await;
    let summary = restarted
        .summaries()
        .await
        .into_iter()
        .find(|summary| summary.thread == thread)
        .expect("the thread survives a restart");
    assert_eq!(summary.session, SessionState::Stopped);
    assert_eq!(summary.turn, TurnState::Interrupted(turn));
    assert_eq!(harness.script.starts(), 1, "restart never reattaches");

    let ResponseBody::AgentThreadSnapshot { projection, .. } = restarted
        .open(thread, None)
        .await
        .expect("open resumes a stopped thread")
    else {
        panic!("expected a snapshot");
    };
    assert_eq!(harness.script.starts(), 2, "open starts a resumed provider");
    assert!(
        harness
            .script
            .calls()
            .contains(&FakeCall::Start(thread, Some("cursor-1".to_owned())))
    );
    assert!(
        projection.items.is_empty() || projection.last_seq > Seq(0),
        "a resumed thread keeps its transcript"
    );
}

#[tokio::test]
async fn a_restart_settles_the_gates_the_log_left_open() {
    let harness = Harness::start(full()).await;
    let thread = harness.create(Some("cursor-9".to_owned())).await.thread;
    harness.script.emit(permission_gate(GateId::new())).await;
    harness
        .settle(thread, "an open gate", |projection| {
            !projection.gates.is_empty()
        })
        .await;

    let restarted = harness.restart().await;
    let summary = restarted
        .summaries()
        .await
        .into_iter()
        .find(|summary| summary.thread == thread)
        .expect("the thread survives a restart");
    assert_ne!(
        summary.attention,
        Attention::NeedsYou(AttentionKind::Permission),
        "a gate no adapter can answer does not survive the restart"
    );
}

/// A start must not read a single transcript, whatever the history is.
///
/// This is the failure that made the NDJSON store unshippable: the constructor replayed every
/// thread's whole log, so `fleetd` start cost grew without bound and `AgentThreadList` was a map
/// over whatever that replay had built. The list is now one `SELECT` against `threads`, and the
/// assertion that no log was read is exact rather than a timing guess: reading a log is what
/// hydration does, and hydration is what puts a thread in the manager's map.
#[tokio::test]
async fn a_start_with_five_hundred_threads_lists_them_without_reading_a_log() {
    const THREADS: usize = 500;

    let temp = tempfile::tempdir().expect("tempdir");
    let home = FleetHome::new(temp.path().join("fleet"));
    let worktree = WorktreeId::try_from("acme/api#feature").expect("worktree");
    let created = chrono::DateTime::from_timestamp_millis(1_700_000_000_000).expect("stamp");

    // Seeded through the store, so no provider is ever spawned: 500 threads, each with a settled
    // session, which is what a daemon that has been running for a month looks like.
    let mut threads = Vec::with_capacity(THREADS);
    {
        let store = crate::services::agents::store::SqliteAgentStore::open(home.agents_db_path())
            .expect("open the agent database");
        for index in 0..THREADS {
            let thread = ThreadId::new();
            threads.push(thread);
            store
                .write_record(&AgentThreadRecord {
                    thread,
                    worktree: worktree.clone(),
                    provider: AgentKind::Claude,
                    title: format!("thread {index}"),
                    created: created + chrono::Duration::milliseconds(index as i64),
                    last_activity: created + chrono::Duration::milliseconds(index as i64),
                    resume_cursor: None,
                    model: None,
                    mode: PermissionMode::Ask,
                    last_outcome: None,
                })
                .await
                .expect("record a thread");
            store
                .append(
                    thread,
                    &SeqEvent {
                        seq: Seq(1),
                        at: created,
                        raw: None,
                        event: AgentEvent::SessionExited {
                            code: Some(0),
                            expected: true,
                        },
                    },
                )
                .await
                .expect("settle a thread");
        }
    }

    let manager = AgentSessionManager::new_with_factory(
        home.agents_db_path(),
        BroadcastBus::new(16),
        empty_worktrees(temp.path()),
        None,
        Arc::new(|_, _, _| Err(anyhow::anyhow!("a listing start must spawn no provider"))),
    );
    manager.clone().repair().await;

    let listed = match manager.list().await.expect("list the agent threads") {
        ResponseBody::AgentThreads(summaries) => summaries,
        other => panic!("expected AgentThreads, got {other:?}"),
    };

    assert_eq!(listed.len(), THREADS);
    assert_eq!(
        listed
            .iter()
            .map(|summary| summary.thread)
            .collect::<Vec<_>>(),
        threads,
        "the list is in creation order, as the snapshot expects"
    );
    assert!(
        listed
            .iter()
            .all(|summary| summary.session == SessionState::Stopped && summary.last_seq == Seq(1)),
        "the denormalized list row disagrees with the log it was projected from"
    );
    assert!(
        manager
            .inner
            .threads
            .read()
            .expect("hydrated threads")
            .is_empty(),
        "listing hydrated a thread, which means it replayed a log"
    );
}
