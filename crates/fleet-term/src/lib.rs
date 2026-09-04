//! Daemon-side terminal infrastructure for PTY ownership, virtual-terminal emulation, key encoding, and the dedicated terminal host thread.

pub mod engine;
#[cfg(feature = "ghostty")]
pub mod ghostty;
pub mod host;
pub mod keys;
pub mod pty;
