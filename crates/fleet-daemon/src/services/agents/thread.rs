//! Per-thread runtime state and sequence-safe broadcast coalescing.

use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

use fleet_core::{
    agents::{
        AgentEvent, AgentThreadSummary, GateId, SeqEvent, ThreadProjection, TurnId, UserInput,
    },
    ids::HostId,
};
use tokio::{sync::Mutex, task::AbortHandle};

use super::{
    AgentThreadRecord,
    providers::{AgentProvider, ProviderEvent},
};

/// Mutable reducer state protected by a runtime's serialized operation gate.
pub(super) struct ThreadState {
    pub projection: ThreadProjection,
    pub record: AgentThreadRecord,
    /// The host that owns this thread's log and sequence, or `None` when this daemon does.
    ///
    /// Read once, at hydration, and never again: a thread does not change owner in its life.
    /// Holding it here is what makes every authority check on the mutation path an in-memory
    /// comparison rather than a database round trip on the keystroke path
    /// (`docs/NATIVE-AGENTS.md` §9.3).
    pub owner: Option<HostId>,
    pub inflight_turn: Option<TurnId>,
    /// Prompts already written to the harness whose `TurnStarted` has not been drained yet.
    ///
    /// Both harnesses announce the turn on the event stream, not as the submit's return value, so
    /// a fresh submission is parked here and recorded when that announcement lands — which is
    /// also what makes the user's own bubble part of the log rather than only of the sending
    /// client's optimistic row. A steer is never parked: it is recorded immediately, because the
    /// turn it joined is already running.
    pub pending_inputs: VecDeque<(TurnId, UserInput)>,
    /// Gates an answer has already been written for, so a repeat is a no-op rather than a
    /// second control response for a request the provider has closed.
    pub answered_gates: HashSet<GateId>,
}

/// Cloneable handles for one live or persisted native-agent thread.
#[derive(Clone)]
pub(super) struct ThreadRuntime {
    pub operation: Arc<Mutex<()>>,
    pub state: Arc<std::sync::Mutex<ThreadState>>,
    pub provider: Arc<Mutex<Option<Box<dyn AgentProvider>>>>,
    task_abort: Arc<std::sync::Mutex<Option<AbortHandle>>>,
}

impl ThreadRuntime {
    pub fn new(
        projection: ThreadProjection,
        record: AgentThreadRecord,
        owner: Option<HostId>,
    ) -> Self {
        Self {
            operation: Arc::new(Mutex::new(())),
            state: Arc::new(std::sync::Mutex::new(ThreadState {
                projection,
                record,
                owner,
                inflight_turn: None,
                pending_inputs: VecDeque::new(),
                answered_gates: HashSet::new(),
            })),
            provider: Arc::new(Mutex::new(None)),
            task_abort: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// The host that owns this thread, or `None` when this daemon does.
    pub fn owner(&self) -> Option<HostId> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .owner
            .clone()
    }

    pub fn set_task(&self, handle: AbortHandle) {
        let mut task = self
            .task_abort
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(previous) = task.replace(handle) {
            previous.abort();
        }
    }

    pub fn abort_task(&self) {
        if let Some(task) = self
            .task_abort
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

/// One durable event and an optional summary transition produced by applying it.
pub(super) struct AppliedEvent {
    pub event: SeqEvent,
    pub summary: Option<AgentThreadSummary>,
}

/// Merges each run of adjacent content deltas for one item into a single event.
///
/// §5 coalesces deltas in the daemon "one event per item per ~16 ms … so a fast model does not
/// schedule a render per token". Keeping the empty sequence positions of the merged tokens does
/// not do that: every one of them is still a stored line, a broadcast frame and a projection
/// advance the client re-renders on. The run collapses to one event, and one sequence, instead.
pub(super) fn coalesce_deltas(events: Vec<ProviderEvent>) -> Vec<ProviderEvent> {
    let mut merged: Vec<ProviderEvent> = Vec::with_capacity(events.len());
    for next in events {
        let joined = match (merged.last_mut().map(|last| &mut last.event), &next.event) {
            (
                Some(AgentEvent::ContentDelta {
                    item,
                    stream,
                    delta,
                }),
                AgentEvent::ContentDelta {
                    item: next_item,
                    stream: next_stream,
                    delta: next_delta,
                },
            ) if item == next_item && stream == next_stream => {
                delta.push_str(next_delta);
                true
            }
            _ => false,
        };
        if !joined {
            merged.push(next);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use fleet_core::agents::{ItemId, StreamKind};

    use super::*;

    fn delta(item: ItemId, text: &str) -> ProviderEvent {
        ProviderEvent::new(
            AgentEvent::ContentDelta {
                item,
                stream: StreamKind::AssistantText,
                delta: text.to_owned(),
            },
            Some("stream_event/content_block_delta"),
        )
    }

    #[test]
    fn a_run_of_deltas_for_one_item_becomes_one_event() {
        let item = ItemId::new();
        let other = ItemId::new();
        let coalesced = coalesce_deltas(vec![
            delta(item, "hel"),
            delta(item, "lo"),
            delta(other, "hi"),
            delta(item, "!"),
        ]);
        let deltas = coalesced
            .iter()
            .map(|event| match &event.event {
                AgentEvent::ContentDelta { delta, .. } => delta.as_str(),
                _ => "",
            })
            .collect::<Vec<_>>();

        // One event per item per tick: the merged run costs one sequence, not one per token.
        assert_eq!(deltas, ["hello", "hi", "!"]);
        assert_eq!(
            coalesced[0].raw.as_deref(),
            Some("stream_event/content_block_delta")
        );
    }

    #[test]
    fn a_structural_event_between_deltas_is_never_merged_away() {
        let item = ItemId::new();
        let coalesced = coalesce_deltas(vec![
            delta(item, "a"),
            AgentEvent::ItemCompleted {
                item,
                status: fleet_core::agents::ItemStatus::Completed,
            }
            .into(),
            delta(item, "b"),
        ]);
        assert_eq!(coalesced.len(), 3);
    }
}
