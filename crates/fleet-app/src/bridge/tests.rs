use super::*;

fn empty_seen_cursors() -> Arc<RwLock<HashMap<ThreadId, Seq>>> {
    Arc::new(RwLock::new(HashMap::new()))
}

#[tokio::test]
async fn diagnostic_tail_is_bounded_and_keeps_nonempty_lines() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("logs")).unwrap();
    let path = daemon_log_path(home.path());
    let mut contents = "é".repeat(100_000);
    contents.push_str("\nold\nfirst\n\nsecond\r\nthird\n");
    std::fs::write(&path, contents).unwrap();
    assert_eq!(
        connection::log_tail(home.path()).await,
        ["first", "second", "third"]
    );
    std::fs::write(&path, "x".repeat(100_000)).unwrap();
    assert!(connection::log_tail(home.path()).await.is_empty());
    std::fs::write(&path, "one\n\n two \n").unwrap();
    assert_eq!(connection::log_tail(home.path()).await, ["one", " two "]);
}

#[tokio::test]
async fn offline_requests_finish_without_waiting_for_connection_work() {
    let (requests, receiver) = async_channel::unbounded();
    let (events, _events_rx) = async_channel::unbounded();
    let task = tokio::spawn(requests::run(
        receiver,
        events,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(SettleCounter::default()),
    ));
    let (reply, response) = async_channel::bounded(1);
    requests
        .send(requests::Request::Command {
            client: None,
            body: Box::new(RequestBody::GetSnapshot),
            reply: Some(reply),
            in_flight: InFlight::untracked(),
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(1), response.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        result.unwrap_err().message,
        "the Fleet daemon is not connected"
    );
    drop(requests);
    task.await.unwrap();
}

struct TestDaemon {
    home: tempfile::TempDir,
    sockets: std::sync::mpsc::Receiver<std::os::unix::net::UnixStream>,
    connections: async_channel::Receiver<()>,
    stopping: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TestDaemon {
    fn start(respond: impl FnMut(RequestBody) -> Option<ResponseBody> + Send + 'static) -> Self {
        Self::start_with_metadata(respond, || None, None)
    }

    fn start_with_pong_identity(
        respond: impl FnMut(RequestBody) -> Option<ResponseBody> + Send + 'static,
        pong_identity: impl FnMut() -> Option<(u32, String)> + Send + 'static,
    ) -> Self {
        Self::start_with_metadata(respond, pong_identity, None)
    }

    fn start_with_snapshot_revision(
        respond: impl FnMut(RequestBody) -> Option<ResponseBody> + Send + 'static,
        snapshot_revision: u64,
    ) -> Self {
        Self::start_with_metadata(respond, || None, Some(snapshot_revision))
    }

    fn start_with_metadata(
        mut respond: impl FnMut(RequestBody) -> Option<ResponseBody> + Send + 'static,
        mut pong_identity: impl FnMut() -> Option<(u32, String)> + Send + 'static,
        snapshot_revision: Option<u64>,
    ) -> Self {
        use std::{
            io::{Read, Write},
            os::unix::net::UnixListener,
        };
        let home = tempfile::tempdir().unwrap();
        let listener = UnixListener::bind(FleetHome::new(home.path()).socket_path()).unwrap();
        let (socket_tx, sockets) = std::sync::mpsc::channel();
        let (connection_tx, connections) = async_channel::unbounded();
        let stopping = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stopping = stopping.clone();
        let thread = std::thread::spawn(move || {
            loop {
                let (mut stream, _) = listener.accept().unwrap();
                if thread_stopping.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                socket_tx.send(stream.try_clone().unwrap()).unwrap();
                connection_tx.try_send(()).unwrap();
                loop {
                    let mut length = [0; 4];
                    if stream.read_exact(&mut length).is_err() {
                        break;
                    }
                    let mut body = vec![0; u32::from_be_bytes(length) as usize];
                    if stream.read_exact(&mut body).is_err() {
                        break;
                    }
                    let request: fleet_proto::request::Request =
                        serde_json::from_slice(&body).unwrap();
                    let is_hello = matches!(&request.body, RequestBody::Hello { .. });
                    let result = match request.body {
                        RequestBody::Hello { .. } => Some(ResponseBody::Hello {
                            protocol: fleet_proto::PROTOCOL_VERSION,
                            server: "test-daemon".into(),
                        }),
                        RequestBody::Subscribe { .. } => Some(ResponseBody::Ack),
                        body => respond(body),
                    };
                    if let Some(result) = result {
                        let is_pong = matches!(&result, ResponseBody::Pong);
                        let mut response = serde_json::to_value(fleet_proto::response::Response {
                            id: request.id,
                            result: Ok(result),
                        })
                        .unwrap();
                        if is_pong
                            && let Some((pid, boot_id)) = pong_identity()
                            && let Some(envelope) = response.as_object_mut()
                        {
                            envelope.insert(
                                "daemon".to_owned(),
                                serde_json::json!({"pid": pid, "bootId": boot_id}),
                            );
                        }
                        if let Some(snapshot_revision) = snapshot_revision
                            && let Some(envelope) = response.as_object_mut()
                        {
                            envelope
                                .insert("snapshotRevision".to_owned(), snapshot_revision.into());
                            if is_hello {
                                envelope.insert(
                                    "capabilities".to_owned(),
                                    serde_json::json!(["snapshot.revision"]),
                                );
                            }
                        }
                        let bytes = serde_json::to_vec(&response).unwrap();
                        if stream
                            .write_all(&(bytes.len() as u32).to_be_bytes())
                            .is_err()
                            || stream.write_all(&bytes).is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            home,
            sockets,
            connections,
            stopping,
            thread: Some(thread),
        }
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::Release);
        if let Ok(socket) = self.sockets.recv_timeout(Duration::from_secs(5)) {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        while let Ok(socket) = self.sockets.try_recv() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        let _ =
            std::os::unix::net::UnixStream::connect(FleetHome::new(self.home.path()).socket_path());
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

/// A composer send emits its changed controls first. Those fire-and-forget requests and the
/// reply-bearing send must share one FIFO lane, or the new turn can start before a Claude restart
/// control reaches the daemon and turn a valid boundary change into a mid-turn conflict.
#[tokio::test]
async fn an_agent_send_waits_for_the_preceding_control_acknowledgement() {
    let (observed, observations) = async_channel::unbounded();
    let (release, wait_for_release) = std::sync::mpsc::channel();
    let daemon = TestDaemon::start(move |body| match body {
        RequestBody::AgentSetModel { .. } => {
            observed
                .send_blocking("control")
                .unwrap_or_else(|error| panic!("record control request: {error}"));
            wait_for_release
                .recv()
                .unwrap_or_else(|error| panic!("release control acknowledgement: {error}"));
            Some(ResponseBody::Ack)
        }
        RequestBody::AgentSend { .. } => {
            observed
                .send_blocking("send")
                .unwrap_or_else(|error| panic!("record send request: {error}"));
            Some(ResponseBody::Ack)
        }
        _ => Some(ResponseBody::Ack),
    });
    let client = Client::connect(daemon.home.path())
        .await
        .unwrap_or_else(|error| panic!("connect test client: {error}"));
    let (requests, request_rx) = async_channel::unbounded();
    let (events, _event_rx) = async_channel::unbounded();
    let worker = tokio::spawn(requests::run(
        request_rx,
        events,
        Arc::new(AtomicBool::new(false)),
        Arc::new(SettleCounter::default()),
    ));
    let thread = ThreadId::new();
    requests
        .send(requests::Request::Command {
            client: Some(client.clone()),
            body: Box::new(RequestBody::AgentSetModel {
                thread,
                model: fleet_core::agents::ModelSelection {
                    model: "claude-opus-5".to_owned(),
                    effort: Some("high".to_owned()),
                    provider: None,
                },
            }),
            reply: None,
            in_flight: InFlight::untracked(),
        })
        .await
        .unwrap_or_else(|error| panic!("enqueue control: {error}"));
    let (reply, answer) = async_channel::bounded(1);
    requests
        .send(requests::Request::Command {
            client: Some(client),
            body: Box::new(RequestBody::AgentSend {
                thread,
                input: fleet_core::agents::UserInput {
                    text: "explain this crate".to_owned(),
                    attachments: Vec::new(),
                    item: Some(fleet_core::agents::ItemId::new()),
                    ..fleet_core::agents::UserInput::default()
                },
            }),
            reply: Some(reply),
            in_flight: InFlight::untracked(),
        })
        .await
        .unwrap_or_else(|error| panic!("enqueue send: {error}"));

    let first = observations
        .recv()
        .await
        .unwrap_or_else(|error| panic!("observe first request: {error}"));
    release
        .send(())
        .unwrap_or_else(|error| panic!("release control response: {error}"));
    assert_eq!(first, "control", "the send overtook its pending control");
    assert_eq!(
        observations
            .recv()
            .await
            .unwrap_or_else(|error| panic!("observe second request: {error}")),
        "send"
    );
    assert!(matches!(
        answer
            .recv()
            .await
            .unwrap_or_else(|error| panic!("receive send acknowledgement: {error}")),
        Ok(ResponseBody::Ack)
    ));

    drop(requests);
    worker
        .await
        .unwrap_or_else(|error| panic!("join request worker: {error}"));
}

fn empty_snapshot() -> Snapshot {
    Snapshot {
        boards: Vec::new(),
        generated_at: String::new(),
        revision: None,
        contexts: vec![],
        repos: vec![],
        clones: vec![],
        worktrees: vec![],
        active_context: None,
        sessions: vec![],
        agent_threads: Vec::new(),
        statuses: vec![],
        pools: vec![],
        hosts: vec![],
        jobs: vec![],
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: "test".into(),
            pid: 1,
            started_at: String::new(),
            home: String::new(),
        },
    }
}

#[tokio::test]
async fn stalled_health_check_does_not_delay_fifo_input_or_shutdown() {
    let (stalled, health_started) = async_channel::bounded(1);
    let (input, received) = async_channel::unbounded();
    let mut pings = 0;
    let daemon = TestDaemon::start(move |body| match body {
        RequestBody::DaemonPing => {
            pings += 1;
            if pings > 1 {
                let _ = stalled.try_send(());
                None
            } else {
                Some(ResponseBody::Pong)
            }
        }
        RequestBody::GetSnapshot => Some(ResponseBody::Snapshot(empty_snapshot())),
        RequestBody::GetConfig => Some(ResponseBody::Config(fleet_core::config::default_config(
            "/tmp/fleet-test",
        ))),
        RequestBody::TerminalInput { bytes, .. } => {
            input.try_send(bytes).unwrap();
            Some(ResponseBody::Ack)
        }
        _ => Some(ResponseBody::Ack),
    });
    let (commands, command_rx) = async_channel::unbounded();
    let (events, event_rx) = async_channel::unbounded();
    let resync_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let home = daemon.home.path().to_owned();
    let runtime_resync = resync_pending.clone();
    let task = tokio::spawn(async move {
        runtime::run(
            &home,
            &command_rx,
            &events,
            &runtime_resync,
            Arc::new(SettleCounter::default()),
            empty_seen_cursors(),
        )
        .await
    });
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        if matches!(event, BridgeEvent::Connected(_)) {
            break;
        }
    }
    tokio::time::timeout(Duration::from_secs(5), health_started.recv())
        .await
        .unwrap()
        .unwrap();
    for index in 0..128_u8 {
        commands
            .send(Command::Request {
                body: Box::new(RequestBody::TerminalInput {
                    terminal: fleet_core::ids::TerminalId(1),
                    bytes: vec![index],
                }),
                reply: None,
                in_flight: InFlight::untracked(),
            })
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        for index in 0..128_u8 {
            assert_eq!(received.recv().await.unwrap(), [index]);
        }
    })
    .await
    .unwrap();
    commands.send(Command::Shutdown).await.unwrap();
    tokio::time::timeout(Duration::from_millis(250), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_manual_reconnect_reopens_a_link_that_is_still_alive() {
    let daemon = TestDaemon::start(|body| match body {
        RequestBody::DaemonPing => Some(ResponseBody::Pong),
        RequestBody::GetConfig => Some(ResponseBody::Config(fleet_core::config::default_config(
            "/tmp/fleet-test",
        ))),
        RequestBody::GetSnapshot => Some(ResponseBody::Snapshot(empty_snapshot())),
        _ => Some(ResponseBody::Ack),
    });
    let (commands, command_rx) = async_channel::unbounded();
    let (events, event_rx) = async_channel::unbounded();
    let resync_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let home = daemon.home.path().to_owned();
    let runtime_resync = resync_pending.clone();
    let task = tokio::spawn(async move {
        runtime::run(
            &home,
            &command_rx,
            &events,
            &runtime_resync,
            Arc::new(SettleCounter::default()),
            empty_seen_cursors(),
        )
        .await
    });
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        if matches!(event, BridgeEvent::Connected(_)) {
            break;
        }
    }
    daemon.connections.recv().await.unwrap();

    // The app writes `DaemonLink::Starting` the moment `r` is pressed; the banner it replaces
    // is gone, so a reconnect the runtime silently drops would leave no way back.
    commands.send(Command::Reconnect).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), daemon.connections.recv())
        .await
        .expect("an explicit reconnect opens a fresh connection")
        .unwrap();

    commands.send(Command::Shutdown).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_stalled_mutation_does_not_delay_shutdown() {
    let daemon = TestDaemon::start(|body| match body {
        RequestBody::DaemonPing => Some(ResponseBody::Pong),
        RequestBody::GetConfig => Some(ResponseBody::Config(fleet_core::config::default_config(
            "/tmp/fleet-test",
        ))),
        RequestBody::GetSnapshot => Some(ResponseBody::Snapshot(empty_snapshot())),
        // The daemon accepts the keystroke and never answers it.
        RequestBody::TerminalInput { .. } => None,
        _ => Some(ResponseBody::Ack),
    });
    let (commands, command_rx) = async_channel::unbounded();
    let (events, event_rx) = async_channel::unbounded();
    let resync_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let home = daemon.home.path().to_owned();
    let runtime_resync = resync_pending.clone();
    let task = tokio::spawn(async move {
        runtime::run(
            &home,
            &command_rx,
            &events,
            &runtime_resync,
            Arc::new(SettleCounter::default()),
            empty_seen_cursors(),
        )
        .await
    });
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        if matches!(event, BridgeEvent::Connected(_)) {
            break;
        }
    }

    // Enough to fill the admission lane, the mutation lane, and the hand of each worker
    // parked between them, so the next send is what would park the control loop.
    for index in 0..(4 * COMMAND_CAPACITY) {
        commands
            .send(Command::Request {
                body: Box::new(RequestBody::TerminalInput {
                    terminal: fleet_core::ids::TerminalId(1),
                    bytes: vec![index as u8],
                }),
                reply: None,
                in_flight: InFlight::untracked(),
            })
            .await
            .unwrap();
    }
    commands.send(Command::Shutdown).await.unwrap();

    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("a saturated mutation lane must not hold the control loop")
        .unwrap();
}

#[tokio::test]
async fn shutdown_is_observed_while_initial_connection_is_waiting() {
    let (stalled, opening) = async_channel::bounded(1);
    let daemon = TestDaemon::start(move |body| {
        if matches!(body, RequestBody::DaemonPing) {
            stalled.try_send(()).unwrap();
            None
        } else {
            Some(ResponseBody::Ack)
        }
    });
    let (commands, command_rx) = async_channel::unbounded();
    let (events, _event_rx) = async_channel::unbounded();
    let resync_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let home = daemon.home.path().to_owned();
    let runtime_resync = resync_pending.clone();
    let task = tokio::spawn(async move {
        runtime::run(
            &home,
            &command_rx,
            &events,
            &runtime_resync,
            Arc::new(SettleCounter::default()),
            empty_seen_cursors(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), opening.recv())
        .await
        .unwrap()
        .unwrap();
    commands.send(Command::Shutdown).await.unwrap();
    tokio::time::timeout(Duration::from_millis(250), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn failed_health_ping_recovers_on_a_fresh_connection_without_disconnect() {
    let mut pings = 0;
    let daemon = TestDaemon::start(move |body| match body {
        RequestBody::DaemonPing => {
            pings += 1;
            Some(if pings == 2 {
                ResponseBody::Ack
            } else {
                ResponseBody::Pong
            })
        }
        RequestBody::GetConfig => Some(ResponseBody::Config(fleet_core::config::default_config(
            "/tmp/fleet-test",
        ))),
        RequestBody::GetSnapshot => Some(ResponseBody::Snapshot(empty_snapshot())),
        _ => Some(ResponseBody::Ack),
    });
    let (commands, command_rx) = async_channel::unbounded();
    let (events, event_rx) = async_channel::unbounded();
    let resync_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let home = daemon.home.path().to_owned();
    let runtime_resync = resync_pending.clone();
    let task = tokio::spawn(async move {
        runtime::run_with_intervals(
            &home,
            &command_rx,
            &events,
            &runtime_resync,
            Arc::new(SettleCounter::default()),
            empty_seen_cursors(),
            runtime::RuntimeIntervals {
                health: Duration::from_millis(10),
                identity: IDENTITY_INTERVAL,
            },
        )
        .await
    });

    let reconnected = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match event_rx.recv().await.unwrap() {
                BridgeEvent::Disconnected { attempt } => {
                    panic!("recovery probe emitted Disconnected {{ attempt: {attempt} }}")
                }
                BridgeEvent::Reconnected { restarted, .. } => break restarted,
                _ => {}
            }
        }
    })
    .await
    .unwrap();

    assert!(!reconnected, "the recovery connection kept the daemon PID");
    commands.send(Command::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn failed_recovery_probe_enters_normal_backoff() {
    let mut pings = 0;
    let daemon = TestDaemon::start(move |body| match body {
        RequestBody::DaemonPing => {
            pings += 1;
            Some(if pings == 1 {
                ResponseBody::Pong
            } else {
                ResponseBody::Ack
            })
        }
        RequestBody::GetConfig => Some(ResponseBody::Config(fleet_core::config::default_config(
            "/tmp/fleet-test",
        ))),
        RequestBody::GetSnapshot => Some(ResponseBody::Snapshot(empty_snapshot())),
        _ => Some(ResponseBody::Ack),
    });
    let (commands, command_rx) = async_channel::unbounded();
    let (events, event_rx) = async_channel::unbounded();
    let resync_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let home = daemon.home.path().to_owned();
    let runtime_resync = resync_pending.clone();
    let task = tokio::spawn(async move {
        runtime::run_with_intervals(
            &home,
            &command_rx,
            &events,
            &runtime_resync,
            Arc::new(SettleCounter::default()),
            empty_seen_cursors(),
            runtime::RuntimeIntervals {
                health: Duration::from_millis(10),
                identity: IDENTITY_INTERVAL,
            },
        )
        .await
    });
    daemon.connections.recv().await.unwrap();
    loop {
        match event_rx.recv().await.unwrap() {
            BridgeEvent::Disconnected { attempt: 0 } => break,
            BridgeEvent::Disconnected { attempt } => {
                panic!("first disconnect had attempt {attempt}")
            }
            _ => {}
        }
    }
    daemon.connections.recv().await.unwrap();

    assert!(
        tokio::time::timeout(Duration::from_millis(100), daemon.connections.recv())
            .await
            .is_err(),
        "ticker ticks must not trigger an early reconnect"
    );
    tokio::time::timeout(Duration::from_millis(1_200), daemon.connections.recv())
        .await
        .unwrap()
        .unwrap();
    commands.send(Command::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[test]
fn manual_reconnect_reports_restart_identity() {
    let reason = runtime::manual_opening_reason(runtime::reconnecting(41, 2));
    let event = runtime::opened_event(reason, 42, empty_snapshot());

    assert!(matches!(
        event,
        BridgeEvent::Reconnected {
            restarted: true,
            ..
        }
    ));
}

#[tokio::test]
async fn health_check_uses_daemon_ping_without_loading_a_snapshot() {
    let (snapshots, snapshot_requested) = async_channel::bounded(1);
    let daemon = TestDaemon::start(move |body| match body {
        RequestBody::GetSnapshot => {
            snapshots.try_send(()).unwrap();
            Some(ResponseBody::Snapshot(empty_snapshot()))
        }
        RequestBody::DaemonPing => Some(ResponseBody::Pong),
        _ => Some(ResponseBody::Ack),
    });
    let client = Client::connect(daemon.home.path()).await.unwrap();

    assert!(connection::check_health(&client).await.is_ok());
    assert!(snapshot_requested.try_recv().is_err());
}

#[tokio::test]
async fn pong_identity_probe_detects_the_daemon_pid_without_a_snapshot() {
    let daemon = TestDaemon::start_with_pong_identity(
        |body| match body {
            RequestBody::DaemonPing => Some(ResponseBody::Pong),
            _ => Some(ResponseBody::Ack),
        },
        || Some((42, "boot-42".to_owned())),
    );
    let client = Client::connect(daemon.home.path()).await.unwrap();

    assert_eq!(
        connection::daemon_identity(&client).await,
        Some((42, "boot-42".to_owned()))
    );
}

#[tokio::test]
async fn transparent_reconnect_detects_new_daemon_pid() {
    let mut snapshots = 0;
    let daemon = TestDaemon::start_with_pong_identity(
        move |body| match body {
            RequestBody::DaemonPing => Some(ResponseBody::Pong),
            RequestBody::GetConfig => Some(ResponseBody::Config(
                fleet_core::config::default_config("/tmp/fleet-test"),
            )),
            RequestBody::GetSnapshot => {
                snapshots += 1;
                let mut snapshot = empty_snapshot();
                snapshot.daemon.pid = if snapshots == 1 { 41 } else { 42 };
                Some(ResponseBody::Snapshot(snapshot))
            }
            _ => Some(ResponseBody::Ack),
        },
        || Some((42, "boot-42".to_owned())),
    );
    let (commands, command_rx) = async_channel::unbounded();
    let (events, event_rx) = async_channel::unbounded();
    let resync_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let home = daemon.home.path().to_owned();
    let runtime_resync = resync_pending.clone();
    let task = tokio::spawn(async move {
        runtime::run_with_intervals(
            &home,
            &command_rx,
            &events,
            &runtime_resync,
            Arc::new(SettleCounter::default()),
            empty_seen_cursors(),
            runtime::RuntimeIntervals {
                health: Duration::from_secs(1),
                identity: Duration::from_millis(10),
            },
        )
        .await
    });
    let mut connected = false;
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match event_rx.recv().await.unwrap() {
                BridgeEvent::Connected(snapshot) => connected = snapshot.daemon.pid == 41,
                BridgeEvent::Disconnected { attempt: 0 } => break,
                _ => {}
            }
        }
    })
    .await
    .unwrap();

    assert!(connected);
    commands.send(Command::Shutdown).await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn baseline_precedes_forwarded_events() {
    let (source_tx, source) = tokio::sync::broadcast::channel(4);
    let (events, event_rx) = async_channel::bounded(4);
    source_tx
        .send(Event::Toast {
            level: fleet_proto::event::ToastLevel::Info,
            message: "newer".to_owned(),
        })
        .unwrap();
    let mut forwarder = connection::Forwarder::pending(source);
    assert!(event_rx.try_recv().is_err());

    events
        .send(BridgeEvent::Connected(Box::new(empty_snapshot())))
        .await
        .unwrap();
    forwarder.start(events);

    assert!(matches!(
        event_rx.recv().await.unwrap(),
        BridgeEvent::Connected(_)
    ));
    assert!(matches!(
        event_rx.recv().await.unwrap(),
        BridgeEvent::Daemon(event) if matches!(*event, Event::Toast { .. })
    ));
}

#[tokio::test]
async fn bounded_queues_coalesce_and_resynchronize() {
    let (commands, command_rx) = async_channel::bounded(2);
    let (event_tx, events) = async_channel::bounded(16);
    let resync_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bridge = Bridge::with_channels(commands, events, event_tx.clone(), resync_pending.clone());
    let event_rx = bridge.events();
    for terminal in 1..=3 {
        bridge.send(RequestBody::RequestFullFrame {
            terminal: fleet_core::ids::TerminalId(terminal),
        });
    }
    assert_eq!(command_rx.len(), 2);
    assert!(resync_pending.load(std::sync::atomic::Ordering::Acquire));
    assert!(matches!(
        event_rx.recv().await.unwrap(),
        BridgeEvent::MutationFailed { message }
            if message == "the Fleet daemon bridge queue was saturated"
    ));

    let daemon = TestDaemon::start(|body| match body {
        RequestBody::DaemonPing => Some(ResponseBody::Pong),
        RequestBody::GetConfig => Some(ResponseBody::Config(fleet_core::config::default_config(
            "/tmp/fleet-test",
        ))),
        RequestBody::GetSnapshot => Some(ResponseBody::Snapshot(empty_snapshot())),
        _ => Some(ResponseBody::Ack),
    });
    let home = daemon.home.path().to_owned();
    let runtime_resync = resync_pending.clone();
    let task = tokio::spawn(async move {
        runtime::run(
            &home,
            &command_rx,
            &event_tx,
            &runtime_resync,
            Arc::new(SettleCounter::default()),
            empty_seen_cursors(),
        )
        .await
    });
    let mut saw_lag = false;
    let mut saw_snapshot = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !saw_lag || !saw_snapshot {
            match event_rx.recv().await.unwrap() {
                BridgeEvent::EventsLagged { .. } => saw_lag = true,
                BridgeEvent::Daemon(event) if matches!(*event, Event::SnapshotChanged(_)) => {
                    saw_snapshot = true;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(!resync_pending.load(std::sync::atomic::Ordering::Acquire));
    bridge.shutdown();
    task.await.unwrap();
}

#[tokio::test]
async fn a_stamped_mutation_reply_waits_for_a_covering_snapshot() {
    let daemon = TestDaemon::start_with_snapshot_revision(|_| Some(ResponseBody::Ack), 12);
    let client = Client::connect(daemon.home.path()).await.unwrap();

    let (commands, command_rx) = async_channel::unbounded();
    let (event_tx, events) = async_channel::unbounded();
    let bridge = Bridge::with_channels(
        commands,
        events,
        event_tx.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let event_rx = bridge.events();
    let counter = bridge.in_flight_requests();
    let settle = bridge.settle_counter();
    bridge.send(RequestBody::SetActiveContext { id: None });

    let Command::Request {
        body,
        reply,
        in_flight,
    } = command_rx.recv().await.unwrap()
    else {
        panic!("mutation admission produced a control command");
    };
    let (request_tx, request_rx) = async_channel::unbounded();
    let worker = tokio::spawn(requests::run(
        request_rx,
        event_tx,
        Arc::new(AtomicBool::new(false)),
        Arc::clone(&settle),
    ));
    request_tx
        .send(requests::Request::Command {
            client: Some(client),
            body,
            reply,
            in_flight,
        })
        .await
        .unwrap();

    assert!(matches!(event_rx.recv().await.unwrap(), BridgeEvent::Nudge));
    assert_eq!(counter.load(Ordering::Acquire), 0);
    assert_eq!(settle.pending(), 1);

    settle.applied(Some(11));
    assert_eq!(settle.pending(), 1);
    settle.applied(Some(12));
    assert_eq!(settle.pending(), 0);
    drop(request_tx);
    worker.await.unwrap();
}

#[tokio::test]
async fn legacy_mutation_settle_grace_expiry_sends_a_nudge() {
    let (event_tx, event_rx) = async_channel::bounded(1);
    let settle = Arc::new(SettleCounter::default());
    let generation = settle.begin(None);
    requests::expire_settle_after(
        Arc::clone(&settle),
        idle_wake(event_tx),
        generation,
        Duration::ZERO,
    )
    .await;

    assert!(matches!(event_rx.recv().await.unwrap(), BridgeEvent::Nudge));
    assert_eq!(settle.pending(), 0);
}

#[tokio::test]
async fn stamped_mutation_settle_grace_expiry_retains_the_claim_without_a_nudge() {
    let (event_tx, event_rx) = async_channel::bounded(1);
    let settle = Arc::new(SettleCounter::default());
    let generation = settle.begin(Some(12));
    requests::expire_settle_after(
        Arc::clone(&settle),
        idle_wake(event_tx),
        generation,
        Duration::ZERO,
    )
    .await;

    assert!(event_rx.try_recv().is_err());
    assert_eq!(settle.pending(), 1);
}

/// `docs/TESTING-HARNESS.md` §2 defines `idle` as "no in-flight app requests, …". A reply-lane
/// claim remains live until the receiver that owns the answer is finished with it.
#[tokio::test]
async fn the_in_flight_counter_covers_a_request_until_its_receiver_is_finished() {
    let (commands, command_rx) = async_channel::unbounded();
    let (event_tx, events) = async_channel::unbounded();
    let bridge = Bridge::with_channels(
        commands,
        events,
        event_tx.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let event_rx = bridge.events();
    let counter = bridge.in_flight_requests();
    assert_eq!(counter.load(Ordering::Acquire), 0);

    let answer = bridge.request(RequestBody::GetSnapshot);
    assert_eq!(
        counter.load(Ordering::Acquire),
        1,
        "the reply-lane request is claimed on admission"
    );

    // The runtime delivers the answer, then waits for this test's receiver to finish with it.
    // Draining the queue here stands in for the runtime admission loop.
    let (requests, request_rx) = async_channel::unbounded();
    let task = tokio::spawn(requests::run(
        request_rx,
        event_tx,
        Arc::new(AtomicBool::new(false)),
        bridge.settle_counter(),
    ));
    while let Ok(command) = command_rx.try_recv() {
        let Command::Request {
            body,
            reply,
            in_flight,
        } = command
        else {
            continue;
        };
        requests
            .send(requests::Request::Command {
                client: None,
                body,
                reply,
                in_flight,
            })
            .await
            .unwrap_or_else(|error| panic!("{error}"));
    }
    drop(requests);
    task.await.unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(
        counter.load(Ordering::Acquire),
        1,
        "the reply-lane claim remains until its waiting receiver is finished"
    );
    assert!(
        answer.recv().await.is_ok(),
        "the waiting caller was answered"
    );
    drop(answer);
    while counter.load(Ordering::Acquire) != 0 {
        assert!(matches!(event_rx.recv().await.unwrap(), BridgeEvent::Nudge));
    }
    assert_eq!(counter.load(Ordering::Acquire), 0);

    let unread = bridge.request(RequestBody::GetSnapshot);
    let Command::Request {
        body,
        reply,
        in_flight,
    } = command_rx.recv().await.unwrap()
    else {
        panic!("request admission produced a control command");
    };
    let (requests, request_rx) = async_channel::unbounded();
    let task = tokio::spawn(requests::run(
        request_rx,
        bridge.event_tx.clone(),
        Arc::new(AtomicBool::new(false)),
        bridge.settle_counter(),
    ));
    requests
        .send(requests::Request::Command {
            client: None,
            body,
            reply,
            in_flight,
        })
        .await
        .unwrap();
    drop(requests);
    task.await.unwrap();
    assert_eq!(
        counter.load(Ordering::Acquire),
        1,
        "an unread answer remains in flight"
    );
    drop(unread);
    while counter.load(Ordering::Acquire) != 0 {
        assert!(matches!(event_rx.recv().await.unwrap(), BridgeEvent::Nudge));
    }
    assert_eq!(counter.load(Ordering::Acquire), 0);
}
