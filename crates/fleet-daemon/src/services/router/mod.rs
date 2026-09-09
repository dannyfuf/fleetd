//! Request classification, remote forwarding, and remote endpoint event pumps.

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use fleet_core::{
    agents::ThreadId,
    ids::{HostId, JobId, TerminalId, WorktreeId},
};
use fleet_proto::{
    event::Event, request::RequestBody, response::ResponseBody, snapshot::LinkState,
};
use futures_util::future::join_all;

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
    pumped_hosts: Mutex<BTreeSet<HostId>>,
    awaiting_reattach: Arc<Mutex<BTreeSet<HostId>>>,
}

impl Router {
    #[must_use]
    pub fn new(machines: Arc<Machines>, mirror: Arc<Mirror>) -> Self {
        Self::with_ids(machines, mirror, RemoteIds::default())
    }

    /// Builds a router around an explicitly shared id allocator.
    #[must_use]
    pub fn with_ids(machines: Arc<Machines>, mirror: Arc<Mirror>, ids: RemoteIds) -> Self {
        let _ = agents::register_thread_events;
        let _ = create::ensure_repo_then_create;
        let _ = lifecycle::merge_lifecycle_fanout;
        let _ = sessions::on_attach;
        let _ = sessions::on_detach;
        Self {
            ids,
            mirror,
            machines,
            event_bus: Mutex::new(None),
            pumped_hosts: Mutex::new(BTreeSet::new()),
            awaiting_reattach: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    #[must_use]
    pub fn route(&self, body: &RequestBody) -> Target {
        classify::classify(body, self)
    }

    pub async fn forward(&self, host: &HostId, body: RequestBody) -> DaemonResult<ResponseBody> {
        let endpoint = self.endpoint(host)?;
        if endpoint.state() == LinkState::Down {
            return Err(unreachable(host));
        }
        self.pump_endpoint(host.clone(), Arc::clone(&endpoint));
        let remote = translate::to_remote(body, host, &self.ids)?;
        endpoint
            .request(remote)
            .await
            .map(|response| translate::response_to_local(response, host, &self.ids))
    }

    pub async fn fanout(
        &self,
        parts: Vec<(HostId, RequestBody)>,
    ) -> Vec<(HostId, DaemonResult<ResponseBody>)> {
        join_all(parts.into_iter().map(|(host, body)| async move {
            let result = match self.forward(&host, body.clone()).await {
                Err(error) => {
                    translate::unavailable_fanout_response(&body, &host, &error).ok_or(error)
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
        for (host, endpoint) in self.machines.endpoints() {
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
        if !lock(&self.pumped_hosts).insert(host.clone()) {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            lock(&self.pumped_hosts).remove(&host);
            return;
        };

        let mut remote_events = endpoint.events();
        let event_ids = self.ids.clone();
        let event_host = host.clone();
        let event_bus = events.clone();
        let event_mirror = Arc::clone(&self.mirror);
        let event_reattach = Arc::clone(&self.awaiting_reattach);
        runtime.spawn(async move {
            loop {
                let event = match remote_events.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(%event_host, skipped, "remote event pump lagged");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if let Event::SnapshotChanged(snapshot) = &event {
                    event_mirror.apply(&event_host, snapshot.clone());
                }
                let Some(local) = translate::event_to_local(event, &event_host, &event_ids) else {
                    continue;
                };
                let reattach = if matches!(local, Event::SnapshotChanged(_))
                    && lock(&event_reattach).remove(&event_host)
                {
                    match &local {
                        Event::SnapshotChanged(snapshot) => snapshot
                            .sessions
                            .iter()
                            .flat_map(|session| {
                                session.terminals.iter().map(|terminal| terminal.id)
                            })
                            .collect::<Vec<_>>(),
                        _ => Vec::new(),
                    }
                } else {
                    Vec::new()
                };
                event_bus.publish(local);
                for terminal in reattach {
                    event_bus.publish(Event::TerminalReattach { terminal });
                }
            }
        });

        let mut states = endpoint.state_changes();
        let state_ids = self.ids.clone();
        let state_host = host;
        let state_bus = events;
        let state_mirror = Arc::clone(&self.mirror);
        let state_reattach = Arc::clone(&self.awaiting_reattach);
        runtime.spawn(async move {
            while states.changed().await.is_ok() {
                let state = *states.borrow_and_update();
                if state == LinkState::Down {
                    state_mirror.mark_stale(&state_host);
                    lock(&state_reattach).insert(state_host.clone());
                    let cleared = state_ids.clear_host(&state_host);
                    for terminal in cleared.terminals {
                        state_bus.publish(Event::TerminalExited {
                            terminal,
                            code: None,
                        });
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
            }
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

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
