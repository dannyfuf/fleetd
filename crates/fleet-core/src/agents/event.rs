//! Append-only normalized events emitted by native-agent providers.

use std::{collections::BTreeMap, path::PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    AgentKind, GateAnswer, GateId, GateKind, GateResolver, ItemId, ItemKind, ModelSelection,
    PermissionMode, Seq, SessionState, TurnId,
};

/// Provider session metadata learned at initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// Active provider.
    pub provider: AgentKind,
    /// Provider-native resume identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_cursor: Option<String>,
    /// Active model, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    /// Active permission policy.
    pub mode: PermissionMode,
    /// Provider-native available tool names.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Provider-native slash command names.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Provider-native skill names.
    #[serde(default)]
    pub skills: Vec<String>,
}

/// Normalized token usage from either provider.
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
    /// Provider-reported total, or Fleet's normalized total.
    pub total_tokens: u64,
    /// Server-side web-search requests.
    pub web_search_requests: u64,
    /// Provider or subagent tool-use count.
    pub tool_uses: u64,
    /// Additive provider usage fields retained losslessly.
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

/// Provider-authoritative terminal classification for a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum TurnOutcome {
    /// The provider completed normally.
    Completed,
    /// The provider completed with an execution error.
    Error {
        /// Best available provider message.
        message: Option<String>,
    },
    /// The provider settled an explicit interrupt.
    Interrupted,
    /// A permission denial prevented completion.
    Denied,
    /// A configured turn limit was reached.
    MaxTurns,
    /// A configured cost or token budget was exhausted.
    BudgetExhausted,
    /// The provider stopped on another named terminal reason.
    Other {
        /// Exact provider terminal reason.
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
    /// The provider process exited unexpectedly.
    ProviderExited,
    /// A bounded provider operation timed out.
    Timeout,
    /// A later prompt superseded this work.
    Superseded,
    /// An additive provider-native reason.
    Other(String),
}

/// Partial replacement fields for an existing projected item.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ItemPatch {
    /// Cumulative assistant or reasoning text replacement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Replacement structured tool input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    /// Replacement one-line tool summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Replacement compact result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Replacement expanded output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Replacement inline diff.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<super::ToolDiff>,
    /// Replacement lifecycle state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ItemStatus>,
}

/// Append-only content channel for [`AgentEvent::ContentDelta`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// Visible assistant prose.
    AssistantText,
    /// Hidden or collapsible reasoning.
    Reasoning,
    /// Streaming tool output.
    ToolOutput,
}

/// Observability boundary recorded in the transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum CheckpointKind {
    /// Context compaction changed the estimated token count.
    CompactBoundary {
        /// Tokens before compaction.
        before: u64,
        /// Tokens after compaction, when the provider reported them.
        after: Option<u64>,
    },
    /// A provider session was resumed after a pause.
    Resumed {
        /// Milliseconds since the prior live process was active.
        age_ms: u64,
    },
}

/// Lifecycle state of a projected item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// Known but not yet executing.
    #[default]
    Pending,
    /// Streaming or executing.
    Running,
    /// Completed successfully.
    Done,
    /// Completed with an error.
    Error,
    /// Rejected by a permission decision.
    Denied,
}

/// One normalized provider event before sequence and time stamping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Provider initialization completed.
    SessionStarted {
        /// Active provider.
        provider: AgentKind,
        /// Provider-native resume cursor.
        #[serde(default)]
        resume_cursor: Option<String>,
        /// Active model, when known.
        #[serde(default)]
        model: Option<ModelSelection>,
        /// Active permission policy.
        #[serde(default)]
        mode: PermissionMode,
        /// Provider-native tool names.
        #[serde(default)]
        tools: Vec<String>,
        /// Provider-native slash commands.
        #[serde(default)]
        commands: Vec<String>,
        /// Provider-native skills.
        #[serde(default)]
        skills: Vec<String>,
    },
    /// Session metadata changed after the session started.
    ///
    /// Mode, model and title are projected state a client renders, so a change to one has to
    /// reach the log and the mirrors as an event rather than as a silent edit of the daemon's
    /// own projection (`NATIVE-AGENTS.md` §3, §6). Only the fields that changed are carried.
    MetadataChanged {
        /// Provider-reported session title, when it changed.
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
    /// Provider process exited.
    SessionExited {
        /// Exit code when available.
        #[serde(default)]
        code: Option<i32>,
        /// Whether Fleet requested the exit.
        #[serde(default)]
        expected: bool,
    },
    /// The provider accepted a user turn.
    TurnStarted {
        /// Turn identity.
        turn: TurnId,
        /// Projected user-message item.
        user_item: ItemId,
    },
    /// Provider-authoritative turn completion.
    TurnCompleted {
        /// Settled turn.
        turn: TurnId,
        /// Terminal classification.
        outcome: TurnOutcome,
        /// Per-turn normalized usage.
        #[serde(default)]
        usage: Usage,
        /// Provider-reported duration.
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
    /// A transcript item became known.
    ItemStarted {
        /// Owning turn.
        turn: TurnId,
        /// Item identity.
        item: ItemId,
        /// Item category and provider payload.
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
        /// Fields to replace.
        patch: ItemPatch,
    },
    /// An item settled.
    ItemCompleted {
        /// Target item.
        item: ItemId,
        /// Terminal item status.
        status: ItemStatus,
    },
    /// A provider decision became actionable.
    GateOpened {
        /// Gate identity.
        gate: GateId,
        /// Associated turn, when known.
        #[serde(default)]
        turn: Option<TurnId>,
        /// Gate presentation and provider mapping payload.
        kind: GateKind,
    },
    /// A gate was authoritatively closed.
    GateResolved {
        /// Gate identity.
        gate: GateId,
        /// Normalized answer.
        answer: GateAnswer,
        /// Resolver authority.
        by: GateResolver,
    },
    /// Updated token, context, and cost observations.
    TokenUsage {
        /// Associated turn.
        turn: TurnId,
        /// Latest provider usage.
        #[serde(default)]
        usage: Usage,
        /// Context-window utilization percentage.
        #[serde(default)]
        context_pct: f32,
        /// Latest cumulative process or session cost.
        #[serde(default)]
        cost_usd: Option<f64>,
    },
    /// A compaction or resume boundary.
    Checkpoint(CheckpointKind),
    /// Provider retry backoff began.
    Retrying {
        /// One-based attempt number.
        attempt: u32,
        /// Delay before the next attempt.
        retry_in_ms: u64,
        /// Provider error text.
        reason: String,
    },
    /// Runtime or protocol failure.
    RuntimeError {
        /// Whether the provider session can no longer continue.
        fatal: bool,
        /// Sanitized human-readable detail.
        message: String,
    },
    /// Non-fatal configuration, compatibility, or deprecation notice.
    Notice(String),
}

/// A normalized event stamped by the serialized thread reducer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeqEvent {
    /// Monotonic per-thread cursor.
    pub seq: Seq,
    /// Reducer timestamp.
    pub at: DateTime<Utc>,
    /// Provider message type or event name retained for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// Provider-neutral payload.
    pub event: AgentEvent,
}
