//! Orthogonal session, turn, attention, mode, and capability state.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use super::{TurnId, TurnOutcome};

/// A supported native-agent provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    /// Anthropic Claude Code.
    Claude,
    /// OpenCode.
    OpenCode,
}

impl AgentKind {
    /// The default executable name for this provider.
    #[must_use]
    pub const fn executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::OpenCode => "opencode",
        }
    }

    /// The provider name used in native UI copy.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::OpenCode => "OpenCode",
        }
    }
}

impl From<crate::config::Agent> for AgentKind {
    fn from(value: crate::config::Agent) -> Self {
        match value {
            crate::config::Agent::Claude => Self::Claude,
            crate::config::Agent::Opencode => Self::OpenCode,
        }
    }
}

impl From<AgentKind> for crate::config::Agent {
    fn from(value: AgentKind) -> Self {
        match value {
            AgentKind::Claude => Self::Claude,
            AgentKind::OpenCode => Self::Opencode,
        }
    }
}

/// Whole-provider-process lifecycle, independent of turn and gate state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// A provider process is starting or resuming.
    Starting,
    /// The provider is ready for input.
    #[default]
    Ready,
    /// The provider reports active work.
    Running,
    /// The provider was stopped intentionally.
    Stopped,
    /// The provider session failed.
    Error,
}

/// Lifecycle of the current or most recently settled turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum TurnState {
    /// No turn has been submitted.
    #[default]
    None,
    /// The provider accepted and is executing this turn.
    Running(TurnId),
    /// The provider authoritatively completed this turn.
    Completed(TurnId, TurnOutcome),
    /// This turn was interrupted by a caller.
    Interrupted(TurnId),
    /// This turn failed without a successful terminal signal.
    Failed(TurnId),
}

/// Why a thread currently needs user attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// An open tool permission.
    Permission,
    /// One or more provider questions.
    Question,
    /// A settled plan awaiting approval.
    Plan,
    /// A newly completed turn.
    Finished,
}

/// Derived user-attention state, ordered by the native-agents attention table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Attention {
    /// A decision or finished turn requires the user.
    NeedsYou(AttentionKind),
    /// The session or turn failed.
    Failed,
    /// Provider, turn, or background work remains active.
    Working,
    /// Non-terminal output arrived since the client last viewed the thread.
    Unread,
    /// No attention signal is active.
    Idle,
}

impl Attention {
    /// Returns the table priority; larger ranks take precedence.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::NeedsYou(AttentionKind::Permission) => 8,
            Self::NeedsYou(AttentionKind::Question) => 7,
            Self::NeedsYou(AttentionKind::Plan) => 6,
            Self::NeedsYou(AttentionKind::Finished) => 5,
            Self::Failed => 4,
            Self::Working => 3,
            Self::Unread => 2,
            Self::Idle => 1,
        }
    }
}

impl Ord for Attention {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl PartialOrd for Attention {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Permission policy selected for provider tool calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Ask before protected operations.
    #[default]
    Ask,
    /// Apply file edits without asking, while retaining other gates.
    AcceptEdits,
    /// Produce and approve a plan before execution.
    Plan,
    /// Auto-allow supported provider operations.
    FullAccess,
}

/// Provider-neutral model and reasoning-effort selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    /// Provider-native model identifier.
    pub model: String,
    /// Optional provider-native reasoning effort or variant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Optional provider identifier, required by OpenCode model references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// Operations supported by one provider adapter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Capabilities {
    /// Resume an existing provider session.
    pub resume: bool,
    /// Fork an existing provider session.
    pub fork: bool,
    /// Steer an in-flight turn.
    pub steer: bool,
    /// Interrupt an in-flight turn.
    pub interrupt: bool,
    /// Change permission or plan modes.
    pub modes: bool,
    /// Change models without restarting the thread.
    pub models: bool,
}
