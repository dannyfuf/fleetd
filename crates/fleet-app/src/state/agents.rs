use super::*;

use fleet_client::{AgentMirror, MirrorOutcome};
use fleet_core::{
    agents::{
        AgentThreadSummary, Attention, AttentionKind, Seq, SeqEvent, ThreadId, ThreadProjection,
    },
    ids::WorktreeId,
};

use crate::screens::agent_thread::{decisions::decision_context, presentation::tab_title};

/// The context bar's `N needs you · N working · N failed`, across every thread in the snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentCounts {
    /// Threads blocked on a permission, a question, a plan, or a fresh completion.
    pub needs_you: usize,
    /// Threads whose provider, turn or background work is alive.
    pub working: usize,
    /// Threads whose session or turn failed.
    pub failed: usize,
}

impl AgentCounts {
    /// Whether any counter is non-zero, which is what zero-suppresses the chips (§2.3).
    #[must_use]
    pub const fn any(self) -> bool {
        self.needs_you > 0 || self.working > 0 || self.failed > 0
    }
}

/// The client's native-agent mirror: daemon summaries, opened projections and seen cursors.
///
/// The daemon owns thread truth, so the summary list is replaced wholesale by every snapshot;
/// the app only adds what is local to a client — which tab is selected per worktree, which
/// sequence this window has actually shown the user, and which threads need a resync.
#[derive(Debug, Default)]
pub struct AgentThreads {
    /// Daemon summaries in snapshot order; the tab strip inherits that order.
    summaries: Vec<AgentThreadSummary>,
    /// Projections of the threads this client has opened.
    mirror: AgentMirror,
    /// The agent tab selected in each worktree's workspace, when one is.
    active: HashMap<WorktreeId, ThreadId>,
    /// Threads whose event stream had a gap and must be re-opened from `last_seq`.
    resync: HashSet<ThreadId>,
    /// The last attention each thread was notified about, so an edge fires exactly once.
    notified: HashMap<ThreadId, Attention>,
    /// Provider slash commands from `SessionStarted`, which the projection does not carry.
    commands: HashMap<ThreadId, Vec<String>>,
    /// Threads whose composer is being typed into while a card is open.
    composing: HashSet<ThreadId>,
    /// Threads whose transcript has its tail frozen (`^s [`).
    scrolling: HashSet<ThreadId>,
    /// The question an open multi-question card's bare keys currently address.
    question_cursor: HashMap<ThreadId, usize>,
    /// The last cursor re-reported to the daemon for a thread it had forgotten.
    reported: HashMap<ThreadId, Seq>,
    /// Threads whose tab `^s x` closed. The daemon keeps listing them (§6 leaves every
    /// thread browsable), so the closed set is what actually takes the tab out of the strip.
    closed: HashSet<ThreadId>,
}

impl AgentThreads {
    /// Every thread the daemon lists, in snapshot order.
    #[must_use]
    pub fn summaries(&self) -> &[AgentThreadSummary] {
        &self.summaries
    }

    /// The threads of one worktree, which are that workspace's agent tabs.
    ///
    /// A thread the user closed with `^s x` is not one of them: the daemon still lists it, and
    /// still counts it in the context bar, but this window has put it away.
    #[must_use]
    pub fn of_worktree(&self, worktree: &WorktreeId) -> Vec<&AgentThreadSummary> {
        self.summaries
            .iter()
            .filter(|summary| {
                &summary.worktree == worktree && !self.closed.contains(&summary.thread)
            })
            .collect()
    }

    /// Whether this window has closed the thread's tab.
    #[must_use]
    pub fn is_closed(&self, thread: ThreadId) -> bool {
        self.closed.contains(&thread)
    }

    /// Takes one thread's tab out of the strip (`^s x`), leaving the thread itself running.
    ///
    /// Returns whether the strip actually changed.
    pub fn close(&mut self, thread: ThreadId) -> bool {
        self.closed.insert(thread)
    }

    /// One thread's summary.
    #[must_use]
    pub fn summary(&self, thread: ThreadId) -> Option<&AgentThreadSummary> {
        self.summaries
            .iter()
            .find(|summary| summary.thread == thread)
    }

    /// One thread's installed projection, when this client has opened it.
    #[must_use]
    pub fn projection(&self, thread: ThreadId) -> Option<&ThreadProjection> {
        self.mirror.projections.get(&thread)
    }

    /// The sequence this client has reported as seen for a thread.
    #[must_use]
    pub fn seen(&self, thread: ThreadId) -> Seq {
        self.mirror
            .last_seen
            .get(&thread)
            .copied()
            .unwrap_or_default()
    }

    /// The attention a client actually shows.
    ///
    /// §3.3 puts the view axis "per client, not persisted daemon-side": the daemon broadcasts
    /// one summary derived against an unseen thread, and each client narrows the two
    /// seen-relative attentions with the cursor it holds itself. Two windows on one thread
    /// therefore disagree honestly instead of clearing each other's amber dot.
    #[must_use]
    pub fn attention(&self, thread: ThreadId) -> Attention {
        let Some(summary) = self.summary(thread) else {
            return Attention::Idle;
        };
        let seen = self.seen(thread);
        match summary.attention_for(seen) {
            // A cursor at the tail is the tab the user is looking at, whatever the last stored
            // completion sequence was.
            Attention::NeedsYou(AttentionKind::Finished) | Attention::Unread
                if seen >= summary.last_seq =>
            {
                Attention::Idle
            }
            attention => attention,
        }
    }

    /// The §3.3 counters the context bar shows, including the thread on the current tab.
    #[must_use]
    pub fn counts(&self) -> AgentCounts {
        let mut counts = AgentCounts::default();
        for summary in &self.summaries {
            match self.attention(summary.thread) {
                Attention::NeedsYou(_) => counts.needs_you += 1,
                Attention::Failed => counts.failed += 1,
                Attention::Working => counts.working += 1,
                Attention::Unread | Attention::Idle => {}
            }
        }
        counts
    }

    /// The agent tab selected in a worktree's workspace.
    #[must_use]
    pub fn active(&self, worktree: &WorktreeId) -> Option<ThreadId> {
        self.active.get(worktree).copied()
    }

    /// Selects an agent tab, replacing whichever one that worktree showed.
    pub fn activate(&mut self, worktree: WorktreeId, thread: ThreadId) {
        self.active.insert(worktree, thread);
    }

    /// Returns to the terminal tabs of a worktree.
    pub fn deactivate(&mut self, worktree: &WorktreeId) -> bool {
        self.active.remove(worktree).is_some()
    }

    /// Records whether the composer, rather than the open card, owns the bare letters.
    ///
    /// Returns whether the answer changed, because it decides the key context and a change
    /// therefore has to reach the next frame.
    pub fn set_composing(&mut self, thread: ThreadId, composing: bool) -> bool {
        if composing {
            self.composing.insert(thread)
        } else {
            self.composing.remove(&thread)
        }
    }

    /// Whether a thread's composer currently owns the bare decision letters.
    #[must_use]
    pub fn is_composing(&self, thread: ThreadId) -> bool {
        self.composing.contains(&thread)
    }

    /// Records the question an open card's bare keys address.
    ///
    /// §2 has the status bar mirror the card's keys, and on a 2-4 question gate the advertised
    /// `1–N` range and `space` follow the cursor: without the cursor here the bar would keep
    /// showing question 0's set while the card showed the cursor question's. Returns whether
    /// the answer changed, so a change reaches the next frame.
    pub fn set_question_cursor(&mut self, thread: ThreadId, cursor: usize) -> bool {
        self.question_cursor.insert(thread, cursor) != Some(cursor)
    }

    /// The question an open card's bare keys address.
    #[must_use]
    pub fn question_cursor(&self, thread: ThreadId) -> usize {
        self.question_cursor.get(&thread).copied().unwrap_or(0)
    }

    /// Records whether a thread's transcript has its tail frozen.
    ///
    /// Returns whether the answer changed: it decides both the key context and the status
    /// bar's mode word, so a change has to reach the next frame.
    pub fn set_scrolling(&mut self, thread: ThreadId, scrolling: bool) -> bool {
        if scrolling {
            self.scrolling.insert(thread)
        } else {
            self.scrolling.remove(&thread)
        }
    }

    /// Whether a thread's transcript is in scroll mode.
    #[must_use]
    pub fn is_scrolling(&self, thread: ThreadId) -> bool {
        self.scrolling.contains(&thread)
    }

    /// §3.3's `Working`: a running turn, a live session, or a background task still alive.
    ///
    /// This is the one predicate the `Agent > AgentWorking` context, the status bar's key row
    /// and the composer's queue decision all read. `Attention` is a *tab badge* and answers a
    /// different question — a plan gate on a running turn is `NeedsYou(Plan)` while the keys
    /// that fire are still the working ones — so deriving the advertised keys from it listed
    /// commands nothing was bound to.
    #[must_use]
    pub fn is_working(&self, thread: ThreadId) -> bool {
        self.projection(thread).is_some_and(|projection| {
            matches!(projection.turn, fleet_core::agents::TurnState::Running(_))
                || projection.session == fleet_core::agents::SessionState::Running
                || !projection.background_tasks.is_empty()
        })
    }

    /// Whether a thread's event stream must be re-opened before deltas may be applied.
    #[must_use]
    pub fn needs_resync(&self, thread: ThreadId) -> bool {
        self.resync.contains(&thread)
    }

    /// Records that a thread must be re-opened.
    pub fn mark_resync(&mut self, thread: ThreadId) {
        self.resync.insert(thread);
    }

    /// Clears the flag while one open request is in flight, so a frame cannot spam the daemon.
    pub fn clear_resync(&mut self, thread: ThreadId) {
        self.resync.remove(&thread);
    }

    /// Replaces a projection with a daemon snapshot and applies its ordered tail.
    pub fn install_snapshot(&mut self, projection: ThreadProjection, events: &[SeqEvent]) {
        let thread = projection.thread;
        match self.mirror.install_snapshot(projection, events) {
            MirrorOutcome::Gap { .. } => {
                self.resync.insert(thread);
            }
            // A replay is already installed, and an event the reducer refused would come back
            // from the daemon unchanged: neither is worth another open request.
            MirrorOutcome::Applied
            | MirrorOutcome::Duplicate { .. }
            | MirrorOutcome::Rejected { .. } => {
                self.resync.remove(&thread);
            }
        }
    }

    /// The provider slash commands `/` completes in one thread.
    #[must_use]
    pub fn commands(&self, thread: ThreadId) -> Vec<String> {
        self.commands.get(&thread).cloned().unwrap_or_default()
    }

    /// Applies one sequenced event, reporting whether the caller must resync the thread.
    pub fn apply_event(&mut self, thread: ThreadId, event: &SeqEvent) -> MirrorOutcome {
        if let fleet_core::agents::AgentEvent::SessionStarted { commands, .. } = &event.event {
            self.commands.insert(thread, commands.clone());
        }
        if !self.mirror.projections.contains_key(&thread) {
            // A thread this client has not opened has no projection to advance; its tab still
            // updates from the summary the daemon broadcasts beside the event.
            return MirrorOutcome::Applied;
        }
        let outcome = self.mirror.apply_event(thread, event);
        if matches!(outcome, MirrorOutcome::Gap { .. }) {
            self.resync.insert(thread);
        }
        outcome
    }

    /// Patches one broadcast summary into the mirror, preserving snapshot order.
    pub fn apply_summary(&mut self, summary: AgentThreadSummary) {
        match self
            .summaries
            .iter_mut()
            .find(|existing| existing.thread == summary.thread)
        {
            Some(existing) => *existing = summary,
            None => self.summaries.push(summary),
        }
    }

    /// Records the sequence the user has actually seen in a focused tab.
    pub fn mark_seen(&mut self, thread: ThreadId, seq: Seq) {
        self.mirror.mark_seen(thread, seq);
    }

    /// Threads the daemon is showing as unread that this window has already read.
    ///
    /// §3.3 clears `NeedsYou(Finished)` when the user views the tab, and the seen cursor lives
    /// in the daemon's runtime only — a daemon restart resets it to zero, so every tab the user
    /// had already read comes back amber. The local cursor survives, so it is re-reported for
    /// every thread rather than only for the one tab that happens to be visible.
    #[must_use]
    pub fn stale_seen(&self) -> Vec<(ThreadId, Seq)> {
        self.summaries
            .iter()
            .filter(|summary| {
                matches!(
                    summary.attention,
                    Attention::NeedsYou(AttentionKind::Finished) | Attention::Unread
                )
            })
            .filter(|summary| self.seen(summary.thread) >= summary.last_seq)
            .filter(|summary| {
                self.reported.get(&summary.thread) != Some(&self.seen(summary.thread))
            })
            .map(|summary| (summary.thread, self.seen(summary.thread)))
            .collect()
    }

    /// Records that a cursor was re-reported, so one restart costs one request per thread.
    pub fn mark_reported(&mut self, thread: ThreadId, seq: Seq) {
        self.reported.insert(thread, seq);
    }

    /// Records current attentions without presenting them, the way a fresh link is adopted.
    ///
    /// A thread that was already blocked before this window connected is not news, so the first
    /// snapshot of a connection seeds the edge detector instead of firing every state at once.
    pub fn seed(&mut self, threads: &[AgentThreadSummary]) {
        self.notified = threads
            .iter()
            .map(|summary| (summary.thread, summary.attention))
            .collect();
    }

    /// Replaces the summary list from an authoritative snapshot and forgets vanished threads.
    pub fn sync_snapshot(&mut self, threads: Vec<AgentThreadSummary>) {
        let live: HashSet<ThreadId> = threads.iter().map(|summary| summary.thread).collect();
        self.summaries = threads;
        self.mirror
            .projections
            .retain(|thread, _| live.contains(thread));
        self.mirror
            .last_seen
            .retain(|thread, _| live.contains(thread));
        self.resync.retain(|thread| live.contains(thread));
        self.notified.retain(|thread, _| live.contains(thread));
        self.commands.retain(|thread, _| live.contains(thread));
        self.composing.retain(|thread| live.contains(thread));
        self.scrolling.retain(|thread| live.contains(thread));
        self.question_cursor
            .retain(|thread, _| live.contains(thread));
        self.reported.retain(|thread, _| live.contains(thread));
        self.active.retain(|_, thread| live.contains(thread));
    }

    /// The threads that just entered an attention worth a notification, and their tab labels.
    ///
    /// Only the two states the design treats as signals — `needs you` and `failed` — fire, and
    /// each fires once per entry, so a thread that stays blocked does not notify on every event.
    pub fn attention_edges(&mut self) -> Vec<(String, Attention)> {
        let mut edges = Vec::new();
        for summary in &self.summaries {
            let seen = self
                .mirror
                .last_seen
                .get(&summary.thread)
                .copied()
                .unwrap_or_default();
            let attention = match summary.attention_for(seen) {
                Attention::NeedsYou(AttentionKind::Finished) | Attention::Unread
                    if seen >= summary.last_seq =>
                {
                    Attention::Idle
                }
                attention => attention,
            };
            let previous = self.notified.insert(summary.thread, attention);
            if previous == Some(attention) {
                continue;
            }
            if matches!(attention, Attention::NeedsYou(_) | Attention::Failed) {
                edges.push((tab_title(summary), attention));
            }
        }
        edges
    }
}

impl AppState {
    /// The agent thread the Workspace is showing, when an agent tab is selected.
    #[must_use]
    pub fn active_agent_thread(&self) -> Option<ThreadId> {
        let session = self.active_session()?;
        let fleet_core::sessions::SessionKind::Worktree(worktree) = &session.kind else {
            return None;
        };
        let thread = self.agents.active(worktree)?;
        self.agents.summary(thread).map(|summary| summary.thread)
    }

    /// The `Agent > …` key contexts an active agent tab owns (§9).
    ///
    /// The decision sub-context is derived from the open gate rather than from the view, so the
    /// keyboard is routed to the card in the same frame the gate appears.
    #[must_use]
    pub fn agent_context_chain(&self) -> Option<Vec<&'static str>> {
        let thread = self.active_agent_thread()?;
        // §9 makes `^s [` a real mode: while the tail is frozen the transcript owns `j`/`k`,
        // half/page and `gg`/`G`, exactly as the terminal scroll mode does, and nothing the
        // composer binds may swallow them.
        if self.agents.is_scrolling(thread) {
            return Some(vec!["Agent", "AgentNativeScroll"]);
        }
        let projection = self.agents.projection(thread);
        let gate = projection.and_then(|projection| projection.gates.last());
        // §9 routes the bare letters to the card — but a payload being corrected after `e`, and
        // a plan note being written for `n`, are text. While one of those is in progress the
        // composer keeps its own keys, or the note could not contain a `y` or an `n`.
        if let Some(gate) = gate
            && !self.agents.is_composing(thread)
        {
            return Some(vec!["Agent", "AgentDecision", decision_context(gate)]);
        }
        Some(vec![
            "Agent",
            if self.agents.is_working(thread) {
                "AgentWorking"
            } else {
                "AgentIdle"
            },
        ])
    }

    /// Applies one native-agent event to the mirror, returning a thread that needs a resync.
    pub fn apply_agent_event(&mut self, thread: ThreadId, event: &SeqEvent) -> Option<ThreadId> {
        match self.agents.apply_event(thread, event) {
            MirrorOutcome::Gap { .. } => Some(thread),
            MirrorOutcome::Applied
            | MirrorOutcome::Duplicate { .. }
            | MirrorOutcome::Rejected { .. } => None,
        }
    }

    /// Applies one broadcast summary and presents whatever attention edge it created.
    pub fn apply_agent_summary(&mut self, summary: AgentThreadSummary, now: Instant) {
        self.agents.apply_summary(summary);
        self.notify_agent_attention(now);
    }

    /// Presents every fresh `needs you` / `failed` edge through the activity notification path.
    pub(super) fn notify_agent_attention(&mut self, now: Instant) {
        for (label, attention) in self.agents.attention_edges() {
            self.notify_agent_thread(&label, attention, now);
        }
    }
}

#[cfg(test)]
mod tests;
