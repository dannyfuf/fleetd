//! In-memory remote snapshot fragments used by the federating router.

use std::{collections::BTreeMap, sync::RwLock};

use fleet_core::{
    agents::AgentThreadSummary,
    ids::{HostId, WorktreeId},
    model::Worktree,
    sessions::{Session, WorktreeStatus},
};
use fleet_proto::snapshot::Snapshot;

/// One cached remote snapshot and its link freshness state.
#[derive(Debug, Clone)]
pub struct MirrorFragment {
    pub snapshot: Snapshot,
    pub stale: bool,
    pub received_at: String,
}

/// Thread-safe collection of authoritative remote snapshot fragments.
#[derive(Default)]
pub struct Mirror {
    fragments: RwLock<BTreeMap<HostId, MirrorFragment>>,
}

impl Mirror {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&self, host: &HostId, mut snapshot: Snapshot) {
        for worktree in &mut snapshot.worktrees {
            worktree.host = Some(host.clone());
        }
        let fragment = MirrorFragment {
            snapshot,
            stale: false,
            received_at: chrono::Utc::now().to_rfc3339(),
        };
        write(&self.fragments).insert(host.clone(), fragment);
    }

    pub fn mark_stale(&self, host: &HostId) {
        if let Some(fragment) = write(&self.fragments).get_mut(host) {
            fragment.stale = true;
        }
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
                        session.id =
                            fleet_core::ids::SessionId::try_from(format!("{host}/{}", session.id))
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
            .flat_map(|fragment| fragment.snapshot.statuses.clone())
            .collect()
    }

    #[must_use]
    pub fn agent_threads(&self) -> Vec<AgentThreadSummary> {
        read(&self.fragments)
            .values()
            .flat_map(|fragment| fragment.snapshot.agent_threads.clone())
            .collect()
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
