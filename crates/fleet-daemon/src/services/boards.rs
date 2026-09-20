//! Board persistence, backend synchronization jobs, and card worktree orchestration.

use super::{
    repos::RepoContextMover,
    worktrees::{WorktreeCascade, Worktrees},
};
use crate::{
    DaemonError, DaemonResult,
    adapters::{board::BoardBackends, clock::Clock},
    jobs::{JobCtx, JobManager},
    server::BroadcastBus,
    stores::{board::BoardStore, state::StateStore},
};
use fleet_core::{
    board::*,
    ids::{BoardId, CardId, ContextId, HostId, JobId, RepoId, StatusId, WorktreeId},
    model::{Context, Worktree},
    state::State,
};
use fleet_proto::{
    event::{BoardChangeReason, Event},
    job::JobKind,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, RwLock};

/// The worktrees other hosts own, as this daemon last mirrored them.
///
/// A worktree board may be scoped to a mirrored remote worktree, and those worktrees never
/// reach the local `StateStore` — they live only in the router's mirror. The trait is the
/// late-bound seam that lets `Boards` see them without naming the mirror's type, in the same
/// shape as [`WorktreeCascade`] (`docs/BOARD.md` §4).
pub(super) trait RemoteWorktrees: Send + Sync {
    /// Every mirrored worktree currently known, each carrying its owning host.
    fn worktrees(&self) -> Vec<Worktree>;
}

impl RemoteWorktrees for super::mirror::Mirror {
    fn worktrees(&self) -> Vec<Worktree> {
        super::mirror::Mirror::worktrees(self)
    }
}

/// Where the boards service found a worktree it was asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorktreeOrigin {
    /// Published by this daemon and present in its state.
    Local,
    /// Owned by another host and seen only through the mirror.
    Mirrored,
}

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
    /// Serializes the short choose-id-and-save section across differently based boards.
    /// Per-board gates cannot protect suffixes shared by distinct base ids.
    allocation: Arc<Mutex<()>>,
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
    /// The mirror of the worktrees other hosts own, installed once composition has built it.
    /// Absent in unit tests that compose `Boards` alone, which then see local worktrees only.
    remote_worktrees: Arc<std::sync::OnceLock<Arc<dyn RemoteWorktrees>>>,
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
            allocation: Arc::new(Mutex::new(())),
            unreadable: Arc::new(std::sync::Mutex::new(HashMap::new())),
            summaries: Arc::new(RwLock::new(HashMap::new())),
            remote_worktrees: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Installs the mirrored view of remote worktrees, once composition has built the mirror.
    pub(super) fn set_remote_worktrees(&self, remote: Arc<dyn RemoteWorktrees>) {
        if self.remote_worktrees.set(remote).is_err() {
            tracing::warn!("the boards service already has a remote worktree view");
        }
    }

    /// The mirrored worktrees, gathered once so a scan over every board takes one snapshot.
    fn mirrored_worktrees(&self) -> Vec<Worktree> {
        self.remote_worktrees
            .get()
            .map_or_else(Vec::new, |remote| remote.worktrees())
    }

    /// The worktree `id` names, whether this daemon published it or a host it mirrors owns it.
    fn known_worktree(&self, state: &State, id: &WorktreeId) -> Option<(Worktree, WorktreeOrigin)> {
        find_worktree(state, &self.mirrored_worktrees(), id)
            .map(|(worktree, origin)| (worktree.clone(), origin))
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

/// Finds a worktree in this daemon's state, then among the worktrees it mirrors.
///
/// Local state wins: a worktree this daemon published is authoritative over a fragment that
/// still lists it, and only one of the two can be acted on locally anyway.
fn find_worktree<'a>(
    state: &'a State,
    mirrored: &'a [Worktree],
    id: &WorktreeId,
) -> Option<(&'a Worktree, WorktreeOrigin)> {
    state
        .worktrees
        .iter()
        .find(|worktree| worktree.id == *id)
        .map(|worktree| (worktree, WorktreeOrigin::Local))
        .or_else(|| {
            mirrored
                .iter()
                .find(|worktree| worktree.id == *id)
                .map(|worktree| (worktree, WorktreeOrigin::Mirrored))
        })
}

/// Whether `id` names a worktree this daemon published or one it mirrors.
fn worktree_exists(state: &State, mirrored: &[Worktree], id: &WorktreeId) -> bool {
    find_worktree(state, mirrored, id).is_some()
}
