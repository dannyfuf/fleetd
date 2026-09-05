//! Daemon-owned session and terminal lifecycle orchestration.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, Weak},
    thread,
};

use fleet_core::{
    config::Agent,
    ids::{RepoId, SessionId, TerminalId, WorktreeId},
    sessions::{
        Session, SessionKind, SessionState, Terminal, TerminalStatus, WorktreeStatus,
        WorktreeWindowStatus, agent_session_id, default_terminals,
    },
};
use fleet_proto::{
    event::Event,
    terminal::{FrameUpdate, KeyEvent, MouseEvent, ScrollCommand, WheelEvent},
};
use fleet_term::{HostEvent, PtyOptions, TerminalHost, TerminalHostOptions};
use tokio::sync::broadcast;

use crate::{
    DaemonError, DaemonResult,
    adapters::process::Process,
    server::BroadcastBus,
    stores::{config::ConfigStore, state::StateStore},
};

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
    active_worktree: Option<SessionId>,
}

/// Runtime seam shared by the session and sleep services without persisting PTYs.
pub(crate) struct SessionRuntime {
    registry: Mutex<Registry>,
    frames: broadcast::Sender<FrameUpdate>,
    process: Mutex<Option<Arc<dyn Process>>>,
    events: Mutex<Option<BroadcastBus>>,
}

impl SessionRuntime {
    fn new(frames: broadcast::Sender<FrameUpdate>) -> Self {
        Self {
            registry: Mutex::new(Registry {
                next_terminal: 1,
                ..Registry::default()
            }),
            frames,
            process: Mutex::new(None),
            events: Mutex::new(None),
        }
    }

    fn register_events(&self, events: BroadcastBus) {
        *self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(events);
    }

    fn notify_session(&self, id: &SessionId) {
        let session = self.session(id);
        let events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(events) = events {
            if let Some(session) = session {
                events.publish(Event::SessionChanged(session));
            }
            events.request_snapshot_current();
        }
    }

    fn notify_snapshot(&self) {
        if let Some(events) = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            events.request_snapshot_current();
        }
    }

    fn notify_terminal(&self, terminal: TerminalId) {
        let session = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .terminal_sessions
            .get(&terminal)
            .cloned();
        if let Some(session) = session {
            self.notify_session(&session);
        } else {
            self.notify_snapshot();
        }
    }

    fn publish(&self, event: Event) {
        if let Some(events) = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            events.publish(event);
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

    pub(crate) fn update_observation(
        &self,
        terminal: TerminalId,
        foreground_command: Option<String>,
        keep_alive: Vec<String>,
    ) {
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session_id) = registry.terminal_sessions.get(&terminal).cloned() else {
            return;
        };
        let Some(session) = registry.sessions.get_mut(&session_id) else {
            return;
        };
        if let Some(entry) = session
            .terminals
            .iter_mut()
            .find(|entry| entry.id == terminal)
        {
            entry.foreground_command = foreground_command;
            entry.keep_alive = keep_alive;
        }
        drop(registry);
        self.notify_session(&session_id);
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

    pub(crate) fn close_terminal_if_present(&self, terminal: TerminalId) -> Option<String> {
        let (name, host) = {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let session_id = registry.terminal_sessions.remove(&terminal)?;
            registry.attachments.remove(&terminal);
            let host = registry.hosts.remove(&terminal);
            let session = registry.sessions.get_mut(&session_id)?;
            let position = session
                .terminals
                .iter()
                .position(|entry| entry.id == terminal)?;
            let name = session.terminals.remove(position).name;
            if session.active_terminal == Some(terminal) {
                session.active_terminal = session.terminals.first().map(|entry| entry.id);
            }
            if session.terminals.is_empty() {
                registry.sessions.remove(&session_id);
                if registry.active_worktree.as_ref() == Some(&session_id) {
                    registry.active_worktree = None;
                }
            }
            (name, host)
        };
        if let Some(host) = host {
            let _ = host.kill();
        }
        self.notify_snapshot();
        Some(name)
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
                .filter_map(|terminal| {
                    registry.terminal_sessions.remove(&terminal.id);
                    registry.attachments.remove(&terminal.id);
                    registry.hosts.remove(&terminal.id)
                })
                .collect::<Vec<_>>()
        };
        for host in hosts {
            let _ = host.kill();
        }
        self.notify_snapshot();
        true
    }
}

fn runtimes() -> &'static Mutex<HashMap<usize, Weak<SessionRuntime>>> {
    static RUNTIMES: OnceLock<Mutex<HashMap<usize, Weak<SessionRuntime>>>> = OnceLock::new();
    RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn shared_runtime(state: &Arc<StateStore>) -> Arc<SessionRuntime> {
    let key = Arc::as_ptr(state) as usize;
    let mut entries = runtimes()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(runtime) = entries.get(&key).and_then(Weak::upgrade) {
        return runtime;
    }
    let (frames, _receiver) = broadcast::channel(256);
    let runtime = Arc::new(SessionRuntime::new(frames));
    entries.insert(key, Arc::downgrade(&runtime));
    runtime
}

/// Daemon-owned PTY session service and terminal-frame source.
#[derive(Clone)]
pub struct Sessions {
    config: Arc<ConfigStore>,
    state: Arc<StateStore>,
    runtime: Arc<SessionRuntime>,
}

impl Sessions {
    /// Creates an empty runtime session registry.
    #[must_use]
    pub fn new(config: Arc<ConfigStore>, state: Arc<StateStore>) -> Self {
        let runtime = shared_runtime(&state);
        Self {
            config,
            state,
            runtime,
        }
    }

    /// Connects session mutations and PTY metadata changes to the daemon event bus.
    #[must_use]
    pub fn with_events(self, events: BroadcastBus) -> Self {
        self.runtime.register_events(events);
        self
    }

    /// Ensures a worktree or agent session and repairs configured terminals missing by name.
    pub async fn ensure(
        &self,
        worktree: Option<WorktreeId>,
        agent: Option<Agent>,
        sleep_previous: bool,
    ) -> DaemonResult<Session> {
        let config = self.config.load().await?;
        let (session_id, kind, cwd, specs) = match (worktree, agent) {
            (Some(worktree_id), None) => {
                let state = self.state.load().await?;
                let worktree = state
                    .worktrees
                    .iter()
                    .find(|entry| entry.id == worktree_id)
                    .ok_or_else(|| DaemonError::NotFound(worktree_id.to_string()))?;
                if worktree.host.is_some() {
                    return Err(DaemonError::Unsupported(
                        "remote hosts are not supported yet".to_owned(),
                    ));
                }
                let session_id = SessionId::try_from(worktree.session.as_str())
                    .map_err(|error| DaemonError::Validation(error.to_string()))?;
                (
                    session_id,
                    SessionKind::Worktree(worktree_id),
                    worktree.path.clone(),
                    default_terminals(&config, config.agent),
                )
            }
            (None, Some(agent)) => {
                let session_id = agent_session_id(agent)
                    .map_err(|error| DaemonError::Validation(error.to_string()))?;
                std::fs::create_dir_all(&config.repos_dir)
                    .map_err(|error| DaemonError::fs(&config.repos_dir, error))?;
                let name = match agent {
                    Agent::Claude => "claude",
                    Agent::Opencode => "opencode",
                };
                (
                    session_id,
                    SessionKind::Agent(agent),
                    config.repos_dir.clone(),
                    vec![fleet_core::sessions::TerminalSpec {
                        name: name.to_owned(),
                        command: config.agent_commands.command(agent).to_owned(),
                    }],
                )
            }
            _ => {
                return Err(DaemonError::Validation(
                    "exactly one of worktree or agent is required".to_owned(),
                ));
            }
        };
        if specs.is_empty() {
            return Err(DaemonError::Validation(
                "a session requires at least one configured terminal".to_owned(),
            ));
        }
        let configured_order = specs
            .iter()
            .enumerate()
            .map(|(index, spec)| (spec.name.clone(), index))
            .collect::<HashMap<_, _>>();

        let created = {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(existing) = registry.sessions.get(&session_id) {
                if existing.kind != kind {
                    return Err(DaemonError::Conflict(format!(
                        "session `{session_id}` belongs to a different workload"
                    )));
                }
                false
            } else {
                registry.sessions.insert(
                    session_id.clone(),
                    Session {
                        id: session_id.clone(),
                        kind: kind.clone(),
                        cwd: cwd.clone(),
                        terminals: Vec::new(),
                        active_terminal: None,
                        slept_at: None,
                        kept_terminals: Vec::new(),
                    },
                );
                true
            }
        };

        for spec in specs {
            let missing = self.runtime.session(&session_id).is_some_and(|session| {
                !session
                    .terminals
                    .iter()
                    .any(|entry| entry.name == spec.name)
            });
            if missing
                && let Err(error) = self
                    .new_terminal(session_id.clone(), spec.name, spec.command, cwd.clone())
                    .await
            {
                if created {
                    self.runtime.kill_if_present(&session_id);
                }
                return Err(error);
            }
        }
        {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(session) = registry.sessions.get_mut(&session_id) {
                session.terminals.sort_by_key(|terminal| {
                    configured_order
                        .get(&terminal.name)
                        .copied()
                        .unwrap_or(usize::MAX)
                });
            }
        }

        let previous = if matches!(kind, SessionKind::Worktree(_)) {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = registry.active_worktree.replace(session_id.clone());
            previous.filter(|previous| previous != &session_id)
        } else {
            None
        };
        if sleep_previous
            && let Some(previous) = previous
            && let Some(process) = self.runtime.process()
            && let Err(error) = crate::services::sleep::apply_session(
                &self.runtime,
                &self.config,
                process.as_ref(),
                &previous,
            )
            .await
        {
            tracing::warn!(%error, %previous, "failed to sleep previous session");
        }

        let session = self
            .runtime
            .session(&session_id)
            .ok_or_else(|| DaemonError::NotFound(session_id.to_string()))?;
        self.runtime.notify_session(&session_id);
        Ok(session)
    }

    /// Lists daemon-owned runtime sessions in stable identity order.
    pub async fn list(&self) -> DaemonResult<Vec<Session>> {
        Ok(self.snapshot())
    }

    /// Hard-kills a runtime session and all of its terminals.
    pub async fn kill(&self, session: SessionId) -> DaemonResult<()> {
        if self.runtime.kill_if_present(&session) {
            Ok(())
        } else if self.is_remote_session(&session).await? {
            Err(DaemonError::Unsupported(
                "remote hosts are not supported yet".to_owned(),
            ))
        } else {
            Err(DaemonError::NotFound(session.to_string()))
        }
    }

    /// Adds a login-shell terminal and types its configured command.
    pub async fn new_terminal(
        &self,
        session: SessionId,
        name: String,
        command: String,
        cwd: String,
    ) -> DaemonResult<Terminal> {
        validate_terminal_input(&name, &command)?;
        let terminal_id = {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let parent = registry
                .sessions
                .get(&session)
                .ok_or_else(|| DaemonError::NotFound(session.to_string()))?;
            if parent.terminals.iter().any(|entry| entry.name == name) {
                return Err(DaemonError::Conflict(format!(
                    "terminal name `{name}` already exists in session `{session}`"
                )));
            }
            let id = TerminalId(registry.next_terminal);
            registry.next_terminal = registry.next_terminal.saturating_add(1);
            id
        };

        let (terminal, host) = spawn_terminal(
            terminal_id,
            &session,
            name,
            command,
            cwd,
            self.config.load().await?.terminal.scrollback_bytes,
            1,
        )?;
        forward_host_events(Arc::clone(&self.runtime), terminal_id, &host)?;
        let host = Arc::new(host);
        {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let parent = registry
                .sessions
                .get_mut(&session)
                .ok_or_else(|| DaemonError::NotFound(session.to_string()))?;
            parent.terminals.push(terminal.clone());
            parent.active_terminal.get_or_insert(terminal_id);
            registry
                .terminal_sessions
                .insert(terminal_id, session.clone());
            registry.hosts.insert(terminal_id, host);
        }
        self.runtime.notify_session(&session);
        Ok(terminal)
    }

    /// Closes one terminal without affecting siblings; closing the last removes the session.
    pub async fn close_terminal(&self, terminal: TerminalId) -> DaemonResult<()> {
        self.runtime
            .close_terminal_if_present(terminal)
            .map(|_| ())
            .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))
    }

    /// Recreates an exited terminal from its retained command and working directory.
    pub async fn restart_terminal(&self, terminal: TerminalId) -> DaemonResult<Terminal> {
        let (session_id, old, starting_sequence) = {
            let registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let session_id = registry
                .terminal_sessions
                .get(&terminal)
                .cloned()
                .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
            let entry = registry
                .sessions
                .get(&session_id)
                .and_then(|session| session.terminals.iter().find(|entry| entry.id == terminal))
                .cloned()
                .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
            if !matches!(entry.status, TerminalStatus::Exited { .. }) {
                return Err(DaemonError::Conflict(format!(
                    "terminal `{terminal}` is still running"
                )));
            }
            (
                session_id,
                entry,
                registry.next_sequences.get(&terminal).copied().unwrap_or(1),
            )
        };
        let (replacement, host) = spawn_terminal(
            terminal,
            &session_id,
            old.name,
            old.command,
            old.cwd,
            self.config.load().await?.terminal.scrollback_bytes,
            starting_sequence,
        )?;
        forward_host_events(Arc::clone(&self.runtime), terminal, &host)?;
        {
            let mut registry = self
                .runtime
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let session = registry
                .sessions
                .get_mut(&session_id)
                .ok_or_else(|| DaemonError::NotFound(session_id.to_string()))?;
            let entry = session
                .terminals
                .iter_mut()
                .find(|entry| entry.id == terminal)
                .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
            *entry = replacement.clone();
            registry.hosts.insert(terminal, Arc::new(host));
        }
        self.runtime.notify_session(&session_id);
        Ok(replacement)
    }

    /// Renames one terminal while preserving its process.
    pub async fn rename_terminal(
        &self,
        terminal: TerminalId,
        name: String,
    ) -> DaemonResult<Terminal> {
        if name.trim().is_empty() {
            return Err(DaemonError::Validation(
                "terminal name must not be empty".to_owned(),
            ));
        }
        let mut registry = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session_id = registry
            .terminal_sessions
            .get(&terminal)
            .cloned()
            .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
        let session = registry
            .sessions
            .get_mut(&session_id)
            .ok_or_else(|| DaemonError::NotFound(session_id.to_string()))?;
        if session
            .terminals
            .iter()
            .any(|entry| entry.id != terminal && entry.name == name)
        {
            return Err(DaemonError::Conflict(format!(
                "terminal name `{name}` already exists in session `{session_id}`"
            )));
        }
        let entry = session
            .terminals
            .iter_mut()
            .find(|entry| entry.id == terminal)
            .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
        entry.name = name;
        let terminal = entry.clone();
        drop(registry);
        self.runtime.notify_session(&session_id);
        Ok(terminal)
    }

    /// Selects the active terminal and clears its unseen-output flag.
    pub async fn select_terminal(
        &self,
        session: SessionId,
        terminal: TerminalId,
    ) -> DaemonResult<Session> {
        let mut registry = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let parent = registry
            .sessions
            .get_mut(&session)
            .ok_or_else(|| DaemonError::NotFound(session.to_string()))?;
        let entry = parent
            .terminals
            .iter_mut()
            .find(|entry| entry.id == terminal)
            .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))?;
        entry.has_unseen_output = false;
        parent.active_terminal = Some(terminal);
        let parent = parent.clone();
        drop(registry);
        self.runtime.notify_session(&session);
        Ok(parent)
    }

    /// Attaches a client, makes its size authoritative, and emits a full frame.
    pub async fn attach(&self, terminal: TerminalId, cols: u16, rows: u16) -> DaemonResult<()> {
        let host = self.host_or_not_found(terminal)?;
        let frame = host
            .attach(cols, rows)
            .map_err(|error| terminal_error(terminal, error))?;
        let mut registry = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next = registry.next_sequences.entry(terminal).or_insert(1);
        *next = (*next).max(frame.seq.saturating_add(1));
        let _ = self.runtime.frames.send(frame);
        *registry.attachments.entry(terminal).or_default() += 1;
        drop(registry);
        self.runtime.notify_terminal(terminal);
        Ok(())
    }

    /// Removes one client attachment without stopping the terminal.
    pub async fn detach(&self, terminal: TerminalId) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?;
        let mut registry = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = registry.attachments.get_mut(&terminal) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                registry.attachments.remove(&terminal);
            }
        }
        drop(registry);
        self.runtime.notify_terminal(terminal);
        Ok(())
    }

    /// Writes already encoded bytes to the terminal process.
    pub async fn input(&self, terminal: TerminalId, bytes: Vec<u8>) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .write(bytes)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Encodes a semantic key using current terminal modes.
    pub async fn key(&self, terminal: TerminalId, key: KeyEvent) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .key(key)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Encodes mouse reporting only when the terminal requests it.
    pub async fn mouse(&self, terminal: TerminalId, mouse: MouseEvent) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .mouse(mouse)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Resizes a PTY, with the most recent client dimensions winning.
    pub async fn resize(&self, terminal: TerminalId, cols: u16, rows: u16) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .resize(cols, rows)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Moves the server-side scrollback viewport without affecting the process.
    pub async fn scroll(&self, terminal: TerminalId, scroll: ScrollCommand) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .scroll(scroll)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Routes wheel input on the owning host thread.
    pub async fn wheel(&self, terminal: TerminalId, wheel: WheelEvent) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .wheel(wheel)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Routes a viewport shortcut on the owning host thread.
    pub async fn scroll_or_key(
        &self,
        terminal: TerminalId,
        scroll: ScrollCommand,
        key: KeyEvent,
    ) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .scroll_or_key(scroll, key)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Emits a complete replacement frame after attach or sequence loss.
    pub async fn request_full_frame(&self, terminal: TerminalId) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .request_full()
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Pastes text with bracketed-paste encoding when enabled.
    pub async fn paste(&self, terminal: TerminalId, text: String) -> DaemonResult<()> {
        self.host_or_not_found(terminal)?
            .paste(text)
            .map_err(|error| terminal_error(terminal, error))
    }

    /// Computes local worktree status from the runtime registry.
    pub async fn refresh_statuses(
        &self,
        repo: Option<RepoId>,
    ) -> DaemonResult<Vec<WorktreeStatus>> {
        let state = self.state.load().await?;
        let registry = self
            .runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(state
            .worktrees
            .iter()
            .filter(|worktree| repo.as_ref().is_none_or(|repo| &worktree.repo_id == repo))
            .map(|worktree| {
                if worktree.host.is_some() {
                    return unknown_status(worktree.id.clone());
                }
                let Ok(session_id) = SessionId::try_from(worktree.session.as_str()) else {
                    return unknown_status(worktree.id.clone());
                };
                let Some(session) = registry.sessions.get(&session_id) else {
                    return WorktreeStatus {
                        worktree_id: worktree.id.clone(),
                        session: SessionState::None,
                        windows: Vec::new(),
                        running: Vec::new(),
                    };
                };
                let attached = session.terminals.iter().any(|terminal| {
                    registry.attachments.get(&terminal.id).copied().unwrap_or(0) > 0
                });
                let windows = session
                    .terminals
                    .iter()
                    .enumerate()
                    .map(|(index, terminal)| WorktreeWindowStatus {
                        index: u32::try_from(index).unwrap_or(u32::MAX),
                        name: terminal.name.clone(),
                        command: terminal
                            .foreground_command
                            .clone()
                            .unwrap_or_else(|| terminal.command.clone()),
                        keep_alive: terminal.keep_alive.clone(),
                    })
                    .collect::<Vec<_>>();
                let mut running = Vec::new();
                for label in session
                    .terminals
                    .iter()
                    .flat_map(|terminal| terminal.keep_alive.iter())
                {
                    if !running.contains(label) {
                        running.push(label.clone());
                    }
                }
                WorktreeStatus {
                    worktree_id: worktree.id.clone(),
                    session: if attached {
                        SessionState::Attached
                    } else {
                        SessionState::Detached
                    },
                    windows,
                    running,
                }
            })
            .collect())
    }

    /// Subscribes a connection actor to all terminal frames for attachment filtering.
    pub fn subscribe_frames(&self) -> broadcast::Receiver<FrameUpdate> {
        self.runtime.frames.subscribe()
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

    pub(crate) fn snapshot(&self) -> Vec<Session> {
        self.runtime
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .values()
            .cloned()
            .collect()
    }

    fn host_or_not_found(&self, terminal: TerminalId) -> DaemonResult<Arc<TerminalHost>> {
        self.runtime
            .host(terminal)
            .ok_or_else(|| DaemonError::NotFound(terminal.to_string()))
    }

    async fn is_remote_session(&self, session: &SessionId) -> DaemonResult<bool> {
        let config = self.config.load().await?;
        Ok(config
            .hosts
            .keys()
            .any(|host| session.as_str().starts_with(&format!("{host}/"))))
    }
}

fn unknown_status(worktree_id: WorktreeId) -> WorktreeStatus {
    WorktreeStatus {
        worktree_id,
        session: SessionState::Unknown,
        windows: Vec::new(),
        running: Vec::new(),
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

fn spawn_terminal(
    terminal: TerminalId,
    session: &SessionId,
    name: String,
    command: String,
    cwd: String,
    scrollback_bytes: usize,
    starting_sequence: u64,
) -> DaemonResult<(Terminal, TerminalHost)> {
    let pty = PtyOptions::login_shell(
        PathBuf::from(&cwd),
        session.as_str(),
        &name,
        true,
        INITIAL_COLS,
        INITIAL_ROWS,
    );
    let host = TerminalHost::spawn(TerminalHostOptions {
        terminal,
        pty,
        scrollback_bytes,
        starting_sequence,
        initial_command: Some(command.clone()),
    })
    .map_err(|error| terminal_error(terminal, error))?;
    let shell_pid = host.child_pid();
    let Some(shell_pid) = shell_pid else {
        let _ = host.kill();
        return Err(DaemonError::Process(format!(
            "terminal `{terminal}` did not report its login-shell pid"
        )));
    };
    Ok((
        Terminal {
            id: terminal,
            name,
            command,
            cwd,
            shell_pid: Some(shell_pid),
            foreground_command: None,
            status: TerminalStatus::Running,
            title: None,
            keep_alive: Vec::new(),
            has_unseen_output: false,
        },
        host,
    ))
}

fn forward_host_events(
    runtime: Arc<SessionRuntime>,
    terminal: TerminalId,
    host: &TerminalHost,
) -> DaemonResult<()> {
    let receiver = host.event_receiver();
    thread::Builder::new()
        .name(format!("fleet-terminal-events-{terminal}"))
        .spawn(move || {
            while let Ok(event) = receiver.recv_blocking() {
                match event {
                    HostEvent::Frame(frame) => {
                        let mut registry = runtime
                            .registry
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let next = registry.next_sequences.entry(terminal).or_insert(1);
                        *next = (*next).max(frame.seq.saturating_add(1));
                        if let Some(session_id) = registry.terminal_sessions.get(&terminal).cloned()
                            && let Some(session) = registry.sessions.get_mut(&session_id)
                            && session.active_terminal != Some(terminal)
                            && let Some(entry) = session
                                .terminals
                                .iter_mut()
                                .find(|entry| entry.id == terminal)
                        {
                            entry.has_unseen_output = true;
                        }
                        drop(registry);
                        let _ = runtime.frames.send(frame);
                    }
                    HostEvent::Exited(code) => {
                        update_terminal(&runtime, terminal, |entry| {
                            entry.status = TerminalStatus::Exited { code };
                            entry.foreground_command = None;
                        });
                        runtime.publish(Event::TerminalExited { terminal, code });
                    }
                    HostEvent::Title(title) => {
                        update_terminal(&runtime, terminal, |entry| {
                            entry.title = Some(title.clone());
                        });
                        runtime.publish(Event::TerminalTitle { terminal, title });
                    }
                    HostEvent::Bell | HostEvent::Cwd(_) | HostEvent::ClipboardWrite { .. } => {}
                }
            }
        })
        .map(|_| ())
        .map_err(|error| DaemonError::Process(format!("terminal event thread: {error}")))
}

fn update_terminal(
    runtime: &SessionRuntime,
    terminal: TerminalId,
    update: impl FnOnce(&mut Terminal),
) {
    let mut registry = runtime
        .registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(session_id) = registry.terminal_sessions.get(&terminal).cloned() else {
        return;
    };
    let Some(entry) = registry.sessions.get_mut(&session_id).and_then(|session| {
        session
            .terminals
            .iter_mut()
            .find(|entry| entry.id == terminal)
    }) else {
        return;
    };
    update(entry);
    drop(registry);
    runtime.notify_session(&session_id);
}

fn terminal_error(terminal: TerminalId, error: impl std::fmt::Display) -> DaemonError {
    DaemonError::Process(format!("terminal `{terminal}`: {error}"))
}
