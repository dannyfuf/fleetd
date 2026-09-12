//! Daemon-side terminal infrastructure for PTY ownership, virtual-terminal emulation, key encoding, the dedicated terminal host thread, and the detached holder process that keeps a terminal's child alive across a daemon restart.

pub mod engine;
#[cfg(feature = "ghostty")]
pub mod ghostty;
pub mod holder;
#[cfg(feature = "ghostty")]
pub mod host;
#[cfg(feature = "ghostty")]
mod keys;
pub mod pty;

pub use engine::{EngineError, EngineEvent, VtEngine};
#[cfg(feature = "ghostty")]
pub use ghostty::GhosttyEngine;
pub use holder::{
    AttachError, HOLDER_PROTOCOL_VERSION, HolderError, HolderOptions, HolderPty,
    REPLAY_BUFFER_BYTES,
};
#[cfg(feature = "ghostty")]
pub use host::{
    HolderTarget, HostCommand, HostError, HostEvent, PtySource, TerminalActivity, TerminalHost,
    TerminalHostOptions,
};
pub use pty::{Pty, PtyBackend, PtyError, PtyOptions, PtyWritePermit};
