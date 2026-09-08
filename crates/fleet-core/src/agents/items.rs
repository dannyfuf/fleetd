//! Transcript item and tool projection contracts.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Attachment, ItemId, ItemStatus, TurnId};

/// Normalized visual category for a provider tool call.
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
    /// Search paths or other provider indexes.
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
        /// Provider-native server name.
        server: String,
    },
    /// A tool not recognized by this Fleet build.
    Unknown {
        /// Provider-native tool name.
        name: String,
    },
}

/// Provider-neutral kind of one transcript item.
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
    },
    /// Visible assistant prose.
    AssistantText,
    /// Collapsible provider reasoning.
    Thinking,
    /// A tool call with its raw provider name and input.
    Tool {
        /// Normalized kind-column value.
        kind: ToolKind,
        /// Provider-native tool name.
        name: String,
        /// Provider-native structured input.
        input: Value,
    },
    /// A subagent task projected as a nested row.
    Subagent {
        /// Provider-native agent name.
        name: String,
        /// Human-readable task description.
        description: String,
    },
    /// A transcript-level error item.
    Error,
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

/// Current projected state of one transcript item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// Stable item identity, including provider part identity after normalization.
    pub id: ItemId,
    /// Owning user turn.
    pub turn: TurnId,
    /// Optional parent tool or subagent item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ItemId>,
    /// Rendering and semantic category.
    pub kind: ItemKind,
    /// Current lifecycle state.
    pub status: ItemStatus,
    /// Accumulated assistant text or reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// One-line tool summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Compact right-aligned tool result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Expanded tool output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Optional inline unified diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<ToolDiff>,
    /// Ordered nested item identities.
    #[serde(default)]
    pub children: Vec<ItemId>,
    /// Time execution or streaming began.
    pub started: DateTime<Utc>,
    /// Time the item settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<DateTime<Utc>>,
}
