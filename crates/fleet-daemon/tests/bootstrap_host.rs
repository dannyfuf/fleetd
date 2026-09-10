use std::{sync::Arc, time::Duration};

use fleet_core::ids::{HostId, JobId};
use fleet_daemon::{
    jobs::JobManager,
    machines::{ExecOutput, MachineProvider, RemoteEndpoint, RemoteHello},
    services::bootstrap::Bootstrap,
    testing::{FakeMachine, FakeRemote},
};
use fleet_proto::job::{JobRecord, JobStatus};
use fleet_proto::request::RequestBody;
use fleet_proto::snapshot::LinkState;

fn host() -> HostId {
    HostId::try_from("dev-box").unwrap_or_else(|error| panic!("{error}"))
}

fn success(stdout: &str) -> ExecOutput {
    ExecOutput {
        status: 0,
        stdout: stdout.to_owned(),
        stderr: String::new(),
    }
}

async fn finished(jobs: &JobManager, id: &JobId) -> JobRecord {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let record = jobs
                .record(id)
                .unwrap_or_else(|| panic!("missing job {id}"));
            if matches!(
                record.status,
                JobStatus::Succeeded | JobStatus::Failed { .. } | JobStatus::Cancelled
            ) {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|error| panic!("bootstrap did not finish: {error}"))
}

#[tokio::test]
async fn bootstrap_runs_the_exact_remote_build_install_restart_sequence() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let jobs = Arc::new(JobManager::new(temp.path()));
    let machine = Arc::new(FakeMachine::new(host()));
    machine.push_exec(Ok(success("git version 2.51\n")));
    machine.push_exec(Ok(success("cargo 1.97.1\n")));
    let remote = Arc::new(FakeRemote::new(host()));
    remote.set_hello(RemoteHello {
        version: "fleetd test".to_owned(),
        daemon_id: "remote-daemon".to_owned(),
        build_commit: Some("previous".to_owned()),
        capabilities: vec![fleet_proto::REMOTE_MACHINES_CAPABILITY.to_owned()],
    });
    for _ in 0..8 {
        remote.push_response(Ok(fleet_proto::response::ResponseBody::Pong));
    }
    let refreshed = Arc::clone(&remote);
    let refresh = tokio::spawn(async move {
        loop {
            if refreshed.requests().contains(&RequestBody::DaemonPing) {
                refreshed.set_hello(RemoteHello {
                    version: "fleetd test".to_owned(),
                    daemon_id: "remote-daemon".to_owned(),
                    build_commit: Some("abc123".to_owned()),
                    capabilities: vec![fleet_proto::REMOTE_MACHINES_CAPABILITY.to_owned()],
                });
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    let service = Bootstrap::with_machine_and_endpoint(
        Arc::clone(&jobs),
        Arc::clone(&machine) as Arc<dyn MachineProvider>,
        Arc::clone(&remote) as Arc<dyn RemoteEndpoint>,
        "https://github.com/acme/fleet.git",
        Some("abc123".to_owned()),
    );

    let id = service
        .start(host(), None)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let record = finished(&jobs, &id).await;
    tokio::time::timeout(Duration::from_secs(1), refresh)
        .await
        .expect("refresh observation timed out")
        .expect("refresh observer");
    assert_eq!(record.status, JobStatus::Succeeded);
    assert_eq!(record.target, "dev-box");
    assert_eq!(
        machine.exec_calls(),
        vec![
            vec!["git".to_owned(), "--version".to_owned()],
            vec!["cargo".to_owned(), "--version".to_owned()],
            vec![
                "sh".to_owned(),
                "-lc".to_owned(),
                "if [ -d \"$HOME\"/.fleet/src/fleet/.git ]; then git -C \"$HOME\"/.fleet/src/fleet fetch --prune origin; else mkdir -p \"$HOME\"/.fleet/src && git clone -- https://github.com/acme/fleet.git \"$HOME\"/.fleet/src/fleet; fi".to_owned(),
            ],
            vec![
                "sh".to_owned(),
                "-lc".to_owned(),
                "git -C \"$HOME\"/.fleet/src/fleet checkout --detach abc123".to_owned(),
            ],
            vec![
                "sh".to_owned(),
                "-lc".to_owned(),
                "cd \"$HOME\"/.fleet/src/fleet && FLEET_BUILD_COMMIT=abc123 cargo build --release -p fleet-daemon".to_owned(),
            ],
            vec![
                "sh".to_owned(),
                "-lc".to_owned(),
                "mkdir -p \"$(dirname -- \"$HOME/.local/bin\"/fleetd)\" && install -m 755 \"$HOME\"/.fleet/src/fleet/target/release/fleetd \"$HOME/.local/bin\"/fleetd".to_owned(),
            ],
            vec![
                "sh".to_owned(),
                "-lc".to_owned(),
                "fleetd_pid=''; kill_pid=''; if [ -s \"$HOME\"/.fleet/fleetd.pid ]; then fleetd_pid=\"$(cat \"$HOME\"/.fleet/fleetd.pid)\"; kill_pid=\"$fleetd_pid\"; case \"$(ps -p \"$kill_pid\" -o comm= 2>/dev/null)\" in *fleetd*) ;; *) kill_pid='' ;; esac; fi; if [ -n \"$kill_pid\" ]; then kill -TERM \"$kill_pid\" 2>/dev/null || true; i=0; while [ $i -lt 50 ] && kill -0 \"$kill_pid\" 2>/dev/null; do i=$((i + 1)); sleep 0.1; done; if kill -0 \"$kill_pid\" 2>/dev/null; then kill -KILL \"$kill_pid\" 2>/dev/null || true; i=0; while [ $i -lt 20 ] && kill -0 \"$kill_pid\" 2>/dev/null; do i=$((i + 1)); sleep 0.1; done; fi; fi; mkdir -p \"$HOME\"/.fleet/logs; nohup fleetd --home \"$HOME\"/.fleet >>\"$HOME\"/.fleet/logs/fleetd.out 2>&1 </dev/null & i=0; while [ $i -lt 100 ]; do if [ -s \"$HOME\"/.fleet/fleetd.pid ]; then started_pid=\"$(cat \"$HOME\"/.fleet/fleetd.pid)\"; if [ -n \"$started_pid\" ] && [ \"$started_pid\" != \"$fleetd_pid\" ] && kill -0 \"$started_pid\" 2>/dev/null; then exit 0; fi; fi; i=$((i + 1)); sleep 0.1; done; echo 'fleetd failed to start' >&2; tail -n 40 \"$HOME\"/.fleet/logs/fleetd.out >&2 || true; exit 1".to_owned(),
            ],
        ]
    );
    let log = jobs
        .tail(&id, 100)
        .await
        .unwrap_or_else(|error| panic!("{error}"))
        .join("\n");
    assert!(log.contains("git version 2.51"));
    assert!(log.contains("cargo 1.97.1"));
    assert!(log.contains("host dev-box is ready"));
    let requests = remote.requests();
    assert!(!requests.is_empty(), "the probe must refresh a stale Hello");
    assert!(
        requests
            .iter()
            .all(|request| request == &RequestBody::DaemonPing),
        "unexpected probe requests: {requests:?}"
    );
}

#[tokio::test]
async fn bootstrap_nudges_a_reconnecting_link_instead_of_pinging_it() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let jobs = Arc::new(JobManager::new(temp.path()));
    let machine = Arc::new(FakeMachine::new(host()));
    machine.push_exec(Ok(success("git version 2.51\n")));
    machine.push_exec(Ok(success("cargo 1.97.1\n")));
    let remote = Arc::new(FakeRemote::new(host()));
    // The restarted host is still reconnecting, so the endpoint has no Hello and cannot be pinged.
    remote.set_state(LinkState::Down);
    let reconnecting = Arc::clone(&remote);
    let observer = tokio::spawn(async move {
        loop {
            if reconnecting.nudges() > 0 {
                reconnecting.set_hello(RemoteHello {
                    version: "fleetd test".to_owned(),
                    daemon_id: "remote-daemon".to_owned(),
                    build_commit: Some("abc123".to_owned()),
                    capabilities: vec![fleet_proto::REMOTE_MACHINES_CAPABILITY.to_owned()],
                });
                reconnecting.set_state(LinkState::Ready);
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    let service = Bootstrap::with_machine_and_endpoint(
        Arc::clone(&jobs),
        Arc::clone(&machine) as Arc<dyn MachineProvider>,
        Arc::clone(&remote) as Arc<dyn RemoteEndpoint>,
        "https://github.com/acme/fleet.git",
        Some("abc123".to_owned()),
    );

    let id = service
        .start(host(), None)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let record = finished(&jobs, &id).await;
    tokio::time::timeout(Duration::from_secs(1), observer)
        .await
        .expect("nudge observation timed out")
        .expect("nudge observer");

    assert_eq!(record.status, JobStatus::Succeeded);
    assert_eq!(remote.nudges(), 1, "the restart must nudge the link once");
    assert!(
        remote.requests().is_empty(),
        "a link that is not Ready must not be pinged: {:?}",
        remote.requests()
    );
}

#[tokio::test]
async fn bootstrap_requires_an_explicit_ref_without_a_build_commit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let jobs = Arc::new(JobManager::new(temp.path()));
    let machine = Arc::new(FakeMachine::new(host()));
    let service = Bootstrap::with_machine(
        jobs,
        machine as Arc<dyn MachineProvider>,
        "https://github.com/acme/fleet.git",
        None,
    );

    let error = service
        .start(host(), None)
        .await
        .expect_err("missing revision must be rejected");
    assert!(error.to_string().contains("requires --ref"));
}

#[tokio::test]
async fn bootstrap_fails_clearly_when_remote_cargo_is_missing() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let jobs = Arc::new(JobManager::new(temp.path()));
    let machine = Arc::new(FakeMachine::new(host()));
    machine.push_exec(Ok(success("git version 2.51\n")));
    machine.push_exec(Ok(ExecOutput {
        status: 127,
        stdout: String::new(),
        stderr: "cargo: command not found".to_owned(),
    }));
    let service = Bootstrap::with_machine(
        Arc::clone(&jobs),
        Arc::clone(&machine) as Arc<dyn MachineProvider>,
        "https://github.com/acme/fleet.git",
        None,
    );

    let id = service
        .start(host(), Some("release".to_owned()))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let record = finished(&jobs, &id).await;
    let JobStatus::Failed { error } = record.status else {
        panic!("expected failed job, got {:?}", record.status);
    };
    assert!(error.contains("remote host `dev-box` is missing required tool `cargo`"));
    assert!(error.contains("cargo: command not found"));
    assert_eq!(machine.exec_calls().len(), 2);
}
