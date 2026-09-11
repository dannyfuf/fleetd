//! Reducer-owned native thread projection contracts.

mod patch;
mod reduce;
mod summary;
mod usage;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    AgentEvent, AgentKind, Attention, AttentionKind, GateId, Item, ItemId, ModelSelection,
    OpenGate, PermissionMode, Seq, SeqEvent, SessionState, StreamKind, ThreadId, TurnId,
    TurnOutcome, TurnState, Usage,
};
use crate::ids::WorktreeId;

pub use summary::AgentThreadSummary;
use summary::{GateDiscriminant, event_is_nonterminal, gate_requires_attention};
use usage::file_totals;

#[cfg(test)]
mod tests;

/// Terminal facts attached to one recorded turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEnd {
    /// Provider-authoritative outcome.
    pub outcome: TurnOutcome,
    /// Normalized per-turn usage.
    pub usage: Usage,
    /// Provider-reported duration.
    pub duration_ms: u64,
    /// Changed files and line counts.
    pub files_changed: Vec<super::FileDelta>,
}

/// One submitted user turn and optional terminal facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnRecord {
    /// Stable turn identity.
    pub id: TurnId,
    /// User-message item that began the turn.
    pub user_item: Option<ItemId>,
    /// Reducer time at submission.
    pub started_at: DateTime<Utc>,
    /// Terminal facts once the provider settles the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<TurnEnd>,
    /// Milliseconds this turn spent parked on an open gate, waiting for the user.
    ///
    /// A provider reports the turn's wall clock, which counts the minutes a permission card sat
    /// on screen. §2 reads the footer as the turn's *own* duration, so the reducer accumulates
    /// the blocked time here and [`TurnRecord::footer`] takes it back off.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub blocked_ms: u64,
}

/// Serde skip predicate: an unblocked turn does not carry the field at all.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if hands the field by reference"
)]
const fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// Compact numeric facts used to render one completed turn footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnFooter {
    /// Provider-reported or reducer-derived turn duration.
    pub duration_ms: u64,
    /// Normalized total token count for the turn.
    pub tokens: u64,
    /// Number of changed files reported by the provider.
    pub files_changed: usize,
    /// Total added lines across changed files.
    pub added: u64,
    /// Total removed lines across changed files.
    pub removed: u64,
}

impl TurnRecord {
    /// Returns the footer facts once this turn has settled.
    #[must_use]
    pub fn footer(&self) -> Option<TurnFooter> {
        let ended = self.ended.as_ref()?;
        let (added, removed) = file_totals(&ended.files_changed);
        Some(TurnFooter {
            duration_ms: ended.duration_ms.saturating_sub(self.blocked_ms),
            tokens: ended.usage.total_tokens,
            files_changed: ended.files_changed.len(),
            added,
            removed,
        })
    }
}

/// One recorded transcript boundary, in the order it was observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRecord {
    /// What the boundary was.
    pub kind: super::CheckpointKind,
    /// The sequence it landed at.
    pub seq: Seq,
    /// The turn it follows, when the thread had already recorded one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_turn: Option<TurnId>,
}

/// One user-facing provider notice, in the order it was observed.
///
/// §3.2 defines `Notice` as a user-facing signal — a config warning, a deprecation, a harness
/// message such as `Stop hook error occurred`. It therefore has to survive in the projection
/// the rows are built from, or the daemon persists a signal no surface can ever show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoticeRecord {
    /// What the provider said.
    pub text: String,
    /// The sequence it landed at.
    pub seq: Seq,
    /// The turn it follows, when the thread had already recorded one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_turn: Option<TurnId>,
}

/// Current retry observation for a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryState {
    /// One-based attempt number.
    pub attempt: u32,
    /// Delay before retry.
    pub retry_in_ms: u64,
    /// Provider error text.
    pub reason: String,
}

/// Complete materialized state for one native-agent thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadProjection {
    /// Thread identity.
    pub thread: ThreadId,
    /// Owning published worktree.
    pub worktree: WorktreeId,
    /// Provider kind.
    pub provider: AgentKind,
    /// Display title, usually derived from the first prompt or provider session.
    pub title: String,
    /// Whole-session state.
    pub session: SessionState,
    /// Current or last turn state.
    pub turn: TurnState,
    /// Open gates in opening order.
    pub gates: Vec<OpenGate>,
    /// Transcript items in display order.
    pub items: Vec<Item>,
    /// Submitted turns in chronological order.
    pub turns: Vec<TurnRecord>,
    /// Active background-task item identities.
    pub background_tasks: Vec<ItemId>,
    /// Compaction and resume boundaries, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<CheckpointRecord>,
    /// User-facing provider notices, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<NoticeRecord>,
    /// Last applied per-thread sequence.
    pub last_seq: Seq,
    /// Latest provider-authoritative turn completion sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completed_seq: Option<Seq>,
    /// Latest event sequence that was not a thread or session terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_nonterminal_seq: Option<Seq>,
    /// Latest event timestamp.
    pub last_activity: Option<DateTime<Utc>>,
    /// Latest cumulative provider usage.
    pub cumulative_usage: Usage,
    /// Latest cumulative provider cost.
    pub cumulative_cost_usd: Option<f64>,
    /// Latest context-window utilization percentage.
    pub context_pct: f32,
    /// Active model.
    pub model: Option<ModelSelection>,
    /// Active permission mode.
    pub mode: PermissionMode,
    /// Provider process exit code.
    pub exit_code: Option<i32>,
    /// Current provider retry, if any.
    pub retrying: Option<RetryState>,
    /// Item identity to vector-position lookup, rebuilt while deserializing.
    #[serde(skip)]
    item_index: HashMap<ItemId, usize>,
    /// Turn identity to vector-position lookup, rebuilt while deserializing.
    #[serde(skip)]
    turn_index: HashMap<TurnId, usize>,
}

impl ThreadProjection {
    /// Builds the empty projection for a newly allocated thread.
    #[must_use]
    pub fn new(thread: ThreadId, worktree: WorktreeId, provider: AgentKind) -> Self {
        Self {
            thread,
            worktree,
            provider,
            title: provider.display_name().to_owned(),
            session: SessionState::Starting,
            turn: TurnState::None,
            gates: Vec::new(),
            items: Vec::new(),
            turns: Vec::new(),
            background_tasks: Vec::new(),
            checkpoints: Vec::new(),
            notices: Vec::new(),
            last_seq: Seq::default(),
            last_completed_seq: None,
            last_nonterminal_seq: None,
            last_activity: None,
            cumulative_usage: Usage::default(),
            cumulative_cost_usd: None,
            context_pct: 0.0,
            model: None,
            mode: PermissionMode::Ask,
            exit_code: None,
            retrying: None,
            item_index: HashMap::new(),
            turn_index: HashMap::new(),
        }
    }

    /// Applies one ordered event to this projection.
    pub fn apply(&mut self, ev: &SeqEvent) -> Result<(), ProjectionError> {
        // Reductions are transactional, and they are that without cloning the transcript: every
        // rejection this reducer can produce is decided by `validate` before a single field is
        // written, so a rejected event leaves the projection exactly as it was. Cloning instead
        // would make both replay and streaming quadratic in transcript size.
        self.rebuild_indices_if_needed();
        self.accepts(ev)?;
        self.reduce(ev)?;
        self.last_seq = ev.seq;
        if matches!(ev.event, AgentEvent::TurnSettled { .. }) {
            self.last_completed_seq = Some(ev.seq);
        }
        if event_is_nonterminal(&ev.event) {
            self.last_nonterminal_seq = Some(ev.seq);
        }
        self.last_activity = Some(ev.at);
        Ok(())
    }

    /// Whether this event would be accepted, decided without writing anything.
    ///
    /// [`ThreadProjection::apply`] answers this for itself before it mutates. It is public so a
    /// caller that has to make an event durable *before* it becomes visible — the daemon's
    /// reducer, whose log, memory and broadcast must not diverge (§6) — can settle the question
    /// first and then apply an event it already knows will land.
    ///
    /// # Errors
    ///
    /// Returns the same [`ProjectionError`] `apply` would have returned for this event.
    pub fn accepts(&self, ev: &SeqEvent) -> Result<(), ProjectionError> {
        let expected = self.last_seq.next();
        if ev.seq != expected {
            return Err(ProjectionError::OutOfOrder {
                expected,
                got: ev.seq,
            });
        }
        self.validate(ev)
    }

    /// Decides every rejection before [`ThreadProjection::reduce`] writes anything.
    ///
    /// The two functions are kept in step by construction: each arm here mirrors exactly the
    /// lookup its reducer arm performs, so a transition that passes validation cannot fail
    /// halfway through.
    fn validate(&self, ev: &SeqEvent) -> Result<(), ProjectionError> {
        match &ev.event {
            AgentEvent::TurnStarted { turn, .. } => {
                if matches!(self.turn, TurnState::Running(_)) || self.turn_position(*turn).is_some()
                {
                    return Err(ProjectionError::WrongTurn(*turn));
                }
            }
            AgentEvent::TurnSettled { turn, .. } => {
                if let TurnState::Running(active) = self.turn
                    && active != *turn
                {
                    return Err(ProjectionError::WrongTurn(*turn));
                }
            }
            AgentEvent::TurnAborted { turn, .. } => {
                self.require_active_turn(*turn)?;
            }
            AgentEvent::ItemStarted {
                turn, item, parent, ..
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
                if let Some(existing) = self
                    .item_position(*item)
                    .and_then(|index| self.items.get(index))
                    && existing.turn != *turn
                {
                    return Err(ProjectionError::WrongTurn(*turn));
                }
            }
            AgentEvent::ContentDelta { item, .. }
            | AgentEvent::ItemUpdated { item, .. }
            | AgentEvent::ItemCompleted { item, .. } => {
                if self.item_position(*item).is_none() {
                    return Err(ProjectionError::UnknownItem(*item));
                }
            }
            AgentEvent::GateResolved { gate, .. } | AgentEvent::GateWithdrawn { gate } => {
                if !self.gates.iter().any(|entry| entry.id == *gate) {
                    return Err(ProjectionError::UnknownGate(*gate));
                }
            }
            AgentEvent::TokenUsage { turn, .. } => {
                if self.turn_position(*turn).is_none() {
                    return Err(ProjectionError::WrongTurn(*turn));
                }
            }
            AgentEvent::SessionConfigured { .. }
            | AgentEvent::MetadataChanged { .. }
            | AgentEvent::SessionStateChanged(_)
            | AgentEvent::SessionActivity { .. }
            | AgentEvent::SessionExited { .. }
            | AgentEvent::GateOpened { .. }
            | AgentEvent::PlanProposed { .. }
            | AgentEvent::TurnDiff { .. }
            | AgentEvent::PlanSteps { .. }
            | AgentEvent::RateLimits { .. }
            | AgentEvent::Compacted(_)
            | AgentEvent::Retrying { .. }
            | AgentEvent::ModelRerouted { .. }
            | AgentEvent::RuntimeError { .. }
            | AgentEvent::Notice(_)
            | AgentEvent::Unknown { .. } => {}
        }
        Ok(())
    }

    fn require_active_turn(&self, turn: TurnId) -> Result<(), ProjectionError> {
        if self.turn == TurnState::Running(turn) {
            Ok(())
        } else {
            Err(ProjectionError::WrongTurn(turn))
        }
    }

    fn item_mut(&mut self, item: ItemId) -> Result<&mut Item, ProjectionError> {
        let index = self
            .item_position(item)
            .ok_or(ProjectionError::UnknownItem(item))?;
        self.items
            .get_mut(index)
            .ok_or(ProjectionError::UnknownItem(item))
    }

    /// The position of one item, in constant time once the index is warm.
    ///
    /// The index is not serialized, so a projection decoded from storage answers its first few
    /// lookups by scan — `accepts` takes `&self` and cannot build one. `apply` rebuilds the index
    /// before it reduces, so a decoded projection pays that scan once rather than per event, and
    /// the streaming path a `ContentDelta` runs on is always warm.
    fn item_position(&self, item: ItemId) -> Option<usize> {
        self.item_index
            .get(&item)
            .copied()
            .or_else(|| self.items.iter().position(|candidate| candidate.id == item))
    }

    /// The position of one turn, in constant time once the index is warm.
    fn turn_position(&self, turn: TurnId) -> Option<usize> {
        self.turn_index
            .get(&turn)
            .copied()
            .or_else(|| self.turns.iter().position(|candidate| candidate.id == turn))
    }

    /// Rebuilds the lookups when they cannot describe the vectors they index.
    ///
    /// Every reduction that pushes an item or a turn inserts its index in the same arm, so the
    /// lengths agree unless the projection was just deserialized — the indices are
    /// `#[serde(skip)]` — or a future arm forgets. Both cases are the same repair, and comparing
    /// lengths keeps the warm path to two integer comparisons per event.
    fn rebuild_indices_if_needed(&mut self) {
        if self.item_index.len() != self.items.len() {
            self.item_index = self
                .items
                .iter()
                .enumerate()
                .map(|(index, item)| (item.id, index))
                .collect();
        }
        if self.turn_index.len() != self.turns.len() {
            self.turn_index = self
                .turns
                .iter()
                .enumerate()
                .map(|(index, turn)| (turn.id, index))
                .collect();
        }
    }

    /// Derives the attention state for a client's last-seen cursor.
    #[must_use]
    pub fn attention(&self, last_seen: Seq) -> Attention {
        for (kind, attention) in [
            (GateDiscriminant::Permission, AttentionKind::Permission),
            (GateDiscriminant::Question, AttentionKind::Question),
            (GateDiscriminant::Plan, AttentionKind::Plan),
        ] {
            if self
                .gates
                .iter()
                .any(|gate| kind.matches(&gate.kind) && gate_requires_attention(gate))
            {
                return Attention::NeedsYou(attention);
            }
        }

        // §3 rule 7 fixes the order: gates, then work, then failure, then fresh completion. A
        // live turn and a dead session are both facts about *now*, and an older completed turn
        // nobody has looked at yet may not shadow either of them — which is what put
        // `needs-you` on a session the user had just killed.
        if self.session == SessionState::Running
            || matches!(self.turn, TurnState::Running(_))
            || !self.background_tasks.is_empty()
            || self.retrying.is_some()
        {
            return Attention::Working;
        }

        if matches!(self.session, SessionState::Waiting(_)) {
            return Attention::Waiting;
        }

        if self.session == SessionState::Error
            || matches!(self.turn, TurnState::Settled(_, TurnOutcome::Error { .. }))
        {
            return Attention::Failed;
        }

        // A finished turn is `needs you` whether or not the tab was ever opened: §3.3 defines it
        // purely against `last_seen_seq`, and the "never opened is not unread" carve-out below
        // is written for `Unread` alone. A thread started from the Hub or the CLI must still
        // raise its amber dot when it finishes.
        if self
            .last_completed_seq
            .is_some_and(|completed| completed > last_seen)
        {
            return Attention::NeedsYou(AttentionKind::Finished);
        }

        let has_been_seen = last_seen != Seq::default();
        if has_been_seen
            && self
                .last_nonterminal_seq
                .is_some_and(|event| event > last_seen)
        {
            return Attention::Unread;
        }

        Attention::Idle
    }

    /// Returns the compact snapshot and tab summary for this thread.
    #[must_use]
    pub fn summary(&self, last_seen: Seq) -> AgentThreadSummary {
        AgentThreadSummary {
            thread: self.thread,
            worktree: self.worktree.clone(),
            host: None,
            provider: self.provider,
            title: self.title.clone(),
            attention: self.attention(last_seen),
            session: self.session.clone(),
            turn: self.turn.clone(),
            last_seq: self.last_seq,
            last_activity: self.last_activity,
            exit_code: self.exit_code,
            last_completed_seq: self.last_completed_seq,
            last_nonterminal_seq: self.last_nonterminal_seq,
        }
    }
}

/// A rejected event-to-projection transition.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProjectionError {
    /// An event targeted a non-active or otherwise incompatible turn.
    #[error("event targets the wrong turn: {0}")]
    WrongTurn(TurnId),
    /// An event targeted an item that has not been started.
    #[error("unknown item: {0}")]
    UnknownItem(ItemId),
    /// A resolution targeted a gate that is not open.
    #[error("unknown gate: {0}")]
    UnknownGate(GateId),
    /// A content stream did not belong to the target item's typed payload.
    #[error("stream {1:?} is incompatible with item {0}")]
    WrongStream(ItemId, StreamKind),
    /// The per-thread sequence had a gap or moved backwards.
    #[error("out-of-order event: expected {expected}, got {got}")]
    OutOfOrder {
        /// Required next sequence.
        expected: Seq,
        /// Received sequence.
        got: Seq,
    },
}
