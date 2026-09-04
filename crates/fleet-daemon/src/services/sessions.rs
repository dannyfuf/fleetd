//! Persistent daemon-owned session and terminal lifecycle orchestration contracts.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use fleet_core::{
    config::Agent,
    ids::{SessionId, TerminalId, WorktreeId},
    sessions::{Session, Terminal},
};
use fleet_proto::terminal::{FrameUpdate, KeyEvent, MouseEvent, ScrollCommand};
use fleet_term::TerminalHost;
use tokio::sync::broadcast;

use crate::{
    DaemonError, DaemonResult,
    stores::{config::ConfigStore, state::StateStore},
};

/// Daemon-owned PTY session service and terminal-frame source.
#[derive(Clone)]
pub struct Sessions {
    _config: Arc<ConfigStore>,
    _state: Arc<StateStore>,
    _hosts: Arc<Mutex<HashMap<TerminalId, TerminalHost>>>,
    frames: broadcast::Sender<FrameUpdate>,
}

impl Sessions {
    /// Creates an empty runtime session registry.
    #[must_use]
    pub fn new(config: Arc<ConfigStore>, state: Arc<StateStore>) -> Self {
        let (frames, _receiver) = broadcast::channel(256);
        Self {
            _config: config,
            _state: state,
            _hosts: Arc::new(Mutex::new(HashMap::new())),
            frames,
        }
    }

    /// Ensures a worktree or agent session with the fixed configured layout (inventory section 4).
    pub async fn ensure(
        &self,
        _worktree: Option<WorktreeId>,
        _agent: Option<Agent>,
        _sleep_previous: bool,
    ) -> DaemonResult<Session> {
        Err(DaemonError::Unimplemented("sessions::ensure"))
    }

    /// Lists daemon-owned runtime sessions using swarm-compatible identities (inventory sections 1 and 4).
    pub async fn list(&self) -> DaemonResult<Vec<Session>> {
        Err(DaemonError::Unimplemented("sessions::list"))
    }

    /// Hard-kills a runtime session and all of its terminals (inventory section 4).
    pub async fn kill(&self, _session: SessionId) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("sessions::kill"))
    }

    /// Adds a login-shell terminal and types its configured command (inventory section 4).
    pub async fn new_terminal(
        &self,
        _session: SessionId,
        _name: String,
        _command: String,
        _cwd: String,
    ) -> DaemonResult<Terminal> {
        Err(DaemonError::Unimplemented("sessions::new_terminal"))
    }

    /// Closes one terminal without affecting siblings (inventory section 4).
    pub async fn close_terminal(&self, _terminal: TerminalId) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("sessions::close_terminal"))
    }

    /// Recreates an exited terminal from its retained command and working directory.
    pub async fn restart_terminal(&self, _terminal: TerminalId) -> DaemonResult<Terminal> {
        Err(DaemonError::Unimplemented("sessions::restart_terminal"))
    }

    /// Renames one terminal while preserving its process (inventory section 4).
    pub async fn rename_terminal(
        &self,
        _terminal: TerminalId,
        _name: String,
    ) -> DaemonResult<Terminal> {
        Err(DaemonError::Unimplemented("sessions::rename_terminal"))
    }

    /// Selects the active terminal using swarm window ordering (inventory section 4).
    pub async fn select_terminal(
        &self,
        _session: SessionId,
        _terminal: TerminalId,
    ) -> DaemonResult<Session> {
        Err(DaemonError::Unimplemented("sessions::select_terminal"))
    }

    /// Attaches a client and resizes the PTY while preserving detached lifetime (inventory sections 4 and 6).
    pub async fn attach(&self, _terminal: TerminalId, _cols: u16, _rows: u16) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("sessions::attach"))
    }

    /// Removes a client attachment without stopping the terminal (inventory sections 4 and 6).
    pub async fn detach(&self, _terminal: TerminalId) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("sessions::detach"))
    }

    /// Writes already encoded bytes to the terminal process (inventory section 4).
    pub async fn input(&self, _terminal: TerminalId, _bytes: Vec<u8>) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("sessions::input"))
    }

    /// Encodes a semantic key using current terminal modes (inventory section 4).
    pub async fn key(&self, _terminal: TerminalId, _key: KeyEvent) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("sessions::key"))
    }

    /// Encodes mouse reporting only when the terminal requests it (inventory section 4).
    pub async fn mouse(&self, _terminal: TerminalId, _mouse: MouseEvent) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("sessions::mouse"))
    }

    /// Resizes a PTY, with the most recent client dimensions winning (inventory section 4).
    pub async fn resize(&self, _terminal: TerminalId, _cols: u16, _rows: u16) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("sessions::resize"))
    }

    /// Moves the server-side scrollback viewport without affecting the process (inventory section 4).
    pub async fn scroll(&self, _terminal: TerminalId, _scroll: ScrollCommand) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("sessions::scroll"))
    }

    /// Emits a complete replacement frame after attach or sequence loss (inventory section 4).
    pub async fn request_full_frame(&self, _terminal: TerminalId) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("sessions::request_full_frame"))
    }

    /// Pastes text with bracketed-paste encoding when enabled (inventory section 4).
    pub async fn paste(&self, _terminal: TerminalId, _text: String) -> DaemonResult<()> {
        Err(DaemonError::Unimplemented("sessions::paste"))
    }

    /// Subscribes a connection actor to all terminal frames for attachment filtering.
    pub fn subscribe_frames(&self) -> broadcast::Receiver<FrameUpdate> {
        self.frames.subscribe()
    }

    pub(crate) fn snapshot(&self) -> Vec<Session> {
        Vec::new()
    }
}
