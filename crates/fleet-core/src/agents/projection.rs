//! Reducer-owned native thread projection contracts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    AgentEvent, AgentKind, Attention, AttentionKind, FileDelta, GateKind, Item, ItemId, ItemKind,
    ItemPatch, ItemStatus, ModelSelection, OpenGate, PermissionMode, Seq, SeqEvent, SessionState,
    StreamKind, ThreadId, TurnId, TurnOutcome, TurnState, Usage,
};
use crate::ids::WorktreeId;

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
    pub user_item: ItemId,
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
        }
    }

    /// Applies one ordered event to this projection.
    pub fn apply(&mut self, ev: &SeqEvent) -> Result<(), ProjectionError> {
        // Reductions are transactional, and they are that without cloning the transcript: every
        // rejection this reducer can produce is decided by `validate` before a single field is
        // written, so a rejected event leaves the projection exactly as it was. Cloning instead
        // would make both replay and streaming quadratic in transcript size.
        self.accepts(ev)?;
        self.reduce(ev)?;
        self.last_seq = ev.seq;
        if matches!(ev.event, AgentEvent::TurnCompleted { .. }) {
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
                if matches!(self.turn, TurnState::Running(_))
                    || self.turns.iter().any(|record| record.id == *turn)
                {
                    return Err(ProjectionError::WrongTurn(*turn));
                }
            }
            AgentEvent::TurnCompleted { turn, .. } | AgentEvent::TurnAborted { turn, .. } => {
                self.require_active_turn(*turn)?;
            }
            AgentEvent::ItemStarted {
                turn, item, parent, ..
            } => {
                if !self.turns.iter().any(|record| record.id == *turn) {
                    return Err(ProjectionError::WrongTurn(*turn));
                }
                if let Some(parent) = parent {
                    let parent_item = self
                        .items
                        .iter()
                        .find(|candidate| candidate.id == *parent)
                        .ok_or(ProjectionError::UnknownItem(*parent))?;
                    if parent_item.turn != *turn {
                        return Err(ProjectionError::WrongTurn(*turn));
                    }
                }
                if let Some(existing) = self.items.iter().find(|entry| entry.id == *item)
                    && existing.turn != *turn
                {
                    return Err(ProjectionError::WrongTurn(*turn));
                }
            }
            AgentEvent::ContentDelta { item, .. }
            | AgentEvent::ItemUpdated { item, .. }
            | AgentEvent::ItemCompleted { item, .. } => {
                if !self.items.iter().any(|entry| entry.id == *item) {
                    return Err(ProjectionError::UnknownItem(*item));
                }
            }
            AgentEvent::GateResolved { gate, .. } => {
                if !self.gates.iter().any(|entry| entry.id == *gate) {
                    return Err(ProjectionError::UnknownGate(*gate));
                }
            }
            AgentEvent::TokenUsage { turn, .. } => {
                if !self.turns.iter().any(|record| record.id == *turn) {
                    return Err(ProjectionError::WrongTurn(*turn));
                }
            }
            AgentEvent::SessionStarted { .. }
            | AgentEvent::MetadataChanged { .. }
            | AgentEvent::SessionStateChanged(_)
            | AgentEvent::SessionExited { .. }
            | AgentEvent::GateOpened { .. }
            | AgentEvent::Checkpoint(_)
            | AgentEvent::Retrying { .. }
            | AgentEvent::RuntimeError { .. }
            | AgentEvent::Notice(_) => {}
        }
        Ok(())
    }

    /// Derives the attention state for a client's last-seen cursor.
    #[must_use]
    pub fn attention(&self, last_seen: Seq) -> Attention {
        for (kind, attention) in [
            (GateDiscriminant::Permission, AttentionKind::Permission),
            (GateDiscriminant::Question, AttentionKind::Question),
            (GateDiscriminant::Plan, AttentionKind::Plan),
        ] {
            if self.gates.iter().any(|gate| kind.matches(&gate.kind)) {
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

        if self.session == SessionState::Error || matches!(self.turn, TurnState::Failed(_)) {
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
            session: self.session,
            turn: self.turn.clone(),
            last_seq: self.last_seq,
            last_activity: self.last_activity,
            exit_code: self.exit_code,
            last_completed_seq: self.last_completed_seq,
            last_nonterminal_seq: self.last_nonterminal_seq,
        }
    }

    fn reduce(&mut self, ev: &SeqEvent) -> Result<(), ProjectionError> {
        match &ev.event {
            AgentEvent::SessionStarted {
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
                self.session = *state;
                self.retrying = None;
            }
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
                if matches!(self.turn, TurnState::Running(_))
                    || self.turns.iter().any(|record| record.id == *turn)
                {
                    return Err(ProjectionError::WrongTurn(*turn));
                }
                self.turn = TurnState::Running(*turn);
                self.turns.push(TurnRecord {
                    id: *turn,
                    user_item: *user_item,
                    started_at: ev.at,
                    ended: None,
                    blocked_ms: 0,
                });
                self.retrying = None;
            }
            AgentEvent::TurnCompleted {
                turn,
                outcome,
                usage,
                duration_ms,
                files_changed,
            } => {
                self.require_active_turn(*turn)?;
                self.close_open_items(*turn, ev.at);
                let record = self
                    .turns
                    .iter_mut()
                    .find(|record| record.id == *turn)
                    .ok_or(ProjectionError::WrongTurn(*turn))?;
                record.ended = Some(TurnEnd {
                    outcome: outcome.clone(),
                    usage: usage.clone(),
                    duration_ms: *duration_ms,
                    files_changed: files_changed.clone(),
                });
                self.cumulative_usage = aggregate_usage(&self.turns);
                // §3.3: an authoritative error result is a failed turn, not a completed one.
                // The outcome stays on the turn record, which is what the footer reads.
                self.turn = if matches!(outcome, TurnOutcome::Error { .. }) {
                    TurnState::Failed(*turn)
                } else {
                    TurnState::Completed(*turn, outcome.clone())
                };
                self.retrying = None;
            }
            AgentEvent::TurnAborted { turn, reason: _ } => {
                self.require_active_turn(*turn)?;
                let usage = self.active_turn_usage();
                self.close_open_items(*turn, ev.at);
                self.finish_failed_turn(*turn, ev.at, TurnOutcome::Interrupted, usage);
                self.cumulative_usage = aggregate_usage(&self.turns);
                self.turn = TurnState::Interrupted(*turn);
                self.retrying = None;
            }
            AgentEvent::ItemStarted {
                turn,
                item,
                kind,
                parent,
            } => {
                if !self.turns.iter().any(|record| record.id == *turn) {
                    return Err(ProjectionError::WrongTurn(*turn));
                }
                if let Some(parent) = parent {
                    let parent_item = self
                        .items
                        .iter()
                        .find(|candidate| candidate.id == *parent)
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

                if let Some(existing) = self.items.iter_mut().find(|entry| entry.id == *item) {
                    if existing.turn != *turn {
                        return Err(ProjectionError::WrongTurn(*turn));
                    }
                    existing.parent = *parent;
                    existing.kind = kind.clone();
                    existing.status = ItemStatus::Running;
                    existing.ended = None;
                } else {
                    self.items.push(Item {
                        id: *item,
                        turn: *turn,
                        parent: *parent,
                        kind: kind.clone(),
                        status: ItemStatus::Running,
                        text: None,
                        summary: None,
                        result: None,
                        output: None,
                        diff: None,
                        children: Vec::new(),
                        started: ev.at,
                        ended: None,
                    });
                }

                if let Some(parent) = parent {
                    let parent_item = self
                        .items
                        .iter_mut()
                        .find(|candidate| candidate.id == *parent)
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
                match stream {
                    StreamKind::AssistantText | StreamKind::Reasoning => {
                        item.text.get_or_insert_with(String::new).push_str(delta);
                    }
                    StreamKind::ToolOutput => {
                        item.output.get_or_insert_with(String::new).push_str(delta);
                    }
                }
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
            AgentEvent::TokenUsage {
                turn,
                usage,
                context_pct,
                cost_usd,
            } => {
                let record = self
                    .turns
                    .iter_mut()
                    .find(|record| record.id == *turn)
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
            AgentEvent::Checkpoint(kind) => {
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
        self.items
            .iter_mut()
            .find(|candidate| candidate.id == item)
            .ok_or(ProjectionError::UnknownItem(item))
    }

    fn close_open_items(&mut self, turn: TurnId, at: DateTime<Utc>) {
        for item in self.items.iter_mut().filter(|item| item.turn == turn) {
            if item.status.terminal() {
                continue;
            }
            item.status = if item.result.is_some() {
                ItemStatus::Done
            } else if matches!(&item.kind, ItemKind::Tool { .. } | ItemKind::Error) {
                ItemStatus::Error
            } else {
                ItemStatus::Done
            };
            item.ended = Some(at);
        }
        self.background_tasks.retain(|item_id| {
            self.items
                .iter()
                .find(|item| item.id == *item_id)
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
        self.turn = TurnState::Failed(turn);
    }

    fn finish_failed_turn(
        &mut self,
        turn: TurnId,
        at: DateTime<Utc>,
        outcome: TurnOutcome,
        usage: Usage,
    ) {
        let Some(record) = self.turns.iter_mut().find(|record| record.id == turn) else {
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
        if let Some(record) = self.turns.iter_mut().find(|record| record.id == owner) {
            record.blocked_ms = record.blocked_ms.saturating_add(blocked);
        }
    }

    fn active_turn_usage(&self) -> Usage {
        subtract_usage(&self.cumulative_usage, &aggregate_usage(&self.turns))
    }
}

#[derive(Debug, Clone, Copy)]
enum GateDiscriminant {
    Permission,
    Question,
    Plan,
}

impl GateDiscriminant {
    fn matches(self, kind: &GateKind) -> bool {
        matches!(
            (self, kind),
            (Self::Permission, GateKind::Permission { .. })
                | (Self::Question, GateKind::Question { .. })
                | (Self::Plan, GateKind::Plan { .. })
        )
    }
}

trait ItemStatusExt {
    fn terminal(self) -> bool;
}

impl ItemStatusExt for ItemStatus {
    fn terminal(self) -> bool {
        matches!(self, Self::Done | Self::Error | Self::Denied)
    }
}

fn apply_item_patch(item: &mut Item, patch: &ItemPatch, at: DateTime<Utc>) -> bool {
    if let Some(text) = &patch.text {
        item.text = Some(text.clone());
    }
    if let Some(input) = &patch.input
        && let ItemKind::Tool { input: current, .. } = &mut item.kind
    {
        *current = input.clone();
    }
    if let Some(summary) = &patch.summary {
        item.summary = Some(summary.clone());
    }
    if let Some(result) = &patch.result {
        item.result = Some(result.clone());
    }
    if let Some(output) = &patch.output {
        item.output = Some(output.clone());
    }
    if let Some(diff) = &patch.diff {
        item.diff = Some(diff.clone());
    }
    if let Some(status) = patch.status {
        item.status = status;
        item.ended = status.terminal().then_some(at);
    }
    item.status.terminal()
}

fn file_totals(files: &[FileDelta]) -> (u64, u64) {
    files.iter().fold((0, 0), |(added, removed), file| {
        (
            added.saturating_add(file.added),
            removed.saturating_add(file.removed),
        )
    })
}

fn aggregate_usage(turns: &[TurnRecord]) -> Usage {
    turns.iter().filter_map(|turn| turn.ended.as_ref()).fold(
        Usage::default(),
        |mut cumulative, ended| {
            add_usage(&mut cumulative, &ended.usage);
            cumulative
        },
    )
}

fn add_usage(cumulative: &mut Usage, usage: &Usage) {
    cumulative.input_tokens = cumulative.input_tokens.saturating_add(usage.input_tokens);
    cumulative.output_tokens = cumulative.output_tokens.saturating_add(usage.output_tokens);
    cumulative.reasoning_tokens = cumulative
        .reasoning_tokens
        .saturating_add(usage.reasoning_tokens);
    cumulative.cache_read_tokens = cumulative
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens);
    cumulative.cache_write_tokens = cumulative
        .cache_write_tokens
        .saturating_add(usage.cache_write_tokens);
    cumulative.total_tokens = cumulative.total_tokens.saturating_add(usage.total_tokens);
    cumulative.web_search_requests = cumulative
        .web_search_requests
        .saturating_add(usage.web_search_requests);
    cumulative.tool_uses = cumulative.tool_uses.saturating_add(usage.tool_uses);
    for (key, value) in &usage.extra {
        cumulative.extra.insert(key.clone(), value.clone());
    }
}

fn subtract_usage(cumulative: &Usage, completed: &Usage) -> Usage {
    Usage {
        input_tokens: cumulative
            .input_tokens
            .saturating_sub(completed.input_tokens),
        output_tokens: cumulative
            .output_tokens
            .saturating_sub(completed.output_tokens),
        reasoning_tokens: cumulative
            .reasoning_tokens
            .saturating_sub(completed.reasoning_tokens),
        cache_read_tokens: cumulative
            .cache_read_tokens
            .saturating_sub(completed.cache_read_tokens),
        cache_write_tokens: cumulative
            .cache_write_tokens
            .saturating_sub(completed.cache_write_tokens),
        total_tokens: cumulative
            .total_tokens
            .saturating_sub(completed.total_tokens),
        web_search_requests: cumulative
            .web_search_requests
            .saturating_sub(completed.web_search_requests),
        tool_uses: cumulative.tool_uses.saturating_sub(completed.tool_uses),
        extra: cumulative.extra.clone(),
    }
}

fn thread_title(text: &str) -> String {
    const MAX_CHARS: usize = 48;

    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= MAX_CHARS {
        return normalized;
    }

    let prefix = normalized.chars().take(MAX_CHARS).collect::<String>();
    prefix
        .rsplit_once(char::is_whitespace)
        .map_or(prefix.clone(), |(words, _)| words.to_owned())
}

fn event_is_nonterminal(event: &AgentEvent) -> bool {
    !matches!(
        event,
        AgentEvent::SessionExited { .. }
            | AgentEvent::TurnCompleted { .. }
            | AgentEvent::TurnAborted { .. }
            | AgentEvent::RuntimeError { fatal: true, .. }
    )
}

/// Compact daemon snapshot state used by tabs and context-bar counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThreadSummary {
    /// Thread identity.
    pub thread: ThreadId,
    /// Owning worktree.
    pub worktree: WorktreeId,
    /// Owning remote host, or `None` for a daemon-local thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<crate::ids::HostId>,
    /// Provider kind.
    pub provider: AgentKind,
    /// Display title.
    pub title: String,
    /// Attention derived against an unseen thread, which every client narrows itself.
    ///
    /// §3.3 puts the view axis "per client, not persisted daemon-side", and one
    /// [`AgentThreadSummary`] is broadcast to every subscriber: a cursor folded in here would
    /// be one client's, and would clear the amber dot on all the others. The two seen-relative
    /// attentions are re-derived by [`AgentThreadSummary::attention_for`] from the cursor the
    /// reading client actually holds.
    pub attention: Attention,
    /// Whole-session state.
    pub session: SessionState,
    /// Current or most recently settled turn.
    pub turn: TurnState,
    /// Last persisted sequence.
    pub last_seq: Seq,
    /// Latest persisted event time.
    pub last_activity: Option<DateTime<Utc>>,
    /// Sequence of the last turn settlement, for the reader's own `needs you` derivation.
    #[serde(default)]
    pub last_completed_seq: Option<Seq>,
    /// Sequence of the last non-terminal event, for the reader's own `unread` derivation.
    #[serde(default)]
    pub last_nonterminal_seq: Option<Seq>,
    /// Provider process exit code, if exited.
    pub exit_code: Option<i32>,
}

impl AgentThreadSummary {
    /// The attention this summary means for one client's own seen cursor (§3.3).
    ///
    /// The gate, work and failure rows are facts about the thread and are shared verbatim.
    /// Only `needs you (finished)` and `unread` are defined against a cursor, and that cursor
    /// belongs to the reader: two windows on the same thread have two of them, so neither may
    /// be answered from the daemon's copy.
    #[must_use]
    pub fn attention_for(&self, last_seen: Seq) -> Attention {
        match self.attention {
            Attention::NeedsYou(AttentionKind::Finished) | Attention::Unread | Attention::Idle => {
                if self
                    .last_completed_seq
                    .is_some_and(|completed| completed > last_seen)
                {
                    Attention::NeedsYou(AttentionKind::Finished)
                } else if last_seen != Seq::default()
                    && self
                        .last_nonterminal_seq
                        .is_some_and(|event| event > last_seen)
                {
                    Attention::Unread
                } else {
                    Attention::Idle
                }
            }
            other => other,
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
    UnknownGate(super::GateId),
    /// The per-thread sequence had a gap or moved backwards.
    #[error("out-of-order event: expected {expected}, got {got}")]
    OutOfOrder {
        /// Required next sequence.
        expected: Seq,
        /// Received sequence.
        got: Seq,
    },
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{DateTime, Utc};
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::agents::{
        AbortReason, Attachment, GateAnswer, GateId, GateResolver, PermissionChoice,
        PermissionOption, PlanAnswer, ProviderOptionId, Question, QuestionOption, ToolDiff,
        ToolKind,
    };

    struct EventBuilder {
        seq: u64,
    }

    impl EventBuilder {
        fn new() -> Self {
            Self { seq: 0 }
        }

        fn next(&mut self, event: AgentEvent) -> SeqEvent {
            self.seq += 1;
            SeqEvent {
                seq: Seq(self.seq),
                at: timestamp(self.seq),
                raw: None,
                event,
            }
        }
    }

    fn timestamp(second: u64) -> DateTime<Utc> {
        DateTime::from_timestamp(second as i64, 0)
            .unwrap_or_else(|| panic!("test timestamp must be valid"))
    }

    fn thread_id(value: u128) -> ThreadId {
        ThreadId::from_uuid(Uuid::from_u128(value))
    }

    fn turn_id(value: u128) -> TurnId {
        TurnId::from_uuid(Uuid::from_u128(value))
    }

    fn item_id(value: u128) -> ItemId {
        ItemId::from_uuid(Uuid::from_u128(value))
    }

    fn gate_id(value: u128) -> GateId {
        GateId::from_uuid(Uuid::from_u128(value))
    }

    fn projection() -> ThreadProjection {
        ThreadProjection::new(
            thread_id(1),
            WorktreeId::try_from("acme/api#native-agents")
                .unwrap_or_else(|error| panic!("{error}")),
            AgentKind::Claude,
        )
    }

    fn apply(
        projection: &mut ThreadProjection,
        events: &mut EventBuilder,
        event: AgentEvent,
    ) -> SeqEvent {
        let event = events.next(event);
        projection
            .apply(&event)
            .unwrap_or_else(|error| panic!("{error}"));
        event
    }

    fn session_started() -> AgentEvent {
        AgentEvent::SessionStarted {
            provider: AgentKind::Claude,
            resume_cursor: Some("session-1".to_owned()),
            model: Some(ModelSelection {
                model: "claude-sonnet-5".to_owned(),
                effort: Some("high".to_owned()),
                provider: None,
            }),
            mode: PermissionMode::Ask,
            tools: vec!["Read".to_owned(), "Edit".to_owned()],
            commands: vec!["compact".to_owned()],
            skills: vec!["review".to_owned()],
        }
    }

    fn user_message(text: &str) -> ItemKind {
        ItemKind::UserMessage {
            text: text.to_owned(),
            attachments: Vec::<Attachment>::new(),
        }
    }

    fn usage(total_tokens: u64) -> Usage {
        Usage {
            input_tokens: total_tokens / 2,
            output_tokens: total_tokens / 4,
            reasoning_tokens: total_tokens / 4,
            total_tokens,
            ..Usage::default()
        }
    }

    fn start_turn(
        projection: &mut ThreadProjection,
        events: &mut EventBuilder,
        turn: TurnId,
        user: ItemId,
        text: &str,
    ) {
        apply(
            projection,
            events,
            AgentEvent::TurnStarted {
                turn,
                user_item: user,
            },
        );
        apply(
            projection,
            events,
            AgentEvent::ItemStarted {
                turn,
                item: user,
                kind: user_message(text),
                parent: None,
            },
        );
    }

    fn complete_turn(
        projection: &mut ThreadProjection,
        events: &mut EventBuilder,
        turn: TurnId,
    ) -> SeqEvent {
        apply(
            projection,
            events,
            AgentEvent::TurnCompleted {
                turn,
                outcome: TurnOutcome::Completed,
                usage: usage(120),
                duration_ms: 48_000,
                files_changed: Vec::new(),
            },
        )
    }

    fn permission_gate() -> GateKind {
        GateKind::Permission {
            tool: ToolKind::Bash,
            title: "Run command?".to_owned(),
            payload: "cargo test".to_owned(),
            rationale: Some("Tests need approval".to_owned()),
            options: vec![PermissionOption {
                id: ProviderOptionId("once".to_owned()),
                label: PermissionChoice::AllowOnce,
            }],
        }
    }

    fn question_gate() -> GateKind {
        GateKind::Question {
            questions: vec![Question {
                text: "Which database?".to_owned(),
                header: "Database".to_owned(),
                options: vec![QuestionOption {
                    label: "SQLite".to_owned(),
                    description: "Local".to_owned(),
                }],
                multi_select: false,
                allow_other: true,
            }],
        }
    }

    fn plan_gate() -> GateKind {
        GateKind::Plan {
            markdown: "# Plan".to_owned(),
            steps: vec!["Implement reducer".to_owned()],
        }
    }

    /// The provider reports the turn's wall clock, which counts the minutes a permission card
    /// sat on screen waiting for a `y`. §2 reads the footer as the turn's own duration, so the
    /// gate's open window is charged to the turn and taken back off again.
    #[test]
    fn a_turn_footer_excludes_the_time_the_turn_stood_blocked_on_the_user() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(2);
        let gate = gate_id(3);
        start_turn(&mut projection, &mut events, turn, item_id(4), "touch it");

        // The clock ticks one second per event, so the six events between the open and the
        // answer are six seconds the user spent reading the card.
        apply(
            &mut projection,
            &mut events,
            AgentEvent::GateOpened {
                gate,
                turn: Some(turn),
                kind: permission_gate(),
            },
        );
        for _ in 0..5 {
            apply(
                &mut projection,
                &mut events,
                AgentEvent::Notice("waiting".to_owned()),
            );
        }
        apply(
            &mut projection,
            &mut events,
            AgentEvent::GateResolved {
                gate,
                answer: GateAnswer::Permission {
                    choice: PermissionChoice::AllowOnce,
                    edited_payload: None,
                },
                by: GateResolver::User,
            },
        );
        assert_eq!(projection.turns[0].blocked_ms, 6_000);

        // `duration_ms: 48_000` is what the provider measured, gate included.
        complete_turn(&mut projection, &mut events, turn);
        let footer = projection.turns[0]
            .footer()
            .unwrap_or_else(|| panic!("a settled turn has a footer"));
        assert_eq!(
            footer.duration_ms, 42_000,
            "48s wall clock minus the 6s parked"
        );
    }

    /// A second gate opening behind the first does not restart the clock, or the overlap would
    /// be charged twice and the footer could go to zero.
    #[test]
    fn overlapping_gates_are_one_wait_not_two() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(2);
        let (first, second) = (gate_id(3), gate_id(4));
        start_turn(&mut projection, &mut events, turn, item_id(5), "two gates");

        for gate in [first, second] {
            apply(
                &mut projection,
                &mut events,
                AgentEvent::GateOpened {
                    gate,
                    turn: Some(turn),
                    kind: permission_gate(),
                },
            );
        }
        for gate in [first, second] {
            apply(
                &mut projection,
                &mut events,
                AgentEvent::GateResolved {
                    gate,
                    answer: GateAnswer::Permission {
                        choice: PermissionChoice::AllowOnce,
                        edited_payload: None,
                    },
                    by: GateResolver::User,
                },
            );
        }
        // Opened at seq 3 and 4, closed at 5 and 6: one window of three seconds.
        assert_eq!(projection.turns[0].blocked_ms, 3_000);
    }

    #[test]
    fn plain_text_turn_projects_text_usage_footer_title_and_summary() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(10);
        let user = item_id(11);
        let assistant = item_id(12);
        apply(&mut projection, &mut events, session_started());
        start_turn(
            &mut projection,
            &mut events,
            turn,
            user,
            "Implement a deterministic native agent projection reducer with tests",
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ItemStarted {
                turn,
                item: assistant,
                kind: ItemKind::AssistantText,
                parent: None,
            },
        );
        for delta in ["Hello", ", world"] {
            apply(
                &mut projection,
                &mut events,
                AgentEvent::ContentDelta {
                    item: assistant,
                    stream: StreamKind::AssistantText,
                    delta: delta.to_owned(),
                },
            );
        }
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TokenUsage {
                turn,
                usage: usage(100),
                context_pct: 34.0,
                cost_usd: Some(0.42),
            },
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ItemCompleted {
                item: assistant,
                status: ItemStatus::Done,
            },
        );
        let completed = complete_turn(&mut projection, &mut events, turn);

        assert_eq!(
            projection.turn,
            TurnState::Completed(turn, TurnOutcome::Completed)
        );
        assert_eq!(projection.items[0].status, ItemStatus::Done);
        assert_eq!(projection.items[1].text.as_deref(), Some("Hello, world"));
        assert_eq!(projection.title, "Implement a deterministic native agent");
        assert_eq!(projection.cumulative_usage, usage(120));
        assert_eq!(projection.cumulative_cost_usd, Some(0.42));
        assert_eq!(projection.context_pct, 34.0);
        assert_eq!(
            projection.turns[0].footer(),
            Some(TurnFooter {
                duration_ms: 48_000,
                tokens: 120,
                files_changed: 0,
                added: 0,
                removed: 0,
            })
        );
        assert_eq!(projection.last_activity, Some(completed.at));
        assert_eq!(
            projection.summary(completed.seq),
            projection.summary(completed.seq)
        );
    }

    #[test]
    fn edit_tool_projects_replacement_patch_diff_output_and_file_totals() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(20);
        let user = item_id(21);
        let tool = item_id(22);
        apply(&mut projection, &mut events, session_started());
        start_turn(&mut projection, &mut events, turn, user, "Edit two files");
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ItemStarted {
                turn,
                item: tool,
                kind: ItemKind::Tool {
                    kind: ToolKind::Edit,
                    name: "Edit".to_owned(),
                    input: json!({"path": "src/lib.rs"}),
                },
                parent: None,
            },
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ContentDelta {
                item: tool,
                stream: StreamKind::ToolOutput,
                delta: "patching ".to_owned(),
            },
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ContentDelta {
                item: tool,
                stream: StreamKind::ToolOutput,
                delta: "complete".to_owned(),
            },
        );
        let diff = ToolDiff {
            path: PathBuf::from("src/lib.rs"),
            added: 14,
            removed: 3,
            unified: "@@ -1 +1 @@".to_owned(),
        };
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ItemUpdated {
                item: tool,
                patch: ItemPatch {
                    input: Some(json!({"path": "src/lib.rs", "replace_all": true})),
                    summary: Some("src/lib.rs +14 -3".to_owned()),
                    result: Some("updated".to_owned()),
                    diff: Some(diff.clone()),
                    ..ItemPatch::default()
                },
            },
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TurnCompleted {
                turn,
                outcome: TurnOutcome::Completed,
                usage: usage(250),
                duration_ms: 1_200,
                files_changed: vec![
                    FileDelta {
                        path: PathBuf::from("src/lib.rs"),
                        added: 14,
                        removed: 3,
                    },
                    FileDelta {
                        path: PathBuf::from("src/main.rs"),
                        added: 22,
                        removed: 0,
                    },
                ],
            },
        );

        let tool = projection
            .items
            .iter()
            .find(|item| item.id == tool)
            .unwrap_or_else(|| panic!("tool item must exist"));
        assert_eq!(tool.status, ItemStatus::Done);
        assert_eq!(tool.output.as_deref(), Some("patching complete"));
        assert_eq!(tool.diff.as_ref(), Some(&diff));
        assert_eq!(
            projection.turns[0].footer(),
            Some(TurnFooter {
                duration_ms: 1_200,
                tokens: 250,
                files_changed: 2,
                added: 36,
                removed: 3,
            })
        );
    }

    #[test]
    fn permission_gate_opens_mid_turn_and_resolves_independently() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(30);
        let gate = gate_id(31);
        apply(&mut projection, &mut events, session_started());
        start_turn(&mut projection, &mut events, turn, item_id(32), "Run tests");
        apply(
            &mut projection,
            &mut events,
            AgentEvent::GateOpened {
                gate,
                turn: Some(turn),
                kind: permission_gate(),
            },
        );
        assert_eq!(
            projection.attention(projection.last_seq),
            Attention::NeedsYou(AttentionKind::Permission)
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::GateResolved {
                gate,
                answer: GateAnswer::Permission {
                    choice: PermissionChoice::AllowOnce,
                    edited_payload: None,
                },
                by: GateResolver::User,
            },
        );
        assert!(projection.gates.is_empty());
        assert_eq!(
            projection.attention(projection.last_seq),
            Attention::Working
        );
    }

    #[test]
    fn question_gate_remains_open_after_turn_completion() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(40);
        apply(&mut projection, &mut events, session_started());
        start_turn(
            &mut projection,
            &mut events,
            turn,
            item_id(41),
            "Ask a question",
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::GateOpened {
                gate: gate_id(42),
                turn: Some(turn),
                kind: question_gate(),
            },
        );
        complete_turn(&mut projection, &mut events, turn);

        assert_eq!(projection.gates.len(), 1);
        assert_eq!(
            projection.attention(projection.last_seq),
            Attention::NeedsYou(AttentionKind::Question)
        );
    }

    #[test]
    fn plan_gate_has_its_own_attention_priority() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        apply(&mut projection, &mut events, session_started());
        apply(
            &mut projection,
            &mut events,
            AgentEvent::GateOpened {
                gate: gate_id(50),
                turn: None,
                kind: plan_gate(),
            },
        );
        assert_eq!(
            projection.attention(Seq::default()),
            Attention::NeedsYou(AttentionKind::Plan)
        );
    }

    #[test]
    fn abort_closes_open_text_and_tools_without_inventing_success() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(60);
        let assistant = item_id(62);
        let tool = item_id(63);
        apply(&mut projection, &mut events, session_started());
        start_turn(
            &mut projection,
            &mut events,
            turn,
            item_id(61),
            "Interrupt this",
        );
        for (item, kind) in [
            (assistant, ItemKind::AssistantText),
            (
                tool,
                ItemKind::Tool {
                    kind: ToolKind::Bash,
                    name: "Bash".to_owned(),
                    input: json!({"command": "sleep 10"}),
                },
            ),
        ] {
            apply(
                &mut projection,
                &mut events,
                AgentEvent::ItemStarted {
                    turn,
                    item,
                    kind,
                    parent: None,
                },
            );
        }
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TurnAborted {
                turn,
                reason: AbortReason::User,
            },
        );

        assert_eq!(projection.turn, TurnState::Interrupted(turn));
        assert_eq!(projection.items[1].status, ItemStatus::Done);
        assert_eq!(projection.items[2].status, ItemStatus::Error);
        assert_eq!(
            projection.turns[0]
                .ended
                .as_ref()
                .map(|ended| &ended.outcome),
            Some(&TurnOutcome::Interrupted)
        );
    }

    #[test]
    fn unexpected_exit_fails_the_active_turn_and_session() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(70);
        let tool = item_id(72);
        apply(&mut projection, &mut events, session_started());
        start_turn(
            &mut projection,
            &mut events,
            turn,
            item_id(71),
            "Start a tool",
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ItemStarted {
                turn,
                item: tool,
                kind: ItemKind::Tool {
                    kind: ToolKind::Bash,
                    name: "Bash".to_owned(),
                    input: json!({}),
                },
                parent: None,
            },
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::SessionExited {
                code: Some(1),
                expected: false,
            },
        );

        assert_eq!(projection.session, SessionState::Error);
        assert_eq!(projection.turn, TurnState::Failed(turn));
        assert_eq!(projection.exit_code, Some(1));
        assert_eq!(projection.items[1].status, ItemStatus::Error);
        assert_eq!(projection.attention(Seq(1)), Attention::Failed);
    }

    #[test]
    fn out_of_order_unknown_targets_and_wrong_turns_are_rejected_atomically() {
        let mut projection = projection();
        let original = projection.clone();
        let skipped = SeqEvent {
            seq: Seq(2),
            at: timestamp(2),
            raw: None,
            event: session_started(),
        };
        assert_eq!(
            projection.apply(&skipped),
            Err(ProjectionError::OutOfOrder {
                expected: Seq(1),
                got: Seq(2),
            })
        );
        assert_eq!(projection, original);

        let mut events = EventBuilder::new();
        let turn = turn_id(80);
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TurnStarted {
                turn,
                user_item: item_id(81),
            },
        );
        let before = projection.clone();
        let second_turn = turn_id(82);
        let second_start = events.next(AgentEvent::TurnStarted {
            turn: second_turn,
            user_item: item_id(83),
        });
        assert_eq!(
            projection.apply(&second_start),
            Err(ProjectionError::WrongTurn(second_turn))
        );
        assert_eq!(projection, before);

        let wrong_terminal = SeqEvent {
            event: AgentEvent::TurnCompleted {
                turn: second_turn,
                outcome: TurnOutcome::Completed,
                usage: Usage::default(),
                duration_ms: 0,
                files_changed: Vec::new(),
            },
            ..second_start.clone()
        };
        assert_eq!(
            projection.apply(&wrong_terminal),
            Err(ProjectionError::WrongTurn(second_turn))
        );
        assert_eq!(projection, before);

        let missing_item = SeqEvent {
            event: AgentEvent::ContentDelta {
                item: item_id(84),
                stream: StreamKind::AssistantText,
                delta: "lost".to_owned(),
            },
            ..second_start
        };
        assert_eq!(
            projection.apply(&missing_item),
            Err(ProjectionError::UnknownItem(item_id(84)))
        );
        assert_eq!(projection, before);
    }

    #[test]
    fn attention_follows_every_priority_row_and_seen_transition() {
        let mut permission = projection();
        permission.gates.push(OpenGate {
            id: gate_id(90),
            turn: None,
            kind: permission_gate(),
            opened_seq: Seq(1),
            blocked_since: None,
        });
        permission.gates.push(OpenGate {
            id: gate_id(91),
            turn: None,
            kind: question_gate(),
            opened_seq: Seq(2),
            blocked_since: None,
        });
        assert_eq!(
            permission.attention(Seq::default()),
            Attention::NeedsYou(AttentionKind::Permission)
        );

        permission.gates.remove(0);
        assert_eq!(
            permission.attention(Seq::default()),
            Attention::NeedsYou(AttentionKind::Question)
        );
        permission.gates[0].kind = plan_gate();
        assert_eq!(
            permission.attention(Seq::default()),
            Attention::NeedsYou(AttentionKind::Plan)
        );

        let mut finished = projection();
        let mut events = EventBuilder::new();
        apply(&mut finished, &mut events, session_started());
        start_turn(
            &mut finished,
            &mut events,
            turn_id(92),
            item_id(93),
            "Finish",
        );
        let completed = complete_turn(&mut finished, &mut events, turn_id(92));
        assert_eq!(
            finished.attention(Seq(completed.seq.0 - 1)),
            Attention::NeedsYou(AttentionKind::Finished)
        );
        assert_eq!(finished.attention(completed.seq), Attention::Idle);
        // A thread nobody has opened yet still raises the amber dot when its turn finishes.
        assert_eq!(
            finished.attention(Seq::default()),
            Attention::NeedsYou(AttentionKind::Finished)
        );
        apply(
            &mut finished,
            &mut events,
            AgentEvent::Notice("post-completion notice".to_owned()),
        );
        assert_eq!(finished.attention(completed.seq), Attention::Unread);
        assert_eq!(
            finished.attention(Seq(completed.seq.0 - 1)),
            Attention::NeedsYou(AttentionKind::Finished)
        );

        let mut failed = projection();
        failed.session = SessionState::Error;
        assert_eq!(failed.attention(Seq(1)), Attention::Failed);

        let mut working = projection();
        working.session = SessionState::Running;
        assert_eq!(working.attention(Seq(1)), Attention::Working);
        working.session = SessionState::Ready;
        working.background_tasks.push(item_id(94));
        assert_eq!(working.attention(Seq(1)), Attention::Working);
        working.background_tasks.clear();
        working.retrying = Some(RetryState {
            attempt: 1,
            retry_in_ms: 10,
            reason: "retry".to_owned(),
        });
        assert_eq!(working.attention(Seq(1)), Attention::Working);

        let mut unread = projection();
        let mut events = EventBuilder::new();
        apply(&mut unread, &mut events, session_started());
        let seen = unread.last_seq;
        apply(
            &mut unread,
            &mut events,
            AgentEvent::Notice("new output".to_owned()),
        );
        assert_eq!(unread.attention(seen), Attention::Unread);
        assert_eq!(unread.attention(unread.last_seq), Attention::Idle);
        assert_eq!(unread.attention(Seq::default()), Attention::Idle);
    }

    #[test]
    fn steering_adds_a_user_message_to_the_active_turn_and_queue_has_no_event() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(100);
        apply(&mut projection, &mut events, session_started());
        start_turn(
            &mut projection,
            &mut events,
            turn,
            item_id(101),
            "First prompt",
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ItemStarted {
                turn,
                item: item_id(102),
                kind: user_message("Steer the active prompt"),
                parent: None,
            },
        );
        assert_eq!(projection.items.len(), 2);
        assert!(projection.items.iter().all(|item| item.turn == turn));
        assert_eq!(projection.turn, TurnState::Running(turn));

        let before_queue = projection.clone();
        // Queued messages live only in app state and deliberately produce no AgentEvent.
        assert_eq!(projection, before_queue);
    }

    #[test]
    fn all_observability_events_reduce_without_hidden_side_effects() {
        let mut projection = projection();
        projection.title = "Provider title".to_owned();
        let mut events = EventBuilder::new();
        apply(&mut projection, &mut events, session_started());
        assert_eq!(projection.title, "Provider title");
        apply(
            &mut projection,
            &mut events,
            AgentEvent::Checkpoint(crate::agents::CheckpointKind::Resumed { age_ms: 50 }),
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::Retrying {
                attempt: 2,
                retry_in_ms: 500,
                reason: "overloaded".to_owned(),
            },
        );
        assert_eq!(projection.attention(Seq(1)), Attention::Working);
        apply(
            &mut projection,
            &mut events,
            AgentEvent::RuntimeError {
                fatal: false,
                message: "recovered".to_owned(),
            },
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::SessionStateChanged(SessionState::Ready),
        );
        assert!(projection.retrying.is_none());
        apply(
            &mut projection,
            &mut events,
            AgentEvent::SessionExited {
                code: Some(0),
                expected: true,
            },
        );
        assert_eq!(projection.session, SessionState::Stopped);
        assert_eq!(projection.exit_code, Some(0));
    }

    #[test]
    fn subagent_items_track_background_work_until_the_item_settles() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(103);
        let subagent = item_id(104);
        apply(&mut projection, &mut events, session_started());
        start_turn(
            &mut projection,
            &mut events,
            turn,
            item_id(105),
            "Delegate research",
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ItemStarted {
                turn,
                item: subagent,
                kind: ItemKind::Subagent {
                    name: "explore".to_owned(),
                    description: "Find callers".to_owned(),
                },
                parent: None,
            },
        );
        assert_eq!(projection.background_tasks, vec![subagent]);
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ItemUpdated {
                item: subagent,
                patch: ItemPatch {
                    status: Some(ItemStatus::Done),
                    ..ItemPatch::default()
                },
            },
        );
        assert!(projection.background_tasks.is_empty());
    }

    #[test]
    fn usage_accumulates_by_turn_while_cost_and_context_use_latest_observation() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        apply(&mut projection, &mut events, session_started());

        let first = turn_id(105);
        start_turn(
            &mut projection,
            &mut events,
            first,
            item_id(106),
            "First turn",
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TurnCompleted {
                turn: first,
                outcome: TurnOutcome::Completed,
                usage: usage(100),
                duration_ms: 100,
                files_changed: Vec::new(),
            },
        );

        let second = turn_id(107);
        start_turn(
            &mut projection,
            &mut events,
            second,
            item_id(108),
            "Second turn",
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TokenUsage {
                turn: second,
                usage: usage(25),
                context_pct: 61.5,
                cost_usd: Some(1.25),
            },
        );
        assert_eq!(projection.cumulative_usage.total_tokens, 125);
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TurnAborted {
                turn: second,
                reason: AbortReason::User,
            },
        );
        assert_eq!(
            projection.turns[1].footer().map(|footer| footer.tokens),
            Some(25)
        );
        assert_eq!(projection.cumulative_usage.total_tokens, 125);
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TokenUsage {
                turn: second,
                usage: usage(30),
                context_pct: 62.0,
                cost_usd: None,
            },
        );
        // §5: "turn metadata is withheld until the turn completes, so the footer never moves
        // under the reader". A late observation against a settled turn — Claude publishes one
        // for `last_turn` whenever a `result` arrives with no active turn — is a session fact,
        // never a rewrite of the footer that turn already froze.
        assert_eq!(
            projection.turns[1].footer().map(|footer| footer.tokens),
            Some(25)
        );
        assert_eq!(projection.cumulative_usage.total_tokens, 125);
        assert_eq!(projection.context_pct, 62.0);
        // A frame that omits the cost is not a report of zero: the metadata row keeps the last
        // cost the process actually reported (§4.1, "take the latest, never sum").
        assert_eq!(projection.cumulative_cost_usd, Some(1.25));
    }

    /// BH-3: an unmeasured window is not a report of zero occupancy.
    ///
    /// On an OpenCode resume the first usage frame of the turn arrives before the
    /// `message.updated` that re-learns the model, so the mapper answers 0.0 for "window
    /// unknown". Writing it through blanked `context 34%` mid-turn.
    #[test]
    fn an_unmeasured_context_window_leaves_the_last_known_percentage_standing() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        apply(&mut projection, &mut events, session_started());
        let turn = turn_id(150);
        start_turn(&mut projection, &mut events, turn, item_id(151), "Resume");
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TokenUsage {
                turn,
                usage: usage(10),
                context_pct: 19.5,
                cost_usd: None,
            },
        );
        assert_eq!(projection.context_pct, 19.5);
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TokenUsage {
                turn,
                usage: usage(20),
                context_pct: 0.0,
                cost_usd: None,
            },
        );
        assert_eq!(
            projection.context_pct, 19.5,
            "an unmeasured window blanked the metadata row"
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TokenUsage {
                turn,
                usage: usage(30),
                context_pct: 1.9,
                cost_usd: None,
            },
        );
        assert_eq!(projection.context_pct, 1.9);
    }

    #[test]
    fn a_new_session_starts_a_new_cost_but_a_missing_one_does_not_clear_it() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        apply(&mut projection, &mut events, session_started());
        let turn = turn_id(140);
        start_turn(&mut projection, &mut events, turn, item_id(141), "Cost");
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TokenUsage {
                turn,
                usage: usage(10),
                context_pct: 12.0,
                cost_usd: Some(0.42),
            },
        );
        assert_eq!(projection.cumulative_cost_usd, Some(0.42));
        apply(
            &mut projection,
            &mut events,
            AgentEvent::TokenUsage {
                turn,
                usage: usage(20),
                context_pct: 13.0,
                cost_usd: None,
            },
        );
        assert_eq!(projection.cumulative_cost_usd, Some(0.42));

        apply(&mut projection, &mut events, session_started());
        assert_eq!(projection.cumulative_cost_usd, None);
    }

    #[test]
    fn replay_is_deterministic_and_summary_is_stable() {
        let turn = turn_id(110);
        let assistant = item_id(112);
        let mut builder = EventBuilder::new();
        let sequence = vec![
            builder.next(session_started()),
            builder.next(AgentEvent::TurnStarted {
                turn,
                user_item: item_id(111),
            }),
            builder.next(AgentEvent::ItemStarted {
                turn,
                item: item_id(111),
                kind: user_message("Deterministic replay"),
                parent: None,
            }),
            builder.next(AgentEvent::ItemStarted {
                turn,
                item: assistant,
                kind: ItemKind::Thinking,
                parent: None,
            }),
            builder.next(AgentEvent::ContentDelta {
                item: assistant,
                stream: StreamKind::Reasoning,
                delta: "reasoning".to_owned(),
            }),
            builder.next(AgentEvent::ItemCompleted {
                item: assistant,
                status: ItemStatus::Done,
            }),
            builder.next(AgentEvent::TurnCompleted {
                turn,
                outcome: TurnOutcome::Completed,
                usage: usage(64),
                duration_ms: 700,
                files_changed: Vec::new(),
            }),
        ];
        let mut left = projection();
        let mut right = projection();
        for event in &sequence {
            left.apply(event).unwrap_or_else(|error| panic!("{error}"));
            right.apply(event).unwrap_or_else(|error| panic!("{error}"));
        }

        assert_eq!(left, right);
        let summary = left.summary(Seq(1));
        assert_eq!(summary, left.summary(Seq(1)));
        assert_eq!(summary, right.summary(Seq(1)));
        assert_eq!(left.items[1].text.as_deref(), Some("reasoning"));
    }

    #[test]
    fn projections_without_attention_cursors_remain_wire_compatible() {
        let projection = projection();
        let mut value = serde_json::to_value(&projection)
            .unwrap_or_else(|error| panic!("projection must serialize: {error}"));
        let object = value
            .as_object_mut()
            .unwrap_or_else(|| panic!("projection must serialize as an object"));
        object.remove("lastCompletedSeq");
        object.remove("lastNonterminalSeq");

        let restored: ThreadProjection = serde_json::from_value(value)
            .unwrap_or_else(|error| panic!("legacy projection must deserialize: {error}"));
        assert_eq!(restored, projection);
    }

    #[test]
    fn checkpoints_are_recorded_where_the_transcript_shows_them() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(150);
        apply(&mut projection, &mut events, session_started());
        apply(
            &mut projection,
            &mut events,
            AgentEvent::Checkpoint(crate::agents::CheckpointKind::Resumed { age_ms: 7_200_000 }),
        );
        start_turn(&mut projection, &mut events, turn, item_id(151), "Go on");
        apply(
            &mut projection,
            &mut events,
            AgentEvent::Checkpoint(crate::agents::CheckpointKind::CompactBoundary {
                before: 84_000,
                after: Some(12_000),
            }),
        );

        assert_eq!(
            projection
                .checkpoints
                .iter()
                .map(|checkpoint| checkpoint.after_turn)
                .collect::<Vec<_>>(),
            [None, Some(turn)]
        );
    }

    #[test]
    fn an_error_result_fails_the_turn_so_the_tab_counts_it_as_failed() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(130);
        apply(&mut projection, &mut events, session_started());
        start_turn(&mut projection, &mut events, turn, item_id(131), "Ask");
        let completed = apply(
            &mut projection,
            &mut events,
            AgentEvent::TurnCompleted {
                turn,
                outcome: TurnOutcome::Error {
                    message: Some("Authentication Failed".to_owned()),
                },
                usage: usage(10),
                duration_ms: 900,
                files_changed: Vec::new(),
            },
        );

        assert_eq!(projection.turn, TurnState::Failed(turn));
        assert_eq!(
            projection.turns[0]
                .ended
                .as_ref()
                .map(|ended| ended.outcome.clone()),
            Some(TurnOutcome::Error {
                message: Some("Authentication Failed".to_owned()),
            })
        );
        // §3 rule 7: failure outranks fresh completion, seen or unseen. A turn that ended in an
        // error is not a result waiting to be read.
        assert_eq!(
            projection.attention(Seq(completed.seq.0 - 1)),
            Attention::Failed
        );
        assert_eq!(projection.attention(completed.seq), Attention::Failed);
    }

    #[test]
    fn an_unseen_completion_never_shadows_a_dead_or_a_working_session() {
        // A finished turn nobody has read, followed by a killed provider: §3 rule 7 puts work
        // and failure ahead of fresh completion, so the tab says `failed`, not `needs you`.
        let mut dead = projection();
        let mut events = EventBuilder::new();
        apply(&mut dead, &mut events, session_started());
        start_turn(&mut dead, &mut events, turn_id(200), item_id(201), "Go");
        let completed = complete_turn(&mut dead, &mut events, turn_id(200));
        assert_eq!(
            dead.attention(Seq(completed.seq.0 - 1)),
            Attention::NeedsYou(AttentionKind::Finished)
        );
        apply(
            &mut dead,
            &mut events,
            AgentEvent::SessionExited {
                code: None,
                expected: false,
            },
        );
        assert_eq!(dead.attention(Seq(completed.seq.0 - 1)), Attention::Failed);

        // The same unread completion, followed by a new turn: the tab says `working`.
        let mut busy = projection();
        let mut events = EventBuilder::new();
        apply(&mut busy, &mut events, session_started());
        start_turn(&mut busy, &mut events, turn_id(202), item_id(203), "Go");
        let completed = complete_turn(&mut busy, &mut events, turn_id(202));
        start_turn(&mut busy, &mut events, turn_id(204), item_id(205), "More");
        assert_eq!(busy.attention(Seq(completed.seq.0 - 1)), Attention::Working);
    }

    #[test]
    fn a_rejected_event_leaves_every_field_untouched_without_cloning() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let turn = turn_id(140);
        let tool = item_id(141);
        apply(&mut projection, &mut events, session_started());
        start_turn(&mut projection, &mut events, turn, item_id(142), "Work");
        apply(
            &mut projection,
            &mut events,
            AgentEvent::ItemStarted {
                turn,
                item: tool,
                kind: ItemKind::Tool {
                    kind: ToolKind::Bash,
                    name: "Bash".to_owned(),
                    input: json!({}),
                },
                parent: None,
            },
        );
        let before = projection.clone();

        let next_seq = projection.last_seq.next();
        let orphan_parent = SeqEvent {
            seq: next_seq,
            at: timestamp(next_seq.0),
            raw: None,
            event: AgentEvent::ItemStarted {
                turn,
                item: item_id(143),
                kind: ItemKind::AssistantText,
                parent: Some(item_id(144)),
            },
        };
        assert_eq!(
            projection.apply(&orphan_parent),
            Err(ProjectionError::UnknownItem(item_id(144)))
        );
        assert_eq!(projection, before);

        let unknown_gate = SeqEvent {
            event: AgentEvent::GateResolved {
                gate: gate_id(145),
                answer: GateAnswer::Plan(PlanAnswer::Approve),
                by: GateResolver::User,
            },
            ..orphan_parent.clone()
        };
        assert_eq!(
            projection.apply(&unknown_gate),
            Err(ProjectionError::UnknownGate(gate_id(145)))
        );
        assert_eq!(projection, before);

        let unknown_turn_usage = SeqEvent {
            event: AgentEvent::TokenUsage {
                turn: turn_id(146),
                usage: usage(10),
                context_pct: 1.0,
                cost_usd: None,
            },
            ..orphan_parent.clone()
        };
        assert_eq!(
            projection.apply(&unknown_turn_usage),
            Err(ProjectionError::WrongTurn(turn_id(146)))
        );
        assert_eq!(projection, before);
    }

    #[test]
    fn gate_resolution_and_plan_answers_do_not_mutate_unrelated_turn_state() {
        let mut projection = projection();
        let mut events = EventBuilder::new();
        let gate = gate_id(120);
        apply(
            &mut projection,
            &mut events,
            AgentEvent::GateOpened {
                gate,
                turn: None,
                kind: plan_gate(),
            },
        );
        apply(
            &mut projection,
            &mut events,
            AgentEvent::GateResolved {
                gate,
                answer: GateAnswer::Plan(PlanAnswer::Approve),
                by: GateResolver::Auto,
            },
        );
        assert_eq!(projection.turn, TurnState::None);
        assert!(projection.gates.is_empty());
    }
}
