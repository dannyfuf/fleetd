//! Tokio-based Fleet daemon client for connection management, daemon spawning, request APIs, event streams, and terminal attachment.

pub mod api;
pub mod connection;
pub mod events;
pub mod spawn;
pub mod terminal;
pub mod watches;

pub use api::{CreateWorktreeResult, DaemonVersion, HelloResult, Result};
pub use connection::{Client, ConnectError};
pub use events::{EventReceiverExt, jobs, snapshots};
pub use spawn::{SpawnError, ensure_daemon, resolve_daemon_path, restart_daemon};
pub use terminal::{TerminalHandle, TerminalUpdate};
