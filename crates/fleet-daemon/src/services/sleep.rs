//! Session sleep policy evaluation and shutdown handshake contracts.

use std::sync::Arc;

use fleet_core::ids::{SessionId, WorktreeId};
use fleet_proto::response::SleepResult;

use crate::{DaemonError, DaemonResult, stores::config::ConfigStore};

/// Session sleep-policy service.
#[derive(Clone)]
pub struct Sleep {
    _config: Arc<ConfigStore>,
}

impl Sleep {
    /// Creates the sleep service.
    #[must_use]
    pub fn new(config: Arc<ConfigStore>) -> Self {
        Self { _config: config }
    }

    /// Applies keep-alive rules and editor `:qa` grace handling to a session (inventory section 4).
    pub async fn session(&self, _session: SessionId) -> DaemonResult<SleepResult> {
        Err(DaemonError::Unimplemented("sleep::session"))
    }

    /// Resolves a worktree session and applies the same sleep policy (inventory sections 1 and 4).
    pub async fn worktree(&self, _worktree: WorktreeId) -> DaemonResult<SleepResult> {
        Err(DaemonError::Unimplemented("sleep::worktree"))
    }
}
