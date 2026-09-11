//! What one Codex item becomes: its identity, its payload, its patch and its status.
//!
//! The kind column comes from `commandActions[]` and never from the raw `command`, and
//! `declined` is a first-class terminal status distinct from `failed` — a denied tool is not an
//! error and must not render red.

use std::collections::BTreeMap;

use fleet_core::agents::{
    ItemKind, ItemPatch, ItemPayloadPatch, ItemStatus, ToolCall, ToolDiff, ToolKind, ToolPatch,
};
use serde_json::{Value, json};

use crate::agents::codex::wire::{
    CommandAction, CommandExecutionStatus, FileUpdateChange, McpToolCallStatus, PatchApplyStatus,
    ThreadItem, UserInput,
};

/// The provider's own id for an item.
pub(super) fn item_id_of(item: &ThreadItem) -> String {
    macro_rules! id {
        ($($variant:ident),+ $(,)?) => {
            match item {
                $(ThreadItem::$variant { id, .. } => id.clone(),)+
                ThreadItem::Unknown => String::new(),
            }
        };
    }
    id!(
        UserMessage,
        HookPrompt,
        AgentMessage,
        Plan,
        Reasoning,
        CommandExecution,
        FileChange,
        McpToolCall,
        DynamicToolCall,
        CollabAgentToolCall,
        SubAgentActivity,
        WebSearch,
        ImageView,
        Sleep,
        ImageGeneration,
        EnteredReviewMode,
        ExitedReviewMode,
        ContextCompaction,
    )
}

/// The Fleet payload one Codex item becomes, or `None` for an item kind with no row.
pub(super) fn item_payload(item: &ThreadItem) -> Option<ItemKind> {
    Some(match item {
        // Reached only for a message Fleet did not send — resumed history, or one injected
        // outside this daemon — so it never joined a turn Fleet was steering.
        ThreadItem::UserMessage { content, .. } => ItemKind::UserMessage {
            text: user_text(content),
            attachments: Vec::new(),
            steered: false,
        },
        ThreadItem::AgentMessage { text, .. } => ItemKind::AssistantText { text: text.clone() },
        ThreadItem::Plan { text, .. } => ItemKind::Plan { text: text.clone() },
        ThreadItem::Reasoning {
            summary, content, ..
        } => ItemKind::Reasoning {
            summary: indexed(summary.as_deref().unwrap_or_default()),
            raw: indexed(content.as_deref().unwrap_or_default()),
        },
        ThreadItem::CommandExecution {
            command,
            command_actions,
            cwd,
            aggregated_output,
            exit_code,
            duration_ms,
            ..
        } => ItemKind::Tool(Box::new(ToolCall {
            kind: action_kind(command_actions),
            name: action_name(command_actions),
            input: json!({"command": command, "cwd": cwd}),
            summary: Some(action_summary(command_actions, command)),
            result: None,
            output: aggregated_output.clone().unwrap_or_default(),
            diff: None,
            exit_code: exit_code.and_then(|code| i32::try_from(code).ok()),
            duration_ms: duration_ms.and_then(|ms| u64::try_from(ms).ok()),
            extra: BTreeMap::new(),
        })),
        ThreadItem::FileChange { changes, .. } => ItemKind::Tool(Box::new(ToolCall {
            kind: ToolKind::Edit,
            name: "apply_patch".to_owned(),
            input: json!({"paths": changes.iter().map(|change| change.path.clone()).collect::<Vec<_>>()}),
            summary: Some(changes_summary(changes)),
            result: None,
            output: String::new(),
            diff: changes_diff(changes),
            exit_code: None,
            duration_ms: None,
            extra: BTreeMap::new(),
        })),
        ThreadItem::McpToolCall {
            server,
            tool,
            arguments,
            result,
            duration_ms,
            ..
        } => ItemKind::Tool(Box::new(ToolCall {
            kind: ToolKind::Mcp {
                server: server.clone(),
            },
            name: format!("{server}__{tool}"),
            input: arguments.clone(),
            summary: Some(tool.clone()),
            result: result
                .as_ref()
                .and_then(|result| serde_json::to_value(result).ok()),
            output: String::new(),
            diff: None,
            exit_code: None,
            duration_ms: duration_ms.and_then(|ms| u64::try_from(ms).ok()),
            extra: BTreeMap::new(),
        })),
        ThreadItem::DynamicToolCall {
            tool,
            arguments,
            duration_ms,
            ..
        } => ItemKind::Tool(Box::new(ToolCall {
            kind: ToolKind::Unknown { name: tool.clone() },
            name: tool.clone(),
            input: arguments.clone(),
            summary: Some(tool.clone()),
            result: None,
            output: String::new(),
            diff: None,
            exit_code: None,
            duration_ms: duration_ms.and_then(|ms| u64::try_from(ms).ok()),
            extra: BTreeMap::new(),
        })),
        ThreadItem::WebSearch { .. } => ItemKind::Tool(Box::new(ToolCall {
            kind: ToolKind::Fetch,
            name: "web_search".to_owned(),
            input: Value::Null,
            summary: Some("web search".to_owned()),
            result: None,
            output: String::new(),
            diff: None,
            exit_code: None,
            duration_ms: None,
            extra: BTreeMap::new(),
        })),
        ThreadItem::CollabAgentToolCall { tool, prompt, .. } => ItemKind::Subagent {
            name: serde_json::to_value(tool)
                .ok()
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "subagent".to_owned()),
            description: prompt.clone().unwrap_or_default(),
            result: None,
        },
        ThreadItem::ImageView { path, .. } => ItemKind::Tool(Box::new(ToolCall {
            kind: ToolKind::Read,
            name: "view_image".to_owned(),
            input: json!({"path": path}),
            summary: Some(path.clone()),
            result: None,
            output: String::new(),
            diff: None,
            exit_code: None,
            duration_ms: None,
            extra: BTreeMap::new(),
        })),
        // These carry no row of their own: a compaction is a boundary, a subagent activity is a
        // registration, and the review-mode markers are chrome.
        ThreadItem::ContextCompaction { .. }
        | ThreadItem::SubAgentActivity { .. }
        | ThreadItem::EnteredReviewMode { .. }
        | ThreadItem::ExitedReviewMode { .. }
        | ThreadItem::HookPrompt { .. }
        | ThreadItem::Sleep { .. }
        | ThreadItem::ImageGeneration { .. } => return None,
        ThreadItem::Unknown => return None,
    })
}

/// The patch a completed item applies to its row.
pub(super) fn item_patch(item: &ThreadItem) -> Option<ItemPatch> {
    let payload = match item {
        ThreadItem::AgentMessage { text, .. } => ItemPayloadPatch::AssistantText {
            // The completed item's text is authoritative and replaces the delta buffer.
            text: text.clone(),
        },
        ThreadItem::Reasoning {
            summary, content, ..
        } => ItemPayloadPatch::Reasoning {
            summary: Some(indexed(summary.as_deref().unwrap_or_default())),
            raw: Some(indexed(content.as_deref().unwrap_or_default())),
        },
        ThreadItem::CommandExecution {
            command,
            command_actions,
            aggregated_output,
            exit_code,
            duration_ms,
            ..
        } => ItemPayloadPatch::Tool(Box::new(ToolPatch {
            kind: Some(action_kind(command_actions)),
            summary: Some(action_summary(command_actions, command)),
            output: aggregated_output.clone(),
            exit_code: exit_code.and_then(|code| i32::try_from(code).ok()),
            duration_ms: duration_ms.and_then(|ms| u64::try_from(ms).ok()),
            ..ToolPatch::default()
        })),
        ThreadItem::FileChange { changes, .. } => ItemPayloadPatch::Tool(Box::new(ToolPatch {
            summary: Some(changes_summary(changes)),
            diff: changes_diff(changes),
            ..ToolPatch::default()
        })),
        ThreadItem::McpToolCall { result, error, .. } => {
            ItemPayloadPatch::Tool(Box::new(ToolPatch {
                result: result
                    .as_ref()
                    .and_then(|result| serde_json::to_value(result).ok())
                    .or_else(|| {
                        error
                            .as_ref()
                            .and_then(|error| serde_json::to_value(error).ok())
                    }),
                ..ToolPatch::default()
            }))
        }
        _ => return None,
    };
    Some(ItemPatch {
        payload: Some(payload),
        status: Some(item_status(item)),
    })
}

/// The terminal status of a completed item.
///
/// **`declined` is a first-class terminal status**, distinct from `failed`, on both
/// `CommandExecutionStatus` and `PatchApplyStatus`: a denied tool is not an error and must not
/// render red.
pub(super) fn item_status(item: &ThreadItem) -> ItemStatus {
    match item {
        ThreadItem::CommandExecution { status, .. } => match status {
            CommandExecutionStatus::Completed => ItemStatus::Completed,
            CommandExecutionStatus::Failed => ItemStatus::Failed,
            CommandExecutionStatus::Declined => ItemStatus::Denied,
            CommandExecutionStatus::InProgress | CommandExecutionStatus::Unknown => {
                ItemStatus::Completed
            }
        },
        ThreadItem::FileChange { status, .. } => match status {
            PatchApplyStatus::Completed => ItemStatus::Completed,
            PatchApplyStatus::Failed => ItemStatus::Failed,
            PatchApplyStatus::Declined => ItemStatus::Denied,
            PatchApplyStatus::InProgress | PatchApplyStatus::Unknown => ItemStatus::Completed,
        },
        ThreadItem::McpToolCall { status, .. } => match status {
            McpToolCallStatus::Completed => ItemStatus::Completed,
            McpToolCallStatus::Failed => ItemStatus::Failed,
            McpToolCallStatus::InProgress | McpToolCallStatus::Unknown => ItemStatus::Completed,
        },
        _ => ItemStatus::Completed,
    }
}

/// The kind column, from `commandActions[]`.
pub(super) fn action_kind(actions: &[CommandAction]) -> ToolKind {
    match first_meaningful(actions) {
        Some(CommandAction::Read { .. }) => ToolKind::Read,
        Some(CommandAction::ListFiles { .. }) => ToolKind::Search,
        Some(CommandAction::Search { .. }) => ToolKind::Grep,
        // `{type:"unknown"}` falls back to the raw command line, which is what a shell row is.
        Some(CommandAction::Unknown { .. } | CommandAction::Unrecognized) | None => ToolKind::Bash,
    }
}

/// The tool name a row shows.
pub(super) fn action_name(actions: &[CommandAction]) -> String {
    match first_meaningful(actions) {
        Some(CommandAction::Read { .. }) => "read".to_owned(),
        Some(CommandAction::ListFiles { .. }) => "list".to_owned(),
        Some(CommandAction::Search { .. }) => "search".to_owned(),
        _ => "shell".to_owned(),
    }
}

/// The one-line summary a row shows.
///
/// The summary uses the **first meaningful action** and the expanded body lists all of them,
/// because `commandActions` is an array: a piped command decomposes.
pub(super) fn action_summary(actions: &[CommandAction], command: &str) -> String {
    match first_meaningful(actions) {
        Some(CommandAction::Read { name, .. }) => name.clone(),
        Some(CommandAction::ListFiles { path, .. }) => {
            path.clone().unwrap_or_else(|| "files".to_owned())
        }
        Some(CommandAction::Search { query, path, .. }) => match (query, path) {
            (Some(query), Some(path)) => format!("{query} in {path}"),
            (Some(query), None) => query.clone(),
            (None, Some(path)) => path.clone(),
            (None, None) => "search".to_owned(),
        },
        Some(CommandAction::Unknown { command }) => command.clone(),
        _ => command.to_owned(),
    }
}

/// The first action that says something more than "unknown".
fn first_meaningful(actions: &[CommandAction]) -> Option<&CommandAction> {
    actions
        .iter()
        .find(|action| !matches!(action, CommandAction::Unrecognized))
        .or_else(|| actions.first())
}

/// The unified diff of a `fileChange` item's changes.
pub(super) fn changes_diff(changes: &[FileUpdateChange]) -> Option<ToolDiff> {
    let first = changes.first()?;
    let unified = changes
        .iter()
        .map(|change| change.diff.clone())
        .collect::<Vec<_>>()
        .join("\n");
    let added = unified
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count();
    let removed = unified
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count();
    Some(ToolDiff {
        path: std::path::PathBuf::from(&first.path),
        added: added as u64,
        removed: removed as u64,
        unified,
    })
}

/// The one-line summary of a `fileChange` item.
fn changes_summary(changes: &[FileUpdateChange]) -> String {
    match changes {
        [] => "no changes".to_owned(),
        [single] => single.path.clone(),
        many => format!("{} files", many.len()),
    }
}

/// The user-visible text of a `userMessage` item's parts.
fn user_text(content: &[UserInput]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            UserInput::Text { text, .. } => Some(text.clone()),
            UserInput::Skill { name, .. } => Some(format!("${name}")),
            UserInput::Mention { name, .. } => Some(format!("@{name}")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A reasoning channel's parts, keyed by index.
fn indexed(parts: &[String]) -> BTreeMap<u32, String> {
    parts
        .iter()
        .enumerate()
        .map(|(index, text)| (u32::try_from(index).unwrap_or_default(), text.clone()))
        .collect()
}

/// The top-level steps of a Markdown plan.
pub(super) fn plan_steps(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| {
            let line = line.trim_end();
            ["- ", "* ", "+ "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                .map(ToOwned::to_owned)
        })
        .collect()
}
