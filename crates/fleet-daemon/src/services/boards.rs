//! Board persistence, backend synchronization jobs, and card worktree orchestration.

use super::worktrees::Worktrees;
use crate::{
    DaemonError, DaemonResult,
    adapters::{board::BoardBackends, clock::Clock},
    jobs::{JobCtx, JobManager},
    server::BroadcastBus,
    stores::{board::BoardStore, state::StateStore},
};
use fleet_core::{
    board::*,
    ids::{BoardId, CardId, ContextId, HostId, JobId, RepoId, StatusId},
    model::Worktree,
};
use fleet_proto::{
    event::{BoardChangeReason, Event},
    job::JobKind,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, RwLock};

/// Coordinates board persistence, backend synchronization, and worktree creation.
#[derive(Clone)]
pub struct Boards {
    store: Arc<BoardStore>,
    state_store: Arc<StateStore>,
    backends: BoardBackends,
    clock: Arc<dyn Clock>,
    jobs: Arc<JobManager>,
    worktrees: Arc<Worktrees>,
    events: BroadcastBus,
    index: Arc<RwLock<HashMap<CardId, BoardId>>>,
    /// One lock per board. A single process-wide lock would let a clone or a backend sync on
    /// one board block every request for every other one until the client's timeout.
    gates: Arc<Mutex<HashMap<BoardId, Arc<Mutex<()>>>>>,
    /// The last reported load failure per board. The snapshot refresh rescans every document
    /// roughly every two seconds, and one unreadable file must not fill the log with it.
    unreadable: Arc<std::sync::Mutex<HashMap<BoardId, String>>>,
    /// Each board's summary and the document stamp it was parsed from.
    ///
    /// Every other snapshot field is served from memory; board summaries are the one thing a
    /// snapshot reads from disk, and a snapshot is assembled whenever a session changes — up
    /// to twenty times a second. Reparsing and revalidating every card, comment and activity
    /// entry of every board at that rate is pure waste when no board file has changed.
    summaries: Arc<RwLock<HashMap<BoardId, (DocumentStamp, BoardSummary)>>>,
}

/// The size and modification time a summary was parsed from.
type DocumentStamp = (u64, std::time::SystemTime);

mod cards;
mod documents;
mod lifecycle;
mod sync;
#[cfg(test)]
mod tests;
mod worktree;

use cards::{awaiting_push_baseline, new_card_id, require_push_baseline, validate_parent};
use worktree::scrub_repo;

impl Boards {
    /// Constructs board orchestration around shared daemon services and event bus.
    #[must_use]
    pub fn new(
        store: Arc<BoardStore>,
        state_store: Arc<StateStore>,
        backends: BoardBackends,
        clock: Arc<dyn Clock>,
        jobs: Arc<JobManager>,
        worktrees: Arc<Worktrees>,
        events: BroadcastBus,
    ) -> Self {
        Self {
            store,
            state_store,
            backends,
            clock,
            jobs,
            worktrees,
            events,
            index: Arc::new(RwLock::new(HashMap::new())),
            gates: Arc::new(Mutex::new(HashMap::new())),
            unreadable: Arc::new(std::sync::Mutex::new(HashMap::new())),
            summaries: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Serializes the read-modify-write cycles of one board without touching the others.
    async fn gate(&self, id: &BoardId) -> tokio::sync::OwnedMutexGuard<()> {
        let board = {
            let mut gates = self.gates.lock().await;
            Arc::clone(gates.entry(id.clone()).or_default())
        };
        board.lock_owned().await
    }

    fn now(&self) -> String {
        self.clock.now().to_rfc3339()
    }

    fn changed(&self, id: &BoardId, reason: BoardChangeReason) {
        self.events.publish(Event::BoardChanged {
            board_id: id.clone(),
            reason,
        });
        self.events.request_snapshot_current();
    }
}
