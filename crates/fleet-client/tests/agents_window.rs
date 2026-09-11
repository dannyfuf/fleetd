//! The windowed agent open, end to end over a real socket: the capability gate, the bounded
//! response, and the two stream-control events that repair a subscription.
//!
//! Every wait here has a deadline and nothing sleeps: the fake daemon drives the clock by
//! answering, so a hang fails the test instead of slowing it down.

use std::{path::Path, time::Duration};

use fleet_client::{AgentSnapshot, AgentWindowRequest, Client, MirrorOutcome};
use fleet_core::{
    agents::{
        AgentKind, AgentThreadSummary, Attention, Seq, SessionState, ThreadId, ThreadProjection,
        TurnState,
    },
    ids::WorktreeId,
};
use fleet_proto::{
    AGENT_RESYNC_CAPABILITY, AGENT_SYNC_MARKER_CAPABILITY, AGENT_WINDOW_CAPABILITY,
    PROTOCOL_VERSION,
    agents::{AgentSessionView, AgentThreadWindow, TranscriptPage, TranscriptWindow},
    codec::FleetCodec,
    error::ErrorKind,
    event::Event,
    request::{Request, RequestBody},
    response::{HelloResponse, Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt};
use tempfile::TempDir;
use tokio::{net::UnixListener, time::timeout};
use tokio_util::codec::Framed;

type ServerTransport = Framed<tokio::net::UnixStream, FleetCodec<serde_json::Value, Request>>;

const BUDGET: Duration = Duration::from_secs(2);

#[tokio::test]
async fn a_windowed_open_returns_a_bounded_window_and_its_page_cursor() {
    let home = TempDir::new().expect("home");
    let listener = bind(home.path()).await;
    let thread = thread_id();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept");
        let mut transport = Framed::new(socket, FleetCodec::new());
        handshake(&mut transport, &[AGENT_WINDOW_CAPABILITY]).await;

        let open = next(&mut transport).await;
        let RequestBody::AgentThreadOpen {
            thread: opened,
            from_seq,
            after_seq,
            turn_limit,
            before_cursor,
            request_sync_marker,
        } = open.body
        else {
            panic!("expected a windowed open");
        };
        assert_eq!(opened, thread);
        assert_eq!(
            from_seq, None,
            "a windowing client never sends both cursors"
        );
        assert_eq!(after_seq, None);
        assert_eq!(turn_limit, Some(10));
        assert_eq!(before_cursor, None);
        assert!(request_sync_marker);

        reply(
            &mut transport,
            open.id,
            ResponseBody::AgentThreadWindow(Box::new(window(thread, Seq(12), Seq(12), true))),
        )
        .await;
    });

    let client = Client::connect(home.path()).await.expect("connect");
    let answered = timeout(
        BUDGET,
        client.agent_thread_open_window(thread, AgentWindowRequest::newest(10)),
    )
    .await
    .expect("the open answers inside its budget")
    .expect("a windowed open");

    assert_eq!(answered.head_seq, Seq(12));
    assert_eq!(answered.window.items.len(), 1);
    assert_eq!(
        answered
            .page
            .as_ref()
            .and_then(|page| page.before_cursor.as_deref()),
        Some("fat.1.cursor.4")
    );
    assert!(
        answered
            .fits_wire_budget()
            .expect("a window response is measurable"),
        "every window response is asserted against the 2 MiB budget"
    );
    server.await.expect("server");
}

#[tokio::test]
async fn a_window_field_is_never_sent_to_a_daemon_without_the_capability() {
    let home = TempDir::new().expect("home");
    let listener = bind(home.path()).await;
    let thread = thread_id();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept");
        let mut transport = Framed::new(socket, FleetCodec::new());
        handshake(&mut transport, &[]).await;

        // The very next frame is the *legacy* open the test sends afterwards. If the refused
        // windowed open had gone out anyway, this assertion is what catches it — an older daemon
        // ignores the unknown fields and answers the whole transcript, which above the frame
        // ceiling is a payload the client can never decode.
        let open = next(&mut transport).await;
        assert!(matches!(
            open.body,
            RequestBody::AgentThreadOpen {
                from_seq: Some(Seq(0)),
                after_seq: None,
                turn_limit: None,
                before_cursor: None,
                request_sync_marker: false,
                ..
            }
        ));
        reply(
            &mut transport,
            open.id,
            ResponseBody::AgentThreadSnapshot {
                projection: projection(thread),
                events_after: Vec::new(),
            },
        )
        .await;
    });

    let client = Client::connect(home.path()).await.expect("connect");
    let refused = timeout(
        BUDGET,
        client.agent_thread_open_window(thread, AgentWindowRequest::newest(10)),
    )
    .await
    .expect("a refusal is local and immediate")
    .expect_err("a window field needs the capability");

    assert_eq!(refused.kind, ErrorKind::Unsupported);
    assert!(
        refused.message.contains(AGENT_WINDOW_CAPABILITY),
        "the refusal names the capability so a client need not match on prose: {}",
        refused.message
    );

    timeout(BUDGET, client.agent_thread_open(thread, Some(Seq(0))))
        .await
        .expect("the legacy open answers")
        .expect("legacy snapshot");
    server.await.expect("server");
}

#[tokio::test]
async fn a_budget_overflow_resyncs_the_thread_from_the_cursor_the_daemon_named() {
    let home = TempDir::new().expect("home");
    let listener = bind(home.path()).await;
    let thread = thread_id();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept");
        let mut transport = Framed::new(socket, FleetCodec::new());
        handshake(
            &mut transport,
            &[
                AGENT_WINDOW_CAPABILITY,
                AGENT_RESYNC_CAPABILITY,
                AGENT_SYNC_MARKER_CAPABILITY,
            ],
        )
        .await;

        // The daemon dropped everything after sequence 5 for this connection only.
        send(
            &mut transport,
            serde_json::to_value(Event::AgentResync {
                thread,
                from_seq: Seq(5),
            })
            .expect("resync event"),
        )
        .await;

        let reopen = next(&mut transport).await;
        let RequestBody::AgentThreadOpen { after_seq, .. } = reopen.body else {
            panic!("a resync re-opens the thread");
        };
        assert_eq!(
            after_seq,
            Some(Seq(0)),
            "the mirror had applied nothing, and the smaller of the two cursors is the safe one"
        );
        reply(
            &mut transport,
            reopen.id,
            ResponseBody::AgentThreadWindow(Box::new(window(thread, Seq(0), Seq(0), false))),
        )
        .await;

        // Catch-up is complete, then one live event the subscription must deliver.
        send(
            &mut transport,
            serde_json::to_value(Event::AgentSynchronized { thread }).expect("marker"),
        )
        .await;
        send(
            &mut transport,
            serde_json::to_value(Event::Agent {
                thread,
                event: notice(Seq(1)),
            })
            .expect("agent event"),
        )
        .await;
    });

    let client = Client::connect(home.path()).await.expect("connect");
    let mut events = client.agent_events();
    let (delivered, event) = timeout(BUDGET, events.recv())
        .await
        .expect("the repaired subscription delivers")
        .expect("a live event after the resync");

    assert_eq!(delivered, thread);
    assert_eq!(event.seq, Seq(1));
    assert!(
        events.mirror().is_synchronized(thread),
        "the marker is the only transition into live, and it landed"
    );
    assert_eq!(events.mirror().applied_seq(thread), Seq(1));
    server.await.expect("server");
}

#[tokio::test]
async fn an_installed_snapshot_still_reports_the_four_way_outcome() {
    // The window path must not cost the mirror its distinctions: a replay is not a hole, and a
    // refusal is not something a round trip can repair.
    let home = TempDir::new().expect("home");
    let listener = bind(home.path()).await;
    let thread = thread_id();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept");
        let mut transport = Framed::new(socket, FleetCodec::new());
        handshake(&mut transport, &[AGENT_WINDOW_CAPABILITY]).await;
    });

    let client = Client::connect(home.path()).await.expect("connect");
    let mut events = client.agent_events();
    assert_eq!(
        events.install(AgentSnapshot {
            projection: projection(thread),
            events_after: vec![notice(Seq(1))],
        }),
        MirrorOutcome::Applied
    );
    assert_eq!(events.mirror().applied_seq(thread), Seq(1));
    server.await.expect("server");
}

#[tokio::test]
async fn a_sequence_gap_is_repaired_through_the_window_not_the_unbounded_snapshot() {
    // A cursored legacy open bounds only the event tail: its `projection` is the whole thread
    // whatever the cursor says. On a transcript large enough to matter that repair is the exact
    // frame the daemon has to refuse, so a windowing daemon must be asked for a window.
    let home = TempDir::new().expect("home");
    let listener = bind(home.path()).await;
    let thread = thread_id();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept");
        let mut transport = Framed::new(socket, FleetCodec::new());
        handshake(&mut transport, &[AGENT_WINDOW_CAPABILITY]).await;

        // Sequence 3 arrives while the client has applied only 1.
        send(
            &mut transport,
            serde_json::to_value(Event::Agent {
                thread,
                event: notice(Seq(3)),
            })
            .expect("agent event"),
        )
        .await;

        let repair = next(&mut transport).await;
        let RequestBody::AgentThreadOpen {
            after_seq,
            from_seq,
            request_sync_marker,
            ..
        } = repair.body
        else {
            panic!("a gap repairs through an open");
        };
        assert_eq!(
            after_seq,
            Some(Seq(1)),
            "the repair resumes from what applied"
        );
        assert_eq!(from_seq, None);
        assert!(request_sync_marker);

        let mut window = window(thread, Seq(1), Seq(1), true);
        window.events_after = vec![notice(Seq(2)), notice(Seq(3))];
        reply(
            &mut transport,
            repair.id,
            ResponseBody::AgentThreadWindow(Box::new(window)),
        )
        .await;
    });

    let client = Client::connect(home.path()).await.expect("connect");
    let mut events = client.agent_events();
    assert_eq!(
        events.install(AgentSnapshot {
            projection: projection(thread),
            events_after: vec![notice(Seq(1))],
        }),
        MirrorOutcome::Applied
    );

    let (delivered, event) = timeout(BUDGET, events.recv())
        .await
        .expect("the gap is repaired inside its budget")
        .expect("the event that gapped is delivered");

    assert_eq!(delivered, thread);
    assert_eq!(event.seq, Seq(3));
    assert_eq!(
        events.mirror().applied_seq(thread),
        Seq(3),
        "the repair closed the hole rather than leaving a stale projection"
    );
    server.await.expect("server");
}

async fn bind(home: &Path) -> UnixListener {
    UnixListener::bind(home.join("fleetd.sock")).expect("bind")
}

/// Answers Hello with a capability-bearing envelope, then the initial subscription.
async fn handshake(transport: &mut ServerTransport, capabilities: &[&str]) {
    let hello = next(transport).await;
    assert!(matches!(
        hello.body,
        RequestBody::Hello {
            protocol: PROTOCOL_VERSION,
            ..
        }
    ));
    let envelope = HelloResponse {
        response: Response {
            id: hello.id,
            result: Ok(ResponseBody::Hello {
                protocol: PROTOCOL_VERSION,
                server: "test-daemon".to_owned(),
            }),
        },
        capabilities: capabilities.iter().map(|name| (*name).to_owned()).collect(),
        daemon_id: "daemon-test".to_owned(),
        build_commit: None,
    };
    send(
        transport,
        serde_json::to_value(envelope).expect("hello envelope"),
    )
    .await;

    let subscribe = next(transport).await;
    assert!(matches!(subscribe.body, RequestBody::Subscribe { .. }));
    reply(transport, subscribe.id, ResponseBody::Ack).await;
}

async fn next(transport: &mut ServerTransport) -> Request {
    timeout(BUDGET, transport.next())
        .await
        .expect("a frame arrives inside its budget")
        .expect("stream open")
        .expect("decodable frame")
}

async fn reply(transport: &mut ServerTransport, id: u64, body: ResponseBody) {
    send(
        transport,
        serde_json::to_value(Response {
            id,
            result: Ok(body),
        })
        .expect("response"),
    )
    .await;
}

async fn send(transport: &mut ServerTransport, value: serde_json::Value) {
    transport.send(value).await.expect("send");
}

fn thread_id() -> ThreadId {
    "11111111-2222-4333-8444-555555555555"
        .parse()
        .expect("thread id")
}

fn worktree() -> WorktreeId {
    WorktreeId::try_from("acme/api#feature").expect("worktree")
}

fn projection(thread: ThreadId) -> ThreadProjection {
    ThreadProjection::new(thread, worktree(), AgentKind::Claude)
}

fn notice(seq: Seq) -> fleet_core::agents::SeqEvent {
    serde_json::from_value(serde_json::json!({
        "seq": seq,
        "at": "2026-09-07T12:00:00Z",
        "event": {"type": "notice", "data": "ready"},
    }))
    .expect("notice event")
}

fn window(
    thread: ThreadId,
    head_seq: Seq,
    projected_seq: Seq,
    synchronized: bool,
) -> AgentThreadWindow {
    AgentThreadWindow {
        summary: AgentThreadSummary {
            thread,
            worktree: worktree(),
            host: None,
            provider: AgentKind::Claude,
            title: "Claude".to_owned(),
            attention: Attention::Idle,
            session: SessionState::Ready,
            turn: TurnState::None,
            last_seq: head_seq,
            last_activity: None,
            last_completed_seq: None,
            last_nonterminal_seq: None,
            exit_code: None,
        },
        session: AgentSessionView::default(),
        window: TranscriptWindow {
            items: vec![
                serde_json::from_value(serde_json::json!({
                    "id": "bbbbbbbb-2222-4333-8444-555555555555",
                    "turn": "aaaaaaaa-2222-4333-8444-555555555555",
                    "kind": {"type": "assistant_text", "data": {"text": "hello"}},
                    "status": "completed",
                    "started": "2026-09-07T12:00:00Z",
                }))
                .expect("item"),
            ],
            ..TranscriptWindow::default()
        },
        page: Some(TranscriptPage {
            before_cursor: Some("fat.1.cursor.4".to_owned()),
            has_more: true,
            thread_seq: head_seq,
        }),
        head_seq,
        projected_seq,
        events_after: Vec::new(),
        synchronized,
    }
}
