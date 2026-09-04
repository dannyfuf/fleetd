//! Daemon-side terminal infrastructure for PTY ownership, virtual-terminal emulation, key encoding, and the dedicated terminal host thread.

pub mod engine;
#[cfg(feature = "ghostty")]
pub mod ghostty;
#[cfg(feature = "ghostty")]
pub mod host;
pub mod keys;
pub mod pty;

pub use engine::{EngineError, EngineEvent, VtEngine};
#[cfg(feature = "ghostty")]
pub use ghostty::GhosttyEngine;
#[cfg(feature = "ghostty")]
pub use host::{HostCommand, HostError, HostEvent, TerminalHost, TerminalHostOptions};
pub use pty::{Pty, PtyError, PtyOptions};
