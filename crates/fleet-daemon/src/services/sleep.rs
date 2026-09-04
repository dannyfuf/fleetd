//! Session sleep policy evaluation and shutdown handshake contracts.

use std::sync::Arc;

use fleet_core::ids::{SessionId, WorktreeId};
use fleet_proto::response::KeepAliveRuleMatch;
use fleet_proto::response::SleepResult;

use crate::{
    DaemonError, DaemonResult,
    adapters::process::Process,
    stores::{config::ConfigStore, state::StateStore},
};

/// Session sleep-policy service.
#[derive(Clone)]
pub struct Sleep {
    _config: Arc<ConfigStore>,
    _state: Arc<StateStore>,
    _process: Arc<dyn Process>,
}

impl Sleep {
    /// Creates the sleep service.
    #[must_use]
    pub fn new(
        config: Arc<ConfigStore>,
        state: Arc<StateStore>,
        process: Arc<dyn Process>,
    ) -> Self {
        Self {
            _config: config,
            _state: state,
            _process: process,
        }
    }

    /// Applies keep-alive rules and editor `:qa` grace handling to a session (inventory section 4).
    pub async fn session(&self, _session: SessionId) -> DaemonResult<SleepResult> {
        Err(DaemonError::Unimplemented("sleep::session"))
    }

    /// Resolves a worktree session and applies the same sleep policy (inventory sections 1 and 4).
    pub async fn worktree(&self, _worktree: WorktreeId) -> DaemonResult<SleepResult> {
        Err(DaemonError::Unimplemented("sleep::worktree"))
    }

    /// Counts current process matches for configured keep-alive rules.
    pub async fn match_keep_alive_rules(&self) -> DaemonResult<Vec<KeepAliveRuleMatch>> {
        Err(DaemonError::Unimplemented("sleep::match_keep_alive_rules"))
    }
}
