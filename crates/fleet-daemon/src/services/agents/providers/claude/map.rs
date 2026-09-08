//! Claude Code wire-to-domain normalization.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
};

use fleet_core::agents::{
    AbortReason, AgentEvent, AgentKind, CheckpointKind, FileDelta, GateAnswer, GateId, GateKind,
    GateResolver, ItemId, ItemKind, ItemPatch, ItemStatus, ModelSelection, PermissionChoice,
    PermissionMode, PermissionOption, PlanAnswer, ProviderOptionId, Question, QuestionOption,
    SessionState, StreamKind, ToolDiff, ToolKind, TurnId, TurnOutcome, Usage,
};
use serde_json::{Value, json};

use super::{ProviderError, wire::*};

#[derive(Debug, Default)]
pub(super) struct MapOutput {
    pub events: Vec<AgentEvent>,
    pub writes: Vec<Value>,
    /// The CLI's own name for the frame these events were mapped from (§11).
    pub raw: Option<String>,
}

#[derive(Debug, Clone)]
struct OpenItem {
    /// Turn the item was started in; only that turn's terminal may close it.
    turn: TurnId,
    status: ItemStatus,
    tool_name: Option<String>,
    input: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Thinking,
    Tool,
    Unknown,
}

/// A streaming content block is addressed by its owner plus its index: a subagent stream has
/// its own index space starting at zero, so the index alone collides with the main stream.
type BlockKey = (Option<String>, u64);

#[derive(Debug, Clone)]
struct StreamBlock {
    item: ItemId,
    kind: BlockKind,
    tool_name: Option<String>,
    input_json: String,
    text: String,
    completed: bool,
}

#[derive(Debug, Clone)]
struct PendingGate {
    request_id: String,
    tool_use_id: Option<String>,
    input: Value,
    suggestions: Vec<Value>,
    shape: GateShape,
}

#[derive(Debug, Clone)]
enum GateShape {
    Permission,
    Question(Vec<WireQuestion>),
    Plan,
}

#[derive(Debug, Clone)]
struct WireQuestion {
    text: String,
    multi_select: bool,
}

#[derive(Debug, Default)]
pub(super) struct ClaudeMapper {
    initialized: bool,
    active_turn: Option<TurnId>,
    last_turn: Option<TurnId>,
    interrupted: HashSet<TurnId>,
    /// Steer writes issued into the active turn that the CLI has not settled yet.
    pending_steers: u32,
    stream_blocks: HashMap<BlockKey, StreamBlock>,
    current_message: Option<String>,
    open_items: HashMap<ItemId, OpenItem>,
    tool_items: HashMap<String, ItemId>,
    task_items: HashMap<String, ItemId>,
    background_tasks: HashSet<String>,
    gates: HashMap<GateId, PendingGate>,
    files_changed: BTreeMap<PathBuf, (u64, u64)>,
    tool_uses: u64,
    /// The last non-zero cumulative usage this process reported.
    ///
    /// §4.1 makes Claude's counters cumulative for the process — "take the latest, never sum" —
    /// and an aborted `result` reports them as all zeros. §11 still renders the interrupted turn
    /// as `stopped · 12s · 3.1k tokens`, so the zeros are a gap in the frame, not a cheaper turn.
    last_usage: Option<Usage>,
    /// The last measured resident context, in tokens, for a frame that measured none.
    last_context_tokens: Option<u64>,
    /// The last completed assistant paragraph, which is the plan when `ExitPlanMode` omits it.
    last_assistant_text: Option<String>,
    /// The model `system/init` named for this session, updated by `set_model`.
    ///
    /// `modelUsage` is cumulative across the query process and includes pipeline subcalls, so
    /// picking an arbitrary entry from it reports a summariser's window as §2's `context 34%`.
    session_model: Option<String>,
}

impl ClaudeMapper {
    pub fn active_turn(&self) -> Option<TurnId> {
        self.active_turn
    }

    pub fn begin_turn(&mut self, turn: TurnId) -> Result<Option<AgentEvent>, ProviderError> {
        match self.active_turn {
            Some(active) if active != turn => Err(ProviderError::Protocol {
                message: format!("turn {turn} was sent while turn {active} is active"),
            }),
            // A second `user` line while the turn runs is steering (§4.1): the CLI coalesces it
            // into the same turn, so no new turn starts here.
            Some(_) => Ok(None),
            None => {
                self.active_turn = Some(turn);
                self.last_turn = Some(turn);
                Ok(Some(AgentEvent::TurnStarted {
                    turn,
                    user_item: ItemId::new(),
                }))
            }
        }
    }

    /// Records the model the session switched to, so §2's context bar keeps measuring the right
    /// window: `modelUsage` is keyed by model id and carries every pipeline subcall beside it.
    pub fn set_model(&mut self, model: &str) {
        self.session_model = Some(model.to_owned());
    }

    pub fn rollback_turn_start(&mut self, turn: TurnId) {
        if self.active_turn == Some(turn) {
            self.active_turn = None;
            self.pending_steers = 0;
        }
    }

    /// Records a steer write the CLI has accepted but not yet answered.
    ///
    /// The CLI supersedes the in-flight stream when a steer arrives and emits a `result` with an
    /// aborted terminal reason for it. That result is not the turn's terminal: §3.3 rule 6 says
    /// the turn settles only after the last in-flight send.
    pub fn record_steer(&mut self, turn: TurnId) {
        if self.active_turn == Some(turn) {
            self.pending_steers = self.pending_steers.saturating_add(1);
        }
    }

    pub fn mark_interrupted(&mut self, turn: TurnId) -> Result<(), ProviderError> {
        if self.active_turn != Some(turn) {
            return Err(ProviderError::Protocol {
                message: format!("cannot interrupt inactive turn {turn}"),
            });
        }
        self.interrupted.insert(turn);
        Ok(())
    }

    /// Settles the mapper for a process that is gone.
    ///
    /// The turn is cleared here, not just reported: the mapper outlives the child (the adapter
    /// owns it across a restart), and a stale `active_turn` would make [`Self::begin_turn`]
    /// reject every later send with "turn … is active" for a process that can never answer.
    pub fn process_exit(&mut self, code: Option<i32>, expected: bool) -> Vec<AgentEvent> {
        let mut events = vec![AgentEvent::SessionExited { code, expected }];
        if !expected && self.active_turn.is_some() {
            events.push(AgentEvent::RuntimeError {
                fatal: true,
                message: "Claude Code exited before returning a result for the active turn"
                    .to_owned(),
            });
        }
        self.active_turn = None;
        self.pending_steers = 0;
        events
    }

    pub fn handle(&mut self, message: ClaudeMessage) -> Result<MapOutput, ProviderError> {
        let raw = message.wire_type();
        let mut output = self.dispatch(message)?;
        output.raw = Some(raw);
        Ok(output)
    }

    fn dispatch(&mut self, message: ClaudeMessage) -> Result<MapOutput, ProviderError> {
        match message {
            ClaudeMessage::System(message) => self.system(message),
            ClaudeMessage::Assistant(message) => self.assistant(message),
            ClaudeMessage::User(message) => self.user(message),
            ClaudeMessage::Stream(message) => self.stream(message),
            ClaudeMessage::Result(message) => self.result(message),
            ClaudeMessage::ToolProgress(message) => self.tool_progress(message),
            ClaudeMessage::ControlRequest(message) => self.control_request(message),
            ClaudeMessage::ControlResponse(message) => {
                if message.response.get("subtype").and_then(Value::as_str) == Some("error") {
                    Ok(MapOutput {
                        events: vec![AgentEvent::Notice(format!(
                            "Claude rejected a control request: {}",
                            compact_json(&message.response)
                        ))],
                        ..MapOutput::default()
                    })
                } else {
                    Ok(MapOutput::default())
                }
            }
            ClaudeMessage::ControlCancel(message) => Ok(self.control_cancel(&message.request_id)),
            ClaudeMessage::KeepAlive => Ok(MapOutput::default()),
            ClaudeMessage::Ignored { kind } => {
                tracing::debug!(target: "fleet::agents::claude", kind, "ignoring Claude frame");
                Ok(MapOutput::default())
            }
            // §3.2 reserves `Notice` for what the user has to read; an unrecognised envelope is
            // a diagnostic (§4.3), and routing it to the transcript both hides the real notices
            // and raises the unread dot for output that says nothing.
            ClaudeMessage::Unknown { kind, value } => {
                tracing::warn!(
                    target: "fleet::agents::claude",
                    kind,
                    payload = %compact_json(&value),
                    "unknown Claude message",
                );
                Ok(MapOutput::default())
            }
        }
    }

    pub fn response_for(&self, gate: GateId, answer: &GateAnswer) -> Result<Value, ProviderError> {
        let pending = self
            .gates
            .get(&gate)
            .ok_or_else(|| ProviderError::Protocol {
                message: format!("unknown Claude gate {gate}"),
            })?;

        let response = match (&pending.shape, answer) {
            (
                GateShape::Permission,
                GateAnswer::Permission {
                    choice,
                    edited_payload,
                },
            ) => permission_response(pending, *choice, edited_payload.as_deref())?,
            (GateShape::Question(questions), GateAnswer::Question { answers }) => {
                question_response(pending, questions, answers)?
            }
            (GateShape::Plan, GateAnswer::Plan(answer)) => plan_response(pending, answer),
            _ => {
                return Err(ProviderError::Protocol {
                    message: format!("answer shape does not match Claude gate {gate}"),
                });
            }
        };
        Ok(control_success(&pending.request_id, response))
    }

    pub fn resolve_gate(
        &mut self,
        gate: GateId,
        answer: GateAnswer,
    ) -> Result<AgentEvent, ProviderError> {
        if self.gates.remove(&gate).is_none() {
            return Err(ProviderError::Protocol {
                message: format!("unknown Claude gate {gate}"),
            });
        }
        if matches!(
            answer,
            GateAnswer::Permission {
                choice: PermissionChoice::DenyAndStop,
                ..
            }
        ) && let Some(turn) = self.active_turn
        {
            self.interrupted.insert(turn);
        }
        Ok(AgentEvent::GateResolved {
            gate,
            answer,
            by: GateResolver::User,
        })
    }

    fn system(&mut self, message: SystemMessage) -> Result<MapOutput, ProviderError> {
        let mut output = MapOutput::default();
        match message.subtype.as_str() {
            "init" => {
                if !self.initialized {
                    let model =
                        string_field(&message.fields, "model").map(|model| ModelSelection {
                            model,
                            effort: string_field(&message.fields, "effort"),
                            provider: None,
                        });
                    self.session_model = model.as_ref().map(|model| model.model.clone());
                    let mode = string_field(&message.fields, "permissionMode")
                        .as_deref()
                        .map(permission_mode_from_wire)
                        .unwrap_or_default();
                    output.events.push(AgentEvent::SessionStarted {
                        provider: AgentKind::Claude,
                        resume_cursor: string_field(&message.fields, "session_id"),
                        model,
                        mode,
                        tools: string_list(message.fields.get("tools")),
                        commands: string_list(message.fields.get("slash_commands")),
                        skills: string_list(message.fields.get("skills")),
                    });
                    output
                        .events
                        .push(AgentEvent::SessionStateChanged(SessionState::Ready));
                    self.initialized = true;
                }
            }
            "session_state_changed" => {
                let state = match string_field(&message.fields, "state").as_deref() {
                    Some("idle") => SessionState::Ready,
                    Some("running" | "requires_action") => SessionState::Running,
                    Some(other) => {
                        tracing::warn!(
                            target: "fleet::agents::claude",
                            state = other,
                            "unknown Claude session state"
                        );
                        return Ok(output);
                    }
                    None => return Ok(output),
                };
                output.events.push(AgentEvent::SessionStateChanged(state));
            }
            "compact_boundary" => {
                let metadata = message.fields.get("compact_metadata");
                let before = metadata
                    .and_then(|value| value.get("pre_tokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                let after = metadata
                    .and_then(|value| value.get("post_tokens"))
                    .and_then(Value::as_u64);
                output
                    .events
                    .push(AgentEvent::Checkpoint(CheckpointKind::CompactBoundary {
                        before,
                        after,
                    }));
            }
            "api_retry" => output.events.push(AgentEvent::Retrying {
                attempt: u32_field(&message.fields, "attempt"),
                retry_in_ms: u64_field(&message.fields, "retry_delay_ms"),
                reason: string_field(&message.fields, "error")
                    .unwrap_or_else(|| "Claude API request failed".to_owned()),
            }),
            "permission_denied" => self.permission_denied(&message, &mut output.events),
            "task_started" => self.task_started(&message, &mut output.events),
            "task_updated" => self.task_updated(&message, &mut output.events),
            "task_progress" => self.task_progress_system(&message, &mut output.events),
            "task_notification" => self.task_notification(&message, &mut output.events),
            "background_tasks_changed" => {
                self.background_tasks_changed(&message, &mut output.events)
            }
            "informational" => {
                if let Some(content) = string_field(&message.fields, "content")
                    && let Some(notice) = notice_text(&content)
                {
                    output.events.push(AgentEvent::Notice(notice));
                }
            }
            "notification" => {
                if let Some(text) = string_field(&message.fields, "text")
                    && let Some(notice) = notice_text(&text)
                {
                    output.events.push(AgentEvent::Notice(notice));
                }
            }
            "status"
            | "hook_started"
            | "hook_progress"
            | "hook_response"
            | "control_request_progress"
            | "thinking_tokens"
            | "commands_changed"
            | "worker_shutting_down"
            | "elicitation_complete"
            | "files_persisted"
            | "plugin_install"
            | "local_command_output"
            | "memory_recall" => {}
            other => tracing::warn!(
                target: "fleet::agents::claude",
                subtype = other,
                "unknown Claude system message"
            ),
        }
        Ok(output)
    }

    fn assistant(&mut self, message: AssistantMessage) -> Result<MapOutput, ProviderError> {
        let Some(turn) = self.active_turn else {
            tracing::warn!(
                target: "fleet::agents::claude",
                "Claude assistant output arrived without an active turn"
            );
            return Ok(MapOutput::default());
        };
        self.current_message = message.message.id;
        let owner = message.parent_tool_use_id.clone();
        let parent = owner
            .as_deref()
            .and_then(|id| self.tool_items.get(id).copied());
        let mut events = Vec::new();
        if let Some(error) = message.error {
            events.push(AgentEvent::RuntimeError {
                fatal: false,
                message: error,
            });
        }
        let Some(blocks) = message.message.content.as_array() else {
            return Ok(MapOutput {
                events,
                ..MapOutput::default()
            });
        };
        for block in blocks.iter().cloned() {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    self.complete_streamed_text(
                        turn,
                        owner.as_deref(),
                        BlockKind::Text,
                        StreamKind::AssistantText,
                        ItemKind::AssistantText,
                        text,
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
                    self.complete_streamed_text(
                        turn,
                        owner.as_deref(),
                        BlockKind::Thinking,
                        StreamKind::Reasoning,
                        ItemKind::Thinking,
                        text,
                        parent,
                        &mut events,
                    );
                }
                Some("tool_use") => self.assistant_tool(turn, block, parent, &mut events),
                Some(other) => tracing::warn!(
                    target: "fleet::agents::claude",
                    block = other,
                    "unknown Claude assistant content block"
                ),
                None => {}
            }
        }
        Ok(MapOutput {
            events,
            ..MapOutput::default()
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn complete_streamed_text(
        &mut self,
        turn: TurnId,
        owner: Option<&str>,
        block_kind: BlockKind,
        stream: StreamKind,
        item_kind: ItemKind,
        text: &str,
        parent: Option<ItemId>,
        events: &mut Vec<AgentEvent>,
    ) {
        // Frames arrive in block order *within one owner*, so the oldest open block of this kind
        // belonging to the same stream is the one this frame completes; a subagent's blocks are
        // never matched against the main agent's, which share their index space.
        let streamed = self
            .stream_blocks
            .iter()
            .filter(|((block_owner, _), block)| {
                block_owner.as_deref() == owner && block.kind == block_kind && !block.completed
            })
            .map(|(key, _)| key.clone())
            .min()
            .and_then(|key| self.stream_blocks.get_mut(&key));
        if block_kind == BlockKind::Text && owner.is_none() && !text.trim().is_empty() {
            self.last_assistant_text = Some(text.to_owned());
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
                    patch: ItemPatch {
                        text: Some(text.to_owned()),
                        ..ItemPatch::default()
                    },
                });
            }
            block.item
        } else {
            let item = self.start_item(turn, item_kind, parent, None, Value::Null, events);
            if !text.is_empty() {
                events.push(AgentEvent::ContentDelta {
                    item,
                    stream,
                    delta: text.to_owned(),
                });
            }
            item
        };
        self.complete_item(item, ItemStatus::Done, events);
    }

    fn assistant_tool(
        &mut self,
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
        if let Some(item) = self.tool_items.get(&provider_id).copied() {
            if let Some(open) = self.open_items.get_mut(&item) {
                open.input = input.clone();
                open.tool_name = Some(name.clone());
            }
            events.push(AgentEvent::ItemUpdated {
                item,
                patch: ItemPatch {
                    input: Some(input.clone()),
                    summary: Some(tool_summary(&name, &input, None)),
                    status: Some(ItemStatus::Running),
                    ..ItemPatch::default()
                },
            });
        } else {
            let item = self.start_tool(turn, &provider_id, &name, input, parent, events);
            self.tool_items.insert(provider_id, item);
        }
    }

    fn user(&mut self, message: UserMessage) -> Result<MapOutput, ProviderError> {
        let Some(blocks) = message.message.content.as_array() else {
            return Ok(MapOutput::default());
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
            let Some(item) = self.tool_items.get(provider_id).copied() else {
                tracing::warn!(
                    target: "fleet::agents::claude",
                    tool_use_id = provider_id,
                    "Claude returned a result for an unknown tool"
                );
                continue;
            };
            let is_error = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let output = content_text(block.get("content"));
            let open = self.open_items.get(&item).cloned();
            let diff = open.as_ref().and_then(|open| {
                derive_tool_diff(
                    open.tool_name.as_deref().unwrap_or_default(),
                    &open.input,
                    message.tool_use_result.as_ref(),
                )
            });
            if let Some(diff) = &diff {
                let entry = self.files_changed.entry(diff.path.clone()).or_default();
                entry.0 = entry.0.saturating_add(diff.added);
                entry.1 = entry.1.saturating_add(diff.removed);
            }
            events.push(AgentEvent::ItemUpdated {
                item,
                patch: ItemPatch {
                    summary: open.as_ref().and_then(|open| {
                        diff.as_ref().map(|diff| {
                            tool_summary(
                                open.tool_name.as_deref().unwrap_or_default(),
                                &open.input,
                                Some(diff),
                            )
                        })
                    }),
                    result: Some(if is_error { "error" } else { "done" }.to_owned()),
                    output: (!output.is_empty()).then_some(output),
                    diff,
                    status: Some(if is_error {
                        ItemStatus::Error
                    } else {
                        ItemStatus::Done
                    }),
                    ..ItemPatch::default()
                },
            });
            self.complete_item(
                item,
                if is_error {
                    ItemStatus::Error
                } else {
                    ItemStatus::Done
                },
                &mut events,
            );
        }
        Ok(MapOutput {
            events,
            ..MapOutput::default()
        })
    }

    fn stream(&mut self, message: StreamMessage) -> Result<MapOutput, ProviderError> {
        let event_type = message
            .event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let mut events = Vec::new();
        match event_type {
            "message_start" => {
                self.current_message = message
                    .event
                    .get("message")
                    .and_then(|message| message.get("id"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
            }
            "content_block_start" => {
                let Some(turn) = self.active_turn else {
                    return Ok(MapOutput::default());
                };
                let key = block_key(&message);
                let content = message
                    .event
                    .get("content_block")
                    .cloned()
                    .unwrap_or(Value::Null);
                let content_type = content
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let parent = message
                    .parent_tool_use_id
                    .as_deref()
                    .and_then(|id| self.tool_items.get(id).copied());
                let (item, kind, tool_name) = match content_type {
                    "text" => (
                        self.start_item(
                            turn,
                            ItemKind::AssistantText,
                            parent,
                            None,
                            Value::Null,
                            &mut events,
                        ),
                        BlockKind::Text,
                        None,
                    ),
                    "thinking" => (
                        self.start_item(
                            turn,
                            ItemKind::Thinking,
                            parent,
                            None,
                            Value::Null,
                            &mut events,
                        ),
                        BlockKind::Thinking,
                        None,
                    ),
                    "tool_use" => {
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
                        let item =
                            self.start_tool(turn, &provider_id, &name, input, parent, &mut events);
                        self.tool_items.insert(provider_id.clone(), item);
                        (item, BlockKind::Tool, Some(name))
                    }
                    other => {
                        tracing::warn!(
                            target: "fleet::agents::claude",
                            block = other,
                            "unknown Claude stream content block"
                        );
                        (ItemId::new(), BlockKind::Unknown, None)
                    }
                };
                self.stream_blocks.insert(
                    key,
                    StreamBlock {
                        item,
                        kind,
                        tool_name,
                        input_json: String::new(),
                        text: String::new(),
                        completed: false,
                    },
                );
            }
            "content_block_delta" => {
                let key = block_key(&message);
                if let Some(block) = self.stream_blocks.get_mut(&key) {
                    let delta = message.event.get("delta").unwrap_or(&Value::Null);
                    match delta.get("type").and_then(Value::as_str) {
                        // An empty delta carries nothing to append: §5's update policy exists to
                        // keep a fast model from scheduling a render per token, and a zero-length
                        // one would cost a sequence, a stored line and a re-render for no text.
                        Some("text_delta") => {
                            let text = delta
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
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
                                    stream: StreamKind::Reasoning,
                                    delta: text.to_owned(),
                                });
                            }
                        }
                        Some("input_json_delta") => block.input_json.push_str(
                            delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        ),
                        Some("signature_delta" | "citations_delta") | None => {}
                        Some(other) => tracing::warn!(
                            target: "fleet::agents::claude",
                            delta = other,
                            "unknown Claude stream delta"
                        ),
                    }
                }
            }
            "content_block_stop" => {
                let key = block_key(&message);
                if let Some(block) = self.stream_blocks.remove(&key)
                    && block.kind == BlockKind::Tool
                    && !block.input_json.is_empty()
                {
                    match serde_json::from_str::<Value>(&block.input_json) {
                        Ok(input) => {
                            if let Some(open) = self.open_items.get_mut(&block.item) {
                                open.input = input.clone();
                            }
                            events.push(AgentEvent::ItemUpdated {
                                item: block.item,
                                patch: ItemPatch {
                                    input: Some(input.clone()),
                                    summary: Some(tool_summary(
                                        block.tool_name.as_deref().unwrap_or_default(),
                                        &input,
                                        None,
                                    )),
                                    ..ItemPatch::default()
                                },
                            });
                        }
                        Err(error) => tracing::warn!(
                            target: "fleet::agents::claude",
                            %error,
                            "Claude returned invalid tool input JSON"
                        ),
                    }
                }
            }
            "message_delta" | "message_stop" => {}
            other => tracing::warn!(
                target: "fleet::agents::claude",
                event = other,
                "unknown Claude stream event"
            ),
        }
        Ok(MapOutput {
            events,
            ..MapOutput::default()
        })
    }

    fn result(&mut self, message: ResultMessage) -> Result<MapOutput, ProviderError> {
        let mut usage = usage_from_value(&message.usage);
        if usage_is_empty(&usage) {
            if let Some(last) = &self.last_usage {
                usage = last.clone();
            }
        } else {
            self.last_usage = Some(usage.clone());
        }
        usage.tool_uses = self.tool_uses;
        // Occupancy is measured per turn; an aborted or empty frame reports nothing rather than
        // an empty window, so the last measured level stands (the same rule the reducer applies
        // to a missing cost).
        let context_tokens = context_tokens(&message.usage).or(self.last_context_tokens);
        if context_tokens.is_some() {
            self.last_context_tokens = context_tokens;
        }
        let context_pct = context_pct(
            context_tokens,
            &message.model_usage,
            self.session_model.as_deref(),
        );
        let Some(turn) = self.active_turn else {
            let events = self
                .last_turn
                .map(|turn| AgentEvent::TokenUsage {
                    turn,
                    usage,
                    context_pct,
                    cost_usd: message.total_cost_usd,
                })
                .into_iter()
                .collect();
            return Ok(MapOutput {
                events,
                ..MapOutput::default()
            });
        };

        let terminal_reason = message.terminal_reason.as_deref();
        let aborted = matches!(terminal_reason, Some("aborted_streaming" | "aborted_tools"));
        let interrupted = self.interrupted.contains(&turn);
        // §4.1 and §3.3 rule 6: a steer supersedes the in-flight stream and the CLI reports that
        // supersede as an aborted `result` while the turn keeps running. The turn settles only
        // after the last in-flight send, so this result carries usage and nothing else.
        let superseded =
            aborted && !interrupted && (message.queued_turn_count > 0 || self.pending_steers > 0);
        tracing::debug!(
            target: "fleet::agents::claude",
            %turn,
            sends = message.user_message_uuids.len(),
            queued_turn_count = message.queued_turn_count,
            pending_steers = self.pending_steers,
            superseded,
            "Claude result",
        );
        if superseded {
            self.pending_steers = self.pending_steers.saturating_sub(1);
            self.stream_blocks.clear();
            self.current_message = None;
            return Ok(MapOutput {
                events: vec![AgentEvent::TokenUsage {
                    turn,
                    usage,
                    context_pct,
                    cost_usd: message.total_cost_usd,
                }],
                ..MapOutput::default()
            });
        }

        self.active_turn = None;
        let mut events = Vec::new();
        // §3 rule 3 closes every open item *in that turn* — not every open item there is. A
        // background task is the one row whose lifetime the turn does not own (§3.3 lists
        // "background tasks alive" as its own `Working` source, and `background_tasks_changed`
        // has replace semantics), so a live one survives its turn's terminal, and a row left
        // open by an earlier turn is not this turn's to settle either.
        let background = self
            .background_tasks
            .iter()
            .filter_map(|task| self.task_items.get(task).copied())
            .collect::<HashSet<_>>();
        // A tool that never returned a `tool_result` did not succeed: §5 keeps a failed row
        // exposed when the turn folds, which a silent `Done` would hide.
        let open = self
            .open_items
            .iter()
            .filter(|(item, open)| open.turn == turn && !background.contains(*item))
            .map(|(item, open)| (*item, open.tool_name.is_some()))
            .collect::<Vec<_>>();
        for (item, is_tool) in open {
            let status = if is_tool {
                ItemStatus::Error
            } else {
                ItemStatus::Done
            };
            self.complete_item(item, status, &mut events);
        }
        // Cost and context share the result frame but not the terminal event, so they are
        // published just before it and the terminal event stays last in the turn.
        events.push(AgentEvent::TokenUsage {
            turn,
            usage: usage.clone(),
            context_pct,
            cost_usd: message.total_cost_usd,
        });
        if aborted {
            let reason = if interrupted {
                AbortReason::User
            } else {
                AbortReason::Other(terminal_reason.unwrap_or("aborted").to_owned())
            };
            events.push(AgentEvent::TurnAborted { turn, reason });
            self.files_changed.clear();
        } else {
            let outcome = turn_outcome(&message);
            let files_changed = std::mem::take(&mut self.files_changed)
                .into_iter()
                .map(|(path, (added, removed))| FileDelta {
                    path,
                    added,
                    removed,
                })
                .collect();
            events.push(AgentEvent::TurnCompleted {
                turn,
                outcome,
                usage,
                duration_ms: message.duration_ms,
                files_changed,
            });
        }
        self.stream_blocks.clear();
        self.current_message = None;
        self.interrupted.remove(&turn);
        self.tool_uses = 0;
        self.pending_steers = 0;
        self.prune_settled_items();
        Ok(MapOutput {
            events,
            ..MapOutput::default()
        })
    }

    /// Drops every item the settled turn closed, so the three id maps stay bounded.
    ///
    /// Anything still open — a background task that outlives its turn — is kept, because its
    /// later `task_notification` still has to find the row it belongs to.
    fn prune_settled_items(&mut self) {
        self.open_items.retain(|_, open| {
            !matches!(
                open.status,
                ItemStatus::Done | ItemStatus::Error | ItemStatus::Denied
            )
        });
        self.tool_items
            .retain(|_, item| self.open_items.contains_key(item));
        self.task_items
            .retain(|_, item| self.open_items.contains_key(item));
    }

    fn tool_progress(&mut self, message: ToolProgressMessage) -> Result<MapOutput, ProviderError> {
        let mut events = Vec::new();
        if let Some(item) = self.tool_items.get(&message.tool_use_id).copied() {
            let suffix = message
                .task_id
                .as_deref()
                .map(|task| format!(" · task {task}"))
                .unwrap_or_default();
            events.push(AgentEvent::ItemUpdated {
                item,
                patch: ItemPatch {
                    result: Some(format!(
                        "{:.1}s{suffix}",
                        message.elapsed_time_seconds.max(0.0)
                    )),
                    status: Some(ItemStatus::Running),
                    ..ItemPatch::default()
                },
            });
        } else {
            tracing::warn!(
                target: "fleet::agents::claude",
                tool_use_id = %message.tool_use_id,
                tool = %message.tool_name,
                "progress for an unknown Claude tool"
            );
        }
        Ok(MapOutput {
            events,
            ..MapOutput::default()
        })
    }

    fn control_request(
        &mut self,
        message: ControlRequestMessage,
    ) -> Result<MapOutput, ProviderError> {
        if message.request.subtype == "can_use_tool" {
            return self.can_use_tool(message);
        }
        let response = match message.request.subtype.as_str() {
            "elicitation" => control_success(&message.request_id, json!({"action": "decline"})),
            "hook_callback" | "mcp_message" => control_error(
                &message.request_id,
                &format!(
                    "Fleet does not support Claude {} callbacks",
                    message.request.subtype
                ),
            ),
            other => control_error(
                &message.request_id,
                &format!("Fleet does not support Claude control request `{other}`"),
            ),
        };
        Ok(MapOutput {
            events: Vec::new(),
            writes: vec![response],
            raw: None,
        })
    }

    /// Closes the gate a cancelled control request opened.
    ///
    /// `control_cancel_request` has no response (harness-protocols.md): the CLI has stopped
    /// waiting, so the card must go with it rather than sit on the highest-priority `NeedsYou`
    /// for the life of the thread.
    fn control_cancel(&mut self, request_id: &str) -> MapOutput {
        let Some((gate, pending)) = self
            .gates
            .iter()
            .find(|(_, pending)| pending.request_id == request_id)
            .map(|(gate, pending)| (*gate, pending.clone()))
        else {
            return MapOutput::default();
        };
        self.gates.remove(&gate);
        let answer = match pending.shape {
            GateShape::Permission => GateAnswer::Permission {
                choice: PermissionChoice::Deny,
                edited_payload: None,
            },
            GateShape::Question(_) => GateAnswer::Question {
                answers: Vec::new(),
            },
            GateShape::Plan => GateAnswer::Plan(PlanAnswer::AskForChanges {
                note: String::new(),
            }),
        };
        MapOutput {
            events: vec![AgentEvent::GateResolved {
                gate,
                answer,
                by: GateResolver::ProviderClosed,
            }],
            ..MapOutput::default()
        }
    }

    /// The plan recorded on the assistant `tool_use` block a control request names.
    fn recorded_plan(&self, tool_use_id: Option<&str>) -> Option<String> {
        let item = self.tool_items.get(tool_use_id?).copied()?;
        let plan = self.open_items.get(&item)?.input.get("plan")?.as_str()?;
        (!plan.trim().is_empty()).then(|| plan.to_owned())
    }

    fn can_use_tool(&mut self, message: ControlRequestMessage) -> Result<MapOutput, ProviderError> {
        let tool_name = message
            .request
            .tool_name
            .clone()
            .unwrap_or_else(|| "Unknown".to_owned());
        let gate = GateId::new();
        let turn = self.active_turn;
        let (kind, shape) = match tool_name.as_str() {
            "AskUserQuestion" => {
                let (questions, wire_questions) = questions_from_input(&message.request.input);
                (
                    GateKind::Question { questions },
                    GateShape::Question(wire_questions),
                )
            }
            "ExitPlanMode" => {
                // harness-protocols.md: the declared input has only the deprecated
                // `allowedPrompts`, and 2.1.263 *may* provide `input.plan`. When it does not,
                // the assistant `tool_use` block this request names is where the plan is, and
                // the last thing the model said is where it is otherwise — a plan card with
                // nothing to read is still pinned at `NeedsYou(Plan)` and cannot be judged.
                let markdown = message
                    .request
                    .input
                    .get("plan")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .or_else(|| self.recorded_plan(message.request.tool_use_id.as_deref()))
                    .or_else(|| self.last_assistant_text.clone())
                    .unwrap_or_default();
                let steps = top_level_steps(&markdown);
                (GateKind::Plan { markdown, steps }, GateShape::Plan)
            }
            _ => {
                let mut options =
                    vec![permission_option("allow_once", PermissionChoice::AllowOnce)];
                // A request the CLI says needs a human cannot be answered by a stored rule
                // either, so the session grant is not offered for one. Neither is it offered
                // with nothing to grant: §4.1 makes `allow for this session` the echo of the
                // suggested `addRules` with `destination: "session"`, so with no suggestion the
                // answer would carry `updatedPermissions: []` — an `allow once` wearing the
                // label of a session grant, which §3.2 forbids the copy from promising.
                if !message.request.suppress_always_allow_rule
                    && !message.request.requires_user_interaction
                    && !session_scoped(&message.request.permission_suggestions).is_empty()
                {
                    options.push(permission_option(
                        "allow_session",
                        PermissionChoice::AllowSession,
                    ));
                }
                options.push(permission_option("deny", PermissionChoice::Deny));
                options.push(permission_option(
                    "deny_and_stop",
                    PermissionChoice::DenyAndStop,
                ));
                if tool_name == "Bash" {
                    options.push(permission_option("edit", PermissionChoice::Edit));
                }
                // §4.1: honour `default_to_no` by leading with the CLI's own safe answer, which
                // is the option the card focuses.
                if message.request.default_to_no
                    && let Some(deny) = options
                        .iter()
                        .position(|option| option.label == PermissionChoice::Deny)
                {
                    let deny = options.remove(deny);
                    options.insert(0, deny);
                }
                let title = message
                    .request
                    .title
                    .clone()
                    .unwrap_or_else(|| format!("Claude wants to use {tool_name}"));
                // §3.2, DESIGN-SYSTEM §6.6: the payload is the protected invocation, read
                // literally and seeded into the composer by `e`. The model's prose `description`
                // is a rationale, never the command the user is approving.
                let payload = permission_payload(&tool_name, &message.request.input);
                let rationale = message
                    .request
                    .decision_reason
                    .as_deref()
                    .map(strip_ansi)
                    .or_else(|| message.request.description.clone());
                (
                    GateKind::Permission {
                        tool: tool_kind(&tool_name),
                        title,
                        payload,
                        rationale,
                        options,
                    },
                    GateShape::Permission,
                )
            }
        };
        self.gates.insert(
            gate,
            PendingGate {
                request_id: message.request_id,
                tool_use_id: message.request.tool_use_id,
                input: message.request.input,
                suggestions: message.request.permission_suggestions,
                shape,
            },
        );
        Ok(MapOutput {
            events: vec![AgentEvent::GateOpened { gate, turn, kind }],
            ..MapOutput::default()
        })
    }

    fn start_tool(
        &mut self,
        turn: TurnId,
        provider_id: &str,
        name: &str,
        input: Value,
        parent: Option<ItemId>,
        events: &mut Vec<AgentEvent>,
    ) -> ItemId {
        self.tool_uses = self.tool_uses.saturating_add(1);
        let summary = tool_summary(name, &input, None);
        let item = self.start_item(
            turn,
            ItemKind::Tool {
                kind: tool_kind(name),
                name: name.to_owned(),
                input: input.clone(),
            },
            parent,
            Some(name.to_owned()),
            input,
            events,
        );
        events.push(AgentEvent::ItemUpdated {
            item,
            patch: ItemPatch {
                summary: Some(summary),
                status: Some(ItemStatus::Running),
                ..ItemPatch::default()
            },
        });
        if !provider_id.is_empty() {
            self.tool_items.insert(provider_id.to_owned(), item);
        }
        item
    }

    fn start_item(
        &mut self,
        turn: TurnId,
        kind: ItemKind,
        parent: Option<ItemId>,
        tool_name: Option<String>,
        input: Value,
        events: &mut Vec<AgentEvent>,
    ) -> ItemId {
        let item = ItemId::new();
        events.push(AgentEvent::ItemStarted {
            turn,
            item,
            kind,
            parent,
        });
        self.open_items.insert(
            item,
            OpenItem {
                turn,
                status: ItemStatus::Running,
                tool_name,
                input,
            },
        );
        item
    }

    fn complete_item(&mut self, item: ItemId, status: ItemStatus, events: &mut Vec<AgentEvent>) {
        if let Some(open) = self.open_items.get_mut(&item) {
            if matches!(
                open.status,
                ItemStatus::Done | ItemStatus::Error | ItemStatus::Denied
            ) {
                return;
            }
            open.status = status;
            events.push(AgentEvent::ItemCompleted { item, status });
        }
    }

    fn permission_denied(&mut self, message: &SystemMessage, events: &mut Vec<AgentEvent>) {
        let Some(provider_id) = string_field(&message.fields, "tool_use_id") else {
            return;
        };
        let Some(item) = self.tool_items.get(&provider_id).copied() else {
            return;
        };
        let reason = string_field(&message.fields, "message")
            .or_else(|| string_field(&message.fields, "decision_reason"));
        events.push(AgentEvent::ItemUpdated {
            item,
            patch: ItemPatch {
                result: reason,
                status: Some(ItemStatus::Denied),
                ..ItemPatch::default()
            },
        });
        self.complete_item(item, ItemStatus::Denied, events);
    }

    fn task_started(&mut self, message: &SystemMessage, events: &mut Vec<AgentEvent>) {
        let (Some(turn), Some(task_id)) =
            (self.active_turn, string_field(&message.fields, "task_id"))
        else {
            return;
        };
        let description = string_field(&message.fields, "description").unwrap_or_default();
        let name = string_field(&message.fields, "subagent_type")
            .or_else(|| string_field(&message.fields, "task_type"))
            .unwrap_or_else(|| "subagent".to_owned());
        let parent = string_field(&message.fields, "tool_use_id")
            .and_then(|id| self.tool_items.get(&id).copied());
        let item = self.start_item(
            turn,
            ItemKind::Subagent { name, description },
            parent,
            None,
            Value::Null,
            events,
        );
        self.task_items.insert(task_id, item);
    }

    fn task_updated(&mut self, message: &SystemMessage, events: &mut Vec<AgentEvent>) {
        let Some(task_id) = string_field(&message.fields, "task_id") else {
            return;
        };
        let Some(item) = self.task_items.get(&task_id).copied() else {
            return;
        };
        let patch = message.fields.get("patch").unwrap_or(&Value::Null);
        let status = patch.get("status").and_then(Value::as_str);
        if let Some(description) = patch.get("description").and_then(Value::as_str) {
            events.push(AgentEvent::ItemUpdated {
                item,
                patch: ItemPatch {
                    summary: Some(description.to_owned()),
                    ..ItemPatch::default()
                },
            });
        }
        if let Some(status) = task_status(status) {
            self.complete_item(item, status, events);
        }
    }

    fn task_progress_system(&mut self, message: &SystemMessage, events: &mut Vec<AgentEvent>) {
        let Some(task_id) = string_field(&message.fields, "task_id") else {
            return;
        };
        let Some(item) = self.task_items.get(&task_id).copied() else {
            return;
        };
        let usage = message.fields.get("usage").unwrap_or(&Value::Null);
        let tool_uses = usage
            .get("tool_uses")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let duration_ms = usage
            .get("duration_ms")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        events.push(AgentEvent::ItemUpdated {
            item,
            patch: ItemPatch {
                summary: string_field(&message.fields, "summary")
                    .or_else(|| string_field(&message.fields, "description")),
                result: Some(format!("{tool_uses} tools · {}s", duration_ms / 1_000)),
                status: Some(ItemStatus::Running),
                ..ItemPatch::default()
            },
        });
    }

    fn task_notification(&mut self, message: &SystemMessage, events: &mut Vec<AgentEvent>) {
        let Some(task_id) = string_field(&message.fields, "task_id") else {
            return;
        };
        let Some(item) = self.task_items.get(&task_id).copied() else {
            return;
        };
        let status = match string_field(&message.fields, "status").as_deref() {
            Some("completed") => ItemStatus::Done,
            Some("failed") => ItemStatus::Error,
            Some("stopped") => ItemStatus::Denied,
            _ => return,
        };
        events.push(AgentEvent::ItemUpdated {
            item,
            patch: ItemPatch {
                output: string_field(&message.fields, "summary"),
                status: Some(status),
                ..ItemPatch::default()
            },
        });
        self.complete_item(item, status, events);
    }

    fn background_tasks_changed(&mut self, message: &SystemMessage, events: &mut Vec<AgentEvent>) {
        let tasks = message
            .fields
            .get("tasks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let live = tasks
            .iter()
            .filter_map(|task| {
                task.get("task_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<HashSet<_>>();
        if let Some(turn) = self.active_turn {
            for task in &tasks {
                let Some(task_id) = task.get("task_id").and_then(Value::as_str) else {
                    continue;
                };
                if self.task_items.contains_key(task_id) {
                    continue;
                }
                let name = task
                    .get("task_type")
                    .and_then(Value::as_str)
                    .unwrap_or("subagent")
                    .to_owned();
                let description = task
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let item = self.start_item(
                    turn,
                    ItemKind::Subagent { name, description },
                    None,
                    None,
                    Value::Null,
                    events,
                );
                self.task_items.insert(task_id.to_owned(), item);
            }
        }
        for task in &live {
            if self.task_items.contains_key(task) {
                self.background_tasks.insert(task.clone());
            }
        }
        // Replace semantics cover background tasks only: a foreground subagent row is closed by
        // its own `task_updated`/`task_notification`, never by dropping out of this list.
        let ended = self
            .background_tasks
            .iter()
            .filter(|task| !live.contains(*task))
            .cloned()
            .collect::<Vec<_>>();
        for task in ended {
            self.background_tasks.remove(&task);
            if let Some(item) = self.task_items.get(&task).copied() {
                self.complete_item(item, ItemStatus::Done, events);
            }
        }
    }
}

/// The `(owner, index)` a stream frame addresses.
fn block_key(message: &StreamMessage) -> BlockKey {
    (
        message.parent_tool_use_id.clone(),
        message
            .event
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    )
}

fn permission_response(
    pending: &PendingGate,
    choice: PermissionChoice,
    edited_payload: Option<&str>,
) -> Result<Value, ProviderError> {
    let mut input = pending.input.clone();
    // §9's `e` corrects the invocation and then allows it once, so an edited payload is applied
    // whether the answer arrives as `Edit` or as the `AllowOnce` the card sends after editing.
    if let Some(edited) = edited_payload
        && matches!(choice, PermissionChoice::AllowOnce | PermissionChoice::Edit)
    {
        let Some(object) = input.as_object_mut() else {
            return Err(ProviderError::Protocol {
                message: "Claude permission input is not an object".to_owned(),
            });
        };
        object.insert("command".to_owned(), Value::String(edited.to_owned()));
    }
    let response = match choice {
        PermissionChoice::AllowOnce => json!({
            "behavior": "allow",
            "updatedInput": input,
            "toolUseID": pending.tool_use_id,
            "decisionClassification": "user_allow",
        }),
        PermissionChoice::AllowSession => json!({
            "behavior": "allow",
            "updatedInput": input,
            "updatedPermissions": session_scoped(&pending.suggestions),
            "toolUseID": pending.tool_use_id,
            "decisionClassification": "user_permanent",
        }),
        PermissionChoice::Edit => {
            if edited_payload.is_none() {
                return Err(ProviderError::Protocol {
                    message: "Claude Bash edit answer omitted edited_payload".to_owned(),
                });
            }
            json!({
                "behavior": "allow",
                "updatedInput": input,
                "toolUseID": pending.tool_use_id,
                "decisionClassification": "user_allow",
            })
        }
        PermissionChoice::Deny | PermissionChoice::DenyAndStop => json!({
            "behavior": "deny",
            "message": "User declined tool execution.",
            "interrupt": choice == PermissionChoice::DenyAndStop,
            "toolUseID": pending.tool_use_id,
            "decisionClassification": "user_reject",
        }),
        PermissionChoice::AllowDirectory => {
            return Err(ProviderError::Protocol {
                message: "Claude does not support directory-scoped permission answers".to_owned(),
            });
        }
    };
    Ok(response)
}

/// Rewrites the CLI's suggested permission updates to the scope the card actually promises.
///
/// §4.1: `allow for this session` echoes the suggested `addRules` with
/// `destination: "session"`. A suggestion arrives with whatever destination the CLI would have
/// used — `localSettings` writes a permanent rule into the user's settings file — so the
/// destination is rewritten rather than echoed, and only the two update kinds Fleet's copy
/// describes are forwarded.
fn session_scoped(suggestions: &[Value]) -> Vec<Value> {
    suggestions
        .iter()
        .filter(|suggestion| {
            matches!(
                suggestion.get("type").and_then(Value::as_str),
                Some("addRules" | "setMode")
            )
        })
        .cloned()
        .map(|mut suggestion| {
            if let Some(object) = suggestion.as_object_mut() {
                object.insert(
                    "destination".to_owned(),
                    Value::String("session".to_owned()),
                );
            }
            suggestion
        })
        .collect()
}

fn question_response(
    pending: &PendingGate,
    questions: &[WireQuestion],
    answers: &[Vec<String>],
) -> Result<Value, ProviderError> {
    if answers.len() != questions.len() {
        return Err(ProviderError::Protocol {
            message: format!(
                "Claude question gate expected {} answers, got {}",
                questions.len(),
                answers.len()
            ),
        });
    }
    let mut answer_map = serde_json::Map::new();
    for (question, values) in questions.iter().zip(answers) {
        let value = if question.multi_select {
            Value::Array(values.iter().cloned().map(Value::String).collect())
        } else {
            Value::String(values.first().cloned().unwrap_or_default())
        };
        answer_map.insert(question.text.clone(), value);
    }
    let mut input = pending.input.clone();
    let Some(object) = input.as_object_mut() else {
        return Err(ProviderError::Protocol {
            message: "Claude question input is not an object".to_owned(),
        });
    };
    object.insert("answers".to_owned(), Value::Object(answer_map));
    Ok(json!({
        "behavior": "allow",
        "updatedInput": input,
        "toolUseID": pending.tool_use_id,
    }))
}

fn plan_response(pending: &PendingGate, answer: &PlanAnswer) -> Value {
    match answer {
        PlanAnswer::Approve => json!({
            "behavior": "allow",
            "updatedInput": pending.input,
            "toolUseID": pending.tool_use_id,
        }),
        PlanAnswer::AskForChanges { note } => json!({
            "behavior": "deny",
            "message": note,
            "interrupt": false,
            "toolUseID": pending.tool_use_id,
        }),
    }
}

fn control_success(request_id: &str, response: Value) -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": response,
        }
    })
}

fn control_error(request_id: &str, error: &str) -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": request_id,
            "error": error,
        }
    })
}

fn permission_option(id: &str, label: PermissionChoice) -> PermissionOption {
    PermissionOption {
        id: ProviderOptionId(id.to_owned()),
        label,
    }
}

fn questions_from_input(input: &Value) -> (Vec<Question>, Vec<WireQuestion>) {
    let mut questions = Vec::new();
    let mut wire = Vec::new();
    for value in input
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let text = value
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let multi_select = value
            .get("multiSelect")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let options = value
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|option| QuestionOption {
                label: option
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                description: option
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            })
            .collect();
        questions.push(Question {
            text: text.clone(),
            header: value
                .get("header")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            options,
            multi_select,
            allow_other: true,
        });
        wire.push(WireQuestion { text, multi_select });
    }
    (questions, wire)
}

fn top_level_steps(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| {
            let line = line.trim_end();
            ["- ", "* ", "+ "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                .or_else(|| {
                    let (number, rest) = line.split_once(". ")?;
                    number
                        .chars()
                        .all(|character| character.is_ascii_digit())
                        .then_some(rest)
                })
        })
        .map(ToOwned::to_owned)
        .collect()
}

pub(super) fn permission_mode_to_wire(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "default",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::Plan => "plan",
        PermissionMode::FullAccess => "bypassPermissions",
    }
}

fn permission_mode_from_wire(mode: &str) -> PermissionMode {
    match mode {
        "acceptEdits" => PermissionMode::AcceptEdits,
        "plan" => PermissionMode::Plan,
        "bypassPermissions" => PermissionMode::FullAccess,
        _ => PermissionMode::Ask,
    }
}

pub(super) fn tool_kind(name: &str) -> ToolKind {
    match name {
        "Read" => ToolKind::Read,
        "Edit" | "NotebookEdit" => ToolKind::Edit,
        "Write" => ToolKind::Write,
        "Bash" => ToolKind::Bash,
        "Glob" | "WebSearch" | "ToolSearch" => ToolKind::Search,
        "Grep" => ToolKind::Grep,
        "WebFetch" => ToolKind::Fetch,
        "Task" | "Workflow" => ToolKind::Agent,
        "TodoWrite" => ToolKind::Todo,
        "Skill" => ToolKind::Skill,
        _ if name.starts_with("mcp__") => {
            let server = name
                .trim_start_matches("mcp__")
                .split("__")
                .next()
                .unwrap_or("unknown")
                .to_owned();
            ToolKind::Mcp { server }
        }
        _ => ToolKind::Unknown {
            name: name.to_owned(),
        },
    }
}

fn tool_summary(name: &str, input: &Value, diff: Option<&ToolDiff>) -> String {
    let string = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| input.get(key).and_then(Value::as_str))
            .unwrap_or_default()
    };
    match name {
        "Read" => string(&["file_path", "path"]).to_owned(),
        "Bash" => string(&["command"]).to_owned(),
        "Edit" | "Write" | "NotebookEdit" => {
            let path = string(&["file_path", "notebook_path", "path"]);
            diff.map_or_else(
                || path.to_owned(),
                |diff| format!("{path} +{} −{}", diff.added, diff.removed),
            )
        }
        "Grep" | "Glob" => {
            let pattern = string(&["pattern"]);
            let path = string(&["path"]);
            if path.is_empty() {
                pattern.to_owned()
            } else {
                format!("{pattern} in {path}")
            }
        }
        "WebFetch" => string(&["url"]).to_owned(),
        "Task" | "Workflow" => string(&["description", "prompt"]).to_owned(),
        "TodoWrite" => format!(
            "{} items",
            input
                .get("todos")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default()
        ),
        "Skill" => string(&["skill", "name"]).to_owned(),
        // §2 gives every tool call one 30 px row with a *one-line* summary, and §4.1 turns the
        // `can_use_tool` for these two into the gate itself. The whole questions payload — or a
        // whole plan — as compact JSON is neither one line nor new information above the card
        // that already carries it.
        "AskUserQuestion" => question_headers(input),
        "ExitPlanMode" => "plan".to_owned(),
        _ if name.starts_with("mcp__") => string(&["description", "query", "url"]).to_owned(),
        _ => permission_payload(name, input),
    }
}

/// The question headers of an `AskUserQuestion` input, as one line.
fn question_headers(input: &Value) -> String {
    let headers = input
        .get("questions")
        .and_then(Value::as_array)
        .map(|questions| {
            questions
                .iter()
                .filter_map(|question| {
                    question
                        .get("header")
                        .or_else(|| question.get("question"))
                        .and_then(Value::as_str)
                })
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .unwrap_or_default();
    if headers.is_empty() {
        "question".to_owned()
    } else {
        headers
    }
}

fn permission_payload(tool_name: &str, input: &Value) -> String {
    let summary = tool_summary_without_fallback(tool_name, input);
    if summary.is_empty() {
        compact_json(input)
    } else {
        summary
    }
}

/// How many diff lines a permission card shows before it elides the rest.
///
/// §2 gives the card a fixed 760 px measure; a `Write` of a whole file would otherwise push its
/// own keys off the bottom. The head of the diff is the part that identifies the change, and
/// the elision line says how much was left out rather than pretending there was no more.
const PERMISSION_DIFF_MAX_LINES: usize = 40;

fn tool_summary_without_fallback(tool_name: &str, input: &Value) -> String {
    // §3.2: "`payload` is the invocation itself, never the model's prose description of it,
    // because it is what the card shows literally". For a write the invocation is the path
    // *and* the bytes: a card naming only the destination asks the user to allow a change they
    // have not been shown, while the same card for `Bash` shows the whole command.
    if matches!(tool_name, "Edit" | "Write" | "NotebookEdit") {
        return write_payload(tool_name, input);
    }
    let key = match tool_name {
        "Bash" => "command",
        "Read" => "file_path",
        "Grep" | "Glob" => "pattern",
        "WebFetch" => "url",
        "Task" => "description",
        "Skill" => "skill",
        _ => return String::new(),
    };
    input
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The destination path plus the change itself, as the permission card reads it out.
fn write_payload(tool_name: &str, input: &Value) -> String {
    let path = ["file_path", "notebook_path", "path"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .unwrap_or_default();
    let Some(diff) = synthetic_diff(path, tool_name, input, &Value::Null) else {
        return path.to_owned();
    };
    // The `--- a/… +++ b/…` header repeats the path the first line already carries, and the
    // hunk header is for a diff parser, not for a reader deciding whether to allow this.
    let body = diff
        .lines()
        .skip_while(|line| {
            line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with("@@ ")
        })
        .collect::<Vec<_>>();
    if body.is_empty() {
        return path.to_owned();
    }
    let elided = body.len().saturating_sub(PERMISSION_DIFF_MAX_LINES);
    let mut payload = String::from(path);
    for line in body.iter().take(PERMISSION_DIFF_MAX_LINES) {
        payload.push('\n');
        payload.push_str(line);
    }
    if elided > 0 {
        payload.push_str(&format!("\n… {elided} more lines"));
    }
    payload
}

fn derive_tool_diff(name: &str, input: &Value, result: Option<&Value>) -> Option<ToolDiff> {
    if !matches!(name, "Edit" | "Write" | "NotebookEdit") {
        return None;
    }
    let result = result.unwrap_or(&Value::Null);
    let path = result
        .get("filePath")
        .or_else(|| result.get("file_path"))
        .or_else(|| result.get("path"))
        .or_else(|| input.get("file_path"))
        .or_else(|| input.get("notebook_path"))
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)?;
    let unified = result
        .get("unified_diff")
        .or_else(|| result.get("diff"))
        .or_else(|| result.get("patch"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| structured_patch(path, result))
        .or_else(|| synthetic_diff(path, name, input, result))?;
    let (added, removed) = diff_counts(&unified);
    Some(ToolDiff {
        path: PathBuf::from(path),
        added,
        removed,
        unified,
    })
}

fn structured_patch(path: &str, result: &Value) -> Option<String> {
    let hunks = result.get("structuredPatch")?.as_array()?;
    let mut diff = format!("--- a/{path}\n+++ b/{path}\n");
    for hunk in hunks {
        let old_start = hunk
            .get("oldStart")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let old_lines = hunk
            .get("oldLines")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let new_start = hunk
            .get("newStart")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let new_lines = hunk
            .get("newLines")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        diff.push_str(&format!(
            "@@ -{old_start},{old_lines} +{new_start},{new_lines} @@\n"
        ));
        for line in hunk
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            diff.push_str(line);
            diff.push('\n');
        }
    }
    (diff.lines().count() > 2).then_some(diff)
}

/// A unified diff for an `Edit`/`Write` whose result carried no `structuredPatch`.
///
/// The hunk header is written in full. Both readers of this text — `fleet_git::parse::diff` and
/// `fleet_lazygit::diff_view::hunk_header` — require `@@ <ranges> @@`, and a bare `@@` parses as
/// a file with zero hunks: §5's inline diff under the row would then render empty for every
/// Claude `Write`. The paths are relative (`a/…`), never `a//abs/path`.
fn synthetic_diff(path: &str, name: &str, input: &Value, result: &Value) -> Option<String> {
    let old = if name == "Write" {
        ""
    } else {
        input
            .get("old_string")
            .or_else(|| result.get("oldString"))?
            .as_str()?
    };
    let new = input
        .get("new_string")
        .or_else(|| input.get("content"))
        .or_else(|| result.get("newString"))?
        .as_str()?;
    let relative = path.trim_start_matches('/');
    let old_count = old.lines().count();
    let new_count = new.lines().count();
    // An empty side occupies no line, and unified diffs spell that start as 0.
    let old_start = usize::from(old_count > 0);
    let new_start = usize::from(new_count > 0);
    let mut diff = format!(
        "--- a/{relative}\n+++ b/{relative}\n@@ -{old_start},{old_count} +{new_start},{new_count} @@\n"
    );
    for line in old.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in new.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    Some(diff)
}

fn diff_counts(unified: &str) -> (u64, u64) {
    let added = unified
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count() as u64;
    let removed = unified
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count() as u64;
    (added, removed)
}

/// One provider notice, with the CLI's own terminal affordances removed.
///
/// Claude's copy is written for its TUI and ships the keystroke that would act on it —
/// `Stop hook error occurred · ctrl+o to see`. §5 draws a notice as one muted line in a native
/// transcript and §2/§9 make `^s`-prefixed keys the only vocabulary Fleet has, so `ctrl+o` is
/// dead copy that names a key nothing here implements. Trailing `·` clauses that are only a
/// keystroke are dropped; `None` means nothing was left to say.
fn notice_text(text: &str) -> Option<String> {
    let mut clauses: Vec<&str> = text.split('·').map(str::trim).collect();
    while clauses
        .last()
        .is_some_and(|clause| is_key_affordance(clause))
    {
        clauses.pop();
    }
    let notice = clauses.join(" · ");
    let notice = notice.trim();
    (!notice.is_empty()).then(|| notice.to_owned())
}

/// Whether a notice clause is only a terminal keystroke the user is invited to press.
fn is_key_affordance(clause: &str) -> bool {
    let lower = clause.to_ascii_lowercase();
    [
        "ctrl+", "ctrl-", "cmd+", "shift+", "alt+", "option+", "esc ", "press ",
    ]
    .iter()
    .any(|token| lower.starts_with(token))
        || lower == "esc"
}

/// Whether a usage frame reports nothing at all, which a cumulative counter never does twice.
fn usage_is_empty(usage: &Usage) -> bool {
    usage.total_tokens == 0
        && usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.reasoning_tokens == 0
        && usage.cache_read_tokens == 0
        && usage.cache_write_tokens == 0
}

fn usage_from_value(value: &Value) -> Usage {
    let input_tokens = value
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output_tokens = value
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let reasoning_tokens = value
        .get("output_tokens_details")
        .and_then(|details| details.get("thinking_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cache_read_tokens = value
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cache_write_tokens = value
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let web_search_requests = value
        .get("server_tool_use")
        .and_then(|tools| tools.get("web_search_requests"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let mut extra = BTreeMap::new();
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            if !matches!(
                key.as_str(),
                "input_tokens"
                    | "output_tokens"
                    | "output_tokens_details"
                    | "cache_read_input_tokens"
                    | "cache_creation_input_tokens"
                    | "server_tool_use"
            ) {
                extra.insert(key.clone(), value.clone());
            }
        }
    }
    Usage {
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_read_tokens,
        cache_write_tokens,
        total_tokens: input_tokens
            .saturating_add(output_tokens)
            .saturating_add(cache_read_tokens)
            .saturating_add(cache_write_tokens),
        web_search_requests,
        tool_uses: 0,
        extra,
    }
}

/// The tokens the conversation itself occupies at the end of a turn, if the frame measured any.
///
/// harness-protocols.md:336 makes `usage` "main-loop/per-turn" and `modelUsage` cumulative
/// across the query process. Occupancy is a level, not a total: `usage.iterations` is the
/// per-request breakdown, and its *last* entry is what the model was actually holding when the
/// turn ended — summing the iterations (or reading the cumulative `modelUsage`) counts the same
/// resident context once per request and walks a long thread up to 100% on an empty window.
fn context_tokens(usage: &Value) -> Option<u64> {
    let resident = |value: &Value| {
        [
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ]
        .into_iter()
        .filter_map(|key| value.get(key).and_then(Value::as_u64))
        .fold(0_u64, u64::saturating_add)
    };
    let tokens = usage
        .get("iterations")
        .and_then(Value::as_array)
        .and_then(|iterations| iterations.last())
        .map_or_else(|| resident(usage), resident);
    (tokens > 0).then_some(tokens)
}

/// How much of the session model's context window the turn is holding (§2's `context 34%`).
///
/// `modelUsage` is keyed by model id, is cumulative across the query process and, per
/// harness-protocols.md, "include[s] pipeline subcalls": a haiku summarisation entry is in there
/// beside the model actually answering. `serde_json::Map` is a `BTreeMap` here, so taking the
/// first entry takes the alphabetically-first model id — and with it that model's window. The
/// session's own model is looked up by name, and the widest window is the fallback when the
/// session model is unknown, since a subcall's window is never the larger of the two. Only the
/// *window* is read from it: the numerator is this turn's own occupancy.
fn context_pct(tokens: Option<u64>, model_usage: &Value, session_model: Option<&str>) -> f32 {
    let Some(used) = tokens else {
        return 0.0;
    };
    let Some(models) = model_usage.as_object() else {
        return 0.0;
    };
    let model = session_model
        .and_then(|model| models.get(model))
        .or_else(|| {
            models.values().max_by_key(|model| {
                model
                    .get("contextWindow")
                    .and_then(Value::as_u64)
                    .unwrap_or_default()
            })
        });
    let Some(model) = model else {
        return 0.0;
    };
    let window = model
        .get("contextWindow")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if window == 0 {
        0.0
    } else {
        (used as f64 * 100.0 / window as f64).min(100.0) as f32
    }
}

fn turn_outcome(message: &ResultMessage) -> TurnOutcome {
    let reason = message.terminal_reason.as_deref();
    if matches!(message.subtype.as_str(), "error_max_turns") || reason == Some("max_turns") {
        TurnOutcome::MaxTurns
    } else if matches!(message.subtype.as_str(), "error_max_budget_usd")
        || reason == Some("budget_exhausted")
    {
        TurnOutcome::BudgetExhausted
    } else if !message.permission_denials.is_empty() && message.is_error {
        TurnOutcome::Denied
    } else if message.is_error || message.subtype != "success" {
        TurnOutcome::Error {
            message: message
                .errors
                .first()
                .cloned()
                .or_else(|| message.result.clone())
                .or_else(|| reason.map(ToOwned::to_owned)),
        }
    } else if matches!(reason, None | Some("completed")) {
        TurnOutcome::Completed
    } else {
        TurnOutcome::Other {
            reason: reason.unwrap_or("unknown").to_owned(),
        }
    }
}

fn task_status(status: Option<&str>) -> Option<ItemStatus> {
    match status {
        Some("completed") => Some(ItemStatus::Done),
        Some("failed") => Some(ItemStatus::Error),
        Some("killed") => Some(ItemStatus::Denied),
        _ => None,
    }
}

fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(value) if !value.is_null() => compact_json(value),
        _ => String::new(),
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            value.as_str().map(ToOwned::to_owned).or_else(|| {
                value
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
        })
        .collect()
}

fn string_field(fields: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    fields
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn u64_field(fields: &serde_json::Map<String, Value>, key: &str) -> u64 {
    fields.get(key).and_then(Value::as_u64).unwrap_or_default()
}

fn u32_field(fields: &serde_json::Map<String, Value>, key: &str) -> u32 {
    u64_field(fields, key).try_into().unwrap_or(u32::MAX)
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}

fn strip_ansi(value: &str) -> String {
    let mut clean = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else {
            clean.push(character);
        }
    }
    clean
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_control_sequences() {
        assert_eq!(strip_ansi("\u{1b}[31mblocked\u{1b}[0m"), "blocked");
    }

    #[test]
    fn extracts_only_top_level_plan_steps() {
        assert_eq!(
            top_level_steps("# Plan\n\n- first\n  - nested\n2. second\ntext"),
            ["first", "second"]
        );
    }
}
