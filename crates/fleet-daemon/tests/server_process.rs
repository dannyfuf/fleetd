//! End-to-end daemon binary protocol and lifecycle coverage.

use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use fleet_proto::{
    codec::FleetCodec,
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::UnixStream;
use tokio_util::codec::Framed;

#[tokio::test]
async fn server_binary_answers_snapshot_and_shutdown_and_cleans_up() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet-home");
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_fleetd"))
        .arg("--home")
        .arg(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|error| panic!("failed to start fleetd: {error}"));

    let socket = home.join("fleetd.sock");
    let stream = connect_until_ready(&socket).await;
    let mut client = Framed::new(stream, FleetCodec::<Request, serde_json::Value>::new());
    send(
        &mut client,
        Request {
            id: 1,
            body: RequestBody::Hello {
                protocol: 1,
                client: "server-process-test".to_owned(),
            },
        },
    )
    .await;
    assert!(matches!(
        receive(&mut client).await.result,
        Ok(ResponseBody::Hello { protocol: 1, .. })
    ));

    send(
        &mut client,
        Request {
            id: 2,
            body: RequestBody::GetSnapshot,
        },
    )
    .await;
    let snapshot = receive(&mut client).await;
    assert!(matches!(snapshot.result, Ok(ResponseBody::Snapshot(_))));

    send(
        &mut client,
        Request {
            id: 3,
            body: RequestBody::DaemonShutdown {
                stop_sessions: false,
            },
        },
    )
    .await;
    assert_eq!(
        receive(&mut client).await.result,
        Ok(ResponseBody::ShuttingDown)
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match daemon.try_wait() {
            Ok(Some(status)) => {
                assert!(status.success(), "fleetd exited with {status}");
                break;
            }
            Ok(None) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(None) => {
                let _ignored = daemon.kill();
                panic!("fleetd did not shut down within five seconds");
            }
            Err(error) => panic!("failed to wait for fleetd: {error}"),
        }
    }
    assert!(!socket.exists());
    assert!(!home.join("fleetd.pid").exists());
}

#[test]
fn server_binary_reports_its_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_fleetd"))
        .arg("--version")
        .output()
        .unwrap_or_else(|error| panic!("failed to run fleetd --version: {error}"));
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("fleetd {}", env!("CARGO_PKG_VERSION"))
    );
}

async fn connect_until_ready(socket: &std::path::Path) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match UnixStream::connect(socket).await {
            Ok(stream) => return stream,
            Err(error) if Instant::now() < deadline => {
                let _ignored = error;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("fleetd socket was not ready: {error}"),
        }
    }
}

async fn send(
    client: &mut Framed<UnixStream, FleetCodec<Request, serde_json::Value>>,
    request: Request,
) {
    client
        .send(request)
        .await
        .unwrap_or_else(|error| panic!("failed to send request: {error}"));
}

async fn receive(
    client: &mut Framed<UnixStream, FleetCodec<Request, serde_json::Value>>,
) -> Response {
    let value = client
        .next()
        .await
        .unwrap_or_else(|| panic!("daemon closed the connection"))
        .unwrap_or_else(|error| panic!("failed to receive response: {error}"));
    serde_json::from_value(value).unwrap_or_else(|error| panic!("invalid response: {error}"))
}
