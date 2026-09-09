use std::{path::Path, process::Command, sync::Arc, time::Duration};

use fleet_core::{
    config::default_config,
    ids::{ContextId, HostId, RepoId, SessionId, WorktreeId},
    inspection::WorktreeInspection,
    model::{Context, Repo, RepoHooks, Worktree},
    sessions::SessionState,
    state::default_state,
};
use fleet_daemon::{
    adapters::{clock::SystemClock, files::RealFiles},
    machines::{
        CommandMachine, LinkOptions, MachineProvider, Machines, RemoteEndpoint, RemoteLink,
    },
    services::{
        mirror::Mirror,
        router::{Router, Target, translate},
    },
    stores::state::StateStore,
    testing::FakeRemote,
};
use fleet_proto::{
    request::RequestBody,
    response::{PruneResult, ResponseBody, SleepResult, WorktreeDeleteResult},
    snapshot::{DaemonInfo, LinkState, Snapshot},
};

mod infra;

#[tokio::test]
async fn delete_fanout_keeps_ready_and_down_outcomes_in_request_order() {
    let fixture = Fixture::new();
    fixture
        .ready
        .push_response(Ok(ResponseBody::WorktreesDeleted(vec![
            WorktreeDeleteResult {
                worktree_id: fixture.ready_id.clone(),
                ok: true,
                reason: None,
                trash_entry: Some("ready-trash".to_owned()),
            },
        ])));
    let body = RequestBody::DeleteWorktrees {
        ids: vec![fixture.down_id.clone(), fixture.ready_id.clone()],
    };

    let ResponseBody::WorktreesDeleted(results) = fixture.execute(body).await else {
        panic!("expected delete results");
    };

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].worktree_id, fixture.down_id);
    assert!(!results[0].ok);
    assert!(
        results[0]
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("down"))
    );
    assert_eq!(results[1].worktree_id, fixture.ready_id);
    assert!(results[1].ok);
    assert!(fixture.down.requests().is_empty());
}

#[tokio::test]
async fn inspect_fanout_turns_a_down_host_into_an_item_error() {
    let fixture = Fixture::new();
    fixture
        .ready
        .push_response(Ok(ResponseBody::Inspections(vec![inspection(
            fixture.ready_id.clone(),
            fixture.ready_host.as_str(),
        )])));
    let body = RequestBody::InspectWorktrees {
        ids: vec![fixture.ready_id.clone(), fixture.down_id.clone()],
        repo: None,
        fetch: false,
    };

    let ResponseBody::Inspections(results) = fixture.execute(body).await else {
        panic!("expected inspection results");
    };

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].worktree_id, fixture.ready_id);
    assert!(results[0].error.is_none());
    assert_eq!(results[1].worktree_id, fixture.down_id);
    assert_eq!(results[1].host, fixture.down_host.as_str());
    assert!(
        results[1]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unreachable"))
    );
}

#[tokio::test]
async fn real_remote_inspection_reports_the_forwarding_host() {
    let _ = infra::DaemonProcess::wait;
    let remote_home = tempfile::tempdir().expect("remote home");
    let worktree = prepare_remote_inspection_worktree(remote_home.path()).await;
    let remote = infra::RemoteDaemon::start(remote_home.path());
    infra::assert_remote_contract(&remote);
    let machine: Arc<CommandMachine> = Arc::new(remote.machine);
    let provider: Arc<dyn MachineProvider> = machine;
    let link = RemoteLink::new(
        provider,
        LinkOptions {
            backoff_min: Duration::from_millis(10),
            backoff_max: Duration::from_millis(50),
            hello_timeout: Duration::from_secs(2),
        },
    );
    link.connect().await.expect("connect remote link");

    let machines = Arc::new(Machines::from_config(&default_config(
        remote_home.path().join("local"),
    )));
    let host = link.host().clone();
    machines.install_endpoint(host.clone(), link);
    let router = Router::new(machines, Arc::new(Mirror::new()));

    let response = router
        .forward(
            &host,
            RequestBody::InspectWorktrees {
                ids: vec![worktree.clone()],
                repo: None,
                fetch: false,
            },
        )
        .await
        .expect("inspect remote worktree");
    let ResponseBody::Inspections(inspections) = response else {
        panic!("expected inspection results");
    };

    assert_eq!(inspections.len(), 1);
    assert_eq!(inspections[0].worktree_id, worktree);
    assert_eq!(inspections[0].host, host.as_str());
    assert!(inspections[0].error.is_none(), "{:?}", inspections[0].error);
}

#[tokio::test]
async fn prune_fanout_turns_a_down_host_into_a_skip() {
    let fixture = Fixture::new();
    fixture
        .ready
        .push_response(Ok(ResponseBody::Pruned(PruneResult {
            dry_run: true,
            deleted: vec![fixture.ready_id.clone()],
            skipped: Vec::new(),
        })));
    let body = RequestBody::PruneWorktrees {
        dry_run: true,
        fetch: false,
        kill_sessions: false,
        repo: None,
        ids: Some(vec![fixture.down_id.clone(), fixture.ready_id.clone()]),
    };

    let ResponseBody::Pruned(result) = fixture.execute(body).await else {
        panic!("expected prune results");
    };

    assert_eq!(result.deleted, vec![fixture.ready_id]);
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].worktree_id, fixture.down_id);
    assert!(result.skipped[0].reason.contains("unreachable"));
}

#[tokio::test]
async fn remote_path_sleep_and_kill_use_the_owner_and_remote_session_id() {
    let fixture = Fixture::new();
    fixture.ready.push_response(Ok(ResponseBody::Path {
        path: "/srv/fleet/worktrees/acme/api/ready".to_owned(),
        host: None,
    }));
    let path = fixture
        .router
        .forward(
            &fixture.ready_host,
            RequestBody::WorktreePath {
                id: fixture.ready_id.clone(),
            },
        )
        .await
        .expect("remote path");
    assert_eq!(
        path,
        ResponseBody::Path {
            path: "/srv/fleet/worktrees/acme/api/ready".to_owned(),
            host: Some(fixture.ready_host.clone()),
        }
    );

    let local_session = SessionId::try_from(
        fixture
            .router
            .ids
            .local_session(&fixture.ready_host, "acme/api#ready"),
    )
    .expect("local remote session");
    fixture
        .ready
        .push_response(Ok(ResponseBody::Slept(SleepResult {
            kept: Vec::new(),
            closed: vec!["shell".to_owned()],
            session_killed: true,
        })));
    fixture
        .router
        .forward(
            &fixture.ready_host,
            RequestBody::SleepSession {
                session: local_session.clone(),
            },
        )
        .await
        .expect("remote sleep");
    fixture.ready.push_response(Ok(ResponseBody::Ack));
    fixture
        .router
        .forward(
            &fixture.ready_host,
            RequestBody::KillSession {
                session: local_session,
            },
        )
        .await
        .expect("remote kill");

    assert_eq!(
        fixture.ready.requests(),
        vec![
            RequestBody::WorktreePath {
                id: fixture.ready_id,
            },
            RequestBody::SleepSession {
                session: SessionId::try_from("acme/api#ready").expect("remote session"),
            },
            RequestBody::KillSession {
                session: SessionId::try_from("acme/api#ready").expect("remote session"),
            },
        ]
    );
}

struct Fixture {
    router: Router,
    ready: Arc<FakeRemote>,
    down: Arc<FakeRemote>,
    _down_state: tokio::sync::watch::Receiver<LinkState>,
    ready_host: HostId,
    down_host: HostId,
    ready_id: WorktreeId,
    down_id: WorktreeId,
}

impl Fixture {
    fn new() -> Self {
        let ready_host = host("ready");
        let down_host = host("down");
        let ready_id = worktree("ready");
        let down_id = worktree("down");
        let ready = Arc::new(FakeRemote::new(ready_host.clone()));
        let down = Arc::new(FakeRemote::new(down_host.clone()));
        let down_state = down.state_changes();
        down.set_state(LinkState::Down);
        let machines = Arc::new(Machines::from_config(&default_config(
            "/tmp/fleet-lifecycle",
        )));
        machines.install_endpoint(ready_host.clone(), ready.clone());
        machines.install_endpoint(down_host.clone(), down.clone());
        let mirror = Arc::new(Mirror::new());
        mirror.apply(
            &ready_host,
            snapshot(vec![remote_worktree(ready_id.clone())]),
        );
        mirror.apply(&down_host, snapshot(vec![remote_worktree(down_id.clone())]));
        let router = Router::new(machines, mirror);
        Self {
            router,
            ready,
            down,
            _down_state: down_state,
            ready_host,
            down_host,
            ready_id,
            down_id,
        }
    }

    async fn execute(&self, body: RequestBody) -> ResponseBody {
        let Target::Fanout(parts) = self.router.route(&body) else {
            panic!("expected lifecycle fanout");
        };
        translate::merge_fanout(&body, self.router.fanout(parts).await)
            .expect("merge lifecycle fanout")
    }
}

fn host(value: &str) -> HostId {
    value.parse().expect("host")
}

fn repo() -> RepoId {
    "acme/api".parse().expect("repo")
}

fn worktree(slug: &str) -> WorktreeId {
    format!("acme/api#{slug}").parse().expect("worktree")
}

fn remote_worktree(id: WorktreeId) -> Worktree {
    Worktree {
        repo_id: repo(),
        slug: id.slug().to_owned(),
        branch: id.slug().to_owned(),
        base_ref: "origin/main".to_owned(),
        path: format!("/srv/fleet/worktrees/acme/api/{}", id.slug()),
        session: id.to_string(),
        host: None,
        created_at: "2026-09-08T00:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
        id,
    }
}

fn snapshot(worktrees: Vec<Worktree>) -> Snapshot {
    Snapshot {
        boards: Vec::new(),
        generated_at: "2026-09-08T00:00:00Z".to_owned(),
        contexts: Vec::new(),
        repos: Vec::new(),
        clones: Vec::new(),
        worktrees,
        active_context: None,
        sessions: Vec::new(),
        agent_threads: Vec::new(),
        statuses: Vec::new(),
        pools: Vec::new(),
        hosts: Vec::new(),
        jobs: Vec::new(),
        daemon: DaemonInfo {
            version: "fleetd test".to_owned(),
            pid: 1,
            started_at: "2026-09-08T00:00:00Z".to_owned(),
            home: "/srv/fleet".to_owned(),
        },
    }
}

fn inspection(id: WorktreeId, host: &str) -> WorktreeInspection {
    WorktreeInspection {
        repo_id: repo(),
        worktree_id: id,
        host: host.to_owned(),
        path: "/srv/fleet/worktree".to_owned(),
        branch: "ready".to_owned(),
        base_ref: "origin/main".to_owned(),
        head: Some("abc".to_owned()),
        target_branch: "main".to_owned(),
        upstream: Some("origin/ready".to_owned()),
        ahead: Some(0),
        behind: Some(0),
        upstream_gone: false,
        dirty: false,
        dirty_files: Some(0),
        merged_into_target: true,
        unique_commits: Some(0),
        published: true,
        merged: true,
        pr: None,
        session: SessionState::None,
        running: Vec::new(),
        inspected_at: "2026-09-08T00:00:00Z".to_owned(),
        warnings: Vec::new(),
        error: None,
    }
}

async fn prepare_remote_inspection_worktree(home: &Path) -> WorktreeId {
    let worktree_path = home.join("worktrees/acme/api/remote");
    std::fs::create_dir_all(&worktree_path).expect("worktree directory");
    run_git(&worktree_path, &["init", "-b", "remote"]);
    run_git(
        &worktree_path,
        &["config", "user.email", "fleet@example.test"],
    );
    run_git(&worktree_path, &["config", "user.name", "Fleet Test"]);
    std::fs::write(worktree_path.join("README.md"), "remote fixture\n")
        .expect("write remote fixture");
    run_git(&worktree_path, &["add", "README.md"]);
    run_git(&worktree_path, &["commit", "-m", "remote fixture"]);

    let context_id = ContextId::try_from("acme").expect("context id");
    let repo_id = repo();
    let worktree_id = worktree("remote");
    let mut state = default_state();
    state.contexts.push(Context {
        id: context_id.clone(),
        name: "Acme".to_owned(),
        owners: vec!["acme".to_owned()],
        created_at: "2026-09-08T00:00:00Z".to_owned(),
    });
    state.repos.push(Repo {
        id: repo_id.clone(),
        owner: "acme".to_owned(),
        name: "api".to_owned(),
        url: "https://example.invalid/acme/api".to_owned(),
        context_id,
        default_branch: "main".to_owned(),
        path: worktree_path.display().to_string(),
        cloned_at: "2026-09-08T00:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    });
    state.worktrees.push(Worktree {
        id: worktree_id.clone(),
        repo_id,
        slug: "remote".to_owned(),
        branch: "remote".to_owned(),
        base_ref: "main".to_owned(),
        path: worktree_path.display().to_string(),
        session: "api/remote".to_owned(),
        host: None,
        created_at: "2026-09-08T00:00:00Z".to_owned(),
        last_opened_at: None,
        degraded: None,
    });

    let files = Arc::new(RealFiles::new(
        home.join("trash"),
        [home.join("repos"), home.join("worktrees")],
    ));
    StateStore::new(home, files, Arc::new(SystemClock))
        .save(state)
        .await
        .expect("save remote state");
    worktree_id
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git fixture command");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
