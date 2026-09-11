//! Durable read-through mirror behaviour for remote agent threads.

use super::*;

/// A scripted stand-in for the daemon's durable mirror.
///
/// The real one is the agent service over SQLite and it is tested there. What this proves is the
/// *routing* half of §9.3: which of the three moments the router hands the mirror, in what order,
/// and what it does with the answers.
#[derive(Default)]
struct FakeMirror {
    warm: std::sync::Mutex<Option<ResponseBody>>,
    delta: std::sync::Mutex<Option<RequestBody>>,
    cached: std::sync::Mutex<Vec<fleet_core::agents::AgentThreadSummary>>,
    calls: std::sync::Mutex<Vec<String>>,
    absorbed: std::sync::Mutex<Vec<ResponseBody>>,
    ingested: std::sync::Mutex<Vec<Seq>>,
}

impl FakeMirror {
    fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }
    fn absorbed(&self) -> Vec<ResponseBody> {
        lock(&self.absorbed).clone()
    }
    fn ingested(&self) -> Vec<Seq> {
        lock(&self.ingested).clone()
    }
    fn record(&self, call: &str) {
        lock(&self.calls).push(call.to_owned());
    }
}

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[async_trait::async_trait]
impl AgentMirror for FakeMirror {
    async fn adopt(&self, _host: &HostId, summaries: &[fleet_core::agents::AgentThreadSummary]) {
        self.record("adopt");
        lock(&self.cached).extend(summaries.iter().cloned());
    }

    async fn ingest(&self, _host: &HostId, _thread: ThreadId, event: &SeqEvent) -> MirrorWrite {
        self.record("ingest");
        lock(&self.ingested).push(event.seq);
        MirrorWrite::Stored
    }

    async fn open(
        &self,
        _host: &HostId,
        _body: &RequestBody,
    ) -> Option<DaemonResult<ResponseBody>> {
        self.record("open");
        lock(&self.warm).clone().map(Ok)
    }

    async fn delta(&self, _host: &HostId, _body: &RequestBody) -> Option<RequestBody> {
        self.record("delta");
        lock(&self.delta).clone()
    }

    async fn absorb(
        &self,
        _host: &HostId,
        _body: &RequestBody,
        response: &ResponseBody,
        announce: bool,
    ) -> bool {
        self.record(if announce {
            "absorb+announce"
        } else {
            "absorb"
        });
        lock(&self.absorbed).push(response.clone());
        true
    }

    async fn cached_threads(&self, _host: &HostId) -> Vec<fleet_core::agents::AgentThreadSummary> {
        self.record("cached_threads");
        lock(&self.cached).clone()
    }
}

/// An endpoint whose peer advertises the windowed-transcript capability.
fn windowing_remote(host: &HostId) -> Arc<FakeRemote> {
    let remote = Arc::new(FakeRemote::new(host.clone()));
    remote.set_hello(RemoteHello {
        version: "fleetd test".to_owned(),
        daemon_id: "owner".to_owned(),
        build_commit: None,
        capabilities: vec![
            fleet_proto::REMOTE_MACHINES_CAPABILITY.to_owned(),
            fleet_proto::AGENT_WINDOW_CAPABILITY.to_owned(),
        ],
    });
    remote
}

fn window_open(thread: ThreadId, after_seq: Option<Seq>) -> RequestBody {
    RequestBody::AgentThreadOpen {
        thread,
        from_seq: None,
        after_seq,
        turn_limit: Some(10),
        before_cursor: None,
        request_sync_marker: after_seq.is_some(),
    }
}

fn thread_window(summary: fleet_core::agents::AgentThreadSummary) -> ResponseBody {
    ResponseBody::AgentThreadWindow(Box::new(fleet_proto::agents::AgentThreadWindow {
        head_seq: summary.last_seq,
        projected_seq: summary.last_seq,
        summary,
        session: fleet_proto::agents::AgentSessionView::default(),
        window: fleet_proto::agents::TranscriptWindow::default(),
        page: None,
        events_after: Vec::new(),
        synchronized: false,
    }))
}

#[tokio::test]
async fn a_warm_mirror_answers_an_open_and_asks_the_owner_only_for_the_delta() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host("dev-box");
    let thread = ThreadId::new();
    let mut summary =
        ThreadProjection::new(thread, worktree("warm"), AgentKind::Claude).summary(Seq::default());
    summary.last_seq = Seq(12);
    let machines = Arc::new(Machines::from_config(&default_config(temp.path())));
    let remote = windowing_remote(&host);
    machines.install_endpoint(host.clone(), remote.clone());
    let router = Router::new(machines, Arc::new(Mirror::new()));
    let mirror = Arc::new(FakeMirror::default());
    *lock(&mirror.warm) = Some(thread_window(summary.clone()));
    *lock(&mirror.delta) = Some(window_open(thread, Some(Seq(12))));
    router.set_agent_mirror(mirror.clone());
    router.ids.register_thread(&host, thread);

    let answer = router
        .forward(&host, window_open(thread, None))
        .await
        .expect("the mirror answers the open");

    // The transcript came out of the local database: the owner was never asked for it.
    match answer {
        ResponseBody::AgentThreadWindow(window) => {
            assert_eq!(window.summary.thread, thread);
            assert!(!window.synchronized, "only the owner may say `live`");
        }
        other => panic!("expected a window, got {other:?}"),
    }
    // One round trip, and it asks only for `(12, head]` — zero transcript bytes for what the
    // mirror already holds.
    wait_until(|| !remote.requests().is_empty()).await;
    assert_eq!(remote.requests(), vec![window_open(thread, Some(Seq(12)))]);
    assert_eq!(mirror.calls()[..2], ["open".to_owned(), "delta".to_owned()]);
}

#[tokio::test]
async fn a_cold_mirror_is_proxied_and_the_answer_warms_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host("dev-box");
    let thread = ThreadId::new();
    let summary =
        ThreadProjection::new(thread, worktree("cold"), AgentKind::Claude).summary(Seq::default());
    let machines = Arc::new(Machines::from_config(&default_config(temp.path())));
    let remote = windowing_remote(&host);
    machines.install_endpoint(host.clone(), remote.clone());
    let router = Router::new(machines, Arc::new(Mirror::new()));
    let mirror = Arc::new(FakeMirror::default());
    router.set_agent_mirror(mirror.clone());
    router.ids.register_thread(&host, thread);
    remote.push_response(Ok(thread_window(summary)));

    let open = window_open(thread, None);
    router
        .forward(&host, open.clone())
        .await
        .expect("proxied open");

    assert_eq!(remote.requests(), vec![open]);
    assert_eq!(mirror.absorbed().len(), 1, "the answer warms the mirror");
    assert!(
        !mirror.calls().contains(&"delta".to_owned()),
        "a cold mirror has no delta to ask for"
    );
}

#[tokio::test]
async fn a_mutation_is_forwarded_and_never_offered_to_the_mirror() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host("dev-box");
    let thread = ThreadId::new();
    let machines = Arc::new(Machines::from_config(&default_config(temp.path())));
    let remote = windowing_remote(&host);
    machines.install_endpoint(host.clone(), remote.clone());
    let router = Router::new(machines, Arc::new(Mirror::new()));
    let mirror = Arc::new(FakeMirror::default());
    router.set_agent_mirror(mirror.clone());
    router.ids.register_thread(&host, thread);
    remote.push_response(Ok(ResponseBody::AgentAck));

    let respond = RequestBody::AgentRespond {
        thread,
        gate: fleet_core::agents::GateId::new(),
        answer: fleet_core::agents::GateAnswer::Permission {
            choice: fleet_core::agents::PermissionChoice::AllowOnce,
            edited_payload: None,
        },
    };
    assert_eq!(router.route(&respond), Target::Host(host.clone()));
    router
        .forward(&host, respond.clone())
        .await
        .expect("the owner answers the mutation");

    assert_eq!(remote.requests(), vec![respond]);
    // The mirror is never consulted for a write, and an ack carries nothing it may take.
    assert!(!mirror.calls().contains(&"open".to_owned()));
    assert!(mirror.absorbed().iter().all(|response| !matches!(
        response,
        ResponseBody::AgentThreadWindow(_) | ResponseBody::AgentThreadSnapshot { .. }
    )));
}

#[tokio::test]
async fn an_unreachable_host_lists_its_threads_from_the_durable_mirror() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host("dev-box");
    let thread = ThreadId::new();
    let summary = ThreadProjection::new(thread, worktree("offline"), AgentKind::Claude)
        .summary(Seq::default());
    let machines = Arc::new(Machines::from_config(&default_config(temp.path())));
    let remote = windowing_remote(&host);
    let _states = remote.state_changes();
    remote.set_state(LinkState::Down);
    machines.install_endpoint(host.clone(), remote);
    // No in-memory snapshot fragment at all: the durable mirror is the only cache left, which is
    // the state a daemon restarts into.
    let router = Router::new(machines, Arc::new(Mirror::new()));
    let mirror = Arc::new(FakeMirror::default());
    *lock(&mirror.cached) = vec![summary];
    router.set_agent_mirror(mirror);

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
async fn a_remote_event_is_mirrored_before_it_is_republished() {
    let host = host("dev-box");
    let thread = ThreadId::new();
    let machines = Arc::new(Machines::from_config(&default_config("/tmp/fleet-mirror")));
    let remote = windowing_remote(&host);
    machines.install_endpoint(host.clone(), remote.clone());
    let router = Router::new(machines, Arc::new(Mirror::new()));
    let mirror = Arc::new(FakeMirror::default());
    router.set_agent_mirror(mirror.clone());
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    router.start_event_pumps(events);

    let sequenced = SeqEvent {
        seq: Seq(4),
        at: Utc::now(),
        raw: None,
        event: AgentEvent::Notice("owner said so".to_owned()),
    };
    remote.emit(Event::Agent {
        thread,
        event: sequenced.clone(),
    });

    let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("republished event timeout")
        .expect("republished event");
    assert_eq!(
        event,
        Event::Agent {
            thread,
            event: sequenced
        }
    );
    // Durable before visible: the event was already in the mirror when the local bus saw it.
    assert_eq!(mirror.ingested(), vec![Seq(4)]);
}

#[tokio::test]
async fn a_disconnected_host_still_reads_its_mirrored_transcript() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host("dev-box");
    let thread = ThreadId::new();
    let mut summary = ThreadProjection::new(thread, worktree("offline"), AgentKind::Claude)
        .summary(Seq::default());
    summary.last_seq = Seq(7);
    let machines = Arc::new(Machines::from_config(&default_config(temp.path())));
    let remote = windowing_remote(&host);
    let _states = remote.state_changes();
    remote.set_state(LinkState::Down);
    machines.install_endpoint(host.clone(), remote.clone());
    let router = Router::new(machines, Arc::new(Mirror::new()));
    let mirror = Arc::new(FakeMirror::default());
    *lock(&mirror.warm) = Some(thread_window(summary));
    *lock(&mirror.delta) = Some(window_open(thread, Some(Seq(7))));
    router.set_agent_mirror(mirror.clone());
    router.ids.register_thread(&host, thread);

    // Disconnection changes the status, never the data.
    let answer = router
        .forward(&host, window_open(thread, None))
        .await
        .expect("a mirrored transcript stays readable while its host is unreachable");
    let ResponseBody::AgentThreadWindow(window) = answer else {
        panic!("expected a window");
    };
    assert_eq!(window.summary.thread, thread);

    // A mutation is the thing that fails, and it fails as unreachable.
    let error = router
        .forward(
            &host,
            RequestBody::AgentSend {
                thread,
                input: UserInput {
                    text: "queued?".to_owned(),
                    attachments: Vec::new(),
                    item: None,
                },
            },
        )
        .await
        .expect_err("a mutation on an unreachable host fails");
    assert!(error.to_string().contains("dev-box"));
    assert!(
        remote.requests().is_empty(),
        "nothing was sent over a link that is down"
    );
}
