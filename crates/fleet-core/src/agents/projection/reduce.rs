//! The event-to-projection reduction, one arm per normalised event.

use chrono::{DateTime, Utc};

use super::{
    CheckpointRecord, NoticeRecord, ProjectionError, RetryState, ThreadProjection, TurnEnd,
    TurnRecord,
    patch::{ItemStatusExt, append_content, apply_item_patch, tool_has_result},
    summary::thread_title,
    usage::{add_usage, aggregate_usage, subtract_usage},
};
use crate::agents::{
    AgentEvent, GateKind, Item, ItemKind, ItemStatus, OpenGate, SeqEvent, SessionState, TurnId,
    TurnOutcome, TurnState, Usage, sticky_outcome,
};

impl ThreadProjection {
    pub(super) fn reduce(&mut self, ev: &SeqEvent) -> Result<(), ProjectionError> {
        match &ev.event {
            AgentEvent::SessionConfigured {
                provider,
                resume_cursor: _,
                model,
                mode,
                tools: _,
                commands: _,
                skills: _,
            } => {
                let default_title = self.title == self.provider.display_name();
                self.provider = *provider;
                if default_title {
                    self.title = provider.display_name().to_owned();
                }
                self.model = model.clone();
                self.mode = *mode;
                self.session = SessionState::Ready;
                self.exit_code = None;
                self.retrying = None;
                // §4.1 makes cost cumulative *for the process*: a new session starts a new
                // one, so the metadata row drops back to no cost here and only here.
                self.cumulative_cost_usd = None;
            }
            AgentEvent::MetadataChanged { title, mode, model } => {
                if let Some(title) = title
                    && !title.trim().is_empty()
                {
                    self.title.clone_from(title);
                }
                if let Some(mode) = mode {
                    self.mode = *mode;
                }
                if let Some(model) = model {
                    self.model = Some(model.clone());
                }
            }
            AgentEvent::SessionStateChanged(state) => {
                self.session.clone_from(state);
                self.retrying = None;
            }
            AgentEvent::SessionActivity { .. } => {}
            AgentEvent::SessionExited { code, expected } => {
                self.exit_code = *code;
                self.retrying = None;
                if *expected {
                    self.session = SessionState::Stopped;
                } else {
                    self.session = SessionState::Error;
                    self.fail_active_turn(ev.at, "provider exited unexpectedly");
                }
            }
            AgentEvent::TurnStarted { turn, user_item } => {
                if matches!(self.turn, TurnState::Running(_)) || self.turn_position(*turn).is_some()
                {
                    return Err(ProjectionError::WrongTurn(*turn));
                }
                self.turn = TurnState::Running(*turn);
                let index = self.turns.len();
                self.turns.push(TurnRecord {
                    id: *turn,
                    user_item: Some(*user_item),
                    started_at: ev.at,
                    ended: None,
                    blocked_ms: 0,
                });
                self.turn_index.insert(*turn, index);
                self.retrying = None;
            }
            AgentEvent::TurnSettled {
                turn,
                outcome,
                usage,
                duration_ms,
                files_changed,
            } => {
                let turn_index = if let Some(index) = self.turn_position(*turn) {
                    index
                } else {
                    let index = self.turns.len();
                    self.turns.push(TurnRecord {
                        id: *turn,
                        user_item: None,
                        started_at: ev.at,
                        ended: None,
                        blocked_ms: 0,
                    });
                    self.turn_index.insert(*turn, index);
                    index
                };
                self.close_open_items(*turn, ev.at);
                let record = self
                    .turns
                    .get_mut(turn_index)
                    .ok_or(ProjectionError::WrongTurn(*turn))?;
                let settled_outcome = record.ended.as_ref().map_or_else(
                    || outcome.clone(),
                    |end| sticky_outcome(&end.outcome, outcome),
                );
                record.ended = Some(TurnEnd {
                    outcome: settled_outcome.clone(),
                    usage: usage.clone(),
                    duration_ms: *duration_ms,
                    files_changed: files_changed.clone(),
                });
                self.cumulative_usage = aggregate_usage(&self.turns);
                // §3.3: an authoritative error result is a failed turn, not a completed one.
                // The outcome stays on the turn record, which is what the footer reads.
                self.turn = TurnState::Settled(*turn, settled_outcome);
                self.retrying = None;
            }
            AgentEvent::TurnAborted { turn, reason: _ } => {
                self.require_active_turn(*turn)?;
                let usage = self.active_turn_usage();
                self.close_open_items(*turn, ev.at);
                self.finish_failed_turn(*turn, ev.at, TurnOutcome::Interrupted, usage);
                self.cumulative_usage = aggregate_usage(&self.turns);
                self.turn = TurnState::Settled(*turn, TurnOutcome::Interrupted);
                self.retrying = None;
            }
            AgentEvent::ItemStarted {
                turn,
                item,
                kind,
                parent,
            } => {
                if self.turn_position(*turn).is_none() {
                    return Err(ProjectionError::WrongTurn(*turn));
                }
                if let Some(parent) = parent {
                    let parent_item = self
                        .item_position(*parent)
                        .and_then(|index| self.items.get(index))
                        .ok_or(ProjectionError::UnknownItem(*parent))?;
                    if parent_item.turn != *turn {
                        return Err(ProjectionError::WrongTurn(*turn));
                    }
                }

                let first_user_message = matches!(kind, ItemKind::UserMessage { .. })
                    && !self
                        .items
                        .iter()
                        .any(|candidate| matches!(&candidate.kind, ItemKind::UserMessage { .. }));
                if first_user_message && self.title == self.provider.display_name() {
                    let ItemKind::UserMessage { text, .. } = kind else {
                        unreachable!("the item kind was matched above");
                    };
                    let title = thread_title(text);
                    if !title.is_empty() {
                        self.title = title;
                    }
                }

                if let Some(index) = self.item_position(*item) {
                    let existing = self
                        .items
                        .get_mut(index)
                        .ok_or(ProjectionError::UnknownItem(*item))?;
                    if existing.turn != *turn {
                        return Err(ProjectionError::WrongTurn(*turn));
                    }
                    existing.parent = *parent;
                    existing.kind = kind.clone();
                    existing.status = ItemStatus::InProgress;
                    existing.ended = None;
                } else {
                    let index = self.items.len();
                    self.items.push(Item {
                        id: *item,
                        turn: *turn,
                        parent: *parent,
                        kind: kind.clone(),
                        status: ItemStatus::InProgress,
                        children: Vec::new(),
                        started: ev.at,
                        ended: None,
                    });
                    self.item_index.insert(*item, index);
                }

                if let Some(parent) = parent {
                    let parent_index = self
                        .item_position(*parent)
                        .ok_or(ProjectionError::UnknownItem(*parent))?;
                    let parent_item = self
                        .items
                        .get_mut(parent_index)
                        .ok_or(ProjectionError::UnknownItem(*parent))?;
                    if !parent_item.children.contains(item) {
                        parent_item.children.push(*item);
                    }
                }
                if matches!(kind, ItemKind::Subagent { .. })
                    && !self.background_tasks.contains(item)
                {
                    self.background_tasks.push(*item);
                }
                self.retrying = None;
            }
            AgentEvent::ContentDelta {
                item,
                stream,
                delta,
            } => {
                let item = self.item_mut(*item)?;
                append_content(item, *stream, delta)?;
                self.retrying = None;
            }
            AgentEvent::ItemUpdated { item, patch } => {
                let became_terminal = {
                    let item = self.item_mut(*item)?;
                    apply_item_patch(item, patch, ev.at)
                };
                if became_terminal {
                    self.background_tasks.retain(|candidate| candidate != item);
                }
                self.retrying = None;
            }
            AgentEvent::ItemCompleted { item, status } => {
                let became_terminal = {
                    let item = self.item_mut(*item)?;
                    item.status = *status;
                    item.ended = status.terminal().then_some(ev.at);
                    status.terminal()
                };
                if became_terminal {
                    self.background_tasks.retain(|candidate| candidate != item);
                }
                self.retrying = None;
            }
            AgentEvent::GateOpened { gate, turn, kind } => {
                let open = OpenGate {
                    id: *gate,
                    turn: *turn,
                    kind: kind.clone(),
                    opened_seq: ev.seq,
                    blocked_since: Some(ev.at),
                };
                if let Some(existing) = self.gates.iter_mut().find(|entry| entry.id == *gate) {
                    *existing = open;
                } else {
                    self.gates.push(open);
                }
            }
            AgentEvent::GateResolved {
                gate,
                answer: _,
                by: _,
            } => {
                let Some(index) = self.gates.iter().position(|entry| entry.id == *gate) else {
                    return Err(ProjectionError::UnknownGate(*gate));
                };
                let owner = self.gates[index].turn;
                // The thread has been blocked since the earliest gate of this window opened. A
                // second gate opening behind the first does not restart that clock, or the
                // overlap would be charged twice — and answering the first must not lose it, so
                // whatever stays open inherits the window's start.
                let since = self
                    .gates
                    .iter()
                    .filter_map(|entry| entry.blocked_since)
                    .min();
                self.gates.remove(index);
                if self.gates.is_empty() {
                    self.settle_blocked_time(owner, since, ev.at);
                } else if let Some(since) = since {
                    for entry in &mut self.gates {
                        entry.blocked_since =
                            Some(entry.blocked_since.map_or(since, |at| at.min(since)));
                    }
                }
            }
            AgentEvent::GateWithdrawn { gate } => {
                let Some(index) = self.gates.iter().position(|entry| entry.id == *gate) else {
                    return Err(ProjectionError::UnknownGate(*gate));
                };
                self.gates.remove(index);
            }
            AgentEvent::PlanProposed {
                gate,
                turn,
                markdown,
                steps,
            } => {
                let kind = GateKind::Plan {
                    markdown: markdown.clone(),
                    steps: steps.clone(),
                };
                let open = OpenGate {
                    id: *gate,
                    turn: Some(*turn),
                    kind,
                    opened_seq: ev.seq,
                    blocked_since: Some(ev.at),
                };
                if let Some(existing) = self.gates.iter_mut().find(|entry| entry.id == *gate) {
                    *existing = open;
                } else {
                    self.gates.push(open);
                }
            }
            AgentEvent::TokenUsage {
                turn,
                usage,
                context_pct,
                cost_usd,
            } => {
                let turn_index = self
                    .turn_position(*turn)
                    .ok_or(ProjectionError::WrongTurn(*turn))?;
                let record = self
                    .turns
                    .get_mut(turn_index)
                    .ok_or(ProjectionError::WrongTurn(*turn))?;
                // §5: "turn metadata is withheld until the turn completes, so the footer never
                // moves under the reader". A late observation — Claude publishes one for
                // `last_turn` when a `result` arrives with no active turn — is a *session*
                // fact, and rewriting the settled turn's recorded footer with it would move
                // the very number that was frozen when the turn ended.
                if record.ended.is_none() {
                    let mut cumulative = aggregate_usage(&self.turns);
                    add_usage(&mut cumulative, usage);
                    self.cumulative_usage = cumulative;
                } else {
                    self.cumulative_usage = aggregate_usage(&self.turns);
                }
                // An unmeasured context window is not a report of zero occupancy: OpenCode's
                // mapper answers 0.0 until the resumed turn's `message.updated` re-learns the
                // model, and blanking `context 34%` for that frame is the same mistake the
                // cost line below is written to avoid.
                if *context_pct > 0.0 {
                    self.context_pct = *context_pct;
                }
                // §4.1: cost is cumulative for the process — take the latest *reported* value.
                // A frame that omits it (Claude's `total_cost_usd: null`, OpenCode's zero cost)
                // is not a report of zero, and must not blank a `$0.42` the row already shows.
                if let Some(cost) = cost_usd {
                    self.cumulative_cost_usd = Some(*cost);
                }
                self.retrying = None;
            }
            AgentEvent::Compacted(kind) => {
                // §5 gives a boundary its own row, so it has to survive in the projection the
                // rows are built from rather than only in the log.
                self.checkpoints.push(CheckpointRecord {
                    kind: kind.clone(),
                    seq: ev.seq,
                    after_turn: self.turns.last().map(|record| record.id),
                });
            }
            AgentEvent::Retrying {
                attempt,
                retry_in_ms,
                reason,
            } => {
                self.retrying = Some(RetryState {
                    attempt: *attempt,
                    retry_in_ms: *retry_in_ms,
                    reason: reason.clone(),
                });
            }
            AgentEvent::TurnDiff { .. }
            | AgentEvent::PlanSteps { .. }
            | AgentEvent::RateLimits { .. } => {}
            AgentEvent::ModelRerouted { to, .. } => {
                if let Some(model) = &mut self.model {
                    model.model.clone_from(to);
                }
            }
            AgentEvent::RuntimeError { fatal, message } => {
                if *fatal {
                    self.session = SessionState::Error;
                    self.retrying = None;
                    self.fail_active_turn(ev.at, message);
                }
            }
            AgentEvent::Notice(text) => {
                // §5 gives the notice its own transcript row, so it has to survive in the
                // projection the rows are built from rather than only in the log.
                self.notices.push(NoticeRecord {
                    text: text.clone(),
                    seq: ev.seq,
                    after_turn: self.turns.last().map(|record| record.id),
                });
            }
            AgentEvent::Unknown { method } => {
                self.notices.push(NoticeRecord {
                    text: format!("Fleet does not understand harness event {method}"),
                    seq: ev.seq,
                    after_turn: self.turns.last().map(|record| record.id),
                });
            }
        }
        Ok(())
    }

    fn close_open_items(&mut self, turn: TurnId, at: DateTime<Utc>) {
        for item in self.items.iter_mut().filter(|item| item.turn == turn) {
            if item.status.terminal() {
                continue;
            }
            item.status = if tool_has_result(&item.kind) {
                ItemStatus::Completed
            } else if matches!(&item.kind, ItemKind::Tool(_) | ItemKind::Error { .. }) {
                ItemStatus::Failed
            } else {
                ItemStatus::Completed
            };
            item.ended = Some(at);
        }
        let items = &self.items;
        let item_index = &self.item_index;
        self.background_tasks.retain(|item_id| {
            item_index
                .get(item_id)
                .and_then(|index| items.get(*index))
                .is_some_and(|item| item.turn != turn)
        });
    }

    fn fail_active_turn(&mut self, at: DateTime<Utc>, message: &str) {
        let TurnState::Running(turn) = self.turn else {
            return;
        };
        let usage = self.active_turn_usage();
        self.close_open_items(turn, at);
        self.finish_failed_turn(
            turn,
            at,
            TurnOutcome::Error {
                message: Some(message.to_owned()),
            },
            usage,
        );
        self.cumulative_usage = aggregate_usage(&self.turns);
        self.turn = TurnState::Settled(
            turn,
            TurnOutcome::Error {
                message: Some(message.to_owned()),
            },
        );
    }

    fn finish_failed_turn(
        &mut self,
        turn: TurnId,
        at: DateTime<Utc>,
        outcome: TurnOutcome,
        usage: Usage,
    ) {
        let Some(index) = self.turn_position(turn) else {
            return;
        };
        let Some(record) = self.turns.get_mut(index) else {
            return;
        };
        let duration_ms = at
            .signed_duration_since(record.started_at)
            .num_milliseconds()
            .max(0) as u64;
        record.ended = Some(TurnEnd {
            outcome,
            usage,
            duration_ms,
            files_changed: Vec::new(),
        });
    }

    /// Charges the time the thread stood blocked to the turn that was waiting on the answer.
    ///
    /// The gate names its own turn when the provider supplied one; otherwise the turn that is
    /// still running owns the wait, because that is the footer the user is about to read. With
    /// neither there is nothing to correct and the elapsed time is simply dropped.
    fn settle_blocked_time(
        &mut self,
        gate_turn: Option<TurnId>,
        since: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) {
        let Some(since) = since else {
            return;
        };
        let owner = gate_turn.or(match self.turn {
            TurnState::Running(turn) => Some(turn),
            _ => None,
        });
        let Some(owner) = owner else {
            return;
        };
        let blocked = at.signed_duration_since(since).num_milliseconds().max(0);
        let Ok(blocked) = u64::try_from(blocked) else {
            return;
        };
        // §5: "turn metadata is withheld until the turn completes, so the footer never moves
        // under the reader" — and [`TurnRecord::footer`] re-derives the duration from
        // `blocked_ms` on every read. OpenCode opens the plan gate *inside* turn settlement,
        // so charging a wait that outlived the turn would shrink a footer the user has already
        // read, to zero whenever the card sat open longer than the turn ran.
        if let Some(index) = self.turn_position(owner)
            && let Some(record) = self.turns.get_mut(index)
            && record.ended.is_none()
        {
            record.blocked_ms = record.blocked_ms.saturating_add(blocked);
        }
    }

    fn active_turn_usage(&self) -> Usage {
        subtract_usage(&self.cumulative_usage, &aggregate_usage(&self.turns))
    }
}
