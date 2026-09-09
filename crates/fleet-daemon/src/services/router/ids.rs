//! Bidirectional local/remote identifier mappings.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use fleet_core::{
    agents::ThreadId,
    ids::{HostId, JobId, TerminalId, WorktreeId},
};

/// Identifiers removed when a host link goes down.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ClearedIds {
    pub terminals: Vec<TerminalId>,
    pub sessions: Vec<String>,
    pub jobs: Vec<JobId>,
    pub worktrees: Vec<WorktreeId>,
    pub threads: Vec<ThreadId>,
}

#[derive(Default)]
struct Maps {
    terminal_local: BTreeMap<(HostId, TerminalId), TerminalId>,
    terminal_remote: BTreeMap<TerminalId, (HostId, TerminalId)>,
    job_local: BTreeMap<(HostId, JobId), JobId>,
    job_remote: BTreeMap<JobId, (HostId, JobId)>,
    worktrees: BTreeMap<WorktreeId, HostId>,
    threads: BTreeMap<ThreadId, HostId>,
    sessions: BTreeSet<String>,
}

/// Shared remote-id table used by request, response, event, and snapshot translation.
pub struct RemoteIds {
    next: Arc<AtomicU64>,
    maps: Arc<Mutex<Maps>>,
}

impl Clone for RemoteIds {
    fn clone(&self) -> Self {
        Self {
            next: Arc::clone(&self.next),
            maps: Arc::clone(&self.maps),
        }
    }
}

impl Default for RemoteIds {
    fn default() -> Self {
        // Production composition replaces this with the session runtime's allocator. Keeping the
        // standalone default in the upper half prevents collisions before that wiring is present.
        Self::new(Arc::new(AtomicU64::new(1 << 63)))
    }
}

impl RemoteIds {
    #[must_use]
    pub fn new(next: Arc<AtomicU64>) -> Self {
        Self {
            next,
            maps: Arc::new(Mutex::new(Maps::default())),
        }
    }

    pub fn local_terminal(&self, host: &HostId, remote: TerminalId) -> TerminalId {
        let mut maps = lock(&self.maps);
        if let Some(local) = maps.terminal_local.get(&(host.clone(), remote)) {
            return *local;
        }
        let local = TerminalId(self.next.fetch_add(1, Ordering::Relaxed));
        maps.terminal_local.insert((host.clone(), remote), local);
        maps.terminal_remote.insert(local, (host.clone(), remote));
        local
    }

    #[must_use]
    pub fn existing_local_terminal(&self, host: &HostId, remote: TerminalId) -> Option<TerminalId> {
        lock(&self.maps)
            .terminal_local
            .get(&(host.clone(), remote))
            .copied()
    }

    #[must_use]
    pub fn remote_terminal(&self, local: TerminalId) -> Option<(HostId, TerminalId)> {
        lock(&self.maps).terminal_remote.get(&local).cloned()
    }

    pub fn local_job(&self, host: &HostId, remote: &JobId) -> JobId {
        let mut maps = lock(&self.maps);
        if let Some(local) = maps.job_local.get(&(host.clone(), remote.clone())) {
            return local.clone();
        }
        let local = JobId::try_from(format!(
            "job-remote-{}",
            self.next.fetch_add(1, Ordering::Relaxed)
        ))
        .expect("generated job id is valid");
        maps.job_local
            .insert((host.clone(), remote.clone()), local.clone());
        maps.job_remote
            .insert(local.clone(), (host.clone(), remote.clone()));
        local
    }

    #[must_use]
    pub fn remote_job(&self, local: &JobId) -> Option<(HostId, JobId)> {
        lock(&self.maps).job_remote.get(local).cloned()
    }

    #[must_use]
    pub fn local_session(&self, host: &HostId, remote: &str) -> String {
        let local = format!("{host}/{remote}");
        lock(&self.maps).sessions.insert(local.clone());
        local
    }

    #[must_use]
    pub fn remote_session(&self, local: &str) -> Option<(HostId, String)> {
        let (host, remote) = local.split_once('/')?;
        Some((HostId::try_from(host).ok()?, remote.to_owned()))
    }

    pub fn register_worktree(&self, host: &HostId, worktree: WorktreeId) {
        lock(&self.maps).worktrees.insert(worktree, host.clone());
    }

    pub fn register_thread(&self, host: &HostId, thread: ThreadId) {
        lock(&self.maps).threads.insert(thread, host.clone());
    }

    pub fn forget_terminal(&self, local: TerminalId) {
        let mut maps = lock(&self.maps);
        if let Some(key) = maps.terminal_remote.remove(&local) {
            maps.terminal_local.remove(&key);
        }
    }

    pub fn forget_job(&self, local: &JobId) {
        let mut maps = lock(&self.maps);
        if let Some(key) = maps.job_remote.remove(local) {
            maps.job_local.remove(&key);
        }
    }

    pub fn forget_session(&self, local: &str) {
        lock(&self.maps).sessions.remove(local);
    }

    pub fn forget_worktree(&self, worktree: &WorktreeId) {
        lock(&self.maps).worktrees.remove(worktree);
    }

    pub fn forget_thread(&self, thread: ThreadId) {
        lock(&self.maps).threads.remove(&thread);
    }

    pub fn clear_host(&self, host: &HostId) -> ClearedIds {
        let mut maps = lock(&self.maps);
        let terminals = maps
            .terminal_remote
            .iter()
            .filter(|(_, (item_host, _))| item_host == host)
            .map(|(local, _)| *local)
            .collect::<Vec<_>>();
        let jobs = maps
            .job_remote
            .iter()
            .filter(|(_, (item_host, _))| item_host == host)
            .map(|(local, _)| local.clone())
            .collect::<Vec<_>>();
        let threads = maps
            .threads
            .iter()
            .filter(|(_, item_host)| *item_host == host)
            .map(|(thread, _)| *thread)
            .collect::<Vec<_>>();
        let worktrees = maps
            .worktrees
            .iter()
            .filter(|(_, item_host)| *item_host == host)
            .map(|(worktree, _)| worktree.clone())
            .collect::<Vec<_>>();
        let sessions = maps
            .sessions
            .iter()
            .filter(|session| session.starts_with(&format!("{host}/")))
            .cloned()
            .collect::<Vec<_>>();
        maps.terminal_remote
            .retain(|_, (item_host, _)| item_host != host);
        maps.terminal_local
            .retain(|(item_host, _), _| item_host != host);
        maps.job_remote
            .retain(|_, (item_host, _)| item_host != host);
        maps.job_local.retain(|(item_host, _), _| item_host != host);
        maps.worktrees.retain(|_, item_host| item_host != host);
        maps.threads.retain(|_, item_host| item_host != host);
        maps.sessions
            .retain(|session| !session.starts_with(&format!("{host}/")));
        ClearedIds {
            terminals,
            sessions,
            jobs,
            worktrees,
            threads,
        }
    }

    #[must_use]
    pub fn host_of_worktree(&self, id: &WorktreeId) -> Option<HostId> {
        lock(&self.maps).worktrees.get(id).cloned()
    }
    #[must_use]
    pub fn host_of_session(&self, id: &str) -> Option<HostId> {
        lock(&self.maps)
            .sessions
            .contains(id)
            .then(|| self.remote_session(id))
            .flatten()
            .map(|(host, _)| host)
    }
    #[must_use]
    pub fn host_of_terminal(&self, id: TerminalId) -> Option<HostId> {
        self.remote_terminal(id).map(|(host, _)| host)
    }
    #[must_use]
    pub fn host_of_job(&self, id: &JobId) -> Option<HostId> {
        self.remote_job(id).map(|(host, _)| host)
    }
    #[must_use]
    pub fn host_of_thread(&self, id: &ThreadId) -> Option<HostId> {
        lock(&self.maps).threads.get(id).cloned()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
