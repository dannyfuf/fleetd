//! Tokio-based Fleet daemon client for connection management, daemon spawning, request APIs, event streams, and terminal attachment.

mod api;
mod connection;
mod spawn;
mod terminal;
mod watches;

pub use api::{
    AgentMirror, AgentSnapshot, CreateWorktreeResult, DaemonVersion, HelloResult, MirrorOutcome,
    Result,
};
pub use connection::{Client, ConnectError, ProtocolTransport, protocol_transport};
pub use spawn::{SpawnError, ensure_daemon, resolve_daemon_path, restart_daemon};
pub use terminal::{TerminalHandle, TerminalUpdate};
