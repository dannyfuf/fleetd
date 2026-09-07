use std::{io::ErrorKind as IoErrorKind, path::Path, time::Duration};

use fleet_client::{Client, ConnectError};
use fleet_core::ids::TerminalId;
use fleet_proto::{
    PROTOCOL_VERSION,
    codec::FleetCodec,
    error::{ErrorKind, ProtoError},
    event::{Event, EventKind, ToastLevel},
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt};
use tempfile::TempDir;
use tokio::{
    net::{UnixListener, unix::OwnedReadHalf},
    sync::oneshot,
    task::JoinSet,
    time::timeout,
};
use tokio_util::codec::{Framed, FramedWrite};

type ServerTransport = Framed<tokio::net::UnixStream, FleetCodec<serde_json::Value, Request>>;

#[tokio::test]
async fn blocked_write_does_not_block_response() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let (ping_seen, ping_received) = oneshot::channel();
    let (release_server, server_release) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport: ServerTransport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let ping = transport.next().await.unwrap().unwrap();
        assert!(matches!(ping.body, RequestBody::DaemonPing));
        ping_seen.send(()).unwrap();

        let parts = transport.into_parts();
        assert!(parts.read_buf.is_empty());
        let (reader, writer) = parts.io.into_split();
        let _prefix = read_exact(&reader, 4).await;
        let mut responses =
            FramedWrite::new(writer, FleetCodec::<serde_json::Value, Request>::new());
        send_response(&mut responses, ping.id, Ok(ResponseBody::Pong)).await;
        server_release.await.unwrap();
    });

    let client = Client::connect(home.path()).await.unwrap();
    let ping_client = client.clone();
    let ping = tokio::spawn(async move { ping_client.daemon_ping().await });
    ping_received.await.unwrap();
    client
        .request_background(RequestBody::PasteTerminal {
            terminal: TerminalId(7),
            text: "x".repeat(8 * 1024 * 1024),
        })
        .await
        .unwrap();

    timeout(Duration::from_secs(2), ping)
        .await
        .expect("response progresses while the write is blocked")
        .unwrap()
        .unwrap();
    release_server.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn background_failure_is_observable() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let request = transport.next().await.unwrap().unwrap();
        send_response(
            &mut transport,
            request.id,
            Err(ProtoError {
                kind: ErrorKind::Tmux,
                message: "terminal rejected input".to_owned(),
            }),
        )
        .await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    let mut events = client.events();
    client
        .request_background(RequestBody::TerminalInput {
            terminal: TerminalId(9),
            bytes: b"input".to_vec(),
        })
        .await
        .unwrap();
    assert_eq!(
        timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap(),
        Event::Toast {
            level: ToastLevel::Error,
            message: "terminal rejected input".to_owned(),
        }
    );
    server.await.unwrap();
}

#[tokio::test]
async fn background_not_found_is_not_observable() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let marker = Event::Toast {
        level: ToastLevel::Info,
        message: "response processed".to_owned(),
    };
    let expected_marker = marker.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let request = transport.next().await.unwrap().unwrap();
        send_response(
            &mut transport,
            request.id,
            Err(ProtoError {
                kind: ErrorKind::NotFound,
                message: "not found: 12".to_owned(),
            }),
        )
        .await;
        send_event(&mut transport, marker).await.unwrap();
    });

    let client = Client::connect(home.path()).await.unwrap();
    let mut events = client.events();
    client
        .request_background(RequestBody::RequestFullFrame {
            terminal: TerminalId(12),
        })
        .await
        .unwrap();

    assert_eq!(
        timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap(),
        expected_marker
    );
    server.await.unwrap();
}

#[tokio::test]
async fn reconnect_restores_remaining_attachments() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let (disconnected, connection_dropped) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut first = Framed::new(socket, FleetCodec::new());
        authenticate(&mut first).await;
        for _ in 0..2 {
            let attach = first.next().await.unwrap().unwrap();
            assert!(matches!(attach.body, RequestBody::AttachTerminal { .. }));
            send_response(&mut first, attach.id, Ok(ResponseBody::Ack)).await;
        }
        drop(first);
        disconnected.send(()).unwrap();

        let (socket, _) = listener.accept().await.unwrap();
        let mut second = Framed::new(socket, FleetCodec::new());
        authenticate(&mut second).await;
        for _ in 0..2 {
            let attach = second.next().await.unwrap().unwrap();
            let RequestBody::AttachTerminal { terminal, .. } = attach.body else {
                panic!("expected replayed attachment");
            };
            let result = if terminal == TerminalId(11) {
                Err(ProtoError {
                    kind: ErrorKind::NotFound,
                    message: "terminal vanished".to_owned(),
                })
            } else {
                Ok(ResponseBody::Ack)
            };
            send_response(&mut second, attach.id, result).await;
        }
        let ping = second.next().await.unwrap().unwrap();
        assert!(matches!(ping.body, RequestBody::DaemonPing));
        send_response(&mut second, ping.id, Ok(ResponseBody::Pong)).await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    client
        .attach_terminal(TerminalId(11), 80, 24)
        .await
        .unwrap();
    client
        .attach_terminal(TerminalId(12), 100, 30)
        .await
        .unwrap();
    connection_dropped.await.unwrap();
    timeout(Duration::from_secs(3), async {
        loop {
            if client.daemon_ping().await.is_ok() {
                break;
            }
        }
    })
    .await
    .expect("client reconnects after discarding the vanished attachment");
    server.await.unwrap();
}

#[tokio::test]
async fn restarted_daemon_does_not_receive_replayed_attachment() {
    let home = TempDir::new().unwrap();
    std::fs::write(home.path().join("fleetd.pid"), "41001\n").unwrap();
    let listener = bind(home.path()).await;
    let pid_path = home.path().join("fleetd.pid");
    let (restarted, daemon_restarted) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut first = Framed::new(socket, FleetCodec::new());
        authenticate(&mut first).await;
        let attach = first.next().await.unwrap().unwrap();
        assert!(matches!(
            attach.body,
            RequestBody::AttachTerminal {
                terminal: TerminalId(21),
                ..
            }
        ));
        send_response(&mut first, attach.id, Ok(ResponseBody::Ack)).await;
        std::fs::write(pid_path, "41002\n").unwrap();
        drop(first);
        restarted.send(()).unwrap();

        let (socket, _) = listener.accept().await.unwrap();
        let mut second = Framed::new(socket, FleetCodec::new());
        authenticate(&mut second).await;
        let request = second.next().await.unwrap().unwrap();
        let replayed = matches!(
            &request.body,
            RequestBody::AttachTerminal {
                terminal: TerminalId(21),
                ..
            }
        );
        if replayed {
            send_response(&mut second, request.id, Ok(ResponseBody::Ack)).await;
            let ping = second.next().await.unwrap().unwrap();
            assert!(matches!(ping.body, RequestBody::DaemonPing));
            send_response(&mut second, ping.id, Ok(ResponseBody::Pong)).await;
        } else {
            assert!(matches!(&request.body, RequestBody::DaemonPing));
            send_response(&mut second, request.id, Ok(ResponseBody::Pong)).await;
        }
        replayed
    });

    let client = Client::connect(home.path()).await.unwrap();
    client
        .attach_terminal(TerminalId(21), 80, 24)
        .await
        .unwrap();
    daemon_restarted.await.unwrap();
    timeout(Duration::from_secs(3), async {
        loop {
            if client.daemon_ping().await.is_ok() {
                break;
            }
        }
    })
    .await
    .expect("client reconnects without attaching to a reused terminal id");

    assert!(!server.await.unwrap());
}

#[tokio::test]
async fn disconnected_queue_is_bounded() {
    let home = TempDir::new().unwrap();
    let socket_path = home.path().join("fleetd.sock");
    let listener = bind(home.path()).await;
    let (closed, server_closed) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        drop(transport);
        drop(listener);
        std::fs::remove_file(socket_path).unwrap();
        closed.send(()).unwrap();
    });

    let client = Client::connect(home.path()).await.unwrap();
    server_closed.await.unwrap();

    let mut requests = JoinSet::new();
    for _ in 0..700 {
        let client = client.clone();
        requests.spawn(async move { client.request_background(RequestBody::DaemonPing).await });
    }
    assert!(
        timeout(Duration::from_secs(1), async {
            while requests.join_next().await.is_some() {}
        })
        .await
        .is_err(),
        "a disconnected actor must apply backpressure instead of retaining every request"
    );
    requests.abort_all();
    server.await.unwrap();
}

#[tokio::test]
async fn event_flood_cannot_extend_handshake() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport: ServerTransport = Framed::new(socket, FleetCodec::new());
        let hello = transport.next().await.unwrap().unwrap();
        assert!(matches!(hello.body, RequestBody::Hello { .. }));
        for index in 0..=1024 {
            let event = Event::Toast {
                level: ToastLevel::Info,
                message: format!("handshake event {index}"),
            };
            if send_event(&mut transport, event).await.is_err() {
                break;
            }
        }
    });

    let result = timeout(Duration::from_secs(2), Client::connect(home.path()))
        .await
        .expect("event buffering is bounded");
    assert!(matches!(result, Err(ConnectError::InvalidHandshake(_))));
    server.await.unwrap();
}

async fn bind(home: &Path) -> UnixListener {
    UnixListener::bind(home.join("fleetd.sock")).unwrap()
}

async fn authenticate(transport: &mut ServerTransport) {
    let hello = transport.next().await.unwrap().unwrap();
    assert!(matches!(
        hello.body,
        RequestBody::Hello {
            protocol: PROTOCOL_VERSION,
            ..
        }
    ));
    send_response(
        transport,
        hello.id,
        Ok(ResponseBody::Hello {
            protocol: PROTOCOL_VERSION,
            server: "test-daemon".to_owned(),
        }),
    )
    .await;
    let subscribe = transport.next().await.unwrap().unwrap();
    let RequestBody::Subscribe { events } = subscribe.body else {
        panic!("expected initial subscription");
    };
    assert!(events.contains(&EventKind::Toast));
    send_response(transport, subscribe.id, Ok(ResponseBody::Ack)).await;
}

async fn send_response<S>(transport: &mut S, id: u64, result: Result<ResponseBody, ProtoError>)
where
    S: futures_util::Sink<serde_json::Value> + Unpin,
    S::Error: std::fmt::Debug,
{
    transport
        .send(serde_json::to_value(Response { id, result }).unwrap())
        .await
        .unwrap();
}

async fn send_event(transport: &mut ServerTransport, event: Event) -> Result<(), ()> {
    transport
        .send(serde_json::to_value(event).unwrap())
        .await
        .map_err(|_| ())
}

async fn read_exact(reader: &OwnedReadHalf, length: usize) -> Vec<u8> {
    let mut bytes = vec![0; length];
    let mut offset = 0;
    while offset < length {
        reader.readable().await.unwrap();
        match reader.try_read(&mut bytes[offset..]) {
            Ok(0) => panic!("socket closed before blocked write began"),
            Ok(read) => offset += read,
            Err(error) if error.kind() == IoErrorKind::WouldBlock => {}
            Err(error) => panic!("socket read failed: {error}"),
        }
    }
    bytes
}
