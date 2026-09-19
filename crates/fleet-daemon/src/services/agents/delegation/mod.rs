//! The delegation service: the six wire verbs, and the worker that drains the outbox.
//!
//! A delegation is two things (`docs/NATIVE-AGENTS.md` §15): a perfectly ordinary agent thread,
//! owned by [`AgentSessionManager`] like any other, and a durable record of why it exists and
//! where its answer goes. This module owns the second half. It is split so the two halves can
//! never be confused for each other:
//!
//! - [`transition`] holds the pure rules and runs **inside the writer's transaction**, so a
//!   child's status moves in the same commit as the event that moved it. It imports no manager.
//! - `run`, `complete`, `queries` and `cancel` answer the request verbs, outside every lock.
//! - [`worker`] performs the follow-up actions the transition half queued in the outbox — the
//!   sends, the nudges, the mirrored transcript rows and the delivery — one at a time, on its own
//!   task, holding nothing.
//!
//! The wake channel is what joins them: the store pokes it after a transaction that enqueued
//! work commits, and the worker drains. It is only ever a *hint* — the worker also drains at
//! start and once per [`limits::RETRY_TICK`], so a lost wake costs latency and never an action.

mod cancel;
mod complete;
pub(crate) mod footer;
pub(crate) mod limits;
#[cfg(all(test, feature = "real-agents"))]
#[path = "tests/live.rs"]
mod live;
mod queries;
#[cfg(test)]
#[path = "tests/recovery.rs"]
mod recovery;
pub(in crate::services::agents) mod run;
#[cfg(test)]
mod tests;
pub(crate) mod transition;
mod worker;

use std::{collections::HashSet, sync::Arc};

use fleet_core::{
    agents::{AgentKind, Delegation, DelegationId, ModelSelection, PermissionMode, ThreadId},
    ids::WorktreeId,
};
use fleet_proto::event::Event;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    server::BroadcastBus,
    services::{agents::store::DelegationHooks, worktrees::Worktrees},
    stores::config::ConfigStore,
};

use super::{AgentSessionManager, store::SqliteAgentStore};
use limits::RETRY_TICK;

/// The durable half of delegation: the six verbs, and the state behind them.
#[derive(Clone)]
pub(crate) struct DelegationService {
    inner: Arc<Inner>,
}

struct Inner {
    /// The one place a delegation row is read or written.
    ///
    store: SqliteAgentStore,
    /// The only way this service touches a thread: create it, send to it, patch its transcript.
    manager: AgentSessionManager,
    /// Where `Event::DelegationChanged` goes.
    events: BroadcastBus,
    /// Poked by the store after a committed transaction enqueued outbox work.
    wake: mpsc::UnboundedSender<()>,
    /// Names the provider executables `run` refuses a delegation against when one is missing.
    config: Arc<ConfigStore>,
    /// Resolves the worktree a child is started in.
    worktrees: Worktrees,
    /// Locally accepted submissions awaiting their durable transcript-origin commit.
    in_flight_rows: std::sync::Mutex<HashSet<i64>>,
}

impl DelegationService {
    /// Builds the service and the worker that owns the receive half of its wake channel.
    ///
    /// The two are returned together because they are two halves of one thing: composition
    /// installs the store hooks from the service's sender and spawns the worker, and a service
    /// whose worker was never spawned would enqueue work nothing performs.
    pub(crate) fn new(
        store: SqliteAgentStore,
        manager: AgentSessionManager,
        events: BroadcastBus,
        config: Arc<ConfigStore>,
        worktrees: Worktrees,
    ) -> (Self, DelegationWorker) {
        let (wake, received) = mpsc::unbounded_channel();
        let service = Self {
            inner: Arc::new(Inner {
                store,
                manager,
                events,
                wake,
                config,
                worktrees,
                in_flight_rows: std::sync::Mutex::new(HashSet::new()),
            }),
        };
        let worker = DelegationWorker {
            service: service.clone(),
            wake: received,
        };
        (service, worker)
    }

    /// The sender the store's hooks poke after a transaction that enqueued work commits.
    pub(crate) fn wake_sender(&self) -> mpsc::UnboundedSender<()> {
        self.inner.wake.clone()
    }

    /// Publishes one delegation's current state to every subscribed client.
    ///
    /// Best effort by construction: the bus is bounded and may lag, so every surface treats
    /// `DelegationChanged` as a repaint hint and reads the record when it needs the truth.
    pub(crate) fn publish_changed(&self, delegation: Delegation) {
        self.inner
            .events
            .publish(Event::DelegationChanged(delegation));
    }
}

/// Everything `DelegationRun` carries, after the wire shape is left behind.
///
/// Read by `run.rs` after dispatch has left the wire shape behind.
pub(crate) struct RunRequest {
    pub caller: ThreadId,
    pub provider: AgentKind,
    pub brief: String,
    pub expectation: String,
    pub worktree: Option<WorktreeId>,
    pub mode: Option<PermissionMode>,
    pub model: Option<ModelSelection>,
    pub title: Option<String>,
    /// Absolute path of the `fleet` the caller itself ran, when it could resolve its own.
    ///
    /// Advisory: it describes the caller's host, so it may be absent, stale, or name a path this
    /// daemon does not have. `run` treats it as a hint and falls back rather than refusing.
    pub fleet_path: Option<String>,
    pub eager: bool,
}

/// Everything `DelegationComplete` carries after dispatch has left the wire shape behind.
pub(crate) struct CompleteRequest {
    pub delegation: DelegationId,
    pub child: ThreadId,
    pub token: String,
    pub result: String,
    pub blocked: bool,
}

/// The task that performs every queued follow-up action, for the life of the daemon.
pub(crate) struct DelegationWorker {
    service: DelegationService,
    wake: mpsc::UnboundedReceiver<()>,
}

impl DelegationWorker {
    /// Drains the outbox at start, on every wake, and once per tick, until the daemon stops.
    ///
    /// The drain at start is not an optimisation: an action queued by the previous run is still
    /// open in the database, and nothing else will ever wake for it.
    pub(crate) async fn run(mut self, shutdown: CancellationToken) {
        self.drain_once().await;
        let mut retry = tokio::time::interval(RETRY_TICK);
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // `interval` fires immediately; the drain above already covered that tick.
        retry.tick().await;
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                _ = retry.tick() => self.drain_once().await,
                woken = self.wake.recv() => match woken {
                    Some(()) => self.drain_once().await,
                    // Every sender is gone, which happens only once the store and the service
                    // have both been dropped: there is nothing left that could enqueue work.
                    None => break,
                },
            }
        }
    }

    /// One drain pass, with its failure reported rather than propagated.
    ///
    /// A pass that fails must not end the worker: the rows it could not perform are still open,
    /// and the next wake or tick tries them again.
    async fn drain_once(&self) {
        if let Err(error) = worker::drain(&self.service).await {
            tracing::warn!(
                target: "fleet::agents",
                error = %format!("{error:#}"),
                "the delegation outbox could not be drained",
            );
        }
    }
}

/// Builds the delegation service for a manager and installs its hooks on the store.
///
/// `None` when the agent database could not be opened: the manager already refuses every agent
/// request with that reason, and a delegation service over a store that does not exist could only
/// refuse the same requests less clearly.
pub(crate) fn install(
    manager: &AgentSessionManager,
    events: &BroadcastBus,
    config: &Arc<ConfigStore>,
    worktrees: &Worktrees,
) -> Option<(DelegationService, DelegationWorker)> {
    let store = manager.delegation_store()?;
    let (service, worker) = DelegationService::new(
        store.clone(),
        manager.clone(),
        events.clone(),
        Arc::clone(config),
        worktrees.clone(),
    );
    // The publisher captures the bus and *not* the service: the store holds these hooks for its
    // whole life, and a service clone in here would close a reference cycle through the store's
    // own handle that nothing could ever break.
    let bus = events.clone();
    store.install_delegation_hooks(DelegationHooks {
        wake: service.wake_sender(),
        changed: Arc::new(move |delegation: Delegation| {
            bus.publish(Event::DelegationChanged(delegation));
        }),
    });
    Some((service, worker))
}
