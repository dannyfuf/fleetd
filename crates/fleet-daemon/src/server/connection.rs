//! Per-client protocol decoding, dispatch, responses, and subscriptions.

use std::{
    collections::HashSet,
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
    time::Duration,
};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use fleet_proto::{
    codec::FleetCodec,
    event::{Event, EventKind},
    request::{HelloClient, Request, RequestBody},
    response::{
        DaemonIdentity, HelloResponse, PRUNE_REVIEWED_IDS_CAPABILITY, PongResponse, Response,
        ResponseBody,
    },
};
use futures_util::{SinkExt, StreamExt, stream::FuturesUnordered};
use serde::Serialize;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
#[cfg(test)]
use tokio::sync::oneshot;
use tokio_util::{codec::Framed, sync::CancellationToken};

use crate::{DaemonError, DaemonResult, server::broadcast::BroadcastBus, services::Services};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const OUTBOUND_QUEUE_CAPACITY: usize = 64;
const MAX_PENDING_REQUESTS: usize = 64;
static DAEMON_BOOT_ID: OnceLock<String> = OnceLock::new();

/// One Unix-socket client actor with independent subscriptions and terminal attachments.
pub struct Connection {
    stream: UnixStream,
    services: Arc<Services>,
    events: BroadcastBus,
    shutdown: CancellationToken,
    #[cfg(test)]
    before_serialized_response: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
    #[cfg(test)]
    dispatch_gate: Option<CancellationToken>,
    #[cfg(test)]
    pending_high_water: Option<Arc<AtomicUsize>>,
    #[cfg(test)]
    pending_limit_reached: Option<oneshot::Sender<()>>,
    #[cfg(test)]
    last_request_read: Option<(u64, oneshot::Sender<()>)>,
}

impl Connection {
    /// Creates a connection actor for an accepted stream.
    #[must_use]
    pub fn new(
        stream: UnixStream,
        services: Arc<Services>,
        events: BroadcastBus,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            stream,
            services,
            events,
            shutdown,
            #[cfg(test)]
            before_serialized_response: None,
            #[cfg(test)]
            dispatch_gate: None,
            #[cfg(test)]
            pending_high_water: None,
            #[cfg(test)]
            pending_limit_reached: None,
            #[cfg(test)]
            last_request_read: None,
        }
    }

    #[cfg(test)]
    fn with_before_serialized_response(
        mut self,
        ready: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    ) -> Self {
        self.before_serialized_response = Some((ready, release));
        self
    }

    #[cfg(test)]
    fn with_blocked_dispatch(
        mut self,
        gate: CancellationToken,
        pending_high_water: Arc<AtomicUsize>,
        pending_limit_reached: oneshot::Sender<()>,
        last_request_id: u64,
        last_request_read: oneshot::Sender<()>,
    ) -> Self {
        self.dispatch_gate = Some(gate);
        self.pending_high_water = Some(pending_high_water);
        self.pending_limit_reached = Some(pending_limit_reached);
        self.last_request_read = Some((last_request_id, last_request_read));
        self
    }

    /// Runs Hello negotiation followed by request and event multiplexing.
    pub async fn run(self) -> DaemonResult<()> {
        #[cfg(test)]
        let mut before_serialized_response = self.before_serialized_response;
        #[cfg(test)]
        let dispatch_gate = self.dispatch_gate;
        #[cfg(test)]
        let pending_high_water = self.pending_high_water;
        #[cfg(test)]
        let mut pending_limit_reached = self.pending_limit_reached;
        #[cfg(test)]
        let mut last_request_read = self.last_request_read;
        let mut framed = Framed::new(self.stream, FleetCodec::<Outbound, Request>::new());
        let Some(client) =
            negotiate_hello_with_timeout(&mut framed, HANDSHAKE_TIMEOUT, &self.services).await?
        else {
            return Ok(());
        };

        let (writer, mut reader) = framed.split();
        let (outbound, outbound_rx) = mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
        let mut writer = tokio::spawn(run_writer(writer, outbound_rx));
        let mut writer_finished = false;

        let watch_owner = self.services.watches.owner();
        let owner_id = watch_owner.id;
        let mut subscriptions = HashSet::new();
        let mut attached = HashSet::new();
        let mut events = self.events.subscribe();
        let mut frames = self.services.sessions.subscribe_frames();
        let mut pending = FuturesUnordered::<DispatchFuture>::new();
        let mut shutdown_announced = false;
        let mut result = loop {
            // Shutdown is settled here rather than with `biased;`: biasing this select's six
            // arms in their current order would let a client that never stops sending starve
            // the two buses and manufacture the very lag the arms below have to recover from.
            // Checking the token first instead keeps the select fair while stopping a saturated
            // terminal or event stream from winning rounds after the daemon was told to stop.
            if self.shutdown.is_cancelled() {
                // `Listener::run` publishes `DaemonShuttingDown` only after its accept loop
                // has broken on this same token, by which time every connection actor has
                // left this loop, so on a signal shutdown the bus event reaches no reader.
                // A subscribed client is told the daemon is stopping rather than left to
                // infer it from a closed socket (docs/APP-CONTRACTS.md §4).
                if !shutdown_announced
                    && subscriptions.contains(&EventKind::DaemonShuttingDown)
                    && let Err(error) = enqueue_event(&outbound, Event::DaemonShuttingDown).await
                {
                    tracing::debug!(%error, "client missed the daemon shutdown notice");
                }
                break Ok(());
            }
            tokio::select! {
                () = self.shutdown.cancelled() => continue,
                joined = &mut writer => {
                    writer_finished = true;
                    break match joined {
                        Ok(result) => result,
                        Err(error) => Err(DaemonError::Protocol(format!(
                            "client writer task failed: {error}"
                        ))),
                    };
                }
                request = reader.next(), if pending.len() < MAX_PENDING_REQUESTS => {
                    let Some(request) = request else { break Ok(()); };
                    let request = match request {
                        Ok(request) => request,
                        Err(error) => break Err(DaemonError::Protocol(error.to_string())),
                    };
                    let id = request.id;
                    let shutdown_request = match &request.body {
                        RequestBody::DaemonShutdown { stop_sessions } => Some(*stop_sessions),
                        _ => None,
                    };
                    let snapshot_changed = request_changes_snapshot(&request.body);
                    // Connection-local and terminal requests answer here; everything else joins
                    // the bounded `pending` set and may complete out of order.
                    let answered = match request.body {
                        RequestBody::Hello { .. } => Some(Err(DaemonError::Protocol(
                            "Hello is only valid as the first request".to_owned(),
                        ))),
                        RequestBody::Subscribe { events } => {
                            subscriptions.extend(events);
                            Some(Ok(ResponseBody::Ack))
                        }
                        RequestBody::Unsubscribe => {
                            subscriptions.clear();
                            Some(Ok(ResponseBody::Ack))
                        }
                        body if terminal_request_is_serialized(&body) => {
                            let result = run_terminal_request(
                                &self.services,
                                owner_id,
                                crate::services::RequestContext { client: client.clone() },
                                body,
                                &mut attached,
                            ).await;
                            if result.is_ok() && snapshot_changed {
                                self.events.request_snapshot(Arc::clone(&self.services));
                            }
                            #[cfg(test)]
                            if let Some((ready, release)) = before_serialized_response.take() {
                                let _ = ready.send(());
                                let _ = release.await;
                            }
                            Some(result)
                        }
                        body => {
                            let services = Arc::clone(&self.services);
                            let context = crate::services::RequestContext {
                                client: client.clone(),
                            };
                            #[cfg(test)]
                            let dispatch_gate = dispatch_gate.clone();
                            pending.push(Box::pin(async move {
                                #[cfg(test)]
                                if let Some(gate) = dispatch_gate {
                                    gate.cancelled().await;
                                }
                                let result = services
                                    .dispatch_routed_with_owner(body, owner_id, context)
                                    .await;
                                CompletedRequest { id, result, snapshot_changed, shutdown_request }
                            }));
                            #[cfg(test)]
                            if let Some(high_water) = &pending_high_water {
                                high_water.fetch_max(pending.len(), Ordering::Relaxed);
                            }
                            #[cfg(test)]
                            if pending.len() == MAX_PENDING_REQUESTS
                                && let Some(ready) = pending_limit_reached.take()
                            {
                                let _ignored = ready.send(());
                            }
                            None
                        }
                    };
                    #[cfg(test)]
                    if last_request_read
                        .as_ref()
                        .is_some_and(|(last_request_id, _)| id == *last_request_id)
                        && let Some((_, ready)) = last_request_read.take()
                    {
                        let _ignored = ready.send(());
                    }
                    if let Some(result) = answered
                        && let Err(error) = enqueue_response(&outbound, Response { id, result: result.map_err(Into::into) }).await
                    {
                        break Err(error);
                    }
                }
                Some(completed) = pending.next(), if !pending.is_empty() => {
                    let CompletedRequest { id, result, snapshot_changed, shutdown_request } = completed;
                    let succeeded = result.is_ok();
                    if succeeded && snapshot_changed {
                        self.events.request_snapshot(Arc::clone(&self.services));
                    }
                    if shutdown_request == Some(true) && succeeded {
                        self.services.stop_all_sessions().await;
                    }
                    if let Err(error) = enqueue_response(&outbound, Response { id, result: result.map_err(Into::into) }).await {
                        break Err(error);
                    }
                    if shutdown_request.is_some() && succeeded {
                        self.events.publish(Event::DaemonShuttingDown);
                        self.shutdown.cancel();
                        break Ok(());
                    }
                }
                event = events.recv() => {
                    match event {
                        Ok(event) if event_visible(&event, &subscriptions, &attached) => {
                            shutdown_announced |= matches!(event, Event::DaemonShuttingDown);
                            if let Err(error) = enqueue_event(&outbound, event).await {
                                break Err(error);
                            }
                        }
                        Ok(_) => {}
                        // §6: "a gap in `seq` triggers a resync from the last applied `seq`.
                        // This mirrors the terminal frame recovery rule rather than inventing a
                        // new one" — so a slow reader is resynced, exactly like the frame branch
                        // below, never disconnected. A streaming agent turn publishes an event
                        // per 16 ms delta tick into the shared bus, and dropping the connection
                        // for a few hundred milliseconds of client stall is the one failure the
                        // client's own `AgentThreadSnapshot { from_seq }` recovery was designed
                        // to make unnecessary: the next agent event it does see carries a
                        // sequence gap, which `AgentMirror::apply_or_resync` closes.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(
                                skipped,
                                "event stream lagged; resyncing the client instead of dropping it"
                            );
                            request_full_frames(
                                &self.services,
                                owner_id,
                                &client,
                                &attached,
                            ).await;
                            self.events.request_snapshot(Arc::clone(&self.services));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break Ok(()),
                    }
                }
                frame = frames.recv() => {
                    match frame {
                        Ok(frame)
                            if attached.contains(&frame.terminal)
                                && subscriptions.contains(&EventKind::TerminalFrame) =>
                        {
                            if let Err(error) = enqueue_event(&outbound, Event::TerminalFrame(frame)).await {
                                break Err(error);
                            }
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            request_full_frames(
                                &self.services,
                                owner_id,
                                &client,
                                &attached,
                            ).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break Ok(()),
                    }
                }
            }
        };
        drop(pending);
        drop(outbound);
        if !writer_finished {
            match tokio::time::timeout(SOCKET_WRITE_TIMEOUT, &mut writer).await {
                Ok(joined) => {
                    writer_finished = true;
                    if result.is_ok() {
                        result = match joined {
                            Ok(writer_result) => writer_result,
                            Err(error) => Err(DaemonError::Protocol(format!(
                                "client writer task failed: {error}"
                            ))),
                        };
                    }
                }
                Err(_) if result.is_ok() => {
                    result = Err(DaemonError::Timeout("client writer shutdown".to_owned()));
                }
                Err(_) => {}
            }
        }
        if !writer_finished {
            writer.abort();
            let _ignored = writer.await;
        }
        drop(watch_owner);
        if detach_attached_terminals(&self.services, owner_id, &client, attached).await {
            self.events.request_snapshot(Arc::clone(&self.services));
        }
        result
    }
}

/// Detaches every terminal this client held and reports whether anything was detached.
///
/// The whole loop is bounded: a terminal on a remote host reaches `RemoteEndpoint::request`,
/// which waits for an answer that a wedged-but-`Ready` link never sends. This task holds the
/// connection's admission permit until it returns, so an unbounded cleanup would retire one of
/// `MAX_CONNECTIONS` slots on every such disconnect.
async fn detach_attached_terminals(
    services: &Services,
    owner_id: u64,
    client: &HelloClient,
    attached: HashSet<fleet_core::ids::TerminalId>,
) -> bool {
    if attached.is_empty() {
        return false;
    }
    let cleanup = async {
        for terminal in attached {
            if let Err(error) = services
                .dispatch_routed_with_owner(
                    RequestBody::DetachTerminal { terminal },
                    owner_id,
                    crate::services::RequestContext {
                        client: client.clone(),
                    },
                )
                .await
                && !matches!(error, DaemonError::NotFound(_))
            {
                tracing::warn!(%error, %terminal, "failed to detach disconnected client");
            }
        }
    };
    if tokio::time::timeout(SOCKET_WRITE_TIMEOUT, cleanup)
        .await
        .is_err()
    {
        tracing::warn!("timed out detaching a disconnected client's terminals");
    }
    true
}

/// Answers the mandatory opening Hello and reports whether the session may proceed.
async fn negotiate_hello(
    framed: &mut Framed<UnixStream, FleetCodec<Outbound, Request>>,
    services: &Services,
) -> DaemonResult<Option<HelloClient>> {
    let Some(first) = framed.next().await else {
        return Ok(None);
    };
    let first = first.map_err(|error| DaemonError::Protocol(error.to_string()))?;
    let (result, client) = match first.body {
        RequestBody::Hello {
            protocol: fleet_proto::PROTOCOL_VERSION,
            client,
        } => (
            Ok(ResponseBody::Hello {
                protocol: fleet_proto::PROTOCOL_VERSION,
                server: Services::version(),
            }),
            Some(client),
        ),
        RequestBody::Hello { protocol, .. } => (
            Err(DaemonError::Unsupported(format!(
                "unsupported protocol {protocol}; expected {}",
                fleet_proto::PROTOCOL_VERSION
            ))),
            None,
        ),
        _ => (
            Err(DaemonError::Protocol(
                "Hello must be the first request".to_owned(),
            )),
            None,
        ),
    };
    let accepted = result.is_ok();
    write_response(
        framed,
        Response {
            id: first.id,
            result: result.map_err(Into::into),
        },
        services.daemon_id(),
    )
    .await?;
    Ok(accepted.then_some(client).flatten())
}

async fn negotiate_hello_with_timeout(
    framed: &mut Framed<UnixStream, FleetCodec<Outbound, Request>>,
    timeout: Duration,
    services: &Services,
) -> DaemonResult<Option<HelloClient>> {
    tokio::time::timeout(timeout, negotiate_hello(framed, services))
        .await
        .map_err(|_| DaemonError::Timeout("client Hello handshake".to_owned()))?
}

/// Runs one terminal request to completion and reconciles this connection's membership.
///
/// Input, attachment membership, and PTY dimensions are one ordered piece of per-connection state.
/// Running them concurrently would let a resize overtake input or let two Attach calls both observe
/// a missing membership and leak a service refcount.
async fn run_terminal_request(
    services: &Services,
    owner_id: u64,
    context: crate::services::RequestContext,
    body: RequestBody,
    attached: &mut HashSet<fleet_core::ids::TerminalId>,
) -> DaemonResult<ResponseBody> {
    let membership = match &body {
        RequestBody::AttachTerminal { terminal, .. } => Some((true, *terminal)),
        RequestBody::DetachTerminal { terminal } => Some((false, *terminal)),
        _ => None,
    };
    let result = match body {
        RequestBody::AttachTerminal {
            terminal,
            cols,
            rows,
        } if attached.contains(&terminal) => {
            services
                .dispatch_routed_with_owner(
                    RequestBody::ResizeTerminal {
                        terminal,
                        cols,
                        rows,
                    },
                    owner_id,
                    context,
                )
                .await
        }
        RequestBody::DetachTerminal { terminal } if !attached.contains(&terminal) => {
            Ok(ResponseBody::Ack)
        }
        body => {
            services
                .dispatch_routed_with_owner(body, owner_id, context)
                .await
        }
    };
    if result.is_ok()
        && let Some((attach, terminal)) = membership
    {
        if attach {
            attached.insert(terminal);
        } else {
            attached.remove(&terminal);
        }
    }
    result
}

type DispatchFuture = Pin<Box<dyn Future<Output = CompletedRequest> + Send>>;

/// Requests whose effects append to the PTY input byte stream.
fn pty_input_request_is_ordered(body: &RequestBody) -> bool {
    matches!(
        body,
        RequestBody::StartWatch { .. }
            | RequestBody::AppendWatchOutput { .. }
            | RequestBody::FinishWatch { .. }
            | RequestBody::TerminalInput { .. }
            | RequestBody::TerminalKey { .. }
            | RequestBody::TerminalMouse { .. }
            | RequestBody::PasteTerminal { .. }
            | RequestBody::WheelTerminal { .. }
            | RequestBody::ScrollOrKeyTerminal { .. }
            | RequestBody::ScrollTerminal { .. }
    )
}

fn terminal_request_is_serialized(body: &RequestBody) -> bool {
    pty_input_request_is_ordered(body)
        || matches!(
            body,
            RequestBody::AttachTerminal { .. }
                | RequestBody::DetachTerminal { .. }
                | RequestBody::ResizeTerminal { .. }
        )
}

async fn request_full_frames(
    services: &Services,
    owner_id: u64,
    client: &HelloClient,
    attached: &HashSet<fleet_core::ids::TerminalId>,
) {
    // A broadcast gap does not identify which terminal lost rows.
    for terminal in attached {
        if let Err(error) = services
            .dispatch_routed_with_owner(
                RequestBody::RequestFullFrame {
                    terminal: *terminal,
                },
                owner_id,
                crate::services::RequestContext {
                    client: client.clone(),
                },
            )
            .await
            && !matches!(error, DaemonError::NotFound(_))
        {
            // This is the only recovery a lagged client gets: without a replacement grid it
            // keeps applying dirty-row diffs to rows it never received.
            tracing::warn!(%error, %terminal, "failed to resync a lagged client");
        }
    }
}

struct CompletedRequest {
    id: u64,
    result: DaemonResult<ResponseBody>,
    snapshot_changed: bool,
    shutdown_request: Option<bool>,
}

fn request_changes_snapshot(body: &RequestBody) -> bool {
    matches!(
        body,
        RequestBody::CreateContext { .. }
            | RequestBody::UpdateContext { .. }
            | RequestBody::DeleteContext { .. }
            | RequestBody::SetActiveContext { .. }
            | RequestBody::CloneRepo { .. }
            | RequestBody::DeleteRepo { .. }
            | RequestBody::MoveRepoToContext { .. }
            | RequestBody::SetRepoHooks { .. }
            | RequestBody::DismissClone { .. }
            | RequestBody::CreateWorktree { .. }
            | RequestBody::DeleteWorktrees { .. }
            | RequestBody::PruneWorktrees { .. }
            | RequestBody::TouchWorktreeOpened { .. }
            | RequestBody::RestoreTrash { .. }
            | RequestBody::RefreshStatuses { .. }
            | RequestBody::CreateWorktreeFromPr { .. }
            | RequestBody::EnsureSession { .. }
            | RequestBody::KillWorktree { .. }
            | RequestBody::SleepWorktree { .. }
            | RequestBody::KillSession { .. }
            | RequestBody::SleepSession { .. }
            | RequestBody::NewTerminal { .. }
            | RequestBody::CloseTerminal { .. }
            | RequestBody::RestartTerminal { .. }
            | RequestBody::RenameTerminal { .. }
            | RequestBody::SelectTerminal { .. }
            | RequestBody::AttachTerminal { .. }
            | RequestBody::DetachTerminal { .. }
            | RequestBody::DismissJobs { .. }
            | RequestBody::SetConfig { .. }
            | RequestBody::ImportFromSwarm
            | RequestBody::Update
    )
}

fn event_visible(
    event: &Event,
    subscriptions: &HashSet<EventKind>,
    attached: &HashSet<fleet_core::ids::TerminalId>,
) -> bool {
    if !subscriptions.contains(&event_kind(event)) {
        return false;
    }
    match event {
        Event::TerminalFrame(frame) => attached.contains(&frame.terminal),
        Event::TerminalExited { terminal, .. } | Event::TerminalTitle { terminal, .. } => {
            attached.contains(terminal)
        }
        _ => true,
    }
}

fn event_kind(event: &Event) -> EventKind {
    match event {
        Event::Agent { .. } => EventKind::Agent,
        Event::AgentSummary(_) => EventKind::AgentSummary,
        Event::WatchStarted(_) => EventKind::WatchStarted,
        Event::WatchOutput { .. } => EventKind::WatchOutput,
        Event::WatchExited(_) => EventKind::WatchExited,
        Event::WatchDismissed(_) => EventKind::WatchDismissed,
        Event::SnapshotChanged(_) => EventKind::SnapshotChanged,
        Event::BoardChanged { .. } => EventKind::BoardChanged,
        Event::JobUpdated(_) => EventKind::JobUpdated,
        Event::SessionChanged(_) => EventKind::SessionChanged,
        Event::AgentActivityChanged { .. } => EventKind::AgentActivityChanged,
        Event::TerminalFrame(_) => EventKind::TerminalFrame,
        Event::TerminalExited { .. } => EventKind::TerminalExited,
        Event::TerminalTitle { .. } => EventKind::TerminalTitle,
        Event::HostLinkChanged { .. } => EventKind::HostLinkChanged,
        Event::TerminalReattach { .. } => EventKind::TerminalReattach,
        Event::Toast { .. } => EventKind::Toast,
        Event::DaemonShuttingDown => EventKind::DaemonShuttingDown,
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum Outbound {
    Response(Response),
    Hello(HelloResponse),
    Pong(PongResponse),
    Event(Event),
}

type OutboundSink =
    futures_util::stream::SplitSink<Framed<UnixStream, FleetCodec<Outbound, Request>>, Outbound>;

async fn run_writer(
    mut writer: OutboundSink,
    mut outbound: mpsc::Receiver<Outbound>,
) -> DaemonResult<()> {
    while let Some(message) = outbound.recv().await {
        tokio::time::timeout(SOCKET_WRITE_TIMEOUT, writer.send(message))
            .await
            .map_err(|_| DaemonError::Timeout("client socket write".to_owned()))?
            .map_err(|error| DaemonError::Protocol(error.to_string()))?;
    }
    Ok(())
}

async fn enqueue_response(
    outbound: &mpsc::Sender<Outbound>,
    response: Response,
) -> DaemonResult<()> {
    let message = if matches!(&response.result, Ok(ResponseBody::Pong)) {
        Outbound::Pong(PongResponse {
            response,
            daemon: Some(daemon_identity()),
        })
    } else {
        Outbound::Response(response)
    };
    enqueue_outbound(outbound, message).await
}

async fn enqueue_event(outbound: &mpsc::Sender<Outbound>, event: Event) -> DaemonResult<()> {
    enqueue_outbound(outbound, Outbound::Event(event)).await
}

async fn enqueue_outbound(
    outbound: &mpsc::Sender<Outbound>,
    message: Outbound,
) -> DaemonResult<()> {
    tokio::time::timeout(SOCKET_WRITE_TIMEOUT, outbound.send(message))
        .await
        .map_err(|_| DaemonError::Timeout("client outbound queue".to_owned()))?
        .map_err(|_| DaemonError::Protocol("client writer stopped".to_owned()))
}

async fn write_response(
    framed: &mut Framed<UnixStream, FleetCodec<Outbound, Request>>,
    response: Response,
    daemon_id: &str,
) -> DaemonResult<()> {
    let message = if matches!(&response.result, Ok(ResponseBody::Hello { .. })) {
        Outbound::Hello(HelloResponse {
            response,
            capabilities: vec![
                PRUNE_REVIEWED_IDS_CAPABILITY.to_owned(),
                fleet_proto::REMOTE_MACHINES_CAPABILITY.to_owned(),
            ],
            daemon_id: daemon_id.to_owned(),
            build_commit: option_env!("FLEET_BUILD_COMMIT").map(str::to_owned),
        })
    } else {
        Outbound::Response(response)
    };
    framed
        .send(message)
        .await
        .map_err(|error| DaemonError::Protocol(error.to_string()))
}

fn daemon_identity() -> DaemonIdentity {
    DaemonIdentity {
        pid: std::process::id(),
        boot_id: DAEMON_BOOT_ID
            .get_or_init(|| uuid::Uuid::new_v4().to_string())
            .clone(),
    }
}

#[cfg(test)]
async fn write_event(
    framed: &mut Framed<UnixStream, FleetCodec<Outbound, Request>>,
    event: Event,
) -> DaemonResult<()> {
    framed
        .send(Outbound::Event(event))
        .await
        .map_err(|error| DaemonError::Protocol(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::{net::Shutdown, path::Path, time::Duration};

    use fleet_core::{config::Agent, ids::TerminalId};

    use crate::{
        adapters::{Adapters, clock::SystemClock, files::RealFiles},
        jobs::JobManager,
        stores::{config::ConfigStore, state::StateStore},
    };

    use super::*;

    async fn test_services(home: &Path) -> Arc<Services> {
        let files = Arc::new(RealFiles::new(
            home.join("trash"),
            [home.join("repos"), home.join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(home, files.clone()));
        let mut effective = config.load().await.expect("load test config");
        effective.agent_commands.claude = "/bin/sleep 30".into();
        config.save(effective).await.expect("save test config");
        let state = Arc::new(StateStore::new(home, files.clone(), Arc::new(SystemClock)));
        Services::new_with_events(
            home,
            config,
            state,
            Arc::new(JobManager::new(home)),
            Adapters::system(files),
            BroadcastBus::default(),
        )
    }

    #[test]
    fn hello_and_pong_advertise_connection_metadata() {
        let hello = Outbound::Hello(HelloResponse {
            response: Response {
                id: 1,
                result: Ok(ResponseBody::Hello {
                    protocol: fleet_proto::PROTOCOL_VERSION,
                    server: Services::version(),
                }),
            },
            capabilities: vec![
                PRUNE_REVIEWED_IDS_CAPABILITY.to_owned(),
                fleet_proto::REMOTE_MACHINES_CAPABILITY.to_owned(),
            ],
            daemon_id: "test-daemon".to_owned(),
            build_commit: None,
        });
        let hello = serde_json::to_value(hello).expect("serialize Hello");
        assert_eq!(
            hello["capabilities"],
            serde_json::json!(["prune.reviewed_ids", "remote-machines"])
        );

        let identity = daemon_identity();
        let pong = Outbound::Pong(PongResponse {
            response: Response {
                id: 2,
                result: Ok(ResponseBody::Pong),
            },
            daemon: Some(identity.clone()),
        });
        let pong = serde_json::to_value(pong).expect("serialize Pong");
        assert_eq!(pong["daemon"]["pid"], identity.pid);
        assert_eq!(pong["daemon"]["bootId"], identity.boot_id);
    }

    #[tokio::test]
    async fn outbound_envelopes_preserve_protocol_shapes() {
        let (server, client) = UnixStream::pair().expect("socket pair");
        let mut server = Framed::new(server, FleetCodec::<Outbound, Request>::new());
        let mut client = Framed::new(client, FleetCodec::<Request, serde_json::Value>::new());
        for golden in [
            r#"{"id":7,"result":{"Ok":{"type":"pong"}}}"#,
            r#"{"id":8,"result":{"Err":{"kind":"unsupported","message":"unsupported protocol"}}}"#,
        ] {
            let expected: serde_json::Value =
                serde_json::from_str(golden).expect("response golden");
            let response = serde_json::from_value(expected.clone()).expect("response shape");
            write_response(&mut server, response, "test-daemon")
                .await
                .expect("send response");
            let actual = tokio::time::timeout(Duration::from_secs(1), client.next())
                .await
                .expect("receive deadline")
                .expect("response")
                .expect("frame");
            assert_eq!(actual, expected);
        }
        for golden in [
            r#"{"type":"terminal_title","data":{"terminal":9,"title":"編集中"}}"#,
            r#"{"type":"terminal_exited","data":{"terminal":9,"code":0}}"#,
            r#"{"type":"terminal_frame","data":{"terminal":9,"seq":3,"cols":1,"rows":1,"full":true,"rowsChanged":[{"index":0,"cells":[{"text":"é","fg":"default","bg":"default","underlineColor":null,"attrs":"BOLD","width":"narrow"}],"wrapped":false}],"cursor":{"row":0,"col":0,"visible":true,"shape":"block"},"viewport":{"scrollbackLen":0,"offset":0,"historyEpoch":0},"modes":{"altScreen":false,"mouseReporting":false,"bracketedPaste":false,"focusEvents":false,"kittyKeyboardFlags":0,"appCursorKeys":false},"title":null}}"#,
        ] {
            let expected: serde_json::Value = serde_json::from_str(golden).expect("event golden");
            let event = serde_json::from_value(expected.clone()).expect("event shape");
            write_event(&mut server, event).await.expect("send event");
            let actual = tokio::time::timeout(Duration::from_secs(1), client.next())
                .await
                .expect("receive deadline")
                .expect("event")
                .expect("frame");
            assert_eq!(actual, expected);
        }
    }

    #[tokio::test]
    async fn silent_client_handshake_has_deadline() {
        let temp = tempfile::tempdir().expect("temp home");
        let services = test_services(temp.path()).await;
        let (server, _client) = UnixStream::pair().expect("socket pair");
        let mut framed = Framed::new(server, FleetCodec::<Outbound, Request>::new());
        let result =
            negotiate_hello_with_timeout(&mut framed, Duration::from_millis(10), &services).await;
        assert!(matches!(result, Err(DaemonError::Timeout(_))));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn slow_client_cannot_exhaust_pending_admission() {
        const EXTRA_REQUESTS: usize = 8;

        let temp = tempfile::tempdir().expect("create temp dir");
        let services = test_services(temp.path()).await;
        let shutdown = CancellationToken::new();
        let (server, client) = UnixStream::pair().expect("socket pair");
        let gate = CancellationToken::new();
        let pending_high_water = Arc::new(AtomicUsize::new(0));
        let request_count = MAX_PENDING_REQUESTS + EXTRA_REQUESTS;
        let last_request_id = request_count as u64 + 1;
        let (pending_limit_reached, wait_for_pending_limit) = oneshot::channel();
        let (last_request_read, wait_for_last_request) = oneshot::channel();
        let actor = tokio::spawn(
            Connection::new(server, services, BroadcastBus::default(), shutdown.clone())
                .with_blocked_dispatch(
                    gate.clone(),
                    Arc::clone(&pending_high_water),
                    pending_limit_reached,
                    last_request_id,
                    last_request_read,
                )
                .run(),
        );
        let mut client = Framed::new(client, FleetCodec::<Request, Response>::new());
        client
            .send(Request {
                id: 1,
                body: RequestBody::Hello {
                    protocol: fleet_proto::PROTOCOL_VERSION,
                    client: "admission-test".into(),
                },
            })
            .await
            .expect("send hello");
        client
            .next()
            .await
            .expect("hello response")
            .expect("decode hello response")
            .result
            .expect("hello succeeds");

        for id in 2..=last_request_id {
            client
                .send(Request {
                    id,
                    body: RequestBody::DaemonPing,
                })
                .await
                .expect("queue request");
        }

        tokio::time::timeout(Duration::from_secs(2), wait_for_pending_limit)
            .await
            .expect("pending admission reaches its bound")
            .expect("pending limit observer remains open");
        tokio::task::yield_now().await;
        gate.cancel();
        for _ in 0..request_count {
            let response = tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .expect("response deadline")
                .expect("response")
                .expect("decode response");
            assert_eq!(response.result, Ok(ResponseBody::Pong));
        }
        tokio::time::timeout(Duration::from_secs(2), wait_for_last_request)
            .await
            .expect("last request read deadline")
            .expect("last request observer remains open");
        assert_eq!(
            pending_high_water.load(Ordering::Relaxed),
            MAX_PENDING_REQUESTS
        );

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(2), actor)
            .await
            .expect("connection cleanup deadline")
            .expect("connection task")
            .expect("connection stops cleanly");
    }

    #[tokio::test]
    async fn response_write_failure_after_attach_runs_detach_cleanup() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let services = test_services(temp.path()).await;
        let session = services
            .sessions
            .ensure(None, Some(Agent::Claude), false)
            .await
            .expect("ensure agent session");
        let terminal = session.terminals[0].id;
        let (server, client) = UnixStream::pair().expect("create socket pair");
        let (response_ready, wait_for_response) = oneshot::channel();
        let (release_response, response_released) = oneshot::channel();
        let actor = tokio::spawn(
            Connection::new(
                server,
                Arc::clone(&services),
                services.events.clone(),
                CancellationToken::new(),
            )
            .with_before_serialized_response(response_ready, response_released)
            .run(),
        );
        let mut client = Framed::new(client, FleetCodec::<Request, Response>::new());
        client
            .send(Request {
                id: 1,
                body: RequestBody::Hello {
                    protocol: fleet_proto::PROTOCOL_VERSION,
                    client: "connection-test".into(),
                },
            })
            .await
            .expect("send hello");
        client
            .next()
            .await
            .expect("receive hello response")
            .expect("decode hello response")
            .result
            .expect("hello succeeds");
        client
            .send(Request {
                id: 2,
                body: RequestBody::AttachTerminal {
                    terminal,
                    cols: 80,
                    rows: 24,
                },
            })
            .await
            .expect("send attach");

        tokio::time::timeout(Duration::from_secs(2), wait_for_response)
            .await
            .expect("actor reaches attachment response")
            .expect("attachment response barrier remains open");
        let client = client
            .into_inner()
            .into_std()
            .expect("convert client socket");
        client
            .shutdown(Shutdown::Both)
            .expect("reject response writes");
        drop(client);
        release_response
            .send(())
            .expect("release attachment response");
        let result = tokio::time::timeout(Duration::from_secs(2), actor)
            .await
            .expect("connection actor exits")
            .expect("connection task does not panic");
        assert!(result.is_err(), "the acknowledgement write must fail");
        assert_eq!(services.sessions.attachment_count(terminal), 0);

        services
            .sessions
            .kill(session.id)
            .await
            .expect("kill test session");
    }

    #[test]
    fn watch_events_are_global_and_require_their_subscription() {
        let event = Event::WatchDismissed(fleet_core::watches::WatchId(1));
        assert!(event_visible(
            &event,
            &HashSet::from([EventKind::WatchDismissed]),
            &HashSet::new()
        ));
        assert!(!event_visible(&event, &HashSet::new(), &HashSet::new()));
    }

    #[test]
    fn wheel_and_scroll_share_the_pty_ordering_channel() {
        use fleet_proto::terminal::{Modifiers, ScrollCommand, WheelEvent};
        assert!(pty_input_request_is_ordered(&RequestBody::WheelTerminal {
            terminal: TerminalId(1),
            wheel: WheelEvent {
                steps: -1,
                col: 0,
                row: 0,
                mods: Modifiers::empty()
            },
        }));
        assert!(pty_input_request_is_ordered(
            &RequestBody::ScrollOrKeyTerminal {
                terminal: TerminalId(1),
                scroll: ScrollCommand::Pages(-1),
                key: fleet_proto::terminal::KeyEvent {
                    key: fleet_proto::terminal::Key::PageUp,
                    mods: Modifiers::SHIFT,
                    text: None,
                    action: fleet_proto::terminal::KeyAction::Press,
                },
            }
        ));
        assert!(pty_input_request_is_ordered(&RequestBody::ScrollTerminal {
            terminal: TerminalId(1),
            scroll: ScrollCommand::Bottom,
        }));
    }

    #[test]
    fn resize_cannot_overtake_prior_terminal_input() {
        let terminal = TerminalId(1);
        assert!(terminal_request_is_serialized(
            &RequestBody::TerminalInput {
                terminal,
                bytes: b"input".to_vec(),
            }
        ));
        assert!(terminal_request_is_serialized(
            &RequestBody::AttachTerminal {
                terminal,
                cols: 80,
                rows: 24,
            }
        ));
        assert!(terminal_request_is_serialized(
            &RequestBody::DetachTerminal { terminal }
        ));
        assert!(terminal_request_is_serialized(
            &RequestBody::ResizeTerminal {
                terminal,
                cols: 120,
                rows: 40,
            }
        ));
        assert!(!terminal_request_is_serialized(
            &RequestBody::RequestFullFrame { terminal }
        ));
    }

    #[tokio::test]
    async fn event_lag_forces_reconnect_and_full_frames() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let events = BroadcastBus::new(1);
        let files = Arc::new(RealFiles::new(
            temp.path().join("trash"),
            [temp.path().join("repos"), temp.path().join("worktrees")],
        ));
        let config = Arc::new(ConfigStore::new(temp.path(), files.clone()));
        let mut effective = config.load().await.expect("load test config");
        effective.agent_commands.claude = "/bin/sleep 30".into();
        config.save(effective).await.expect("save test config");
        let state = Arc::new(StateStore::new(
            temp.path(),
            files.clone(),
            Arc::new(SystemClock),
        ));
        let services = Services::new_with_events(
            temp.path(),
            config,
            state,
            Arc::new(JobManager::new(temp.path())),
            Adapters::system(files),
            events.clone(),
        );
        let session = services
            .sessions
            .ensure(None, Some(Agent::Claude), false)
            .await
            .expect("ensure agent session");
        let terminal = session.terminals[0].id;
        let mut frames = services.sessions.subscribe_frames();
        let (server, client) = UnixStream::pair().expect("create socket pair");
        let (response_ready, wait_for_response) = oneshot::channel();
        let (release_response, response_released) = oneshot::channel();
        let actor = tokio::spawn(
            Connection::new(
                server,
                Arc::clone(&services),
                events.clone(),
                CancellationToken::new(),
            )
            .with_before_serialized_response(response_ready, response_released)
            .run(),
        );
        let mut client = Framed::new(client, FleetCodec::<Request, Response>::new());
        for (id, body) in [
            (
                1,
                RequestBody::Hello {
                    protocol: fleet_proto::PROTOCOL_VERSION,
                    client: "lag-test".into(),
                },
            ),
            (
                2,
                RequestBody::Subscribe {
                    events: vec![EventKind::Toast],
                },
            ),
        ] {
            client.send(Request { id, body }).await.expect("send setup");
            client
                .next()
                .await
                .expect("setup response")
                .expect("decode setup response")
                .result
                .expect("setup succeeds");
        }
        client
            .send(Request {
                id: 3,
                body: RequestBody::AttachTerminal {
                    terminal,
                    cols: 80,
                    rows: 24,
                },
            })
            .await
            .expect("send attach");
        tokio::time::timeout(Duration::from_secs(2), wait_for_response)
            .await
            .expect("connection reaches attach barrier")
            .expect("attach barrier remains open");

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let frame = frames.recv().await.expect("frame channel");
                if frame.terminal == terminal && frame.full {
                    return;
                }
            }
        })
        .await
        .expect("initial full frame");
        while frames.try_recv().is_ok() {}
        for index in 0..4 {
            events.publish(Event::Toast {
                level: fleet_proto::event::ToastLevel::Info,
                message: format!("event {index}"),
            });
        }
        release_response.send(()).expect("release attach response");
        client
            .next()
            .await
            .expect("attach response")
            .expect("decode attach response")
            .result
            .expect("attach succeeds");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let frame = frames.recv().await.expect("frame channel");
                if frame.terminal == terminal && frame.full {
                    return;
                }
            }
        })
        .await
        .expect("lag requests a replacement full frame");

        // §6 recovers a gap with a resync, not a disconnect: the stream stays open and the next
        // event still reaches the client, where the sequence gap it carries is what triggers
        // the client-side resync.
        events.publish(Event::Toast {
            level: fleet_proto::event::ToastLevel::Info,
            message: "after the lag".to_owned(),
        });
        assert!(
            tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .expect("the connection survives the lag")
                .is_some(),
            "a lagged client is resynced, never disconnected"
        );
        assert!(!actor.is_finished(), "the lag must not end the connection");
        actor.abort();

        services
            .sessions
            .kill(session.id)
            .await
            .expect("kill test session");
    }

    /// A remote link that reports itself `Ready` and never answers a request.
    struct WedgedRemote {
        host: fleet_core::ids::HostId,
        events: tokio::sync::broadcast::Sender<Event>,
        states: tokio::sync::watch::Sender<fleet_proto::snapshot::LinkState>,
    }

    impl WedgedRemote {
        fn new(host: fleet_core::ids::HostId) -> Self {
            let (events, _) = tokio::sync::broadcast::channel(8);
            let (states, _) = tokio::sync::watch::channel(fleet_proto::snapshot::LinkState::Ready);
            Self {
                host,
                events,
                states,
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::machines::RemoteEndpoint for WedgedRemote {
        fn host(&self) -> &fleet_core::ids::HostId {
            &self.host
        }
        fn state(&self) -> fleet_proto::snapshot::LinkState {
            *self.states.borrow()
        }
        fn hello(&self) -> Option<crate::machines::RemoteHello> {
            None
        }
        async fn request(&self, _body: RequestBody) -> DaemonResult<ResponseBody> {
            std::future::pending().await
        }
        fn events(&self) -> tokio::sync::broadcast::Receiver<Event> {
            self.events.subscribe()
        }
        fn state_changes(&self) -> tokio::sync::watch::Receiver<fleet_proto::snapshot::LinkState> {
            self.states.subscribe()
        }
        async fn close(&self) {}
    }

    #[tokio::test]
    async fn a_wedged_remote_cannot_stall_disconnect_cleanup() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let services = test_services(temp.path()).await;
        let host = fleet_core::ids::HostId::try_from("wedged").expect("host id");
        services
            .machines
            .install_endpoint(host.clone(), Arc::new(WedgedRemote::new(host.clone())));
        let terminal = services.router.ids.local_terminal(&host, TerminalId(1));

        let detached = tokio::time::timeout(
            Duration::from_secs(20),
            detach_attached_terminals(
                &services,
                0,
                &HelloClient::default(),
                HashSet::from([terminal]),
            ),
        )
        .await
        .expect("a wedged remote link cannot hold the connection's admission permit");

        assert!(detached, "the cleanup reports the terminals it released");
    }

    #[tokio::test]
    async fn a_cancelled_shutdown_token_tells_a_subscribed_client_before_the_socket_closes() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let services = test_services(temp.path()).await;
        // Nothing else ever publishes on this bus, so the only way the client can learn the
        // daemon is stopping is the connection's own shutdown branch. `Listener::run` publishes
        // the same event, but only after its accept loop has broken on this very token, by
        // which time every connection actor has already left its loop.
        let events = BroadcastBus::default();
        let shutdown = CancellationToken::new();
        let (server, client) = UnixStream::pair().expect("create socket pair");
        let actor = tokio::spawn(
            Connection::new(server, Arc::clone(&services), events, shutdown.clone()).run(),
        );
        let mut client = Framed::new(client, FleetCodec::<Request, serde_json::Value>::new());
        for (id, body) in [
            (
                1,
                RequestBody::Hello {
                    protocol: fleet_proto::PROTOCOL_VERSION,
                    client: "shutdown-notice-test".into(),
                },
            ),
            (
                2,
                RequestBody::Subscribe {
                    events: vec![EventKind::DaemonShuttingDown],
                },
            ),
        ] {
            client.send(Request { id, body }).await.expect("send setup");
            let value = client
                .next()
                .await
                .expect("setup response")
                .expect("decode setup response");
            let response: Response =
                serde_json::from_value(value).expect("setup response envelope");
            response.result.expect("setup succeeds");
        }

        shutdown.cancel();

        let announced = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(frame) = client.next().await {
                let value = frame.expect("decode frame");
                if value.get("type").and_then(serde_json::Value::as_str)
                    == Some("daemon_shutting_down")
                {
                    return true;
                }
            }
            false
        })
        .await
        .expect("shutdown notice deadline");

        assert!(
            announced,
            "a subscribed client is told the daemon is stopping, not left to infer it from a close"
        );
        actor
            .await
            .expect("connection task")
            .expect("connection ends cleanly");
    }

    #[tokio::test]
    async fn an_explicit_shutdown_publishes_daemon_shutting_down_after_the_response() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let services = test_services(temp.path()).await;
        let events = BroadcastBus::default();
        let mut bus = events.subscribe();
        let shutdown = CancellationToken::new();
        let (server, client) = UnixStream::pair().expect("create socket pair");
        let actor = tokio::spawn(
            Connection::new(
                server,
                Arc::clone(&services),
                events.clone(),
                shutdown.clone(),
            )
            .run(),
        );
        let mut client = Framed::new(client, FleetCodec::<Request, Response>::new());
        client
            .send(Request {
                id: 1,
                body: RequestBody::Hello {
                    protocol: fleet_proto::PROTOCOL_VERSION,
                    client: "shutdown-test".into(),
                },
            })
            .await
            .expect("send hello");
        client
            .next()
            .await
            .expect("hello response")
            .expect("decode hello response")
            .result
            .expect("hello succeeds");

        client
            .send(Request {
                id: 2,
                body: RequestBody::DaemonShutdown {
                    stop_sessions: false,
                },
            })
            .await
            .expect("send shutdown");
        let response = client
            .next()
            .await
            .expect("shutdown response")
            .expect("decode shutdown response")
            .result
            .expect("shutdown succeeds");

        // The client that asked learns the outcome from its own response; every other
        // connection learns it from the bus event this test asserts below.
        assert_eq!(response, ResponseBody::ShuttingDown);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match bus.recv().await.expect("event bus stays open") {
                    Event::DaemonShuttingDown => return,
                    _ => continue,
                }
            }
        })
        .await
        .expect("an explicit shutdown publishes DaemonShuttingDown");
        assert!(
            shutdown.is_cancelled(),
            "the shutdown request cancels the daemon token"
        );
        actor
            .await
            .expect("connection task")
            .expect("connection ends cleanly");
    }

    #[test]
    fn terminal_events_are_visible_only_to_attached_subscribers() {
        let terminal = TerminalId(9);
        let event = Event::TerminalExited {
            terminal,
            code: Some(0),
        };
        let subscriptions = HashSet::from([EventKind::TerminalExited]);

        assert!(!event_visible(&event, &subscriptions, &HashSet::new()));
        assert!(event_visible(
            &event,
            &subscriptions,
            &HashSet::from([terminal])
        ));
        assert!(!event_visible(
            &event,
            &HashSet::new(),
            &HashSet::from([terminal])
        ));
    }
}
