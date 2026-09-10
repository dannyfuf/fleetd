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

#[tokio::test]
async fn hosted_create_creates_and_updates_a_missing_remote_context_before_cloning() {
    let target = host("context-sync-create");
    let (router, remote) = router_with_remote(target.clone());
    let context = context_sync_local_context("Personal");
    let repo = context_sync_repo();
    script_context_sync_responses(
        &remote,
        vec![
            Ok(ResponseBody::Snapshot(context_sync_snapshot(None))),
            Ok(ResponseBody::Context(context_sync_remote_context(
                "personal",
                &["dannyfuf"],
            ))),
            Ok(ResponseBody::Context(context.clone())),
        ],
        &repo,
    );

    router
        .ensure_repo_then_create(
            &target,
            &context,
            &repo,
            &[],
            context_sync_create(&repo, Some(target.clone())),
        )
        .await
        .expect("create with missing remote context");

    assert_eq!(
        remote.requests(),
        context_sync_expected_requests(
            &context,
            &repo,
            vec![
                RequestBody::GetSnapshot,
                RequestBody::CreateContext {
                    name: "personal".to_owned(),
                    owners: vec!["dannyfuf".to_owned()],
                },
                context_sync_update_request(&context),
            ],
        )
    );
}

#[tokio::test]
async fn hosted_create_rejects_a_context_create_response_with_a_different_id() {
    let target = host("context-sync-wrong-create-id");
    let (router, remote) = router_with_remote(target.clone());
    let context = context_sync_local_context("Personal");
    let repo = context_sync_repo();
    let mut mismatched = context_sync_remote_context("personal", &["dannyfuf"]);
    mismatched.id = "other".parse().expect("context id");
    remote.push_response(Ok(ResponseBody::Snapshot(context_sync_snapshot(None))));
    remote.push_response(Ok(ResponseBody::Context(mismatched.clone())));

    let error = router
        .ensure_repo_then_create(
            &target,
            &context,
            &repo,
            &[],
            context_sync_create(&repo, Some(target.clone())),
        )
        .await
        .expect_err("mismatched create response must fail");

    assert!(matches!(error, fleet_daemon::DaemonError::Protocol(message)
    if message == format!(
        "remote context ensure returned unexpected response {:?}",
        ResponseBody::Context(mismatched)
    )));
    assert_eq!(
        remote.requests(),
        vec![
            RequestBody::GetSnapshot,
            RequestBody::CreateContext {
                name: "personal".to_owned(),
                owners: vec!["dannyfuf".to_owned()],
            },
        ]
    );
}

#[tokio::test]
async fn hosted_create_leaves_an_identical_remote_context_unchanged_before_cloning() {
    let target = host("context-sync-identical");
    let (router, remote) = router_with_remote(target.clone());
    let context = context_sync_local_context("Personal");
    let repo = context_sync_repo();
    script_context_sync_responses(
        &remote,
        vec![Ok(ResponseBody::Snapshot(context_sync_snapshot(Some(
            context.clone(),
        ))))],
        &repo,
    );

    router
        .ensure_repo_then_create(
            &target,
            &context,
            &repo,
            &[],
            context_sync_create(&repo, Some(target.clone())),
        )
        .await
        .expect("create with identical remote context");

    assert_eq!(
        remote.requests(),
        context_sync_expected_requests(&context, &repo, vec![RequestBody::GetSnapshot])
    );
}

#[tokio::test]
async fn hosted_create_updates_each_mismatched_remote_context_once_before_cloning() {
    let cases = [
        (
            "context-sync-name",
            context_sync_remote_context("Old Personal", &["dannyfuf"]),
        ),
        (
            "context-sync-owners",
            context_sync_remote_context("Personal", &["someone-else"]),
        ),
    ];

    for (host_name, remote_context) in cases {
        let target = host(host_name);
        let (router, remote) = router_with_remote(target.clone());
        let context = context_sync_local_context("Personal");
        let repo = context_sync_repo();
        script_context_sync_responses(
            &remote,
            vec![
                Ok(ResponseBody::Snapshot(context_sync_snapshot(Some(
                    remote_context,
                )))),
                Ok(ResponseBody::Context(context.clone())),
            ],
            &repo,
        );

        router
            .ensure_repo_then_create(
                &target,
                &context,
                &repo,
                &[],
                context_sync_create(&repo, Some(target.clone())),
            )
            .await
            .expect("create with mismatched remote context");

        assert_eq!(
            remote.requests(),
            context_sync_expected_requests(
                &context,
                &repo,
                vec![
                    RequestBody::GetSnapshot,
                    context_sync_update_request(&context)
                ],
            ),
            "mismatch case {host_name}"
        );
    }
}

#[tokio::test]
async fn hosted_create_recovers_from_a_remote_context_create_conflict() {
    let target = host("context-sync-conflict");
    let (router, remote) = router_with_remote(target.clone());
    let context = context_sync_local_context("Personal");
    let repo = context_sync_repo();
    script_context_sync_responses(
        &remote,
        vec![
            Ok(ResponseBody::Snapshot(context_sync_snapshot(None))),
            Err(fleet_daemon::DaemonError::Conflict(
                "context personal already exists".to_owned(),
            )),
            Ok(ResponseBody::Snapshot(context_sync_snapshot(Some(
                context.clone(),
            )))),
        ],
        &repo,
    );

    router
        .ensure_repo_then_create(
            &target,
            &context,
            &repo,
            &[],
            context_sync_create(&repo, Some(target.clone())),
        )
        .await
        .expect("create after context conflict");

    assert_eq!(
        remote.requests(),
        context_sync_expected_requests(
            &context,
            &repo,
            vec![
                RequestBody::GetSnapshot,
                RequestBody::CreateContext {
                    name: "personal".to_owned(),
                    owners: vec!["dannyfuf".to_owned()],
                },
                RequestBody::GetSnapshot,
            ],
        )
    );
}

#[tokio::test]
async fn hosted_create_uses_context_id_then_restores_a_renamed_display_name() {
    let target = host("context-sync-renamed");
    let (router, remote) = router_with_remote(target.clone());
    let context = context_sync_local_context("Personal Stuff");
    let repo = context_sync_repo();
    script_context_sync_responses(
        &remote,
        vec![
            Ok(ResponseBody::Snapshot(context_sync_snapshot(None))),
            Ok(ResponseBody::Context(context_sync_remote_context(
                "personal",
                &["dannyfuf"],
            ))),
            Ok(ResponseBody::Context(context.clone())),
        ],
        &repo,
    );

    router
        .ensure_repo_then_create(
            &target,
            &context,
            &repo,
            &[],
            context_sync_create(&repo, Some(target.clone())),
        )
        .await
        .expect("create with renamed local context");

    assert_eq!(
        remote.requests(),
        context_sync_expected_requests(
            &context,
            &repo,
            vec![
                RequestBody::GetSnapshot,
                RequestBody::CreateContext {
                    name: "personal".to_owned(),
                    owners: vec!["dannyfuf".to_owned()],
                },
                context_sync_update_request(&context),
            ],
        )
    );
}

fn context_sync_local_context(name: &str) -> fleet_core::model::Context {
    context_sync_remote_context(name, &["dannyfuf"])
}

fn context_sync_remote_context(name: &str, owners: &[&str]) -> fleet_core::model::Context {
    fleet_core::model::Context {
        id: "personal".parse().expect("context id"),
        name: name.to_owned(),
        owners: owners.iter().map(|owner| (*owner).to_owned()).collect(),
        created_at: "2026-09-09T12:00:00Z".to_owned(),
    }
}

fn context_sync_repo() -> fleet_core::model::Repo {
    fleet_core::model::Repo {
        id: "acme/api".parse().expect("repo id"),
        owner: "acme".to_owned(),
        name: "api".to_owned(),
        url: "ssh://git.example/acme/api.git".to_owned(),
        context_id: "personal".parse().expect("context id"),
        default_branch: "main".to_owned(),
        path: "/tmp/context-sync/repos/acme/api".to_owned(),
        cloned_at: "2026-09-09T12:00:00Z".to_owned(),
        hooks: fleet_core::model::RepoHooks {
            prepare: vec!["mise install".to_owned()],
            post_create: vec!["bin/setup".to_owned()],
        },
    }
}

fn context_sync_snapshot(
    context: Option<fleet_core::model::Context>,
) -> fleet_proto::snapshot::Snapshot {
    fleet_proto::snapshot::Snapshot {
        boards: Vec::new(),
        generated_at: "2026-09-09T12:00:00Z".to_owned(),
        contexts: context.into_iter().collect(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees: Vec::new(),
        active_context: None,
        sessions: Vec::new(),
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: fleet_proto::snapshot::DaemonInfo {
            version: "fleetd context-sync test".to_owned(),
            pid: 1,
            started_at: "2026-09-09T12:00:00Z".to_owned(),
            home: "/tmp/context-sync".to_owned(),
        },
    }
}

fn context_sync_create(repo: &fleet_core::model::Repo, host: Option<HostId>) -> RequestBody {
    RequestBody::CreateWorktree {
        repo: repo.id.clone(),
        slug: "context-sync".to_owned(),
        branch: Some("feature/context-sync".to_owned()),
        base: Some("origin/main".to_owned()),
        host,
        hooks: repo.hooks.clone(),
    }
}

fn context_sync_update_request(context: &fleet_core::model::Context) -> RequestBody {
    RequestBody::UpdateContext {
        id: context.id.clone(),
        name: Some(context.name.clone()),
        owners: Some(context.owners.clone()),
    }
}

fn script_context_sync_responses(
    remote: &FakeRemote,
    context_responses: Vec<Result<ResponseBody, fleet_daemon::DaemonError>>,
    repo: &fleet_core::model::Repo,
) {
    for response in context_responses {
        remote.push_response(response);
    }
    remote.push_response(Ok(ResponseBody::Repo(repo.clone())));
    remote.push_response(Ok(ResponseBody::Repo(repo.clone())));
    remote.push_response(Ok(ResponseBody::Ack));
}

fn context_sync_expected_requests(
    context: &fleet_core::model::Context,
    repo: &fleet_core::model::Repo,
    mut context_requests: Vec<RequestBody>,
) -> Vec<RequestBody> {
    context_requests.extend([
        RequestBody::CloneRepo {
            owner: repo.owner.clone(),
            name: repo.name.clone(),
            url: repo.url.clone(),
            context: context.id.clone(),
            default_branch: Some(repo.default_branch.clone()),
        },
        RequestBody::SetRepoHooks {
            repo: repo.id.clone(),
            hooks: repo.hooks.clone(),
        },
        context_sync_create(repo, None),
    ]);
    context_requests
}
