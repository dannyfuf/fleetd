//! Server-wide event coalescing and single-instance guard coverage.

use std::{sync::Arc, time::Duration};

use fleet_daemon::{
    DaemonError,
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    jobs::JobManager,
    server::{BroadcastBus, Listener},
    services::Services,
    stores::{config::ConfigStore, state::StateStore},
};
use fleet_proto::event::Event;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn server_snapshot_requests_are_coalesced() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet");
    let services = services(&home);
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();

    events.request_snapshot(Arc::clone(&services));
    events.request_snapshot(Arc::clone(&services));
    events.request_snapshot(services);

    let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .unwrap_or_else(|_| panic!("snapshot event timed out"))
        .unwrap_or_else(|error| panic!("snapshot bus closed: {error}"));
    assert!(matches!(event, Event::SnapshotChanged(_)));
    assert!(
        tokio::time::timeout(Duration::from_millis(80), receiver.recv())
            .await
            .is_err(),
        "burst emitted more than one snapshot"
    );
}

#[tokio::test]
async fn snapshot_requests_from_terminal_threads_use_the_daemon_runtime() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet");
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    let _services = services_with_events(&home, events.clone());

    std::thread::spawn(move || events.request_snapshot_current())
        .join()
        .unwrap_or_else(|_| panic!("snapshot request thread panicked"));

    let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .unwrap_or_else(|_| panic!("snapshot event timed out"))
        .unwrap_or_else(|error| panic!("snapshot bus closed: {error}"));
    assert!(matches!(event, Event::SnapshotChanged(_)));
}

#[tokio::test]
async fn server_pid_guard_rejects_a_second_instance_even_without_socket_path() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet");
    let services = services(&home);
    let first = Listener::bind(
        &home,
        Arc::clone(&services),
        BroadcastBus::default(),
        CancellationToken::new(),
    )
    .await
    .unwrap_or_else(|error| panic!("{error}"));
    std::fs::remove_file(first.socket_path()).unwrap_or_else(|error| panic!("{error}"));

    let second = Listener::bind(
        &home,
        services,
        BroadcastBus::default(),
        CancellationToken::new(),
    )
    .await;
    assert!(matches!(second, Err(DaemonError::Conflict(_))));
}

fn services(home: &std::path::Path) -> Arc<Services> {
    let (config, state, jobs, adapters) = service_parts(home);
    Arc::new(Services::new(home, config, state, jobs, adapters))
}

fn services_with_events(home: &std::path::Path, events: BroadcastBus) -> Arc<Services> {
    let (config, state, jobs, adapters) = service_parts(home);
    Services::new_with_events(home, config, state, jobs, adapters, events)
}

fn service_parts(
    home: &std::path::Path,
) -> (Arc<ConfigStore>, Arc<StateStore>, Arc<JobManager>, Adapters) {
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    let config = Arc::new(ConfigStore::new(home, files.clone()));
    let state = Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock)));
    let jobs = Arc::new(JobManager::new(home));
    let adapters = Adapters::system(files);
    (config, state, jobs, adapters)
}
