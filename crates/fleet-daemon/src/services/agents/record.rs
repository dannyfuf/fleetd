//! Durable per-thread metadata: the record the store persists and the index that lists them.
//!
//! These two types are storage-agnostic on purpose. They outlived the NDJSON `index.json` the
//! store used to write them into, because the metadata itself is not a storage detail: a record
//! is what `AgentThreadCreate` mints, what restart recovery updates, and what a resume needs
//! before any log has been read. `fleet-daemon` owns them rather than `fleet-core` because
//! nothing above the daemon has any use for `resume_cursor`.

use chrono::{DateTime, Utc};
use fleet_core::{
    agents::{AgentKind, ModelSelection, PermissionMode, ThreadId, TurnOutcome},
    ids::WorktreeId,
};
use serde::{Deserialize, Serialize};

/// Persisted metadata index version.
///
/// It survives the move to SQLite for two reasons: the legacy `index.json` the one-shot import
/// reads is stamped with it, and [`AgentIndex`] is still the value the store's index seam
/// exchanges, so a shape this build cannot write has to stay refusable.
pub const AGENT_INDEX_VERSION: u32 = 1;

/// Versioned native-agent thread metadata index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIndex {
    /// Persisted schema version.
    pub version: u32,
    /// Threads in creation order.
    pub threads: Vec<AgentThreadRecord>,
}

impl Default for AgentIndex {
    fn default() -> Self {
        Self {
            version: AGENT_INDEX_VERSION,
            threads: Vec::new(),
        }
    }
}

/// Persisted metadata needed to list and resume one thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadRecord {
    /// Thread identity.
    pub thread: ThreadId,
    /// Owning worktree.
    pub worktree: WorktreeId,
    /// Provider kind.
    pub provider: AgentKind,
    /// Display title.
    pub title: String,
    /// Creation time.
    pub created: DateTime<Utc>,
    /// Most recent event time.
    pub last_activity: DateTime<Utc>,
    /// Provider-native resume cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_cursor: Option<String>,
    /// Last active model selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    /// Last active permission mode.
    pub mode: PermissionMode,
    /// Last settled turn outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<TurnOutcome>,
}
