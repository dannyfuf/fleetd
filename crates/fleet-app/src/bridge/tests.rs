use super::*;

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
    let task = tokio::spawn(requests::run(receiver, events));
    let (reply, response) = async_channel::bounded(1);
    requests
        .send(requests::Request {
            client: None,
            body: Box::new(RequestBody::GetSnapshot),
            reply: Some(reply),
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
    socket: std::sync::mpsc::Receiver<std::os::unix::net::UnixStream>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TestDaemon {
    fn start(
        mut respond: impl FnMut(RequestBody) -> Option<ResponseBody> + Send + 'static,
    ) -> Self {
        use std::{
            io::{Read, Write},
            os::unix::net::UnixListener,
        };
        let home = tempfile::tempdir().unwrap();
        let listener = UnixListener::bind(FleetHome::new(home.path()).socket_path()).unwrap();
        let (socket_tx, socket) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            socket_tx.send(stream.try_clone().unwrap()).unwrap();
            loop {
                let mut length = [0; 4];
                if stream.read_exact(&mut length).is_err() {
                    break;
                }
                let mut body = vec![0; u32::from_be_bytes(length) as usize];
                stream.read_exact(&mut body).unwrap();
                let request: fleet_proto::request::Request = serde_json::from_slice(&body).unwrap();
                let result = match request.body {
                    RequestBody::Hello { .. } => Some(ResponseBody::Hello {
                        protocol: fleet_proto::PROTOCOL_VERSION,
                        server: "test-daemon".into(),
                    }),
                    RequestBody::Subscribe { .. } => Some(ResponseBody::Ack),
                    body => respond(body),
                };
                if let Some(result) = result {
                    let bytes = serde_json::to_vec(&fleet_proto::response::Response {
                        id: request.id,
                        result: Ok(result),
                    })
                    .unwrap();
                    if stream
                        .write_all(&(bytes.len() as u32).to_be_bytes())
                        .is_err()
                        || stream.write_all(&bytes).is_err()
                    {
                        break;
                    }
                }
            }
        });
        Self {
            home,
            socket,
            thread: Some(thread),
        }
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        if let Ok(socket) = self.socket.recv_timeout(Duration::from_secs(5)) {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn empty_snapshot() -> Snapshot {
    Snapshot {
        generated_at: String::new(),
        contexts: vec![],
        repos: vec![],
        clones: vec![],
        worktrees: vec![],
        active_context: None,
        sessions: vec![],
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
        RequestBody::GetConfig => Some(ResponseBody::Config(fleet_core::config::default_config(
            "/tmp/fleet-test",
        ))),
        RequestBody::GetSnapshot => Some(ResponseBody::Snapshot(empty_snapshot())),
        RequestBody::TerminalInput { bytes, .. } => {
            input.try_send(bytes).unwrap();
            Some(ResponseBody::Ack)
        }
        _ => Some(ResponseBody::Ack),
    });
    let (commands, command_rx) = async_channel::unbounded();
    let (events, event_rx) = async_channel::unbounded();
    let home = daemon.home.path().to_owned();
    let task = tokio::spawn(async move { runtime::run(&home, &command_rx, &events).await });
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
    let home = daemon.home.path().to_owned();
    let task = tokio::spawn(async move { runtime::run(&home, &command_rx, &events).await });
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
