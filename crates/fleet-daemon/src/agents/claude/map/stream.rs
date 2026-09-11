//! Streaming deltas, assistant snapshots and tool results.
//!
//! Six invariants from spec A.2.5, each of which is a bug if violated:
//!
//! 1. **Empty deltas are dropped at the source.** They cost a frame, a wakeup and a notify.
//! 2. **A content block, not a message, is the transcript atom.** One `message.id` legitimately
//!    spans four `assistant` frames with different block types.
//! 3. **A reused block index within one turn mints a new item id.**
//! 4. **The assistant text item closes before the following tool item opens.**
//! 5. **A block that never streamed is synthesised from its snapshot at completion**, or a
//!    provider that streamed nothing yields an empty message.
//! 6. **Subagent narration is dropped; subagent tool blocks are not.** Emitting the narration
//!    interleaves N subagents' prose into the chat; dropping the input deltas empties attributed
//!    tool inputs. Both halves are t3code live-test findings.

use fleet_core::agents::{
    AgentEvent, ItemId, ItemPatch, ItemPayloadPatch, ItemStatus, StreamKind, ToolPatch, TurnId,
};
use serde_json::{Value, json};

use super::{
    MapOutput,
    text::{assistant_item, content_text, reasoning_item, text_replacement, value_fingerprint},
    tools::{derive_tool_diff, tool_summary},
};
use crate::agents::claude::{
    frames::{AssistantFrame, StreamFrame, UserFrame},
    session::{BlockKey, BlockKind, ClaudeSession, StreamBlock},
};

/// Maps one `assistant` snapshot frame.
///
/// Snapshots **backfill**; they never stream. They are also where a plan can arrive with no
/// permission callback behind it, and where the two failure latches live.
pub(in crate::agents::claude) fn assistant(
    session: &mut ClaudeSession,
    frame: AssistantFrame,
) -> MapOutput {
    let mut events = Vec::new();
    if let Some(error) = &frame.error {
        // A latch, not a row: it only speaks when the `result` names no cause of its own.
        session.failure_latch = Some(error.clone());
    }
    let Some(turn) = session.active_turn() else {
        tracing::warn!(
            target: "fleet::agents::claude",
            "Claude assistant output arrived with no active turn"
        );
        return MapOutput::default();
    };
    session.current_message = frame.message.id.clone();
    let owner = frame.parent_tool_use_id.clone();
    let parent = owner
        .as_deref()
        .and_then(|id| session.tool_items.get(id).copied());
    let Some(blocks) = frame.message.content.as_array() else {
        return MapOutput::from(events);
    };
    for block in blocks.iter().cloned() {
        match block.get("type").and_then(Value::as_str) {
            // Invariant 6: subagent narration never enters the main transcript.
            Some("text") if owner.is_some() => {}
            Some("thinking") if owner.is_some() => {}
            Some("text") => {
                let text = block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                complete_streamed_text(
                    session,
                    turn,
                    SnapshotBlock {
                        owner: owner.as_deref(),
                        kind: BlockKind::Text,
                        stream: StreamKind::AssistantText,
                        text,
                    },
                    parent,
                    &mut events,
                );
            }
            Some("thinking") => {
                let text = block
                    .get("thinking")
                    .or_else(|| block.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                complete_streamed_text(
                    session,
                    turn,
                    SnapshotBlock {
                        owner: owner.as_deref(),
                        kind: BlockKind::Thinking,
                        stream: StreamKind::ReasoningSummary { part: 0 },
                        text,
                    },
                    parent,
                    &mut events,
                );
            }
            Some("tool_use") => assistant_tool(session, turn, block, parent, &mut events),
            Some(other) => tracing::debug!(
                target: "fleet::agents::claude",
                block = other,
                "no projection for this Claude assistant content block"
            ),
            None => {}
        }
    }
    MapOutput::from(events)
}

/// One content block of an `assistant` snapshot, named by the stream it backfills.
///
/// The four fields travel together at both call sites: they are the block's identity, and the
/// snapshot frame is the only thing that knows them.
struct SnapshotBlock<'a> {
    /// The subagent whose index space this block belongs to, or `None` for the main agent.
    owner: Option<&'a str>,
    kind: BlockKind,
    stream: StreamKind,
    text: &'a str,
}

/// Completes the streamed block an `assistant` snapshot backfills, or synthesises one.
fn complete_streamed_text(
    session: &mut ClaudeSession,
    turn: TurnId,
    block: SnapshotBlock<'_>,
    parent: Option<ItemId>,
    events: &mut Vec<AgentEvent>,
) {
    let SnapshotBlock {
        owner,
        kind: block_kind,
        stream,
        text,
    } = block;
    // Frames arrive in block order *within one owner*, so the oldest open block of this kind
    // belonging to the same stream is the one this frame completes; a subagent's blocks are never
    // matched against the main agent's, which share their index space.
    let streamed = session
        .stream_blocks
        .iter()
        .filter(|((block_owner, _), block)| {
            block_owner.as_deref() == owner && block.kind == block_kind && !block.completed
        })
        .map(|(key, _)| key.clone())
        .min()
        .and_then(|key| session.stream_blocks.get_mut(&key));
    if block_kind == BlockKind::Text && owner.is_none() && !text.trim().is_empty() {
        session.last_assistant_text = Some(text.to_owned());
    }
    let item = if let Some(block) = streamed {
        block.completed = true;
        if let Some(missing) = text.strip_prefix(&block.text) {
            if !missing.is_empty() {
                events.push(AgentEvent::ContentDelta {
                    item: block.item,
                    stream,
                    delta: missing.to_owned(),
                });
            }
        } else if block.text != text {
            events.push(AgentEvent::ItemUpdated {
                item: block.item,
                patch: text_replacement(stream, text),
            });
        }
        block.item
    } else {
        // Invariant 5: a block that never streamed is still a row.
        let kind = match block_kind {
            BlockKind::Thinking => reasoning_item(),
            _ => assistant_item(),
        };
        let item = session.start_item(turn, kind, parent, None, Value::Null, events);
        if !text.is_empty() {
            events.push(AgentEvent::ContentDelta {
                item,
                stream,
                delta: text.to_owned(),
            });
        }
        item
    };
    session.complete_item(item, ItemStatus::Completed, events);
}

/// Registers or updates the tool a snapshot's `tool_use` block names.
fn assistant_tool(
    session: &mut ClaudeSession,
    turn: TurnId,
    block: Value,
    parent: Option<ItemId>,
    events: &mut Vec<AgentEvent>,
) {
    let provider_id = block
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let name = block
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Unknown")
        .to_owned();
    let input = block.get("input").cloned().unwrap_or(Value::Null);
    if let Some(item) = session.tool_items.get(&provider_id).copied() {
        if let Some(open) = session.open_items.get_mut(&item) {
            open.input = input.clone();
            open.tool_name = Some(name.clone());
        }
        events.push(AgentEvent::ItemUpdated {
            item,
            patch: ItemPatch {
                payload: Some(ItemPayloadPatch::Tool(Box::new(ToolPatch {
                    input: Some(input.clone()),
                    summary: Some(tool_summary(&name, &input, None)),
                    ..ToolPatch::default()
                }))),
                status: Some(ItemStatus::InProgress),
            },
        });
    } else {
        let item = session.start_tool(turn, &provider_id, &name, input, parent, events);
        session.tool_items.insert(provider_id, item);
    }
}

/// Maps one `user` frame, which is how tool results arrive.
///
/// `content[]` is the model-visible rendering; `tool_use_result` is the structured payload a
/// `Read` row's line count or an `Edit` row's `+14 −3` is computed from.
pub(in crate::agents::claude) fn user(session: &mut ClaudeSession, frame: UserFrame) -> MapOutput {
    let Some(blocks) = frame.message.content.as_array() else {
        return MapOutput::default();
    };
    let mut events = Vec::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        let provider_id = block
            .get("tool_use_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(item) = session.tool_items.get(provider_id).copied() else {
            tracing::warn!(
                target: "fleet::agents::claude",
                "Claude returned a result for a tool this thread never opened"
            );
            continue;
        };
        let is_error = block
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let output = content_text(block.get("content"));
        let open = session.open_items.get(&item).cloned();
        let diff = open.as_ref().and_then(|open| {
            derive_tool_diff(
                open.tool_name.as_deref().unwrap_or_default(),
                &open.input,
                frame.tool_use_result.as_ref(),
            )
        });
        if let Some(diff) = &diff {
            let entry = session.files_changed.entry(diff.path.clone()).or_default();
            entry.0 = entry.0.saturating_add(diff.added);
            entry.1 = entry.1.saturating_add(diff.removed);
        }
        let status = if is_error {
            ItemStatus::Failed
        } else {
            ItemStatus::Completed
        };
        events.push(AgentEvent::ItemUpdated {
            item,
            patch: ItemPatch {
                payload: Some(ItemPayloadPatch::Tool(Box::new(ToolPatch {
                    summary: open.as_ref().and_then(|open| {
                        diff.as_ref().map(|diff| {
                            tool_summary(
                                open.tool_name.as_deref().unwrap_or_default(),
                                &open.input,
                                Some(diff),
                            )
                        })
                    }),
                    result: frame
                        .tool_use_result
                        .clone()
                        .or_else(|| Some(json!(if is_error { "error" } else { "done" }))),
                    output: (!output.is_empty()).then_some(output),
                    diff,
                    ..ToolPatch::default()
                }))),
                status: Some(status),
            },
        });
        session.complete_item(item, status, &mut events);
    }
    MapOutput::from(events)
}

/// Maps one `stream_event` frame.
pub(in crate::agents::claude) fn stream(
    session: &mut ClaudeSession,
    frame: StreamFrame,
) -> MapOutput {
    let event_type = frame
        .event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut events = Vec::new();
    match event_type {
        "message_start" => {
            session.current_message = frame
                .event
                .get("message")
                .and_then(|message| message.get("id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
        }
        "content_block_start" => block_start(session, &frame, &mut events),
        "content_block_delta" => block_delta(session, &frame, &mut events),
        "content_block_stop" => block_stop(session, &frame, &mut events),
        "message_delta" => {
            // Subagent usage must never move the thread meter.
            if frame.parent_tool_use_id.is_none()
                && let Some(turn) = session.active_turn()
                && let Some(usage) = frame.event.get("usage")
            {
                let mapped = super::settle::usage_from_value(usage);
                if !super::settle::usage_is_empty(&mapped) {
                    events.push(AgentEvent::TokenUsage {
                        turn,
                        usage: mapped,
                        context_pct: 0.0,
                        cost_usd: None,
                    });
                }
            }
        }
        "message_stop" => {}
        other => tracing::debug!(
            target: "fleet::agents::claude",
            event = other,
            "no projection for this Claude stream event"
        ),
    }
    MapOutput::from(events)
}

fn block_start(session: &mut ClaudeSession, frame: &StreamFrame, events: &mut Vec<AgentEvent>) {
    let Some(turn) = session.active_turn() else {
        return;
    };
    let key = block_key(frame);
    let content = frame
        .event
        .get("content_block")
        .cloned()
        .unwrap_or(Value::Null);
    let content_type = content
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let owner = frame.parent_tool_use_id.as_deref();
    let parent = owner.and_then(|id| session.tool_items.get(id).copied());
    let is_tool = matches!(
        content_type,
        "tool_use" | "server_tool_use" | "mcp_tool_use"
    );
    // Invariant 6: a subagent's non-tool blocks are dropped, its tool blocks are not.
    if owner.is_some() && !is_tool {
        return;
    }
    let (item, kind, tool_name) = if is_tool {
        let provider_id = content
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let name = content
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_owned();
        let input = content.get("input").cloned().unwrap_or(Value::Null);
        let item = session.start_tool(turn, &provider_id, &name, input, parent, events);
        (item, BlockKind::Tool, Some(name))
    } else {
        match content_type {
            "text" => (
                session.start_item(turn, assistant_item(), parent, None, Value::Null, events),
                BlockKind::Text,
                None,
            ),
            "thinking" => (
                session.start_item(turn, reasoning_item(), parent, None, Value::Null, events),
                BlockKind::Thinking,
                None,
            ),
            other => {
                tracing::debug!(
                    target: "fleet::agents::claude",
                    block = other,
                    "no projection for this Claude stream content block"
                );
                (ItemId::new(), BlockKind::Unknown, None)
            }
        }
    };
    // Invariant 3: the index is reused across messages within one turn, and the *new* block gets
    // a new item id because `start_item`/`start_tool` mint one. Replacing the entry is what keeps
    // the two rows separate.
    session.stream_blocks.insert(
        key,
        StreamBlock {
            item,
            kind,
            tool_name,
            input_json: String::new(),
            text: String::new(),
            completed: false,
            input_fingerprint: None,
        },
    );
}

fn block_delta(session: &mut ClaudeSession, frame: &StreamFrame, events: &mut Vec<AgentEvent>) {
    let key = block_key(frame);
    let Some(block) = session.stream_blocks.get_mut(&key) else {
        return;
    };
    let delta = frame.event.get("delta").unwrap_or(&Value::Null);
    match delta.get("type").and_then(Value::as_str) {
        Some("text_delta") => {
            let text = delta
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            // Invariant 1: an empty delta carries nothing to append.
            if !text.is_empty() {
                block.text.push_str(text);
                events.push(AgentEvent::ContentDelta {
                    item: block.item,
                    stream: StreamKind::AssistantText,
                    delta: text.to_owned(),
                });
            }
        }
        Some("thinking_delta") => {
            let text = delta
                .get("thinking")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !text.is_empty() {
                block.text.push_str(text);
                events.push(AgentEvent::ContentDelta {
                    item: block.item,
                    // Claude fills only summary part 0; the part index is load-bearing on Codex.
                    stream: StreamKind::ReasoningSummary { part: 0 },
                    delta: text.to_owned(),
                });
            }
        }
        Some("input_json_delta") => {
            block.input_json.push_str(
                delta
                    .get("partial_json")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            // Re-parse the whole document and republish only when the *parsed* input changed.
            if let Ok(input) = serde_json::from_str::<Value>(&block.input_json) {
                let fingerprint = value_fingerprint(&input);
                if block.input_fingerprint != Some(fingerprint) {
                    block.input_fingerprint = Some(fingerprint);
                    let item = block.item;
                    let name = block.tool_name.clone().unwrap_or_default();
                    if let Some(open) = session.open_items.get_mut(&item) {
                        open.input = input.clone();
                    }
                    events.push(tool_input_update(item, &name, &input));
                }
            }
        }
        // `signature_delta` closes the thinking block's cryptographic signature and is **not**
        // display text: appending it to the thinking body is a visible corruption bug.
        Some("signature_delta" | "citations_delta") | None => {}
        Some(other) => tracing::debug!(
            target: "fleet::agents::claude",
            delta = other,
            "no projection for this Claude stream delta"
        ),
    }
}

fn block_stop(session: &mut ClaudeSession, frame: &StreamFrame, events: &mut Vec<AgentEvent>) {
    let key = block_key(frame);
    let Some(block) = session.stream_blocks.remove(&key) else {
        return;
    };
    match block.kind {
        BlockKind::Tool => {
            if block.input_json.is_empty() {
                return;
            }
            match serde_json::from_str::<Value>(&block.input_json) {
                Ok(input) => {
                    let fingerprint = value_fingerprint(&input);
                    if block.input_fingerprint == Some(fingerprint) {
                        return;
                    }
                    if let Some(open) = session.open_items.get_mut(&block.item) {
                        open.input = input.clone();
                    }
                    events.push(tool_input_update(
                        block.item,
                        block.tool_name.as_deref().unwrap_or_default(),
                        &input,
                    ));
                }
                // A truncated document is a diagnostic: the tool row keeps the input the
                // snapshot backfills instead.
                Err(_) => tracing::warn!(
                    target: "fleet::agents::claude",
                    "a Claude tool input document did not parse; keeping the snapshot's input"
                ),
            }
        }
        // Invariant 4: the text item closes here at the latest, before the next block opens.
        // Normally the `assistant` snapshot has already closed it, and `complete_item` is a
        // no-op the second time.
        BlockKind::Text | BlockKind::Thinking => {
            if !block.completed {
                let stream = if block.kind == BlockKind::Thinking {
                    StreamKind::ReasoningSummary { part: 0 }
                } else {
                    StreamKind::AssistantText
                };
                if !block.text.is_empty() {
                    events.push(AgentEvent::ItemUpdated {
                        item: block.item,
                        patch: text_replacement(stream, &block.text),
                    });
                }
                session.complete_item(block.item, ItemStatus::Completed, events);
            }
        }
        BlockKind::Unknown => {}
    }
}

/// The `ItemUpdated` a re-parsed tool input produces.
fn tool_input_update(item: ItemId, name: &str, input: &Value) -> AgentEvent {
    AgentEvent::ItemUpdated {
        item,
        patch: ItemPatch {
            payload: Some(ItemPayloadPatch::Tool(Box::new(ToolPatch {
                input: Some(input.clone()),
                summary: Some(tool_summary(name, input, None)),
                ..ToolPatch::default()
            }))),
            status: None,
        },
    }
}

/// The `(owner, index)` a stream frame addresses.
fn block_key(frame: &StreamFrame) -> BlockKey {
    (
        frame.parent_tool_use_id.clone(),
        frame
            .event
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    )
}
