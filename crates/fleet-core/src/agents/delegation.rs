//! Durable parent-to-child native-agent delegation contracts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{AgentKind, DelegationId, ItemId, Seq, ThreadId, TurnId, Usage};
use crate::ids::{BoardId, CardId};

/// Who asked for a delegated child.
///
/// Untagged on purpose: a thread caller is a bare id string, exactly the shape the field had
/// before a card could call, so every record written by an older build still decodes and every
/// record this build writes for a thread caller is byte for byte what it always was. A card
/// caller is the only new shape, and it is a map, so the two never collide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DelegationCaller {
    /// An agent thread delegated to the child.
    Thread(ThreadId),
    /// A board card's column automation started the child.
    Card {
        /// Board owning the card.
        board: BoardId,
        /// Card whose run this is.
        card: CardId,
    },
}

impl DelegationCaller {
    /// The calling thread, when a thread called.
    #[must_use]
    pub const fn thread(&self) -> Option<&ThreadId> {
        match self {
            Self::Thread(thread) => Some(thread),
            Self::Card { .. } => None,
        }
    }

    /// The calling board and card, when a card called.
    #[must_use]
    pub const fn card(&self) -> Option<(&BoardId, &CardId)> {
        match self {
            Self::Thread(_) => None,
            Self::Card { board, card } => Some((board, card)),
        }
    }

    /// Whether a card called.
    #[must_use]
    pub const fn is_card(&self) -> bool {
        matches!(self, Self::Card { .. })
    }
}

/// A thread caller renders as its thread id, exactly the text every log line carried before a
/// card could call; a card caller renders as `board/card`. Never use this for a column value: the
/// store writes the caller's parts to their own columns.
impl std::fmt::Display for DelegationCaller {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Thread(thread) => thread.fmt(formatter),
            Self::Card { board, card } => {
                write!(formatter, "{}/{}", board.as_str(), card.as_str())
            }
        }
    }
}

/// The durable link between a caller thread and the child it spawned. The token is never on this
/// type: the daemon stores its SHA-256 and only `RequestBody::DelegationComplete` carries plaintext.
///
/// Not `Eq`: [`usage`](Self::usage) carries provider floats, and a cost that compares equal by bit
/// pattern is not a guarantee this type can honestly make.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delegation {
    /// Delegation identity.
    pub id: DelegationId,
    /// Who requested the child: an agent thread, or a board card's column automation.
    pub caller: DelegationCaller,
    /// Caller turn that requested the child. `Some` iff the caller is a thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_turn: Option<TurnId>,
    /// Caller transcript item representing the child. `Some` iff the caller is a thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_item: Option<ItemId>,
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
    /// What the child's own thread has spent, **computed on read and never persisted**.
    ///
    /// The store decodes this as `None`, `EncodedDelegation` ignores it, and the record
    /// `publish_changed` broadcasts carries `None`: only the daemon's `get`, `list` and `wait`
    /// read paths fill it, from the child thread's own turns. Do not add a column for it — a
    /// persisted copy would be a second, staler answer to a question SQL already answers, and it
    /// would have to be rewritten on every token-usage event the child emits.
    ///
    /// The numbers are the **child thread's own**; a grandchild's spend is not summed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<DelegationUsage>,
}

impl Delegation {
    /// Returns the time elapsed from creation to completion, or to `now` while live.
    #[must_use]
    pub fn elapsed(&self, now: DateTime<Utc>) -> chrono::Duration {
        self.finished.unwrap_or(now) - self.created
    }
}

/// What a delegated child's own thread has spent so far.
///
/// The same three numbers `ThreadProjection` shows in the GUI, over the child thread alone:
/// [`Usage`] rather than eight restated counters, so the CLI and the GUI cannot drift on the
/// arithmetic.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationUsage {
    /// Cumulative provider token usage over the child's settled turns, plus its live turn.
    #[serde(default)]
    pub usage: Usage,
    /// Latest cumulative provider cost, when the provider reports one.
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// Latest context-window utilization percentage.
    #[serde(default)]
    pub context_pct: f32,
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
    /// The caller already read the result itself, so it is never appended to its transcript.
    ///
    /// Set when a caller waits on its own child and is handed the terminal record: injecting the
    /// same text again once the caller's turn settles is the duplicate this state exists to stop.
    /// Terminal like `Delivered`, and reached only from `Pending`.
    Consumed,
    /// Delivery cannot be completed automatically.
    Undeliverable {
        /// Human-readable reason.
        reason: String,
    },
    /// A card caller: the board write that recorded the outcome has committed.
    ///
    /// Terminal for delivery, like `Delivered` and `Consumed`, and never overwritten by the
    /// repair sweep: that sweep only looks at rows whose delivery is `pending` and whose caller
    /// is a thread, because there is no transcript to append a card's result to.
    Recorded,
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
            Self::Consumed => "consumed",
            Self::Undeliverable { .. } => "undeliverable",
            Self::Recorded => "recorded",
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

    /// Round trip for a value whose floats make `assert_eq!` a bit-pattern comparison: the JSON
    /// text is what has to match, because that is what crosses the wire.
    fn assert_round_trip_debug<T>(value: T)
    where
        T: Serialize + DeserializeOwned + std::fmt::Debug,
    {
        let json = serde_json::to_string(&value).unwrap_or_else(|error| panic!("{error}"));
        let decoded: T = serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        let again = serde_json::to_string(&decoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(again, json);
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
        assert_round_trip(DeliveryState::Consumed);
        assert_round_trip(DeliveryState::Undeliverable {
            reason: "caller stopped".to_owned(),
        });
        assert_round_trip(DeliveryState::Recorded);
    }

    #[test]
    fn a_recorded_delivery_encodes_as_a_tagged_object_and_is_not_pending() {
        let json = serde_json::to_string(&DeliveryState::Recorded)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(json, r#"{"type":"recorded"}"#);
        assert!(!DeliveryState::Recorded.is_pending());
    }

    #[test]
    fn delivery_state_words_are_distinct() {
        let words = [
            DeliveryState::Pending.word(),
            DeliveryState::Delivered {
                seq: Seq(1),
                turn: TurnId::new(),
            }
            .word(),
            DeliveryState::Consumed.word(),
            DeliveryState::Undeliverable {
                reason: String::new(),
            }
            .word(),
            DeliveryState::Recorded.word(),
        ];
        let unique: std::collections::BTreeSet<_> = words.iter().collect();
        assert_eq!(unique.len(), words.len(), "{words:?}");
    }

    #[test]
    fn a_consumed_delivery_is_not_pending() {
        assert!(!DeliveryState::Consumed.is_pending());
    }

    #[test]
    fn delegation_usage_round_trips() {
        assert_round_trip_debug(DelegationUsage::default());
        assert_round_trip_debug(DelegationUsage {
            usage: Usage {
                input_tokens: 11,
                output_tokens: 22,
                total_tokens: 33,
                ..Usage::default()
            },
            cost_usd: Some(0.125),
            context_pct: 12.5,
        });
    }

    fn delegation(caller: DelegationCaller) -> Delegation {
        let thread_called = caller.thread().is_some();
        Delegation {
            id: DelegationId::new(),
            caller,
            caller_turn: thread_called.then(TurnId::new),
            caller_item: thread_called.then(ItemId::new),
            child: ThreadId::new(),
            provider: AgentKind::Codex,
            depth: 1,
            brief: "brief".to_owned(),
            expectation: "expectation".to_owned(),
            eager: false,
            status: DelegationStatus::Starting,
            status_payload: None,
            result: None,
            nudges: 0,
            recoveries: 0,
            delivery: DeliveryState::Pending,
            created: DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default(),
            finished: None,
            headline: None,
            usage: None,
        }
    }

    fn card_caller() -> DelegationCaller {
        DelegationCaller::Card {
            board: BoardId::try_from("work".to_owned()).unwrap_or_else(|error| panic!("{error}")),
            card: CardId::try_from("card-12".to_owned()).unwrap_or_else(|error| panic!("{error}")),
        }
    }

    #[test]
    fn a_delegation_without_usage_serializes_without_the_key() {
        let json = serde_json::to_string(&delegation(DelegationCaller::Thread(ThreadId::new())))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(!json.contains("usage"), "{json}");
    }

    #[test]
    fn a_thread_called_delegation_encodes_its_caller_as_a_bare_id() {
        // The shape the field had before a card could call: a record an older build wrote still
        // decodes, and this build writes the same bytes back.
        let thread = ThreadId::new();
        let delegation = delegation(DelegationCaller::Thread(thread));
        let json = serde_json::to_string(&delegation).unwrap_or_else(|error| panic!("{error}"));
        assert!(json.contains(&format!(r#""caller":"{thread}""#)), "{json}");
        assert!(json.contains(r#""callerTurn":"#), "{json}");
        assert!(json.contains(r#""callerItem":"#), "{json}");
        assert_round_trip(delegation);
    }

    #[test]
    fn a_card_called_delegation_encodes_its_caller_as_a_board_and_card_and_omits_the_turn_keys() {
        let delegation = delegation(card_caller());
        let json = serde_json::to_string(&delegation).unwrap_or_else(|error| panic!("{error}"));
        assert!(
            json.contains(r#""caller":{"board":"work","card":"card-12"}"#),
            "{json}"
        );
        assert!(!json.contains("callerTurn"), "{json}");
        assert!(!json.contains("callerItem"), "{json}");
        assert_round_trip(delegation);
    }

    #[test]
    fn a_caller_answers_which_kind_it_is() {
        let thread = ThreadId::new();
        let thread_caller = DelegationCaller::Thread(thread);
        assert_eq!(thread_caller.thread(), Some(&thread));
        assert!(thread_caller.card().is_none());
        assert!(!thread_caller.is_card());

        let card_caller = card_caller();
        assert!(card_caller.thread().is_none());
        assert!(card_caller.card().is_some());
        assert!(card_caller.is_card());
    }
}
