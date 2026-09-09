use std::{future::Future, path::Path, time::Duration};

use fleet_client::{Client, TerminalUpdate, ensure_daemon};
use fleet_core::{
    board::{
        BackendDescriptor, BackendRef, BackendSchema, BoardPatch, BoardView, Card, CardDraft,
        CardPatch, ConflictResolution, new_board, summarize,
    },
    ids::TerminalId,
    model::{Context, Worktree},
};
use fleet_proto::{
    PROTOCOL_VERSION,
    codec::FleetCodec,
    error::{ErrorKind, ProtoError},
    event::{Event, EventKind, ToastLevel},
    job::{JobKind, JobRecord, JobStatus},
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
    terminal::{
        Cell, CellAttrs, CellWidth, Color, CursorShape, CursorState, FrameUpdate, RowUpdate,
        TerminalModes, ViewportInfo,
    },
};
use futures_util::{SinkExt, StreamExt};
use smol_str::SmolStr;
use tempfile::TempDir;
use tokio::{net::UnixListener, sync::oneshot, time::timeout};
use tokio_util::codec::Framed;

#[tokio::test]
async fn wheel_and_shortcut_are_enqueued_between_keys_without_waiting_for_a_response() {
    use fleet_proto::terminal::{Modifiers, WheelEvent};
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let (done, release) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport, None).await;
        let first = transport.next().await.unwrap().unwrap();
        assert!(matches!(first.body, RequestBody::TerminalInput { bytes, .. } if bytes == b"a"));
        let second = transport.next().await.unwrap().unwrap();
        assert!(
            matches!(second.body, RequestBody::WheelTerminal { wheel, .. } if wheel.steps == -3)
        );
        let third = transport.next().await.unwrap().unwrap();
        assert!(matches!(
            third.body,
            RequestBody::ScrollOrKeyTerminal {
                scroll: fleet_proto::terminal::ScrollCommand::Top,
                ..
            }
        ));
        let fourth = transport.next().await.unwrap().unwrap();
        assert!(matches!(fourth.body, RequestBody::TerminalInput { bytes, .. } if bytes == b"b"));
        // Deliberately send no acknowledgements while the caller enqueues all four.
        release.await.unwrap();
    });
    let client = Client::connect(home.path()).await.unwrap();
    timeout(Duration::from_secs(1), async {
        client
            .request_background(RequestBody::TerminalInput {
                terminal: TerminalId(7),
                bytes: b"a".to_vec(),
            })
            .await
            .unwrap();
        client
            .wheel_terminal(
                TerminalId(7),
                WheelEvent {
                    steps: -3,
                    col: 4,
                    row: 5,
                    mods: Modifiers::empty(),
                },
            )
            .await
            .unwrap();
        client
            .scroll_or_key_terminal(
                TerminalId(7),
                fleet_proto::terminal::ScrollCommand::Top,
                fleet_proto::terminal::KeyEvent {
                    key: fleet_proto::terminal::Key::Home,
                    mods: Modifiers::SUPER,
                    text: None,
                    action: fleet_proto::terminal::KeyAction::Press,
                },
            )
            .await
            .unwrap();
        client
            .request_background(RequestBody::TerminalInput {
                terminal: TerminalId(7),
                bytes: b"b".to_vec(),
            })
            .await
            .unwrap();
    })
    .await
    .unwrap();
    done.send(()).unwrap();
    server.await.unwrap();
}

type ServerTransport = Framed<tokio::net::UnixStream, FleetCodec<serde_json::Value, Request>>;

#[tokio::test]
async fn negotiates_correlates_events_and_streams_terminal_frames() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport, None).await;

        let first = transport.next().await.unwrap().unwrap();
        let second = transport.next().await.unwrap().unwrap();
        for request in [&second, &first] {
            let body = match request.body {
                RequestBody::DaemonPing => ResponseBody::Pong,
                RequestBody::DaemonVersion => ResponseBody::Version {
                    version: "test-daemon".to_owned(),
                    protocol: PROTOCOL_VERSION,
                },
                ref other => panic!("unexpected correlated request: {other:?}"),
            };
            send_response(&mut transport, request.id, body).await;
        }
        send_event(
            &mut transport,
            Event::Toast {
                level: ToastLevel::Info,
                message: "ready".to_owned(),
            },
        )
        .await;

        let attach = transport.next().await.unwrap().unwrap();
        assert!(matches!(
            attach.body,
            RequestBody::AttachTerminal {
                terminal: TerminalId(7),
                cols: 100,
                rows: 30
            }
        ));
        send_event(&mut transport, Event::TerminalFrame(frame(7, 1, false))).await;
        send_event(&mut transport, Event::TerminalFrame(frame(7, 2, true))).await;
        send_response(&mut transport, attach.id, ResponseBody::Ack).await;

        let input = transport.next().await.unwrap().unwrap();
        assert!(matches!(
            input.body,
            RequestBody::TerminalInput {
                terminal: TerminalId(7),
                ref bytes
            } if bytes == b"ls\n"
        ));
        send_response(&mut transport, input.id, ResponseBody::Ack).await;
        send_event(
            &mut transport,
            Event::TerminalTitle {
                terminal: TerminalId(7),
                title: "editor".to_owned(),
            },
        )
        .await;
        send_event(
            &mut transport,
            Event::TerminalExited {
                terminal: TerminalId(7),
                code: None,
            },
        )
        .await;

        let detach = timeout(Duration::from_secs(2), transport.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(
            detach.body,
            RequestBody::DetachTerminal {
                terminal: TerminalId(7)
            }
        ));
        send_response(&mut transport, detach.id, ResponseBody::Ack).await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    let mut events = client.events();
    let (ping, version) = tokio::join!(client.daemon_ping(), client.daemon_version());
    ping.unwrap();
    let version = version.unwrap();
    assert_eq!(version.version, "test-daemon");
    assert_eq!(version.protocol, PROTOCOL_VERSION);
    assert_eq!(
        events.recv().await.unwrap(),
        Event::Toast {
            level: ToastLevel::Info,
            message: "ready".to_owned()
        }
    );

    let mut terminal = client.attach(TerminalId(7), 100, 30).await.unwrap();
    let Some(TerminalUpdate::Frame(first_frame)) = terminal.next_update().await else {
        panic!("expected terminal frame");
    };
    assert!(first_frame.full);
    assert_eq!(first_frame.seq, 2);
    terminal.send_input(b"ls\n".to_vec()).await.unwrap();
    assert_eq!(
        terminal.next_update().await,
        Some(TerminalUpdate::Title("editor".to_owned()))
    );
    assert_eq!(
        terminal.next_update().await,
        Some(TerminalUpdate::Exited(None))
    );
    drop(terminal);

    server.await.unwrap();
}

#[tokio::test]
async fn reconnects_and_restores_event_subscription() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let (disconnected_tx, disconnected_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (first_socket, _) = listener.accept().await.unwrap();
        let mut first = Framed::new(first_socket, FleetCodec::new());
        authenticate(&mut first, None).await;
        let subscribe = first.next().await.unwrap().unwrap();
        assert_eq!(
            subscribe.body,
            RequestBody::Subscribe {
                events: vec![EventKind::Toast, EventKind::TerminalFrame]
            }
        );
        send_response(&mut first, subscribe.id, ResponseBody::Ack).await;
        let attach = first.next().await.unwrap().unwrap();
        assert!(matches!(
            attach.body,
            RequestBody::AttachTerminal {
                terminal: TerminalId(11),
                cols: 80,
                rows: 24
            }
        ));
        send_event(&mut first, Event::TerminalFrame(frame(11, 1, true))).await;
        send_response(&mut first, attach.id, ResponseBody::Ack).await;
        drop(first);
        disconnected_tx.send(()).unwrap();

        let (second_socket, _) = listener.accept().await.unwrap();
        let mut second = Framed::new(second_socket, FleetCodec::new());
        authenticate(
            &mut second,
            Some(vec![EventKind::Toast, EventKind::TerminalFrame]),
        )
        .await;
        let reattach = second.next().await.unwrap().unwrap();
        assert!(matches!(
            reattach.body,
            RequestBody::AttachTerminal {
                terminal: TerminalId(11),
                cols: 80,
                rows: 24
            }
        ));
        send_event(&mut second, Event::TerminalFrame(frame(11, 2, true))).await;
        send_response(&mut second, reattach.id, ResponseBody::Ack).await;
        let ping = second.next().await.unwrap().unwrap();
        assert!(matches!(ping.body, RequestBody::DaemonPing));
        send_response(&mut second, ping.id, ResponseBody::Pong).await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    client
        .subscribe(vec![EventKind::Toast, EventKind::TerminalFrame])
        .await
        .unwrap();
    let mut terminal = client.attach(TerminalId(11), 80, 24).await.unwrap();
    let Some(TerminalUpdate::Frame(frame)) = terminal.next_update().await else {
        panic!("expected terminal frame");
    };
    assert_eq!(frame.seq, 1);
    disconnected_rx.await.unwrap();
    assert_eq!(
        timeout(Duration::from_secs(2), terminal.next_update())
            .await
            .unwrap()
            .and_then(|update| match update {
                TerminalUpdate::Frame(frame) => Some(frame.seq),
                TerminalUpdate::Exited(_) | TerminalUpdate::Title(_) => None,
            }),
        Some(2)
    );
    client.daemon_ping().await.unwrap();

    server.await.unwrap();
}

#[tokio::test]
async fn daemon_shutdown_event_stops_reconnection_and_fails_pending_requests() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport, None).await;
        let ping = transport.next().await.unwrap().unwrap();
        assert!(matches!(ping.body, RequestBody::DaemonPing));
        send_event(&mut transport, Event::DaemonShuttingDown).await;
    });

    let client = Client::connect(home.path()).await.unwrap();
    let mut events = client.events();
    let error = client.daemon_ping().await.unwrap_err();
    assert!(error.message.contains("shutting down"));
    assert_eq!(events.recv().await.unwrap(), Event::DaemonShuttingDown);

    server.await.unwrap();
}

#[tokio::test]
async fn ensure_daemon_reuses_a_healthy_daemon() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport, None).await;
        let ping = transport.next().await.unwrap().unwrap();
        assert!(matches!(ping.body, RequestBody::DaemonPing));
        send_response(&mut transport, ping.id, ResponseBody::Pong).await;
    });

    let client = ensure_daemon(home.path(), None).await.unwrap();
    drop(client);
    server.await.unwrap();
}

#[tokio::test]
async fn board_api_round_trips_over_the_unix_socket() {
    timeout(Duration::from_secs(10), board_api_round_trips())
        .await
        .expect("board API round trip");
}

async fn board_api_round_trips() {
    let home = TempDir::new().unwrap();
    let listener = bind(home.path()).await;
    let server = async {
        let (socket, _) = listener.accept().await.unwrap();
        let mut transport = Framed::new(socket, FleetCodec::new());
        authenticate(&mut transport, None).await;
        transport
    };
    let (client, mut transport) = tokio::join!(Client::connect(home.path()), server);
    let client = client.unwrap();
    let context = Context {
        id: "work".parse().unwrap(),
        name: "Work".into(),
        owners: vec![],
        created_at: "now".into(),
    };
    let board = new_board(&context, "now");
    let card: Card = serde_json::from_value(serde_json::json!({
        "id":"card-1", "boardId":"work", "number":1, "title":"Task", "statusId":"todo",
        "createdAt":"now", "updatedAt":"now"
    }))
    .unwrap();
    let view = BoardView {
        board: board.clone(),
        cards: vec![card.clone()],
    };
    let summary = summarize(&board, &view.cards);
    let worktree: Worktree = serde_json::from_value(serde_json::json!({
        "id":"acme/api#task", "repoId":"acme/api", "slug":"task", "branch":"task",
        "baseRef":"main", "path":"/tmp/task", "session":"task", "createdAt":"now"
    }))
    .unwrap();
    let job = JobRecord {
        id: "sync-job".parse().unwrap(),
        kind: JobKind::Custom("board.sync".into()),
        target: board.id.to_string(),
        title: "Sync board".into(),
        status: JobStatus::Queued,
        progress: None,
        log_path: "/tmp/sync.log".into(),
        started_at: "now".into(),
        finished_at: None,
        cancellable: true,
        retryable: false,
    };
    // Each operation is checked at the socket boundary, including every argument and typed result.
    macro_rules! check {
        ($request:expr, $response:expr, $operation:expr, $expected:expr) => {
            assert_eq!(
                exchange(&mut transport, $request, Ok($response), $operation)
                    .await
                    .unwrap(),
                $expected
            );
        };
    }
    check!(
        RequestBody::ListBoards {
            context_id: Some(context.id.clone())
        },
        ResponseBody::Boards(vec![summary.clone()]),
        client.list_boards(Some(context.id.clone())),
        vec![summary]
    );
    check!(
        RequestBody::GetBoard {
            board_id: board.id.clone()
        },
        ResponseBody::Board(view.clone()),
        client.get_board(board.id.clone()),
        view
    );
    check!(
        RequestBody::EnsureBoard {
            context_id: context.id.clone()
        },
        ResponseBody::Board(view.clone()),
        client.ensure_board(context.id.clone()),
        view
    );
    check!(
        RequestBody::CreateBoard {
            context_id: context.id.clone(),
            name: Some("Team".into()),
            prefix: Some("TM".into()),
            backend: Some(BackendRef::default())
        },
        ResponseBody::Board(view.clone()),
        client.create_board(
            context.id.clone(),
            Some("Team".into()),
            Some("TM".into()),
            Some(BackendRef::default())
        ),
        view
    );
    let board_patch = BoardPatch {
        default_repo_id: Some(None),
        ..Default::default()
    };
    check!(
        RequestBody::UpdateBoard {
            board_id: board.id.clone(),
            patch: board_patch.clone()
        },
        ResponseBody::Board(view.clone()),
        client.update_board(board.id.clone(), board_patch),
        view
    );
    let draft = CardDraft {
        title: "Task".into(),
        ..Default::default()
    };
    check!(
        RequestBody::CreateCard {
            board_id: board.id.clone(),
            draft: draft.clone()
        },
        ResponseBody::Card(card.clone()),
        client.create_card(board.id.clone(), draft),
        card
    );
    let patch = CardPatch {
        assignee: Some(None),
        estimate: Some(Some(3)),
        ..Default::default()
    };
    check!(
        RequestBody::UpdateCard {
            card_id: card.id.clone(),
            patch: patch.clone()
        },
        ResponseBody::Card(card.clone()),
        client.update_card(card.id.clone(), patch),
        card
    );
    check!(
        RequestBody::MoveCard {
            card_id: card.id.clone(),
            status_id: card.status_id.clone(),
            index: Some(2)
        },
        ResponseBody::Card(card.clone()),
        client.move_card(card.id.clone(), card.status_id.clone(), Some(2)),
        card
    );
    check!(
        RequestBody::AddCardComment {
            card_id: card.id.clone(),
            body: "Hello".into()
        },
        ResponseBody::Card(card.clone()),
        client.add_card_comment(card.id.clone(), "Hello".into()),
        card
    );
    check!(
        RequestBody::CreateWorktreeFromCard {
            card_id: card.id.clone(),
            repo_id: Some(worktree.repo_id.clone()),
            base: Some("main".into()),
            host: Some("devbox".parse().unwrap())
        },
        ResponseBody::CardWorktree {
            card: card.clone(),
            worktree: worktree.clone(),
            created: true
        },
        client.create_worktree_from_card(
            card.id.clone(),
            Some(worktree.repo_id.clone()),
            Some("main".into()),
            Some("devbox".parse().unwrap())
        ),
        (card.clone(), worktree, true)
    );
    check!(
        RequestBody::SyncBoard {
            board_id: board.id.clone(),
            full: false
        },
        ResponseBody::Job(job.clone()),
        client.sync_board(board.id.clone(), false),
        job.id
    );
    check!(
        RequestBody::SyncBoard {
            board_id: board.id.clone(),
            full: true
        },
        ResponseBody::Job(job.clone()),
        client.sync_board(board.id.clone(), true),
        job.id
    );
    let descriptors = vec![BackendDescriptor {
        kind: "local".into(),
        label: "Local".into(),
        capabilities: Default::default(),
        settings_schema: vec![],
    }];
    check!(
        RequestBody::ListBoardBackends {},
        ResponseBody::BoardBackends(descriptors.clone()),
        client.list_board_backends(),
        descriptors
    );
    check!(
        RequestBody::ResolveCardConflict {
            card_id: card.id.clone(),
            resolution: ConflictResolution::TakeRemote
        },
        ResponseBody::Card(card.clone()),
        client.resolve_card_conflict(card.id.clone(), ConflictResolution::TakeRemote),
        card
    );
    let schema = BackendSchema {
        key_prefix: Some("EXT".into()),
        ..Default::default()
    };
    check!(
        RequestBody::DescribeBoardBackend {
            board_id: board.id.clone()
        },
        ResponseBody::BoardBackendSchema(schema.clone()),
        client.describe_board_backend(board.id.clone()),
        schema
    );
    check!(
        RequestBody::DeleteCard {
            card_id: card.id.clone()
        },
        ResponseBody::Ack,
        client.delete_card(card.id.clone()),
        ()
    );
    check!(
        RequestBody::DeleteBoard {
            board_id: board.id.clone()
        },
        ResponseBody::Ack,
        client.delete_board(board.id.clone()),
        ()
    );

    let error = ProtoError {
        kind: ErrorKind::NotFound,
        message: "board missing".into(),
    };
    assert_eq!(
        exchange(
            &mut transport,
            RequestBody::GetBoard {
                board_id: board.id.clone()
            },
            Err(error.clone()),
            client.get_board(board.id.clone())
        )
        .await
        .unwrap_err(),
        error
    );
    let unexpected = exchange(
        &mut transport,
        RequestBody::SyncBoard {
            board_id: board.id.clone(),
            full: false,
        },
        Ok(ResponseBody::Ack),
        client.sync_board(board.id, false),
    )
    .await
    .unwrap_err();
    assert_eq!(unexpected.kind, ErrorKind::Unknown);
    assert!(unexpected.message.contains("sync_board"));
}

/// Answers the next request with `result` while `operation` runs, asserting what was sent.
async fn exchange<T>(
    transport: &mut ServerTransport,
    expected: RequestBody,
    result: Result<ResponseBody, ProtoError>,
    operation: impl Future<Output = fleet_client::Result<T>>,
) -> fleet_client::Result<T> {
    let server = async {
        let request = transport.next().await.unwrap().unwrap();
        assert_eq!(request.body, expected);
        transport
            .send(
                serde_json::to_value(Response {
                    id: request.id,
                    result,
                })
                .unwrap(),
            )
            .await
            .unwrap();
    };
    let (_, result) = tokio::join!(server, operation);
    result
}

async fn bind(home: &Path) -> UnixListener {
    UnixListener::bind(home.join("fleetd.sock")).unwrap()
}

async fn authenticate(
    transport: &mut ServerTransport,
    expected_subscription: Option<Vec<EventKind>>,
) {
    let hello = transport.next().await.unwrap().unwrap();
    assert!(matches!(
        hello.body,
        RequestBody::Hello {
            protocol: PROTOCOL_VERSION,
            ..
        }
    ));
    send_response(
        transport,
        hello.id,
        ResponseBody::Hello {
            protocol: PROTOCOL_VERSION,
            server: "test-daemon".to_owned(),
        },
    )
    .await;

    let subscribe = transport.next().await.unwrap().unwrap();
    let RequestBody::Subscribe { events } = subscribe.body else {
        panic!("expected initial subscription");
    };
    if let Some(expected) = expected_subscription {
        assert_eq!(events, expected);
    } else {
        assert_eq!(events.len(), 18);
        for kind in [
            EventKind::Agent,
            EventKind::AgentSummary,
            EventKind::WatchStarted,
            EventKind::WatchOutput,
            EventKind::WatchExited,
            EventKind::WatchDismissed,
            EventKind::AgentActivityChanged,
            EventKind::BoardChanged,
            EventKind::HostLinkChanged,
            EventKind::TerminalReattach,
        ] {
            assert!(events.contains(&kind));
        }
    }
    send_response(transport, subscribe.id, ResponseBody::Ack).await;
}

async fn send_response(transport: &mut ServerTransport, id: u64, body: ResponseBody) {
    transport
        .send(
            serde_json::to_value(Response {
                id,
                result: Ok(body),
            })
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn send_event(transport: &mut ServerTransport, event: Event) {
    transport
        .send(serde_json::to_value(event).unwrap())
        .await
        .unwrap();
}

fn frame(terminal: u64, seq: u64, full: bool) -> FrameUpdate {
    FrameUpdate {
        terminal: TerminalId(terminal),
        seq,
        cols: 100,
        rows: 30,
        full,
        shift: None,
        rows_changed: vec![RowUpdate {
            index: 0,
            cells: vec![Cell {
                text: SmolStr::new("$"),
                fg: Color::Default,
                bg: Color::Default,
                underline_color: None,
                attrs: CellAttrs::empty(),
                width: CellWidth::Narrow,
            }],
            wrapped: false,
        }],
        cursor: CursorState {
            row: 0,
            col: 1,
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
