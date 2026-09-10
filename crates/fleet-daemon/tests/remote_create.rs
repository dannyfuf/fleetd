use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use fleet_core::{
    config::default_config,
    ids::{ContextId, HostId, RepoId},
    model::{Context, Repo, RepoHooks},
    state::{State, default_state},
};
use fleet_proto::{
    PROTOCOL_VERSION,
    codec::FleetCodec,
    request::{ClientKind, HelloClient, Request, RequestBody},
    response::{Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt};
use tokio::net::UnixStream;
use tokio_util::codec::Framed;

mod infra;

const IO_TIMEOUT: Duration = Duration::from_secs(60);
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const CONTEXT_ID: &str = "personal";
const REPO_ID: &str = "dannyfuf/remote-context-sync";

#[tokio::test]
async fn real_two_daemon_remote_create_contract() {
    run_remote_create_scenario(None, "missing-context").await;
    run_remote_create_scenario(Some("Former Personal"), "existing-context").await;
}

async fn run_remote_create_scenario(remote_context_name: Option<&str>, slug: &str) {
    let fixture = tempfile::tempdir().expect("create two-daemon fixture");
    let source = create_bare_repository(fixture.path());
    let local_home = fixture.path().join("local");
    let remote_home = fixture.path().join("remote");
    let context = fixture_context("Personal");
    let repo = fixture_repo(&source);

    write_state(&local_home, Some(context.clone()), Some(repo.clone()));
    write_config(&remote_home, None);
    write_state(&remote_home, remote_context_name.map(fixture_context), None);

    let remote = infra::RemoteDaemon::start(&remote_home);
    infra::assert_remote_contract(&remote);
    let infra::RemoteDaemon {
        daemon: mut remote_process,
        machine: _remote_machine,
        config: remote_config,
    } = remote;
    let host = HostId::try_from("loopback").expect("loopback host id");
    write_config(&local_home, Some((host.clone(), remote_config)));
    let mut local_process = infra::DaemonProcess::start(&local_home);

    let mut local = DaemonClient::connect(&local_home.join("fleetd.sock")).await;
    let create_response = request_when_remote_ready(
        &mut local,
        RequestBody::CreateWorktree {
            repo: repo.id.clone(),
            slug: slug.to_owned(),
            branch: None,
            base: None,
            host: Some(host.clone()),
            hooks: RepoHooks::default(),
        },
    )
    .await;

    shutdown(&mut local, &mut local_process).await;
    let mut remote_client = DaemonClient::connect(&remote_home.join("fleetd.sock")).await;
    shutdown(&mut remote_client, &mut remote_process).await;

    let response = create_response
        .result
        .unwrap_or_else(|error| panic!("hosted create failed: {}", error.message));
    let ResponseBody::Worktree {
        created, worktree, ..
    } = response
    else {
        panic!("expected worktree response, got {response:?}");
    };
    assert!(created, "the remote worktree should be newly created");
    assert_eq!(worktree.host.as_ref(), Some(&host));
    assert_eq!(worktree.repo_id, repo.id);
    assert_eq!(worktree.slug, slug);

    let remote_state = read_state(&remote_home);
    let remote_contexts = remote_state
        .contexts
        .iter()
        .filter(|candidate| candidate.id == context.id)
        .collect::<Vec<_>>();
    assert_eq!(remote_state.contexts.len(), 1);
    assert_eq!(remote_contexts.len(), 1);
    assert_eq!(remote_contexts[0].name, context.name);
    assert_eq!(remote_contexts[0].owners, context.owners);

    let remote_repos = remote_state
        .repos
        .iter()
        .filter(|candidate| candidate.id == repo.id)
        .collect::<Vec<_>>();
    assert_eq!(remote_repos.len(), 1);
    assert_eq!(remote_repos[0].context_id, context.id);
    assert_eq!(remote_repos[0].url, repo.url);

    let remote_worktrees = remote_state
        .worktrees
        .iter()
        .filter(|candidate| candidate.id == worktree.id)
        .collect::<Vec<_>>();
    assert_eq!(remote_worktrees.len(), 1);
    assert!(remote_worktrees[0].host.is_none());
    assert_eq!(remote_worktrees[0].repo_id, repo.id);

    let local_state = read_state(&local_home);
    assert!(
        local_state
            .worktrees
            .iter()
            .all(|candidate| candidate.id != worktree.id || candidate.host.is_some()),
        "the local daemon persisted a duplicate local worktree"
    );
}

async fn request_when_remote_ready(client: &mut DaemonClient, body: RequestBody) -> Response {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        let response = client.request(body.clone()).await;
        let link_starting = response
            .result
            .as_ref()
            .is_err_and(|error| error.message.contains("host loopback is unreachable"));
        if !link_starting {
            return response;
        }
        assert!(
            Instant::now() < deadline,
            "local daemon did not establish its remote link: {response:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

struct DaemonClient {
    framed: Framed<UnixStream, FleetCodec<Request, serde_json::Value>>,
    next_id: u64,
}

impl DaemonClient {
    async fn connect(socket: &Path) -> Self {
        let stream = connect_until_ready(socket).await;
        let mut client = Self {
            framed: Framed::new(stream, FleetCodec::new()),
            next_id: 1,
        };
        let hello = client
            .request(RequestBody::Hello {
                protocol: PROTOCOL_VERSION,
                client: HelloClient {
                    kind: ClientKind::Cli,
                    host_id: None,
                },
            })
            .await;
        assert!(
            matches!(
                hello.result,
                Ok(ResponseBody::Hello {
                    protocol: PROTOCOL_VERSION,
                    ..
                })
            ),
            "daemon rejected test handshake: {hello:?}"
        );
        client
    }

    async fn request(&mut self, body: RequestBody) -> Response {
        let id = self.next_id;
        self.next_id += 1;
        tokio::time::timeout(IO_TIMEOUT, self.framed.send(Request { id, body }))
            .await
            .expect("daemon request write timed out")
            .expect("write daemon request");
        let value = tokio::time::timeout(IO_TIMEOUT, self.framed.next())
            .await
            .expect("daemon response timed out")
            .expect("daemon closed connection")
            .expect("read daemon response");
        let response: Response = serde_json::from_value(value).expect("decode daemon response");
        assert_eq!(response.id, id, "daemon response id mismatch");
        response
    }
}

async fn connect_until_ready(socket: &Path) -> UnixStream {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        match UnixStream::connect(socket).await {
            Ok(stream) => return stream,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("daemon socket {} was not ready: {error}", socket.display()),
        }
    }
}

async fn shutdown(client: &mut DaemonClient, process: &mut infra::DaemonProcess) {
    let response = client
        .request(RequestBody::DaemonShutdown {
            stop_sessions: false,
        })
        .await;
    assert_eq!(response.result, Ok(ResponseBody::ShuttingDown));
    process.wait().await;
}

fn fixture_context(name: &str) -> Context {
    Context {
        id: ContextId::try_from(CONTEXT_ID).expect("context id"),
        name: name.to_owned(),
        owners: vec!["dannyfuf".to_owned()],
        created_at: "2026-09-09T00:00:00Z".to_owned(),
    }
}

fn fixture_repo(source: &BareRepository) -> Repo {
    Repo {
        id: RepoId::try_from(REPO_ID).expect("repo id"),
        owner: "dannyfuf".to_owned(),
        name: "remote-context-sync".to_owned(),
        url: source.bare.display().to_string(),
        context_id: ContextId::try_from(CONTEXT_ID).expect("context id"),
        default_branch: "main".to_owned(),
        path: source.seed.display().to_string(),
        cloned_at: "2026-09-09T00:00:00Z".to_owned(),
        hooks: RepoHooks::default(),
    }
}

fn write_config(home: &Path, remote: Option<(HostId, fleet_core::model::HostConfigEntry)>) {
    std::fs::create_dir_all(home).expect("create fleet home");
    let mut config = default_config(home);
    config.hot_pool_size = 0;
    config.hot_refresh_interval_ms = 0;
    if let Some((host, entry)) = remote {
        config.hosts.insert(host, entry);
    }
    std::fs::write(
        home.join("config.json"),
        serde_json::to_string_pretty(&config).expect("encode daemon config"),
    )
    .expect("write daemon config");
}

fn write_state(home: &Path, context: Option<Context>, repo: Option<Repo>) {
    std::fs::create_dir_all(home).expect("create fleet home");
    let mut state = default_state();
    state.contexts.extend(context);
    state.repos.extend(repo);
    std::fs::write(
        home.join("state.json"),
        serde_json::to_string_pretty(&state).expect("encode daemon state"),
    )
    .expect("write daemon state");
}

fn read_state(home: &Path) -> State {
    serde_json::from_str(&std::fs::read_to_string(home.join("state.json")).expect("read state"))
        .expect("decode state")
}

struct BareRepository {
    bare: PathBuf,
    seed: PathBuf,
}

fn create_bare_repository(root: &Path) -> BareRepository {
    let bare = root.join("source.git");
    let seed = root.join("seed");
    run_git(root, &["init", "--bare", path_text(&bare)]);
    run_git(root, &["init", path_text(&seed)]);
    run_git(&seed, &["config", "user.email", "fleet@example.test"]);
    run_git(&seed, &["config", "user.name", "Fleet Test"]);
    std::fs::write(seed.join("README.md"), "remote context fixture\n")
        .expect("write repository fixture");
    run_git(&seed, &["add", "README.md"]);
    run_git(&seed, &["commit", "-m", "initial"]);
    run_git(&seed, &["branch", "-M", "main"]);
    run_git(&seed, &["remote", "add", "origin", path_text(&bare)]);
    run_git(&seed, &["push", "-u", "origin", "main"]);
    run_git(
        root,
        &[
            "--git-dir",
            path_text(&bare),
            "symbolic-ref",
            "HEAD",
            "refs/heads/main",
        ],
    );
    BareRepository { bare, seed }
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("fixture path must be UTF-8")
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
