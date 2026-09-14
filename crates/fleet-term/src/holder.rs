//! Detached PTY holder: a separate process that owns a terminal's child so the child outlives
//! the daemon that started it.
//!
//! The daemon never opens a terminal's PTY itself. It launches one holder process per terminal
//! (`fleetd pty-hold`), which owns the pseudo-terminal and the login shell, listens on a
//! per-terminal Unix socket, and relays bytes both ways. A daemon restart closes the socket and
//! nothing else: the next daemon reconnects, receives the holder's bounded replay buffer, feeds
//! it to a fresh [`crate::GhosttyEngine`], and the terminal re-renders with its shell, its agent
//! and its scrollback intact.
//!
//! - [`run`] is the holder process itself.
//! - [`HolderPty`] is the daemon-side [`crate::PtyBackend`] that speaks to one.
//! - [`protocol`] is the framed wire format between them.

mod client;
mod protocol;
mod replay;
mod server;

pub use client::{AttachError, HolderPty};
pub use protocol::{
    HOLDER_PROTOCOL_VERSION, HolderEvent, HolderRequest, MAX_FRAME_BYTES, ProtocolError,
};
pub use server::{HolderError, HolderOptions, REPLAY_BUFFER_BYTES, run};
