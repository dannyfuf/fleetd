//! End-to-end daemon binary protocol and lifecycle coverage.

use std::{
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

mod infra;

use infra::DaemonProcess;

use fleet_daemon::{
    adapters::files::{Files, RealFiles},
    server::SingletonGuard,
    stores::config::ConfigStore,
};
use fleet_proto::{
    PROTOCOL_VERSION,
    codec::FleetCodec,
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::UnixStream;
use tokio_util::codec::Framed;

#[tokio::test]
async fn server_binary_answers_snapshot_and_shutdown_and_cleans_up() {
    let _ = infra::RemoteDaemon::start;
    let _ = infra::assert_remote_contract;
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet-home");
    let mut daemon = DaemonProcess::start(&home);

    let socket = home.join("fleetd.sock");
    let mut incompatible = Framed::new(
        connect_until_ready(&socket).await,
        FleetCodec::<Request, serde_json::Value>::new(),
    );
    send(
        &mut incompatible,
        Request {
            id: 0,
            body: RequestBody::Hello {
                protocol: 6,
                client: "server-process-test".into(),
            },
        },
    )
    .await;
    let rejected = receive(&mut incompatible).await;
    let error = rejected.result.expect_err("protocol 6 must be rejected");
    assert!(error.message.contains("unsupported protocol 6"));
    assert!(error.message.contains("expected 7"));

    let stream = connect_until_ready(&socket).await;
    let mut client = Framed::new(stream, FleetCodec::<Request, serde_json::Value>::new());
    send(
        &mut client,
        Request {
            id: 1,
            body: RequestBody::Hello {
                protocol: PROTOCOL_VERSION,
                client: "server-process-test".into(),
            },
        },
    )
    .await;
    assert!(matches!(
        receive(&mut client).await.result,
        Ok(ResponseBody::Hello {
            protocol: PROTOCOL_VERSION,
            ..
        })
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

    daemon.wait().await;
    assert!(!socket.exists());
    assert!(!home.join("fleetd.pid").exists());
}

#[tokio::test]
async fn singleton_precedes_all_recovery_mutation() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let home = temp.path().join("fleet-home");
    let _owner = SingletonGuard::acquire(&home)
        .await
        .expect("acquire competing singleton");

    let output = Command::new(env!("CARGO_BIN_EXE_fleetd"))
        .arg("--home")
        .arg(&home)
        .output()
        .expect("run rejected fleetd");

    assert!(
        !output.status.success(),
        "second daemon unexpectedly started"
    );
    assert!(
        !home.join("logs").exists(),
        "rejected startup mutated daemon state before singleton acquisition"
    );
}

/// A daemon the bridge starts can lose the singleton race against another starter. Its failure
/// must reach `logs/fleetd.out`, the same startup log the remote restart script appends to.
#[tokio::test]
async fn bridge_records_a_losing_daemon_start() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let home = temp.path().join("fleet-home");
    let _owner = SingletonGuard::acquire(&home)
        .await
        .expect("acquire competing singleton");

    let mut bridge = Command::new(env!("CARGO_BIN_EXE_fleetd"))
        .arg("connect")
        .arg("--home")
        .arg(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the connect bridge");

    let log = home.join("logs").join("fleetd.out");
    let deadline = Instant::now() + Duration::from_secs(10);
    let recorded = loop {
        let contents = std::fs::read_to_string(&log).unwrap_or_default();
        if contents.contains("already running") {
            break contents;
        }
        assert!(
            Instant::now() < deadline,
            "bridge-started daemon left no trace in {}: {contents:?}",
            log.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let _ = bridge.kill();
    let _ = bridge.wait();

    assert!(
        recorded.contains(&home.join("fleetd.sock").display().to_string()),
        "startup log does not name the contended socket: {recorded:?}"
    );
}

/// `$RUST_LOG` selects the daemon's level, exactly as docs/DEVELOPMENT.md § Logs promises. With no
/// filter installed the subscriber caps itself at INFO and every `debug!` site in the daemon is
/// unreachable at runtime, so the log holds no DEBUG record whatever the environment says.
#[tokio::test]
async fn the_daemon_honours_rust_log_for_its_debug_records() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let home = temp.path().join("fleet-home");
    std::fs::create_dir_all(&home).expect("create fleet home");
    let files: Arc<dyn Files> = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    // A host whose command prefix cannot be spawned: the daemon builds its endpoint on the first
    // dispatched request, the connection fails at once, and the registry records it at DEBUG.
    ConfigStore::new(&home, files)
        .update(serde_json::json!({
            "hosts": {
                "absent": {
                    "provider": "command",
                    "run": ["/nonexistent/fleet-command-prefix"],
                    "fleetd": "fleetd"
                }
            }
        }))
        .await
        .expect("write the host configuration");

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_fleetd"))
        .arg("--home")
        .arg(&home)
        .env("RUST_LOG", "fleet_daemon=debug")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start fleetd under RUST_LOG");

    let socket = home.join("fleetd.sock");
    let mut client = Framed::new(
        connect_until_ready(&socket).await,
        FleetCodec::<Request, serde_json::Value>::new(),
    );
    send(
        &mut client,
        Request {
            id: 0,
            body: RequestBody::Hello {
                protocol: PROTOCOL_VERSION,
                client: "rust-log-test".into(),
            },
        },
    )
    .await;
    let _hello = receive(&mut client).await;
    send(
        &mut client,
        Request {
            id: 1,
            body: RequestBody::DaemonPing,
        },
    )
    .await;
    assert_eq!(receive(&mut client).await.result, Ok(ResponseBody::Pong));

    let log = home.join("logs").join("fleetd.log");
    let deadline = Instant::now() + Duration::from_secs(10);
    let recorded = loop {
        let contents = std::fs::read_to_string(&log).unwrap_or_default();
        if contents.contains("DEBUG") {
            break contents;
        }
        if Instant::now() >= deadline {
            let _ = daemon.kill();
            let _ = daemon.wait();
            panic!("fleetd logged no DEBUG record under RUST_LOG=fleet_daemon=debug: {contents:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let _ = daemon.kill();
    let _ = daemon.wait();

    assert!(
        recorded.contains("initial remote link connection failed"),
        "the DEBUG record is not the daemon's own: {recorded:?}"
    );
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
            Err(_) if Instant::now() < deadline => {
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
    tokio::time::timeout(Duration::from_secs(5), client.send(request))
        .await
        .expect("request write timed out")
        .unwrap_or_else(|error| panic!("failed to send request: {error}"));
}

async fn receive(
    client: &mut Framed<UnixStream, FleetCodec<Request, serde_json::Value>>,
) -> Response {
    let value = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("response timed out")
        .unwrap_or_else(|| panic!("daemon closed the connection"))
        .unwrap_or_else(|error| panic!("failed to receive response: {error}"));
    serde_json::from_value(value).unwrap_or_else(|error| panic!("invalid response: {error}"))
}
