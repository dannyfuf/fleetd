//! Long-lived Fleet daemon services that own persisted state, filesystem operations, background jobs, GitHub data, terminal sessions, sleep policy, and the Unix socket server.

pub mod adapters;
pub mod error;
pub mod jobs;
pub mod machines;
pub mod server;
pub mod services;
pub mod stores;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use error::{DaemonError, DaemonResult};
