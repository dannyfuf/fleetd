//! Two real daemons proving a worktree board answers from the daemon that owns the worktree.
//!
//! Before `docs/decisions/0021-hosted-worktree-boards-route-to-owner.md` every board request was
//! `Target::Local`, so the laptop answered `EnsureWorktreeBoard` for a worktree the dev box owns
//! by writing an empty `boards/wt-*.json` of its own and the user saw an empty board. This drives
//! the whole path over a real loopback link: the owner's board and cards come back untranslated,
//! a card moves from the far side and lands in the owner's document, the listing names the hosted
//! board once, and the stale local document the old build left behind is retired into the trash.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use fleet_core::{
    board::{
        BOARD_DOCUMENT_VERSION, BoardDocument, Card, CardDraft, create_card, new_worktree_board,
        worktree_board_id,
    },
    config::default_config,
    ids::{BoardId, CardId, ContextId, HostId, RepoId, WorktreeId},
    model::{Context, Repo, RepoHooks, Worktree},
    paths::FleetHome,
    state::default_state,
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
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const CONTEXT_ID: &str = "personal";
const REPO_ID: &str = "dannyfuf/hosted-boards";
const WORKTREE_ID: &str = "dannyfuf/hosted-boards#hosted";
/// The card the laptop's own document holds: it must survive in the trash, never in a board.
const STALE_CARD_TITLE: &str = "card the laptop board kept";
const FIXTURE_TIME: &str = "2026-09-09T00:00:00Z";
/// The warning `retire_hosted_worktree_board` logs when the document it trashes had cards.
const RETIREMENT_WARNING: &str = "retired this daemon's board for a worktree another host owns";

#[tokio::test]
async fn real_two_daemon_hosted_worktree_board_contract() {
    let fixture = tempfile::tempdir().expect("create two-daemon fixture");
    let source = create_bare_repository(fixture.path());
    let local_home = fixture.path().join("local");
    let remote_home = fixture.path().join("remote");
    let context = fixture_context();
    let repo = fixture_repo(&source);
    let worktree = fixture_worktree(fixture.path());
    let worktree_id = worktree.id.clone();
    let board_id = worktree_board_id(&worktree_id);

    // The owner has the worktree record; the laptop has the same context and repository and no
    // worktree, which is exactly the shape that used to make it answer the board request itself.
    write_config(&remote_home, None);
    write_state(
        &remote_home,
        Some(context.clone()),
        Some(repo.clone()),
        vec![worktree.clone()],
    );
    write_state(
        &local_home,
        Some(context.clone()),
        Some(repo.clone()),
        Vec::new(),
    );
    let stale = write_stale_board(&local_home, &context, &worktree);
    assert_eq!(
        stale.board.id, board_id,
        "the stale document must shadow the owner's board id"
    );

    let remote = infra::RemoteDaemon::start(&remote_home);
    infra::assert_remote_contract(&remote);
    let infra::RemoteDaemon {
        daemon: mut remote_process,
        machine: _remote_machine,
        config: remote_config,
    } = remote;

    let mut owner = DaemonClient::connect(&remote_home.join("fleetd.sock")).await;
    let ensured = owner
        .request(RequestBody::EnsureWorktreeBoard {
            worktree_id: worktree_id.clone(),
        })
        .await;
    let ResponseBody::Board(owner_view) = expect_body(ensured, "ensure the owner's worktree board")
    else {
        panic!("expected a board response from the owner");
    };
    assert_eq!(owner_view.board.id, board_id);
    let first = create_remote_card(&mut owner, &board_id, "first").await;
    let second = create_remote_card(&mut owner, &board_id, "second").await;

    let host = HostId::try_from("loopback").expect("loopback host id");
    write_config(&local_home, Some((host.clone(), remote_config)));
    let mut local_process = infra::DaemonProcess::start(&local_home);
    let mut local = DaemonClient::connect(&local_home.join("fleetd.sock")).await;

    let ensured = request_when_remote_ready(
        &mut local,
        RequestBody::EnsureWorktreeBoard {
            worktree_id: worktree_id.clone(),
        },
        &worktree_id,
    )
    .await;
    let ResponseBody::Board(view) = expect_body(ensured, "ensure the hosted worktree board") else {
        panic!("expected a board response from the local daemon");
    };
    assert_eq!(view.board.id, board_id);
    assert_eq!(view.board.worktree_id.as_ref(), Some(&worktree_id));
    assert_eq!(
        titles(&view.cards),
        vec!["first".to_owned(), "second".to_owned()],
        "the local daemon answered with its own document instead of the owner's"
    );
    assert_eq!(second.board_id, board_id);

    // A card id is globally unique and passes through untranslated, so the far side of the link
    // can be addressed by it alone: nothing in this request names the host.
    let status_id = view
        .board
        .statuses
        .iter()
        .map(|status| status.id.clone())
        .find(|status| *status != first.status_id)
        .expect("a default board has more than one status");
    let moved = local
        .request(RequestBody::MoveCard {
            card_id: first.id.clone(),
            status_id: status_id.clone(),
            index: None,
        })
        .await;
    let ResponseBody::Card(moved_card) = expect_body(moved, "move a hosted card") else {
        panic!("expected a card response from the local daemon");
    };
    assert_eq!(moved_card.id, first.id);
    assert_eq!(moved_card.status_id, status_id);
    let persisted = read_board_document(&remote_home, &board_id);
    let owned = persisted
        .cards
        .iter()
        .find(|card| card.id == first.id)
        .expect("the owner's document still holds the moved card");
    assert_eq!(
        owned.status_id, status_id,
        "the move was answered but never written on the owning host"
    );

    let listed = local
        .request(RequestBody::ListBoards { context_id: None })
        .await;
    let ResponseBody::Boards(boards) = expect_body(listed, "list boards") else {
        panic!("expected a board list from the local daemon");
    };
    let hosted = boards
        .iter()
        .filter(|summary| summary.id == board_id)
        .collect::<Vec<_>>();
    assert_eq!(
        hosted.len(),
        1,
        "the hosted board is listed once, not merged or duplicated: {boards:?}"
    );
    assert_eq!(hosted[0].card_count, 2);
    assert_eq!(hosted[0].worktree_id.as_ref(), Some(&worktree_id));

    let layout = FleetHome::new(&local_home);
    assert!(
        !layout.board_path(&board_id).exists(),
        "the stale local document still shadows the owner's board"
    );
    assert_eq!(
        json_files(&layout.boards_dir()),
        Vec::<PathBuf>::new(),
        "the local daemon kept a board document of its own"
    );
    let trashed = json_files(&layout.trash_dir());
    assert_eq!(
        trashed.len(),
        1,
        "retirement must leave exactly one recoverable document: {trashed:?}"
    );
    let retired: BoardDocument = read_json(&trashed[0]);
    assert_eq!(retired.board.id, board_id);
    assert_eq!(retired.board.worktree_id.as_ref(), Some(&worktree_id));
    assert_eq!(
        titles(&retired.cards),
        vec![STALE_CARD_TITLE.to_owned()],
        "the retired document is the laptop's, cards and all"
    );
    let log = await_retirement_warning(&local_home).await;
    assert!(
        log.contains("cards=1"),
        "the retirement warning must name the card count the user has to re-create: {log:?}"
    );

    shutdown(&mut local, &mut local_process).await;
    shutdown(&mut owner, &mut remote_process).await;
}

/// Requests until the local daemon both has its link and knows who owns the worktree.
///
/// Two answers mean "not yet". The link may still be starting, which is
/// `host loopback is unreachable` exactly as in `remote_create.rs`; and until the mirror carries
/// the host's snapshot nothing names the worktree's owner, so the request routes locally and the
/// owner-only scope rule refuses it with `not found: worktree <id>`.
async fn request_when_remote_ready(
    client: &mut DaemonClient,
    body: RequestBody,
    worktree: &WorktreeId,
) -> Response {
    let deadline = Instant::now() + READY_TIMEOUT;
    let unrouted = format!("worktree {worktree}");
    loop {
        let response = client.request(body.clone()).await;
        let routing_pending = response.result.as_ref().is_err_and(|error| {
            error.message.contains("host loopback is unreachable")
                || error.message.contains(&unrouted)
        });
        if !routing_pending {
            return response;
        }
        assert!(
            Instant::now() < deadline,
            "local daemon did not route the board to its owner: {response:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Reads the local daemon's log until the retirement warning appears.
///
/// The log writer is non-blocking, so the line the dispatch path emitted before forwarding is
/// visible a moment after the response it preceded.
async fn await_retirement_warning(home: &Path) -> String {
    let path = FleetHome::new(home).logs_dir().join("fleetd.log");
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        let log = strip_ansi(&std::fs::read_to_string(&path).unwrap_or_default());
        if log.contains(RETIREMENT_WARNING) {
            return log;
        }
        assert!(
            Instant::now() < deadline,
            "{} never recorded the retirement warning: {log:?}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Drops the ANSI escapes `fleetd` writes into its log file, so a field reads as `cards=1`.
///
/// The daemon tees one formatter to stderr and to `logs/fleetd.log`, and that formatter colours
/// both; every escape it emits is a CSI sequence ending in `m`.
fn strip_ansi(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('\u{1b}') {
        plain.push_str(&rest[..start]);
        let escape = &rest[start..];
        let Some(end) = escape.find('m') else {
            // A truncated escape at the end of a partially flushed read: nothing left to keep.
            return plain;
        };
        rest = &escape[end + 1..];
    }
    plain.push_str(rest);
    plain
}

async fn create_remote_card(client: &mut DaemonClient, board: &BoardId, title: &str) -> Card {
    let response = client
        .request(RequestBody::CreateCard {
            board_id: board.clone(),
            draft: CardDraft {
                title: title.to_owned(),
                ..Default::default()
            },
        })
        .await;
    let ResponseBody::Card(card) = expect_body(response, "create a card on the owner") else {
        panic!("expected a card response, got a different body for {title}");
    };
    card
}

fn expect_body(response: Response, what: &str) -> ResponseBody {
    response
        .result
        .unwrap_or_else(|error| panic!("failed to {what}: {}", error.message))
}

fn titles(cards: &[Card]) -> Vec<String> {
    cards.iter().map(|card| card.title.clone()).collect()
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
                    client_id: None,
                    kind: ClientKind::Cli,
                    host_id: None,
                    capabilities: Vec::new(),
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

fn fixture_context() -> Context {
    Context {
        id: ContextId::try_from(CONTEXT_ID).expect("context id"),
        name: "Personal".to_owned(),
        owners: vec!["dannyfuf".to_owned()],
        created_at: FIXTURE_TIME.to_owned(),
    }
}

fn fixture_repo(source: &BareRepository) -> Repo {
    Repo {
        id: RepoId::try_from(REPO_ID).expect("repo id"),
        owner: "dannyfuf".to_owned(),
        name: "hosted-boards".to_owned(),
        url: source.bare.display().to_string(),
        context_id: ContextId::try_from(CONTEXT_ID).expect("context id"),
        default_branch: "main".to_owned(),
        path: source.seed.display().to_string(),
        cloned_at: FIXTURE_TIME.to_owned(),
        hooks: RepoHooks::default(),
    }
}

/// The worktree the owning daemon publishes, checked out at a directory that exists.
fn fixture_worktree(root: &Path) -> Worktree {
    let id = WorktreeId::try_from(WORKTREE_ID).expect("worktree id");
    let path = root.join("hosted-tree");
    std::fs::create_dir_all(&path).expect("create the hosted worktree directory");
    Worktree {
        repo_id: RepoId::try_from(REPO_ID).expect("repo id"),
        slug: id.slug().to_owned(),
        id,
        branch: "hosted".to_owned(),
        base_ref: "origin/main".to_owned(),
        path: path.display().to_string(),
        session: "hosted-boards/hosted".to_owned(),
        host: None,
        created_at: FIXTURE_TIME.to_owned(),
        last_opened_at: None,
        degraded: None,
    }
}

/// Writes the document an older build left on the laptop: the owner's board id, with a card.
fn write_stale_board(home: &Path, context: &Context, worktree: &Worktree) -> BoardDocument {
    let layout = FleetHome::new(home);
    std::fs::create_dir_all(layout.boards_dir()).expect("create the local boards directory");
    let mut board = new_worktree_board(context, worktree, FIXTURE_TIME);
    let card = create_card(
        &mut board,
        &[],
        CardId::try_from(uuid::Uuid::new_v4().to_string()).expect("card id"),
        CardDraft {
            title: STALE_CARD_TITLE.to_owned(),
            ..Default::default()
        },
        FIXTURE_TIME,
    )
    .expect("build the stale card");
    let document = BoardDocument {
        version: BOARD_DOCUMENT_VERSION,
        board,
        cards: vec![card],
    };
    std::fs::write(
        layout.board_path(&document.board.id),
        serde_json::to_string_pretty(&document).expect("encode the stale board document"),
    )
    .expect("write the stale board document");
    document
}

fn read_board_document(home: &Path, id: &BoardId) -> BoardDocument {
    read_json(&FleetHome::new(home).board_path(id))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("decode {}: {error}", path.display()))
}

/// Every JSON document in a directory, sorted; an absent directory holds nothing.
fn json_files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths = entries
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
                .path()
        })
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
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

fn write_state(
    home: &Path,
    context: Option<Context>,
    repo: Option<Repo>,
    worktrees: Vec<Worktree>,
) {
    std::fs::create_dir_all(home).expect("create fleet home");
    let mut state = default_state();
    state.contexts.extend(context);
    state.repos.extend(repo);
    state.worktrees.extend(worktrees);
    std::fs::write(
        home.join("state.json"),
        serde_json::to_string_pretty(&state).expect("encode daemon state"),
    )
    .expect("write daemon state");
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
    std::fs::write(seed.join("README.md"), "hosted boards fixture\n")
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
