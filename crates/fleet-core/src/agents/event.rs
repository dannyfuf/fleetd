//! Append-only normalized events emitted by native-agent harnesses.

use std::{collections::BTreeMap, path::PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    AgentKind, GateAnswer, GateId, GateKind, GateResolver, ItemId, ItemKind, ItemPatch,
    ModelSelection, PermissionMode, Seq, SessionState, TurnId,
};

/// Harness session metadata learned at initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// Active harness.
    pub provider: AgentKind,
    /// Harness-native resume identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_cursor: Option<String>,
    /// Active model, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    /// Active permission policy.
    pub mode: PermissionMode,
    /// Harness-native available tool names.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Harness-native slash command names.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Harness-native skill names.
    #[serde(default)]
    pub skills: Vec<String>,
}

/// Normalized token usage from either harness.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Usage {
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Thinking or reasoning tokens when reported separately.
    pub reasoning_tokens: u64,
    /// Prompt-cache read tokens.
    pub cache_read_tokens: u64,
    /// Prompt-cache creation or write tokens.
    pub cache_write_tokens: u64,
    /// Harness-reported total, or Fleet's normalized total.
    pub total_tokens: u64,
    /// Server-side web-search requests.
    pub web_search_requests: u64,
    /// Harness or subagent tool-use count.
    pub tool_uses: u64,
    /// Additive harness usage fields retained losslessly.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Per-file line change summary for a completed turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDelta {
    /// Changed worktree-relative path.
    pub path: PathBuf,
    /// Added line count.
    pub added: u64,
    /// Removed line count.
    pub removed: u64,
}

/// Harness-authoritative terminal classification for a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum TurnOutcome {
    /// The harness completed normally.
    Completed,
    /// The harness completed with an execution error.
    Error {
        /// Best available harness message.
        message: Option<String>,
    },
    /// The harness settled an explicit interrupt.
    Interrupted,
    /// A permission denial prevented completion.
    Denied,
    /// A configured turn limit was reached.
    MaxTurns,
    /// A configured cost or token budget was exhausted.
    BudgetExhausted,
    /// The harness stopped on another named terminal reason.
    Other {
        /// Exact harness terminal reason.
        reason: String,
    },
}

/// Why a turn was explicitly aborted before normal completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AbortReason {
    /// A user requested interruption.
    User,
    /// The native thread was stopped.
    SessionStopped,
    /// The harness process exited unexpectedly.
    ProviderExited,
    /// A bounded harness operation timed out.
    Timeout,
    /// A later prompt superseded this work.
    Superseded,
    /// An additive harness-native reason.
    Other(String),
}

/// Append-only content channel for [`AgentEvent::ContentDelta`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum StreamKind {
    /// Visible assistant prose.
    AssistantText,
    /// User-facing reasoning summary part.
    ReasoningSummary {
        /// Stable harness part index.
        part: u32,
    },
    /// Raw reasoning part, retained separately from summaries.
    ReasoningRaw {
        /// Stable harness part index.
        part: u32,
    },
    /// Streaming command or tool output.
    CommandOutput,
    /// Streaming plan Markdown.
    PlanText,
}

/// Observability boundary recorded in the transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum CheckpointKind {
    /// Context compaction changed the estimated token count.
    CompactBoundary {
        /// Tokens before compaction.
        before: u64,
        /// Tokens after compaction, when the harness reported them.
        after: Option<u64>,
    },
    /// A harness session was resumed after a pause.
    Resumed {
        /// Milliseconds since the prior live process was active.
        age_ms: u64,
    },
}

/// Lifecycle state of a projected item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// Streaming or executing.
    #[default]
    InProgress,
    /// Completed successfully.
    Completed,
    /// Completed with an error.
    Failed,
    /// Refused by a permission decision.
    Denied,
    /// Stopped before completion.
    Stopped,
}

/// One normalized harness event before sequence and time stamping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Harness initialization completed and exposed its session metadata.
    SessionConfigured {
        /// Active harness.
        provider: AgentKind,
        /// Harness-native resume cursor.
        #[serde(default)]
        resume_cursor: Option<String>,
        /// Active model, when known.
        #[serde(default)]
        model: Option<ModelSelection>,
        /// Active permission policy.
        #[serde(default)]
        mode: PermissionMode,
        /// Harness-native tool names.
        #[serde(default)]
        tools: Vec<String>,
        /// Harness-native slash commands.
        #[serde(default)]
        commands: Vec<String>,
        /// Harness-native skills.
        #[serde(default)]
        skills: Vec<String>,
    },
    /// Session metadata changed after initialization.
    MetadataChanged {
        /// Harness-reported session title, when it changed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Permission policy now in force, when it changed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<PermissionMode>,
        /// Model selection now in force, when it changed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<ModelSelection>,
    },
    /// Whole-session lifecycle changed.
    SessionStateChanged(SessionState),
    /// Fine-grained non-terminal harness activity.
    SessionActivity {
        /// Harness-neutral phase label.
        phase: String,
    },
    /// Harness process exited.
    SessionExited {
        /// Exit code when available.
        #[serde(default)]
        code: Option<i32>,
        /// Whether Fleet requested the exit.
        #[serde(default)]
        expected: bool,
    },
    /// The harness accepted a user turn.
    TurnStarted {
        /// Turn identity.
        turn: TurnId,
        /// Projected user-message item.
        user_item: ItemId,
    },
    /// Harness-authoritative turn settlement.
    TurnSettled {
        /// Settled turn.
        turn: TurnId,
        /// Terminal classification.
        outcome: TurnOutcome,
        /// Per-turn normalized usage.
        #[serde(default)]
        usage: Usage,
        /// Harness-reported duration.
        #[serde(default)]
        duration_ms: u64,
        /// Changed-file summary.
        #[serde(default)]
        files_changed: Vec<FileDelta>,
    },
    /// Explicit non-completion settlement.
    TurnAborted {
        /// Aborted turn.
        turn: TurnId,
        /// Abort cause.
        reason: AbortReason,
    },
    /// Updated turn-level unified diff.
    TurnDiff {
        /// Owning turn.
        turn: TurnId,
        /// Complete unified diff.
        unified: String,
        /// Structured changed-file summary, when available.
        #[serde(default)]
        files_changed: Vec<FileDelta>,
    },
    /// Updated harness plan/todo steps for an active turn.
    PlanSteps {
        /// Owning turn.
        turn: TurnId,
        /// Ordered human-readable step labels.
        steps: Vec<String>,
    },
    /// A transcript item became known.
    ItemStarted {
        /// Owning turn.
        turn: TurnId,
        /// Item identity.
        item: ItemId,
        /// Variant-owned item payload.
        kind: ItemKind,
        /// Optional parent item.
        #[serde(default)]
        parent: Option<ItemId>,
    },
    /// Append-only content arrived.
    ContentDelta {
        /// Target item.
        item: ItemId,
        /// Content channel.
        stream: StreamKind,
        /// Text to append.
        delta: String,
    },
    /// Cumulative or metadata fields replaced on an item.
    ItemUpdated {
        /// Target item.
        item: ItemId,
        /// Typed fields to replace.
        patch: ItemPatch,
    },
    /// An item settled.
    ItemCompleted {
        /// Target item.
        item: ItemId,
        /// Terminal item status.
        status: ItemStatus,
    },
    /// A harness decision became actionable.
    GateOpened {
        /// Gate identity.
        gate: GateId,
        /// Associated turn, when known.
        #[serde(default)]
        turn: Option<TurnId>,
        /// Gate presentation and harness mapping payload.
        kind: GateKind,
    },
    /// A gate was authoritatively answered.
    GateResolved {
        /// Gate identity.
        gate: GateId,
        /// Normalized answer.
        answer: GateAnswer,
        /// Resolver authority.
        by: GateResolver,
    },
    /// The harness withdrew a gate and must not be answered.
    GateWithdrawn {
        /// Gate identity.
        gate: GateId,
    },
    /// A durable plan became available for a turn.
    PlanProposed {
        /// Stable gate identity used for the resulting decision.
        gate: GateId,
        /// Owning turn.
        turn: TurnId,
        /// Full Markdown proposal.
        markdown: String,
        /// Ordered short step summaries.
        #[serde(default)]
        steps: Vec<String>,
    },
    /// Updated token, context, and cost observations.
    TokenUsage {
        /// Associated turn.
        turn: TurnId,
        /// Latest harness usage.
        #[serde(default)]
        usage: Usage,
        /// Context-window utilization percentage.
        #[serde(default)]
        context_pct: f32,
        /// Latest cumulative process or session cost.
        #[serde(default)]
        cost_usd: Option<f64>,
    },
    /// Updated provider rate-limit observations.
    RateLimits {
        /// Lossless normalized/provider fields without credentials or prompt data.
        limits: Value,
    },
    /// A compaction or resume boundary.
    Compacted(CheckpointKind),
    /// Harness retry backoff began.
    Retrying {
        /// One-based attempt number.
        attempt: u32,
        /// Delay before the next attempt.
        retry_in_ms: u64,
        /// Harness error text.
        reason: String,
    },
    /// The harness rerouted the requested model.
    ModelRerouted {
        /// Originally requested model.
        from: String,
        /// Model now serving the turn.
        to: String,
        /// Harness-provided reason.
        reason: String,
    },
    /// Runtime or protocol failure.
    RuntimeError {
        /// Whether the harness session can no longer continue.
        fatal: bool,
        /// Sanitized human-readable detail.
        message: String,
    },
    /// Non-fatal configuration, compatibility, or deprecation notice.
    Notice(String),
    /// A harness method this Fleet build does not understand.
    Unknown {
        /// Method or frame type only; raw data never crosses this boundary.
        method: String,
    },
}

/// A normalized event stamped by the serialized thread reducer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeqEvent {
    /// Monotonic per-thread cursor.
    pub seq: Seq,
    /// Reducer timestamp.
    pub at: DateTime<Utc>,
    /// Harness message type or event name retained for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// Harness-neutral payload.
    pub event: AgentEvent,
}
