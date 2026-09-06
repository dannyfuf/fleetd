//! Server-wide event coalescing and single-instance guard coverage.

use std::{sync::Arc, time::Duration};

use fleet_core::{
    config::WindowConfig,
    ids::{ContextId, RepoId, SessionId, TerminalId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    sessions::AgentActivity,
    state::default_state,
};
use fleet_daemon::{
    DaemonError,
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    jobs::JobManager,
    server::{BroadcastBus, Listener},
    services::Services,
    stores::{config::ConfigStore, state::StateStore},
};
use fleet_proto::{event::Event, request::RequestBody, response::ResponseBody};
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
async fn explicit_agent_activity_updates_status_and_emits_one_transition_and_snapshot() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("fleet");
    let events = BroadcastBus::default();
    let (services, session_id, terminal_id) = services_with_session(&home, events.clone()).await;
    let mut receiver = events.subscribe();

    let response = services
        .dispatch(RequestBody::SetAgentActivity {
            session: session_id.clone(),
            terminal_id,
            activity: AgentActivity::Idle,
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(response, ResponseBody::Ack);

    let transition = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .unwrap_or_else(|_| panic!("agent activity event timed out"))
        .unwrap_or_else(|error| panic!("event bus closed: {error}"));
    assert!(matches!(
        transition,
        Event::AgentActivityChanged {
            session: ref event_session,
            terminal_id: event_terminal,
            activity: AgentActivity::Idle,
            ..
        } if event_session == &session_id && event_terminal == terminal_id
    ));

    let snapshot = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
        .await
        .unwrap_or_else(|_| panic!("snapshot event timed out"))
        .unwrap_or_else(|error| panic!("event bus closed: {error}"));
    let Event::SnapshotChanged(snapshot) = snapshot else {
        panic!("expected snapshot after activity transition");
    };
    assert_eq!(snapshot.statuses[0].agent_activity, AgentActivity::Idle);
    assert_eq!(
        snapshot.statuses[0].windows[0].agent_activity,
        AgentActivity::Idle
    );

    services
        .dispatch(RequestBody::SetAgentActivity {
            session: session_id.clone(),
            terminal_id,
            activity: AgentActivity::Idle,
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), receiver.recv())
            .await
            .is_err(),
        "unchanged explicit activity emitted another event"
    );
    services
        .sessions
        .kill(session_id)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
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

async fn services_with_session(
    home: &std::path::Path,
    events: BroadcastBus,
) -> (Arc<Services>, SessionId, TerminalId) {
    let (config, state, jobs, adapters) = service_parts(home);
    let worktree_path = home.join("worktrees/repo/feature");
    std::fs::create_dir_all(&worktree_path).unwrap_or_else(|error| panic!("{error}"));
    let mut effective = config
        .load()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    effective.windows = vec![WindowConfig {
        name: "agent".to_owned(),
        command: "/bin/sh -c 'sleep 30'".to_owned(),
    }];
    config
        .save(effective)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let context_id = ContextId::try_from("team").unwrap_or_else(|error| panic!("{error}"));
    let repo_id = RepoId::try_from("owner/repo").unwrap_or_else(|error| panic!("{error}"));
    let worktree_id =
        WorktreeId::try_from("owner/repo#feature").unwrap_or_else(|error| panic!("{error}"));
    let mut persisted = default_state();
    persisted.contexts.push(Context {
        id: context_id.clone(),
        name: "Team".to_owned(),
        owners: vec!["owner".to_owned()],
        created_at: "2026-09-05T00:00:00Z".to_owned(),
    });
    persisted.repos.push(Repo {
        id: repo_id.clone(),
        owner: "owner".to_owned(),
        name: "repo".to_owned(),
        url: "https://example.invalid/owner/repo".to_owned(),
        context_id,
        default_branch: "main".to_owned(),
        path: home.join("repos/owner/repo").to_string_lossy().into_owned(),
        cloned_at: "2026-09-05T00:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    });
    persisted.worktrees.push(Worktree {
        id: worktree_id.clone(),
        repo_id,
        slug: "feature".to_owned(),
        branch: "feature".to_owned(),
        base_ref: "main".to_owned(),
        path: worktree_path.to_string_lossy().into_owned(),
        session: "repo/feature".to_owned(),
        host: None,
        created_at: "2026-09-05T00:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    });
    state
        .save(persisted)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let services = Services::new_with_events(home, config, state, jobs, adapters, events);
    let session = services
        .sessions
        .ensure(Some(worktree_id), None, false)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    (services, session.id, session.terminals[0].id)
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
