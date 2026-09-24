use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::Utc;
use fleet_core::{
    agents::{
        AgentEvent, AgentKind, PermissionMode, Seq, SeqEvent, ThreadId, ThreadProjection, UserInput,
    },
    config::default_config,
    ids::{ContextId, HostId, RepoId, WorktreeId},
    model::{Context, HostConfigEntry, Repo, RepoHooks, Worktree},
    paths::FleetHome,
    state::default_state,
};
use fleet_daemon::{
    DaemonResult,
    adapters::{clock::SystemClock, files::RealFiles},
    machines::{LinkOptions, MachineProvider, Machines, RemoteEndpoint, RemoteHello, RemoteLink},
    server::BroadcastBus,
    services::{
        mirror::Mirror,
        router::{
            Router, Target,
            agents::{
                AgentMirror, MirrorBatchWrite, MirrorWrite, ThreadRegistrations, agent_list_target,
                register_mirror_threads, register_thread_events,
            },
        },
    },
    stores::{config::ConfigStore, state::StateStore},
    testing::FakeRemote,
};
use fleet_proto::{
    event::Event,
    request::RequestBody,
    response::ResponseBody,
    snapshot::{DaemonInfo, LinkState, Snapshot},
};

use crate::infra;

#[path = "agents_remote/gaps.rs"]
mod gaps;
#[path = "agents_remote/mirror.rs"]
mod mirror;

#[derive(Default)]
struct BatchMirror {
    calls: Mutex<Vec<(ThreadId, Vec<Seq>)>>,
    trace: Mutex<Vec<String>>,
    gap_after: Mutex<Option<usize>>,
    delta: Mutex<Option<RequestBody>>,
    refill_finished: tokio::sync::Notify,
}

impl BatchMirror {
    fn calls(&self) -> Vec<(ThreadId, Vec<Seq>)> {
        lock_test(&self.calls).clone()
    }

    fn trace(&self) -> Vec<String> {
        lock_test(&self.trace).clone()
    }
}

#[async_trait::async_trait]
impl AgentMirror for BatchMirror {
    async fn adopt(&self, _host: &HostId, _summaries: &[fleet_core::agents::AgentThreadSummary]) {
        lock_test(&self.trace).push("summary".to_owned());
    }

    async fn ingest(&self, _host: &HostId, _thread: ThreadId, _event: &SeqEvent) -> MirrorWrite {
        MirrorWrite::Stored
    }

    async fn ingest_batch(
        &self,
        _host: &HostId,
        thread: ThreadId,
        events: &[SeqEvent],
    ) -> MirrorBatchWrite {
        let sequences = events.iter().map(|event| event.seq).collect::<Vec<_>>();
        lock_test(&self.calls).push((thread, sequences.clone()));
        lock_test(&self.trace).push(format!(
            "events:{thread}:{}",
            sequences
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ));
        match *lock_test(&self.gap_after) {
            Some(publishable) => MirrorBatchWrite {
                publishable: publishable.min(events.len()),
                needs_window: true,
            },
            None => MirrorBatchWrite {
                publishable: events.len(),
                needs_window: false,
            },
        }
    }

    async fn open(
        &self,
        _host: &HostId,
        _body: &RequestBody,
    ) -> Option<DaemonResult<ResponseBody>> {
        None
    }

    async fn delta(&self, _host: &HostId, _body: &RequestBody) -> Option<RequestBody> {
        lock_test(&self.delta).clone()
    }

    async fn absorb(
        &self,
        _host: &HostId,
        _body: &RequestBody,
        _response: &ResponseBody,
        _announce: bool,
    ) -> bool {
        self.refill_finished.notify_one();
        false
    }

    async fn cached_threads(&self, _host: &HostId) -> Vec<fleet_core::agents::AgentThreadSummary> {
        Vec::new()
    }
}

fn lock_test<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn remote_agent_event(thread: ThreadId, seq: u64) -> Event {
    Event::Agent {
        thread,
        event: SeqEvent {
            seq: Seq(seq),
            at: Utc::now(),
            raw: None,
            event: AgentEvent::Notice(format!("event {seq}")),
        },
    }
}

fn drain_agent_events(receiver: &mut tokio::sync::broadcast::Receiver<Event>) -> Vec<Event> {
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        if matches!(event, Event::Agent { .. } | Event::AgentSummary(_)) {
            events.push(event);
        }
    }
    events
}

async fn receive_agent_events(
    receiver: &mut tokio::sync::broadcast::Receiver<Event>,
    count: usize,
) -> Result<Vec<Event>, tokio::sync::broadcast::error::RecvError> {
    let mut events = Vec::with_capacity(count);
    while events.len() < count {
        let event = receiver.recv().await?;
        if matches!(event, Event::Agent { .. } | Event::AgentSummary(_)) {
            events.push(event);
        }
    }
    Ok(events)
}

fn batching_router(host: &HostId) -> (Router, Arc<FakeRemote>, Arc<BatchMirror>, BroadcastBus) {
    let machines = Arc::new(Machines::from_config(&default_config(
        "/tmp/fleet-agent-batch",
    )));
    let remote = Arc::new(FakeRemote::new(host.clone()));
    remote.set_hello(RemoteHello {
        version: "fleetd test".to_owned(),
        daemon_id: "owner".to_owned(),
        build_commit: None,
        capabilities: vec![fleet_proto::AGENT_WINDOW_CAPABILITY.to_owned()],
    });
    machines.install_endpoint(host.clone(), remote.clone());
    let router = Router::new(machines, Arc::new(Mirror::new()));
    let mirror = Arc::new(BatchMirror::default());
    router.set_agent_mirror(mirror.clone());
    let events = BroadcastBus::default();
    router.start_event_pumps(events.clone());
    (router, remote, mirror, events)
}

#[tokio::test]
async fn remote_agent_create_events_followups_restart_resume_and_deletion() {
    let temp = tempfile::tempdir().expect("tempdir");
    let local_agents = FleetHome::new(temp.path()).agents_path();
    let host = host("dev-box");
    let worktree = worktree("remote-agent");
    let thread = ThreadId::new();
    let summary =
        ThreadProjection::new(thread, worktree.clone(), AgentKind::Claude).summary(Seq::default());

    let mirror = Arc::new(Mirror::new());
    mirror.apply(
        &host,
        snapshot(
            vec![remote_worktree(worktree.clone())],
            Vec::new(),
            temp.path().to_string_lossy().into_owned(),
        ),
    );
    let machines = Arc::new(Machines::from_config(&default_config(temp.path())));
    let remote = Arc::new(FakeRemote::new(host.clone()));
    machines.install_endpoint(host.clone(), remote.clone());
    let router = Router::new(machines, mirror);
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    router.start_event_pumps(events);

    let create = RequestBody::AgentThreadCreate {
        worktree: worktree.clone(),
        provider: AgentKind::Claude,
        model: None,
        mode: Some(PermissionMode::Ask),
        resume_cursor: None,
        title: Some("remote task".to_owned()),
    };
    assert_eq!(router.route(&create), Target::Host(host.clone()));
    remote.push_response(Ok(ResponseBody::AgentThreadCreated(summary.clone())));
    let created = router
        .forward(&host, create.clone())
        .await
        .expect("forward remote thread creation");
    assert!(matches!(created, ResponseBody::AgentThreadCreated(value) if value.thread == thread));
    assert_eq!(remote.requests(), vec![create]);
    assert_eq!(router.ids.host_of_thread(&thread), Some(host.clone()));

    let sequenced = SeqEvent {
        seq: Seq(1),
        at: Utc::now(),
        raw: None,
        event: AgentEvent::Notice("remote provider ready".to_owned()),
    };
    remote.emit(Event::Agent {
        thread,
        event: sequenced.clone(),
    });
    let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("remote agent event timeout")
        .expect("remote agent event");
    assert_eq!(
        event,
        Event::Agent {
            thread,
            event: sequenced
        }
    );

    let send = RequestBody::AgentSend {
        thread,
        input: UserInput {
            text: "continue".to_owned(),
            attachments: Vec::new(),
            item: None,
            origin: Default::default(),
        },
    };
    assert_eq!(router.route(&send), Target::Host(host.clone()));

    remote.set_state(LinkState::Down);
    wait_until(|| router.ids.host_of_thread(&thread).is_none()).await;
    remote.set_state(LinkState::Ready);
    remote.emit(Event::SnapshotChanged(snapshot(
        vec![remote_worktree(worktree)],
        vec![summary.clone()],
        temp.path().to_string_lossy().into_owned(),
    )));
    wait_until(|| router.ids.host_of_thread(&thread) == Some(host.clone())).await;

    let open = RequestBody::AgentThreadOpen {
        thread,
        from_seq: None,
        after_seq: None,
        turn_limit: None,
        before_cursor: None,
        request_sync_marker: false,
    };
    assert_eq!(router.route(&open), Target::Host(host.clone()));
    remote.push_response(Ok(ResponseBody::AgentThreadSnapshot {
        projection: ThreadProjection::new(thread, summary.worktree.clone(), AgentKind::Claude),
        events_after: Vec::new(),
    }));
    router
        .forward(&host, open.clone())
        .await
        .expect("reopen remote thread after mirror re-sync");
    assert_eq!(remote.requests().last(), Some(&open));
    assert!(
        !local_agents.exists(),
        "routing a remote thread must not create local transcript files"
    );

    let registrations = ThreadRegistrations::default();
    register_mirror_threads(
        &registrations,
        &host,
        std::slice::from_ref(&summary),
        &router.ids,
    );
    register_thread_events(
        &registrations,
        &Event::SnapshotChanged(snapshot(
            Vec::new(),
            Vec::new(),
            temp.path().to_string_lossy().into_owned(),
        )),
        &host,
        &router.ids,
    );
    assert!(router.ids.host_of_thread(&thread).is_none());
    // The router's id map has forgotten the thread, but the snapshot mirror re-synced at link
    // recovery still names its owner. A send must therefore still be forwarded, never answered
    // locally and refused — the regression for the window after a local daemon restart in which
    // the id map is empty and only the mirror knows the owner (`docs/NATIVE-AGENTS.md` §9.3).
    assert_eq!(router.route(&send), Target::Host(host.clone()));

    // Only once the owner's own snapshot drops the thread from the mirror does a verb fall local.
    router.mirror.apply(
        &host,
        snapshot(
            vec![remote_worktree(summary.worktree.clone())],
            Vec::new(),
            temp.path().to_string_lossy().into_owned(),
        ),
    );
    assert!(router.mirror.host_of_thread(&thread).is_none());
    assert_eq!(router.route(&send), Target::Local);
}

#[tokio::test]
async fn a_fifty_delta_link_burst_uses_at_most_two_mirror_transactions_and_keeps_order() {
    let host = host("dev-box");
    let thread = ThreadId::new();
    let (_router, remote, mirror, events) = batching_router(&host);
    let mut receiver = events.subscribe();

    for seq in 1..=50 {
        remote.emit(remote_agent_event(thread, seq));
    }

    let published = receive_agent_events(&mut receiver, 50)
        .await
        .expect("fifty remote events")
        .into_iter()
        .map(|event| match event {
            Event::Agent {
                thread: seen,
                event,
            } => (seen, event.seq),
            other => panic!("expected an agent event, got {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        published,
        (1..=50).map(|seq| (thread, Seq(seq))).collect::<Vec<_>>()
    );
    let calls = mirror.calls();
    assert!(
        calls.len() <= 2,
        "50 immediately ready events used {} mirror transactions",
        calls.len()
    );
    assert_eq!(
        calls
            .into_iter()
            .flat_map(|(_, sequences)| sequences)
            .collect::<Vec<_>>(),
        (1..=50).map(Seq).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn interleaved_remote_threads_keep_link_order() {
    let host = host("dev-box");
    let first = ThreadId::new();
    let second = ThreadId::new();
    let (_router, remote, _mirror, events) = batching_router(&host);
    let mut receiver = events.subscribe();
    for event in [
        remote_agent_event(first, 1),
        remote_agent_event(second, 1),
        remote_agent_event(first, 2),
        remote_agent_event(second, 2),
    ] {
        remote.emit(event);
    }

    let published = receive_agent_events(&mut receiver, 4)
        .await
        .expect("four interleaved events")
        .into_iter()
        .map(|event| match event {
            Event::Agent { thread, event } => (thread, event.seq),
            other => panic!("expected an agent event, got {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        published,
        vec![
            (first, Seq(1)),
            (second, Seq(1)),
            (first, Seq(2)),
            (second, Seq(2)),
        ]
    );
}

#[tokio::test]
async fn a_gap_mid_batch_publishes_the_committed_prefix_and_starts_resync() {
    let host = host("dev-box");
    let thread = ThreadId::new();
    let (_router, remote, mirror, events) = batching_router(&host);
    let mut receiver = events.subscribe();
    *lock_test(&mirror.gap_after) = Some(1);
    *lock_test(&mirror.delta) = Some(RequestBody::AgentThreadOpen {
        thread,
        from_seq: None,
        after_seq: Some(Seq(1)),
        turn_limit: Some(10),
        before_cursor: None,
        request_sync_marker: true,
    });
    remote.push_response(Ok(ResponseBody::AgentAck));

    for seq in [1, 3, 4] {
        remote.emit(remote_agent_event(thread, seq));
    }

    let published = receive_agent_events(&mut receiver, 1)
        .await
        .expect("committed prefix event");
    mirror.refill_finished.notified().await;
    let extra = drain_agent_events(&mut receiver);
    assert!(matches!(
        published.as_slice(),
        [Event::Agent { thread: seen, event }] if *seen == thread && event.seq == Seq(1)
    ));
    assert!(extra.is_empty(), "the gap suffix was published");
    assert_eq!(mirror.calls().len(), 1);
    assert!(matches!(
        remote.requests().as_slice(),
        [RequestBody::AgentThreadOpen { thread: requested, after_seq: Some(Seq(1)), .. }]
            if *requested == thread
    ));
}

#[tokio::test]
async fn a_structural_event_between_runs_is_not_reordered() {
    let host = host("dev-box");
    let thread = ThreadId::new();
    let worktree = worktree("structural-order");
    let summary =
        ThreadProjection::new(thread, worktree, AgentKind::Claude).summary(Seq::default());
    let (_router, remote, mirror, events) = batching_router(&host);
    let mut receiver = events.subscribe();
    remote.emit(remote_agent_event(thread, 1));
    remote.emit(Event::AgentSummary(summary));
    remote.emit(remote_agent_event(thread, 2));

    let published = receive_agent_events(&mut receiver, 3)
        .await
        .expect("structurally ordered events");
    assert!(matches!(
        &published[..],
        [
            Event::Agent { event: first, .. },
            Event::AgentSummary(_),
            Event::Agent { event: second, .. },
        ] if first.seq == Seq(1) && second.seq == Seq(2)
    ));
    assert_eq!(
        mirror.trace(),
        vec![
            format!("events:{thread}:1"),
            "summary".to_owned(),
            format!("events:{thread}:2"),
        ]
    );
}

#[test]
fn agent_listing_is_a_fanout_with_unchanged_requests() {
    let first = host("alpha");
    let second = host("beta");
    assert_eq!(
        agent_list_target([first.clone(), second.clone()]),
        Target::Fanout(vec![
            (first, RequestBody::AgentThreadList),
            (second, RequestBody::AgentThreadList),
        ])
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let router = Router::new(
        Arc::new(Machines::from_config(&default_config(temp.path()))),
        Arc::new(Mirror::new()),
    );
    assert_eq!(
        router.route(&RequestBody::AgentThreadList),
        Target::Fanout(Vec::new()),
    );
}

#[tokio::test]
async fn agent_listing_creates_partitions_before_any_snapshot_request() {
    let remote_host = host("configured");
    let mut config = default_config("/tmp/fleet-agent-list");
    config.hosts.insert(
        remote_host.clone(),
        HostConfigEntry::Command {
            run: vec!["false".to_owned()],
            fleetd: "fleetd".to_owned(),
            fleet_home: None,
            display: None,
        },
    );
    let router = Router::new(
        Arc::new(Machines::from_config(&config)),
        Arc::new(Mirror::new()),
    );

    assert_eq!(
        router.route(&RequestBody::AgentThreadList),
        Target::Fanout(vec![(remote_host, RequestBody::AgentThreadList)])
    );
}

#[tokio::test]
async fn agent_listing_keeps_cached_remote_threads_when_one_host_is_down() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host("dev-box");
    let worktree = worktree("cached");
    let thread = ThreadId::new();
    let summary =
        ThreadProjection::new(thread, worktree.clone(), AgentKind::Claude).summary(Seq::default());
    let mirror = Arc::new(Mirror::new());
    mirror.apply(
        &host,
        snapshot(
            vec![remote_worktree(worktree)],
            vec![summary],
            temp.path().display().to_string(),
        ),
    );
    let machines = Arc::new(Machines::from_config(&default_config(temp.path())));
    let remote = Arc::new(FakeRemote::new(host.clone()));
    let _state_guard = remote.state_changes();
    remote.set_state(LinkState::Down);
    machines.install_endpoint(host.clone(), remote);
    let router = Router::new(machines, mirror);

    let parts = router
        .fanout(vec![(host.clone(), RequestBody::AgentThreadList)])
        .await;
    let merged = fleet_daemon::services::router::translate::merge_fanout(
        &RequestBody::AgentThreadList,
        parts,
    )
    .expect("cached agent list");
    let ResponseBody::AgentThreads(threads) = merged else {
        panic!("expected agent threads");
    };
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].thread, thread);
    assert_eq!(threads[0].host.as_ref(), Some(&host));
}

#[tokio::test]
async fn forwarded_provider_errors_name_the_execution_host() {
    let host = host("dev-box");
    let machines = Arc::new(Machines::from_config(&default_config("/tmp/fleet-agents")));
    let remote = Arc::new(FakeRemote::new(host.clone()));
    remote.push_response(Err(fleet_daemon::DaemonError::Unsupported(
        "provider unavailable on this machine".to_owned(),
    )));
    machines.install_endpoint(host.clone(), remote);
    let router = Router::new(machines, Arc::new(Mirror::new()));

    let error = router
        .forward(&host, RequestBody::AgentThreadList)
        .await
        .expect_err("provider error");
    assert!(error.to_string().contains("host dev-box"));
}

#[tokio::test]
async fn two_daemon_agent_resume_over_real_remote_link() {
    let _ = infra::DaemonProcess::wait;
    let temp = tempfile::tempdir().expect("tempdir");
    let remote_home = temp.path().join("remote");
    let local_home = temp.path().join("local");
    let worktree = prepare_remote_agent_home(&remote_home).await;
    let remote = infra::RemoteDaemon::start(&remote_home);
    infra::assert_remote_contract(&remote);
    let infra::RemoteDaemon {
        daemon, machine, ..
    } = remote;
    let provider: Arc<dyn MachineProvider> = Arc::new(machine);
    let link = RemoteLink::new(
        provider,
        LinkOptions {
            backoff_min: Duration::from_millis(20),
            backoff_max: Duration::from_millis(100),
            hello_timeout: Duration::from_secs(5),
        },
    );
    let mut states = link.state_changes();
    link.connect().await.expect("remote link");
    let host = link.host().clone();
    let machines = Arc::new(Machines::from_config(&default_config(&local_home)));
    machines.install_endpoint(host.clone(), link.clone());
    let router = Router::new(machines, Arc::new(Mirror::new()));
    router.ids.register_worktree(&host, worktree.clone());
    router.start_event_pumps(BroadcastBus::default());

    let created = router
        .forward(
            &host,
            RequestBody::AgentThreadCreate {
                worktree: worktree.clone(),
                provider: AgentKind::Claude,
                model: None,
                mode: Some(PermissionMode::Ask),
                resume_cursor: None,
                title: None,
            },
        )
        .await
        .expect("create remote agent");
    let ResponseBody::AgentThreadCreated(summary) = created else {
        panic!("expected created thread");
    };
    router
        .forward(
            &host,
            RequestBody::AgentSend {
                thread: summary.thread,
                input: UserInput {
                    text: "continue".to_owned(),
                    attachments: Vec::new(),
                    item: None,
                    origin: Default::default(),
                },
            },
        )
        .await
        .expect("send remote prompt");
    wait_for_remote_text(&router, &host, summary.thread, "remote continuity").await;

    drop(daemon);
    wait_for_link_state(&mut states, LinkState::Down).await;
    let restarted = infra::DaemonProcess::start(&remote_home);
    wait_for_link_state(&mut states, LinkState::Ready).await;
    wait_until(|| router.ids.host_of_thread(&summary.thread) == Some(host.clone())).await;
    let reopened = router
        .forward(
            &host,
            RequestBody::AgentThreadOpen {
                thread: summary.thread,
                from_seq: None,
                after_seq: None,
                turn_limit: None,
                before_cursor: None,
                request_sync_marker: false,
            },
        )
        .await
        .expect("reopen persisted remote agent");
    let ResponseBody::AgentThreadSnapshot { projection, .. } = reopened else {
        panic!("expected thread snapshot");
    };
    assert!(
        projection
            .items
            .iter()
            .any(|item| matches!(&item.kind, fleet_core::agents::ItemKind::AssistantText { text } if text == "remote continuity")),
        "persisted transcript must survive the remote daemon restart"
    );
    assert!(
        !FleetHome::new(&local_home).agents_path().exists(),
        "remote transcripts must never be persisted by the local daemon"
    );
    link.close().await;
    drop(restarted);
}

async fn wait_for_remote_text(router: &Router, host: &HostId, thread: ThreadId, expected: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(ResponseBody::AgentThreadSnapshot { projection, .. }) = router
                .forward(
                    host,
                    RequestBody::AgentThreadOpen {
                        thread,
                        from_seq: None,
                        after_seq: None,
                        turn_limit: None,
                        before_cursor: None,
                        request_sync_marker: false,
                    },
                )
                .await
                && projection
                    .items
                    .iter()
                    .any(|item| matches!(&item.kind, fleet_core::agents::ItemKind::AssistantText { text } if text == expected))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("remote transcript text");
}

async fn wait_for_link_state(
    states: &mut tokio::sync::watch::Receiver<LinkState>,
    expected: LinkState,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if *states.borrow_and_update() == expected {
                return;
            }
            states.changed().await.expect("link state sender");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("link did not reach {expected:?}"));
}

async fn prepare_remote_agent_home(home: &std::path::Path) -> WorktreeId {
    use std::os::unix::fs::PermissionsExt as _;

    let script = home.join("fake-claude");
    let worktree_path = home.join("worktrees/owner/repo/remote-agent");
    let repos = home.join("repos");
    std::fs::create_dir_all(&worktree_path).expect("worktree directory");
    std::fs::create_dir_all(&repos).expect("repos directory");
    std::fs::write(
        &script,
        r##"#!/bin/sh
case " $* " in
  *" --version "*) printf '%s\n' '2.1.266 (Claude Code)'; exit 0 ;;
esac
printf '%s\n' '{"type":"system","subtype":"init","session_id":"cursor-remote","model":"test","tools":[],"slash_commands":[],"capabilities":["interrupt_receipt_v1","interrupt_cancel_queued_v1","msg_lifecycle_v1"]}'
while IFS= read -r line; do
  printf '%s\n' '{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text","text":"remote continuity"}]},"parent_tool_use_id":null}'
  printf '%s\n' '{"type":"result","subtype":"success","terminal_reason":"completed","usage":{}}'
done
"##,
    )
    .expect("fake Claude script");
    let mut permissions = std::fs::metadata(&script)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("script permissions");

    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [repos.clone(), home.join("worktrees")],
    ));
    let config = ConfigStore::new(home, files.clone());
    let mut effective = config.load().await.expect("remote config");
    // `agentBinaries`, not `agentCommands`: a native thread is `execve`'d by the daemon, and the
    // PTY line is a different setting entirely.
    effective.agent_binaries.claude = script.display().to_string();
    config.save(effective).await.expect("save remote config");

    let state = StateStore::new(home, files, Arc::new(SystemClock));
    let context = ContextId::try_from("team").expect("context");
    let repo = RepoId::try_from("owner/repo").expect("repo");
    let worktree = worktree("remote-agent");
    let mut persisted = default_state();
    persisted.contexts.push(Context {
        id: context.clone(),
        name: "Team".to_owned(),
        owners: vec!["owner".to_owned()],
        created_at: Utc::now().to_rfc3339(),
    });
    persisted.repos.push(Repo {
        id: repo.clone(),
        owner: "owner".to_owned(),
        name: "repo".to_owned(),
        url: "https://example.invalid/owner/repo".to_owned(),
        context_id: context,
        default_branch: "main".to_owned(),
        path: repos.join("owner/repo").display().to_string(),
        cloned_at: Utc::now().to_rfc3339(),
        hooks: RepoHooks::default(),
    });
    persisted.worktrees.push(Worktree {
        id: worktree.clone(),
        repo_id: repo,
        slug: "remote-agent".to_owned(),
        branch: "remote-agent".to_owned(),
        base_ref: "main".to_owned(),
        path: worktree_path.display().to_string(),
        session: "owner/repo/remote-agent".to_owned(),
        host: None,
        created_at: Utc::now().to_rfc3339(),
        last_opened_at: None,
        degraded: None,
    });
    state.save(persisted).await.expect("save remote state");
    worktree
}

async fn wait_until(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("condition timeout");
}

fn snapshot(
    worktrees: Vec<Worktree>,
    agent_threads: Vec<fleet_core::agents::AgentThreadSummary>,
    home: String,
) -> Snapshot {
    Snapshot {
        boards: Vec::new(),
        generated_at: Utc::now().to_rfc3339(),
        revision: None,
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees,
        active_context: None,
        sessions: Vec::new(),
        agent_threads,
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: DaemonInfo {
            version: "test".to_owned(),
            pid: 1,
            started_at: Utc::now().to_rfc3339(),
            home,
        },
    }
}

fn remote_worktree(id: WorktreeId) -> Worktree {
    Worktree {
        id,
        repo_id: "owner/repo".parse().expect("repo"),
        slug: "remote-agent".to_owned(),
        branch: "remote-agent".to_owned(),
        base_ref: "main".to_owned(),
        path: "/remote/worktrees/remote-agent".to_owned(),
        session: "owner/repo/remote-agent".to_owned(),
        host: None,
        created_at: Utc::now().to_rfc3339(),
        last_opened_at: None,
        degraded: None,
    }
}

fn host(value: &str) -> HostId {
    value.parse().expect("host")
}

fn worktree(slug: &str) -> WorktreeId {
    format!("owner/repo#{slug}").parse().expect("worktree")
}
