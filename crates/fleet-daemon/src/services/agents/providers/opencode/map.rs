//! Stateful OpenCode wire-to-domain event mapper.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use fleet_core::agents::{
    AbortReason, AgentEvent, CheckpointKind, FileDelta, GateAnswer, GateId, GateKind, GateResolver,
    ItemId, ItemKind, ItemPatch, ItemStatus, ModelSelection, PermissionChoice, PermissionMode,
    PermissionOption, PlanAnswer, ProviderOptionId, Question, QuestionOption, SessionState,
    StreamKind, ToolDiff, ToolKind, TurnId, TurnOutcome, Usage, UserInput,
};
use serde_json::{Value, json};

use super::sse::WireEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PendingGateKind {
    Permission,
    Question,
    Plan,
}

#[derive(Debug, Clone)]
pub(super) struct PendingGate {
    pub(super) provider_id: String,
    pub(super) kind: PendingGateKind,
}

#[derive(Debug, Clone)]
pub(super) enum MapAction {
    AutoPermission {
        gate: GateId,
        provider_id: String,
        answer: GateAnswer,
    },
}

#[derive(Debug, Default)]
pub(super) struct Mapped {
    pub(super) events: Vec<AgentEvent>,
    pub(super) actions: Vec<MapAction>,
    /// OpenCode's own name for the event these were mapped from (§11).
    pub(super) raw: Option<String>,
}

impl Mapped {
    /// Names the wire event every mapped event came from.
    fn with_raw(mut self, raw: &str) -> Self {
        self.raw = Some(raw.to_owned());
        self
    }
}

#[derive(Debug)]
struct TurnContext {
    id: TurnId,
    started: Instant,
    agent: String,
    usage: Usage,
    cost_usd: Option<f64>,
    error: Option<String>,
    assistant_error_name: Option<String>,
    assistant_started_ms: Option<u64>,
    assistant_completed_ms: Option<u64>,
    abort_outstanding: bool,
    assistant_items: Vec<ItemId>,
    /// Added/removed lines per path, from every `PatchPart` this turn produced.
    ///
    /// §2 fixes the turn footer as `… 2 files changed +36 −3`; OpenCode publishes the numbers
    /// only per patch part, so the turn accumulates them the way the Claude mapper does.
    files_changed: BTreeMap<PathBuf, (u64, u64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartKind {
    Text,
    Reasoning,
    Tool,
}

#[derive(Debug, Clone, PartialEq)]
struct ToolSnapshot {
    input: Value,
    summary: Option<String>,
    result: Option<String>,
    output: Option<String>,
    status: ItemStatus,
}

#[derive(Debug)]
struct PartRecord {
    item: ItemId,
    kind: PartKind,
    text: String,
    completed: bool,
    /// What the last `ItemUpdated` for this tool part carried, so §4.2's cumulative replacement
    /// stays an upsert: a part that arrives unchanged is a no-op, not another sequence.
    tool: Option<ToolSnapshot>,
}

#[derive(Debug)]
pub(super) struct Mapper {
    session_id: String,
    mode: PermissionMode,
    active: Option<TurnContext>,
    parts: HashMap<String, PartRecord>,
    message_roles: HashMap<String, String>,
    open_items: Vec<ItemId>,
    item_status: HashMap<ItemId, ItemStatus>,
    edit_items_by_message: HashMap<String, ItemId>,
    latest_edit_item: Option<ItemId>,
    provider_gates: HashMap<String, GateId>,
    gates: HashMap<GateId, PendingGate>,
    todo_item: Option<ItemId>,
    title: Option<String>,
    /// The last session state reported, so an unchanged one costs no sequence.
    session_state: SessionState,
    /// The last retry reported, so a countdown does not re-append per tick.
    retry: Option<(u32, String)>,
    /// The last usage reported, so an all-zero repeat costs no sequence.
    usage: Option<(Usage, Option<f64>)>,
    /// The model the assistant messages report, once it is known.
    model: Option<(String, String)>,
    /// Context window per `providerID/modelID`, from `GET /config/providers`.
    context_limits: HashMap<String, u64>,
    /// Turns admitted so far, so a reconciliation can tell its snapshot has been outrun.
    admissions: u64,
}

impl Mapper {
    pub(super) fn new(session_id: String, mode: PermissionMode) -> Self {
        Self {
            session_id,
            mode,
            active: None,
            parts: HashMap::new(),
            message_roles: HashMap::new(),
            open_items: Vec::new(),
            item_status: HashMap::new(),
            edit_items_by_message: HashMap::new(),
            latest_edit_item: None,
            provider_gates: HashMap::new(),
            gates: HashMap::new(),
            todo_item: None,
            title: None,
            session_state: SessionState::Ready,
            retry: None,
            usage: None,
            model: None,
            context_limits: HashMap::new(),
            admissions: 0,
        }
    }

    /// Records the context window of every model the server offers (§2's `context 34%`).
    pub(super) fn set_context_limits(&mut self, limits: HashMap<String, u64>) {
        self.context_limits = limits;
    }

    /// The session-state event, dropped when the state is already what it says.
    ///
    /// §6 keeps the log lean: a thread idling in a provider backoff must not grow its transcript
    /// with identical `running` lines that every daemon start then replays.
    fn state_changed(&mut self, state: SessionState) -> Option<AgentEvent> {
        (self.session_state != state).then(|| {
            self.session_state = state;
            AgentEvent::SessionStateChanged(state)
        })
    }

    /// The retry event, dropped while only the countdown moves.
    fn retrying(&mut self, attempt: u32, retry_in_ms: u64, reason: String) -> Option<AgentEvent> {
        let observation = (attempt, reason.clone());
        (self.retry.as_ref() != Some(&observation)).then(|| {
            self.retry = Some(observation);
            AgentEvent::Retrying {
                attempt,
                retry_in_ms,
                reason,
            }
        })
    }

    /// The usage event, dropped when neither the counters nor the cost moved.
    ///
    /// A cost of exactly zero is OpenCode saying it has no cost to report — a local or a
    /// subscription model — and §2's metadata row would otherwise print `$0.00` forever.
    fn token_usage(
        &mut self,
        turn: TurnId,
        usage: Usage,
        cost_usd: Option<f64>,
    ) -> Option<AgentEvent> {
        let cost_usd = cost_usd.filter(|cost| *cost > 0.0);
        // The first usage frame of a session is all zeros, and the dedupe below only compares
        // against the previous observation: emitting it drives §2's metadata row to
        // `context 0%` at the head of every turn. An empty frame reports nothing, so it is
        // dropped outright, exactly as the Claude mapper drops its own.
        if usage_is_empty(&usage) && cost_usd.is_none() {
            return None;
        }
        let observation = (usage.clone(), cost_usd);
        (self.usage.as_ref() != Some(&observation)).then(|| {
            self.usage = Some(observation);
            AgentEvent::TokenUsage {
                turn,
                context_pct: self.context_pct(&usage),
                usage,
                cost_usd,
            }
        })
    }

    /// How much of the active model's context window this turn is holding.
    ///
    /// Zero when the window is unknown, which the metadata row zero-suppresses rather than
    /// printing a percentage nothing measured.
    fn context_pct(&self, usage: &Usage) -> f32 {
        let limit = self
            .model
            .as_ref()
            .map(|(provider, model)| format!("{provider}/{model}"))
            .and_then(|key| self.context_limits.get(&key).copied())
            .filter(|limit| *limit > 0);
        let Some(limit) = limit else {
            return 0.0;
        };
        let held = usage
            .input_tokens
            .saturating_add(usage.cache_read_tokens)
            .saturating_add(usage.cache_write_tokens)
            .saturating_add(usage.output_tokens);
        #[expect(
            clippy::cast_precision_loss,
            reason = "token counts far below f32's exact integer range"
        )]
        let pct = (held as f32 / limit as f32) * 100.0;
        pct.clamp(0.0, 100.0)
    }

    pub(super) fn set_mode(&mut self, mode: PermissionMode) {
        self.mode = mode;
    }

    pub(super) fn is_running(&self) -> bool {
        self.active.is_some()
    }

    /// How many turns this mapper has admitted, as an epoch for out-of-band reconciliation.
    ///
    /// `reconcile_status` reads `GET /session/status` and then awaits a second round trip
    /// before applying the answer. A prompt admitted in that window is *newer* than the
    /// snapshot, and §3.3 rule 2 lets only the provider's own terminal for a turn settle it:
    /// carrying the epoch across the awaits is what stops an `idle` about finished work from
    /// settling work that had not started when it was observed.
    pub(super) const fn admissions(&self) -> u64 {
        self.admissions
    }

    pub(super) fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(super) fn active_turn(&self) -> Option<TurnId> {
        self.active.as_ref().map(|turn| turn.id)
    }

    pub(super) fn gate(&self, gate: GateId) -> Option<PendingGate> {
        self.gates.get(&gate).cloned()
    }

    pub(super) fn admit(
        &mut self,
        turn: TurnId,
        input: UserInput,
        agent: String,
    ) -> Result<Vec<AgentEvent>, String> {
        let user_item = ItemId::new();
        let mut events = Vec::new();
        self.admissions = self.admissions.saturating_add(1);
        match &self.active {
            Some(active) if active.id != turn => {
                return Err(format!(
                    "cannot submit turn {turn} while turn {} is active",
                    active.id
                ));
            }
            Some(_) => {}
            None => {
                self.latest_edit_item = None;
                self.active = Some(TurnContext {
                    id: turn,
                    started: Instant::now(),
                    agent,
                    usage: Usage::default(),
                    cost_usd: None,
                    error: None,
                    assistant_error_name: None,
                    assistant_started_ms: None,
                    assistant_completed_ms: None,
                    abort_outstanding: false,
                    files_changed: BTreeMap::new(),
                    assistant_items: Vec::new(),
                });
                events.push(AgentEvent::TurnStarted { turn, user_item });
            }
        }
        events.push(AgentEvent::ItemStarted {
            turn,
            item: user_item,
            kind: ItemKind::UserMessage {
                text: input.text,
                attachments: input.attachments,
            },
            parent: None,
        });
        events.push(AgentEvent::ItemCompleted {
            item: user_item,
            status: ItemStatus::Done,
        });
        Ok(events)
    }

    /// Takes back an admission whose prompt request never reached the server.
    ///
    /// The turn is admitted before the request so no SSE frame that lands while it is in flight
    /// is dropped; a request that fails has to undo that, or the reducer keeps a turn nothing
    /// will ever settle. Only the turn this call created is withdrawn — a steer joined an
    /// already-running turn and must survive its own failed write.
    pub(super) fn withdraw(&mut self, turn: TurnId) -> bool {
        let matches = self.active.as_ref().is_some_and(|active| active.id == turn);
        if matches {
            self.active = None;
            self.usage = None;
        }
        matches
    }

    pub(super) fn mark_abort(&mut self, turn: TurnId) -> Result<(), String> {
        let Some(active) = self.active.as_mut() else {
            return Err(format!("cannot abort inactive turn {turn}"));
        };
        if active.id != turn {
            return Err(format!(
                "cannot abort turn {turn} while turn {} is active",
                active.id
            ));
        }
        active.abort_outstanding = true;
        Ok(())
    }

    pub(super) fn clear_abort(&mut self, turn: TurnId) {
        if let Some(active) = self.active.as_mut()
            && active.id == turn
        {
            active.abort_outstanding = false;
        }
    }

    pub(super) fn resolve_gate(
        &mut self,
        gate: GateId,
        answer: GateAnswer,
        by: GateResolver,
    ) -> Option<AgentEvent> {
        let pending = self.gates.remove(&gate)?;
        if !pending.provider_id.is_empty() {
            self.provider_gates.remove(&pending.provider_id);
        }
        Some(AgentEvent::GateResolved { gate, answer, by })
    }

    pub(super) fn handle(&mut self, event: WireEvent) -> Mapped {
        if self.is_for_other_session(&event) {
            return Mapped::default();
        }
        let raw = event.kind.clone();
        let mut mapped = match event.kind.as_str() {
            "message.updated" => self.message_updated(&event.properties),
            "message.part.updated" => self.part_updated(&event.properties),
            "message.part.delta" => self.part_delta(&event.properties),
            "todo.updated" => self.todo_updated(&event.properties),
            "session.updated" => self.session_updated(&event.properties),
            "session.error" => self.session_error(&event.properties),
            "session.status" => self.session_status(&event.properties),
            "session.idle" => self.settle_turn(),
            "permission.asked" | "permission.v2.asked" => self.permission_asked(&event.properties),
            "question.asked" | "question.v2.asked" => self.question_asked(&event.properties),
            // §4.2 makes `GET /permission` and `GET /question` recovery truth, and a gate the
            // user answered somewhere else has to close here too, or the card sits at the top
            // of `NeedsYou` for a request the server has already forgotten.
            "permission.replied"
            | "permission.v2.replied"
            | "question.replied"
            | "question.v2.replied"
            | "question.rejected"
            | "question.v2.rejected" => self.gate_replied(&event.properties),
            _ => Mapped::default(),
        };
        if matches!(event.kind.as_str(), "server.connected") {
            let state = if self.active.is_some() {
                SessionState::Running
            } else {
                SessionState::Ready
            };
            mapped.events.extend(self.state_changed(state));
        }
        mapped.with_raw(&raw)
    }

    /// Closes the gate one `*.replied` / `*.rejected` event settled elsewhere.
    ///
    /// harness-protocols.md:644-651 names the request `id` only on the `*.asked` events; every
    /// resolution spells it `requestID`. Reading `id` alone never matched, so a permission the
    /// user answered in the OpenCode TUI left its card pinned at `NeedsYou(Permission)` here
    /// forever. `id` stays as a fallback for a server that spells it the asking way.
    fn gate_replied(&mut self, properties: &Value) -> Mapped {
        let Some(provider_id) = properties
            .get("requestID")
            .or_else(|| properties.get("id"))
            .and_then(Value::as_str)
        else {
            return Mapped::default();
        };
        Mapped {
            events: self
                .close_provider_gate(provider_id, GateResolver::ProviderClosed)
                .into_iter()
                .collect(),
            ..Mapped::default()
        }
    }

    /// Every gate the server still owns, as `(gate, provider id)`.
    ///
    /// The plan gate is Fleet's own (§4.2: OpenCode has no plan-proposed event) and carries no
    /// provider id, so reconciliation never sees it and never closes it.
    pub(super) fn provider_gates(&self) -> Vec<(GateId, String)> {
        self.gates
            .iter()
            .filter(|(_, pending)| !pending.provider_id.is_empty())
            .map(|(gate, pending)| (*gate, pending.provider_id.clone()))
            .collect()
    }

    /// Resolves the gate behind one provider id, with the answer a closed gate carries.
    pub(super) fn close_provider_gate(
        &mut self,
        provider_id: &str,
        by: GateResolver,
    ) -> Option<AgentEvent> {
        let gate = self.provider_gates.get(provider_id).copied()?;
        let answer = match self.gates.get(&gate).map(|pending| pending.kind.clone()) {
            Some(PendingGateKind::Question) => GateAnswer::Question {
                answers: Vec::new(),
            },
            Some(PendingGateKind::Plan) => GateAnswer::Plan(PlanAnswer::AskForChanges {
                note: String::new(),
            }),
            Some(PendingGateKind::Permission) | None => GateAnswer::Permission {
                choice: PermissionChoice::Deny,
                edited_payload: None,
            },
        };
        self.resolve_gate(gate, answer, by)
    }

    pub(super) fn settle_from_status_map(
        &mut self,
        status: Option<&Value>,
        observed_at: u64,
    ) -> Mapped {
        // The snapshot describes the work that was running when it was taken. A turn admitted
        // since then is not that work, and an `idle` about the old turn must not settle it.
        if observed_at != self.admissions {
            return Mapped::default();
        }
        let Some(status) = status else {
            return self.settle_turn();
        };
        match status.get("type").and_then(Value::as_str) {
            Some("idle") => self.settle_turn(),
            Some("busy") => Mapped {
                events: self
                    .state_changed(SessionState::Running)
                    .into_iter()
                    .collect(),
                ..Mapped::default()
            },
            Some("retry") => self.retry_status(status),
            Some(_) | None => Mapped::default(),
        }
    }

    fn is_for_other_session(&self, event: &WireEvent) -> bool {
        let session = event
            .properties
            .get("sessionID")
            .and_then(Value::as_str)
            .or_else(|| {
                event
                    .properties
                    .get("part")
                    .and_then(|part| part.get("sessionID"))
                    .and_then(Value::as_str)
            });
        session.is_some_and(|session| session != self.session_id)
    }

    fn message_updated(&mut self, properties: &Value) -> Mapped {
        let Some(info) = properties.get("info") else {
            return Mapped::default();
        };
        let Some(message_id) = info.get("id").and_then(Value::as_str) else {
            return Mapped::default();
        };
        let role = info.get("role").and_then(Value::as_str).unwrap_or_default();
        self.message_roles
            .insert(message_id.to_owned(), role.to_owned());
        if role != "assistant" {
            return Mapped::default();
        }

        let mut events = Vec::new();
        // §4.2 fixes the metadata row as `build agent · claude-sonnet-5 · high · asks before
        // edits`. A session created without an explicit model has none until the first
        // assistant message names the one the server actually chose.
        let named = (
            info.get("providerID").and_then(Value::as_str),
            info.get("modelID").and_then(Value::as_str),
        );
        if let (Some(provider), Some(model)) = named {
            let observed = (provider.to_owned(), model.to_owned());
            if self.model.as_ref() != Some(&observed) {
                self.model = Some(observed);
                events.push(AgentEvent::MetadataChanged {
                    title: None,
                    mode: None,
                    model: Some(ModelSelection {
                        model: model.to_owned(),
                        effort: None,
                        provider: Some(provider.to_owned()),
                    }),
                });
            }
        }
        if let Some(active) = self.active.as_mut() {
            if let Some(tokens) = info.get("tokens") {
                record_usage(active, usage(tokens));
            }
            if let Some(provider) = named.0 {
                active
                    .usage
                    .extra
                    .insert("providerID".to_owned(), json!(provider));
            }
            if let Some(model) = named.1 {
                active
                    .usage
                    .extra
                    .insert("modelID".to_owned(), json!(model));
            }
            active.cost_usd = info.get("cost").and_then(Value::as_f64).or(active.cost_usd);
            active.assistant_started_ms = info
                .pointer("/time/created")
                .and_then(Value::as_u64)
                .or(active.assistant_started_ms);
            active.assistant_completed_ms = info
                .pointer("/time/completed")
                .and_then(Value::as_u64)
                .or(active.assistant_completed_ms);
            if let Some(error) = info.get("error") {
                let message = error_message(error);
                let name = error.get("name").and_then(Value::as_str);
                let aborted = is_abort_error(name, &message);
                active.assistant_error_name = name.map(ToOwned::to_owned);
                if active.error.as_deref() != Some(message.as_str()) {
                    active.error = Some(message.clone());
                    // §11: an interrupted turn shows its footer and no error card. The abort is
                    // the shape §4.2 already settles as `TurnAborted`, so it is not an error.
                    if !aborted {
                        events.push(AgentEvent::RuntimeError {
                            fatal: false,
                            message,
                        });
                    }
                }
            }
            let (turn, usage, cost) = (active.id, active.usage.clone(), active.cost_usd);
            events.extend(self.token_usage(turn, usage, cost));
        }
        Mapped {
            events,
            ..Mapped::default()
        }
    }

    fn part_updated(&mut self, properties: &Value) -> Mapped {
        let Some(part) = properties.get("part") else {
            return Mapped::default();
        };
        match part.get("type").and_then(Value::as_str) {
            Some("text") => self.text_part(part, PartKind::Text),
            Some("reasoning") => self.text_part(part, PartKind::Reasoning),
            Some("tool") => self.tool_part(part),
            Some("patch") => self.patch_part(part),
            Some("compaction") => self.compaction_part(),
            Some("retry") => self.retry_part(part),
            Some("step-start") => Mapped::default(),
            Some("step-finish") => self.step_finish(part),
            _ => Mapped::default(),
        }
    }

    fn text_part(&mut self, part: &Value, kind: PartKind) -> Mapped {
        let Some(turn) = self.active.as_ref().map(|turn| turn.id) else {
            return Mapped::default();
        };
        let Some(part_id) = part.get("id").and_then(Value::as_str) else {
            return Mapped::default();
        };
        let message_id = part
            .get("messageID")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if message_id
            .as_ref()
            .and_then(|id| self.message_roles.get(id))
            .is_some_and(|role| role == "user")
        {
            return Mapped::default();
        }
        let text = part
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut events = Vec::new();
        if let Some(record) = self.parts.get_mut(part_id) {
            // §3.3 rule 3: a part the turn already closed stays closed, so a late cumulative
            // replacement cannot put a spinner back on a finished row.
            if record.completed {
                return Mapped::default();
            }
            if record.text != text {
                record.text.clone_from(&text);
                events.push(AgentEvent::ItemUpdated {
                    item: record.item,
                    patch: ItemPatch {
                        text: Some(text),
                        status: Some(ItemStatus::Running),
                        ..ItemPatch::default()
                    },
                });
            }
            return Mapped {
                events,
                ..Mapped::default()
            };
        }

        let item = ItemId::new();
        self.parts.insert(
            part_id.to_owned(),
            PartRecord {
                item,
                kind,
                text: text.clone(),
                completed: false,
                tool: None,
            },
        );
        self.open_item(item, ItemStatus::Running);
        if kind == PartKind::Text
            && let Some(active) = self.active.as_mut()
        {
            active.assistant_items.push(item);
        }
        events.push(AgentEvent::ItemStarted {
            turn,
            item,
            kind: match kind {
                PartKind::Text => ItemKind::AssistantText,
                PartKind::Reasoning => ItemKind::Thinking,
                PartKind::Tool => unreachable!("tool parts use tool_part"),
            },
            parent: None,
        });
        if !text.is_empty() {
            events.push(AgentEvent::ItemUpdated {
                item,
                patch: ItemPatch {
                    text: Some(text),
                    status: Some(ItemStatus::Running),
                    ..ItemPatch::default()
                },
            });
        }
        Mapped {
            events,
            ..Mapped::default()
        }
    }

    fn part_delta(&mut self, properties: &Value) -> Mapped {
        let Some(turn) = self.active.as_ref().map(|turn| turn.id) else {
            return Mapped::default();
        };
        let Some(part_id) = properties.get("partID").and_then(Value::as_str) else {
            return Mapped::default();
        };
        let field = properties
            .get("field")
            .and_then(Value::as_str)
            .unwrap_or("text");
        let Some(delta) = properties.get("delta").and_then(Value::as_str) else {
            return Mapped::default();
        };
        if field != "text" {
            let Some(record) = self.parts.get_mut(part_id) else {
                return Mapped::default();
            };
            // §3 rule 3: a part the turn already settled takes no more content, exactly as
            // `text_part` and `tool_part` refuse it.
            if record.completed {
                return Mapped::default();
            }
            record.text.push_str(delta);
            let item = record.item;
            let event = if record.kind == PartKind::Tool && field == "input" {
                serde_json::from_str(&record.text)
                    .ok()
                    .map(|input| AgentEvent::ItemUpdated {
                        item,
                        patch: ItemPatch {
                            input: Some(input),
                            ..ItemPatch::default()
                        },
                    })
            } else {
                Some(AgentEvent::ContentDelta {
                    item,
                    stream: if record.kind == PartKind::Tool {
                        StreamKind::ToolOutput
                    } else if record.kind == PartKind::Reasoning {
                        StreamKind::Reasoning
                    } else {
                        StreamKind::AssistantText
                    },
                    delta: delta.to_owned(),
                })
            };
            return Mapped {
                events: event.into_iter().collect(),
                ..Mapped::default()
            };
        }
        if self
            .parts
            .get(part_id)
            .is_some_and(|record| record.completed)
        {
            return Mapped::default();
        }
        let mut started = None;
        let record = self.parts.entry(part_id.to_owned()).or_insert_with(|| {
            let item = ItemId::new();
            started = Some(item);
            PartRecord {
                item,
                kind: PartKind::Text,
                text: String::new(),
                completed: false,
                tool: None,
            }
        });
        let item = record.item;
        record.text.push_str(delta);
        let stream = match record.kind {
            PartKind::Reasoning => StreamKind::Reasoning,
            PartKind::Tool => StreamKind::ToolOutput,
            PartKind::Text => StreamKind::AssistantText,
        };
        let mut events = Vec::new();
        if started.is_some() {
            self.open_item(item, ItemStatus::Running);
            if let Some(active) = self.active.as_mut() {
                active.assistant_items.push(item);
            }
            events.push(AgentEvent::ItemStarted {
                turn,
                item,
                kind: ItemKind::AssistantText,
                parent: None,
            });
        }
        events.push(AgentEvent::ContentDelta {
            item,
            stream,
            delta: delta.to_owned(),
        });
        Mapped {
            events,
            ..Mapped::default()
        }
    }

    fn tool_part(&mut self, part: &Value) -> Mapped {
        let Some(turn) = self.active.as_ref().map(|turn| turn.id) else {
            return Mapped::default();
        };
        let Some(part_id) = part.get("id").and_then(Value::as_str) else {
            return Mapped::default();
        };
        let tool_name = part
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let tool_kind = tool_kind(tool_name);
        let state = part.get("state").unwrap_or(&Value::Null);
        let status = state
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        let input = state.get("input").cloned().unwrap_or_else(|| json!({}));
        let message_id = part
            .get("messageID")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let mut events = Vec::new();
        let item = if let Some(record) = self.parts.get_mut(part_id) {
            if record.completed {
                return Mapped::default();
            }
            let item = record.item;
            // §4.2 makes the tool frame the owner of this id's kind. A `message.part.delta`
            // that beat its `message.part.updated` opened the id speculatively as assistant
            // prose, so the row is re-keyed here — same item, real `ItemStarted` — instead of
            // rendering a tool call as a paragraph with no tool row at all.
            if record.kind != PartKind::Tool {
                record.kind = PartKind::Tool;
                record.text.clear();
                self.open_item(item, ItemStatus::Pending);
                events.push(AgentEvent::ItemStarted {
                    turn,
                    item,
                    kind: ItemKind::Tool {
                        kind: tool_kind.clone(),
                        name: tool_name.to_owned(),
                        input: input.clone(),
                    },
                    parent: None,
                });
            }
            item
        } else {
            let item = ItemId::new();
            self.parts.insert(
                part_id.to_owned(),
                PartRecord {
                    item,
                    kind: PartKind::Tool,
                    text: String::new(),
                    completed: false,
                    tool: None,
                },
            );
            self.open_item(item, ItemStatus::Pending);
            events.push(AgentEvent::ItemStarted {
                turn,
                item,
                kind: ItemKind::Tool {
                    kind: tool_kind.clone(),
                    name: tool_name.to_owned(),
                    input: input.clone(),
                },
                parent: None,
            });
            item
        };
        if matches!(tool_kind, ToolKind::Edit | ToolKind::Write) {
            self.latest_edit_item = Some(item);
            if let Some(message_id) = message_id {
                self.edit_items_by_message.insert(message_id, item);
            }
        }

        let normalized = match status {
            "pending" => ItemStatus::Pending,
            "running" => ItemStatus::Running,
            "completed" => ItemStatus::Done,
            "error" => ItemStatus::Error,
            _ => ItemStatus::Running,
        };
        let title = state
            .get("title")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let output = state
            .get("output")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let error = state
            .get("error")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        // §2 and §5: the summary is what the tool did, the right-aligned result is how it went.
        // `title` is the summary; repeating it as the result would print the command twice and
        // drop the exit code and the elapsed time the canvas puts there.
        let snapshot = ToolSnapshot {
            input,
            summary: title.or_else(|| error.clone()),
            result: tool_result(state, normalized),
            output: output.or(error),
            status: normalized,
        };
        let unchanged = self
            .parts
            .get(part_id)
            .is_some_and(|record| record.tool.as_ref() == Some(&snapshot));
        if unchanged {
            return Mapped {
                events,
                ..Mapped::default()
            };
        }
        events.push(AgentEvent::ItemUpdated {
            item,
            patch: ItemPatch {
                input: Some(snapshot.input.clone()),
                summary: snapshot.summary.clone(),
                result: snapshot.result.clone(),
                output: snapshot.output.clone(),
                status: Some(normalized),
                ..ItemPatch::default()
            },
        });
        if let Some(record) = self.parts.get_mut(part_id) {
            record.tool = Some(snapshot);
        }
        self.item_status.insert(item, normalized);
        if matches!(normalized, ItemStatus::Done | ItemStatus::Error) {
            let completed = self
                .parts
                .get(part_id)
                .is_some_and(|record| record.completed);
            if !completed {
                if let Some(record) = self.parts.get_mut(part_id) {
                    record.completed = true;
                }
                self.close_item(item);
                events.push(AgentEvent::ItemCompleted {
                    item,
                    status: normalized,
                });
            }
        }
        Mapped {
            events,
            ..Mapped::default()
        }
    }

    fn patch_part(&mut self, part: &Value) -> Mapped {
        let message_id = part.get("messageID").and_then(Value::as_str);
        let item = message_id
            .and_then(|id| self.edit_items_by_message.get(id).copied())
            .or(self.latest_edit_item);
        let Some(item) = item else {
            return Mapped::default();
        };
        let path = part
            .get("files")
            .and_then(Value::as_array)
            .and_then(|files| files.first())
            .and_then(Value::as_str)
            .unwrap_or("patch");
        let unified = part
            .get("unified")
            .or_else(|| part.get("diff"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let (added, removed) = diff_counts(&unified);
        let path = PathBuf::from(path);
        if let Some(active) = self.active.as_mut() {
            let entry = active.files_changed.entry(path.clone()).or_default();
            entry.0 = entry.0.saturating_add(added);
            entry.1 = entry.1.saturating_add(removed);
        }
        Mapped {
            events: vec![AgentEvent::ItemUpdated {
                item,
                patch: ItemPatch {
                    diff: Some(ToolDiff {
                        path,
                        added,
                        removed,
                        unified,
                    }),
                    ..ItemPatch::default()
                },
            }],
            ..Mapped::default()
        }
    }

    fn compaction_part(&self) -> Mapped {
        let before = self
            .active
            .as_ref()
            .map(|turn| turn.usage.total_tokens)
            .unwrap_or_default();
        Mapped {
            events: vec![AgentEvent::Checkpoint(CheckpointKind::CompactBoundary {
                before,
                after: None,
            })],
            ..Mapped::default()
        }
    }

    fn retry_part(&mut self, part: &Value) -> Mapped {
        let attempt = part
            .get("attempt")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(1);
        let reason = part
            .get("error")
            .map(error_message)
            .unwrap_or_else(|| "OpenCode retry".to_owned());
        Mapped {
            events: self.retrying(attempt, 0, reason).into_iter().collect(),
            ..Mapped::default()
        }
    }

    fn step_finish(&mut self, part: &Value) -> Mapped {
        let Some(active) = self.active.as_mut() else {
            return Mapped::default();
        };
        if let Some(tokens) = part.get("tokens") {
            record_usage(active, usage(tokens));
        }
        active.cost_usd = part.get("cost").and_then(Value::as_f64).or(active.cost_usd);
        let (turn, usage, cost) = (active.id, active.usage.clone(), active.cost_usd);
        Mapped {
            events: self.token_usage(turn, usage, cost).into_iter().collect(),
            ..Mapped::default()
        }
    }

    fn todo_updated(&mut self, properties: &Value) -> Mapped {
        let Some(turn) = self.active.as_ref().map(|turn| turn.id) else {
            return Mapped::default();
        };
        let todos = properties
            .get("todos")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let total = todos.as_array().map_or(0, Vec::len);
        let completed = todos.as_array().map_or(0, |todos| {
            todos
                .iter()
                .filter(|todo| {
                    matches!(
                        todo.get("status").and_then(Value::as_str),
                        Some("completed" | "cancelled")
                    )
                })
                .count()
        });
        let mut events = Vec::new();
        let item = if let Some(item) = self.todo_item {
            item
        } else {
            let item = ItemId::new();
            self.todo_item = Some(item);
            self.open_item(item, ItemStatus::Running);
            events.push(AgentEvent::ItemStarted {
                turn,
                item,
                kind: ItemKind::Tool {
                    kind: ToolKind::Todo,
                    name: "todo".to_owned(),
                    input: json!({}),
                },
                parent: None,
            });
            item
        };
        events.push(AgentEvent::ItemUpdated {
            item,
            patch: ItemPatch {
                summary: Some(format!("{completed}/{total} completed")),
                output: Some(todos.to_string()),
                status: Some(ItemStatus::Running),
                ..ItemPatch::default()
            },
        });
        Mapped {
            events,
            ..Mapped::default()
        }
    }

    fn session_updated(&mut self, properties: &Value) -> Mapped {
        let title = properties
            .pointer("/info/title")
            .and_then(Value::as_str)
            .filter(|title| !title.is_empty())
            // §10 titles a tab from the first message or a real `Session.title`. OpenCode's
            // `New session - <timestamp>` is neither: it is the row the server writes before
            // its auto-titler has run, and a session it never renames would keep the timestamp
            // as its tab name forever.
            .filter(|title| !is_placeholder_title(title));
        let Some(title) = title else {
            return Mapped::default();
        };
        if self.title.as_deref() == Some(title) {
            return Mapped::default();
        }
        self.title = Some(title.to_owned());
        // §4.2: the tab's title comes from `Session.title`, so it is projected metadata rather
        // than a transcript line.
        Mapped {
            events: vec![AgentEvent::MetadataChanged {
                title: Some(title.to_owned()),
                mode: None,
                model: None,
            }],
            ..Mapped::default()
        }
    }

    fn session_error(&mut self, properties: &Value) -> Mapped {
        let error = properties.get("error");
        let message = error
            .map(error_message)
            .unwrap_or_else(|| "OpenCode session error".to_owned());
        let name = error
            .and_then(|error| error.get("name"))
            .and_then(Value::as_str);
        let mut is_new = true;
        if let Some(active) = self.active.as_mut() {
            is_new = active.error.as_deref() != Some(message.as_str());
            active.error = Some(message.clone());
            active.assistant_error_name = name.map(ToOwned::to_owned);
        }
        Mapped {
            events: (is_new && !is_abort_error(name, &message))
                .then_some(AgentEvent::RuntimeError {
                    fatal: false,
                    message,
                })
                .into_iter()
                .collect(),
            ..Mapped::default()
        }
    }

    fn session_status(&mut self, properties: &Value) -> Mapped {
        // A live SSE frame is observed and applied under the same lock, so it can never be
        // outrun by an admission: it is its own epoch.
        let observed_at = self.admissions;
        properties
            .get("status")
            .map_or_else(Mapped::default, |status| {
                self.settle_from_status_map(Some(status), observed_at)
            })
    }

    fn retry_status(&mut self, status: &Value) -> Mapped {
        let next = status
            .get("next")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis();
        let retry_in_ms = u64::try_from(now)
            .ok()
            .map_or(0, |now| next.saturating_sub(now));
        let attempt = status
            .get("attempt")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(1);
        let reason = status
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("OpenCode retry")
            .to_owned();
        let mut events = Vec::new();
        events.extend(self.state_changed(SessionState::Running));
        events.extend(self.retrying(attempt, retry_in_ms, reason));
        Mapped {
            events,
            ..Mapped::default()
        }
    }

    fn permission_asked(&mut self, properties: &Value) -> Mapped {
        let Some(provider_id) = properties.get("id").and_then(Value::as_str) else {
            return Mapped::default();
        };
        if self.provider_gates.contains_key(provider_id) {
            return Mapped::default();
        }
        let permission = properties
            .get("permission")
            .or_else(|| properties.get("action"))
            .and_then(Value::as_str)
            .unwrap_or("permission");
        let patterns = properties
            .get("patterns")
            .or_else(|| properties.get("resources"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let payload = properties
            .pointer("/metadata/command")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| patterns.join(" "));
        let title = if patterns.is_empty() {
            permission.to_owned()
        } else {
            format!("{permission}: {}", patterns.join(", "))
        };
        let gate = GateId::new();
        self.provider_gates.insert(provider_id.to_owned(), gate);
        self.gates.insert(
            gate,
            PendingGate {
                provider_id: provider_id.to_owned(),
                kind: PendingGateKind::Permission,
            },
        );
        let answer = GateAnswer::Permission {
            choice: PermissionChoice::AllowOnce,
            edited_payload: None,
        };
        let mut mapped = Mapped {
            events: vec![AgentEvent::GateOpened {
                gate,
                turn: self.active.as_ref().map(|turn| turn.id),
                kind: GateKind::Permission {
                    tool: tool_kind(permission),
                    title,
                    payload,
                    rationale: None,
                    options: vec![
                        PermissionOption {
                            id: ProviderOptionId("once".to_owned()),
                            label: PermissionChoice::AllowOnce,
                        },
                        PermissionOption {
                            id: ProviderOptionId("always".to_owned()),
                            label: PermissionChoice::AllowDirectory,
                        },
                        PermissionOption {
                            id: ProviderOptionId("reject".to_owned()),
                            label: PermissionChoice::Deny,
                        },
                    ],
                },
            }],
            ..Mapped::default()
        };
        if self.mode == PermissionMode::FullAccess {
            mapped.actions.push(MapAction::AutoPermission {
                gate,
                provider_id: provider_id.to_owned(),
                answer,
            });
        }
        mapped
    }

    fn question_asked(&mut self, properties: &Value) -> Mapped {
        let Some(provider_id) = properties.get("id").and_then(Value::as_str) else {
            return Mapped::default();
        };
        if self.provider_gates.contains_key(provider_id) {
            return Mapped::default();
        }
        let questions = properties
            .get("questions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|question| Question {
                text: question
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                header: question
                    .get("header")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                options: question
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
                    .collect(),
                multi_select: question
                    .get("multiple")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                allow_other: question
                    .get("custom")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
            .collect::<Vec<_>>();
        let gate = GateId::new();
        self.provider_gates.insert(provider_id.to_owned(), gate);
        self.gates.insert(
            gate,
            PendingGate {
                provider_id: provider_id.to_owned(),
                kind: PendingGateKind::Question,
            },
        );
        Mapped {
            events: vec![AgentEvent::GateOpened {
                gate,
                turn: self.active.as_ref().map(|turn| turn.id),
                kind: GateKind::Question { questions },
            }],
            ..Mapped::default()
        }
    }

    fn settle_turn(&mut self) -> Mapped {
        let Some(active) = self.active.take() else {
            return Mapped::default();
        };
        let mut events = Vec::new();
        for item in std::mem::take(&mut self.open_items) {
            let status = self
                .item_status
                .remove(&item)
                .filter(|status| matches!(status, ItemStatus::Error | ItemStatus::Denied))
                .unwrap_or(ItemStatus::Done);
            events.push(AgentEvent::ItemCompleted { item, status });
        }
        for part in self.parts.values_mut() {
            part.completed = true;
        }
        self.todo_item = None;

        let final_text = active
            .assistant_items
            .iter()
            .filter_map(|item| {
                self.parts
                    .values()
                    .find(|part| part.item == *item)
                    .map(|part| part.text.as_str())
            })
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        if active.agent == "plan" && !final_text.is_empty() {
            let gate = GateId::new();
            self.gates.insert(
                gate,
                PendingGate {
                    provider_id: String::new(),
                    kind: PendingGateKind::Plan,
                },
            );
            events.push(AgentEvent::GateOpened {
                gate,
                turn: Some(active.id),
                kind: GateKind::Plan {
                    steps: plan_steps(&final_text),
                    markdown: final_text,
                },
            });
        }

        let duration_ms = duration_ms(&active);
        let files_changed = active
            .files_changed
            .iter()
            .map(|(path, (added, removed))| FileDelta {
                path: path.clone(),
                added: *added,
                removed: *removed,
            })
            .collect::<Vec<_>>();
        // §4.3: either signal identifies an abort. The local abort is the authority — it is what
        // the user asked for — and `MessageAbortedError` corroborates it or stands on its own
        // when the abort came from elsewhere. Neither may settle as a completed or failed turn.
        let aborted_error = active.assistant_error_name.as_deref() == Some("MessageAbortedError");
        if active.abort_outstanding || aborted_error {
            events.push(AgentEvent::TurnAborted {
                turn: active.id,
                reason: if active.abort_outstanding {
                    AbortReason::User
                } else {
                    AbortReason::Other("MessageAbortedError".to_owned())
                },
            });
        } else {
            events.push(AgentEvent::TurnCompleted {
                turn: active.id,
                outcome: match active.error {
                    Some(message) => TurnOutcome::Error {
                        message: Some(message),
                    },
                    None => TurnOutcome::Completed,
                },
                usage: active.usage,
                duration_ms,
                files_changed,
            });
        }
        self.retry = None;
        self.usage = None;
        self.prune_settled_parts(&active.assistant_items);
        events.extend(self.state_changed(SessionState::Ready));
        Mapped {
            events,
            ..Mapped::default()
        }
    }

    /// Drops the per-part bookkeeping of a settled turn, so a long thread stays bounded.
    ///
    /// §6 expects a thread to live indefinitely and §4.2 keeps one adapter per thread for its
    /// whole life; `reconcile_messages` re-feeds every part of the last exchange on each poll,
    /// so nothing may be kept only because it was once seen.
    fn prune_settled_parts(&mut self, keep_items: &[ItemId]) {
        let open = &self.open_items;
        self.parts
            .retain(|_, part| !part.completed || open.contains(&part.item));
        let live: HashSet<ItemId> = self
            .parts
            .values()
            .map(|part| part.item)
            .chain(keep_items.iter().copied())
            .chain(self.open_items.iter().copied())
            .collect();
        self.edit_items_by_message
            .retain(|_, item| live.contains(item));
        self.message_roles.clear();
        self.item_status.retain(|item, _| live.contains(item));
        if let Some(item) = self.latest_edit_item
            && !live.contains(&item)
        {
            self.latest_edit_item = None;
        }
    }

    fn open_item(&mut self, item: ItemId, status: ItemStatus) {
        if !self.open_items.contains(&item) {
            self.open_items.push(item);
        }
        self.item_status.insert(item, status);
    }

    fn close_item(&mut self, item: ItemId) {
        self.open_items.retain(|open| *open != item);
        self.item_status.remove(&item);
    }

    #[cfg(test)]
    pub(super) fn part_text(&self, part: &str) -> Option<&str> {
        self.parts.get(part).map(|part| part.text.as_str())
    }
}

/// Adopts one usage observation, ignoring a mid-turn regression to all zeros.
///
/// §5 keeps turn metadata from moving under the reader, and OpenCode reports a fresh assistant
/// message with empty counters before the provider fills them in; taking that literally drops
/// the turn's tokens back to zero and then raises them again.
fn record_usage(turn: &mut TurnContext, observed: Usage) {
    if usage_is_zero(&observed) && !usage_is_zero(&turn.usage) {
        return;
    }
    let extra = std::mem::take(&mut turn.usage.extra);
    turn.usage = Usage { extra, ..observed };
}

fn usage_is_zero(usage: &Usage) -> bool {
    usage.total_tokens == 0
        && usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.reasoning_tokens == 0
        && usage.cache_read_tokens == 0
        && usage.cache_write_tokens == 0
}

fn usage(value: &Value) -> Usage {
    let input = value
        .get("input")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output = value
        .get("output")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let reasoning = value
        .get("reasoning")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cache_read = value
        .pointer("/cache/read")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let cache_write = value
        .pointer("/cache/write")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    Usage {
        input_tokens: input,
        output_tokens: output,
        reasoning_tokens: reasoning,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        total_tokens: value
            .get("total")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| input.saturating_add(output).saturating_add(reasoning)),
        ..Usage::default()
    }
}

/// The right-aligned result of one tool row: `exit 0 · 1.2s` for a command, `1.2s` otherwise.
///
/// §5's table gives Bash `exit 0 · 1.2s`; `ToolState` carries the exit status in the open
/// `metadata` map and the elapsed time in `time.start`/`time.end`. A row that has neither yet —
/// a pending or a running call — states nothing rather than repeating its own summary.
fn tool_result(state: &Value, status: ItemStatus) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(exit) = state
        .pointer("/metadata/exit")
        .or_else(|| state.pointer("/metadata/exitCode"))
        .and_then(Value::as_i64)
    {
        parts.push(format!("exit {exit}"));
    } else if status == ItemStatus::Error {
        parts.push("error".to_owned());
    }
    if let (Some(start), Some(end)) = (
        state.pointer("/time/start").and_then(Value::as_u64),
        state.pointer("/time/end").and_then(Value::as_u64),
    ) {
        let elapsed = end.saturating_sub(start);
        parts.push(format!("{:.1}s", elapsed as f64 / 1000.0));
    }
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
}

/// Whether a `Session.title` is OpenCode's own `New session - <ISO timestamp>` placeholder.
fn is_placeholder_title(title: &str) -> bool {
    let Some(rest) = title.strip_prefix("New session - ") else {
        return false;
    };
    // The suffix is an ISO-8601 instant; anything else under that prefix is a real title.
    chrono::DateTime::parse_from_rfc3339(rest.trim()).is_ok()
}

/// Whether a usage frame carries no counter at all, which reports nothing worth a sequence.
fn usage_is_empty(usage: &Usage) -> bool {
    usage.total_tokens == 0
        && usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.reasoning_tokens == 0
        && usage.cache_read_tokens == 0
        && usage.cache_write_tokens == 0
}

/// Whether an OpenCode error is the ordinary abort §4.2 already settles as `TurnAborted`.
///
/// §11 gives an interrupted turn its footer and *no* error card, so this shape never becomes a
/// `RuntimeError`: the abort is the outcome, not a failure on top of it.
fn is_abort_error(name: Option<&str>, message: &str) -> bool {
    name == Some("MessageAbortedError") || message.eq_ignore_ascii_case("aborted")
}

fn error_message(error: &Value) -> String {
    error
        .pointer("/data/message")
        .or_else(|| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("OpenCode reported an unknown error")
        .to_owned()
}

fn duration_ms(turn: &TurnContext) -> u64 {
    match (turn.assistant_started_ms, turn.assistant_completed_ms) {
        (Some(start), Some(end)) => end.saturating_sub(start),
        _ => u64::try_from(turn.started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

fn tool_kind(name: &str) -> ToolKind {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "read" => ToolKind::Read,
        "edit" | "apply_patch" => ToolKind::Edit,
        "write" => ToolKind::Write,
        "bash" | "shell" => ToolKind::Bash,
        "grep" => ToolKind::Grep,
        "glob" | "search" | "list" => ToolKind::Search,
        "fetch" | "webfetch" | "web_fetch" => ToolKind::Fetch,
        "agent" | "task" | "subtask" => ToolKind::Agent,
        "todo" | "todowrite" | "todo_write" => ToolKind::Todo,
        "skill" => ToolKind::Skill,
        _ if lower.starts_with("mcp__") => ToolKind::Mcp {
            server: lower
                .trim_start_matches("mcp__")
                .split("__")
                .next()
                .unwrap_or("unknown")
                .to_owned(),
        },
        _ => ToolKind::Unknown {
            name: name.to_owned(),
        },
    }
}

fn diff_counts(unified: &str) -> (u64, u64) {
    unified.lines().fold((0, 0), |(added, removed), line| {
        if line.starts_with('+') && !line.starts_with("+++") {
            (added.saturating_add(1), removed)
        } else if line.starts_with('-') && !line.starts_with("---") {
            (added, removed.saturating_add(1))
        } else {
            (added, removed)
        }
    })
}

fn plan_steps(markdown: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    markdown
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))
                .or_else(|| {
                    let (number, text) = line.split_once(". ")?;
                    number.chars().all(|ch| ch.is_ascii_digit()).then_some(text)
                })
        })
        .filter(|step| seen.insert((*step).to_owned()))
        .map(ToOwned::to_owned)
        .collect()
}

pub(super) fn plan_answer_note(answer: &GateAnswer) -> Option<&str> {
    match answer {
        GateAnswer::Plan(PlanAnswer::AskForChanges { note }) => Some(note),
        _ => None,
    }
}
