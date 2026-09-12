//! Daemon-owned session and terminal lifecycle orchestration.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    thread,
    time::Instant,
};

use fleet_core::{
    agents::AttentionKind,
    config::{Agent, is_native_command},
    ids::{RepoId, SessionId, TerminalId, WorktreeId},
    paths::FleetHome,
    sessions::{
        AgentActivity, Session, SessionKind, SessionState, Terminal, TerminalKind, TerminalStatus,
        WorktreeStatus, WorktreeWindowStatus, agent_session_id, aggregate_agent_activity,
        default_terminals, worktree_agent_session_id,
    },
};
use fleet_proto::{
    event::Event,
    terminal::{FrameUpdate, KeyEvent, MouseEvent, ScrollCommand, WheelEvent},
};
use fleet_term::{HolderTarget, HostEvent, PtySource, TerminalHost, TerminalHostOptions};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, broadcast};

use crate::{
    DaemonError, DaemonResult,
    adapters::process::Process,
    error::remote_unsupported,
    server::BroadcastBus,
    stores::{config::ConfigStore, state::StateStore},
};

use super::agent_activity::AgentActivityTracker;
use holder::PtySidecar;
use host_bridge::{TerminalSpawn, spawn_terminal};

mod adoption;
pub(crate) mod holder;
mod host_bridge;
mod lifecycle;
mod observations;
mod registry;

const INITIAL_COLS: u16 = 120;
const INITIAL_ROWS: u16 = 36;

#[derive(Default)]
struct Registry {
    sessions: BTreeMap<SessionId, Session>,
    terminal_sessions: HashMap<TerminalId, SessionId>,
    hosts: HashMap<TerminalId, Arc<TerminalHost>>,
    attachments: HashMap<TerminalId, usize>,
    next_terminal: u64,
    next_sequences: HashMap<TerminalId, u64>,
    observed_output_bytes: HashMap<TerminalId, u64>,
    active_worktree: Option<SessionId>,
    observed_agents: HashMap<TerminalId, Option<String>>,
    activity_trackers: HashMap<TerminalId, AgentActivityTracker>,
    activity_changed_at: HashMap<TerminalId, String>,
}

/// One activity transition produced by a heuristic or explicit signal.
pub(crate) struct AgentActivityTransition {
    pub(crate) session: SessionId,
    pub(crate) terminal: TerminalId,
    pub(crate) agent: Option<String>,
    pub(crate) activity: AgentActivity,
    pub(crate) attention: Option<AttentionKind>,
    pub(crate) changed_at: String,
}

/// Runtime seam shared by the session and sleep services without persisting PTYs.
pub(crate) struct SessionRuntime {
    registry: Mutex<Registry>,
    /// Serializes EnsureSession repair per stable session id.
    ensure_locks: Mutex<HashMap<SessionId, Weak<EnsureLock>>>,
    terminal_transition_locks: Mutex<HashMap<SessionId, Weak<TransitionLock>>>,
    worktree_lifecycle_locks: Mutex<HashMap<WorktreeId, Weak<TransitionLock>>>,
    watches: super::watches::Watches,
    frames: broadcast::Sender<FrameUpdate>,
    process: Mutex<Option<Arc<dyn Process>>>,
    events: Mutex<Option<BroadcastBus>>,
}

/// One session's shared mutex. Lock futures retain the mutex, while claims retain this wrapper.
struct EnsureLock {
    mutex: Arc<AsyncMutex<()>>,
}

/// One serialized ensure operation. The final active or waiting claim removes its map entry.
struct EnsureLockClaim {
    runtime: Weak<SessionRuntime>,
    session: SessionId,
    lock: Arc<EnsureLock>,
    guard: Option<OwnedMutexGuard<()>>,
}

struct TransitionLock {
    mutex: Arc<AsyncMutex<()>>,
}

/// Exclusive ownership of one session transition or worktree lifecycle operation.
pub struct TransitionLockClaim {
    _lock: Arc<TransitionLock>,
    _guard: OwnedMutexGuard<()>,
}

impl Drop for EnsureLockClaim {
    fn drop(&mut self) {
        // Lock futures retain only the inner mutex, so this count represents claims exactly and
        // remains correct regardless of the order in which a cancelled future drops its fields.
        if Arc::strong_count(&self.lock) != 1 {
            return;
        }
        let Some(runtime) = self.runtime.upgrade() else {
            return;
        };
        let mut locks = runtime
            .ensure_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if Arc::strong_count(&self.lock) == 1
            && locks
                .get(&self.session)
                .is_some_and(|entry| Weak::ptr_eq(entry, &Arc::downgrade(&self.lock)))
        {
            locks.remove(&self.session);
        }
    }
}

/// Daemon-owned PTY session service and terminal-frame source.
#[derive(Clone)]
pub struct Sessions {
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    /// Filesystem layout, for the `pty/` directory holders and their sidecars live in.
    home: FleetHome,
    pub(super) runtime: Arc<SessionRuntime>,
}

impl Sessions {
    /// Shared watch registry for this terminal runtime.
    #[must_use]
    pub fn watches(&self) -> super::watches::Watches {
        self.runtime.watches.clone()
    }

    pub(crate) fn start_watch(
        &self,
        owner: u64,
        body: fleet_proto::request::RequestBody,
    ) -> DaemonResult<fleet_core::watches::WatchId> {
        use fleet_core::watches::{Watch, WatchId, WatchSource, WatchStatus};
        let fleet_proto::request::RequestBody::StartWatch {
            terminal,
            label,
            command,
            cwd,
            pid,
        } = body
        else {
            return Err(DaemonError::Validation("expected StartWatch".into()));
        };
        let registry = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = registry
            .terminal_sessions
            .get(&terminal)
            .cloned()
            .ok_or_else(|| DaemonError::NotFound(format!("terminal {terminal}")))?;
        // Keep the terminal lock until insertion, so close cannot leave an orphan watch.
        Ok(self.runtime.watches.start(
            owner,
            Watch {
                id: WatchId(0),
                session,
                terminal,
                label,
                command,
                cwd,
                pid,
                started_at: chrono::Utc::now().to_rfc3339(),
                status: WatchStatus::Running,
                source: WatchSource::Cooperative,
                log_file: None,
            },
        ))
    }

    /// Creates an empty runtime session registry.
    #[must_use]
    pub fn new(config: Arc<ConfigStore>, state: Arc<StateStore>) -> Self {
        let (frames, _) = broadcast::channel(256);
        let runtime = Arc::new(SessionRuntime::new(frames));
        // One source of truth for the layout: the store already knows the home it was rooted at.
        let home = FleetHome::new(config.home().to_path_buf());
        Self {
            config,
            state,
            home,
            runtime,
        }
    }

    /// Connects session mutations and PTY metadata changes to the daemon event bus.
    #[must_use]
    pub(crate) fn with_events(self, events: BroadcastBus) -> Self {
        self.runtime.register_events(events);
        self
    }

    /// Returns the worktree session most recently made active.
    #[must_use]
    pub fn current(&self) -> Option<SessionId> {
        self.runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_worktree
            .clone()
    }

    /// Lists daemon-owned runtime sessions in stable identity order.
    pub fn snapshot(&self) -> Vec<Session> {
        self.runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .values()
            .cloned()
            .collect()
    }

    pub(super) async fn claim_worktree_lifecycle(
        &self,
        worktree: WorktreeId,
    ) -> TransitionLockClaim {
        self.runtime.claim_worktree_lifecycle(worktree).await
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

fn validate_terminal_input(name: &str, command: &str) -> DaemonResult<()> {
    if name.trim().is_empty() {
        return Err(DaemonError::Validation(
            "terminal name must not be empty".to_owned(),
        ));
    }
    if command.trim().is_empty() {
        return Err(DaemonError::Validation(
            "terminal command must not be empty".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
