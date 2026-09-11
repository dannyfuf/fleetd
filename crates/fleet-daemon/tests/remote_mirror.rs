use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use fleet_core::{
    ids::HostId,
    model::{Context, HostConfigEntry, Repo, RepoHooks, Worktree},
    sessions::{AgentActivity, SessionState, WorktreeStatus},
    state::default_state,
};
use fleet_daemon::{
    DaemonError, DaemonResult,
    adapters::{Adapters, clock::SystemClock, files::RealFiles},
    jobs::JobManager,
    machines::{MachineProvider, RemoteEndpoint, RemoteHello},
    server::BroadcastBus,
    services::Services,
    stores::{config::ConfigStore, state::StateStore},
    testing::FakeRemote,
};
use fleet_proto::{
    PROTOCOL_VERSION,
    codec::FleetCodec,
    event::Event,
    request::{ClientKind, HelloClient, Request, RequestBody},
    response::{Response, ResponseBody},
    snapshot::{DaemonInfo, LinkState, Snapshot},
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{broadcast, watch};
use tokio_util::codec::Framed;

mod infra;

#[tokio::test]
async fn real_remote_daemon_inventory_is_merged_with_host_ownership() {
    let _ = infra::DaemonProcess::wait;
    let _ = infra::assert_remote_contract;
    let remote_home = tempfile::tempdir().expect("remote home");
    write_published_state(remote_home.path());
    let infra::RemoteDaemon {
        daemon: _daemon,
        machine,
        config: remote_config,
    } = infra::RemoteDaemon::start(remote_home.path());
    let host = HostId::try_from("loopback").expect("host");
    let endpoint: Arc<dyn RemoteEndpoint> = Arc::new(CommandEndpoint::new(
        Arc::new(machine),
        remote_home.path().join("fleetd.sock"),
    ));
    let (services, config) = local_services(&host, remote_config).await;
    services.machines.install_endpoint(host.clone(), endpoint);
    let mut changed = services.events.subscribe();

    let initial = services.snapshot().await.expect("initial snapshot");
    assert!(initial.worktrees.is_empty());
    wait_for_fragment(&services, &host, |fragment| !fragment.stale).await;
    let merged = services.snapshot().await.expect("merged snapshot");
    let broadcast =
        wait_for_snapshot(&mut changed, |snapshot| !snapshot.worktrees.is_empty()).await;
    assert_eq!(broadcast.worktrees, merged.worktrees);

    let worktree = &merged.worktrees[0];
    assert_eq!(worktree.id.as_str(), "acme/api#remote");
    assert_eq!(worktree.host.as_ref(), Some(&host));
    assert_eq!(worktree.session, "loopback/api/remote");
    assert_eq!(merged.statuses[0].session, SessionState::None);
    assert_eq!(
        services.router.ids.host_of_worktree(&worktree.id),
        Some(host.clone())
    );

    let mut without_host = config.load().await.expect("load local config");
    without_host.hosts.clear();
    config.save(without_host).await.expect("remove host");
    let without_remote = services.snapshot().await.expect("snapshot after removal");
    assert!(without_remote.worktrees.is_empty());
    assert!(services.mirror.fragment(&host).is_none());
}

#[tokio::test]
async fn fake_remote_updates_live_and_becomes_unknown_while_down() {
    let host = HostId::try_from("dev-box").expect("host");
    let (services, _config) = local_services(
        &host,
        HostConfigEntry::Command {
            run: vec!["false".into()],
            fleetd: "fleetd".into(),
            fleet_home: None,
            display: Some("fake".into()),
        },
    )
    .await;
    let remote = Arc::new(FakeRemote::new(host.clone()));
    let mut stale_persisted = published_state(Some(host.clone()));
    stale_persisted.worktrees[0].path = "/stale-import/acme/api/remote".into();
    services
        .state
        .save(stale_persisted)
        .await
        .expect("save stale imported record");
    remote.push_response(Ok(ResponseBody::Ack));
    remote.push_response(Ok(ResponseBody::Snapshot(remote_snapshot(
        SessionState::Detached,
    ))));
    services
        .machines
        .install_endpoint(host.clone(), remote.clone());
    let mut changed = services.events.subscribe();

    services.snapshot().await.expect("start observer");
    wait_for_fragment(&services, &host, |fragment| !fragment.stale).await;
    let ready = services.snapshot().await.expect("ready snapshot");
    assert_eq!(ready.statuses[0].session, SessionState::Detached);
    let ready_broadcast = wait_for_snapshot(&mut changed, |snapshot| {
        snapshot
            .statuses
            .first()
            .is_some_and(|status| status.session == SessionState::Detached)
    })
    .await;
    assert_eq!(ready_broadcast.worktrees, ready.worktrees);
    assert_eq!(ready.worktrees[0].host.as_ref(), Some(&host));
    assert_eq!(ready.worktrees[0].path, "/remote/acme/api/remote");
    assert_eq!(
        remote.requests(),
        vec![
            RequestBody::Subscribe {
                events: vec![
                    fleet_proto::event::EventKind::SnapshotChanged,
                    fleet_proto::event::EventKind::SessionChanged,
                    fleet_proto::event::EventKind::AgentSummary,
                ],
            },
            RequestBody::GetSnapshot,
        ]
    );

    remote.emit(Event::SnapshotChanged(remote_snapshot(
        SessionState::Attached,
    )));
    let updated = wait_for_snapshot(&mut changed, |snapshot| {
        snapshot
            .statuses
            .first()
            .is_some_and(|status| status.session == SessionState::Attached)
    })
    .await;
    assert_eq!(updated.statuses[0].session, SessionState::Attached);

    remote.set_state(LinkState::Down);
    let stale = wait_for_snapshot(&mut changed, |snapshot| {
        snapshot
            .statuses
            .first()
            .is_some_and(|status| status.session == SessionState::Unknown)
    })
    .await;
    assert_eq!(stale.worktrees[0].host.as_ref(), Some(&host));
    assert!(services.mirror.fragment(&host).expect("fragment").stale);
}

#[tokio::test]
async fn a_local_record_wins_a_global_id_collision_with_a_remote_fragment() {
    let host = HostId::try_from("dev-box").expect("host");
    let (services, _config) = local_services(
        &host,
        HostConfigEntry::Command {
            run: vec!["false".into()],
            fleetd: "fleetd".into(),
            fleet_home: None,
            display: Some("fake".into()),
        },
    )
    .await;
    let mut local = published_state(None);
    local.worktrees[0].path = "/local/acme/api/remote".into();
    services.state.save(local).await.expect("save local record");
    services
        .mirror
        .apply(&host, remote_snapshot(SessionState::Detached));

    let snapshot = services.snapshot().await.expect("merged snapshot");

    assert_eq!(snapshot.worktrees.len(), 1);
    assert_eq!(snapshot.worktrees[0].path, "/local/acme/api/remote");
    assert!(snapshot.worktrees[0].host.is_none());
    assert_eq!(
        services
            .router
            .ids
            .host_of_worktree(&snapshot.worktrees[0].id),
        None
    );
}

struct CommandEndpoint {
    provider: Arc<dyn MachineProvider>,
    socket: PathBuf,
    events: broadcast::Sender<Event>,
    state: watch::Sender<LinkState>,
    requests: Mutex<u64>,
}

impl CommandEndpoint {
    fn new(provider: Arc<dyn MachineProvider>, socket: PathBuf) -> Self {
        let (events, _) = broadcast::channel(16);
        let (state, _) = watch::channel(LinkState::Ready);
        Self {
            provider,
            socket,
            events,
            state,
            requests: Mutex::new(0),
        }
    }

    async fn request_once(&self, body: RequestBody) -> DaemonResult<ResponseBody> {
        let stream = tokio::net::UnixStream::connect(&self.socket)
            .await
            .map_err(|error| DaemonError::fs(&self.socket, error))?;
        let mut framed = Framed::new(stream, FleetCodec::<Request, serde_json::Value>::new());
        framed
            .send(Request {
                id: 1,
                body: RequestBody::Hello {
                    protocol: PROTOCOL_VERSION,
                    client: HelloClient {
                        kind: ClientKind::Proxy,
                        host_id: Some("mirror-test".parse().expect("proxy id")),
                        capabilities: Vec::new(),
                    },
                },
            })
            .await
            .map_err(|error| DaemonError::Protocol(error.to_string()))?;
        response(&mut framed, 1).await?;
        framed
            .send(Request { id: 2, body })
            .await
            .map_err(|error| DaemonError::Protocol(error.to_string()))?;
        response(&mut framed, 2).await
    }
}

#[async_trait]
impl RemoteEndpoint for CommandEndpoint {
    fn host(&self) -> &HostId {
        self.provider.id()
    }

    fn state(&self) -> LinkState {
        *self.state.borrow()
    }

    fn hello(&self) -> Option<RemoteHello> {
        None
    }

    async fn request(&self, body: RequestBody) -> DaemonResult<ResponseBody> {
        *self
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout(Duration::from_secs(1), self.request_once(body.clone()))
                .await
            {
                Ok(Ok(response)) => return Ok(response),
                Ok(Err(error)) if Instant::now() < deadline => {
                    tracing::debug!(%error, "remote daemon not ready yet");
                }
                Err(_) if Instant::now() < deadline => {}
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err(DaemonError::Timeout("remote test endpoint".into())),
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    fn state_changes(&self) -> watch::Receiver<LinkState> {
        self.state.subscribe()
    }

    async fn close(&self) {
        self.state.send_replace(LinkState::Down);
    }
}

async fn response<T>(
    framed: &mut Framed<T, FleetCodec<Request, serde_json::Value>>,
    id: u64,
) -> DaemonResult<ResponseBody>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let value = framed
            .next()
            .await
            .ok_or_else(|| DaemonError::Protocol("remote closed the stream".into()))?
            .map_err(|error| DaemonError::Protocol(error.to_string()))?;
        let Ok(response) = serde_json::from_value::<Response>(value) else {
            continue;
        };
        if response.id != id {
            continue;
        }
        return response
            .result
            .map_err(|error| DaemonError::Protocol(error.message));
    }
}

async fn local_services(
    host: &HostId,
    remote_config: HostConfigEntry,
) -> (Arc<Services>, Arc<ConfigStore>) {
    let temp = tempfile::tempdir().expect("local home");
    let home = temp.keep();
    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    let config = Arc::new(ConfigStore::new(&home, files.clone()));
    let mut effective = config.load().await.expect("load config");
    effective.hosts.insert(host.clone(), remote_config);
    config.save(effective).await.expect("save config");
    let events = BroadcastBus::default();
    let services = Services::new_with_events(
        &home,
        config.clone(),
        Arc::new(StateStore::new(&home, files.clone(), Arc::new(SystemClock))),
        Arc::new(JobManager::new(&home)),
        Adapters::system(files),
        events,
    );
    (services, config)
}

fn write_published_state(home: &std::path::Path) {
    std::fs::create_dir_all(home).expect("create remote home");
    let state = published_state(None);
    std::fs::write(
        home.join("state.json"),
        serde_json::to_string_pretty(&state).expect("state json"),
    )
    .expect("write state");
}

fn published_state(host: Option<HostId>) -> fleet_core::state::State {
    let mut state = default_state();
    state.contexts.push(Context {
        id: "acme".parse().expect("context"),
        name: "Acme".into(),
        owners: vec!["acme".into()],
        created_at: "2026-09-08T00:00:00Z".into(),
    });
    state.repos.push(Repo {
        id: "acme/api".parse().expect("repo"),
        owner: "acme".into(),
        name: "api".into(),
        url: "https://example.invalid/acme/api".into(),
        context_id: "acme".parse().expect("context"),
        default_branch: "main".into(),
        path: "/repos/acme/api".into(),
        cloned_at: "2026-09-08T00:00:00Z".into(),
        hooks: RepoHooks::default(),
    });
    let mut worktree = remote_worktree();
    worktree.host = host;
    state.worktrees.push(worktree);
    state
}

fn remote_snapshot(session: SessionState) -> Snapshot {
    Snapshot {
        boards: Vec::new(),
        generated_at: "2026-09-08T00:00:00Z".into(),
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees: vec![remote_worktree()],
        active_context: None,
        sessions: Vec::new(),
        agent_threads: Vec::new(),
        statuses: vec![WorktreeStatus {
            worktree_id: "acme/api#remote".parse().expect("worktree"),
            session,
            windows: Vec::new(),
            running: Vec::new(),
            agent_activity: AgentActivity::Unknown,
            agent_activity_changed_at: None,
        }],
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: DaemonInfo {
            version: "fleetd test".into(),
            pid: 1,
            started_at: "2026-09-08T00:00:00Z".into(),
            home: "/remote/.fleet".into(),
        },
    }
}

fn remote_worktree() -> Worktree {
    Worktree {
        id: "acme/api#remote".parse().expect("worktree"),
        repo_id: "acme/api".parse().expect("repo"),
        slug: "remote".into(),
        branch: "remote".into(),
        base_ref: "origin/main".into(),
        path: "/remote/acme/api/remote".into(),
        session: "api/remote".into(),
        host: None,
        created_at: "2026-09-08T00:00:00Z".into(),
        last_opened_at: None,
        degraded: None,
    }
}

async fn wait_for_snapshot(
    events: &mut broadcast::Receiver<Event>,
    predicate: impl Fn(&Snapshot) -> bool,
) -> Snapshot {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Event::SnapshotChanged(snapshot) = events.recv().await.expect("snapshot event")
                && predicate(&snapshot)
            {
                return snapshot;
            }
        }
    })
    .await
    .expect("timed out waiting for matching snapshot")
}

async fn wait_for_fragment(
    services: &Services,
    host: &HostId,
    predicate: impl Fn(&fleet_daemon::services::mirror::MirrorFragment) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if services
                .mirror
                .fragment(host)
                .as_ref()
                .is_some_and(&predicate)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("timed out waiting for remote mirror fragment");
}
