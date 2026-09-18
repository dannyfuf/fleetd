//! Per-thread runtime state and sequence-safe broadcast coalescing.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
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
    providers::{AgentProvider, ProviderEvent, ProviderEvents},
};

/// Maximum time assistant text waits for an adjacent-delta merge.
pub(super) const DELTA_TICK: Duration = Duration::from_millis(16);
/// Maximum time repeated item patches wait for a last-wins collapse.
pub(super) const UPDATE_TICK: Duration = Duration::from_millis(50);

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
    /// Identity of the provider currently occupying the slot. Event tasks carry the generation
    /// they were spawned for so a late exit from a retired adapter cannot detach its replacement.
    provider_generation: Arc<AtomicU64>,
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
            provider_generation: Arc::new(AtomicU64::new(0)),
            task_abort: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Mints the identity of a provider immediately before its event task is installed.
    pub fn next_provider_generation(&self) -> u64 {
        self.provider_generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Whether an event task still belongs to the adapter in the provider slot.
    pub fn provider_is_current(&self, generation: u64) -> bool {
        self.provider_generation.load(Ordering::Acquire) == generation
    }

    /// Invalidates the current event task before an explicit stop settles the transcript itself.
    pub fn invalidate_provider(&self) {
        self.provider_generation.fetch_add(1, Ordering::AcqRel);
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

/// Waits for one coalesced provider batch or one structural event.
///
/// The receiver itself is an unbounded compatibility seam, so this loop is its coalescing drain:
/// it continuously consumes stream traffic until the earliest deadline. A structural event wakes
/// the select immediately, flushes pending stream events first, and is appended unchanged.
pub(super) async fn next_coalesced_batch(
    receiver: &mut ProviderEvents,
) -> Option<Vec<ProviderEvent>> {
    let mut pending = Vec::new();
    let mut deadline = None;
    loop {
        if pending.is_empty() {
            let first = receiver.recv().await?;
            let Some(window) = coalescing_window(&first.event) else {
                return Some(vec![first]);
            };
            deadline = Some(tokio::time::Instant::now() + window);
            pending.push(first);
        }

        let flush_at = deadline.expect("a non-empty coalescing batch has a deadline");
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(flush_at) => {
                return Some(coalesce_events(pending));
            }
            event = receiver.recv() => match event {
                Some(event) => {
                    if let Some(window) = coalescing_window(&event.event) {
                        let event_deadline = tokio::time::Instant::now() + window;
                        deadline = Some(flush_at.min(event_deadline));
                        pending.push(event);
                    } else {
                        let mut batch = coalesce_events(pending);
                        batch.push(event);
                        return Some(batch);
                    }
                }
                None => return Some(coalesce_events(pending)),
            }
        }
    }
}

fn coalescing_window(event: &AgentEvent) -> Option<Duration> {
    match event {
        AgentEvent::ContentDelta { .. } => Some(DELTA_TICK),
        AgentEvent::ItemUpdated { .. } => Some(UPDATE_TICK),
        _ => None,
    }
}

/// Merges adjacent deltas and keeps only the last item patch per item in one window.
///
/// §5 coalesces deltas in the daemon "one event per item per ~16 ms … so a fast model does not
/// schedule a render per token". Keeping the empty sequence positions of the merged tokens does
/// not do that: every one of them is still a stored line, a broadcast frame and a projection
/// advance the client re-renders on. The run collapses to one event, and one sequence, instead.
pub(super) fn coalesce_events(events: Vec<ProviderEvent>) -> Vec<ProviderEvent> {
    let last_updates = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event.event {
            AgentEvent::ItemUpdated { item, .. } => Some((item, index)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let mut merged: Vec<ProviderEvent> = Vec::with_capacity(events.len());
    let mut previous_delta = None;
    for (index, next) in events.into_iter().enumerate() {
        let delta_key = match &next.event {
            AgentEvent::ContentDelta { item, stream, .. } => Some((*item, *stream)),
            _ => None,
        };
        let joined = match (
            previous_delta.as_ref(),
            merged.last_mut().map(|last| &mut last.event),
            &next.event,
        ) {
            (
                Some((previous_item, previous_stream)),
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
            ) if previous_item == next_item
                && previous_stream == next_stream
                && item == next_item
                && stream == next_stream =>
            {
                delta.push_str(next_delta);
                true
            }
            _ => false,
        };
        let superseded_update = match &next.event {
            AgentEvent::ItemUpdated { item, .. } => last_updates.get(item) != Some(&index),
            _ => false,
        };
        if !joined && !superseded_update {
            merged.push(next);
        }
        previous_delta = delta_key;
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
        let coalesced = coalesce_events(vec![
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
        let coalesced = coalesce_events(vec![
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

    #[test]
    fn a_superseded_update_still_breaks_delta_adjacency() {
        let item = ItemId::new();
        let coalesced = coalesce_events(vec![
            delta(item, "a"),
            update(item, fleet_core::agents::ItemStatus::InProgress),
            delta(item, "b"),
            update(item, fleet_core::agents::ItemStatus::Completed),
        ]);

        assert_eq!(coalesced.len(), 3);
        assert!(matches!(
            coalesced[0].event,
            AgentEvent::ContentDelta { .. }
        ));
        assert!(matches!(
            coalesced[1].event,
            AgentEvent::ContentDelta { .. }
        ));
        assert!(matches!(coalesced[2].event, AgentEvent::ItemUpdated { .. }));
    }

    fn update(item: ItemId, status: fleet_core::agents::ItemStatus) -> ProviderEvent {
        AgentEvent::ItemUpdated {
            item,
            patch: fleet_core::agents::ItemPatch {
                payload: None,
                status: Some(status),
            },
        }
        .into()
    }

    #[tokio::test(start_paused = true)]
    async fn a_structural_event_flushes_stream_work_without_advancing_the_clock() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let item = ItemId::new();
        sender.send(delta(item, "a")).expect("receiver is open");
        let task = tokio::spawn(async move { next_coalesced_batch(&mut receiver).await });
        tokio::task::yield_now().await;
        let before = tokio::time::Instant::now();
        sender
            .send(
                AgentEvent::ItemCompleted {
                    item,
                    status: fleet_core::agents::ItemStatus::Completed,
                }
                .into(),
            )
            .expect("receiver is open");
        let batch = task
            .await
            .expect("coalescer task completes")
            .expect("a batch");

        assert_eq!(tokio::time::Instant::now(), before);
        assert!(matches!(batch[0].event, AgentEvent::ContentDelta { .. }));
        assert!(matches!(batch[1].event, AgentEvent::ItemCompleted { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_item_updates_are_last_wins_at_fifty_milliseconds() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let item = ItemId::new();
        sender
            .send(update(item, fleet_core::agents::ItemStatus::InProgress))
            .expect("receiver is open");
        sender
            .send(update(item, fleet_core::agents::ItemStatus::Completed))
            .expect("receiver is open");
        let task = tokio::spawn(async move { next_coalesced_batch(&mut receiver).await });
        tokio::task::yield_now().await;
        tokio::time::advance(UPDATE_TICK).await;
        let batch = task
            .await
            .expect("coalescer task completes")
            .expect("a batch");

        assert_eq!(batch.len(), 1);
        let AgentEvent::ItemUpdated { patch, .. } = &batch[0].event else {
            panic!("the last update is retained");
        };
        assert_eq!(
            patch.status,
            Some(fleet_core::agents::ItemStatus::Completed)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_delta_behind_an_update_keeps_the_sixteen_millisecond_deadline() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let item = ItemId::new();
        sender
            .send(update(item, fleet_core::agents::ItemStatus::InProgress))
            .expect("receiver is open");
        let task = tokio::spawn(async move { next_coalesced_batch(&mut receiver).await });
        tokio::task::yield_now().await;
        sender.send(delta(item, "a")).expect("receiver is open");
        tokio::task::yield_now().await;
        assert!(!task.is_finished(), "the delta still owns a merge window");
        tokio::time::advance(DELTA_TICK).await;
        let batch = task
            .await
            .expect("coalescer task completes")
            .expect("a batch");

        assert_eq!(batch.len(), 2);
        assert!(matches!(batch[0].event, AgentEvent::ItemUpdated { .. }));
        assert!(matches!(batch[1].event, AgentEvent::ContentDelta { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn terminal_events_are_neither_dropped_nor_reordered() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let item = ItemId::new();
        let turn = TurnId::new();
        sender.send(delta(item, "done")).expect("receiver is open");
        sender
            .send(
                AgentEvent::ItemCompleted {
                    item,
                    status: fleet_core::agents::ItemStatus::Completed,
                }
                .into(),
            )
            .expect("receiver is open");
        sender
            .send(
                AgentEvent::TurnSettled {
                    turn,
                    outcome: fleet_core::agents::TurnOutcome::Completed,
                    usage: fleet_core::agents::Usage::default(),
                    duration_ms: 1,
                    files_changed: Vec::new(),
                }
                .into(),
            )
            .expect("receiver is open");

        let first = next_coalesced_batch(&mut receiver)
            .await
            .expect("first batch");
        let second = next_coalesced_batch(&mut receiver)
            .await
            .expect("second batch");
        assert!(matches!(first[0].event, AgentEvent::ContentDelta { .. }));
        assert!(matches!(first[1].event, AgentEvent::ItemCompleted { .. }));
        assert!(matches!(second[0].event, AgentEvent::TurnSettled { .. }));
    }
}
