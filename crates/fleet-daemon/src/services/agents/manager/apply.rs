//! The write path: settle against the reducer, persist, apply, publish.
//!
//! [`apply_event`] is the only place in the daemon that mints a `Seq`, and the order of its three
//! steps is the invariant everything else depends on (`docs/NATIVE-AGENTS.md` §6):
//!
//! 1. **Settle.** `ThreadProjection::accepts` refuses an event the reducer would not take, before
//!    anything is written. A rejection costs the event and nothing else.
//! 2. **Persist.** The store commits the log row, every projection row it produces, and the head
//!    bump in one transaction, and the `await` resolves strictly after `COMMIT`.
//! 3. **Apply.** Only then does memory move. A write that fails therefore leaves `last_seq`
//!    exactly where it was, instead of advancing memory past a sequence the log never received
//!    and making every later append discontinuous.
//!
//! The state lock is released across step 2, because it is a `std::sync::Mutex` and holding one
//! across an `.await` is exactly what `rust-async-background-work` forbids. What makes that safe
//! is not a convention: every function here takes the thread's serialized-operation guard as an
//! argument, so the compiler will not let a caller reach the write path without holding it. Two
//! appends interleaving between steps 1 and 2 would both mint the same sequence, and the log's
//! `UNIQUE (thread_id, seq)` would abort one of their transactions.
//!
//! Releasing the guard is deliberate and the lint below is what keeps it from being reintroduced
//! by accident: the old store held a `std::sync::Mutex` across four `std::fs` syscalls on a tokio
//! worker for every streamed token, which is `rust-async-background-work` Rule 10 exactly.
#![deny(clippy::await_holding_lock)]

use anyhow::Context;
use chrono::Utc;
use fleet_core::agents::{
    AgentEvent, AgentThreadSummary, GateAnswer, GateKind, ItemId, ItemKind, PermissionChoice,
    PlanAnswer, Seq, SeqEvent, SessionState, ToolKind, TurnId, TurnOutcome, TurnState, UserInput,
};
use fleet_proto::event::Event;
use tokio::sync::MutexGuard;

use super::{
    AgentThreadRecord, ManagerInner,
    thread::{AppliedEvent, ThreadRuntime},
};

/// Proof, checked by the compiler, that the caller holds one thread's serialized-operation gate.
pub(super) type Serialized<'guard> = &'guard MutexGuard<'guard, ()>;

/// Sequences one event, makes it durable, applies it to memory, and reports what changed.
///
/// See the module docs for why the three steps are in this order and why the guard is a
/// parameter.
pub(super) async fn apply_event(
    inner: &ManagerInner,
    runtime: &ThreadRuntime,
    _serialized: Serialized<'_>,
    event: AgentEvent,
    raw: Option<String>,
) -> anyhow::Result<AppliedEvent> {
    let (thread, before, sequenced) = {
        let state = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sequenced = SeqEvent {
            seq: state.projection.last_seq.next(),
            at: Utc::now(),
            raw: raw.or_else(|| Some(event_name(&event).to_owned())),
            event,
        };
        state
            .projection
            .accepts(&sequenced)
            .with_context(|| format!("apply native-agent event {}", sequenced.seq))?;
        (
            state.record.thread,
            state.projection.summary(Seq::default()),
            sequenced,
        )
    };
    inner
        .store()?
        .append(thread, &sequenced)
        .await
        .context("persist native-agent event")?;
    let (applied, record, persist_record) = {
        let mut state = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .projection
            .apply(&sequenced)
            .with_context(|| format!("apply native-agent event {}", sequenced.seq))?;
        // Reborrowed so `record` and `title` are two disjoint field borrows: taking them both
        // through the guard would need a clone of the whole transcript per applied event.
        let state = &mut *state;
        let previous = state.record.clone();
        update_record(&mut state.record, &sequenced, &state.projection.title);
        if ends_the_turn(&sequenced.event) {
            state.inflight_turn = None;
        }
        // The gate is settled everywhere now, so the answered-once guard can forget it.
        if let AgentEvent::GateResolved { gate, .. } = &sequenced.event {
            state.answered_gates.remove(gate);
        }
        let after = state.projection.summary(Seq::default());
        let persist_record = record_metadata_changed(&previous, &state.record);
        (
            AppliedEvent {
                event: sequenced,
                summary: summary_transition(&before, &after).then_some(after),
            },
            state.record.clone(),
            persist_record,
        )
    };
    // The record is thread *metadata*, not a second event log. It is now one row and one
    // statement rather than a whole-file rewrite, but it is still a separate commit, so
    // `last_activity` alone rides along with the next real metadata transition instead of
    // costing a streaming turn a write per delta — every consumer reads the log for the rest.
    if persist_record && let Err(error) = inner.store()?.write_record(&record).await {
        tracing::warn!(%error, thread = %record.thread, "could not update a native-agent thread record");
    }
    Ok(applied)
}

/// Whether a record changed in a way the durable row has to learn about now.
///
/// `last_activity` moves on every event and is deliberately excluded: it is a listing nicety, the
/// projector already advances `threads.last_activity_at` inside the append's own transaction, and
/// the in-memory copy is persisted anyway by the next transition that matters.
fn record_metadata_changed(before: &AgentThreadRecord, after: &AgentThreadRecord) -> bool {
    before.title != after.title
        || before.resume_cursor != after.resume_cursor
        || before.model != after.model
        || before.mode != after.mode
        || before.last_outcome != after.last_outcome
}

/// Broadcasts one applied event and, when it moved the list row, the new summary.
pub(super) fn publish_applied(
    inner: &ManagerInner,
    runtime: &ThreadRuntime,
    applied: AppliedEvent,
) {
    let thread = runtime
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record
        .thread;
    inner.events.publish(Event::Agent {
        thread,
        event: applied.event,
    });
    if let Some(summary) = applied.summary {
        inner.events.publish(Event::AgentSummary(summary));
    }
}

/// The `ItemStarted` that records what the user typed.
///
/// `steered` is the harness's own answer to "did this message join a turn that was already
/// running?" ([`crate::agents::harness::Submitted::queued`]), never a guess from the projection:
/// §7.2 marks a steer with a leading `↳` and the mark has to survive a reload.
pub(super) fn user_item_started(
    turn: TurnId,
    item: ItemId,
    input: UserInput,
    steered: bool,
) -> AgentEvent {
    AgentEvent::ItemStarted {
        turn,
        item,
        kind: ItemKind::UserMessage {
            text: input.text,
            attachments: input.attachments,
            steered,
        },
        parent: None,
    }
}

/// The worktree-relative or absolute paths an edit-shaped tool call is about to write.
///
/// Provider-neutral by reading both shapes rather than by branching on the harness: Codex's
/// `fileChange` item maps to `{"paths": [...]}` and Claude's `Edit`/`Write`/`NotebookEdit` to a
/// single `file_path`. A tool that names no path yields nothing, and nothing is captured — which
/// is the same outcome as a capture that fails.
pub(super) fn edited_paths(event: &AgentEvent) -> Option<(TurnId, Vec<String>)> {
    let AgentEvent::ItemStarted { turn, kind, .. } = event else {
        return None;
    };
    let ItemKind::Tool(call) = kind else {
        return None;
    };
    if !matches!(call.kind, ToolKind::Edit | ToolKind::Write) {
        return None;
    }
    let mut paths = Vec::new();
    if let Some(listed) = call.input.get("paths").and_then(|value| value.as_array()) {
        paths.extend(
            listed
                .iter()
                .filter_map(|value| value.as_str())
                .map(ToOwned::to_owned),
        );
    }
    for key in ["file_path", "notebook_path", "path"] {
        if let Some(path) = call.input.get(key).and_then(|value| value.as_str()) {
            paths.push(path.to_owned());
        }
    }
    (!paths.is_empty()).then_some((*turn, paths))
}

/// The turn this thread is working on, projected or merely in flight.
pub(super) fn runtime_inflight(runtime: &ThreadRuntime) -> Option<TurnId> {
    let state = runtime
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match state.projection.turn {
        TurnState::Running(turn) => Some(turn),
        _ => state.inflight_turn,
    }
}

/// The turn the deque is still holding prompts for, when it holds any.
pub(super) fn pending_input_turn(runtime: &ThreadRuntime) -> Option<TurnId> {
    runtime
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pending_inputs
        .front()
        .map(|(turn, _)| *turn)
}

/// Folds one applied event into the durable record.
pub(super) fn update_record(record: &mut AgentThreadRecord, event: &SeqEvent, title: &str) {
    if record.title != title {
        record.title = title.to_owned();
    }
    record.last_activity = event.at;
    match &event.event {
        AgentEvent::SessionConfigured {
            resume_cursor,
            model,
            mode,
            ..
        } => {
            if resume_cursor.is_some() {
                record.resume_cursor.clone_from(resume_cursor);
            }
            record.model.clone_from(model);
            record.mode = *mode;
        }
        AgentEvent::MetadataChanged { mode, model, .. } => {
            if let Some(mode) = mode {
                record.mode = *mode;
            }
            if model.is_some() {
                record.model.clone_from(model);
            }
        }
        AgentEvent::TurnSettled { outcome, .. } => {
            record.last_outcome = Some(outcome.clone());
        }
        AgentEvent::TurnAborted { .. } => {
            record.last_outcome = Some(TurnOutcome::Interrupted);
        }
        AgentEvent::SessionExited {
            expected: false, ..
        }
        | AgentEvent::RuntimeError { fatal: true, .. } => {
            record.last_outcome = Some(TurnOutcome::Error {
                message: Some("provider exited unexpectedly".to_owned()),
            });
        }
        _ => {}
    }
}

/// The events that clear `inflight_turn`, and with it the turn a queued prompt belongs to.
pub(super) fn ends_the_turn(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::TurnSettled { .. }
            | AgentEvent::TurnAborted { .. }
            | AgentEvent::SessionExited { .. }
            | AgentEvent::RuntimeError { fatal: true, .. }
    )
}

/// Whether this event ends the provider session, so its open gates go with it.
pub(super) fn ends_the_session(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::SessionExited { .. }
            | AgentEvent::RuntimeError { fatal: true, .. }
            | AgentEvent::SessionStateChanged(SessionState::Stopped)
    )
}

/// The answer a gate nobody can respond to any more settles with.
///
/// It mirrors the one `control_cancel_request` already writes: the request is gone, so the safe
/// reading is that nothing was allowed.
pub(super) fn closed_gate_answer(kind: &GateKind) -> GateAnswer {
    match kind {
        GateKind::Permission { .. } => GateAnswer::Permission {
            choice: PermissionChoice::Deny,
            edited_payload: None,
        },
        GateKind::Question { .. } => GateAnswer::Question {
            answers: Vec::new(),
        },
        GateKind::Plan { .. } => GateAnswer::Plan(PlanAnswer::AskForChanges {
            note: String::new(),
        }),
    }
}

/// Whether the list row moved in a way a subscriber has to repaint.
pub(super) fn summary_transition(before: &AgentThreadSummary, after: &AgentThreadSummary) -> bool {
    before.attention != after.attention
        || before.session != after.session
        || before.turn != after.turn
        || before.title != after.title
        || before.exit_code != after.exit_code
}

/// The `raw` label an event carries when the provider gave none.
pub(super) fn event_name(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::SessionConfigured { .. } => "session_configured",
        AgentEvent::MetadataChanged { .. } => "metadata_changed",
        AgentEvent::SessionStateChanged(_) => "session_state_changed",
        AgentEvent::SessionActivity { .. } => "session_activity",
        AgentEvent::SessionExited { .. } => "session_exited",
        AgentEvent::TurnStarted { .. } => "turn_started",
        AgentEvent::TurnSettled { .. } => "turn_settled",
        AgentEvent::TurnAborted { .. } => "turn_aborted",
        AgentEvent::TurnDiff { .. } => "turn_diff",
        AgentEvent::PlanSteps { .. } => "plan_steps",
        AgentEvent::ItemStarted { .. } => "item_started",
        AgentEvent::ContentDelta { .. } => "content_delta",
        AgentEvent::ItemUpdated { .. } => "item_updated",
        AgentEvent::ItemCompleted { .. } => "item_completed",
        AgentEvent::GateOpened { .. } => "gate_opened",
        AgentEvent::GateResolved { .. } => "gate_resolved",
        AgentEvent::GateWithdrawn { .. } => "gate_withdrawn",
        AgentEvent::PlanProposed { .. } => "plan_proposed",
        AgentEvent::TokenUsage { .. } => "token_usage",
        AgentEvent::RateLimits { .. } => "rate_limits",
        AgentEvent::Compacted(_) => "compacted",
        AgentEvent::Retrying { .. } => "retrying",
        AgentEvent::ModelRerouted { .. } => "model_rerouted",
        AgentEvent::RuntimeError { .. } => "runtime_error",
        AgentEvent::Notice(_) => "notice",
        AgentEvent::Unknown { .. } => "unknown",
    }
}
