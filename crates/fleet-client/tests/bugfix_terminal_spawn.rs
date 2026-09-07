use std::{path::Path, time::Duration};

use fleet_client::Client;
use fleet_core::ids::TerminalId;
use fleet_proto::{
    PROTOCOL_VERSION,
    codec::FleetCodec,
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt};
use tempfile::TempDir;
use tokio::{net::UnixListener, sync::oneshot};
use tokio_util::codec::Framed;

type ServerTransport = Framed<tokio::net::UnixStream, FleetCodec<serde_json::Value, Request>>;

#[tokio::test]
async fn cancelled_attach_cleans_membership() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let (attach_seen_tx, attach_seen_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        let attach = next_request(&mut transport).await;
        assert!(matches!(
            attach.body,
            RequestBody::AttachTerminal {
                terminal: TerminalId(17),
                ..
            }
        ));
        attach_seen_tx.send(()).unwrap();
        let detach = next_request(&mut transport).await;
        assert_eq!(
            detach.body,
            RequestBody::DetachTerminal {
                terminal: TerminalId(17)
            }
        );
    });

    let client = Client::connect(home.path()).await.unwrap();
    let attaching = tokio::spawn(async move { client.attach(TerminalId(17), 80, 24).await });
    attach_seen_rx.await.unwrap();
    attaching.abort();
    assert!(attaching.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("cancelled attach did not clean up membership")
        .unwrap();
}

#[tokio::test]
async fn one_clone_does_not_detach() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport).await;
        for _ in 0..2 {
            let attach = next_request(&mut transport).await;
            assert!(matches!(
                attach.body,
                RequestBody::AttachTerminal {
                    terminal: TerminalId(23),
                    ..
                }
            ));
            send_response(&mut transport, attach.id, ResponseBody::Ack).await;
        }

        let ping = next_request(&mut transport).await;
        assert_eq!(ping.body, RequestBody::DaemonPing);
        send_response(&mut transport, ping.id, ResponseBody::Pong).await;

        let detach = next_request(&mut transport).await;
        assert_eq!(
            detach.body,
            RequestBody::DetachTerminal {
                terminal: TerminalId(23)
            }
        );
    });

    let client = Client::connect(home.path()).await.unwrap();
    let first = client.attach(TerminalId(23), 80, 24).await.unwrap();
    let second = client.attach(TerminalId(23), 80, 24).await.unwrap();
    drop(first);
    client.daemon_ping().await.unwrap();
    drop(second);
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("final terminal lease did not detach")
        .unwrap();
}

async fn bind(home: &Path) -> UnixListener {
    UnixListener::bind(home.join("fleetd.sock")).unwrap()
}

async fn authenticate(transport: &mut ServerTransport) {
    let hello = next_request(transport).await;
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
        ResponseBody::Hello {
            protocol: PROTOCOL_VERSION,
            server: "test-daemon".to_owned(),
        },
    )
    .await;
    let subscribe = next_request(transport).await;
    assert!(matches!(subscribe.body, RequestBody::Subscribe { .. }));
    send_response(transport, subscribe.id, ResponseBody::Ack).await;
}

async fn next_request(transport: &mut ServerTransport) -> Request {
    transport.next().await.expect("connection closed").unwrap()
}

async fn send_response(transport: &mut ServerTransport, id: u64, body: ResponseBody) {
    transport
        .send(
            serde_json::to_value(Response {
                id,
                result: Ok(body),
            })
            .unwrap(),
        )
        .await
        .unwrap();
}
