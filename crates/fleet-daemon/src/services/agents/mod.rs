//! Daemon-owned native-agent provider, persistence, and lifecycle seams.

pub(crate) mod delegation;
mod manager;
pub mod providers;
mod record;
mod store;
mod thread;

pub use manager::{AgentSessionManager, CreateOptions};
pub use record::{AGENT_INDEX_VERSION, AgentIndex, AgentThreadRecord};

/// Native-agent service registered in [`crate::services::Services`].
pub type AgentService = AgentSessionManager;
