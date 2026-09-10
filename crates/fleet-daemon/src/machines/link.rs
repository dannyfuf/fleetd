//! Persistent, reconnecting links to remote Fleet daemons.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use fleet_core::ids::HostId;
use fleet_proto::{
    PROTOCOL_VERSION, REMOTE_MACHINES_CAPABILITY,
    codec::{CodecError, FleetCodec},
    error::{ErrorKind, ProtoError},
    event::{Event, EventKind},
    request::{ClientKind, HelloClient, Request, RequestBody},
    response::{HelloResponse, Response, ResponseBody},
    snapshot::{LinkState, Snapshot},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::{
    sync::{Notify, broadcast, mpsc, oneshot, watch},
    task::JoinHandle,
    time::{Instant, sleep, timeout_at},
};
use tokio_util::codec::Framed;

use crate::{DaemonError, DaemonResult};

use super::{AsyncDuplex, MachineProvider};

const COMMAND_CAPACITY: usize = 256;
const EVENT_CAPACITY: usize = 1_024;

type Transport = Framed<Box<dyn AsyncDuplex>, FleetCodec<Value, Value>>;

/// Remote Hello metadata retained by an endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteHello {
    pub version: String,
    pub daemon_id: String,
    pub build_commit: Option<String>,
    pub capabilities: Vec<String>,
}

/// Backoff and handshake bounds for a remote link.
#[derive(Debug, Clone, Copy)]
pub struct LinkOptions {
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    pub hello_timeout: Duration,
}

impl Default for LinkOptions {
    fn default() -> Self {
        Self {
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            hello_timeout: Duration::from_secs(10),
        }
    }
}

/// Request and event surface exposed by a remote daemon connection.
#[async_trait]
pub trait RemoteEndpoint: Send + Sync {
    fn host(&self) -> &HostId;
    fn state(&self) -> LinkState;
    fn hello(&self) -> Option<RemoteHello>;
    /// Most recent complete snapshot retained before this endpoint became ready.
    fn last_snapshot_seen(&self) -> Option<Snapshot> {
        None
    }
    async fn request(&self, body: RequestBody) -> DaemonResult<ResponseBody>;
    /// Wakes a link that is sleeping in reconnect backoff so its next attempt runs immediately
    /// and its backoff restarts from the configured floor.
    ///
    /// Implementations must treat this as a no-op for a link that is already connected.
    fn nudge_reconnect(&self) {}
    fn events(&self) -> broadcast::Receiver<Event>;
    fn state_changes(&self) -> watch::Receiver<LinkState>;
    async fn close(&self);
}

struct Command {
    request: Request,
    response: oneshot::Sender<DaemonResult<ResponseBody>>,
}

/// A framed remote-daemon link with request correlation and automatic reconnection.
pub struct RemoteLink {
    provider: Arc<dyn MachineProvider>,
    local_daemon_id: HostId,
    options: LinkOptions,
    commands_tx: mpsc::Sender<Command>,
    commands_rx: Mutex<Option<mpsc::Receiver<Command>>>,
    events_tx: broadcast::Sender<Event>,
    state_tx: watch::Sender<LinkState>,
    hello: Arc<RwLock<Option<RemoteHello>>>,
    last_snapshot: Arc<RwLock<Option<Snapshot>>>,
    last_error: Arc<RwLock<Option<String>>>,
    next_id: AtomicU64,
    started: AtomicBool,
    closed: Arc<AtomicBool>,
    shutdown_tx: watch::Sender<bool>,
    reconnect: Arc<Notify>,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl RemoteLink {
    /// Creates a link for registry callers that do not yet provide their daemon identity.
    /// Federation composition should prefer [`Self::new_with_daemon_id`].
    #[must_use]
    pub fn new(provider: Arc<dyn MachineProvider>, options: LinkOptions) -> Arc<Self> {
        let fallback_id = provider.id().clone();
        Self::new_with_daemon_id(provider, fallback_id, options)
    }

    /// Creates a reconnecting link whose proxy Hello identifies the local daemon.
    #[must_use]
    pub fn new_with_daemon_id(
        provider: Arc<dyn MachineProvider>,
        local_daemon_id: HostId,
        options: LinkOptions,
    ) -> Arc<Self> {
        let (commands_tx, commands_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (events_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let (state_tx, _) = watch::channel(LinkState::Down);
        let (shutdown_tx, _) = watch::channel(false);
        Arc::new(Self {
            provider,
            local_daemon_id,
            options,
            commands_tx,
            commands_rx: Mutex::new(Some(commands_rx)),
            events_tx,
            state_tx,
            hello: Arc::new(RwLock::new(None)),
            last_snapshot: Arc::new(RwLock::new(None)),
            last_error: Arc::new(RwLock::new(None)),
            next_id: AtomicU64::new(3),
            started: AtomicBool::new(false),
            closed: Arc::new(AtomicBool::new(false)),
            shutdown_tx,
            reconnect: Arc::new(Notify::new()),
            task: tokio::sync::Mutex::new(None),
        })
    }

    /// Opens the initial stream and starts the reconnecting connection actor.
    pub async fn connect(&self) -> DaemonResult<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(DaemonError::Remote("remote link is closed".to_owned()));
        }
        if self.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let commands = lock(&self.commands_rx).take().ok_or_else(|| {
            DaemonError::Remote("remote link connection actor is unavailable".to_owned())
        })?;
        self.set_state(LinkState::Connecting);
        let initial = establish(
            &self.provider,
            &self.local_daemon_id,
            self.options.hello_timeout,
        )
        .await;
        let initial_error = match &initial {
            Ok(established) => {
                self.set_ready(established);
                None
            }
            Err(error) => {
                self.set_down(error.to_string());
                Some(error.to_string())
            }
        };
        let actor = LinkActor {
            provider: Arc::clone(&self.provider),
            local_daemon_id: self.local_daemon_id.clone(),
            options: self.options,
            commands,
            events: self.events_tx.clone(),
            state: self.state_tx.clone(),
            hello: Arc::clone(&self.hello),
            last_snapshot: Arc::clone(&self.last_snapshot),
            last_error: Arc::clone(&self.last_error),
            closed: Arc::clone(&self.closed),
            shutdown: self.shutdown_tx.subscribe(),
            reconnect: Arc::clone(&self.reconnect),
        };
        let task = tokio::spawn(run_link(actor, initial.ok()));
        *self.task.lock().await = Some(task);
        match initial_error {
            Some(error) => Err(DaemonError::Remote(error)),
            None => Ok(()),
        }
    }

    /// Most recent transport or handshake error, if the link is down.
    #[must_use]
    pub fn last_error(&self) -> Option<String> {
        read(&self.last_error).clone()
    }

    fn set_state(&self, state: LinkState) {
        self.state_tx.send_replace(state);
    }

    fn set_ready(&self, established: &Established) {
        *write(&self.hello) = Some(established.hello.clone());
        *write(&self.last_snapshot) = Some(established.snapshot.clone());
        *write(&self.last_error) = None;
        for event in &established.buffered_events {
            let _ = self.events_tx.send(event.clone());
        }
        self.set_state(LinkState::Ready);
        self.emit_link_changed(
            LinkState::Ready,
            Some(established.hello.version.clone()),
            None,
        );
    }

    fn set_down(&self, error: String) {
        let version = read(&self.hello)
            .as_ref()
            .map(|hello| hello.version.clone());
        *write(&self.hello) = None;
        *write(&self.last_error) = Some(error.clone());
        if self.state_tx.send_replace(LinkState::Down) != LinkState::Down {
            self.emit_link_changed(LinkState::Down, version, Some(error));
        }
    }

    fn emit_link_changed(&self, link: LinkState, version: Option<String>, error: Option<String>) {
        let _ = self.events_tx.send(Event::HostLinkChanged {
            host: self.provider.id().clone(),
            link,
            version,
            error,
        });
    }
}

#[async_trait]
impl RemoteEndpoint for RemoteLink {
    fn host(&self) -> &HostId {
        self.provider.id()
    }

    fn state(&self) -> LinkState {
        *self.state_tx.borrow()
    }

    fn hello(&self) -> Option<RemoteHello> {
        read(&self.hello).clone()
    }

    fn last_snapshot_seen(&self) -> Option<Snapshot> {
        read(&self.last_snapshot).clone()
    }

    async fn request(&self, body: RequestBody) -> DaemonResult<ResponseBody> {
        if !self.started.load(Ordering::Acquire) {
            return Err(DaemonError::Remote(
                "remote link has not been connected".to_owned(),
            ));
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(DaemonError::Remote("remote link is closed".to_owned()));
        }
        let mut states = self.state_tx.subscribe();
        if *states.borrow() != LinkState::Ready {
            return Err(unreachable(self.provider.id()));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (response, receiver) = oneshot::channel();
        let command = Command {
            request: Request { id, body },
            response,
        };
        tokio::select! {
            sent = self.commands_tx.send(command) => {
                sent.map_err(|_| DaemonError::Remote("remote link is closed".to_owned()))?;
            }
            changed = states.changed() => {
                let _ = changed;
                return Err(unreachable(self.provider.id()));
            }
        }
        tokio::select! {
            result = receiver => {
                result.map_err(|_| DaemonError::Remote("remote link is closed".to_owned()))?
            }
            changed = states.changed() => {
                let _ = changed;
                Err(unreachable(self.provider.id()))
            }
        }
    }

    fn nudge_reconnect(&self) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        // A permit is retained when the actor is not currently sleeping, so a nudge that races a
        // disconnect still shortens the following backoff instead of being lost.
        self.reconnect.notify_one();
    }

    fn events(&self) -> broadcast::Receiver<Event> {
        self.events_tx.subscribe()
    }

    fn state_changes(&self) -> watch::Receiver<LinkState> {
        self.state_tx.subscribe()
    }

    async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let _ = self.shutdown_tx.send(true);
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
        self.set_down("remote link is closed".to_owned());
    }
}

struct Established {
    transport: Transport,
    hello: RemoteHello,
    snapshot: Snapshot,
    buffered_events: Vec<Event>,
}

struct LinkActor {
    provider: Arc<dyn MachineProvider>,
    local_daemon_id: HostId,
    options: LinkOptions,
    commands: mpsc::Receiver<Command>,
    events: broadcast::Sender<Event>,
    state: watch::Sender<LinkState>,
    hello: Arc<RwLock<Option<RemoteHello>>>,
    last_snapshot: Arc<RwLock<Option<Snapshot>>>,
    last_error: Arc<RwLock<Option<String>>>,
    closed: Arc<AtomicBool>,
    shutdown: watch::Receiver<bool>,
    reconnect: Arc<Notify>,
}

async fn run_link(actor: LinkActor, mut established: Option<Established>) {
    let LinkActor {
        provider,
        local_daemon_id,
        options,
        mut commands,
        events,
        state,
        hello,
        last_snapshot,
        last_error,
        closed,
        mut shutdown,
        reconnect,
    } = actor;
    let mut pending = HashMap::<u64, oneshot::Sender<DaemonResult<ResponseBody>>>::new();
    let backoff_floor = options.backoff_min.min(options.backoff_max);
    let mut backoff = backoff_floor;
    loop {
        if *shutdown.borrow() || closed.load(Ordering::Acquire) {
            fail_pending(&mut pending, "remote link is closed");
            return;
        }
        if established.is_none() {
            let mut nudged = false;
            tokio::select! {
                () = sleep(backoff) => {}
                () = reconnect.notified() => nudged = true,
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        fail_pending(&mut pending, "remote link is closed");
                        return;
                    }
                    continue;
                }
            }
            if nudged {
                backoff = backoff_floor;
            }
            state.send_replace(LinkState::Connecting);
            match establish(&provider, &local_daemon_id, options.hello_timeout).await {
                Ok(connection) => {
                    backoff = backoff_floor;
                    *write(&hello) = Some(connection.hello.clone());
                    *write(&last_snapshot) = Some(connection.snapshot.clone());
                    *write(&last_error) = None;
                    for event in &connection.buffered_events {
                        let _ = events.send(event.clone());
                    }
                    state.send_replace(LinkState::Ready);
                    let _ = events.send(Event::HostLinkChanged {
                        host: provider.id().clone(),
                        link: LinkState::Ready,
                        version: Some(connection.hello.version.clone()),
                        error: None,
                    });
                    established = Some(connection);
                }
                Err(error) => {
                    set_shared_down(
                        provider.id(),
                        &events,
                        &state,
                        &hello,
                        &last_error,
                        error.to_string(),
                    );
                    backoff = doubled(backoff, options.backoff_max);
                    continue;
                }
            }
        }

        let mut connection = established.take().expect("established link");
        connection.buffered_events.clear();
        let reason = connected_loop(
            &mut connection.transport,
            &mut commands,
            &events,
            &mut pending,
            &mut shutdown,
        )
        .await;
        fail_pending(&mut pending, &reason);
        set_shared_down(provider.id(), &events, &state, &hello, &last_error, reason);
    }
}

async fn connected_loop(
    transport: &mut Transport,
    commands: &mut mpsc::Receiver<Command>,
    events: &broadcast::Sender<Event>,
    pending: &mut HashMap<u64, oneshot::Sender<DaemonResult<ResponseBody>>>,
    shutdown: &mut watch::Receiver<bool>,
) -> String {
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return "remote link is closed".to_owned();
                }
            }
            command = commands.recv() => {
                let Some(command) = command else {
                    return "remote link request channel closed".to_owned();
                };
                if command.response.is_closed() {
                    continue;
                }
                let id = command.request.id;
                pending.insert(id, command.response);
                let encoded = match serde_json::to_value(command.request) {
                    Ok(encoded) => encoded,
                    Err(error) => {
                        if let Some(response) = pending.remove(&id) {
                            let _ = response.send(Err(DaemonError::Json(error)));
                        }
                        continue;
                    }
                };
                if let Err(error) = transport.send(encoded).await {
                    if let Some(response) = pending.remove(&id) {
                        let _ = response.send(Err(DaemonError::Remote(error.to_string())));
                    }
                    return format!("remote stream write failed: {error}");
                }
            }
            incoming = transport.next() => match incoming {
                Some(Ok(value)) => handle_incoming(value, pending, events),
                Some(Err(error)) => return format!("remote stream decode failed: {error}"),
                None => return "remote stream closed".to_owned(),
            }
        }
    }
}

fn handle_incoming(
    value: Value,
    pending: &mut HashMap<u64, oneshot::Sender<DaemonResult<ResponseBody>>>,
    events: &broadcast::Sender<Event>,
) {
    if value.get("id").is_some() {
        match serde_json::from_value::<Response>(value) {
            Ok(response) => {
                if let Some(sender) = pending.remove(&response.id) {
                    let _ = sender.send(response.result.map_err(proto_error));
                }
            }
            Err(error) => tracing::warn!(%error, "ignored malformed remote response"),
        }
    } else {
        match serde_json::from_value::<Event>(value) {
            Ok(event) => {
                let _ = events.send(event);
            }
            Err(error) => tracing::warn!(%error, "ignored malformed remote event"),
        }
    }
}

async fn establish(
    provider: &Arc<dyn MachineProvider>,
    local_daemon_id: &HostId,
    hello_timeout: Duration,
) -> DaemonResult<Established> {
    let deadline = Instant::now() + hello_timeout;
    let first = timeout_at(
        deadline,
        establish_before(provider, local_daemon_id, deadline, false),
    )
    .await
    .map_err(|_| DaemonError::Timeout(format!("{} remote Hello", provider.id())))?;
    match first {
        Err(error) if should_retry_legacy_hello(&error) => {
            let deadline = Instant::now() + hello_timeout;
            timeout_at(
                deadline,
                establish_before(provider, local_daemon_id, deadline, true),
            )
            .await
            .map_err(|_| DaemonError::Timeout(format!("{} legacy remote Hello", provider.id())))?
        }
        result => result,
    }
}

async fn establish_before(
    provider: &Arc<dyn MachineProvider>,
    local_daemon_id: &HostId,
    deadline: Instant,
    legacy_hello: bool,
) -> DaemonResult<Established> {
    let stream = provider.open_stream().await.map_err(DaemonError::from)?;
    let mut transport = Framed::new(stream, FleetCodec::new());
    let mut buffered_events = Vec::new();
    let hello_request = if legacy_hello {
        serde_json::json!({
            "id": 0,
            "body": {
                "type": "hello",
                "protocol": PROTOCOL_VERSION,
                "client": "fleet",
            },
        })
    } else {
        serde_json::to_value(Request {
            id: 0,
            body: RequestBody::Hello {
                protocol: PROTOCOL_VERSION,
                client: HelloClient {
                    kind: ClientKind::Proxy,
                    host_id: Some(local_daemon_id.clone()),
                },
            },
        })?
    };
    transport.send(hello_request).await.map_err(codec_error)?;
    let hello_value = response_before(&mut transport, 0, deadline, &mut buffered_events).await?;
    let envelope: HelloResponse = serde_json::from_value(hello_value)
        .map_err(|error| DaemonError::Protocol(format!("invalid remote Hello: {error}")))?;
    let (protocol, version) = match envelope.response.result {
        Ok(ResponseBody::Hello { protocol, server }) => (protocol, server),
        Ok(other) => {
            return Err(DaemonError::Protocol(format!(
                "invalid remote Hello response: {other:?}"
            )));
        }
        Err(error) if error.message.contains("unsupported protocol") => {
            return Err(version_mismatch(provider.id(), &error.message));
        }
        Err(error) => return Err(proto_error(error)),
    };
    if protocol != PROTOCOL_VERSION {
        return Err(DaemonError::Protocol(format!(
            "protocol version mismatch: local {PROTOCOL_VERSION}, remote {protocol}; run fleet host bootstrap {}",
            provider.id()
        )));
    }
    let hello = RemoteHello {
        version,
        daemon_id: envelope.daemon_id,
        build_commit: envelope.build_commit,
        capabilities: envelope.capabilities,
    };
    if !hello
        .capabilities
        .iter()
        .any(|capability| capability == REMOTE_MACHINES_CAPABILITY)
    {
        return Err(DaemonError::Unsupported(format!(
            "host {} does not advertise the {REMOTE_MACHINES_CAPABILITY} capability",
            provider.id()
        )));
    }

    transport
        .send(serde_json::to_value(Request {
            id: 1,
            body: RequestBody::Subscribe {
                events: all_event_kinds(),
            },
        })?)
        .await
        .map_err(codec_error)?;
    let subscribe = response_before(&mut transport, 1, deadline, &mut buffered_events).await?;
    let response: Response = serde_json::from_value(subscribe)
        .map_err(|error| DaemonError::Protocol(format!("invalid Subscribe response: {error}")))?;
    match response.result {
        Ok(ResponseBody::Ack) => {}
        Ok(other) => {
            return Err(DaemonError::Protocol(format!(
                "invalid Subscribe response: {other:?}"
            )));
        }
        Err(error) => return Err(proto_error(error)),
    }

    transport
        .send(serde_json::to_value(Request {
            id: 2,
            body: RequestBody::GetSnapshot,
        })?)
        .await
        .map_err(codec_error)?;
    let snapshot = response_before(&mut transport, 2, deadline, &mut buffered_events).await?;
    let response: Response = serde_json::from_value(snapshot)
        .map_err(|error| DaemonError::Protocol(format!("invalid GetSnapshot response: {error}")))?;
    let snapshot = match response.result {
        Ok(ResponseBody::Snapshot(snapshot)) => snapshot,
        Ok(other) => {
            return Err(DaemonError::Protocol(format!(
                "invalid GetSnapshot response: {other:?}"
            )));
        }
        Err(error) => return Err(proto_error(error)),
    };

    Ok(Established {
        transport,
        hello,
        snapshot,
        buffered_events,
    })
}

fn unreachable(host: &HostId) -> DaemonError {
    DaemonError::Remote(format!("host {host} is unreachable"))
}

async fn response_before(
    transport: &mut Transport,
    expected_id: u64,
    deadline: Instant,
    buffered_events: &mut Vec<Event>,
) -> DaemonResult<Value> {
    loop {
        let value = timeout_at(deadline, transport.next())
            .await
            .map_err(|_| DaemonError::Timeout("remote Hello handshake".to_owned()))?
            .ok_or_else(|| DaemonError::Protocol("remote stream closed during Hello".to_owned()))?
            .map_err(codec_error)?;
        if value.get("id").is_none() {
            buffered_events.push(serde_json::from_value(value)?);
            continue;
        }
        let response: Response = serde_json::from_value(value.clone())?;
        if response.id != expected_id {
            return Err(DaemonError::Protocol(format!(
                "expected remote response id {expected_id}, got {}",
                response.id
            )));
        }
        return Ok(value);
    }
}

fn version_mismatch(host: &HostId, remote_message: &str) -> DaemonError {
    DaemonError::Protocol(format!(
        "protocol version mismatch: local {PROTOCOL_VERSION}, remote {remote_message}; run fleet host bootstrap {host}"
    ))
}

fn should_retry_legacy_hello(error: &DaemonError) -> bool {
    matches!(
        error,
        DaemonError::Protocol(message)
            if message.contains("closed during Hello")
                || message.contains("invalid protocol JSON")
    )
}

fn codec_error(error: CodecError) -> DaemonError {
    match error {
        CodecError::Io(error) if error.kind() == std::io::ErrorKind::HostUnreachable => {
            DaemonError::Remote(error.to_string())
        }
        error => DaemonError::Protocol(error.to_string()),
    }
}

fn proto_error(error: ProtoError) -> DaemonError {
    match error.kind {
        ErrorKind::NotFound => DaemonError::NotFound(error.message),
        ErrorKind::Conflict => DaemonError::Conflict(error.message),
        ErrorKind::Validation => DaemonError::Validation(error.message),
        ErrorKind::Cancelled => DaemonError::Cancelled,
        ErrorKind::Unsupported => DaemonError::Unsupported(error.message),
        ErrorKind::Remote => DaemonError::Remote(error.message),
        ErrorKind::Git => DaemonError::Git(error.message),
        ErrorKind::Github => DaemonError::Github(error.message),
        ErrorKind::Fs | ErrorKind::Tmux | ErrorKind::Unknown => {
            DaemonError::Protocol(error.message)
        }
    }
}

fn all_event_kinds() -> Vec<EventKind> {
    vec![
        EventKind::Agent,
        EventKind::AgentSummary,
        EventKind::BoardChanged,
        EventKind::WatchStarted,
        EventKind::WatchOutput,
        EventKind::WatchExited,
        EventKind::WatchDismissed,
        EventKind::SnapshotChanged,
        EventKind::JobUpdated,
        EventKind::SessionChanged,
        EventKind::AgentActivityChanged,
        EventKind::TerminalFrame,
        EventKind::TerminalExited,
        EventKind::TerminalTitle,
        EventKind::Toast,
        EventKind::DaemonShuttingDown,
    ]
}

fn doubled(backoff: Duration, maximum: Duration) -> Duration {
    backoff.saturating_mul(2).min(maximum)
}

fn fail_pending(
    pending: &mut HashMap<u64, oneshot::Sender<DaemonResult<ResponseBody>>>,
    message: &str,
) {
    for (_, sender) in pending.drain() {
        let _ = sender.send(Err(DaemonError::Remote(message.to_owned())));
    }
}

fn set_shared_down(
    host: &HostId,
    events: &broadcast::Sender<Event>,
    state: &watch::Sender<LinkState>,
    hello: &RwLock<Option<RemoteHello>>,
    last_error: &RwLock<Option<String>>,
    error: String,
) {
    let version = read(hello).as_ref().map(|hello| hello.version.clone());
    *write(hello) = None;
    *write(last_error) = Some(error.clone());
    if state.send_replace(LinkState::Down) != LinkState::Down {
        let _ = events.send(Event::HostLinkChanged {
            host: host.clone(),
            link: LinkState::Down,
            version,
            error: Some(error),
        });
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::machines::{ExecOutput, MachineAddress, MachineError, ProbeReport};

    /// Provider whose stream never opens, so every reconnect attempt fails immediately.
    struct UnreachableProvider {
        id: HostId,
        attempts: AtomicUsize,
    }

    impl UnreachableProvider {
        fn new() -> Self {
            Self {
                id: HostId::try_from("backoff-box").unwrap_or_else(|error| panic!("{error}")),
                attempts: AtomicUsize::new(0),
            }
        }

        fn attempts(&self) -> usize {
            self.attempts.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl MachineProvider for UnreachableProvider {
        fn id(&self) -> &HostId {
            &self.id
        }
        fn provider_name(&self) -> &'static str {
            "command"
        }
        async fn resolve(&self) -> Result<MachineAddress, MachineError> {
            Err(MachineError::Unreachable("offline".to_owned()))
        }
        async fn probe(&self, _timeout: Duration) -> ProbeReport {
            ProbeReport {
                reachable: false,
                latency_ms: None,
                version: None,
                error: Some("offline".to_owned()),
                stderr: None,
            }
        }
        async fn exec(
            &self,
            _argv: &[String],
            _timeout: Duration,
        ) -> Result<ExecOutput, MachineError> {
            Err(MachineError::Unreachable("offline".to_owned()))
        }
        async fn open_stream(&self) -> Result<Box<dyn AsyncDuplex>, MachineError> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            Err(MachineError::Unreachable("offline".to_owned()))
        }
        fn fleetd_binary(&self) -> &str {
            "fleetd"
        }
        fn fleet_home(&self) -> Option<&str> {
            Some("~/.fleet")
        }
    }

    #[tokio::test(start_paused = true)]
    async fn nudge_wakes_a_sleeping_link_and_restarts_backoff_from_the_floor() {
        let provider = Arc::new(UnreachableProvider::new());
        let link = RemoteLink::new(
            Arc::clone(&provider) as Arc<dyn MachineProvider>,
            LinkOptions {
                backoff_min: Duration::from_secs(1),
                backoff_max: Duration::from_secs(60),
                hello_timeout: Duration::from_secs(1),
            },
        );
        link.connect().await.expect_err("provider is unreachable");

        // Let the backoff grow well past its floor (1s, 2s, 4s, 8s, 16s, ...).
        tokio::time::sleep(Duration::from_secs(40)).await;
        let grown = provider.attempts();
        assert!(grown >= 5, "expected several failed attempts, got {grown}");

        link.nudge_reconnect();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            provider.attempts(),
            grown + 1,
            "a nudge must retry immediately instead of waiting out the grown backoff"
        );

        // With the backoff reset to the floor the next attempt lands ~2s later; without the reset
        // the link would still be sleeping for tens of seconds.
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            provider.attempts() >= grown + 2,
            "a nudge must reset the backoff to its floor"
        );

        link.close().await;
    }

    #[tokio::test]
    async fn nudge_on_a_connected_link_does_not_disturb_it() {
        let provider = Arc::new(UnreachableProvider::new());
        let link = RemoteLink::new(
            Arc::clone(&provider) as Arc<dyn MachineProvider>,
            LinkOptions::default(),
        );
        // Not connected yet: the nudge must be inert rather than panic or spawn work.
        link.nudge_reconnect();
        assert_eq!(provider.attempts(), 0);
        assert_eq!(link.state(), LinkState::Down);
    }
}
