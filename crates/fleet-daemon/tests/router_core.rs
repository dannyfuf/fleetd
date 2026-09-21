use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use fleet_core::{
    agents::ThreadId,
    board::{Board, BoardSettings, BoardSummary, BoardView, Card, Priority, SyncState},
    config::default_config,
    ids::{BoardId, CardId, HostId, JobId, StatusId, TerminalId, WorktreeId},
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
    hosted_board: BoardId,
    hosted_card: CardId,
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

    fn host_of_board(&self, id: &BoardId) -> Option<HostId> {
        (id == &self.hosted_board).then(|| self.first_host.clone())
    }

    fn host_of_card(&self, id: &CardId) -> Option<HostId> {
        (id == &self.hosted_card).then(|| self.first_host.clone())
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
        hosted_board: board("wt-acme-api-one"),
        hosted_card: card("card-one"),
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
        Target::Host(first_host.clone())
    );
    assert_eq!(
        classify::classify(
            &RequestBody::GetBoard {
                board_id: board("wt-acme-api-one"),
            },
            &resolver,
        ),
        Target::Host(first_host.clone())
    );
    assert_eq!(
        classify::classify(
            &RequestBody::MoveCard {
                card_id: card("card-one"),
                status_id: status(),
                index: None,
            },
            &resolver,
        ),
        Target::Host(first_host)
    );
    assert_eq!(
        classify::classify(
            &RequestBody::GetBoard {
                board_id: board("personal"),
            },
            &resolver,
        ),
        Target::Local
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
    ids.register_board(&target_host, board("wt-acme-api-gone"));
    ids.register_card(&board("wt-acme-api-gone"), card("card-kept"));

    let cleared = ids.clear_host(&target_host);

    assert_eq!(cleared.terminals, vec![local]);
    assert_eq!(cleared.sessions, vec![session]);
    assert_eq!(cleared.boards, vec![board("wt-acme-api-gone")]);
    assert!(ids.remote_terminal(local).is_none());
    assert!(ids.host_of_worktree(&worktree("gone")).is_none());
    assert!(ids.host_of_board(&board("wt-acme-api-gone")).is_none());
    // Card→board is a fact about the card, not about the link, so it outlives the host going down.
    assert_eq!(
        ids.board_of_card(&card("card-kept")),
        Some(board("wt-acme-api-gone"))
    );
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

#[tokio::test]
async fn a_remote_detach_that_fails_still_releases_the_local_attachment() {
    let host = host("alpha");
    let (router, remote) = router_with_remote(host.clone());
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    router.start_event_pumps(events);
    let local = router.ids.local_terminal(&host, TerminalId(7));

    remote.push_response(Ok(ResponseBody::Ack));
    router
        .forward(
            &host,
            RequestBody::AttachTerminal {
                terminal: local,
                cols: 80,
                rows: 24,
            },
        )
        .await
        .expect("attach");

    remote.push_response(Err(fleet_daemon::DaemonError::NotFound(
        "terminal 7".to_owned(),
    )));
    router
        .forward(&host, RequestBody::DetachTerminal { terminal: local })
        .await
        .expect_err("the remote rejects a detach for a terminal it forgot");

    remote.emit(Event::TerminalFrame(frame(TerminalId(7))));
    let relayed = tokio::time::timeout(Duration::from_millis(100), async {
        loop {
            if matches!(
                receiver.recv().await.expect("router event"),
                Event::TerminalFrame(_)
            ) {
                return true;
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(!relayed, "frames still relayed after a failed detach");
}

fn frame(terminal: TerminalId) -> fleet_proto::terminal::FrameUpdate {
    use fleet_proto::terminal::{CursorShape, CursorState, TerminalModes, ViewportInfo};

    fleet_proto::terminal::FrameUpdate {
        terminal,
        seq: 1,
        cols: 80,
        rows: 24,
        full: true,
        shift: None,
        rows_changed: Vec::new(),
        cursor: CursorState {
            row: 0,
            col: 0,
            visible: true,
            shape: CursorShape::Block,
        },
        viewport: ViewportInfo {
            scrollback_len: 0,
            offset: 0,
            history_epoch: 0,
        },
        modes: TerminalModes::default(),
        title: None,
    }
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
        revision: None,
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

#[tokio::test]
async fn a_remote_daemon_shutting_down_never_reaches_the_local_bus() {
    let host = host("alpha");
    let (router, remote) = router_with_remote(host.clone());
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    router.start_event_pumps(events);

    // A remote daemon stopping is a link-liveness fact about one endpoint; republished verbatim
    // it would read as *this* daemon shutting down and disconnect every local client.
    remote.emit(Event::DaemonShuttingDown);
    remote.emit(Event::Toast {
        level: fleet_proto::event::ToastLevel::Info,
        message: "after the remote shutdown".to_owned(),
    });

    let marker = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match receiver.recv().await.expect("router event") {
                Event::DaemonShuttingDown => return false,
                Event::Toast { message, .. } if message == "after the remote shutdown" => {
                    return true;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the pump keeps running after a remote shutdown event");
    assert!(
        marker,
        "a remote daemon's shutdown must not be republished as the local daemon's"
    );
}

#[tokio::test]
async fn a_hosted_worktree_board_is_forwarded_and_its_cards_become_routable() {
    let host = host("alpha");
    let (router, remote) = router_with_remote(host.clone());
    let worktree_id = worktree("feat-workflows-v1");
    let board_id = board("wt-acme-api-feat-workflows-v1");
    let card_id = card("card-eleven");
    router.ids.register_worktree(&host, worktree_id.clone());
    let ensure = RequestBody::EnsureWorktreeBoard {
        worktree_id: worktree_id.clone(),
    };

    // The worktree's owner owns its board, so the scope request leaves this daemon instead of
    // creating a second empty document here.
    assert_eq!(router.route(&ensure), Target::Host(host.clone()));
    remote.push_response(Ok(ResponseBody::Board(board_view(
        &board_id,
        Some(&worktree_id),
        std::slice::from_ref(&card_id),
    ))));
    let answer = router
        .forward(&host, ensure.clone())
        .await
        .expect("forward the worktree board ensure");
    assert!(matches!(answer, ResponseBody::Board(view) if view.board.id == board_id));
    assert_eq!(remote.requests(), vec![ensure]);

    // The answer taught the router both halves, so a later card mutation follows the same daemon.
    assert_eq!(
        router.route(&RequestBody::MoveCard {
            card_id,
            status_id: status(),
            index: None,
        }),
        Target::Host(host)
    );
}

#[tokio::test]
async fn a_board_event_is_published_only_for_a_board_this_daemon_routes() {
    let host = host("alpha");
    let (router, remote) = router_with_remote(host.clone());
    let routed = board("wt-acme-api-routed");
    let context_board = board("personal");
    router.ids.register_board(&host, routed.clone());
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    router.start_event_pumps(events);

    // A host's context board carries an id this daemon also uses, so republishing its change
    // would make every client re-read the wrong board.
    remote.emit(Event::BoardChanged {
        board_id: context_board.clone(),
        reason: fleet_proto::event::BoardChangeReason::CardChanged,
    });
    remote.emit(Event::BoardChanged {
        board_id: routed.clone(),
        reason: fleet_proto::event::BoardChangeReason::CardChanged,
    });

    let published = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Event::BoardChanged { board_id, .. } =
                receiver.recv().await.expect("router event")
            {
                return board_id;
            }
        }
    })
    .await
    .expect("a routed board event reaches the local bus");
    assert_eq!(published, routed);
}

#[tokio::test]
async fn a_host_without_the_worktree_board_capability_is_refused_by_name() {
    let host = host("alpha");
    let (router, remote) = router_with_remote(host.clone());
    remote.set_hello(fleet_daemon::machines::RemoteHello {
        version: "fleetd old".to_owned(),
        daemon_id: "old".to_owned(),
        build_commit: None,
        capabilities: vec![fleet_proto::REMOTE_MACHINES_CAPABILITY.to_owned()],
    });

    let error = router
        .forward(
            &host,
            RequestBody::EnsureWorktreeBoard {
                worktree_id: worktree("feat-workflows-v1"),
            },
        )
        .await
        .expect_err("an un-upgraded owner cannot serve a worktree board");

    assert!(
        matches!(&error, fleet_daemon::DaemonError::Unsupported(message)
            if message == "host alpha: this daemon does not support worktree boards; run `fleet daemon restart`"),
        "{error:?}"
    );
    // The refusal happens here, so the old daemon never sees a request it cannot answer.
    assert!(remote.requests().is_empty());
}

#[tokio::test]
async fn a_down_host_keeps_answering_board_ownership_through_the_mirror_until_ready() {
    let host = host("alpha");
    let (router, remote) = router_with_remote(host.clone());
    let worktree_id = worktree("feat-workflows-v1");
    let board_id = board("wt-acme-api-feat-workflows-v1");
    router.ids.register_board(&host, board_id.clone());
    router
        .mirror
        .apply(&host, snapshot_with_board(&board_id, Some(&worktree_id)));
    let events = BroadcastBus::default();
    router.start_event_pumps(events);

    remote.set_state(fleet_proto::snapshot::LinkState::Down);
    wait_until(|| router.ids.host_of_board(&board_id).is_none()).await;
    // Ownership left the id table with the link, and the snapshot fragment covers the window:
    // a board request still goes to its owner rather than being answered by an empty local board.
    assert_eq!(
        Resolver::host_of_board(&router, &board_id),
        Some(host.clone())
    );

    remote.set_last_snapshot(snapshot_with_board(&board_id, Some(&worktree_id)));
    remote.set_state(fleet_proto::snapshot::LinkState::Ready);
    wait_until(|| router.ids.host_of_board(&board_id) == Some(host.clone())).await;
}

fn board(value: &str) -> BoardId {
    value.parse().expect("board id")
}

fn card(value: &str) -> CardId {
    value.parse().expect("card id")
}

fn status() -> StatusId {
    "todo".parse().expect("status id")
}

fn board_summary(id: &BoardId, worktree: Option<&WorktreeId>) -> BoardSummary {
    BoardSummary {
        id: id.clone(),
        context_id: "personal".parse().expect("context id"),
        worktree_id: worktree.cloned(),
        name: "board".to_owned(),
        prefix: "FLT".to_owned(),
        backend_kind: "local".to_owned(),
        card_count: 11,
        open_count: 11,
        dirty_count: 0,
        conflict_count: 0,
        working_count: 0,
        attention_count: 0,
        last_synced_at: None,
        last_error: None,
    }
}

fn board_view(id: &BoardId, worktree: Option<&WorktreeId>, cards: &[CardId]) -> BoardView {
    BoardView {
        board: Board {
            id: id.clone(),
            context_id: "personal".parse().expect("context id"),
            worktree_id: worktree.cloned(),
            name: "board".to_owned(),
            prefix: "FLT".to_owned(),
            next_number: 12,
            backend: fleet_core::board::BackendRef::default(),
            statuses: Vec::new(),
            labels: Vec::new(),
            properties: Vec::new(),
            default_repo_id: None,
            settings: BoardSettings::default(),
            sync: SyncState::default(),
            created_at: "2026-09-20T12:00:00Z".to_owned(),
            updated_at: "2026-09-20T12:00:00Z".to_owned(),
        },
        cards: cards
            .iter()
            .cloned()
            .map(|card_id| Card {
                id: card_id,
                board_id: id.clone(),
                number: 1,
                title: "card".to_owned(),
                description: String::new(),
                status_id: status(),
                priority: Priority::default(),
                labels: Vec::new(),
                assignee: None,
                estimate: None,
                due_date: None,
                parent_id: None,
                repo_id: None,
                worktree_id: None,
                properties: Default::default(),
                comments: Vec::new(),
                activity: Vec::new(),
                remote: None,
                conflict: None,
                dirty: false,
                archived: false,
                position: 0,
                agent: None,
                blocked_by: Vec::new(),
                pending_run: None,
                runs: Vec::new(),
                created_at: "2026-09-20T12:00:00Z".to_owned(),
                updated_at: "2026-09-20T12:00:00Z".to_owned(),
            })
            .collect(),
        live_runs: Vec::new(),
    }
}

fn snapshot_with_board(
    id: &BoardId,
    worktree: Option<&WorktreeId>,
) -> fleet_proto::snapshot::Snapshot {
    fleet_proto::snapshot::Snapshot {
        boards: vec![board_summary(id, worktree)],
        ..context_sync_snapshot(None)
    }
}
