use std::{
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use fleet_core::ids::HostId;
use fleet_daemon::{
    DaemonError,
    machines::{
        AsyncDuplex, ExecOutput, LinkOptions, MachineAddress, MachineError, MachineProvider,
        ProbeReport, RemoteEndpoint, RemoteLink, WRITE_BUDGET,
    },
    testing::FakeMachine,
};
use fleet_proto::{
    PROTOCOL_VERSION,
    codec::FleetCodec,
    event::{Event, EventKind},
    media::CHUNK_BYTES,
    request::{ClientKind, MediaAnchor, Request, RequestBody, StageOp, UploadId},
    response::{HelloResponse, Response, ResponseBody},
    snapshot::{LinkState, Snapshot},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::{
    io::DuplexStream,
    sync::{broadcast, oneshot, watch},
    time::Instant,
};
use tokio_util::codec::Framed;

use crate::infra;

const BACKOFF_FLOOR: Duration = Duration::from_millis(100);
const TEN_MEGABITS_PER_SECOND: usize = 10_000_000 / 8;
const FIFTY_MIB_IN_CHUNKS: usize = 50 * 1024 * 1024 / CHUNK_BYTES;

#[tokio::test]
async fn link_handshakes_pings_and_reconnects_after_remote_restart() {
    let home = tempfile::tempdir().expect("remote home");
    let remote = infra::RemoteDaemon::start(home.path());
    wait_for_socket(home.path()).await;
    infra::assert_remote_contract(&remote);
    let _ = infra::DaemonProcess::wait;
    let infra::RemoteDaemon {
        daemon, machine, ..
    } = remote;
    let provider: Arc<dyn MachineProvider> = Arc::new(machine);
    let link = RemoteLink::new(
        provider,
        LinkOptions {
            backoff_min: BACKOFF_FLOOR,
            backoff_max: Duration::from_millis(400),
            hello_timeout: Duration::from_secs(5),
        },
    );
    let mut states = link.state_changes();
    let mut events = link.events();

    link.connect().await.expect("connect remote link");
    assert_eq!(link.state(), LinkState::Ready);
    wait_for_link_event(&mut events, LinkState::Ready).await;
    let hello = link.hello().expect("remote Hello metadata");
    assert!(hello.version.contains("fleetd"), "{}", hello.version);
    assert!(!hello.daemon_id.is_empty());
    let first_snapshot = link
        .last_snapshot_seen()
        .expect("snapshot fetched before Ready");
    assert!(matches!(
        link.request(RequestBody::DaemonPing).await,
        Ok(ResponseBody::Pong)
    ));

    drop(daemon);
    wait_for_state(&mut states, LinkState::Down).await;
    wait_for_link_event(&mut events, LinkState::Down).await;
    assert_eq!(link.last_snapshot_seen(), Some(first_snapshot));

    let daemon = infra::DaemonProcess::start(home.path());
    wait_for_state(&mut states, LinkState::Ready).await;
    wait_for_link_event(&mut events, LinkState::Ready).await;
    assert!(matches!(
        link.request(RequestBody::DaemonPing).await,
        Ok(ResponseBody::Pong)
    ));

    link.close().await;
    assert_eq!(link.state(), LinkState::Down);
    drop(daemon);
}

#[tokio::test]
async fn mid_request_transport_loss_fails_fast_and_close_interrupts_backoff() {
    let host = HostId::try_from("local-daemon").expect("scripted host id");
    let machine = Arc::new(FakeMachine::new(host));
    let provider: Arc<dyn MachineProvider> = machine.clone();
    let link = RemoteLink::new(
        provider,
        LinkOptions {
            backoff_min: Duration::from_secs(2),
            backoff_max: Duration::from_millis(250),
            hello_timeout: Duration::from_secs(1),
        },
    );
    let mut states = link.state_changes();
    let mut events = link.events();
    let connecting = {
        let link = Arc::clone(&link);
        tokio::spawn(async move { link.connect().await })
    };
    let peer = wait_for_fake_stream(&machine).await;
    let (mut remote, snapshot) = complete_scripted_handshake(peer).await;

    connecting
        .await
        .expect("connect task")
        .expect("scripted handshake");
    wait_for_link_event(&mut events, LinkState::Ready).await;
    assert_eq!(link.last_snapshot_seen(), Some(snapshot.clone()));

    let pending = {
        let link = Arc::clone(&link);
        tokio::spawn(async move { link.request(RequestBody::DaemonPing).await })
    };
    let request = next_request(&mut remote).await;
    assert_eq!(request.id, 3);
    assert!(matches!(request.body, RequestBody::DaemonPing));
    drop(remote);

    let error = tokio::time::timeout(BACKOFF_FLOOR, pending)
        .await
        .expect("request did not fail before the backoff floor")
        .expect("request task")
        .expect_err("request should fail when its transport closes");
    assert!(matches!(error, DaemonError::Remote(_)), "{error}");
    wait_for_state(&mut states, LinkState::Down).await;
    wait_for_link_event(&mut events, LinkState::Down).await;
    assert_eq!(link.last_snapshot_seen(), Some(snapshot));

    let retry_peer =
        tokio::time::timeout(Duration::from_millis(500), wait_for_fake_stream(&machine))
            .await
            .expect("backoff exceeded its configured maximum");
    wait_for_state(&mut states, LinkState::Connecting).await;
    let (reconnected_remote, reconnected_snapshot) = complete_scripted_handshake(retry_peer).await;
    wait_for_state(&mut states, LinkState::Ready).await;
    wait_for_link_event(&mut events, LinkState::Ready).await;
    assert_eq!(link.last_snapshot_seen(), Some(reconnected_snapshot));
    drop(reconnected_remote);
    wait_for_state(&mut states, LinkState::Down).await;
    wait_for_link_event(&mut events, LinkState::Down).await;
    tokio::time::timeout(BACKOFF_FLOOR, link.close())
        .await
        .expect("close should interrupt the bounded backoff");
}

#[tokio::test]
async fn link_recovers_when_connect_starts_before_the_remote_daemon() {
    let machine = Arc::new(AppearingMachine::new(
        HostId::try_from("local-daemon").expect("appearing host id"),
    ));
    let provider: Arc<dyn MachineProvider> = machine.clone();
    let link = RemoteLink::new(
        provider,
        LinkOptions {
            backoff_min: BACKOFF_FLOOR,
            backoff_max: Duration::from_millis(400),
            hello_timeout: Duration::from_secs(1),
        },
    );
    let mut states = link.state_changes();
    let mut events = link.events();

    let error = link
        .connect()
        .await
        .expect_err("initial connection should fail while the daemon is absent");
    assert!(matches!(error, DaemonError::Remote(_)), "{error}");
    assert_eq!(link.state(), LinkState::Down);
    let down_request = tokio::time::timeout(BACKOFF_FLOOR, link.request(RequestBody::DaemonPing))
        .await
        .expect("a request made while down should fail before the backoff floor");
    assert!(matches!(down_request, Err(DaemonError::Remote(_))));

    machine.set_available();
    let peer = wait_for_fake_stream(&machine.machine).await;
    let (mut remote, _) = complete_scripted_handshake(peer).await;
    wait_for_state(&mut states, LinkState::Ready).await;
    wait_for_link_event(&mut events, LinkState::Ready).await;
    assert!(link.last_snapshot_seen().is_some());
    let ping = {
        let link = Arc::clone(&link);
        tokio::spawn(async move { link.request(RequestBody::DaemonPing).await })
    };
    let request = next_request(&mut remote).await;
    assert!(matches!(request.body, RequestBody::DaemonPing));
    send_value(
        &mut remote,
        Response {
            id: request.id,
            result: Ok(ResponseBody::Pong),
        },
    )
    .await;
    assert!(matches!(
        ping.await.expect("ping task"),
        Ok(ResponseBody::Pong)
    ));

    link.close().await;
    drop(remote);
}

#[tokio::test(start_paused = true)]
async fn a_stalled_remote_write_fails_its_request_instead_of_wedging_the_link() {
    // `RemoteLink::new` identifies itself with the provider id, and the scripted handshake asserts
    // that id in the proxy Hello.
    let host = HostId::try_from("local-daemon").expect("scripted host id");
    let machine = Arc::new(FakeMachine::new(host));
    let provider: Arc<dyn MachineProvider> = machine.clone();
    let link = RemoteLink::new(
        provider,
        LinkOptions {
            backoff_min: Duration::from_millis(10),
            backoff_max: Duration::from_millis(50),
            hello_timeout: Duration::from_millis(50),
        },
    );
    let connecting = {
        let link = Arc::clone(&link);
        tokio::spawn(async move { link.connect().await })
    };
    let peer = wait_for_fake_stream(&machine).await;
    // The scripted peer answers the handshake and then stops reading, exactly as a remote daemon
    // does once it reaches MAX_PENDING_REQUESTS. Keeping `remote` alive fills the pipe instead of
    // closing it, so the link actor blocks inside its write.
    let (remote, _snapshot) = complete_scripted_handshake(peer).await;
    connecting
        .await
        .expect("connect task")
        .expect("scripted handshake");

    let mut wedged = Vec::new();
    for terminal in 0..8_u64 {
        let link = Arc::clone(&link);
        wedged.push(tokio::spawn(async move {
            link.request(RequestBody::TerminalInput {
                terminal: fleet_core::ids::TerminalId(terminal),
                bytes: vec![b'x'; 32 * 1024],
            })
            .await
        }));
    }

    let request = wedged.pop().expect("a wedged request");
    let failed = tokio::time::timeout(WRITE_BUDGET * 6, request)
        .await
        .expect("a stalled write must fail its pending request instead of wedging the actor")
        .expect("request task");
    assert!(matches!(failed, Err(DaemonError::Remote(_))), "{failed:?}");

    tokio::time::timeout(WRITE_BUDGET * 6, link.close())
        .await
        .expect("close must not hang behind a stalled write");
    drop(remote);
}

#[tokio::test(start_paused = true)]
async fn a_media_window_keeps_paste_latency_bounded_on_a_slow_link() {
    let host = HostId::try_from("local-daemon").expect("scripted host id");
    let machine = Arc::new(FakeMachine::new(host.clone()));
    machine.throttle_stream_writes(
        NonZeroUsize::new(TEN_MEGABITS_PER_SECOND).expect("non-zero link rate"),
    );
    let provider: Arc<dyn MachineProvider> = machine.clone();
    let link = RemoteLink::new(provider, LinkOptions::default());
    let connecting = {
        let link = Arc::clone(&link);
        tokio::spawn(async move { link.connect().await })
    };
    let peer = wait_for_fake_stream(&machine).await;
    let (remote, _snapshot) = complete_scripted_handshake(peer).await;
    connecting
        .await
        .expect("connect task")
        .expect("scripted handshake");

    let data = Arc::new(fleet_proto::media::encode(&vec![0xa5; CHUNK_BYTES]));
    let uploads = [
        "00000000-0000-4000-8000-000000000051"
            .parse::<UploadId>()
            .expect("first upload id"),
        "00000000-0000-4000-8000-000000000052"
            .parse::<UploadId>()
            .expect("second upload id"),
    ];
    let (paste_seen_tx, paste_seen_rx) = oneshot::channel();
    let (release_responses_tx, release_responses_rx) = oneshot::channel();
    let responder = tokio::spawn(answer_slow_media_stream(
        remote,
        paste_seen_tx,
        release_responses_rx,
    ));
    let mut first_window = Vec::new();
    for lane in 0..fleet_proto::media::UPLOAD_WINDOW {
        let link = Arc::clone(&link);
        let data = Arc::clone(&data);
        let upload = uploads[lane / 2];
        let lane = lane % 2;
        let mut request = Box::pin(async move {
            link.request(media_chunk_request(&link, upload, lane, 0, &data))
                .await
        });
        assert!(
            futures_util::poll!(request.as_mut()).is_pending(),
            "the throttled first-window write must await its response"
        );
        first_window.push(tokio::spawn(request));
    }
    let paste_started = Instant::now();
    let mut paste_request = {
        let link = Arc::clone(&link);
        Box::pin(async move {
            link.request(RequestBody::PasteTerminal {
                terminal: fleet_core::ids::TerminalId(9),
                text: "x".to_owned(),
            })
            .await
        })
    };
    assert!(
        futures_util::poll!(paste_request.as_mut()).is_pending(),
        "paste is queued behind the undrained media window"
    );
    let paste = tokio::spawn(paste_request);
    let mut lanes = Vec::new();
    for lane in 0..fleet_proto::media::UPLOAD_WINDOW {
        lanes.push(tokio::spawn(send_media_lane(
            Arc::clone(&link),
            uploads[lane / 2],
            lane % 2,
            1,
            Arc::clone(&data),
        )));
    }
    paste_seen_rx.await.expect("remote observed the paste");
    let paste_latency = Instant::now().duration_since(paste_started);
    release_responses_tx
        .send(())
        .expect("scripted remote still awaits its release");
    assert_eq!(
        paste.await.expect("paste task").expect("paste response"),
        ResponseBody::Ack
    );
    for request in first_window {
        request
            .await
            .expect("first-window task")
            .expect("first-window response");
    }
    for lane in lanes {
        lane.await.expect("media lane task");
    }
    let (remote, chunks_before_paste, maximum_frame_time) =
        responder.await.expect("scripted remote task");

    assert_eq!(chunks_before_paste, fleet_proto::media::UPLOAD_WINDOW);
    assert!(
        paste_latency <= Duration::from_millis(600),
        "paste waited {paste_latency:?} behind the media window; longest frame was {maximum_frame_time:?}"
    );
    assert!(
        maximum_frame_time < WRITE_BUDGET / 10,
        "one throttled frame occupied the link for {maximum_frame_time:?}"
    );
    assert_eq!(link.state(), LinkState::Ready);
    link.close().await;
    drop(remote);
}

async fn send_media_lane(
    link: Arc<RemoteLink>,
    upload: UploadId,
    lane: usize,
    start_index: usize,
    data: Arc<String>,
) {
    let chunks_per_lane = FIFTY_MIB_IN_CHUNKS / fleet_proto::media::UPLOAD_WINDOW;
    for index in start_index..chunks_per_lane {
        let response = link
            .request(media_chunk_request(&link, upload, lane, index, &data))
            .await
            .expect("media chunk response");
        assert_eq!(response, ResponseBody::Ack);
    }
}

fn media_chunk_request(
    link: &RemoteLink,
    upload: UploadId,
    lane: usize,
    index: usize,
    data: &str,
) -> RequestBody {
    let offset = ((index * 2 + lane) * CHUNK_BYTES) as u64;
    RequestBody::StageMedia {
        anchor: MediaAnchor::Host {
            host: link.host().clone(),
        },
        upload,
        op: StageOp::Chunk {
            file: 0,
            offset,
            data: data.to_owned(),
        },
    }
}

async fn answer_slow_media_stream(
    mut remote: ScriptedRemote,
    paste_seen: oneshot::Sender<()>,
    release_responses: oneshot::Receiver<()>,
) -> (ScriptedRemote, usize, Duration) {
    let mut previous_frame = Instant::now();
    let mut maximum_frame_time = Duration::ZERO;
    let mut pending = Vec::with_capacity(fleet_proto::media::UPLOAD_WINDOW + 1);
    for _ in 0..fleet_proto::media::UPLOAD_WINDOW {
        let request = next_request(&mut remote).await;
        let now = Instant::now();
        maximum_frame_time = maximum_frame_time.max(now.duration_since(previous_frame));
        previous_frame = now;
        assert!(matches!(
            &request.body,
            RequestBody::StageMedia {
                op: StageOp::Chunk { .. },
                ..
            }
        ));
        pending.push(request);
    }
    let paste = next_request(&mut remote).await;
    let now = Instant::now();
    maximum_frame_time = maximum_frame_time.max(now.duration_since(previous_frame));
    previous_frame = now;
    assert!(matches!(&paste.body, RequestBody::PasteTerminal { .. }));
    pending.push(paste);
    paste_seen.send(()).expect("test still awaits the paste");
    release_responses
        .await
        .expect("test releases the scripted responses");
    for request in pending {
        send_value(
            &mut remote,
            Response {
                id: request.id,
                result: Ok(ResponseBody::Ack),
            },
        )
        .await;
    }

    let mut chunks = fleet_proto::media::UPLOAD_WINDOW;
    while chunks < FIFTY_MIB_IN_CHUNKS {
        let request = next_request(&mut remote).await;
        let now = Instant::now();
        maximum_frame_time = maximum_frame_time.max(now.duration_since(previous_frame));
        previous_frame = now;
        match request.body {
            RequestBody::StageMedia {
                op: StageOp::Chunk { .. },
                ..
            } => chunks += 1,
            other => panic!("unexpected request during throttled upload: {other:?}"),
        }
        send_value(
            &mut remote,
            Response {
                id: request.id,
                result: Ok(ResponseBody::Ack),
            },
        )
        .await;
    }
    (
        remote,
        fleet_proto::media::UPLOAD_WINDOW,
        maximum_frame_time,
    )
}

#[tokio::test]
async fn nudge_on_a_connected_link_does_not_disturb_it() {
    let host = HostId::try_from("local-daemon").expect("scripted host id");
    let machine = Arc::new(FakeMachine::new(host));
    let provider: Arc<dyn MachineProvider> = machine.clone();
    let link = RemoteLink::new(provider, LinkOptions::default());
    let connecting = {
        let link = Arc::clone(&link);
        tokio::spawn(async move { link.connect().await })
    };
    let peer = wait_for_fake_stream(&machine).await;
    let (mut remote, _snapshot) = complete_scripted_handshake(peer).await;
    connecting
        .await
        .expect("connect task")
        .expect("scripted handshake");
    assert_eq!(link.state(), LinkState::Ready);
    let opened = machine.stream_opens();
    let states = link.state_changes();

    link.nudge_reconnect();

    // The established transport must survive the nudge: the same scripted peer still answers.
    // Both halves are bounded, so a nudge that tears the transport down fails here instead of
    // leaving the test waiting on a frame that will never arrive.
    let ping = {
        let link = Arc::clone(&link);
        tokio::spawn(async move { link.request(RequestBody::DaemonPing).await })
    };
    let request = tokio::time::timeout(Duration::from_secs(5), next_request(&mut remote))
        .await
        .expect("a nudge must leave the established transport carrying requests");
    assert!(matches!(request.body, RequestBody::DaemonPing));
    send_value(
        &mut remote,
        Response {
            id: request.id,
            result: Ok(ResponseBody::Pong),
        },
    )
    .await;
    let answered = tokio::time::timeout(Duration::from_secs(5), ping)
        .await
        .expect("the ping must be answered over the surviving transport")
        .expect("ping task");
    assert!(matches!(answered, Ok(ResponseBody::Pong)));
    assert_eq!(
        machine.stream_opens(),
        opened,
        "a nudge must not reopen the transport of a connected link"
    );
    assert_eq!(link.state(), LinkState::Ready);
    assert!(
        !states.has_changed().expect("link state sender"),
        "a nudge must not move a Ready link through any other state"
    );

    link.close().await;
    drop(remote);
}

#[tokio::test]
async fn a_nudge_while_ready_is_retained_for_the_next_disconnect() {
    let host = HostId::try_from("local-daemon").expect("scripted host id");
    let machine = Arc::new(FakeMachine::new(host));
    let provider: Arc<dyn MachineProvider> = machine.clone();
    // A backoff floor far above the wait below, so only a retained permit can shorten it.
    let link = RemoteLink::new(
        provider,
        LinkOptions {
            backoff_min: Duration::from_secs(30),
            backoff_max: Duration::from_secs(30),
            hello_timeout: Duration::from_secs(5),
        },
    );
    let connecting = {
        let link = Arc::clone(&link);
        tokio::spawn(async move { link.connect().await })
    };
    let peer = wait_for_fake_stream(&machine).await;
    let (remote, _snapshot) = complete_scripted_handshake(peer).await;
    connecting
        .await
        .expect("connect task")
        .expect("scripted handshake");
    assert_eq!(link.state(), LinkState::Ready);

    // Bootstrap nudges between the remote restart and its probe, while the link still reads as
    // Ready. Discarding the nudge there would make the probe wait out the whole backoff.
    link.nudge_reconnect();
    drop(remote);

    // `wait_for_fake_stream` gives the retried attempt one second: thirty times less than the
    // floor a link that dropped the nudge would sleep through first.
    let retry = wait_for_fake_stream(&machine).await;
    let (retried, _snapshot) = complete_scripted_handshake(retry).await;

    link.close().await;
    drop(retried);
}

type ScriptedRemote = Framed<DuplexStream, FleetCodec<Value, Request>>;
type RawRemote = Framed<DuplexStream, FleetCodec<Value, Value>>;

#[tokio::test]
async fn link_retries_hello_with_the_v6_string_client_shape() {
    let host = HostId::try_from("legacy-peer").expect("host");
    let machine = Arc::new(FakeMachine::new(host));
    let provider: Arc<dyn MachineProvider> = machine.clone();
    let link = RemoteLink::new(provider, LinkOptions::default());
    let connecting = tokio::spawn({
        let link = Arc::clone(&link);
        async move { link.connect().await }
    });

    let first = wait_for_fake_stream(&machine).await;
    let mut first = RawRemote::new(first, FleetCodec::new());
    let current = first.next().await.expect("current Hello").expect("frame");
    assert!(current["body"]["client"].is_object());
    drop(first);

    let retry = wait_for_fake_stream(&machine).await;
    let mut retry = RawRemote::new(retry, FleetCodec::new());
    let legacy = retry.next().await.expect("legacy Hello").expect("frame");
    assert_eq!(legacy["body"]["client"], "fleet");
    retry
        .send(
            serde_json::to_value(Response {
                id: 0,
                result: Err(fleet_proto::error::ProtoError {
                    kind: fleet_proto::error::ErrorKind::Unsupported,
                    message: "unsupported protocol 7; expected 6".to_owned(),
                }),
            })
            .expect("serialize response"),
        )
        .await
        .expect("send response");

    let error = connecting
        .await
        .expect("connect task")
        .expect_err("v6 peer must report a version mismatch");
    assert!(error.to_string().contains("protocol version mismatch"));
    link.close().await;
}

#[tokio::test]
async fn link_refuses_a_v7_daemon_without_remote_machine_capability() {
    let host = HostId::try_from("incapable-peer").expect("host");
    let machine = Arc::new(FakeMachine::new(host));
    let provider: Arc<dyn MachineProvider> = machine.clone();
    let link = RemoteLink::new(provider, LinkOptions::default());
    let connecting = tokio::spawn({
        let link = Arc::clone(&link);
        async move { link.connect().await }
    });
    let peer = wait_for_fake_stream(&machine).await;
    let mut remote = RawRemote::new(peer, FleetCodec::new());
    let _hello = remote.next().await.expect("Hello").expect("frame");
    remote
        .send(
            serde_json::to_value(HelloResponse {
                response: Response {
                    id: 0,
                    result: Ok(ResponseBody::Hello {
                        protocol: PROTOCOL_VERSION,
                        server: "fleetd incapable".to_owned(),
                    }),
                },
                snapshot_revision: None,
                capabilities: Vec::new(),
                daemon_id: "incapable".to_owned(),
                build_commit: None,
            })
            .expect("serialize Hello"),
        )
        .await
        .expect("send Hello");

    let error = connecting
        .await
        .expect("connect task")
        .expect_err("missing capability must be rejected");
    assert!(
        matches!(error, DaemonError::Remote(message) if message.contains("does not advertise"))
    );
    link.close().await;
}

struct AppearingMachine {
    machine: FakeMachine,
    available: AtomicBool,
}

impl AppearingMachine {
    fn new(host: HostId) -> Self {
        Self {
            machine: FakeMachine::new(host),
            available: AtomicBool::new(false),
        }
    }

    fn set_available(&self) {
        self.available.store(true, Ordering::Release);
    }
}

#[async_trait]
impl MachineProvider for AppearingMachine {
    fn id(&self) -> &HostId {
        self.machine.id()
    }

    fn provider_name(&self) -> &'static str {
        self.machine.provider_name()
    }

    async fn resolve(&self) -> Result<MachineAddress, MachineError> {
        self.machine.resolve().await
    }

    async fn probe(&self, timeout: Duration) -> ProbeReport {
        self.machine.probe(timeout).await
    }

    async fn exec(&self, argv: &[String], timeout: Duration) -> Result<ExecOutput, MachineError> {
        self.machine.exec(argv, timeout).await
    }

    async fn open_stream(&self) -> Result<Box<dyn AsyncDuplex>, MachineError> {
        if !self.available.load(Ordering::Acquire) {
            return Err(MachineError::Unreachable(
                "remote daemon is absent".to_owned(),
            ));
        }
        self.machine.open_stream().await
    }

    fn fleetd_binary(&self) -> &str {
        self.machine.fleetd_binary()
    }

    fn fleet_home(&self) -> Option<&str> {
        self.machine.fleet_home()
    }
}

async fn wait_for_fake_stream(machine: &FakeMachine) -> DuplexStream {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Some(peer) = machine.take_stream_peer() {
                return peer;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("link did not ask the fake machine for a stream")
}

async fn complete_scripted_handshake(peer: DuplexStream) -> (ScriptedRemote, Snapshot) {
    let mut remote = Framed::new(peer, FleetCodec::<Value, Request>::new());
    let hello = next_request(&mut remote).await;
    assert_eq!(hello.id, 0);
    assert!(matches!(
        hello.body,
        RequestBody::Hello {
            protocol: PROTOCOL_VERSION,
            client
        } if client.kind == ClientKind::Proxy
            && client.host_id.as_ref().is_some_and(|id| id.as_str() == "local-daemon")
            && client.supports(fleet_proto::TERMINAL_CLIPBOARD_CAPABILITY)
    ));
    send_value(
        &mut remote,
        HelloResponse {
            response: Response {
                id: 0,
                result: Ok(ResponseBody::Hello {
                    protocol: PROTOCOL_VERSION,
                    server: "fleetd scripted".to_owned(),
                }),
            },
            snapshot_revision: None,
            capabilities: vec!["remote-machines".to_owned()],
            daemon_id: "scripted-daemon".to_owned(),
            build_commit: Some("abc123".to_owned()),
        },
    )
    .await;

    let subscribe = next_request(&mut remote).await;
    assert_eq!(subscribe.id, 1);
    assert!(matches!(
        subscribe.body,
        RequestBody::Subscribe { events }
            if events.contains(&EventKind::SnapshotChanged)
                && events.contains(&EventKind::TerminalExited)
                && !events.contains(&EventKind::TerminalClipboard)
                && !events.contains(&EventKind::HostLinkChanged)
                && !events.contains(&EventKind::TerminalReattach)
    ));
    send_value(
        &mut remote,
        Response {
            id: 1,
            result: Ok(ResponseBody::Ack),
        },
    )
    .await;

    let get_snapshot = next_request(&mut remote).await;
    assert_eq!(get_snapshot.id, 2);
    assert!(matches!(get_snapshot.body, RequestBody::GetSnapshot));
    let snapshot = empty_snapshot();
    send_value(
        &mut remote,
        Response {
            id: 2,
            result: Ok(ResponseBody::Snapshot(snapshot.clone())),
        },
    )
    .await;
    (remote, snapshot)
}

async fn next_request(remote: &mut ScriptedRemote) -> Request {
    remote
        .next()
        .await
        .expect("remote stream closed before request")
        .expect("decode request")
}

async fn send_value(remote: &mut ScriptedRemote, value: impl serde::Serialize) {
    remote
        .send(serde_json::to_value(value).expect("serialize response"))
        .await
        .expect("send response");
}

fn empty_snapshot() -> Snapshot {
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
        "daemon": {
            "version": "fleetd scripted",
            "pid": 7,
            "startedAt": "2026-09-08T12:00:00Z",
            "home": "/tmp/scripted"
        }
    }))
    .expect("valid scripted snapshot")
}

async fn wait_for_socket(home: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if tokio::net::UnixStream::connect(home.join("fleetd.sock"))
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("remote daemon socket");
}

async fn wait_for_state(states: &mut watch::Receiver<LinkState>, expected: LinkState) {
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

async fn wait_for_link_event(events: &mut broadcast::Receiver<Event>, expected: LinkState) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match events.recv().await.expect("link event sender") {
                Event::HostLinkChanged { link, .. } if link == expected => return,
                _ => {}
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("link did not emit {expected:?}"));
}
