//! Claude session, turn, item and gate state.
//!
//! One instance per thread, owned by the adapter and mutated only by the reader task and the
//! adapter's own verbs. It outlives the child process (the adapter owns it across a restart with
//! resume), which is why [`ClaudeSession::process_exit`] clears the active turn: a stale one
//! would make every later send fail against a process that can never answer.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::PathBuf,
};

use fleet_core::agents::{
    AgentEvent, ControlCost, GateAnswer, GateId, GateResolver, HarnessCapabilities,
    InterruptSupport, ItemId, ItemKind, ItemPatch, ItemPayloadPatch, ItemStatus, LifecycleKind,
    PermissionChoice, ReasoningChannels, ResumeSupport, SandboxAxes, SteerSupport, ToolCall,
    ToolPatch, TurnId, Usage, should_apply_lifecycle,
};
use semver::Version;
use serde_json::Value;

use crate::agents::harness::{HarnessError, HarnessResult};

use super::map::tools::{tool_kind, tool_summary};

/// A capability string Claude publishes on `system/init`.
///
/// Absent `msg_lifecycle_v1` Fleet **refuses**: the message lifecycle framing the whole item
/// model depends on is missing, and half-working is worse than the terminal fallback (§4.5).
pub const CAPABILITY_MSG_LIFECYCLE: &str = "msg_lifecycle_v1";
/// Absent this, interrupts are unacknowledged; Fleet relies on the eventual `result`. Degrade.
pub const CAPABILITY_INTERRUPT_RECEIPT: &str = "interrupt_receipt_v1";
/// Absent this, Stop cannot cancel queued steers, **and the Stop affordance says so**.
pub const CAPABILITY_INTERRUPT_CANCEL_QUEUED: &str = "interrupt_cancel_queued_v1";

/// An item that has not settled yet.
#[derive(Debug, Clone)]
pub(super) struct OpenItem {
    /// The turn the item was started in; only that turn's terminal may close it.
    pub(super) turn: TurnId,
    pub(super) status: ItemStatus,
    pub(super) tool_name: Option<String>,
    pub(super) input: Value,
}

/// Which kind of content block a streaming block is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockKind {
    Text,
    Thinking,
    Tool,
    Unknown,
}

/// A streaming content block is addressed by its owner plus its index: a subagent stream has its
/// own index space starting at zero, so the index alone collides with the main stream.
pub(super) type BlockKey = (Option<String>, u64);

/// One in-flight streaming content block.
#[derive(Debug, Clone)]
pub(super) struct StreamBlock {
    pub(super) item: ItemId,
    pub(super) kind: BlockKind,
    pub(super) tool_name: Option<String>,
    pub(super) input_json: String,
    pub(super) text: String,
    pub(super) completed: bool,
    /// Fingerprint of the last parsed tool input, so an `ItemUpdated` costs a frame only when
    /// the parsed document actually changed — ~1 event per tool call instead of ~30.
    pub(super) input_fingerprint: Option<u64>,
}

/// A gate awaiting a Fleet answer.
#[derive(Debug, Clone)]
pub(super) struct PendingGate {
    pub(super) request_id: String,
    /// The tool the gate is about, which is what a whole-tool session rule names.
    pub(super) tool_name: String,
    pub(super) tool_use_id: Option<String>,
    pub(super) input: Value,
    pub(super) suggestions: Vec<Value>,
    pub(super) shape: GateShape,
}

/// What kind of answer a pending gate takes.
#[derive(Debug, Clone)]
pub(super) enum GateShape {
    Permission,
    Question(Vec<WireQuestion>),
    Plan,
}

/// A question as the CLI phrased it. The answer map is keyed by this exact text.
#[derive(Debug, Clone)]
pub(super) struct WireQuestion {
    pub(super) text: String,
    pub(super) multi_select: bool,
}

/// Everything one Claude thread knows about its own session.
#[derive(Debug, Default)]
pub(super) struct ClaudeSession {
    pub(super) initialized: bool,
    /// The capability strings `system/init` published. The version gate, and the only one.
    pub(super) declared: BTreeSet<String>,
    /// The resume cursor, updated from every **durable** frame.
    pub(super) cursor: Option<String>,
    /// The model `system/init` named, updated on a restart.
    ///
    /// `modelUsage` is cumulative across the query process and includes pipeline subcalls, so
    /// picking an arbitrary entry from it reports a summariser's window as the context meter.
    pub(super) session_model: Option<String>,
    /// The effort Fleet launched with. Claude does not echo it, so this is the only record.
    pub(super) launched_effort: Option<String>,
    pub(super) active_turn: Option<TurnId>,
    /// A submit Fleet has written that the CLI has not confirmed yet.
    pub(super) pending_start: Option<TurnId>,
    pub(super) last_turn: Option<TurnId>,
    pub(super) interrupted: HashSet<TurnId>,
    /// Steer writes issued into the active turn that the CLI has not settled yet.
    pub(super) pending_steers: u32,
    pub(super) stream_blocks: HashMap<BlockKey, StreamBlock>,
    pub(super) current_message: Option<String>,
    pub(super) open_items: HashMap<ItemId, OpenItem>,
    pub(super) tool_items: HashMap<String, ItemId>,
    pub(super) task_items: HashMap<String, ItemId>,
    pub(super) background_tasks: HashSet<String>,
    pub(super) gates: HashMap<GateId, PendingGate>,
    pub(super) files_changed: BTreeMap<PathBuf, (u64, u64)>,
    pub(super) tool_uses: u64,
    /// The last non-zero cumulative usage this process reported.
    ///
    /// Claude's counters are cumulative for the process — take the latest, never sum — and an
    /// aborted `result` reports them as all zeros, which is a gap in the frame, not a cheaper
    /// turn.
    pub(super) last_usage: Option<Usage>,
    pub(super) last_context_tokens: Option<u64>,
    /// The last completed assistant paragraph, which is the plan when `ExitPlanMode` omits it.
    pub(super) last_assistant_text: Option<String>,
    /// Usage windows already announced for the current turn, keyed `<window>:<resets_at>`.
    ///
    /// A parked window re-fires as the wait shrinks and a turn can park on more than one window,
    /// so the announcement is deduplicated per turn rather than per session.
    pub(super) announced_windows: HashSet<String>,
    /// A failure latch that only speaks when the `result` names no cause of its own.
    pub(super) failure_latch: Option<String>,
    /// The turn a `/compact` prompt was sent as, until its boundary lands.
    ///
    /// Claude has no compaction RPC, so compaction is a slash command whose completion has to be
    /// watched for: a `/compact` turn that settles with no `system/compact_boundary` would
    /// otherwise leave the thread marked compacting forever.
    pub(super) compacting: Option<TurnId>,
}

impl ClaudeSession {
    /// The capabilities of this process, given what the probe and `system/init` published.
    pub(super) fn capabilities(&self, version: Version) -> HarnessCapabilities {
        HarnessCapabilities {
            version,
            resume: ResumeSupport::ByCursor { fork: true },
            // A second `user` line is folded into the running turn by the CLI itself: implicit,
            // with no compare-and-swap to guard it.
            steer: SteerSupport::Implicit,
            interrupt: InterruptSupport::Receipted {
                cancel_queued: self.declared.contains(CAPABILITY_INTERRUPT_CANCEL_QUEUED),
            },
            history_readback: false,
            native_compaction: false,
            turn_diff: false,
            attention_flags: false,
            async_questions: false,
            secret_answers: false,
            sandbox_axes: SandboxAxes::One,
            reasoning_channels: ReasoningChannels::One,
            // Claude publishes the context-window denominator only on `result`, so the meter
            // cannot move during a turn. Faking one would be an invented number (§4.3).
            live_context_meter: false,
            model_switch: ControlCost::RestartWithResume,
            effort_switch: ControlCost::RestartWithResume,
            mode_switch: ControlCost::RestartWithResume,
            declared: self.declared.clone(),
        }
    }

    /// Whether the interrupt receipt is worth waiting for.
    pub(super) fn expects_interrupt_receipt(&self) -> bool {
        self.declared.contains(CAPABILITY_INTERRUPT_RECEIPT)
    }

    /// Whether Stop may also cancel queued steer messages.
    pub(super) fn can_cancel_queued(&self) -> bool {
        self.declared.contains(CAPABILITY_INTERRUPT_CANCEL_QUEUED)
    }

    /// Adopts a resume cursor from a durable frame.
    ///
    /// Returns whether the durable id actually changed, because `ThreadStarted` is emitted only
    /// then — a resumed session must emit exactly one.
    pub(super) fn adopt_cursor(&mut self, session_id: &str) -> bool {
        if session_id.is_empty() || self.cursor.as_deref() == Some(session_id) {
            return false;
        }
        self.cursor = Some(session_id.to_owned());
        true
    }

    /// The active turn, if any.
    pub(super) const fn active_turn(&self) -> Option<TurnId> {
        self.active_turn
    }

    /// Opens a turn for a fresh submit, or answers `None` when the write is a steer.
    ///
    /// The CLI coalesces a second `user` line into the **same** turn, so a steer starts nothing:
    /// no new turn id, no second turn-start, and the settling `result` reports how many were
    /// folded in.
    pub(super) fn begin_turn(
        &mut self,
        turn: TurnId,
        user_item: ItemId,
    ) -> HarnessResult<Option<AgentEvent>> {
        match self.active_turn {
            Some(active) if active != turn => Err(HarnessError::Request {
                method: "submit".to_owned(),
                code: None,
                detail: format!("turn {turn} was submitted while turn {active} is active"),
            }),
            Some(_) => Ok(None),
            None => {
                self.active_turn = Some(turn);
                self.pending_start = Some(turn);
                self.last_turn = Some(turn);
                self.announced_windows.clear();
                Ok(Some(AgentEvent::TurnStarted { turn, user_item }))
            }
        }
    }

    /// Un-announces a turn whose prompt could not be written.
    pub(super) fn rollback_turn_start(&mut self, turn: TurnId) {
        if self.active_turn == Some(turn) {
            self.active_turn = None;
            self.pending_start = None;
            self.pending_steers = 0;
        }
    }

    /// Records a steer write the CLI has accepted but not yet answered.
    ///
    /// The CLI supersedes the in-flight stream when a steer arrives and emits a `result` with an
    /// aborted terminal reason for it. That result is not the turn's terminal.
    pub(super) fn record_steer(&mut self, turn: TurnId) {
        if self.active_turn == Some(turn) {
            self.pending_steers = self.pending_steers.saturating_add(1);
        }
    }

    /// Marks a turn as user-interrupted so its `result` settles as an interruption.
    pub(super) fn mark_interrupted(&mut self, turn: TurnId) -> HarnessResult<()> {
        if self.active_turn != Some(turn) {
            return Err(HarnessError::Request {
                method: "interrupt".to_owned(),
                code: None,
                detail: format!("turn {turn} is not the running turn"),
            });
        }
        self.interrupted.insert(turn);
        Ok(())
    }

    /// Whether a settlement Fleet attributes to its own active turn may be applied.
    ///
    /// Claude's `result` names no turn, so the *adapter* supplies the attribution and this guard
    /// decides whether it may: with no locally-tracked turn it answers `false`, which is what
    /// turns a stray `result` into usage plus a tripwire instead of a settlement for a turn that
    /// never existed.
    pub(super) fn may_settle(&self) -> bool {
        should_apply_lifecycle(
            LifecycleKind::TurnSettled,
            self.active_turn,
            self.active_turn,
            self.pending_start,
        )
    }

    /// Settles the session for a process that is gone.
    pub(super) fn process_exit(&mut self, code: Option<i32>, expected: bool) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        if !expected && self.active_turn.is_some() {
            events.push(AgentEvent::RuntimeError {
                fatal: true,
                message: "Claude Code exited before returning a result for the active turn"
                    .to_owned(),
            });
        }
        events.push(AgentEvent::SessionExited { code, expected });
        self.active_turn = None;
        self.pending_start = None;
        self.pending_steers = 0;
        self.stream_blocks.clear();
        events
    }

    /// Starts a transcript item.
    pub(super) fn start_item(
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
                status: ItemStatus::InProgress,
                tool_name,
                input,
            },
        );
        item
    }

    /// Starts a tool item, with its kind column and one-line summary already computed.
    pub(super) fn start_tool(
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
            ItemKind::Tool(Box::new(ToolCall {
                kind: tool_kind(name),
                name: name.to_owned(),
                input: input.clone(),
                summary: Some(summary.clone()),
                result: None,
                output: String::new(),
                diff: None,
                exit_code: None,
                duration_ms: None,
                extra: BTreeMap::new(),
            })),
            parent,
            Some(name.to_owned()),
            input,
            events,
        );
        events.push(AgentEvent::ItemUpdated {
            item,
            patch: ItemPatch {
                payload: Some(ItemPayloadPatch::Tool(Box::new(ToolPatch {
                    summary: Some(summary),
                    ..ToolPatch::default()
                }))),
                status: Some(ItemStatus::InProgress),
            },
        });
        if !provider_id.is_empty() {
            self.tool_items.insert(provider_id.to_owned(), item);
        }
        item
    }

    /// Settles an item, once.
    pub(super) fn complete_item(
        &mut self,
        item: ItemId,
        status: ItemStatus,
        events: &mut Vec<AgentEvent>,
    ) {
        if let Some(open) = self.open_items.get_mut(&item) {
            if matches!(
                open.status,
                ItemStatus::Completed | ItemStatus::Failed | ItemStatus::Denied
            ) {
                return;
            }
            open.status = status;
            events.push(AgentEvent::ItemCompleted { item, status });
        }
    }

    /// Drops every item the settled turn closed, so the three id maps stay bounded.
    ///
    /// Anything still open — a background task that outlives its turn — is kept, because its
    /// later `task_notification` still has to find the row it belongs to.
    pub(super) fn prune_settled_items(&mut self) {
        self.open_items.retain(|_, open| {
            !matches!(
                open.status,
                ItemStatus::Completed | ItemStatus::Failed | ItemStatus::Denied
            )
        });
        self.tool_items
            .retain(|_, item| self.open_items.contains_key(item));
        self.task_items
            .retain(|_, item| self.open_items.contains_key(item));
    }

    /// The plan recorded on the assistant `tool_use` block a control request names.
    pub(super) fn recorded_plan(&self, tool_use_id: Option<&str>) -> Option<String> {
        let item = self.tool_items.get(tool_use_id?).copied()?;
        let plan = self.open_items.get(&item)?.input.get("plan")?.as_str()?;
        (!plan.trim().is_empty()).then(|| plan.to_owned())
    }

    /// Closes every open gate with the answer a torn-down session owes it.
    ///
    /// A gate nobody can answer must not stay open, or the thread can never settle.
    pub(super) fn settle_open_gates(&mut self, by: GateResolver) -> Vec<AgentEvent> {
        let gates = std::mem::take(&mut self.gates);
        gates
            .into_iter()
            .map(|(gate, pending)| AgentEvent::GateResolved {
                gate,
                answer: closed_answer(&pending.shape),
                by,
            })
            .collect()
    }

    /// Removes an answered gate and reports the resolution.
    pub(super) fn resolve_gate(
        &mut self,
        gate: GateId,
        answer: GateAnswer,
    ) -> HarnessResult<AgentEvent> {
        if self.gates.remove(&gate).is_none() {
            return Err(HarnessError::GateGone { gate });
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
}

/// The answer a gate gets when nobody can answer it any more.
pub(super) fn closed_answer(shape: &GateShape) -> GateAnswer {
    match shape {
        GateShape::Permission => GateAnswer::Permission {
            choice: PermissionChoice::Deny,
            edited_payload: None,
        },
        GateShape::Question(_) => GateAnswer::Question {
            answers: Vec::new(),
        },
        GateShape::Plan => GateAnswer::Plan(fleet_core::agents::PlanAnswer::AskForChanges {
            note: String::new(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_send_into_a_running_turn_is_a_steer_and_starts_nothing() {
        let mut session = ClaudeSession::default();
        let turn = TurnId::new();
        assert!(
            session
                .begin_turn(turn, ItemId::new())
                .unwrap_or_else(|error| panic!("{error}"))
                .is_some()
        );
        assert!(
            session
                .begin_turn(turn, ItemId::new())
                .unwrap_or_else(|error| panic!("{error}"))
                .is_none()
        );
        // A *different* turn while one runs is a caller bug, not a steer.
        assert!(session.begin_turn(TurnId::new(), ItemId::new()).is_err());
    }

    #[test]
    fn a_result_with_no_local_turn_never_settles_anything() {
        let mut session = ClaudeSession::default();
        assert!(!session.may_settle(), "no turn: nothing to settle");
        let turn = TurnId::new();
        session
            .begin_turn(turn, ItemId::new())
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(session.may_settle());
    }

    #[test]
    fn an_unexpected_exit_releases_the_turn_so_the_thread_can_be_resumed() {
        let mut session = ClaudeSession::default();
        let turn = TurnId::new();
        session
            .begin_turn(turn, ItemId::new())
            .unwrap_or_else(|error| panic!("{error}"));
        let events = session.process_exit(Some(137), false);
        assert!(matches!(
            events.first(),
            Some(AgentEvent::RuntimeError { fatal: true, .. })
        ));
        assert!(matches!(
            events.last(),
            Some(AgentEvent::SessionExited {
                code: Some(137),
                expected: false
            })
        ));
        assert!(session.active_turn().is_none());
        // The next send is accepted rather than refused against a dead process.
        assert!(session.begin_turn(TurnId::new(), ItemId::new()).is_ok());
    }

    #[test]
    fn capabilities_degrade_with_the_declared_list_rather_than_a_version_compare() {
        let mut session = ClaudeSession::default();
        assert_eq!(
            session.capabilities(Version::new(2, 1, 266)).interrupt,
            InterruptSupport::Receipted {
                cancel_queued: false
            },
            "an undeclared cancel_queued must not be advertised"
        );
        session
            .declared
            .insert(CAPABILITY_INTERRUPT_CANCEL_QUEUED.to_owned());
        assert_eq!(
            session.capabilities(Version::new(2, 1, 266)).interrupt,
            InterruptSupport::Receipted {
                cancel_queued: true
            }
        );
    }

    #[test]
    fn the_cursor_only_changes_when_the_durable_id_does() {
        let mut session = ClaudeSession::default();
        assert!(session.adopt_cursor("abc"));
        assert!(!session.adopt_cursor("abc"));
        assert!(!session.adopt_cursor(""));
        assert!(session.adopt_cursor("def"));
    }
}
