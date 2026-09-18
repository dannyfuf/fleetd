//! Durable parent-to-child native-agent delegation contracts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{AgentKind, DelegationId, ItemId, Seq, ThreadId, TurnId};

/// The durable link between a caller thread and the child it spawned. The token is never on this
/// type: the daemon stores its SHA-256 and only `RequestBody::DelegationComplete` carries plaintext.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delegation {
    /// Delegation identity.
    pub id: DelegationId,
    /// Thread that requested the child.
    pub caller: ThreadId,
    /// Caller turn that requested the child.
    pub caller_turn: TurnId,
    /// Caller transcript item representing the child.
    pub caller_item: ItemId,
    /// Spawned child thread.
    pub child: ThreadId,
    /// Provider running the child.
    pub provider: AgentKind,
    /// Delegation nesting depth.
    pub depth: u8,
    /// Work assigned to the child.
    pub brief: String,
    /// Completion criteria for the child.
    pub expectation: String,
    /// Whether completion should be delivered as soon as possible.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub eager: bool,
    /// Current child lifecycle.
    pub status: DelegationStatus,
    /// Additional detail explaining the current status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_payload: Option<String>,
    /// Reported or recovered child result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<DelegationResult>,
    /// Number of completion nudges sent to the child.
    #[serde(default)]
    pub nudges: u8,
    /// Number of provider-exit recoveries attempted.
    #[serde(default)]
    pub recoveries: u8,
    /// Result-delivery lifecycle.
    pub delivery: DeliveryState,
    /// Creation timestamp.
    pub created: DateTime<Utc>,
    /// Terminal timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<DateTime<Utc>>,
    /// Latest concise child activity description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headline: Option<String>,
}

impl Delegation {
    /// Returns the time elapsed from creation to completion, or to `now` while live.
    #[must_use]
    pub fn elapsed(&self, now: DateTime<Utc>) -> chrono::Duration {
        self.finished.unwrap_or(now) - self.created
    }
}

/// Current state of a delegated child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationStatus {
    /// The child is being created.
    Starting,
    /// The child is working.
    Running,
    /// The child is waiting for input or otherwise blocked.
    Blocked,
    /// The child settled without a final report and is being nudged or drained.
    Settling,
    /// The child completed successfully.
    Succeeded,
    /// The child stopped without a complete result.
    Incomplete,
    /// The child failed.
    Failed,
    /// The child was cancelled.
    Cancelled,
}

impl DelegationStatus {
    /// Whether no further child work is expected.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Incomplete | Self::Failed | Self::Cancelled
        )
    }

    /// Whether child work may still progress.
    #[must_use]
    pub const fn is_live(self) -> bool {
        !self.is_terminal()
    }

    /// Compact status text used by transcript rows.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "working",
            Self::Blocked => "blocked",
            Self::Settling => "settling",
            Self::Succeeded => "done",
            Self::Incomplete => "incomplete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Result returned by a delegated child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationResult {
    /// Result text.
    pub text: String,
    /// Files the child changed, in first-seen order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_changed: Vec<String>,
    /// How the result was obtained.
    pub source: ResultSource,
    /// Whether the result was truncated to the protocol limit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub elided: bool,
}

/// Source of a delegated child's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSource {
    /// The child explicitly reported completion.
    Reported,
    /// Fleet recovered the last assistant message.
    LastAssistantText,
}

/// Delivery state of a delegated child's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DeliveryState {
    /// Delivery has not yet completed.
    Pending,
    /// The result was appended to the caller.
    Delivered {
        /// Sequence of the delivered caller event.
        seq: Seq,
        /// Caller turn that received the result.
        turn: TurnId,
    },
    /// Delivery cannot be completed automatically.
    Undeliverable {
        /// Human-readable reason.
        reason: String,
    },
}

impl DeliveryState {
    /// Whether delivery is still pending.
    #[must_use]
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    /// Compact delivery-state text.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered { .. } => "delivered",
            Self::Undeliverable { .. } => "undeliverable",
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::{Serialize, de::DeserializeOwned};

    use super::*;

    fn assert_round_trip<T>(value: T)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(&value).unwrap_or_else(|error| panic!("{error}"));
        let decoded: T = serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, value);
    }

    #[test]
    fn delegation_status_variants_round_trip() {
        for status in [
            DelegationStatus::Starting,
            DelegationStatus::Running,
            DelegationStatus::Blocked,
            DelegationStatus::Settling,
            DelegationStatus::Succeeded,
            DelegationStatus::Incomplete,
            DelegationStatus::Failed,
            DelegationStatus::Cancelled,
        ] {
            assert_round_trip(status);
        }
    }

    #[test]
    fn result_source_variants_round_trip() {
        assert_round_trip(ResultSource::Reported);
        assert_round_trip(ResultSource::LastAssistantText);
    }

    #[test]
    fn delivery_state_variants_round_trip() {
        assert_round_trip(DeliveryState::Pending);
        assert_round_trip(DeliveryState::Delivered {
            seq: Seq(7),
            turn: TurnId::new(),
        });
        assert_round_trip(DeliveryState::Undeliverable {
            reason: "caller stopped".to_owned(),
        });
    }
}
