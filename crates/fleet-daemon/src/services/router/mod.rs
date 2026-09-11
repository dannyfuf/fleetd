//! Request classification, remote forwarding, and remote endpoint event pumps.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use fleet_core::{
    agents::ThreadId,
    ids::{HostId, JobId, TerminalId, WorktreeId},
    sessions::Session,
};
use fleet_proto::{
    event::Event, request::RequestBody, response::ResponseBody, snapshot::LinkState,
};
use futures_util::future::join_all;
use tokio_util::sync::CancellationToken;

use crate::{
    DaemonError, DaemonResult,
    machines::{Machines, RemoteEndpoint},
    server::BroadcastBus,
};

use super::mirror::Mirror;

pub mod agents;
pub mod classify;
pub(crate) mod create;
pub mod ids;
pub(crate) mod lifecycle;
pub(crate) mod sessions;
pub mod translate;

pub use classify::Target;
pub use ids::{ClearedIds, RemoteIds};

/// Resolves protocol identifiers to the machine that owns them.
pub trait Resolver {
    fn host_of_worktree(&self, id: &WorktreeId) -> Option<HostId>;
    fn host_of_session(&self, id: &str) -> Option<HostId>;
    fn host_of_terminal(&self, id: TerminalId) -> Option<HostId>;
    fn host_of_job(&self, id: &JobId) -> Option<HostId>;
    fn host_of_thread(&self, id: &ThreadId) -> Option<HostId>;
}

/// Federating request router shared by every local client connection.
pub struct Router {
    pub ids: RemoteIds,
    pub mirror: Arc<Mirror>,
    pub machines: Arc<Machines>,
    event_bus: Mutex<Option<BroadcastBus>>,
    endpoint_pumps: Arc<Mutex<BTreeMap<HostId, EndpointPump>>>,
    awaiting_reattach: Arc<Mutex<BTreeMap<HostId, Vec<TerminalTombstone>>>>,
    session_intents: Arc<Mutex<BTreeMap<HostId, Vec<SessionIntent>>>>,
    terminal_frames: sessions::RemoteTerminalFrames,
    thread_registrations: Arc<agents::ThreadRegistrations>,
    /// The durable read-through mirror of the threads other hosts own, once the agent service
    /// has been built. `None` until then, and in every test that routes without one — which is
    /// what keeps the routing model itself independent of the cache hanging off it.
    agent_mirror: Mutex<Option<Arc<dyn agents::AgentMirror>>>,
}

struct EndpointPump {
    endpoint: Arc<dyn RemoteEndpoint>,
    cancel: CancellationToken,
}

#[derive(Clone)]
struct SessionIntent {
    request: RequestBody,
    previous: Session,
}

#[derive(Clone, Copy)]
struct TerminalTombstone {
    remote: TerminalId,
    local: TerminalId,
    attachment_count: usize,
}

impl Router {
    #[must_use]
    pub fn new(machines: Arc<Machines>, mirror: Arc<Mirror>) -> Self {
        Self::with_ids(machines, mirror, RemoteIds::default())
    }

    /// Builds a router around an explicitly shared id allocator.
    #[must_use]
    pub fn with_ids(machines: Arc<Machines>, mirror: Arc<Mirror>, ids: RemoteIds) -> Self {
        let thread_registrations = Arc::new(agents::ThreadRegistrations::default());
        for (host, _) in machines.iter() {
            if let Some(fragment) = mirror.fragment(&host) {
                agents::register_mirror_threads(
                    &thread_registrations,
                    &host,
                    &fragment.snapshot.agent_threads,
                    &ids,
                );
            }
        }
        Self {
            ids,
            mirror,
            machines,
            event_bus: Mutex::new(None),
            endpoint_pumps: Arc::new(Mutex::new(BTreeMap::new())),
            awaiting_reattach: Arc::new(Mutex::new(BTreeMap::new())),
            session_intents: Arc::new(Mutex::new(BTreeMap::new())),
            terminal_frames: sessions::RemoteTerminalFrames::default(),
            thread_registrations,
            agent_mirror: Mutex::new(None),
        }
    }

    /// Installs the durable native-agent mirror.
    ///
    /// Separate from construction because the agent service is built before the router and the
    /// two need each other: the router asks the mirror whether it can answer an open, and the
    /// mirror needs the router's classification to have already sent mutations upstream.
    pub fn set_agent_mirror(&self, mirror: Arc<dyn agents::AgentMirror>) {
        *lock(&self.agent_mirror) = Some(mirror);
    }

    /// Registers the thread ownership the local daemon's mirror already knows about.
    ///
    /// Without this a mirrored thread classifies as *local* between a daemon start and the
    /// owner's first snapshot, which is exactly the window in which a mutation must be routed
    /// upstream rather than answered here (`docs/NATIVE-AGENTS.md` §9.3).
    pub fn adopt_mirrored_threads(&self, owners: impl IntoIterator<Item = (ThreadId, HostId)>) {
        for (thread, host) in owners {
            self.ids.register_thread(&host, thread);
        }
    }

    fn agent_mirror(&self) -> Option<Arc<dyn agents::AgentMirror>> {
        lock(&self.agent_mirror).clone()
    }

    #[must_use]
    pub fn route(&self, body: &RequestBody) -> Target {
        if matches!(body, RequestBody::AgentThreadList) {
            let hosts = self
                .machines
                .iter()
                .into_iter()
                .filter(|(_, provider)| provider.provider_name() != "legacy")
                .filter_map(|(host, _)| {
                    let endpoint = self.machines.endpoint(&host)?;
                    self.pump_endpoint(host.clone(), endpoint);
                    Some(host)
                });
            return agents::agent_list_target(hosts);
        }
        if let Some(parts) = self.lifecycle_fanout(body) {
            return Target::Fanout(parts);
        }
        classify::classify(body, self)
    }

    pub async fn forward(&self, host: &HostId, body: RequestBody) -> DaemonResult<ResponseBody> {
        let endpoint = self.endpoint(host)?;
        let mirror = self.agent_mirror();
        // Snapshot-then-delta: a warm mirrored thread is answered from this daemon's own database
        // and the owner is asked for the rest in the background (§9.3). It runs before the link
        // state is consulted, because disconnection changes the status of a transcript and never
        // its readability.
        if let Some(answer) =
            agents::open_from_mirror(mirror.as_ref(), &endpoint, host, &body).await
        {
            return answer;
        }
        if endpoint.state() == LinkState::Down {
            return Err(unreachable(host));
        }
        self.pump_endpoint(host.clone(), Arc::clone(&endpoint));
        let local_terminal = match &body {
            RequestBody::AttachTerminal { terminal, .. }
            | RequestBody::DetachTerminal { terminal } => Some(*terminal),
            _ => None,
        };
        let attaching = matches!(body, RequestBody::AttachTerminal { .. });
        // Kept for the mirror, which records what the owner answered against the request that
        // asked for it, because `body` is consumed by the id translation below. Only the three
        // requests whose answers a cache may take are cloned: terminal input is forwarded through
        // here too, and a clone per keystroke-sized frame is a cost with nothing to show for it.
        let mirrored_body = matches!(
            body,
            RequestBody::AgentThreadOpen { .. }
                | RequestBody::AgentThreadList
                | RequestBody::AgentThreadCreate { .. }
        )
        .then(|| body.clone());
        if attaching
            && let (Some(terminal), Some(events)) = (local_terminal, lock(&self.event_bus).clone())
        {
            sessions::on_attach(
                &self.terminal_frames,
                Arc::clone(&endpoint),
                host,
                terminal,
                self.ids.clone(),
                events,
            )?;
        }
        let remote = match translate::to_remote(body, host, &self.ids) {
            Ok(remote) => remote,
            Err(error) => {
                if attaching && let Some(terminal) = local_terminal {
                    sessions::on_detach(&self.terminal_frames, host, terminal);
                }
                return Err(error);
            }
        };
        let session_request =
            matches!(remote, RequestBody::EnsureSession { .. }).then(|| remote.clone());
        let response = endpoint.request(remote).await;
        // A detach releases the local attachment whatever the remote answers; an attach
        // releases it only when the remote refused.
        if let Some(terminal) = local_terminal
            && (!attaching || response.is_err())
        {
            sessions::on_detach(&self.terminal_frames, host, terminal);
        }
        let response = response
            .map(|response| {
                if let (Some(request), ResponseBody::Session(session)) =
                    (session_request, &response)
                {
                    remember_session_intent(
                        &self.session_intents,
                        host,
                        SessionIntent {
                            request,
                            previous: session.clone(),
                        },
                    );
                }
                translate::response_to_local(response, host, &self.ids)
            })
            .map_err(|error| annotate_remote_error(host, error));
        // A cold mirror warms up on the way back, so the *next* open of this thread is a local
        // read. The answer the client gets is the owner's either way.
        if let (Ok(answer), Some(request)) = (&response, &mirrored_body) {
            agents::absorb_forwarded(mirror.as_ref(), host, request, answer).await;
        }
        response
    }

    pub async fn fanout(
        &self,
        parts: Vec<(HostId, RequestBody)>,
    ) -> Vec<(HostId, DaemonResult<ResponseBody>)> {
        join_all(parts.into_iter().map(|(host, body)| async move {
            let result = match self.forward(&host, body.clone()).await {
                Err(error) => {
                    if matches!(body, RequestBody::AgentThreadList) {
                        // Cached summaries stay visible while a host is unreachable, from the
                        // durable mirror first and from the in-memory snapshot fragment when
                        // this daemon has no mirror installed (§9.3).
                        let mut cached =
                            agents::cached_threads(self.agent_mirror().as_ref(), &host).await;
                        if cached.is_empty() {
                            cached = self
                                .mirror
                                .fragment(&host)
                                .map(|fragment| fragment.snapshot.agent_threads)
                                .unwrap_or_default();
                        }
                        Ok(translate::response_to_local(
                            ResponseBody::AgentThreads(cached),
                            &host,
                            &self.ids,
                        ))
                    } else {
                        translate::unavailable_fanout_response(&body, &host, &error).ok_or(error)
                    }
                }
                result => result,
            };
            (host, result)
        }))
        .await
    }

    /// Starts translation pumps for current endpoints and remembers the daemon event bus for
    /// endpoints created lazily by later requests.
    pub fn start_event_pumps(&self, events: BroadcastBus) {
        *lock(&self.event_bus) = Some(events);
        let mut active = BTreeMap::new();
        for (host, endpoint) in self.machines.endpoints() {
            active.insert(host, endpoint);
        }
        for (host, provider) in self.machines.iter() {
            if provider.provider_name() == "legacy" {
                continue;
            }
            if let Some(endpoint) = self.machines.endpoint(&host) {
                active.insert(host, endpoint);
            }
        }
        {
            let mut pumps = lock(&self.endpoint_pumps);
            pumps.retain(|host, pump| {
                let keep = active
                    .get(host)
                    .is_some_and(|endpoint| Arc::ptr_eq(&pump.endpoint, endpoint));
                if !keep {
                    pump.cancel.cancel();
                }
                keep
            });
        }
        for (host, endpoint) in active {
            self.pump_endpoint(host, endpoint);
        }
    }

    fn endpoint(&self, host: &HostId) -> DaemonResult<Arc<dyn RemoteEndpoint>> {
        self.machines
            .endpoint(host)
            .ok_or_else(|| unreachable(host))
    }

    fn pump_endpoint(&self, host: HostId, endpoint: Arc<dyn RemoteEndpoint>) {
        let Some(events) = lock(&self.event_bus).clone() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let cancel = {
            let mut pumps = lock(&self.endpoint_pumps);
            if let Some(current) = pumps.get(&host) {
                if Arc::ptr_eq(&current.endpoint, &endpoint) {
                    return;
                }
                current.cancel.cancel();
            }
            let cancel = CancellationToken::new();
            pumps.insert(
                host.clone(),
                EndpointPump {
                    endpoint: Arc::clone(&endpoint),
                    cancel: cancel.clone(),
                },
            );
            cancel
        };

        let mut remote_events = endpoint.events();
        let event_ids = self.ids.clone();
        let event_host = host.clone();
        let event_bus = events.clone();
        let event_mirror = Arc::clone(&self.mirror);
        let event_threads = Arc::clone(&self.thread_registrations);
        let event_cancel = cancel.clone();
        let event_pumps = Arc::clone(&self.endpoint_pumps);
        let event_endpoint = Arc::clone(&endpoint);
        let event_agent_mirror = self.agent_mirror();
        runtime.spawn(async move {
            loop {
                let event = match tokio::select! {
                    () = event_cancel.cancelled() => break,
                    event = remote_events.recv() => event,
                } {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(%event_host, skipped, "remote event pump lagged");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                // Mirrored before it is republished: a client that reacts to an event by
                // opening the thread must not be able to read a transcript older than the event
                // that woke it (§9.3).
                agents::ingest_remote_event(
                    event_agent_mirror.as_ref(),
                    &event_endpoint,
                    &event_host,
                    &event,
                )
                .await;
                if let Event::SnapshotChanged(snapshot) = &event {
                    event_mirror.apply(&event_host, snapshot.clone());
                    agents::register_thread_events(&event_threads, &event, &event_host, &event_ids);
                    event_bus.request_snapshot_current();
                    continue;
                }
                agents::register_thread_events(&event_threads, &event, &event_host, &event_ids);
                // A remote daemon stopping is a fact about one endpoint, not about this one:
                // republished verbatim, `DaemonShuttingDown` would tell every local client that
                // *this* daemon is going away and end their connections. The link expresses it
                // locally as a `LinkState` change, which the state pump below turns into
                // `HostLinkChanged` (docs/APP-CONTRACTS.md §4).
                if matches!(
                    event,
                    Event::TerminalFrame(_)
                        | Event::HostLinkChanged { .. }
                        | Event::DaemonShuttingDown
                ) {
                    continue;
                }
                let Some(local) = translate::event_to_local(event, &event_host, &event_ids) else {
                    continue;
                };
                event_bus.publish(local);
            }
            finish_pump(&event_pumps, &event_host, &event_endpoint);
        });

        let mut states = endpoint.state_changes();
        let state_ids = self.ids.clone();
        let state_host = host;
        let state_bus = events;
        let state_mirror = Arc::clone(&self.mirror);
        let state_reattach = Arc::clone(&self.awaiting_reattach);
        let state_intents = Arc::clone(&self.session_intents);
        let state_threads = Arc::clone(&self.thread_registrations);
        let state_frames = self.terminal_frames.shared_attachments();
        let state_cancel = cancel;
        let state_pumps = Arc::clone(&self.endpoint_pumps);
        let state_endpoint = Arc::clone(&endpoint);
        let state_agent_mirror = self.agent_mirror();
        runtime.spawn(async move {
            loop {
                let state = *states.borrow_and_update();
                if state == LinkState::Down {
                    state_mirror.mark_stale(&state_host);
                    let attachments = sessions::attachments_from_shared(&state_frames, &state_host);
                    let tombstones = attachments
                        .into_iter()
                        .filter_map(|(local, attachment_count)| {
                            let (owner, remote) = state_ids.remote_terminal(local)?;
                            (owner == state_host).then_some(TerminalTombstone {
                                remote,
                                local,
                                attachment_count,
                            })
                        })
                        .collect::<Vec<_>>();
                    lock(&state_reattach).insert(state_host.clone(), tombstones);
                    let cleared = state_ids.clear_host(&state_host);
                    for terminal in cleared.terminals {
                        state_bus.publish(Event::TerminalExited {
                            terminal,
                            code: None,
                        });
                    }
                } else if state == LinkState::Ready {
                    if let Some(snapshot) = endpoint.last_snapshot_seen() {
                        // The durable mirror learns the host's threads before anything asks for
                        // one, so a reconnect leaves every header cached even if no tab is open.
                        if let Some(mirror) = &state_agent_mirror {
                            mirror.adopt(&state_host, &snapshot.agent_threads).await;
                        }
                        state_mirror.apply(&state_host, snapshot.clone());
                        state_ids.replace_host_inventory(
                            &state_host,
                            &snapshot.worktrees,
                            &snapshot.agent_threads,
                        );
                        agents::register_mirror_threads(
                            &state_threads,
                            &state_host,
                            &snapshot.agent_threads,
                            &state_ids,
                        );
                    }
                    let reattached = restore_attached_sessions(
                        &endpoint,
                        &state_host,
                        &state_ids,
                        &state_intents,
                        &state_reattach,
                    )
                    .await;
                    state_bus.request_snapshot_current();
                    for terminal in reattached {
                        state_bus.publish(Event::TerminalReattach { terminal });
                    }
                }
                let version = endpoint.hello().map(|hello| hello.version);
                state_bus.publish(Event::HostLinkChanged {
                    host: state_host.clone(),
                    link: state,
                    version,
                    error: (state == LinkState::Down)
                        .then(|| format!("host {state_host} is unreachable")),
                });
                tokio::select! {
                    () = state_cancel.cancelled() => break,
                    changed = states.changed() => if changed.is_err() { break },
                }
            }
            finish_pump(&state_pumps, &state_host, &state_endpoint);
        });
    }
}

impl Resolver for Router {
    fn host_of_worktree(&self, id: &WorktreeId) -> Option<HostId> {
        self.ids
            .host_of_worktree(id)
            .or_else(|| self.mirror.host_of_worktree(id))
    }
    fn host_of_session(&self, id: &str) -> Option<HostId> {
        self.ids.host_of_session(id).or_else(|| {
            let (host, _) = self.ids.remote_session(id)?;
            (self.machines.get(&host).is_some()
                || self
                    .machines
                    .endpoints()
                    .iter()
                    .any(|(candidate, _)| candidate == &host))
            .then_some(host)
        })
    }
    fn host_of_terminal(&self, id: TerminalId) -> Option<HostId> {
        self.ids.host_of_terminal(id)
    }
    fn host_of_job(&self, id: &JobId) -> Option<HostId> {
        self.ids.host_of_job(id)
    }
    fn host_of_thread(&self, id: &ThreadId) -> Option<HostId> {
        self.ids.host_of_thread(id)
    }
}

fn unreachable(host: &HostId) -> DaemonError {
    DaemonError::Remote(format!("host {host} is unreachable"))
}

fn annotate_remote_error(host: &HostId, error: DaemonError) -> DaemonError {
    match error {
        DaemonError::Unsupported(message) => {
            DaemonError::Unsupported(format!("host {host}: {message}"))
        }
        error => error,
    }
}

fn remember_session_intent(
    intents: &Mutex<BTreeMap<HostId, Vec<SessionIntent>>>,
    host: &HostId,
    intent: SessionIntent,
) {
    let mut intents = lock(intents);
    let host_intents = intents.entry(host.clone()).or_default();
    if let Some(existing) = host_intents
        .iter_mut()
        .find(|existing| existing.request == intent.request)
    {
        *existing = intent;
    } else {
        host_intents.push(intent);
    }
}

async fn restore_attached_sessions(
    endpoint: &Arc<dyn RemoteEndpoint>,
    host: &HostId,
    ids: &RemoteIds,
    intents: &Mutex<BTreeMap<HostId, Vec<SessionIntent>>>,
    awaiting: &Mutex<BTreeMap<HostId, Vec<TerminalTombstone>>>,
) -> Vec<TerminalId> {
    let tombstones = lock(awaiting).get(host).cloned().unwrap_or_default();
    if tombstones.is_empty() {
        lock(awaiting).remove(host);
        return Vec::new();
    }
    let host_intents = lock(intents).get(host).cloned().unwrap_or_default();
    let mut updated = Vec::new();
    let mut restored = Vec::new();
    for mut intent in host_intents {
        match endpoint.request(intent.request.clone()).await {
            Ok(ResponseBody::Session(session)) => {
                for terminal in &session.terminals {
                    let Some(previous) = intent
                        .previous
                        .terminals
                        .iter()
                        .find(|previous| previous.name == terminal.name)
                    else {
                        continue;
                    };
                    if let Some(tombstone) = tombstones.iter().find(|tombstone| {
                        tombstone.remote == previous.id && tombstone.attachment_count > 0
                    }) {
                        ids.restore_terminal(host, terminal.id, tombstone.local);
                        restored.push(tombstone.local);
                    }
                }
                intent.previous = session;
            }
            Ok(other) => tracing::warn!(
                %host,
                ?other,
                "remote session re-ensure returned an unexpected response"
            ),
            Err(error) => tracing::warn!(%host, %error, "failed to re-ensure remote session"),
        }
        updated.push(intent);
    }
    if !updated.is_empty() {
        lock(intents).insert(host.clone(), updated);
    }
    restored.sort_unstable();
    restored.dedup();
    if restored.len() == tombstones.len() {
        lock(awaiting).remove(host);
    }
    restored
}

fn finish_pump(
    pumps: &Mutex<BTreeMap<HostId, EndpointPump>>,
    host: &HostId,
    endpoint: &Arc<dyn RemoteEndpoint>,
) {
    let mut pumps = lock(pumps);
    if pumps
        .get(host)
        .is_some_and(|current| Arc::ptr_eq(&current.endpoint, endpoint))
        && let Some(current) = pumps.remove(host)
    {
        current.cancel.cancel();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
