//! Items: the transcript, and the kind column Codex computes for Fleet.
//!
//! **The completed item *is* the tool result** — there is no separate result frame — and
//! `turn/completed.turn.items` is a *summary*, so the transcript is accumulated here and only
//! here.
//!
//! **The renderable command is `commandActions[]`, never `command`.** The raw field is
//! `/usr/bin/bash -lc "sed -n '1,120p' note.txt"`; the same item carries
//! `[{type:"read", command:"sed -n '1,120p' note.txt", name:"note.txt", path:"…"}]`. Codex
//! classifies its own shell commands and that array **is** the kind column, so rendering from
//! `command` shows a `bash -lc` wrapper where Claude shows a clean `Read`.

mod payload;

use fleet_core::agents::{
    AgentEvent, GateKind, ItemPatch, ItemPayloadPatch, ItemStatus, ProviderOptionId, Question,
    QuestionOption, StreamKind, ToolPatch, TurnId,
};
use serde_json::{Value, json};

use self::payload::{changes_diff, item_id_of, item_patch, item_payload, item_status, plan_steps};
use super::{MapOutput, decode, item_type};
use crate::agents::codex::{
    session::{ApprovalShape, CodexSession, PendingApproval, async_gate_id},
    wire::{
        AgentMessageDeltaNotification, CommandExecutionOutputDeltaNotification,
        FileChangePatchUpdatedNotification, ItemCompletedNotification, ItemStartedNotification,
        McpToolCallProgressNotification, PlanDeltaNotification,
        ReasoningSummaryPartAddedNotification, ReasoningSummaryTextDeltaNotification,
        ReasoningTextDeltaNotification, TerminalInteractionNotification, ThreadItem,
    },
};

/// `item/started`.
pub(in crate::agents::codex) fn started(session: &mut CodexSession, params: &Value) -> MapOutput {
    let kind = item_type(params);
    let notification: ItemStartedNotification = match decode("item/started", params) {
        Ok(decoded) => decoded,
        Err(_) => {
            // Named by its own item type, so the UI can say what it did not understand.
            return MapOutput::one(AgentEvent::Unknown {
                method: format!("item/started:{kind}"),
            });
        }
    };
    let turn = session.turn_for(&notification.turn_id);
    if session.root.as_deref() != Some(notification.thread_id.as_str()) {
        return subagent_item(session, &notification.item);
    }
    let provider_item = item_id_of(&notification.item);
    let item = session.item_for(&notification.thread_id, &provider_item);
    // The user's own message is echoed back: reconcile against the optimistic row rather than
    // appending a duplicate of the user's bubble.
    if let ThreadItem::UserMessage { client_id, .. } = &notification.item
        && let Some(client_id) = client_id
        && session.item_for_client(client_id).is_some()
    {
        return MapOutput::default();
    }
    let Some(payload) = item_payload(&notification.item) else {
        return MapOutput::one(AgentEvent::Unknown {
            method: format!("item/started:{kind}"),
        });
    };
    session.note_open_item(item, turn);
    MapOutput::one(AgentEvent::ItemStarted {
        turn,
        item,
        kind: payload,
        parent: None,
    })
}

/// `item/completed`: the item **is** the result.
pub(in crate::agents::codex) fn completed(session: &mut CodexSession, params: &Value) -> MapOutput {
    let kind = item_type(params);
    let notification: ItemCompletedNotification = match decode("item/completed", params) {
        Ok(decoded) => decoded,
        Err(_) => {
            return MapOutput::one(AgentEvent::Unknown {
                method: format!("item/completed:{kind}"),
            });
        }
    };
    let turn = session.turn_for(&notification.turn_id);
    if session.root.as_deref() != Some(notification.thread_id.as_str()) {
        return subagent_item(session, &notification.item);
    }
    let provider_item = item_id_of(&notification.item);
    let item = session.item_for(&notification.thread_id, &provider_item);
    let mut events = Vec::new();

    // Three item kinds are decision surfaces or boundaries rather than rows.
    match &notification.item {
        ThreadItem::Plan { text, .. } => {
            let steps = plan_steps(text);
            events.push(AgentEvent::PlanProposed {
                gate: async_gate_id(&notification.thread_id, &provider_item),
                turn,
                markdown: text.clone(),
                steps,
            });
            return MapOutput::from(events);
        }
        ThreadItem::ContextCompaction { .. } => {
            return MapOutput::one(AgentEvent::Compacted(
                fleet_core::agents::CheckpointKind::CompactBoundary {
                    before: session.context_window.unwrap_or_default(),
                    after: None,
                },
            ));
        }
        ThreadItem::SubAgentActivity { .. } => {
            return subagent_item(session, &notification.item);
        }
        _ => {}
    }

    // An async `agentMessage` carrying questions is a second, distinct question channel: **no
    // JSON-RPC request is pending and the turn does not end**, so the gate is synthesised with a
    // deterministic id and the answer is delivered as a new message.
    if async_questions(
        session,
        params,
        &notification.thread_id,
        &provider_item,
        turn,
        &mut events,
    )
    .is_some()
    {
        return MapOutput::from(events);
    }

    // A completed item Fleet never saw start still becomes a row, and the row it becomes
    // carries the whole completed payload rather than a patch against nothing.
    if session.open_items.contains_key(&item) {
        if let Some(patch) = item_patch(&notification.item) {
            events.push(AgentEvent::ItemUpdated { item, patch });
        }
    } else if let Some(payload) = item_payload(&notification.item) {
        session.note_open_item(item, turn);
        events.push(AgentEvent::ItemStarted {
            turn,
            item,
            kind: payload,
            parent: None,
        });
    }
    session.close_item(item, item_status(&notification.item), &mut events);
    MapOutput::from(events)
}

/// `item/agentMessage/delta`.
pub(in crate::agents::codex) fn agent_message_delta(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: AgentMessageDeltaNotification =
        match decode("item/agentMessage/delta", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    delta(
        session,
        &notification.thread_id,
        &notification.item_id,
        StreamKind::AssistantText,
        notification.delta,
    )
}

/// `item/reasoning/summaryTextDelta`, keyed by `summaryIndex`.
pub(in crate::agents::codex) fn reasoning_summary_delta(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: ReasoningSummaryTextDeltaNotification =
        match decode("item/reasoning/summaryTextDelta", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    let part = u32::try_from(notification.summary_index).unwrap_or_default();
    delta(
        session,
        &notification.thread_id,
        &notification.item_id,
        StreamKind::ReasoningSummary { part },
        notification.delta,
    )
}

/// `item/reasoning/summaryPartAdded`: a **paragraph boundary**, not more text.
///
/// Concatenating across it produces one run-on wall of text, which is why the part index is
/// load-bearing. The boundary itself needs no event: the deltas that follow carry the new index,
/// and the projection keys reasoning text by part.
pub(in crate::agents::codex) fn reasoning_part_added(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: ReasoningSummaryPartAddedNotification =
        match decode("item/reasoning/summaryPartAdded", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    if let Some(item) = session.items.get(&notification.item_id).copied() {
        session.reasoning_parts.insert(
            item,
            u32::try_from(notification.summary_index).unwrap_or_default(),
        );
    }
    MapOutput::default()
}

/// `item/reasoning/textDelta`: the raw chain-of-thought channel, keyed by `contentIndex`.
pub(in crate::agents::codex) fn reasoning_text_delta(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: ReasoningTextDeltaNotification =
        match decode("item/reasoning/textDelta", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    let part = u32::try_from(notification.content_index).unwrap_or_default();
    delta(
        session,
        &notification.thread_id,
        &notification.item_id,
        StreamKind::ReasoningRaw { part },
        notification.delta,
    )
}

/// `item/plan/delta`.
///
/// Clients must not assume the concatenated deltas match the completed plan item: the completed
/// item is authoritative, and this is a live preview.
pub(in crate::agents::codex) fn plan_delta(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: PlanDeltaNotification = match decode("item/plan/delta", params) {
        Ok(decoded) => decoded,
        Err(degraded) => return degraded,
    };
    delta(
        session,
        &notification.thread_id,
        &notification.item_id,
        StreamKind::PlanText,
        notification.delta,
    )
}

/// `item/commandExecution/outputDelta`: **plain text**, stdout and stderr interleaved.
///
/// Not to be confused with `command/exec/outputDelta`, which is base64, stream-tagged, and
/// belongs to the client-initiated family Fleet does not use.
pub(in crate::agents::codex) fn command_output_delta(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: CommandExecutionOutputDeltaNotification =
        match decode("item/commandExecution/outputDelta", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    delta(
        session,
        &notification.thread_id,
        &notification.item_id,
        StreamKind::CommandOutput,
        notification.delta,
    )
}

/// `item/commandExecution/terminalInteraction`: something was typed into the agent's PTY.
pub(in crate::agents::codex) fn terminal_interaction(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: TerminalInteractionNotification =
        match decode("item/commandExecution/terminalInteraction", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    delta(
        session,
        &notification.thread_id,
        &notification.item_id,
        StreamKind::CommandOutput,
        notification.stdin,
    )
}

/// `item/fileChange/patchUpdated`: a live patch preview for an in-flight `fileChange` item.
pub(in crate::agents::codex) fn patch_updated(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: FileChangePatchUpdatedNotification =
        match decode("item/fileChange/patchUpdated", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    let Some(item) = session
        .items
        .get(&notification.item_id)
        .copied()
        .or_else(|| Some(session.item_for(&notification.thread_id, &notification.item_id)))
    else {
        return MapOutput::default();
    };
    let diff = changes_diff(&notification.changes);
    MapOutput::one(AgentEvent::ItemUpdated {
        item,
        patch: ItemPatch {
            payload: Some(ItemPayloadPatch::Tool(Box::new(ToolPatch {
                diff,
                ..ToolPatch::default()
            }))),
            status: None,
        },
    })
}

/// `item/mcpToolCall/progress`: a free-text progress line.
pub(in crate::agents::codex) fn mcp_progress(
    session: &mut CodexSession,
    params: &Value,
) -> MapOutput {
    let notification: McpToolCallProgressNotification =
        match decode("item/mcpToolCall/progress", params) {
            Ok(decoded) => decoded,
            Err(degraded) => return degraded,
        };
    let Some(item) = session.items.get(&notification.item_id).copied() else {
        return MapOutput::default();
    };
    MapOutput::one(AgentEvent::ItemUpdated {
        item,
        patch: ItemPatch {
            payload: Some(ItemPayloadPatch::Tool(Box::new(ToolPatch {
                result: Some(json!(notification.message)),
                ..ToolPatch::default()
            }))),
            status: Some(ItemStatus::InProgress),
        },
    })
}

/// `item/autoApprovalReview/{started,completed}`.
///
/// This is Codex's own reviewer acting where a human would. The transcript **must** show it, or
/// an approval resolves itself and the user never learns why.
pub(in crate::agents::codex) fn auto_review(params: &Value, started: bool) -> MapOutput {
    let action = params
        .pointer("/action/type")
        .or_else(|| params.get("action"))
        .and_then(Value::as_str)
        .unwrap_or("a request");
    let notice = if started {
        format!("Codex is reviewing {action} on your behalf.")
    } else {
        format!("Codex's reviewer answered {action}.")
    };
    MapOutput::one(AgentEvent::Notice(notice))
}

/// One `ContentDelta` for a known item, or nothing when the item is unknown.
fn delta(
    session: &mut CodexSession,
    thread: &str,
    provider_item: &str,
    stream: StreamKind,
    text: String,
) -> MapOutput {
    if text.is_empty() {
        return MapOutput::default();
    }
    if session.root.as_deref() != Some(thread) {
        // A child's deltas are chatter: they never enter the parent's transcript.
        return MapOutput::default();
    }
    let item = session.item_for(thread, provider_item);
    MapOutput::one(AgentEvent::ContentDelta {
        item,
        stream,
        delta: text,
    })
}

/// A subagent item: register the child and record its activity.
fn subagent_item(session: &mut CodexSession, item: &ThreadItem) -> MapOutput {
    if let ThreadItem::SubAgentActivity {
        agent_thread_id,
        agent_path,
        ..
    } = item
    {
        let root = session.root.clone().unwrap_or_default();
        session
            .subagents
            .register(&root, agent_thread_id, Some(agent_path.as_str()));
    }
    // A child's own rows render on the subagent surface, which the parent's transcript does not
    // carry; the registration above is what makes Stop reach it.
    MapOutput::default()
}

/// One question of an async `agentMessage`.
///
/// Read from the raw params rather than from a generated type: `delivery` and `questions` are not
/// in `codex-cli 0.147.0`'s `agentMessage` schema at all, and a channel the schema does not
/// declare is exactly the drift tolerant decoding exists for.
#[derive(Debug, Clone, serde::Deserialize)]
struct AsyncQuestion {
    title: String,
    #[serde(default)]
    options: Option<Vec<String>>,
}

/// The async question channel: an `agentMessage` whose delivery is async and which carries
/// questions.
fn async_questions(
    session: &mut CodexSession,
    params: &Value,
    thread_id: &str,
    provider_item: &str,
    turn: TurnId,
    events: &mut Vec<AgentEvent>,
) -> Option<()> {
    let raw = params.pointer("/item/questions")?;
    let questions: Vec<AsyncQuestion> = serde_json::from_value(raw.clone()).ok()?;
    if questions.is_empty() {
        return None;
    }
    let gate = async_gate_id(thread_id, provider_item);
    let titles = questions
        .iter()
        .map(|question| question.title.clone())
        .collect::<Vec<_>>();
    let normalized = questions
        .iter()
        .map(|question| Question {
            id: question.title.clone(),
            header: question.title.clone(),
            prompt: question.title.clone(),
            options: question
                .options
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|label| QuestionOption {
                    id: ProviderOptionId(label.clone()),
                    label,
                    description: String::new(),
                })
                .collect(),
            multi_select: false,
            allows_other: true,
            is_secret: false,
            // The turn proceeds past an async question, so the thread must **not** be marked
            // "needs you" and the card must stay answerable after settlement.
            blocking: false,
        })
        .collect::<Vec<_>>();
    session.open_gate(
        gate,
        PendingApproval {
            request_id: Value::Null,
            thread: thread_id.to_owned(),
            item: Some(provider_item.to_owned()),
            shape: ApprovalShape::AsyncQuestions { titles },
            decisions: Vec::new(),
        },
    );
    events.push(AgentEvent::GateOpened {
        gate,
        turn: Some(turn),
        kind: GateKind::Question {
            questions: normalized,
        },
    });
    Some(())
}

#[cfg(test)]
mod tests {
    use fleet_core::agents::ToolKind;

    use self::payload::{action_kind, action_name, action_summary};
    use super::*;
    use crate::agents::codex::wire::{CommandAction, CommandExecutionStatus};

    #[test]
    fn the_kind_column_comes_from_command_actions_and_never_from_the_raw_command() {
        // The real capture: an unrenderable raw command with a clean action parse beside it.
        let command = r#"/usr/bin/bash -lc "sed -n '1,120p' note.txt""#;
        let actions = vec![CommandAction::Read {
            command: "sed -n '1,120p' note.txt".to_owned(),
            name: "note.txt".to_owned(),
            path: "/w/note.txt".to_owned(),
        }];
        assert_eq!(action_kind(&actions), ToolKind::Read);
        assert_eq!(action_summary(&actions, command), "note.txt");
        assert_eq!(action_name(&actions), "read");

        // With no actions at all the row is a shell row showing the command line.
        assert_eq!(action_kind(&[]), ToolKind::Bash);
        assert_eq!(action_summary(&[], command), command);
    }

    #[test]
    fn a_piped_command_summarises_on_its_first_meaningful_action() {
        let actions = vec![
            CommandAction::Unrecognized,
            CommandAction::Search {
                command: "rg todo".to_owned(),
                path: Some("src".to_owned()),
                query: Some("todo".to_owned()),
            },
        ];
        assert_eq!(action_kind(&actions), ToolKind::Grep);
        assert_eq!(action_summary(&actions, ""), "todo in src");
    }

    #[test]
    fn declined_is_denied_and_not_failed() {
        let declined = ThreadItem::CommandExecution {
            aggregated_output: None,
            command: "rm -rf /".to_owned(),
            command_actions: Vec::new(),
            cwd: "/w".to_owned(),
            duration_ms: None,
            exit_code: None,
            id: "exec-1".to_owned(),
            plugin_id: None,
            process_id: None,
            script_path: None,
            source: None,
            status: CommandExecutionStatus::Declined,
        };
        assert_eq!(item_status(&declined), ItemStatus::Denied);
    }

    #[test]
    fn an_unknown_item_variant_is_named_rather_than_lost() {
        let params = serde_json::json!({
            "threadId": "t",
            "turnId": "01a089f2-579b-75c2-8e51-3492ec617046",
            "startedAtMs": 1,
            "item": {"type": "holograph", "id": "x", "secret": "sk-live-DEADBEEF"},
        });
        let mut session = CodexSession {
            root: Some("t".to_owned()),
            ..CodexSession::default()
        };
        let output = started(&mut session, &params);
        match output.events.first() {
            Some(AgentEvent::Unknown { method }) => {
                assert_eq!(method, "item/started:holograph");
                assert!(!method.contains("sk-live"));
            }
            other => panic!("expected a named unknown item, got {other:?}"),
        }
    }
}
