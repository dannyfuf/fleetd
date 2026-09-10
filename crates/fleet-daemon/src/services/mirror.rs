//! In-memory remote snapshot fragments used by the federating router.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use fleet_core::{
    agents::{AgentThreadSummary, ThreadId},
    ids::{HostId, SessionId, WorktreeId},
    model::Worktree,
    sessions::{AgentActivity, Session, SessionState, WorktreeStatus},
};
use fleet_proto::{
    event::{Event, EventKind},
    request::RequestBody,
    response::ResponseBody,
    snapshot::{LinkState, Snapshot},
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::{
    DaemonError, DaemonResult,
    machines::{Machines, RemoteEndpoint},
    server::BroadcastBus,
};

/// One cached remote snapshot and its link freshness state.
#[derive(Debug, Clone)]
pub struct MirrorFragment {
    pub snapshot: Snapshot,
    pub stale: bool,
    pub received_at: String,
}

struct Observer {
    endpoint: Arc<dyn RemoteEndpoint>,
    cancel: CancellationToken,
}

/// Thread-safe collection of authoritative remote snapshot fragments.
#[derive(Default)]
pub struct Mirror {
    fragments: RwLock<BTreeMap<HostId, MirrorFragment>>,
    observers: Mutex<BTreeMap<HostId, Observer>>,
}

impl Mirror {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reconciles background endpoint observers with the current host configuration.
    pub fn reconcile(
        self: &Arc<Self>,
        configured: &BTreeSet<HostId>,
        machines: &Machines,
        events: &BroadcastBus,
    ) {
        let endpoints = configured
            .iter()
            .filter_map(|host| {
                machines
                    .endpoint(host)
                    .map(|endpoint| (host.clone(), endpoint))
            })
            .collect::<BTreeMap<_, _>>();
        let mut observers = lock(&self.observers);
        let removed = observers
            .keys()
            .filter(|host| !endpoints.contains_key(*host))
            .cloned()
            .collect::<Vec<_>>();
        let mut snapshot_changed = false;
        for host in removed {
            if let Some(observer) = observers.remove(&host) {
                observer.cancel.cancel();
            }
            if configured.contains(&host) {
                snapshot_changed |= self.mark_stale_changed(&host);
            } else {
                snapshot_changed |= write(&self.fragments).remove(&host).is_some();
            }
        }

        for (host, endpoint) in endpoints {
            let unchanged = observers
                .get(&host)
                .is_some_and(|observer| Arc::ptr_eq(&observer.endpoint, &endpoint));
            if unchanged {
                continue;
            }
            if let Some(previous) = observers.remove(&host) {
                previous.cancel.cancel();
                snapshot_changed |= self.mark_stale_changed(&host);
            }
            let cancel = CancellationToken::new();
            observers.insert(
                host.clone(),
                Observer {
                    endpoint: Arc::clone(&endpoint),
                    cancel: cancel.clone(),
                },
            );
            tokio::spawn(Arc::clone(self).observe(host, endpoint, events.clone(), cancel));
        }
        drop(observers);
        if snapshot_changed {
            events.request_snapshot_current();
        }
    }

    pub fn apply(&self, host: &HostId, snapshot: Snapshot) {
        self.apply_changed(host, snapshot);
    }

    fn apply_changed(&self, host: &HostId, snapshot: Snapshot) -> bool {
        let fragment = MirrorFragment {
            snapshot,
            stale: false,
            received_at: chrono::Utc::now().to_rfc3339(),
        };
        write(&self.fragments).insert(host.clone(), fragment);
        true
    }

    pub fn mark_stale(&self, host: &HostId) {
        self.mark_stale_changed(host);
    }

    fn mark_stale_changed(&self, host: &HostId) -> bool {
        let mut fragments = write(&self.fragments);
        let Some(fragment) = fragments.get_mut(host) else {
            return false;
        };
        let changed = !fragment.stale;
        fragment.stale = true;
        changed
    }

    pub fn clear(&self, host: &HostId) {
        write(&self.fragments).remove(host);
    }

    #[must_use]
    pub fn fragment(&self, host: &HostId) -> Option<MirrorFragment> {
        read(&self.fragments).get(host).cloned()
    }

    #[must_use]
    pub fn host_of_worktree(&self, id: &WorktreeId) -> Option<HostId> {
        read(&self.fragments).iter().find_map(|(host, fragment)| {
            fragment
                .snapshot
                .worktrees
                .iter()
                .any(|worktree| &worktree.id == id)
                .then(|| host.clone())
        })
    }

    #[must_use]
    pub fn worktrees(&self) -> Vec<Worktree> {
        read(&self.fragments)
            .iter()
            .flat_map(|(host, fragment)| {
                fragment
                    .snapshot
                    .worktrees
                    .iter()
                    .cloned()
                    .map(|mut worktree| {
                        worktree.host = Some(host.clone());
                        worktree.session = prefixed_session(host, &worktree.session);
                        worktree
                    })
            })
            .collect()
    }

    #[must_use]
    pub fn sessions(&self) -> Vec<Session> {
        read(&self.fragments)
            .iter()
            .flat_map(|(host, fragment)| {
                fragment
                    .snapshot
                    .sessions
                    .iter()
                    .cloned()
                    .filter_map(move |mut session| {
                        session.host = Some(host.clone());
                        session.id =
                            SessionId::try_from(prefixed_session(host, session.id.as_str()))
                                .ok()?;
                        Some(session)
                    })
            })
            .collect()
    }

    #[must_use]
    pub fn statuses(&self) -> Vec<WorktreeStatus> {
        read(&self.fragments)
            .values()
            .flat_map(|fragment| {
                let statuses = fragment
                    .snapshot
                    .statuses
                    .iter()
                    .map(|status| (&status.worktree_id, status))
                    .collect::<BTreeMap<_, _>>();
                fragment.snapshot.worktrees.iter().map(move |worktree| {
                    if fragment.stale {
                        unknown_status(worktree.id.clone())
                    } else {
                        statuses.get(&worktree.id).map_or_else(
                            || unknown_status(worktree.id.clone()),
                            |status| (*status).clone(),
                        )
                    }
                })
            })
            .collect()
    }

    #[must_use]
    pub fn agent_threads(&self) -> Vec<AgentThreadSummary> {
        read(&self.fragments)
            .iter()
            .flat_map(|(host, fragment)| {
                fragment
                    .snapshot
                    .agent_threads
                    .iter()
                    .cloned()
                    .map(|mut summary| {
                        summary.host = Some(host.clone());
                        summary
                    })
            })
            .collect()
    }

    /// Returns worktree ownership rows used to rebuild router mappings.
    #[must_use]
    pub fn worktree_ownerships(&self) -> Vec<(HostId, WorktreeId)> {
        let fragments = read(&self.fragments);
        fragments
            .iter()
            .flat_map(|(host, fragment)| {
                fragment
                    .snapshot
                    .worktrees
                    .iter()
                    .map(|worktree| (host.clone(), worktree.id.clone()))
            })
            .collect()
    }

    /// Returns native-agent thread ownership rows used to rebuild router mappings.
    #[must_use]
    pub fn thread_ownerships(&self) -> Vec<(HostId, ThreadId)> {
        read(&self.fragments)
            .iter()
            .flat_map(|(host, fragment)| {
                fragment
                    .snapshot
                    .agent_threads
                    .iter()
                    .map(|summary| (host.clone(), summary.thread))
            })
            .collect()
    }

    async fn observe(
        self: Arc<Self>,
        host: HostId,
        endpoint: Arc<dyn RemoteEndpoint>,
        events: BroadcastBus,
        cancel: CancellationToken,
    ) {
        let mut states = endpoint.state_changes();
        let mut remote_events = endpoint.events();
        loop {
            match endpoint.state() {
                LinkState::Ready => match refresh(endpoint.as_ref()).await {
                    Ok(snapshot) => {
                        self.apply_changed(&host, snapshot);
                        events.request_snapshot_current();
                        loop {
                            tokio::select! {
                                () = cancel.cancelled() => return,
                                changed = states.changed() => {
                                    if changed.is_err() || endpoint.state() != LinkState::Ready {
                                        break;
                                    }
                                }
                                event = remote_events.recv() => match event {
                                    Ok(event) => {
                                        if self.apply_event(&host, event) {
                                            events.request_snapshot_current();
                                        }
                                    }
                                    Err(broadcast::error::RecvError::Lagged(_)) => break,
                                    Err(broadcast::error::RecvError::Closed) => return,
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if self.mark_stale_changed(&host) {
                            events.request_snapshot_current();
                        }
                        tracing::warn!(%host, %error, "failed to refresh remote snapshot mirror");
                        tokio::select! {
                            () = cancel.cancelled() => return,
                            changed = states.changed() => {
                                if changed.is_err() { return; }
                            }
                            () = tokio::time::sleep(Duration::from_millis(250)) => {}
                        }
                    }
                },
                LinkState::Connecting | LinkState::Down | LinkState::Legacy => {
                    if self.mark_stale_changed(&host) {
                        events.request_snapshot_current();
                    }
                    tokio::select! {
                        () = cancel.cancelled() => return,
                        changed = states.changed() => {
                            if changed.is_err() { return; }
                        }
                    }
                }
            }
        }
    }

    fn apply_event(&self, host: &HostId, event: Event) -> bool {
        match event {
            Event::SnapshotChanged(snapshot) => self.apply_changed(host, snapshot),
            Event::SessionChanged(session) => self.update_session(host, session),
            Event::AgentSummary(summary) => self.update_thread(host, summary),
            _ => false,
        }
    }

    fn update_session(&self, host: &HostId, session: Session) -> bool {
        let mut fragments = write(&self.fragments);
        let Some(fragment) = fragments.get_mut(host) else {
            return false;
        };
        if let Some(existing) = fragment
            .snapshot
            .sessions
            .iter_mut()
            .find(|existing| existing.id == session.id)
        {
            *existing = session;
        } else {
            fragment.snapshot.sessions.push(session);
        }
        fragment.received_at = chrono::Utc::now().to_rfc3339();
        fragment.stale = false;
        true
    }

    fn update_thread(&self, host: &HostId, summary: AgentThreadSummary) -> bool {
        let mut fragments = write(&self.fragments);
        let Some(fragment) = fragments.get_mut(host) else {
            return false;
        };
        if let Some(existing) = fragment
            .snapshot
            .agent_threads
            .iter_mut()
            .find(|existing| existing.thread == summary.thread)
        {
            *existing = summary;
        } else {
            fragment.snapshot.agent_threads.push(summary);
        }
        fragment.received_at = chrono::Utc::now().to_rfc3339();
        fragment.stale = false;
        true
    }
}

async fn refresh(endpoint: &dyn RemoteEndpoint) -> DaemonResult<Snapshot> {
    let subscribed = endpoint
        .request(RequestBody::Subscribe {
            events: vec![
                EventKind::SnapshotChanged,
                EventKind::SessionChanged,
                EventKind::AgentSummary,
            ],
        })
        .await?;
    if subscribed != ResponseBody::Ack {
        return Err(DaemonError::Protocol(
            "remote Subscribe returned an unexpected response".to_owned(),
        ));
    }
    match endpoint.request(RequestBody::GetSnapshot).await? {
        ResponseBody::Snapshot(snapshot) => Ok(snapshot),
        _ => Err(DaemonError::Protocol(
            "remote GetSnapshot returned an unexpected response".to_owned(),
        )),
    }
}

fn prefixed_session(host: &HostId, session: &str) -> String {
    let prefix = format!("{host}/");
    if session.starts_with(&prefix) {
        session.to_owned()
    } else {
        format!("{prefix}{session}")
    }
}

fn unknown_status(worktree_id: WorktreeId) -> WorktreeStatus {
    WorktreeStatus {
        worktree_id,
        session: SessionState::Unknown,
        windows: Vec::new(),
        running: Vec::new(),
        agent_activity: AgentActivity::Unknown,
        agent_activity_changed_at: None,
    }
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
