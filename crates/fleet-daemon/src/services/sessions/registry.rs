use super::*;

use std::sync::{OnceLock, atomic::AtomicU64};

type TerminalIdCounters = BTreeMap<PathBuf, Arc<AtomicU64>>;

fn terminal_id_counters() -> &'static Mutex<TerminalIdCounters> {
    static COUNTERS: OnceLock<Mutex<TerminalIdCounters>> = OnceLock::new();
    COUNTERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

impl Sessions {
    /// Returns the allocator shared with remote terminal-id translation.
    pub(crate) fn terminal_id_counter(&self) -> Arc<AtomicU64> {
        let key = self.config.path().to_path_buf();
        Arc::clone(
            terminal_id_counters()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(key)
                .or_insert_with(|| Arc::new(AtomicU64::new(1))),
        )
    }
}

impl SessionRuntime {
    pub(super) fn new(frames: broadcast::Sender<FrameUpdate>) -> Self {
        Self {
            registry: Mutex::new(Registry {
                next_terminal: 1,
                ..Registry::default()
            }),
            ensure_locks: Mutex::new(HashMap::new()),
            terminal_transition_locks: Mutex::new(HashMap::new()),
            worktree_lifecycle_locks: Mutex::new(HashMap::new()),
            watches: crate::services::watches::Watches::default(),
            frames,
            process: Mutex::new(None),
            events: Mutex::new(None),
        }
    }

    pub(super) fn register_events(&self, events: BroadcastBus) {
        self.watches.with_events(events.clone());
        *self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(events);
    }

    pub(super) async fn claim_ensure_lock(self: &Arc<Self>, session: SessionId) -> EnsureLockClaim {
        let lock = {
            let mut locks = self
                .ensure_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            locks.retain(|_, lock| lock.strong_count() > 0);
            if let Some(lock) = locks.get(&session).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(EnsureLock {
                    mutex: Arc::new(AsyncMutex::new(())),
                });
                locks.insert(session.clone(), Arc::downgrade(&lock));
                lock
            }
        };
        // Construct the pruning guard before awaiting. If this future is cancelled while queued,
        // its Drop implementation removes the expired weak map entry after the lock future's Arc
        // has unwound.
        let mut claim = EnsureLockClaim {
            runtime: Arc::downgrade(self),
            session,
            lock,
            guard: None,
        };
        claim.guard = Some(Arc::clone(&claim.lock.mutex).lock_owned().await);
        claim
    }

    async fn claim_transition<K>(
        locks: &Mutex<HashMap<K, Weak<TransitionLock>>>,
        key: K,
    ) -> TransitionLockClaim
    where
        K: Eq + std::hash::Hash,
    {
        let lock = {
            let mut locks = locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            locks.retain(|_, lock| lock.strong_count() > 0);
            locks.get(&key).and_then(Weak::upgrade).unwrap_or_else(|| {
                let lock = Arc::new(TransitionLock {
                    mutex: Arc::new(AsyncMutex::new(())),
                });
                locks.insert(key, Arc::downgrade(&lock));
                lock
            })
        };
        let guard = Arc::clone(&lock.mutex).lock_owned().await;
        TransitionLockClaim {
            _lock: lock,
            _guard: guard,
        }
    }

    pub(crate) async fn claim_terminal_transition(
        self: &Arc<Self>,
        session: SessionId,
    ) -> TransitionLockClaim {
        Self::claim_transition(&self.terminal_transition_locks, session).await
    }

    pub(super) async fn claim_worktree_lifecycle(
        self: &Arc<Self>,
        worktree: WorktreeId,
    ) -> TransitionLockClaim {
        Self::claim_transition(&self.worktree_lifecycle_locks, worktree).await
    }

    fn prune_ensure_lock(&self, session: &SessionId) {
        let mut locks = self
            .ensure_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if locks
            .get(session)
            .is_some_and(|lock| lock.strong_count() == 0)
        {
            locks.remove(session);
        }
    }

    pub(crate) fn register_process(&self, process: Arc<dyn Process>) {
        *self
            .process
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(process);
    }

    pub(crate) fn process(&self) -> Option<Arc<dyn Process>> {
        self.process
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn session(&self, id: &SessionId) -> Option<Session> {
        self.registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .get(id)
            .cloned()
    }

    pub(crate) fn sessions(&self) -> Vec<Session> {
        self.registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .values()
            .cloned()
            .collect()
    }

    pub(crate) fn host(&self, terminal: TerminalId) -> Option<Arc<TerminalHost>> {
        self.registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .hosts
            .get(&terminal)
            .cloned()
    }

    pub(crate) fn record_sleep(
        &self,
        session: &SessionId,
        kept: Vec<fleet_core::sessions::KeptTerminal>,
    ) {
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = registry.sessions.get_mut(session) {
            entry.slept_at = Some(chrono::Utc::now().to_rfc3339());
            entry.kept_terminals = kept;
        }
        drop(registry);
        self.notify_session(session);
    }

    /// Drops every per-terminal record and returns the host that still needs killing.
    /// One place to edit whenever the registry gains another terminal-keyed map.
    fn forget_terminal(
        &self,
        registry: &mut Registry,
        terminal: TerminalId,
    ) -> Option<Arc<TerminalHost>> {
        self.watches.remove_terminal(terminal);
        registry.terminal_sessions.remove(&terminal);
        registry.attachments.remove(&terminal);
        registry.next_sequences.remove(&terminal);
        registry.observed_output_bytes.remove(&terminal);
        registry.observed_agents.remove(&terminal);
        registry.activity_trackers.remove(&terminal);
        registry.activity_changed_at.remove(&terminal);
        registry.hosts.remove(&terminal)
    }

    pub(crate) fn close_terminal_if_present(&self, terminal: TerminalId) -> Option<String> {
        let (name, host, removed_session) = {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let session_id = registry.terminal_sessions.get(&terminal).cloned()?;
            let host = self.forget_terminal(&mut registry, terminal);
            let session = registry.sessions.get_mut(&session_id)?;
            let position = session
                .terminals
                .iter()
                .position(|entry| entry.id == terminal)?;
            let name = session.terminals.remove(position).name;
            if session.active_terminal == Some(terminal) {
                session.active_terminal = session.terminals.first().map(|entry| entry.id);
            }
            let removed_session = session.terminals.is_empty();
            if removed_session {
                registry.sessions.remove(&session_id);
                if registry.active_worktree.as_ref() == Some(&session_id) {
                    registry.active_worktree = None;
                }
            }
            (name, host, removed_session.then_some(session_id))
        };
        if let Some(host) = host {
            let _ = host.kill();
        }
        if let Some(session) = removed_session.as_ref() {
            self.prune_ensure_lock(session);
        }
        self.notify_snapshot();
        Some(name)
    }

    pub(crate) async fn close_terminal_gated(
        self: &Arc<Self>,
        terminal: TerminalId,
    ) -> Option<String> {
        let session = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .terminal_sessions
            .get(&terminal)
            .cloned()?;
        let _transition = self.claim_terminal_transition(session).await;
        self.close_terminal_if_present(terminal)
    }

    pub(crate) fn kill_if_present(&self, session: &SessionId) -> bool {
        let hosts = {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(removed) = registry.sessions.remove(session) else {
                return false;
            };
            if registry.active_worktree.as_ref() == Some(session) {
                registry.active_worktree = None;
            }
            removed
                .terminals
                .into_iter()
                .filter_map(|terminal| self.forget_terminal(&mut registry, terminal.id))
                .collect::<Vec<_>>()
        };
        for host in hosts {
            let _ = host.kill();
        }
        self.prune_ensure_lock(session);
        self.notify_snapshot();
        true
    }
}
