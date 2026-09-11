//! Transcript item and typed payload contracts.

use std::{collections::BTreeMap, path::PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Attachment, ItemId, ItemStatus, TurnId};

/// Normalized visual category for a harness tool call.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ToolKind {
    /// Read a file.
    Read,
    /// Edit an existing file.
    Edit,
    /// Write or replace a file.
    Write,
    /// Run a shell command.
    Bash,
    /// Search paths or other harness indexes.
    Search,
    /// Search file contents.
    Grep,
    /// Fetch a remote resource.
    Fetch,
    /// Spawn or communicate with a subagent.
    Agent,
    /// Update task or todo state.
    Todo,
    /// Load a skill.
    Skill,
    /// Invoke a Model Context Protocol server.
    Mcp {
        /// Harness-native server name.
        server: String,
    },
    /// A tool not recognized by this Fleet build.
    Unknown {
        /// Harness-native tool name.
        name: String,
    },
}

/// A unified file diff attached to a tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDiff {
    /// Changed path.
    pub path: PathBuf,
    /// Added line count.
    pub added: u64,
    /// Removed line count.
    pub removed: u64,
    /// Complete unified-diff text.
    pub unified: String,
}

/// One tool invocation: what the harness was asked to do, and what it reported back.
///
/// Input and result stay as JSON values so an adapter never has to flatten structured output
/// into presentation text before the app sees it (§3.2). Field names are the serialized names of
/// the payload, so they are deliberately not renamed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Normalized kind-column value.
    pub kind: ToolKind,
    /// Harness-native tool name.
    pub name: String,
    /// Harness-native structured input.
    pub input: Value,
    /// Optional one-line semantic summary supplied by the adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Structured terminal result. Strings remain JSON strings rather than being privileged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Streaming textual output.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    /// Optional inline unified diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<ToolDiff>,
    /// Process exit status, when the tool is command-shaped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Harness-reported tool duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Additive harness fields retained losslessly.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

/// Typed payload for one transcript item.
///
/// Fields live only on the variants that can carry them. In particular, tool input and results
/// stay as JSON values so an adapter never has to flatten structured output into presentation
/// text before the app sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ItemKind {
    /// A submitted user message and its attachments.
    UserMessage {
        /// Message text.
        text: String,
        /// Ordered attachments.
        #[serde(default)]
        attachments: Vec<Attachment>,
        /// Whether this message joined a turn that was already running.
        ///
        /// §7.2 draws a steer as an ordinary bubble with a leading `↳`, and only the harness can
        /// answer whether a submission was folded into the running turn or started its own —
        /// Claude coalesces and reports `queued_turn_count`, Codex answers `turn/steer` or
        /// refuses it. The adapter's answer is recorded here so the mark survives a reload
        /// instead of living only in the sending client's optimistic row.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        steered: bool,
    },
    /// Visible assistant prose.
    AssistantText {
        /// Accumulated Markdown text.
        #[serde(default)]
        text: String,
    },
    /// Collapsible harness reasoning, split by stable part index.
    Reasoning {
        /// User-facing reasoning summary parts.
        #[serde(default)]
        summary: BTreeMap<u32, String>,
        /// Raw reasoning parts, retained separately from summaries.
        #[serde(default)]
        raw: BTreeMap<u32, String>,
    },
    /// A tool call and its structured input and result.
    ///
    /// Boxed because it is by far the widest payload an item can carry and most items in a
    /// transcript are text: inline, every assistant paragraph would occupy a tool-sized slot in
    /// `ThreadProjection::items`.
    Tool(Box<ToolCall>),
    /// A subagent task projected as a nested row.
    Subagent {
        /// Harness-native agent name.
        name: String,
        /// Human-readable task description.
        description: String,
        /// Structured final accounting or result.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
    },
    /// A durable proposed plan.
    Plan {
        /// Full Markdown plan text.
        #[serde(default)]
        text: String,
    },
    /// A transcript-level error item.
    Error {
        /// Sanitized human-readable detail.
        #[serde(default)]
        message: String,
    },
}

/// Variant-specific replacement fields for an existing item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ItemPayloadPatch {
    /// Replace user-message fields.
    UserMessage {
        /// Replacement text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Replacement attachment list.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attachments: Option<Vec<Attachment>>,
        /// Replacement steer mark, for a harness that answers after the row already exists.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        steered: Option<bool>,
    },
    /// Replace the cumulative assistant text.
    AssistantText { text: String },
    /// Replace reasoning parts without merging the two channels.
    Reasoning {
        /// Replacement summary parts.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<BTreeMap<u32, String>>,
        /// Replacement raw parts.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw: Option<BTreeMap<u32, String>>,
    },
    /// Replace tool metadata or structured results.
    ///
    /// Boxed because a tool patch is by far the widest payload here, and this enum rides inside
    /// every `ItemUpdated` event Fleet appends, broadcasts and mirrors.
    Tool(Box<ToolPatch>),
    /// Replace subagent metadata.
    Subagent {
        /// Replacement agent name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Replacement task description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// Replacement structured result.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
    },
    /// Replace the cumulative plan text.
    Plan { text: String },
    /// Replace an error message.
    Error { message: String },
}

/// Replacement fields for a tool payload.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ToolPatch {
    /// Replacement normalized tool category.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ToolKind>,
    /// Replacement harness-native tool name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Replacement structured input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    /// Replacement semantic summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Replacement structured result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Replacement cumulative output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Replacement inline diff.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<ToolDiff>,
    /// Replacement command exit status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Replacement harness-reported duration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Replacement additive harness fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<BTreeMap<String, Value>>,
}

/// Partial update for an existing projected item.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ItemPatch {
    /// Variant-specific payload changes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<ItemPayloadPatch>,
    /// Replacement lifecycle state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ItemStatus>,
}

/// Current projected state of one transcript item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// Stable item identity, including harness part identity after normalization.
    pub id: ItemId,
    /// Owning user turn.
    pub turn: TurnId,
    /// Optional parent tool or subagent item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ItemId>,
    /// Variant-owned rendering and semantic payload.
    pub kind: ItemKind,
    /// Current lifecycle state.
    pub status: ItemStatus,
    /// Ordered nested item identities.
    #[serde(default)]
    pub children: Vec<ItemId>,
    /// Time execution or streaming began.
    pub started: DateTime<Utc>,
    /// Time the item settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<DateTime<Utc>>,
}
