use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use fleet_core::{
    agents::ThreadId,
    config::default_config,
    ids::{HostId, JobId, TerminalId, WorktreeId},
    sessions::{Session, SessionKind, Terminal, TerminalKind, TerminalStatus},
};
use fleet_daemon::{
    machines::{Machines, RemoteEndpoint},
    server::BroadcastBus,
    services::{
        mirror::Mirror,
        router::{RemoteIds, Resolver, Router, Target, classify, translate},
    },
    testing::FakeRemote,
};
use fleet_proto::{
    event::Event,
    request::RequestBody,
    response::{ResponseBody, WorktreeDeleteResult},
};

struct ScriptedResolver {
    first: WorktreeId,
    second: WorktreeId,
    first_host: HostId,
    second_host: HostId,
}

impl Resolver for ScriptedResolver {
    fn host_of_worktree(&self, id: &WorktreeId) -> Option<HostId> {
        if id == &self.first {
            Some(self.first_host.clone())
        } else if id == &self.second {
            Some(self.second_host.clone())
        } else {
            None
        }
    }

    fn host_of_session(&self, _id: &str) -> Option<HostId> {
        None
    }

    fn host_of_terminal(&self, id: TerminalId) -> Option<HostId> {
        (id == TerminalId(91)).then(|| self.first_host.clone())
    }

    fn host_of_job(&self, _id: &JobId) -> Option<HostId> {
        None
    }

    fn host_of_thread(&self, _id: &ThreadId) -> Option<HostId> {
        None
    }
}

#[test]
fn classifies_local_host_and_fanout_requests() {
    let first = worktree("one");
    let second = worktree("two");
    let local = worktree("local");
    let first_host = host("alpha");
    let second_host = host("beta");
    let resolver = ScriptedResolver {
        first: first.clone(),
        second: second.clone(),
        first_host: first_host.clone(),
        second_host: second_host.clone(),
    };

    assert_eq!(
        classify::classify(&RequestBody::GetConfig, &resolver),
        Target::Local
    );
    assert_eq!(
        classify::classify(&RequestBody::WorktreePath { id: first.clone() }, &resolver),
        Target::Host(first_host.clone())
    );
    assert_eq!(
        classify::classify(
            &RequestBody::CreateWorktree {
                repo: "acme/api".parse().expect("repo"),
                slug: "new".to_owned(),
                branch: None,
                base: None,
                host: Some(second_host.clone()),
                hooks: Default::default(),
            },
            &resolver,
        ),
        Target::Host(second_host.clone())
    );
    let target = classify::classify(
        &RequestBody::DeleteWorktrees {
            ids: vec![local, first, second],
        },
        &resolver,
    );
    assert!(matches!(target, Target::Fanout(parts) if parts.len() == 2));
    assert_eq!(
        classify::classify(
            &RequestBody::RequestFullFrame {
                terminal: TerminalId(91),
            },
            &resolver,
        ),
        Target::Host(first_host)
    );
}

#[tokio::test]
async fn forward_translates_request_and_response_ids_and_clears_placement() {
    let host = host("alpha");
    let (router, remote) = router_with_remote(host.clone());
    let local = router.ids.local_terminal(&host, TerminalId(1));
    remote.push_response(Ok(ResponseBody::Terminal(terminal(TerminalId(1)))));

    let response = router
        .forward(&host, RequestBody::RequestFullFrame { terminal: local })
        .await
        .expect("forward terminal request");
    assert!(matches!(response, ResponseBody::Terminal(value) if value.id == local));
    assert_eq!(
        remote.requests(),
        vec![RequestBody::RequestFullFrame {
            terminal: TerminalId(1),
        }]
    );

    remote.push_response(Ok(ResponseBody::Ack));
    router
        .forward(
            &host,
            RequestBody::CreateWorktree {
                repo: "acme/api".parse().expect("repo"),
                slug: "remote".to_owned(),
                branch: None,
                base: None,
                host: Some(host.clone()),
                hooks: Default::default(),
            },
        )
        .await
        .expect("forward create request");
    assert!(matches!(
        &remote.requests()[1],
        RequestBody::CreateWorktree { host: None, .. }
    ));
}

#[tokio::test]
async fn fanout_preserves_per_item_failure_for_a_down_host() {
    let ready_host = host("alpha");
    let down_host = host("beta");
    let machines = Arc::new(Machines::from_config(&default_config("/tmp/fleet-router")));
    let ready = Arc::new(FakeRemote::new(ready_host.clone()));
    let down = Arc::new(FakeRemote::new(down_host.clone()));
    let _state_guard = down.state_changes();
    down.set_state(fleet_proto::snapshot::LinkState::Down);
    machines.install_endpoint(ready_host.clone(), ready.clone());
    machines.install_endpoint(down_host.clone(), down);
    let router = Router::new(machines, Arc::new(Mirror::new()));
    let ready_id = worktree("ready");
    let down_id = worktree("down");
    ready.push_response(Ok(ResponseBody::WorktreesDeleted(vec![
        WorktreeDeleteResult {
            worktree_id: ready_id.clone(),
            ok: true,
            reason: None,
            trash_entry: Some("ready-trash".to_owned()),
        },
    ])));
    let original = RequestBody::DeleteWorktrees {
        ids: vec![ready_id.clone(), down_id.clone()],
    };

    let parts = router
        .fanout(vec![
            (
                ready_host,
                RequestBody::DeleteWorktrees {
                    ids: vec![ready_id.clone()],
                },
            ),
            (
                down_host,
                RequestBody::DeleteWorktrees {
                    ids: vec![down_id.clone()],
                },
            ),
        ])
        .await;
    let merged = translate::merge_fanout(&original, parts).expect("merge delete fanout");
    let ResponseBody::WorktreesDeleted(results) = merged else {
        panic!("expected delete results");
    };
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .any(|result| result.worktree_id == ready_id && result.ok)
    );
    assert!(
        results.iter().any(|result| {
            result.worktree_id == down_id
                && !result.ok
                && result
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("beta"))
        }),
        "{results:?}"
    );
}

#[test]
fn identical_remote_terminal_ids_are_distinct_and_share_the_injected_counter() {
    let next = Arc::new(AtomicU64::new(40));
    let ids = RemoteIds::new(Arc::clone(&next));
    let first = ids.local_terminal(&host("alpha"), TerminalId(1));
    let second = ids.local_terminal(&host("beta"), TerminalId(1));

    assert_eq!(first, TerminalId(40));
    assert_eq!(second, TerminalId(41));
    assert_ne!(first, second);
    assert_eq!(next.load(Ordering::Relaxed), 42);
}

#[test]
fn clear_host_removes_every_mapping_for_only_that_host() {
    let target_host = host("alpha");
    let other = host("beta");
    let ids = RemoteIds::default();
    let local = ids.local_terminal(&target_host, TerminalId(1));
    let retained = ids.local_terminal(&other, TerminalId(1));
    let session = ids.local_session(&target_host, "acme/api");
    ids.register_worktree(&target_host, worktree("gone"));

    let cleared = ids.clear_host(&target_host);

    assert_eq!(cleared.terminals, vec![local]);
    assert_eq!(cleared.sessions, vec![session]);
    assert!(ids.remote_terminal(local).is_none());
    assert!(ids.host_of_worktree(&worktree("gone")).is_none());
    assert_eq!(ids.remote_terminal(retained), Some((other, TerminalId(1))));
}

#[tokio::test]
async fn down_transition_clears_ids_and_emits_terminal_end_and_link_events() {
    let host = host("alpha");
    let (router, remote) = router_with_remote(host.clone());
    let local = router.ids.local_terminal(&host, TerminalId(1));
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    router.start_event_pumps(events);

    remote.set_state(fleet_proto::snapshot::LinkState::Down);

    let mut exited = false;
    let mut down = false;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !exited || !down {
            match receiver.recv().await.expect("router event") {
                Event::TerminalExited { terminal, code } => {
                    exited = terminal == local && code.is_none();
                }
                Event::HostLinkChanged {
                    host: event_host,
                    link: fleet_proto::snapshot::LinkState::Down,
                    ..
                } => down = event_host == host,
                _ => {}
            }
        }
    })
    .await
    .expect("down events");
    assert!(router.ids.remote_terminal(local).is_none());
}

#[tokio::test]
async fn ready_reensures_attached_session_and_restores_its_local_terminal_id() {
    let host = host("alpha");
    let (router, remote) = router_with_remote(host.clone());
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    router.start_event_pumps(events);
    let worktree = worktree("attached");
    let ensure = RequestBody::EnsureSession {
        worktree: Some(worktree.clone()),
        agent: None,
        sleep_previous: false,
    };
    remote.push_response(Ok(ResponseBody::Session(session(
        worktree.clone(),
        TerminalId(7),
    ))));
    let ResponseBody::Session(first) = router
        .forward(&host, ensure.clone())
        .await
        .expect("initial ensure")
    else {
        panic!("expected session");
    };
    let stable = first.terminals[0].id;
    remote.push_response(Ok(ResponseBody::Ack));
    router
        .forward(
            &host,
            RequestBody::AttachTerminal {
                terminal: stable,
                cols: 80,
                rows: 24,
            },
        )
        .await
        .expect("attach");

    remote.set_state(fleet_proto::snapshot::LinkState::Down);
    wait_until(|| router.ids.remote_terminal(stable).is_none()).await;
    remote.push_response(Ok(ResponseBody::Session(session(worktree, TerminalId(99)))));
    remote.set_state(fleet_proto::snapshot::LinkState::Ready);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if matches!(
                receiver.recv().await.expect("router event"),
                Event::TerminalReattach { terminal } if terminal == stable
            ) {
                break;
            }
        }
    })
    .await
    .expect("reattach event");
    assert_eq!(
        router.ids.remote_terminal(stable),
        Some((host, TerminalId(99)))
    );
    assert_eq!(
        remote
            .requests()
            .iter()
            .filter(|request| **request == ensure)
            .count(),
        2
    );
}

#[tokio::test]
async fn remote_snapshot_fragments_are_not_published_as_global_snapshots() {
    let host = host("alpha");
    let (router, remote) = router_with_remote(host.clone());
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    router.start_event_pumps(events);
    remote.emit(Event::SnapshotChanged(
        serde_json::from_value(serde_json::json!({
            "boards": [],
            "generatedAt": "2026-09-08T12:00:00Z",
            "contexts": [],
            "repos": [],
            "clones": [],
            "worktrees": [],
            "activeContext": null,
            "sessions": [],
            "agentThreads": [],
            "statuses": [],
            "pools": [],
            "hosts": [],
            "jobs": [],
            "daemon": {"version":"remote","pid":1,"startedAt":"now","home":"/remote"}
        }))
        .expect("snapshot"),
    ));

    let leaked = tokio::time::timeout(Duration::from_millis(100), async {
        loop {
            if matches!(receiver.recv().await, Ok(Event::SnapshotChanged(_))) {
                return true;
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(!leaked, "a remote fragment must not replace local state");
    assert!(router.mirror.fragment(&host).is_some());
}

#[tokio::test]
async fn replacing_an_endpoint_replaces_its_event_pump() {
    let host = host("alpha");
    let machines = Arc::new(Machines::from_config(&default_config("/tmp/fleet-router")));
    let old = Arc::new(FakeRemote::new(host.clone()));
    machines.install_endpoint(host.clone(), old.clone());
    let router = Router::new(Arc::clone(&machines), Arc::new(Mirror::new()));
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    router.start_event_pumps(events.clone());

    let replacement = Arc::new(FakeRemote::new(host.clone()));
    machines.install_endpoint(host, replacement.clone());
    router.start_event_pumps(events);
    tokio::time::sleep(Duration::from_millis(10)).await;
    old.emit(Event::Toast {
        level: fleet_proto::event::ToastLevel::Info,
        message: "stale".to_owned(),
    });
    replacement.emit(Event::Toast {
        level: fleet_proto::event::ToastLevel::Info,
        message: "current".to_owned(),
    });

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Event::Toast { message, .. } = receiver.recv().await.expect("event") {
                assert_eq!(message, "current");
                break;
            }
        }
    })
    .await
    .expect("replacement event");
}

fn router_with_remote(host: HostId) -> (Router, Arc<FakeRemote>) {
    let machines = Arc::new(Machines::from_config(&default_config("/tmp/fleet-router")));
    let remote = Arc::new(FakeRemote::new(host.clone()));
    machines.install_endpoint(host, remote.clone());
    (Router::new(machines, Arc::new(Mirror::new())), remote)
}

fn host(value: &str) -> HostId {
    value.parse().expect("host")
}

fn worktree(slug: &str) -> WorktreeId {
    format!("acme/api#{slug}").parse().expect("worktree")
}

fn terminal(id: TerminalId) -> Terminal {
    Terminal {
        id,
        name: "shell".to_owned(),
        command: "zsh".to_owned(),
        cwd: "/tmp".to_owned(),
        shell_pid: Some(10),
        foreground_command: None,
        status: TerminalStatus::Running,
        title: None,
        keep_alive: Vec::new(),
        has_unseen_output: false,
        agent_attention: None,
        kind: TerminalKind::Pty,
    }
}

fn session(worktree: WorktreeId, terminal_id: TerminalId) -> Session {
    Session {
        id: "acme/api/attached".parse().expect("session"),
        host: None,
        kind: SessionKind::Worktree(worktree),
        cwd: "/tmp".to_owned(),
        terminals: vec![terminal(terminal_id)],
        active_terminal: Some(terminal_id),
        slept_at: None,
        kept_terminals: Vec::new(),
    }
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
