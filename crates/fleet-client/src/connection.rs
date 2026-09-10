//! Unix socket connection lifecycle and protocol transport.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use fleet_core::{ids::TerminalId, paths::FleetHome};
use fleet_proto::{
    PROTOCOL_VERSION,
    codec::FleetCodec,
    error::{ErrorKind, ProtoError},
    event::{Event, EventKind, ToastLevel},
    request::{Request, RequestBody},
    response::{DaemonIdentity, HelloResponse, PongResponse, Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt, stream::SplitSink, stream::SplitStream};
use serde_json::Value;
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::UnixStream,
    runtime::Handle,
    sync::{broadcast, mpsc, oneshot},
    time::{Instant, sleep_until, timeout_at},
};
use tokio_util::codec::Framed;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_BUDGET: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
const INITIAL_RECONNECT_BACKOFF: Duration = Duration::from_millis(50);
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(2);
const COMMAND_CAPACITY: usize = 256;
const EVENT_CAPACITY: usize = 1_024;
const HANDSHAKE_EVENT_CAPACITY: usize = 1_024;

/// Fleet's framed client transport over any Tokio byte stream.
pub type ProtocolTransport<S> = Framed<S, FleetCodec<Request, Value>>;

type Transport = ProtocolTransport<UnixStream>;
type TransportWriter<S> = SplitSink<ProtocolTransport<S>, Request>;
type TransportReader<S> = SplitStream<ProtocolTransport<S>>;

/// Wraps an arbitrary Tokio duplex stream in Fleet's client-side codec.
pub fn protocol_transport<S>(stream: S) -> ProtocolTransport<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    Framed::new(stream, FleetCodec::new())
}

#[derive(Debug)]
struct Established<S> {
    transport: ProtocolTransport<S>,
    buffered_events: Vec<Event>,
}

/// A failure to establish and negotiate a daemon connection.
#[derive(Debug, Error)]
pub enum ConnectError {
    /// The daemon socket could not be connected or used.
    #[error("could not connect to Fleet daemon: {0}")]
    Io(#[from] std::io::Error),
    /// The protocol handshake could not be encoded or decoded.
    #[error("Fleet protocol handshake failed: {0}")]
    Codec(#[from] fleet_proto::codec::CodecError),
    /// The daemon rejected the protocol handshake.
    #[error("Fleet daemon rejected the handshake: {0}")]
    Protocol(#[from] ProtoError),
    /// The daemon did not complete the handshake in time.
    #[error("Fleet daemon handshake timed out")]
    Timeout,
    /// The daemon returned an invalid handshake response.
    #[error("Fleet daemon returned an invalid handshake response: {0}")]
    InvalidHandshake(String),
}

/// An asynchronous, reconnecting client for a Fleet daemon.
#[derive(Clone, Debug)]
pub struct Client {
    pub(crate) inner: Arc<ClientInner>,
}

#[derive(Debug)]
pub(crate) struct ClientInner {
    commands: mpsc::Sender<Command>,
    events: broadcast::Sender<Event>,
    next_id: AtomicU64,
    metadata: Arc<RwLock<ConnectionMetadata>>,
}

#[derive(Debug)]
struct Command {
    request: Request,
    response: Option<oneshot::Sender<Result<ResponseBody, ProtoError>>>,
    expires_at: Option<Instant>,
}

#[derive(Debug)]
struct Pending {
    effect: ConnectionEffect,
    response: Option<oneshot::Sender<Result<ResponseBody, ProtoError>>>,
}

#[derive(Debug, Clone, Copy)]
struct Attachment {
    cols: u16,
    rows: u16,
}

#[derive(Debug, Clone, Default)]
struct ConnectionMetadata {
    capabilities: HashSet<String>,
    daemon_identity: Option<DaemonIdentity>,
}

#[derive(Debug)]
struct ConnectionState {
    subscriptions: Vec<EventKind>,
    attachments: HashMap<TerminalId, Attachment>,
    capabilities: HashSet<String>,
    daemon_identity: Option<DaemonIdentity>,
    daemon_identity_initialized: bool,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            subscriptions: all_event_kinds(),
            attachments: HashMap::new(),
            capabilities: HashSet::new(),
            daemon_identity: None,
            daemon_identity_initialized: false,
        }
    }
}

impl Client {
    /// Connects to `home/fleetd.sock`, negotiates Fleet's current protocol, and starts the actor.
    pub async fn connect(home: impl AsRef<Path>) -> Result<Self, ConnectError> {
        let home = home.as_ref().to_path_buf();
        let mut state = ConnectionState::default();
        let established = establish(&home, &mut state).await?;
        let (commands, command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let events = broadcast::Sender::new(EVENT_CAPACITY);
        let inner = Arc::new(ClientInner {
            commands,
            events: events.clone(),
            next_id: AtomicU64::new(2),
            metadata: Arc::new(RwLock::new(ConnectionMetadata {
                capabilities: state.capabilities.clone(),
                daemon_identity: None,
            })),
        });
        let metadata = Arc::clone(&inner.metadata);
        tokio::spawn(run_connection(
            home,
            established,
            state,
            command_rx,
            events,
            metadata,
        ));
        Ok(Self { inner })
    }

    /// Sends a raw protocol request and returns its correlated response payload.
    pub async fn request(&self, body: RequestBody) -> Result<ResponseBody, ProtoError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (response_tx, response_rx) = oneshot::channel();
        let enqueue_deadline = Instant::now() + REQUEST_TIMEOUT;
        let expires_at = request_timeout(&body).map(|duration| Instant::now() + duration);
        let command = Command {
            request: Request { id, body },
            response: Some(response_tx),
            expires_at,
        };
        timeout_at(enqueue_deadline, self.inner.commands.send(command))
            .await
            .map_err(|_| transport_error("Fleet daemon request timed out"))?
            .map_err(|_| transport_error("Fleet daemon connection is closed"))?;

        match expires_at {
            Some(deadline) => match timeout_at(deadline, response_rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(transport_error("Fleet daemon connection is closed")),
                Err(_) => Err(transport_error("Fleet daemon request timed out")),
            },
            None => response_rx
                .await
                .map_err(|_| transport_error("Fleet daemon connection is closed"))?,
        }
    }

    /// Enqueues a raw protocol request without waiting for its response.
    ///
    /// Awaiting this method preserves the caller's request order through the connection actor.
    /// It is intended for event-backed mutations whose result is observed through daemon events.
    pub async fn request_background(&self, body: RequestBody) -> Result<(), ProtoError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let enqueue_deadline = Instant::now() + REQUEST_TIMEOUT;
        let command = Command {
            request: Request { id, body },
            response: None,
            expires_at: Some(enqueue_deadline),
        };
        timeout_at(enqueue_deadline, self.inner.commands.send(command))
            .await
            .map_err(|_| transport_error("Fleet daemon request timed out"))?
            .map_err(|_| transport_error("Fleet daemon connection is closed"))
    }

    /// Returns a receiver for all subscribed daemon events.
    #[must_use]
    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// Capabilities advertised by the daemon during the most recent Hello exchange.
    #[must_use]
    pub fn capabilities(&self) -> Vec<String> {
        let mut capabilities = self
            .inner
            .metadata
            .read()
            .map(|metadata| metadata.capabilities.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        capabilities.sort_unstable();
        capabilities
    }

    /// Whether the most recently negotiated daemon advertised `capability`.
    #[must_use]
    pub fn supports_capability(&self, capability: &str) -> bool {
        self.inner
            .metadata
            .read()
            .is_ok_and(|metadata| metadata.capabilities.contains(capability))
    }

    /// PID reported by the most recent identity-bearing Pong.
    #[must_use]
    pub fn daemon_pid(&self) -> Option<u32> {
        self.daemon_identity().map(|(pid, _)| pid)
    }

    /// PID and per-process boot identity reported by the most recent Pong.
    #[must_use]
    pub fn daemon_identity(&self) -> Option<(u32, String)> {
        self.inner
            .metadata
            .read()
            .ok()
            .and_then(|metadata| metadata.daemon_identity.clone())
            .map(|identity| (identity.pid, identity.boot_id))
    }

    pub(crate) fn send_background(&self, body: RequestBody) {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let command = Command {
            request: Request { id, body },
            response: None,
            expires_at: Some(Instant::now() + REQUEST_TIMEOUT),
        };
        match self.inner.commands.try_send(command) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            Err(mpsc::error::TrySendError::Full(command)) => {
                if let Ok(runtime) = Handle::try_current() {
                    let commands = self.inner.commands.clone();
                    runtime.spawn(async move {
                        if let Some(deadline) = command.expires_at {
                            let _ = timeout_at(deadline, commands.send(command)).await;
                        }
                    });
                }
            }
        }
    }
}

async fn run_connection(
    home: PathBuf,
    established: Established<UnixStream>,
    mut state: ConnectionState,
    mut commands: mpsc::Receiver<Command>,
    events: broadcast::Sender<Event>,
    metadata: Arc<RwLock<ConnectionMetadata>>,
) {
    let (mut writer, mut reader) = established.transport.split();
    let mut pending = HashMap::<u64, Pending>::new();
    let mut queued = VecDeque::<Command>::new();
    let mut backoff = INITIAL_RECONNECT_BACKOFF;

    if publish_events(established.buffered_events, &events) {
        return;
    }

    loop {
        'connected: loop {
            if let Some(command) = queued.pop_front() {
                match send_command(
                    &mut writer,
                    &mut reader,
                    command,
                    &mut pending,
                    &mut state,
                    &events,
                    &metadata,
                )
                .await
                {
                    DispatchOutcome::Sent => {}
                    DispatchOutcome::Disconnected => break 'connected,
                    DispatchOutcome::ShuttingDown => {
                        fail_pending(&mut pending, "Fleet daemon is shutting down");
                        return;
                    }
                }
                continue;
            }

            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else {
                        fail_pending(&mut pending, "Fleet client was dropped");
                        return;
                    };
                    match send_command(
                        &mut writer,
                        &mut reader,
                        command,
                        &mut pending,
                        &mut state,
                        &events,
                        &metadata,
                    ).await {
                        DispatchOutcome::Sent => {}
                        DispatchOutcome::Disconnected => break 'connected,
                        DispatchOutcome::ShuttingDown => {
                            fail_pending(&mut pending, "Fleet daemon is shutting down");
                            return;
                        }
                    }
                }
                incoming = reader.next() => {
                    match incoming {
                        Some(Ok(value)) => {
                            if handle_incoming(value, &mut pending, &mut state, &events, &metadata) {
                                fail_pending(&mut pending, "Fleet daemon is shutting down");
                                return;
                            }
                        }
                        Some(Err(error)) => {
                            tracing::debug!(%error, "Fleet daemon connection decoding failed");
                            break 'connected;
                        }
                        None => break 'connected,
                    }
                }
            }
        }

        fail_pending(&mut pending, "Fleet daemon connection was lost");
        match reconnect(
            &home,
            &mut state,
            &mut commands,
            &mut queued,
            &mut backoff,
            &events,
            &metadata,
        )
        .await
        {
            Some(new_transport) => (writer, reader) = new_transport.split(),
            None => return,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchOutcome {
    Sent,
    Disconnected,
    ShuttingDown,
}

async fn send_command<S>(
    writer: &mut TransportWriter<S>,
    reader: &mut TransportReader<S>,
    command: Command,
    pending: &mut HashMap<u64, Pending>,
    state: &mut ConnectionState,
    events: &broadcast::Sender<Event>,
    metadata: &RwLock<ConnectionMetadata>,
) -> DispatchOutcome
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some(command) = command_for_dispatch(command) else {
        return DispatchOutcome::Sent;
    };
    let id = command.request.id;
    let effect = ConnectionEffect::from(&command.request.body);
    let write_deadline = socket_write_deadline();
    pending.insert(
        id,
        Pending {
            effect,
            response: command.response,
        },
    );
    let write = writer.send(command.request);
    tokio::pin!(write);
    let deadline = sleep_until(write_deadline);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            result = &mut write => {
                if let Err(error) = result {
                    tracing::debug!(%error, "Fleet daemon connection was lost while sending");
                    return DispatchOutcome::Disconnected;
                }
                return DispatchOutcome::Sent;
            }
            incoming = reader.next() => {
                match incoming {
                    Some(Ok(value)) => {
                        if handle_incoming(value, pending, state, events, metadata) {
                            return DispatchOutcome::ShuttingDown;
                        }
                    }
                    Some(Err(error)) => {
                        tracing::debug!(%error, "Fleet daemon connection decoding failed");
                        return DispatchOutcome::Disconnected;
                    }
                    None => return DispatchOutcome::Disconnected,
                }
            }
            () = &mut deadline => {
                tracing::debug!(request_id = id, "Fleet daemon socket write timed out");
                return DispatchOutcome::Disconnected;
            }
        }
    }
}

fn socket_write_deadline() -> Instant {
    Instant::now() + WRITE_BUDGET
}

fn handle_incoming(
    value: Value,
    pending: &mut HashMap<u64, Pending>,
    state: &mut ConnectionState,
    events: &broadcast::Sender<Event>,
    metadata: &RwLock<ConnectionMetadata>,
) -> bool {
    if value.get("id").is_some() {
        let pong_identity = serde_json::from_value::<PongResponse>(value.clone())
            .ok()
            .and_then(|pong| pong.daemon);
        match serde_json::from_value::<Response>(value) {
            Ok(response) => {
                if let Some(request) = pending.remove(&response.id) {
                    if response.result.is_ok() {
                        request.effect.apply(state);
                    }
                    if matches!(&response.result, Ok(ResponseBody::Pong))
                        && let Some(identity) = pong_identity
                    {
                        reconcile_daemon_identity(state, Some(identity));
                        retain_pong_identity(metadata, state.daemon_identity.clone());
                    }
                    let shutting_down = matches!(&response.result, Ok(ResponseBody::ShuttingDown));
                    match request.response {
                        Some(sender) => {
                            let _ = sender.send(response.result);
                        }
                        None => {
                            if let Err(error) = response.result {
                                publish_background_error(events, error);
                            }
                        }
                    }
                    return shutting_down;
                }
            }
            Err(error) => tracing::warn!(%error, "ignored malformed Fleet response"),
        }
        return false;
    }

    match serde_json::from_value::<Event>(value) {
        Ok(event) => publish(events, event),
        Err(error) => {
            tracing::warn!(%error, "ignored malformed Fleet event");
            false
        }
    }
}

fn publish_background_error(events: &broadcast::Sender<Event>, error: ProtoError) {
    if matches!(error.kind, ErrorKind::NotFound | ErrorKind::Cancelled) {
        tracing::debug!(kind = ?error.kind, message = %error.message, "ignored non-actionable background failure");
        return;
    }
    publish(
        events,
        Event::Toast {
            level: ToastLevel::Error,
            message: error.message,
        },
    );
}

#[derive(Debug)]
enum ConnectionEffect {
    None,
    Subscribe(Vec<EventKind>),
    Unsubscribe,
    Attach(TerminalId, Attachment),
    Detach(TerminalId),
    Resize(TerminalId, Attachment),
}

impl From<&RequestBody> for ConnectionEffect {
    fn from(body: &RequestBody) -> Self {
        match body {
            RequestBody::Subscribe { events } => Self::Subscribe(events.clone()),
            RequestBody::Unsubscribe => Self::Unsubscribe,
            RequestBody::AttachTerminal {
                terminal,
                cols,
                rows,
            } => Self::Attach(
                *terminal,
                Attachment {
                    cols: *cols,
                    rows: *rows,
                },
            ),
            RequestBody::DetachTerminal { terminal } | RequestBody::CloseTerminal { terminal } => {
                Self::Detach(*terminal)
            }
            RequestBody::ResizeTerminal {
                terminal,
                cols,
                rows,
            } => Self::Resize(
                *terminal,
                Attachment {
                    cols: *cols,
                    rows: *rows,
                },
            ),
            _ => Self::None,
        }
    }
}

impl ConnectionEffect {
    fn apply(self, state: &mut ConnectionState) {
        match self {
            Self::None => {}
            // The daemon adds the kinds a Subscribe names to the set it already holds
            // (`server/connection.rs`), so the reconnect handshake must replay that union or the
            // same connection would deliver a narrower event set after a reconnect than before.
            Self::Subscribe(events) => {
                for kind in events {
                    if !state.subscriptions.contains(&kind) {
                        state.subscriptions.push(kind);
                    }
                }
            }
            Self::Unsubscribe => state.subscriptions.clear(),
            Self::Attach(terminal, attachment) => {
                state.attachments.insert(terminal, attachment);
            }
            Self::Detach(terminal) => {
                state.attachments.remove(&terminal);
            }
            Self::Resize(terminal, size) => {
                if let Some(attachment) = state.attachments.get_mut(&terminal) {
                    *attachment = size;
                }
            }
        }
    }
}

fn request_timeout(body: &RequestBody) -> Option<Duration> {
    if matches!(
        body,
        RequestBody::CreateWorktree { .. }
            | RequestBody::CreateWorktreeFromPr { .. }
            | RequestBody::CreateWorktreeFromCard { .. }
            // A board backend validates and describes itself by shelling out to its own CLI,
            // which has a deadline and a retry budget of its own an order of magnitude past
            // this one. Timing these out here replaces the backend's own sentence — the
            // install hint, the throttling notice, the JQL Jira refused — with a transport
            // error, while the daemon keeps running the call the client stopped waiting for.
            | RequestBody::CreateBoard { .. }
            | RequestBody::UpdateBoard { .. }
            | RequestBody::DescribeBoardBackend { .. }
    ) {
        None
    } else {
        Some(REQUEST_TIMEOUT)
    }
}

async fn reconnect(
    home: &Path,
    state: &mut ConnectionState,
    commands: &mut mpsc::Receiver<Command>,
    queued: &mut VecDeque<Command>,
    backoff: &mut Duration,
    events: &broadcast::Sender<Event>,
    metadata: &RwLock<ConnectionMetadata>,
) -> Option<Transport> {
    loop {
        discard_obsolete_commands(queued);
        let sleep = tokio::time::sleep(*backoff);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                command = commands.recv(), if queued.len() < COMMAND_CAPACITY => match command {
                    Some(command) => queue_disconnected_command(queued, command),
                    None => return None,
                },
                () = &mut sleep => break,
            }
        }

        match establish(home, state).await {
            Ok(established) => {
                *backoff = INITIAL_RECONNECT_BACKOFF;
                synchronize_metadata(metadata, state);
                if publish_events(established.buffered_events, events) {
                    return None;
                }
                return Some(established.transport);
            }
            Err(error) => {
                tracing::debug!(%error, "Fleet daemon reconnect attempt failed");
                *backoff = (*backoff * 2).min(MAX_RECONNECT_BACKOFF);
                discard_obsolete_commands(queued);
            }
        }
    }
}

fn queue_disconnected_command(queued: &mut VecDeque<Command>, command: Command) {
    if command_is_expired(&command) {
        fail_command(command, "Fleet daemon request timed out while reconnecting");
        return;
    }
    if command.response.is_none()
        && let RequestBody::ResizeTerminal { terminal, .. } = &command.request.body
        && let Some(index) = queued.iter().position(|queued| {
            queued.response.is_none()
                && matches!(
                    &queued.request.body,
                    RequestBody::ResizeTerminal {
                        terminal: queued_terminal,
                        ..
                    } if queued_terminal == terminal
                )
        })
    {
        queued.remove(index);
    }
    queued.push_back(command);
}

fn discard_obsolete_commands(queued: &mut VecDeque<Command>) {
    let mut active = VecDeque::with_capacity(queued.len());
    while let Some(command) = queued.pop_front() {
        if command_is_expired(&command) {
            fail_command(command, "Fleet daemon request timed out while reconnecting");
        } else {
            active.push_back(command);
        }
    }
    *queued = active;
}

async fn establish(
    home: &Path,
    state: &mut ConnectionState,
) -> Result<Established<UnixStream>, ConnectError> {
    let socket = UnixStream::connect(FleetHome::new(home).socket_path()).await?;
    let mut transport = protocol_transport(socket);
    let mut buffered_events = Vec::new();
    let capabilities = exchange(
        &mut transport,
        &mut buffered_events,
        Request {
            id: 0,
            body: RequestBody::Hello {
                protocol: PROTOCOL_VERSION,
                client: fleet_proto::request::HelloClient::default(),
            },
        },
        negotiated_capabilities,
    )
    .await?;
    state.capabilities = capabilities.into_iter().collect();

    reconcile_daemon_identity(state, read_daemon_identity(home));

    exchange(
        &mut transport,
        &mut buffered_events,
        Request {
            id: 1,
            body: RequestBody::Subscribe {
                events: state.subscriptions.clone(),
            },
        },
        expect_ack,
    )
    .await?;

    let attachments = state
        .attachments
        .iter()
        .map(|(terminal, attachment)| (*terminal, *attachment))
        .collect::<Vec<_>>();
    for (offset, (terminal, attachment)) in attachments.into_iter().enumerate() {
        let id = u64::try_from(offset).map_or(u64::MAX, |value| value.saturating_add(2));
        let result = exchange(
            &mut transport,
            &mut buffered_events,
            Request {
                id,
                body: RequestBody::AttachTerminal {
                    terminal,
                    cols: attachment.cols,
                    rows: attachment.rows,
                },
            },
            expect_ack,
        )
        .await;
        match result {
            Ok(()) => {}
            Err(ConnectError::Protocol(error)) if error.kind == ErrorKind::NotFound => {
                state.attachments.remove(&terminal);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(Established {
        transport,
        buffered_events,
    })
}

fn reconcile_daemon_identity(state: &mut ConnectionState, identity: Option<DaemonIdentity>) {
    if state.daemon_identity_initialized && state.daemon_identity != identity {
        state.attachments.clear();
    }
    state.daemon_identity = identity;
    state.daemon_identity_initialized = true;
}

fn read_daemon_identity(home: &Path) -> Option<DaemonIdentity> {
    let contents = fs::read_to_string(FleetHome::new(home).pid_path()).ok()?;
    let pid = contents
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid != 0)?;
    Some(DaemonIdentity {
        pid,
        boot_id: process_started_at(pid).map_or_else(
            || format!("legacy-pid-{pid}"),
            |(seconds, fraction)| format!("process-start-{seconds}-{fraction}"),
        ),
    })
}

fn synchronize_metadata(metadata: &RwLock<ConnectionMetadata>, state: &ConnectionState) {
    let Ok(mut metadata) = metadata.write() else {
        return;
    };
    metadata.capabilities.clone_from(&state.capabilities);
    metadata.daemon_identity = None;
}

fn retain_pong_identity(metadata: &RwLock<ConnectionMetadata>, identity: Option<DaemonIdentity>) {
    if let Ok(mut metadata) = metadata.write() {
        metadata.daemon_identity = identity;
    }
}

fn negotiated_capabilities(body: ResponseBody, value: &Value) -> Result<Vec<String>, ConnectError> {
    match body {
        ResponseBody::Hello { protocol, .. } if protocol == PROTOCOL_VERSION => {
            serde_json::from_value::<HelloResponse>(value.clone())
                .map(|hello| hello.capabilities)
                .map_err(|error| ConnectError::InvalidHandshake(error.to_string()))
        }
        other => Err(ConnectError::InvalidHandshake(format!("{other:?}"))),
    }
}

#[cfg(target_os = "macos")]
fn process_started_at(pid: u32) -> Option<(u64, u64)> {
    use std::mem::MaybeUninit;

    let raw_pid = libc::pid_t::try_from(pid).ok()?;
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let info_size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: proc_pidinfo receives a correctly sized writable proc_bsdinfo buffer.
    let read = unsafe {
        libc::proc_pidinfo(
            raw_pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            info_size,
        )
    };
    if read != info_size {
        return None;
    }
    // SAFETY: a full proc_bsdinfo was initialized when proc_pidinfo returned its size.
    let info = unsafe { info.assume_init() };
    Some((info.pbi_start_tvsec, info.pbi_start_tvusec))
}

#[cfg(target_os = "linux")]
fn process_started_at(pid: u32) -> Option<(u64, u64)> {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()?
        .rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse::<u64>()
        .ok()
        .map(|ticks| (ticks, 0))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_started_at(_pid: u32) -> Option<(u64, u64)> {
    None
}

async fn exchange<S, T>(
    transport: &mut ProtocolTransport<S>,
    buffered_events: &mut Vec<Event>,
    request: Request,
    validate: impl FnOnce(ResponseBody, &Value) -> Result<T, ConnectError>,
) -> Result<T, ConnectError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let expected_id = request.id;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    timeout_at(deadline, transport.send(request))
        .await
        .map_err(|_| ConnectError::Timeout)??;
    loop {
        let value = timeout_at(deadline, transport.next())
            .await
            .map_err(|_| ConnectError::Timeout)?
            .ok_or_else(|| ConnectError::InvalidHandshake("connection closed".to_owned()))??;
        if value.get("id").is_none() {
            let event = serde_json::from_value(value)
                .map_err(|error| ConnectError::InvalidHandshake(error.to_string()))?;
            if buffered_events.len() >= HANDSHAKE_EVENT_CAPACITY {
                return Err(ConnectError::InvalidHandshake(
                    "too many events arrived during handshake".to_owned(),
                ));
            }
            buffered_events.push(event);
            continue;
        }
        let response: Response = serde_json::from_value(value.clone())
            .map_err(|error| ConnectError::InvalidHandshake(error.to_string()))?;
        if response.id != expected_id {
            return Err(ConnectError::InvalidHandshake(format!(
                "expected response id {expected_id}, got {}",
                response.id
            )));
        }
        return validate(response.result?, &value);
    }
}

fn publish_events(buffered: Vec<Event>, events: &broadcast::Sender<Event>) -> bool {
    let mut shutting_down = false;
    for event in buffered {
        shutting_down |= publish(events, event);
    }
    shutting_down
}

/// Delivers one daemon event and reports whether the daemon announced its shutdown.
fn publish(events: &broadcast::Sender<Event>, event: Event) -> bool {
    let shutting_down = matches!(event, Event::DaemonShuttingDown);
    let _ = events.send(event);
    shutting_down
}

fn expect_ack(body: ResponseBody, _value: &Value) -> Result<(), ConnectError> {
    match body {
        ResponseBody::Ack => Ok(()),
        other => Err(ConnectError::InvalidHandshake(format!(
            "expected acknowledgement, got {other:?}"
        ))),
    }
}

fn all_event_kinds() -> Vec<EventKind> {
    vec![
        EventKind::Agent,
        EventKind::AgentSummary,
        EventKind::WatchStarted,
        EventKind::WatchOutput,
        EventKind::WatchExited,
        EventKind::WatchDismissed,
        EventKind::SnapshotChanged,
        EventKind::BoardChanged,
        EventKind::JobUpdated,
        EventKind::SessionChanged,
        EventKind::AgentActivityChanged,
        EventKind::TerminalFrame,
        EventKind::TerminalExited,
        EventKind::TerminalTitle,
        EventKind::HostLinkChanged,
        EventKind::TerminalReattach,
        EventKind::Toast,
        EventKind::DaemonShuttingDown,
    ]
}

fn command_is_expired(command: &Command) -> bool {
    command
        .expires_at
        .is_some_and(|expires_at| expires_at <= Instant::now())
        || command
            .response
            .as_ref()
            .is_some_and(oneshot::Sender::is_closed)
}

fn command_for_dispatch(command: Command) -> Option<Command> {
    if command_is_expired(&command) {
        fail_command(command, "Fleet daemon request timed out before dispatch");
        None
    } else {
        Some(command)
    }
}

fn fail_command(command: Command, message: &str) {
    if let Some(sender) = command.response {
        let _ = sender.send(Err(transport_error(message)));
    }
}

fn fail_pending(pending: &mut HashMap<u64, Pending>, message: &str) {
    for (_, request) in pending.drain() {
        if let Some(sender) = request.response {
            let _ = sender.send(Err(transport_error(message)));
        }
    }
}

fn transport_error(message: impl Into<String>) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unknown,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use fleet_core::{ids::RepoId, model::RepoHooks};

    use super::*;

    #[test]
    fn create_requests_are_not_bound_by_the_generic_rpc_deadline() {
        let repo = RepoId::try_from("acme/api").unwrap();
        assert!(
            request_timeout(&RequestBody::CreateWorktree {
                repo,
                slug: "slow".to_owned(),
                branch: None,
                base: None,
                host: None,
                hooks: RepoHooks::default(),
            })
            .is_none()
        );
        // A card worktree runs the same prepare hooks, so it waits just as long.
        assert!(
            request_timeout(&RequestBody::CreateWorktreeFromCard {
                card_id: "card".parse().unwrap_or_else(|error| panic!("{error}")),
                repo_id: None,
                base: None,
                host: None,
            })
            .is_none()
        );
        assert_eq!(
            request_timeout(&RequestBody::DaemonPing),
            Some(REQUEST_TIMEOUT)
        );
    }

    #[test]
    fn board_backend_requests_wait_for_the_backend_to_answer() {
        let board_id = "board".parse().unwrap_or_else(|error| panic!("{error}"));
        // `describe` is two or three CLI calls, each with its own retry budget: a deadline
        // here would report a transport error instead of what the backend had to say.
        assert!(
            request_timeout(&RequestBody::DescribeBoardBackend {
                board_id: "board".parse().unwrap_or_else(|error| panic!("{error}")),
            })
            .is_none()
        );
        // Setting or creating a backend validates it against the remote before it is stored.
        assert!(
            request_timeout(&RequestBody::UpdateBoard {
                board_id,
                patch: fleet_core::board::BoardPatch::default(),
            })
            .is_none()
        );
        assert!(
            request_timeout(&RequestBody::CreateBoard {
                context_id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
                name: None,
                prefix: None,
                backend: None,
            })
            .is_none()
        );
    }

    #[test]
    fn expired_request_never_dispatches() {
        let (response, mut receiver) = oneshot::channel();
        let command = Command {
            request: Request {
                id: 42,
                body: RequestBody::DaemonPing,
            },
            response: Some(response),
            expires_at: Some(Instant::now() - Duration::from_millis(1)),
        };

        assert!(command_for_dispatch(command).is_none());
        let error = receiver
            .try_recv()
            .expect("expired command reports its failure")
            .expect_err("expired command cannot succeed");
        assert!(error.message.contains("before dispatch"));
    }

    #[test]
    fn a_second_subscribe_adds_to_the_set_the_daemon_holds() {
        let mut state = ConnectionState {
            subscriptions: vec![EventKind::JobUpdated],
            ..ConnectionState::default()
        };

        ConnectionEffect::Subscribe(vec![EventKind::Toast, EventKind::JobUpdated])
            .apply(&mut state);

        assert_eq!(
            state.subscriptions,
            vec![EventKind::JobUpdated, EventKind::Toast],
            "Subscribe is additive on the daemon, so the replayed handshake must add too"
        );
        ConnectionEffect::Unsubscribe.apply(&mut state);
        assert!(
            state.subscriptions.is_empty(),
            "Unsubscribe is the only way to clear the set"
        );
    }

    fn resize(
        terminal: u64,
        cols: u16,
        awaited: bool,
    ) -> (
        Command,
        Option<oneshot::Receiver<Result<ResponseBody, ProtoError>>>,
    ) {
        let (response, receiver) = if awaited {
            let (sender, receiver) = oneshot::channel();
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        (
            Command {
                request: Request {
                    id: terminal,
                    body: RequestBody::ResizeTerminal {
                        terminal: TerminalId(terminal),
                        cols,
                        rows: 24,
                    },
                },
                response,
                expires_at: Some(Instant::now() + REQUEST_TIMEOUT),
            },
            receiver,
        )
    }

    fn queued_resize_sizes(queued: &VecDeque<Command>) -> Vec<(TerminalId, u16)> {
        queued
            .iter()
            .filter_map(|command| match &command.request.body {
                RequestBody::ResizeTerminal { terminal, cols, .. } => Some((*terminal, *cols)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_queued_resize_is_superseded_only_for_the_same_terminal() {
        let mut queued = VecDeque::new();
        for (terminal, cols) in [(1, 80), (2, 90), (1, 100)] {
            let (command, _) = resize(terminal, cols, false);
            queue_disconnected_command(&mut queued, command);
        }

        assert_eq!(
            queued_resize_sizes(&queued),
            vec![(TerminalId(2), 90), (TerminalId(1), 100)],
            "only the same terminal's older resize is superseded"
        );
    }

    #[tokio::test]
    async fn an_awaited_resize_is_never_dropped_by_coalescing() {
        let mut queued = VecDeque::new();
        let (awaited, mut receiver) = resize(1, 80, true);
        queue_disconnected_command(&mut queued, awaited);
        let (background, _) = resize(1, 100, false);
        queue_disconnected_command(&mut queued, background);

        assert_eq!(
            queued_resize_sizes(&queued),
            vec![(TerminalId(1), 80), (TerminalId(1), 100)],
            "a caller awaiting its resize keeps its queued command"
        );
        let receiver = receiver.as_mut().expect("awaited resize has a receiver");
        assert!(
            matches!(
                receiver.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "the awaited resize is still queued, not failed"
        );
    }

    #[tokio::test]
    async fn a_failed_reconnect_attempt_expires_its_queued_commands() {
        let mut queued = VecDeque::new();
        let (response, mut receiver) = oneshot::channel();
        queued.push_back(Command {
            request: Request {
                id: 7,
                body: RequestBody::DaemonPing,
            },
            response: Some(response),
            expires_at: Some(Instant::now() - Duration::from_millis(1)),
        });
        let (live, _) = resize(1, 80, false);
        queued.push_back(live);

        discard_obsolete_commands(&mut queued);

        assert_eq!(queued.len(), 1, "the unexpired command stays queued");
        let error = receiver
            .try_recv()
            .expect("expired command reports its failure")
            .expect_err("expired command cannot succeed");
        assert!(
            error.message.contains("while reconnecting"),
            "unexpected message: {}",
            error.message
        );
    }

    #[tokio::test]
    async fn a_nearly_expired_request_is_still_written_to_the_socket() {
        let (client, peer) = UnixStream::pair().expect("socket pair");
        let transport = protocol_transport(client);
        let (mut writer, mut reader) = transport.split();
        let (response, _receiver) = oneshot::channel();
        let command = Command {
            request: Request {
                id: 1,
                body: RequestBody::DaemonPing,
            },
            response: Some(response),
            expires_at: Some(Instant::now() + Duration::from_millis(1)),
        };
        let mut pending = HashMap::new();
        let mut state = ConnectionState::default();
        let events = broadcast::Sender::new(16);
        let metadata = RwLock::new(ConnectionMetadata::default());

        let outcome = send_command(
            &mut writer,
            &mut reader,
            command,
            &mut pending,
            &mut state,
            &events,
            &metadata,
        )
        .await;

        assert_eq!(
            outcome,
            DispatchOutcome::Sent,
            "a request one millisecond from expiry still gets the connection write budget"
        );
        assert!(pending.contains_key(&1), "the request awaits its response");
        drop(peer);
    }

    #[tokio::test(start_paused = true)]
    async fn the_socket_write_budget_is_not_clamped_to_the_request_expiry() {
        // A peer that never reads parks the frame mid-write, so the deadline is the only thing
        // that can end the dispatch and the virtual clock reports which deadline was used.
        let (client, _peer) = tokio::io::duplex(1);
        let transport = protocol_transport(client);
        let (mut writer, mut reader) = transport.split();
        let (response, _receiver) = oneshot::channel();
        let command = Command {
            request: Request {
                id: 1,
                body: RequestBody::DaemonPing,
            },
            response: Some(response),
            expires_at: Some(Instant::now() + Duration::from_millis(1)),
        };
        let mut pending = HashMap::new();
        let mut state = ConnectionState::default();
        let events = broadcast::Sender::new(16);
        let metadata = RwLock::new(ConnectionMetadata::default());
        let started = Instant::now();

        let outcome = send_command(
            &mut writer,
            &mut reader,
            command,
            &mut pending,
            &mut state,
            &events,
            &metadata,
        )
        .await;

        assert_eq!(
            outcome,
            DispatchOutcome::Disconnected,
            "a write nobody drains ends by giving up on the socket"
        );
        assert!(
            started.elapsed() >= WRITE_BUDGET,
            "the write waited {:?}, not the connection's {WRITE_BUDGET:?} budget",
            started.elapsed()
        );
    }

    #[test]
    fn hello_capabilities_and_pong_identity_are_retained() {
        let hello = serde_json::json!({
            "id": 0,
            "result": {"Ok": {"type": "hello", "data": {
                "protocol": PROTOCOL_VERSION,
                "server": "fleetd test"
            }}},
            "capabilities": ["prune.reviewed_ids"]
        });
        let body = serde_json::from_value::<Response>(hello.clone())
            .expect("Hello response")
            .result
            .expect("successful Hello");
        let capabilities = negotiated_capabilities(body, &hello).expect("capabilities");
        assert_eq!(capabilities, ["prune.reviewed_ids"]);

        let (commands, _command_rx) = mpsc::channel(1);
        let events = broadcast::Sender::new(1);
        let inner = Arc::new(ClientInner {
            commands,
            events: events.clone(),
            next_id: AtomicU64::new(1),
            metadata: Arc::new(RwLock::new(ConnectionMetadata {
                capabilities: capabilities.into_iter().collect(),
                daemon_identity: None,
            })),
        });
        let client = Client {
            inner: Arc::clone(&inner),
        };
        let (reply, _answer) = oneshot::channel();
        let mut pending = HashMap::from([(
            7,
            Pending {
                effect: ConnectionEffect::None,
                response: Some(reply),
            },
        )]);
        let mut state = ConnectionState {
            capabilities: client.capabilities().into_iter().collect(),
            ..ConnectionState::default()
        };
        let pong = serde_json::json!({
            "id": 7,
            "result": {"Ok": {"type": "pong"}},
            "daemon": {"pid": 42, "bootId": "boot-42"}
        });

        assert!(!handle_incoming(
            pong,
            &mut pending,
            &mut state,
            &events,
            &inner.metadata,
        ));
        assert!(client.supports_capability("prune.reviewed_ids"));
        assert_eq!(client.daemon_pid(), Some(42));
    }
}
