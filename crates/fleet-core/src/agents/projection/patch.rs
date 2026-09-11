//! Item payload patching and the append-only content channels.

use chrono::{DateTime, Utc};

use super::ProjectionError;
use crate::agents::{
    Item, ItemKind, ItemPatch, ItemPayloadPatch, ItemStatus, StreamKind, ToolCall, ToolPatch,
};

pub(super) trait ItemStatusExt {
    fn terminal(self) -> bool;
}

impl ItemStatusExt for ItemStatus {
    fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Denied | Self::Stopped
        )
    }
}

pub(super) fn apply_item_patch(item: &mut Item, patch: &ItemPatch, at: DateTime<Utc>) -> bool {
    if let Some(payload) = &patch.payload {
        apply_payload_patch(&mut item.kind, payload);
    }
    if let Some(status) = patch.status {
        item.status = status;
        item.ended = status.terminal().then_some(at);
    }
    item.status.terminal()
}

fn apply_payload_patch(item: &mut ItemKind, patch: &ItemPayloadPatch) {
    if let ItemPayloadPatch::Tool(tool_patch) = patch
        && let ItemKind::Tool(call) = item
    {
        apply_tool_patch(call, tool_patch);
        return;
    }
    match (item, patch) {
        (
            ItemKind::UserMessage {
                text,
                attachments,
                steered,
            },
            ItemPayloadPatch::UserMessage {
                text: next_text,
                attachments: next_attachments,
                steered: next_steered,
            },
        ) => {
            if let Some(next) = next_text {
                text.clone_from(next);
            }
            if let Some(next) = next_attachments {
                attachments.clone_from(next);
            }
            if let Some(next) = next_steered {
                *steered = *next;
            }
        }
        (ItemKind::AssistantText { text }, ItemPayloadPatch::AssistantText { text: next })
        | (ItemKind::Plan { text }, ItemPayloadPatch::Plan { text: next }) => {
            text.clone_from(next);
        }
        (
            ItemKind::Reasoning { summary, raw },
            ItemPayloadPatch::Reasoning {
                summary: next_summary,
                raw: next_raw,
            },
        ) => {
            if let Some(next) = next_summary {
                summary.clone_from(next);
            }
            if let Some(next) = next_raw {
                raw.clone_from(next);
            }
        }
        (
            ItemKind::Subagent {
                name,
                description,
                result,
            },
            ItemPayloadPatch::Subagent {
                name: next_name,
                description: next_description,
                result: next_result,
            },
        ) => {
            if let Some(next) = next_name {
                name.clone_from(next);
            }
            if let Some(next) = next_description {
                description.clone_from(next);
            }
            if let Some(next) = next_result {
                *result = Some(next.clone());
            }
        }
        (ItemKind::Error { message }, ItemPayloadPatch::Error { message: next }) => {
            message.clone_from(next);
        }
        _ => {}
    }
}

fn apply_tool_patch(call: &mut ToolCall, patch: &ToolPatch) {
    let ToolCall {
        kind,
        name,
        input,
        summary,
        result,
        output,
        diff,
        exit_code,
        duration_ms,
        extra,
    } = call;
    if let Some(next) = &patch.kind {
        kind.clone_from(next);
    }
    if let Some(next) = &patch.name {
        name.clone_from(next);
    }
    if let Some(next) = &patch.input {
        input.clone_from(next);
    }
    if let Some(next) = &patch.summary {
        *summary = Some(next.clone());
    }
    if let Some(next) = &patch.result {
        *result = Some(next.clone());
    }
    if let Some(next) = &patch.output {
        output.clone_from(next);
    }
    if let Some(next) = &patch.diff {
        *diff = Some(next.clone());
    }
    if let Some(next) = patch.exit_code {
        *exit_code = Some(next);
    }
    if let Some(next) = patch.duration_ms {
        *duration_ms = Some(next);
    }
    if let Some(next) = &patch.extra {
        extra.clone_from(next);
    }
}

pub(super) fn append_content(
    item: &mut Item,
    stream: StreamKind,
    delta: &str,
) -> Result<(), ProjectionError> {
    match (&mut item.kind, stream) {
        (ItemKind::AssistantText { text }, StreamKind::AssistantText)
        | (ItemKind::Plan { text }, StreamKind::PlanText) => text.push_str(delta),
        (ItemKind::Reasoning { summary, .. }, StreamKind::ReasoningSummary { part }) => {
            summary.entry(part).or_default().push_str(delta);
        }
        (ItemKind::Reasoning { raw, .. }, StreamKind::ReasoningRaw { part }) => {
            raw.entry(part).or_default().push_str(delta);
        }
        (ItemKind::Tool(call), StreamKind::CommandOutput) => call.output.push_str(delta),
        _ => return Err(ProjectionError::WrongStream(item.id, stream)),
    }
    Ok(())
}

pub(super) fn tool_has_result(item: &ItemKind) -> bool {
    matches!(item, ItemKind::Tool(call) if call.result.is_some())
}
