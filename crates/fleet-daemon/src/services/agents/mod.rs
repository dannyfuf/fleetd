//! Daemon-owned native-agent provider, persistence, and lifecycle seams.

mod manager;
pub mod providers;
mod store;
mod thread;

pub use manager::AgentSessionManager;
pub use store::{AgentIndex, AgentStore, AgentThreadRecord};

/// Native-agent service registered in [`crate::services::Services`].
pub type AgentService = AgentSessionManager;
