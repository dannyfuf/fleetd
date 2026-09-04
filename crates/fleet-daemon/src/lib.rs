//! Long-lived Fleet daemon services that own persisted state, filesystem operations, background jobs, GitHub data, terminal sessions, sleep policy, and the Unix socket server.

pub mod adapters;
pub mod jobs;
pub mod server;
pub mod services;
pub mod stores;
pub mod testing;
