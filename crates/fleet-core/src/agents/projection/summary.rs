//! Attention derivation, the thread title, and the compact wire summary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    agents::{
        AgentEvent, AgentKind, Attention, AttentionKind, GateKind, OpenGate, Seq, SessionState,
        ThreadId, TurnState,
    },
    ids::WorktreeId,
};

#[derive(Debug, Clone, Copy)]
pub(super) enum GateDiscriminant {
    Permission,
    Question,
    Plan,
}

impl GateDiscriminant {
    pub(super) fn matches(self, kind: &GateKind) -> bool {
        matches!(
            (self, kind),
            (Self::Permission, GateKind::Permission { .. })
                | (Self::Question, GateKind::Question { .. })
                | (Self::Plan, GateKind::Plan { .. })
        )
    }
}

pub(super) fn gate_requires_attention(gate: &OpenGate) -> bool {
    match &gate.kind {
        GateKind::Question { questions } => questions.iter().any(|question| question.blocking),
        GateKind::Permission { .. } | GateKind::Plan { .. } => true,
    }
}

pub(super) fn thread_title(text: &str) -> String {
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

pub(super) fn event_is_nonterminal(event: &AgentEvent) -> bool {
    !matches!(
        event,
        AgentEvent::SessionExited { .. }
            | AgentEvent::TurnSettled { .. }
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
