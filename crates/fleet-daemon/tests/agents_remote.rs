use std::{sync::Arc, time::Duration};

use chrono::Utc;
use fleet_core::{
    agents::{
        AgentEvent, AgentKind, PermissionMode, Seq, SeqEvent, ThreadId, ThreadProjection, UserInput,
    },
    config::default_config,
    ids::{ContextId, HostId, RepoId, WorktreeId},
    model::{Context, HostConfigEntry, Repo, RepoHooks, Worktree},
    state::default_state,
};
use fleet_daemon::{
    adapters::{clock::SystemClock, files::RealFiles},
    machines::{LinkOptions, MachineProvider, Machines, RemoteEndpoint, RemoteLink},
    server::BroadcastBus,
    services::{
        mirror::Mirror,
        router::{
            Router, Target,
            agents::{
                ThreadRegistrations, agent_list_target, register_mirror_threads,
                register_thread_events,
            },
        },
    },
    stores::{config::ConfigStore, state::StateStore},
    testing::FakeRemote,
};
use fleet_proto::{
    event::Event,
    request::RequestBody,
    response::ResponseBody,
    snapshot::{DaemonInfo, LinkState, Snapshot},
};

mod infra;

#[tokio::test]
async fn remote_agent_create_events_followups_restart_resume_and_deletion() {
    let temp = tempfile::tempdir().expect("tempdir");
    let local_agents = temp.path().join("agents");
    let host = host("dev-box");
    let worktree = worktree("remote-agent");
    let thread = ThreadId::new();
    let summary =
        ThreadProjection::new(thread, worktree.clone(), AgentKind::Claude).summary(Seq::default());

    let mirror = Arc::new(Mirror::new());
    mirror.apply(
        &host,
        snapshot(
            vec![remote_worktree(worktree.clone())],
            Vec::new(),
            temp.path().to_string_lossy().into_owned(),
        ),
    );
    let machines = Arc::new(Machines::from_config(&default_config(temp.path())));
    let remote = Arc::new(FakeRemote::new(host.clone()));
    machines.install_endpoint(host.clone(), remote.clone());
    let router = Router::new(machines, mirror);
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    router.start_event_pumps(events);

    let create = RequestBody::AgentThreadCreate {
        worktree: worktree.clone(),
        provider: AgentKind::Claude,
        model: None,
        mode: PermissionMode::Ask,
        resume_cursor: None,
        title: Some("remote task".to_owned()),
    };
    assert_eq!(router.route(&create), Target::Host(host.clone()));
    remote.push_response(Ok(ResponseBody::AgentThreadCreated(summary.clone())));
    let created = router
        .forward(&host, create.clone())
        .await
        .expect("forward remote thread creation");
    assert!(matches!(created, ResponseBody::AgentThreadCreated(value) if value.thread == thread));
    assert_eq!(remote.requests(), vec![create]);
    assert_eq!(router.ids.host_of_thread(&thread), Some(host.clone()));

    let sequenced = SeqEvent {
        seq: Seq(1),
        at: Utc::now(),
        raw: None,
        event: AgentEvent::Notice("remote provider ready".to_owned()),
    };
    remote.emit(Event::Agent {
        thread,
        event: sequenced.clone(),
    });
    let event = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("remote agent event timeout")
        .expect("remote agent event");
    assert_eq!(
        event,
        Event::Agent {
            thread,
            event: sequenced
        }
    );

    let send = RequestBody::AgentSend {
        thread,
        input: UserInput {
            text: "continue".to_owned(),
            attachments: Vec::new(),
        },
    };
    assert_eq!(router.route(&send), Target::Host(host.clone()));

    remote.set_state(LinkState::Down);
    wait_until(|| router.ids.host_of_thread(&thread).is_none()).await;
    remote.set_state(LinkState::Ready);
    remote.emit(Event::SnapshotChanged(snapshot(
        vec![remote_worktree(worktree)],
        vec![summary.clone()],
        temp.path().to_string_lossy().into_owned(),
    )));
    wait_until(|| router.ids.host_of_thread(&thread) == Some(host.clone())).await;

    let open = RequestBody::AgentThreadOpen {
        thread,
        from_seq: None,
    };
    assert_eq!(router.route(&open), Target::Host(host.clone()));
    remote.push_response(Ok(ResponseBody::AgentThreadSnapshot {
        projection: ThreadProjection::new(thread, summary.worktree.clone(), AgentKind::Claude),
        events_after: Vec::new(),
    }));
    router
        .forward(&host, open.clone())
        .await
        .expect("reopen remote thread after mirror re-sync");
    assert_eq!(remote.requests().last(), Some(&open));
    assert!(
        !local_agents.exists(),
        "routing a remote thread must not create local transcript files"
    );

    let registrations = ThreadRegistrations::default();
    register_mirror_threads(
        &registrations,
        &host,
        std::slice::from_ref(&summary),
        &router.ids,
    );
    register_thread_events(
        &registrations,
        &Event::SnapshotChanged(snapshot(
            Vec::new(),
            Vec::new(),
            temp.path().to_string_lossy().into_owned(),
        )),
        &host,
        &router.ids,
    );
    assert!(router.ids.host_of_thread(&thread).is_none());
    assert_eq!(router.route(&send), Target::Local);
}

#[test]
fn agent_listing_is_a_fanout_with_unchanged_requests() {
    let first = host("alpha");
    let second = host("beta");
    assert_eq!(
        agent_list_target([first.clone(), second.clone()]),
        Target::Fanout(vec![
            (first, RequestBody::AgentThreadList),
            (second, RequestBody::AgentThreadList),
        ])
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let router = Router::new(
        Arc::new(Machines::from_config(&default_config(temp.path()))),
        Arc::new(Mirror::new()),
    );
    assert_eq!(
        router.route(&RequestBody::AgentThreadList),
        Target::Fanout(Vec::new()),
    );
}

#[tokio::test]
async fn agent_listing_creates_partitions_before_any_snapshot_request() {
    let remote_host = host("configured");
    let mut config = default_config("/tmp/fleet-agent-list");
    config.hosts.insert(
        remote_host.clone(),
        HostConfigEntry::Command {
            run: vec!["false".to_owned()],
            fleetd: "fleetd".to_owned(),
            fleet_home: None,
            display: None,
        },
    );
    let router = Router::new(
        Arc::new(Machines::from_config(&config)),
        Arc::new(Mirror::new()),
    );

    assert_eq!(
        router.route(&RequestBody::AgentThreadList),
        Target::Fanout(vec![(remote_host, RequestBody::AgentThreadList)])
    );
}

#[tokio::test]
async fn agent_listing_keeps_cached_remote_threads_when_one_host_is_down() {
    let temp = tempfile::tempdir().expect("tempdir");
    let host = host("dev-box");
    let worktree = worktree("cached");
    let thread = ThreadId::new();
    let summary =
        ThreadProjection::new(thread, worktree.clone(), AgentKind::Claude).summary(Seq::default());
    let mirror = Arc::new(Mirror::new());
    mirror.apply(
        &host,
        snapshot(
            vec![remote_worktree(worktree)],
            vec![summary],
            temp.path().display().to_string(),
        ),
    );
    let machines = Arc::new(Machines::from_config(&default_config(temp.path())));
    let remote = Arc::new(FakeRemote::new(host.clone()));
    let _state_guard = remote.state_changes();
    remote.set_state(LinkState::Down);
    machines.install_endpoint(host.clone(), remote);
    let router = Router::new(machines, mirror);

    let parts = router
        .fanout(vec![(host.clone(), RequestBody::AgentThreadList)])
        .await;
    let merged = fleet_daemon::services::router::translate::merge_fanout(
        &RequestBody::AgentThreadList,
        parts,
    )
    .expect("cached agent list");
    let ResponseBody::AgentThreads(threads) = merged else {
        panic!("expected agent threads");
    };
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].thread, thread);
    assert_eq!(threads[0].host.as_ref(), Some(&host));
}

#[tokio::test]
async fn forwarded_provider_errors_name_the_execution_host() {
    let host = host("dev-box");
    let machines = Arc::new(Machines::from_config(&default_config("/tmp/fleet-agents")));
    let remote = Arc::new(FakeRemote::new(host.clone()));
    remote.push_response(Err(fleet_daemon::DaemonError::Unsupported(
        "provider unavailable on this machine".to_owned(),
    )));
    machines.install_endpoint(host.clone(), remote);
    let router = Router::new(machines, Arc::new(Mirror::new()));

    let error = router
        .forward(&host, RequestBody::AgentThreadList)
        .await
        .expect_err("provider error");
    assert!(error.to_string().contains("host dev-box"));
}

#[tokio::test]
async fn two_daemon_agent_resume_over_real_remote_link() {
    let _ = infra::DaemonProcess::wait;
    let temp = tempfile::tempdir().expect("tempdir");
    let remote_home = temp.path().join("remote");
    let local_home = temp.path().join("local");
    let worktree = prepare_remote_agent_home(&remote_home).await;
    let remote = infra::RemoteDaemon::start(&remote_home);
    infra::assert_remote_contract(&remote);
    let infra::RemoteDaemon {
        daemon, machine, ..
    } = remote;
    let provider: Arc<dyn MachineProvider> = Arc::new(machine);
    let link = RemoteLink::new(
        provider,
        LinkOptions {
            backoff_min: Duration::from_millis(20),
            backoff_max: Duration::from_millis(100),
            hello_timeout: Duration::from_secs(5),
        },
    );
    let mut states = link.state_changes();
    link.connect().await.expect("remote link");
    let host = link.host().clone();
    let machines = Arc::new(Machines::from_config(&default_config(&local_home)));
    machines.install_endpoint(host.clone(), link.clone());
    let router = Router::new(machines, Arc::new(Mirror::new()));
    router.ids.register_worktree(&host, worktree.clone());
    router.start_event_pumps(BroadcastBus::default());

    let created = router
        .forward(
            &host,
            RequestBody::AgentThreadCreate {
                worktree: worktree.clone(),
                provider: AgentKind::Claude,
                model: None,
                mode: PermissionMode::Ask,
                resume_cursor: None,
                title: None,
            },
        )
        .await
        .expect("create remote agent");
    let ResponseBody::AgentThreadCreated(summary) = created else {
        panic!("expected created thread");
    };
    router
        .forward(
            &host,
            RequestBody::AgentSend {
                thread: summary.thread,
                input: UserInput {
                    text: "continue".to_owned(),
                    attachments: Vec::new(),
                },
            },
        )
        .await
        .expect("send remote prompt");
    wait_for_remote_text(&router, &host, summary.thread, "remote continuity").await;

    drop(daemon);
    wait_for_link_state(&mut states, LinkState::Down).await;
    let restarted = infra::DaemonProcess::start(&remote_home);
    wait_for_link_state(&mut states, LinkState::Ready).await;
    wait_until(|| router.ids.host_of_thread(&summary.thread) == Some(host.clone())).await;
    let reopened = router
        .forward(
            &host,
            RequestBody::AgentThreadOpen {
                thread: summary.thread,
                from_seq: None,
            },
        )
        .await
        .expect("reopen persisted remote agent");
    let ResponseBody::AgentThreadSnapshot { projection, .. } = reopened else {
        panic!("expected thread snapshot");
    };
    assert!(
        projection
            .items
            .iter()
            .any(|item| item.text.as_deref() == Some("remote continuity")),
        "persisted transcript must survive the remote daemon restart"
    );
    assert!(
        !local_home.join("agents").exists(),
        "remote transcripts must never be persisted by the local daemon"
    );
    link.close().await;
    drop(restarted);
}

async fn wait_for_remote_text(router: &Router, host: &HostId, thread: ThreadId, expected: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(ResponseBody::AgentThreadSnapshot { projection, .. }) = router
                .forward(
                    host,
                    RequestBody::AgentThreadOpen {
                        thread,
                        from_seq: None,
                    },
                )
                .await
                && projection
                    .items
                    .iter()
                    .any(|item| item.text.as_deref() == Some(expected))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("remote transcript text");
}

async fn wait_for_link_state(
    states: &mut tokio::sync::watch::Receiver<LinkState>,
    expected: LinkState,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if *states.borrow_and_update() == expected {
                return;
            }
            states.changed().await.expect("link state sender");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("link did not reach {expected:?}"));
}

async fn prepare_remote_agent_home(home: &std::path::Path) -> WorktreeId {
    use std::os::unix::fs::PermissionsExt as _;

    let script = home.join("fake-claude");
    let worktree_path = home.join("worktrees/owner/repo/remote-agent");
    let repos = home.join("repos");
    std::fs::create_dir_all(&worktree_path).expect("worktree directory");
    std::fs::create_dir_all(&repos).expect("repos directory");
    std::fs::write(
        &script,
        r##"#!/bin/sh
case " $* " in
  *" --version "*) printf '%s\n' '2.1.0 (Claude Code)'; exit 0 ;;
esac
printf '%s\n' '{"type":"system","subtype":"init","session_id":"cursor-remote","model":"test","tools":[],"slash_commands":[]}'
while IFS= read -r line; do
  printf '%s\n' '{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text","text":"remote continuity"}]},"parent_tool_use_id":null}'
  printf '%s\n' '{"type":"result","subtype":"success","terminal_reason":"completed","usage":{}}'
done
"##,
    )
    .expect("fake Claude script");
    let mut permissions = std::fs::metadata(&script)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).expect("script permissions");

    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [repos.clone(), home.join("worktrees")],
    ));
    let config = ConfigStore::new(home, files.clone());
    let mut effective = config.load().await.expect("remote config");
    effective.agent_commands.claude = script.display().to_string();
    config.save(effective).await.expect("save remote config");

    let state = StateStore::new(home, files, Arc::new(SystemClock));
    let context = ContextId::try_from("team").expect("context");
    let repo = RepoId::try_from("owner/repo").expect("repo");
    let worktree = worktree("remote-agent");
    let mut persisted = default_state();
    persisted.contexts.push(Context {
        id: context.clone(),
        name: "Team".to_owned(),
        owners: vec!["owner".to_owned()],
        created_at: Utc::now().to_rfc3339(),
    });
    persisted.repos.push(Repo {
        id: repo.clone(),
        owner: "owner".to_owned(),
        name: "repo".to_owned(),
        url: "https://example.invalid/owner/repo".to_owned(),
        context_id: context,
        default_branch: "main".to_owned(),
        path: repos.join("owner/repo").display().to_string(),
        cloned_at: Utc::now().to_rfc3339(),
        hooks: RepoHooks::default(),
    });
    persisted.worktrees.push(Worktree {
        id: worktree.clone(),
        repo_id: repo,
        slug: "remote-agent".to_owned(),
        branch: "remote-agent".to_owned(),
        base_ref: "main".to_owned(),
        path: worktree_path.display().to_string(),
        session: "owner/repo/remote-agent".to_owned(),
        host: None,
        created_at: Utc::now().to_rfc3339(),
        last_opened_at: None,
        degraded: None,
    });
    state.save(persisted).await.expect("save remote state");
    worktree
}

async fn wait_until(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("condition timeout");
}

fn snapshot(
    worktrees: Vec<Worktree>,
    agent_threads: Vec<fleet_core::agents::AgentThreadSummary>,
    home: String,
) -> Snapshot {
    Snapshot {
        boards: Vec::new(),
        generated_at: Utc::now().to_rfc3339(),
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees,
        active_context: None,
        sessions: Vec::new(),
        agent_threads,
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: DaemonInfo {
            version: "test".to_owned(),
            pid: 1,
            started_at: Utc::now().to_rfc3339(),
            home,
        },
    }
}

fn remote_worktree(id: WorktreeId) -> Worktree {
    Worktree {
        id,
        repo_id: "owner/repo".parse().expect("repo"),
        slug: "remote-agent".to_owned(),
        branch: "remote-agent".to_owned(),
        base_ref: "main".to_owned(),
        path: "/remote/worktrees/remote-agent".to_owned(),
        session: "owner/repo/remote-agent".to_owned(),
        host: None,
        created_at: Utc::now().to_rfc3339(),
        last_opened_at: None,
        degraded: None,
    }
}

fn host(value: &str) -> HostId {
    value.parse().expect("host")
}

fn worktree(slug: &str) -> WorktreeId {
    format!("owner/repo#{slug}").parse().expect("worktree")
}
