//! Unix socket connection lifecycle and protocol transport.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use fleet_core::ids::TerminalId;
use fleet_proto::{
    PROTOCOL_VERSION,
    codec::FleetCodec,
    error::{ErrorKind, ProtoError},
    event::{Event, EventKind},
    paths::socket_path,
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use thiserror::Error;
use tokio::{
    net::UnixStream,
    runtime::Handle,
    sync::{broadcast, mpsc, oneshot},
    time::{Instant, timeout, timeout_at},
};
use tokio_util::codec::Framed;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
const INITIAL_RECONNECT_BACKOFF: Duration = Duration::from_millis(50);
const MAX_RECONNECT_BACKOFF: Duration = Duration::from_secs(2);
const COMMAND_CAPACITY: usize = 256;
const EVENT_CAPACITY: usize = 1_024;

type Transport = Framed<UnixStream, FleetCodec<Request, Value>>;

#[derive(Debug)]
struct Established {
    transport: Transport,
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
}

#[derive(Debug)]
struct Command {
    request: Request,
    response: Option<oneshot::Sender<Result<ResponseBody, ProtoError>>>,
    expires_at: Option<Instant>,
}

#[derive(Debug)]
struct Pending {
    body: RequestBody,
    response: Option<oneshot::Sender<Result<ResponseBody, ProtoError>>>,
}

#[derive(Debug, Clone, Copy)]
struct Attachment {
    cols: u16,
    rows: u16,
}

#[derive(Debug)]
struct ConnectionState {
    subscriptions: Vec<EventKind>,
    attachments: HashMap<TerminalId, Attachment>,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            subscriptions: all_event_kinds(),
            attachments: HashMap::new(),
        }
    }
}

impl Client {
    /// Connects to `home/fleetd.sock`, negotiates protocol version one, and starts the connection actor.
    pub async fn connect(home: impl AsRef<Path>) -> Result<Self, ConnectError> {
        let home = home.as_ref().to_path_buf();
        let state = ConnectionState::default();
        let established = establish(&home, &state).await?;
        let (commands, command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let inner = Arc::new(ClientInner {
            commands,
            events: events.clone(),
            next_id: AtomicU64::new(2),
        });
        tokio::spawn(run_connection(home, established, state, command_rx, events));
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
    established: Established,
    mut state: ConnectionState,
    mut commands: mpsc::Receiver<Command>,
    events: broadcast::Sender<Event>,
) {
    let mut transport = established.transport;
    let mut pending = HashMap::<u64, Pending>::new();
    let mut queued = VecDeque::<Command>::new();
    let mut backoff = INITIAL_RECONNECT_BACKOFF;

    if publish_events(established.buffered_events, &events) {
        return;
    }

    loop {
        while let Some(command) = queued.pop_front() {
            if command_is_expired(&command) {
                fail_command(command, "Fleet daemon request timed out while reconnecting");
                continue;
            }
            if let Err(error) = send_command(&mut transport, command, &mut pending).await {
                tracing::debug!(%error, "Fleet daemon connection was lost while sending");
                fail_pending(&mut pending, "Fleet daemon connection was lost");
                match reconnect(
                    &home,
                    &state,
                    &mut commands,
                    &mut queued,
                    &mut backoff,
                    &events,
                )
                .await
                {
                    Some(new_transport) => transport = new_transport,
                    None => return,
                }
                continue;
            }
        }

        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else {
                    fail_pending(&mut pending, "Fleet client was dropped");
                    return;
                };
                if let Err(error) = send_command(&mut transport, command, &mut pending).await {
                    tracing::debug!(%error, "Fleet daemon connection was lost while sending");
                    fail_pending(&mut pending, "Fleet daemon connection was lost");
                    match reconnect(&home, &state, &mut commands, &mut queued, &mut backoff, &events).await {
                        Some(new_transport) => transport = new_transport,
                        None => return,
                    }
                }
            }
            incoming = transport.next() => {
                match incoming {
                    Some(Ok(value)) => {
                        if handle_incoming(value, &mut pending, &mut state, &events) {
                            fail_pending(&mut pending, "Fleet daemon is shutting down");
                            return;
                        }
                    }
                    Some(Err(error)) => {
                        tracing::debug!(%error, "Fleet daemon connection decoding failed");
                        fail_pending(&mut pending, "Fleet daemon connection was lost");
                        match reconnect(&home, &state, &mut commands, &mut queued, &mut backoff, &events).await {
                            Some(new_transport) => transport = new_transport,
                            None => return,
                        }
                    }
                    None => {
                        fail_pending(&mut pending, "Fleet daemon connection was lost");
                        match reconnect(&home, &state, &mut commands, &mut queued, &mut backoff, &events).await {
                            Some(new_transport) => transport = new_transport,
                            None => return,
                        }
                    }
                }
            }
        }
    }
}

async fn send_command(
    transport: &mut Transport,
    command: Command,
    pending: &mut HashMap<u64, Pending>,
) -> Result<(), fleet_proto::codec::CodecError> {
    let id = command.request.id;
    let body = command.request.body.clone();
    pending.insert(
        id,
        Pending {
            body,
            response: command.response,
        },
    );
    transport.send(command.request).await
}

fn handle_incoming(
    value: Value,
    pending: &mut HashMap<u64, Pending>,
    state: &mut ConnectionState,
    events: &broadcast::Sender<Event>,
) -> bool {
    if value.get("id").is_some() {
        match serde_json::from_value::<Response>(value) {
            Ok(response) => {
                if let Some(request) = pending.remove(&response.id) {
                    if response.result.is_ok() {
                        update_connection_state(state, &request.body);
                    }
                    let shutting_down = matches!(response.result, Ok(ResponseBody::ShuttingDown));
                    if let Some(sender) = request.response {
                        let _ = sender.send(response.result);
                    }
                    return shutting_down;
                }
            }
            Err(error) => tracing::warn!(%error, "ignored malformed Fleet response"),
        }
        return false;
    }

    match serde_json::from_value::<Event>(value) {
        Ok(event) => {
            let shutting_down = event == Event::DaemonShuttingDown;
            let _ = events.send(event);
            shutting_down
        }
        Err(error) => {
            tracing::warn!(%error, "ignored malformed Fleet event");
            false
        }
    }
}

fn update_connection_state(state: &mut ConnectionState, body: &RequestBody) {
    match body {
        RequestBody::Subscribe { events } => state.subscriptions.clone_from(events),
        RequestBody::Unsubscribe => state.subscriptions.clear(),
        RequestBody::AttachTerminal {
            terminal,
            cols,
            rows,
        } => {
            state.attachments.insert(
                *terminal,
                Attachment {
                    cols: *cols,
                    rows: *rows,
                },
            );
        }
        RequestBody::DetachTerminal { terminal } | RequestBody::CloseTerminal { terminal } => {
            state.attachments.remove(terminal);
        }
        RequestBody::ResizeTerminal {
            terminal,
            cols,
            rows,
        } => {
            if let Some(attachment) = state.attachments.get_mut(terminal) {
                attachment.cols = *cols;
                attachment.rows = *rows;
            }
        }
        _ => {}
    }
}

fn request_timeout(body: &RequestBody) -> Option<Duration> {
    if matches!(
        body,
        RequestBody::CreateWorktree { .. } | RequestBody::CreateWorktreeFromPr { .. }
    ) {
        None
    } else {
        Some(REQUEST_TIMEOUT)
    }
}

async fn reconnect(
    home: &Path,
    state: &ConnectionState,
    commands: &mut mpsc::Receiver<Command>,
    queued: &mut VecDeque<Command>,
    backoff: &mut Duration,
    events: &broadcast::Sender<Event>,
) -> Option<Transport> {
    loop {
        let sleep = tokio::time::sleep(*backoff);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => queued.push_back(command),
                    None => return None,
                },
                () = &mut sleep => break,
            }
        }

        match establish(home, state).await {
            Ok(established) => {
                *backoff = INITIAL_RECONNECT_BACKOFF;
                if publish_events(established.buffered_events, events) {
                    return None;
                }
                return Some(established.transport);
            }
            Err(error) => {
                tracing::debug!(%error, "Fleet daemon reconnect attempt failed");
                *backoff = (*backoff * 2).min(MAX_RECONNECT_BACKOFF);
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
        }
    }
}

async fn establish(home: &Path, state: &ConnectionState) -> Result<Established, ConnectError> {
    let socket = UnixStream::connect(socket_path(home)).await?;
    let mut transport = Framed::new(socket, FleetCodec::new());
    let mut buffered_events = Vec::new();
    exchange(
        &mut transport,
        &mut buffered_events,
        Request {
            id: 0,
            body: RequestBody::Hello {
                protocol: PROTOCOL_VERSION,
                client: format!("fleet-client/{}", env!("CARGO_PKG_VERSION")),
            },
        },
        |body| match body {
            ResponseBody::Hello { protocol, .. } if protocol == PROTOCOL_VERSION => Ok(()),
            other => Err(ConnectError::InvalidHandshake(format!("{other:?}"))),
        },
    )
    .await?;

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

    for (offset, (terminal, attachment)) in state.attachments.iter().enumerate() {
        let id = u64::try_from(offset).map_or(u64::MAX, |value| value.saturating_add(2));
        exchange(
            &mut transport,
            &mut buffered_events,
            Request {
                id,
                body: RequestBody::AttachTerminal {
                    terminal: *terminal,
                    cols: attachment.cols,
                    rows: attachment.rows,
                },
            },
            expect_ack,
        )
        .await?;
    }
    Ok(Established {
        transport,
        buffered_events,
    })
}

async fn exchange(
    transport: &mut Transport,
    buffered_events: &mut Vec<Event>,
    request: Request,
    validate: impl FnOnce(ResponseBody) -> Result<(), ConnectError>,
) -> Result<(), ConnectError> {
    let expected_id = request.id;
    timeout(HANDSHAKE_TIMEOUT, transport.send(request))
        .await
        .map_err(|_| ConnectError::Timeout)??;
    loop {
        let value = timeout(HANDSHAKE_TIMEOUT, transport.next())
            .await
            .map_err(|_| ConnectError::Timeout)?
            .ok_or_else(|| ConnectError::InvalidHandshake("connection closed".to_owned()))??;
        if value.get("id").is_none() {
            let event = serde_json::from_value(value)
                .map_err(|error| ConnectError::InvalidHandshake(error.to_string()))?;
            buffered_events.push(event);
            continue;
        }
        let response: Response = serde_json::from_value(value)
            .map_err(|error| ConnectError::InvalidHandshake(error.to_string()))?;
        if response.id != expected_id {
            return Err(ConnectError::InvalidHandshake(format!(
                "expected response id {expected_id}, got {}",
                response.id
            )));
        }
        return validate(response.result?);
    }
}

fn publish_events(buffered: Vec<Event>, events: &broadcast::Sender<Event>) -> bool {
    let mut shutting_down = false;
    for event in buffered {
        shutting_down |= event == Event::DaemonShuttingDown;
        let _ = events.send(event);
    }
    shutting_down
}

fn expect_ack(body: ResponseBody) -> Result<(), ConnectError> {
    match body {
        ResponseBody::Ack => Ok(()),
        other => Err(ConnectError::InvalidHandshake(format!(
            "expected acknowledgement, got {other:?}"
        ))),
    }
}

fn all_event_kinds() -> Vec<EventKind> {
    vec![
        EventKind::SnapshotChanged,
        EventKind::JobUpdated,
        EventKind::SessionChanged,
        EventKind::TerminalFrame,
        EventKind::TerminalExited,
        EventKind::TerminalTitle,
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
        let repo = RepoId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}"));
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
        assert_eq!(
            request_timeout(&RequestBody::DaemonPing),
            Some(REQUEST_TIMEOUT)
        );
    }
}
