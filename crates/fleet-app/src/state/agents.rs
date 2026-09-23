use super::*;

use fleet_client::{AgentMirror, MirrorOutcome, PageOutcome};
use fleet_core::{
    agents::{
        AgentThreadSummary, Applied, Attention, AttentionKind, Delegation, DelegationId,
        DelegationStatus, PermissionMode, Seq, SeqEvent, ThreadId, ThreadProjection,
    },
    ids::{BoardId, CardId, WorktreeId},
};

use fleet_proto::agents::AgentThreadWindow;

use crate::screens::agent_thread::{
    PreparedDecisionObservable, decisions::decision_context, presentation::tab_title,
};

/// `N needs you · N working · N failed`, across every top-level thread in the snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentCounts {
    /// Threads blocked on a permission, a question, a plan, or a fresh completion.
    pub needs_you: usize,
    /// Threads whose provider, turn or background work is alive.
    pub working: usize,
    /// Threads whose session or turn failed.
    pub failed: usize,
}

#[derive(Debug, Default)]
struct AgentDerived {
    attention: HashMap<ThreadId, Attention>,
    counts: AgentCounts,
    /// The one top-level thread that needs you, while exactly one does.
    waiting: Option<ThreadId>,
    strip_offsets: HashMap<ThreadId, usize>,
    /// Callers with at least one live durable child, rebuilt only when that census changes.
    live_delegation_callers: HashSet<ThreadId>,
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
/// The daemon owns thread truth, so the summary list is replaced by every snapshot — except for
/// a thread no snapshot has listed yet ([`Self::sync_snapshot`]); the app only adds what is local to a client — which tab is selected per worktree, the
/// installation's persisted seen cursor plus this process's newer override, and resync state.
#[derive(Debug, Default)]
pub struct AgentThreads {
    /// Daemon summaries in snapshot order; the tab strip inherits that order.
    summaries: Vec<AgentThreadSummary>,
    summaries_revision: u64,
    /// Moves when a harness declares the models and efforts it offers.
    ///
    /// The vocabulary lives on an installed *projection*, not on a summary, so nothing else
    /// here moves when it lands: a Model or Effort list keyed on `summaries_revision` alone
    /// would serve an empty catalogue until something unrelated changed.
    vocabulary_revision: u64,
    seen_revision: u64,
    /// Durable caller-to-child records, replaced by the capability-gated seed after Hello.
    delegations: HashMap<DelegationId, Delegation>,
    /// Delegation ids in creation order, independent of hash-map iteration order.
    delegation_order: Vec<DelegationId>,
    /// Invalidates transcript rows and harness snapshots when a delegation changes.
    delegations_revision: u64,
    /// Child threads this window chose to show in a tab strip.
    attached: HashSet<ThreadId>,
    /// Invalidates tab projections when the local attached/closed choice changes.
    attached_revision: u64,
    /// Projections of the threads this client has opened.
    mirror: AgentMirror,
    /// The agent tab selected in each worktree's workspace, when one is.
    active: HashMap<WorktreeId, ThreadId>,
    /// Threads whose event stream had a gap and must be re-opened from `last_seq`.
    resync: HashSet<ThreadId>,
    /// The last attention each thread was notified about, so an edge fires exactly once.
    notified: HashMap<ThreadId, Attention>,
    /// Harness slash commands from `SessionConfigured`, which the projection does not carry.
    commands: HashMap<ThreadId, Vec<String>>,
    /// Harness skills, which `$` completes and the projection does not carry either.
    skills: HashMap<ThreadId, Vec<String>>,
    /// Harness-supported permission modes in picker order.
    modes: HashMap<ThreadId, Vec<PermissionMode>>,
    /// Prepared drawer observables relayed by mounted thread views.
    decisions: HashMap<ThreadId, PreparedDecisionObservable>,
    /// The cursor a daemon-declared resync must resume from, per thread.
    resume_from: HashMap<ThreadId, Seq>,
    /// Threads whose composer is being typed into while a card is open.
    composing: HashSet<ThreadId>,
    /// Threads whose transcript has its tail frozen (`^s [`).
    scrolling: HashSet<ThreadId>,
    /// The question an open multi-question card's bare keys currently address.
    question_cursor: HashMap<ThreadId, usize>,
    /// The last cursor re-reported to the daemon for a thread it had forgotten.
    reported: HashMap<ThreadId, Seq>,
    /// Threads whose tab `^s x` closed. The daemon seeds this set on Hello, and snapshots never
    /// prune it, so an omitted thread stays closed if the daemon lists it again later.
    closed: HashSet<ThreadId>,
    /// Threads whose transcript has a row focused inside scroll mode.
    ///
    /// §12 puts row focus **inside** scroll mode, which is what finally makes `⏎`/`u`/`o`/`y`/`d`
    /// fire: the `AgentRow` context is only ever on the chain under `AgentNativeScroll`.
    row_focus: HashSet<ThreadId>,
    /// Focused transcript row kind per mounted thread, for harness assertions.
    focused_rows: HashMap<ThreadId, &'static str>,
    /// Expanded delegation-result cards per mounted thread, for harness assertions.
    expanded_result_cards: HashMap<ThreadId, u32>,
    /// The one agent tab whose composer must take focus after its next mounted frame.
    focus_composer: Option<ThreadId>,
    /// The active thread whose mounted composer most recently proved it held focus.
    composer_focused: Option<ThreadId>,
    /// Narrow damage reported by the latest accepted event for each opened thread.
    last_applied: HashMap<ThreadId, Applied>,
    /// Threads whose authoritative projection was replaced outside the live event stream.
    projection_replaced: HashSet<ThreadId>,
    /// Threads a summary introduced that no snapshot has listed yet (see [`Self::sync_snapshot`]).
    unconfirmed: HashSet<ThreadId>,
    /// `^s a` / `^s A` presses still waiting for their thread (`pending`).
    pending_creates: Vec<pending::PendingCreate>,
    /// The last [`CreateToken`] handed out.
    next_create: u64,
    /// Prepared foreground data. Render getters only read this cache.
    derived: AgentDerived,
}

impl AgentThreads {
    /// Every thread the daemon lists, in snapshot order.
    #[must_use]
    pub fn summaries(&self) -> &[AgentThreadSummary] {
        &self.summaries
    }

    /// Scalar generation for consumers whose projection depends on the summary census.
    #[must_use]
    pub const fn summaries_revision(&self) -> u64 {
        self.summaries_revision
    }

    /// The revision of the declared model and effort vocabulary (see the field).
    #[must_use]
    pub const fn vocabulary_revision(&self) -> u64 {
        self.vocabulary_revision
    }

    /// Scalar generation for consumers whose projection depends on installation-local cursors.
    #[must_use]
    pub const fn seen_revision(&self) -> u64 {
        self.seen_revision
    }

    /// The threads of one worktree, which are that workspace's agent tabs.
    ///
    /// A thread the user closed with `^s x` is not one of them: the daemon still lists it, and
    /// still counts it in the context bar, but this window has put it away.
    #[must_use]
    pub fn of_worktree(&self, worktree: &WorktreeId) -> Vec<&AgentThreadSummary> {
        let mut visible = self
            .summaries
            .iter()
            .filter(|summary| {
                &summary.worktree == worktree
                    && self.derived.strip_offsets.contains_key(&summary.thread)
            })
            .collect::<Vec<_>>();
        visible.sort_by_key(|summary| self.derived.strip_offsets[&summary.thread]);
        visible
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
        let changed = self.closed.insert(thread);
        if changed {
            self.bump_attached_revision();
        }
        changed
    }

    /// Whether this thread currently belongs to its worktree's tab strip.
    #[must_use]
    pub fn is_attached(&self, thread: ThreadId) -> bool {
        self.summary(thread).is_some_and(|summary| {
            if summary.parent.is_some() {
                self.attached.contains(&thread) && !self.closed.contains(&thread)
            } else {
                !self.closed.contains(&thread)
            }
        })
    }

    /// Shows a delegated child in its worktree and clears a stale local close marker.
    pub fn attach(&mut self, thread: ThreadId) -> bool {
        let mut changed = self.closed.remove(&thread);
        if self.caller_of(thread).is_some() {
            changed |= self.attached.insert(thread);
        }
        if changed {
            self.bump_attached_revision();
        }
        changed
    }

    /// Hides a delegated child without changing the daemon-owned thread.
    pub fn detach(&mut self, thread: ThreadId) -> bool {
        let changed = self.attached.remove(&thread);
        if changed {
            self.bump_attached_revision();
        }
        changed
    }

    /// Reopens a caller tab previously hidden with `close`.
    pub fn reopen(&mut self, thread: ThreadId) -> bool {
        let changed = self.closed.remove(&thread);
        if changed {
            self.bump_attached_revision();
        }
        changed
    }

    /// The parent declared by one thread's summary.
    #[must_use]
    pub fn caller_of(&self, thread: ThreadId) -> Option<ThreadId> {
        self.summary(thread).and_then(|summary| summary.parent)
    }

    /// Direct children of a caller in daemon creation order.
    #[must_use]
    pub fn children_of(&self, caller: ThreadId) -> Vec<&AgentThreadSummary> {
        self.summaries
            .iter()
            .filter(|summary| summary.parent == Some(caller))
            .collect()
    }

    /// One durable delegation record.
    #[must_use]
    pub fn delegation(&self, id: DelegationId) -> Option<&Delegation> {
        self.delegations.get(&id)
    }

    /// Delegations spawned by one caller, oldest first.
    #[must_use]
    pub fn delegations_of_caller(&self, caller: ThreadId) -> Vec<&Delegation> {
        self.delegation_order
            .iter()
            .filter_map(|id| self.delegations.get(id))
            .filter(|delegation| delegation.caller.thread() == Some(&caller))
            .collect()
    }

    /// The delegation that owns a child thread.
    #[must_use]
    pub fn delegation_of_child(&self, child: ThreadId) -> Option<&Delegation> {
        self.delegation_order
            .iter()
            .filter_map(|id| self.delegations.get(id))
            .find(|delegation| delegation.child == child)
    }

    /// The live child each card of `board` has out, keyed by card.
    ///
    /// A card-called delegation names its board and its card, so this mirror can say a card has
    /// a child out *before* any board response carries the run: `DelegationChanged` arrives the
    /// moment the daemon starts the child, while the card's own `runs` row only reaches the app
    /// with the next board view. The board's marks are derived from both, and this is the half
    /// that does not wait for a round trip (contracts §5.2, BOARD.md §11.8).
    ///
    /// Later entries win, so a card that has started a second run reads as its newest child.
    #[must_use]
    pub(crate) fn live_card_runs(
        &self,
        board: &BoardId,
    ) -> HashMap<&CardId, (DelegationId, DelegationStatus)> {
        self.delegation_order
            .iter()
            .filter_map(|id| self.delegations.get(id))
            .filter(|delegation| delegation.status.is_live())
            .filter_map(|delegation| {
                let (owner, card) = delegation.caller.card()?;
                (owner == board).then_some((card, (delegation.id, delegation.status)))
            })
            .collect()
    }

    /// The live child one card has out, newest first, or `None`.
    ///
    /// The per-card half of [`Self::live_card_runs`], for the surfaces that ask about one card
    /// rather than fold a whole board: the run keys and the move confirm. It joins on the
    /// delegation's own caller — `Card { board, card }` — and never on a run id the card may
    /// not carry yet, which is the whole point of reading the mirror first (contracts §5.5,
    /// `BOARD.md` §11.8).
    #[must_use]
    pub(crate) fn live_card_run(&self, board: &BoardId, card: &CardId) -> Option<&Delegation> {
        self.delegation_order
            .iter()
            .rev()
            .filter_map(|id| self.delegations.get(id))
            .filter(|delegation| delegation.status.is_live())
            .find(|delegation| delegation.caller.card() == Some((board, card)))
    }

    /// Every delegation in creation order, for stable harness projection.
    #[must_use]
    pub(crate) fn delegations(&self) -> Vec<&Delegation> {
        self.delegation_order
            .iter()
            .filter_map(|id| self.delegations.get(id))
            .collect()
    }

    /// Generation consumed by transcript and harness projection keys.
    #[must_use]
    pub const fn delegations_revision(&self) -> u64 {
        self.delegations_revision
    }

    /// Generation consumed by tab-strip and harness projection keys.
    #[must_use]
    pub const fn attached_revision(&self) -> u64 {
        self.attached_revision
    }

    /// Replaces the capability-gated delegation census received after Hello.
    pub fn seed_delegations(&mut self, mut list: Vec<Delegation>) {
        list.sort_by_key(|delegation| (delegation.created, delegation.id));
        let next_order = list
            .iter()
            .map(|delegation| delegation.id)
            .collect::<Vec<_>>();
        let next = list
            .into_iter()
            .map(|delegation| (delegation.id, delegation))
            .collect::<HashMap<_, _>>();
        if self.delegations == next && self.delegation_order == next_order {
            return;
        }
        self.delegations = next;
        self.delegation_order = next_order;
        self.delegations_revision = self.delegations_revision.wrapping_add(1);
        self.prepare_live_delegation_callers();
    }

    /// Applies one changed delegation, advancing the row generation only for a real change.
    pub fn apply_delegation(&mut self, delegation: Delegation) {
        if self.delegations.get(&delegation.id) == Some(&delegation) {
            return;
        }
        let id = delegation.id;
        let caller = delegation.caller.thread().copied();
        let caller_item = delegation.caller_item;
        let is_new = !self.delegations.contains_key(&id);
        self.delegations.insert(id, delegation);
        if is_new {
            self.delegation_order.push(id);
            self.delegation_order.sort_by_key(|id| {
                self.delegations
                    .get(id)
                    .map(|delegation| (delegation.created, delegation.id))
            });
        }
        if let Some(caller) = caller
            && self
                .mirror
                .projections
                .get(&caller)
                .is_some_and(|projection| {
                    !projection
                        .items
                        .iter()
                        .any(|item| Some(item.id) == caller_item)
                })
        {
            // `DelegationChanged` and the caller's item event travel independently. If this
            // window observed the durable record first, re-open its caller window so the row
            // cannot stay absent after a brief event-stream race.
            self.resync.insert(caller);
        }
        self.delegations_revision = self.delegations_revision.wrapping_add(1);
        self.prepare_live_delegation_callers();
    }

    fn prepare_live_delegation_callers(&mut self) {
        self.derived.live_delegation_callers = self
            .delegations
            .values()
            .filter(|delegation| delegation.status.is_live())
            .filter_map(|delegation| delegation.caller.thread().copied())
            .collect();
    }

    fn bump_attached_revision(&mut self) {
        self.attached_revision = self.attached_revision.wrapping_add(1);
        self.prepare_strip_offsets();
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

    /// Narrow damage reported by the latest accepted event for one thread.
    #[must_use]
    pub fn last_applied(&self, thread: ThreadId) -> Option<&Applied> {
        self.last_applied.get(&thread)
    }

    /// Takes the one-shot signal that a mounted view must adopt a replacement projection.
    pub fn take_projection_replaced(&mut self, thread: ThreadId) -> bool {
        self.projection_replaced.remove(&thread)
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
    /// §3.3 puts the view axis per installation. The daemon broadcasts one shared summary and
    /// each installation narrows the two seen-relative attentions with its own persisted cursor.
    /// Windows from one installation share that identity; separate installations do not.
    #[must_use]
    pub fn attention(&self, thread: ThreadId) -> Attention {
        self.derived
            .attention
            .get(&thread)
            .copied()
            .unwrap_or(Attention::Idle)
    }

    fn own_attention(&self, summary: &AgentThreadSummary) -> Attention {
        let seen = self.seen(summary.thread);
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

    fn attention_in(&self, thread: ThreadId, summaries: &[AgentThreadSummary]) -> Attention {
        let Some(summary) = summaries.iter().find(|summary| summary.thread == thread) else {
            return Attention::Idle;
        };
        let own = self.own_attention(summary);
        summaries
            .iter()
            .filter(|child| child.parent == Some(thread))
            .filter_map(|child| match self.own_attention(child) {
                Attention::NeedsYou(
                    kind @ (AttentionKind::Permission
                    | AttentionKind::Question
                    | AttentionKind::Plan),
                ) => Some(Attention::NeedsYou(kind)),
                Attention::Working | Attention::Waiting => Some(Attention::Working),
                Attention::NeedsYou(AttentionKind::Finished)
                | Attention::Failed
                | Attention::Unread
                | Attention::Idle => None,
            })
            .fold(own, std::cmp::max)
    }

    /// The §3.3 counters the context bar shows, including the thread on the current tab.
    #[must_use]
    pub fn counts(&self) -> AgentCounts {
        self.derived.counts
    }

    /// The top-level thread that needs you when exactly one does: what the title bar's
    /// `1 needs you` opens. With two or more waiting there is no single answer, and the button
    /// opens the agents picker instead.
    #[must_use]
    pub fn waiting_thread(&self) -> Option<ThreadId> {
        self.derived.waiting
    }

    /// Zero-based position among the native tabs of this thread's worktree.
    #[must_use]
    pub fn strip_offset(&self, thread: ThreadId) -> Option<usize> {
        self.derived.strip_offsets.get(&thread).copied()
    }

    fn prepare_attention_counts(&mut self) {
        let own = self
            .summaries
            .iter()
            .map(|summary| (summary.thread, self.own_attention(summary)))
            .collect::<HashMap<_, _>>();
        let mut attention = own.clone();
        for child in self
            .summaries
            .iter()
            .filter(|summary| summary.parent.is_some())
        {
            let Some(parent) = child.parent else {
                continue;
            };
            let propagated = match own.get(&child.thread).copied().unwrap_or(Attention::Idle) {
                Attention::NeedsYou(
                    kind @ (AttentionKind::Permission
                    | AttentionKind::Question
                    | AttentionKind::Plan),
                ) => Attention::NeedsYou(kind),
                Attention::Working | Attention::Waiting => Attention::Working,
                Attention::NeedsYou(AttentionKind::Finished)
                | Attention::Failed
                | Attention::Unread
                | Attention::Idle => continue,
            };
            attention
                .entry(parent)
                .and_modify(|current| *current = std::cmp::max(*current, propagated));
        }
        let mut counts = AgentCounts::default();
        let mut waiting = None;
        for summary in self
            .summaries
            .iter()
            .filter(|summary| summary.parent.is_none())
        {
            match attention
                .get(&summary.thread)
                .copied()
                .unwrap_or(Attention::Idle)
            {
                Attention::NeedsYou(_) => {
                    counts.needs_you += 1;
                    waiting = Some(summary.thread);
                }
                Attention::Failed => counts.failed += 1,
                Attention::Working | Attention::Waiting => counts.working += 1,
                Attention::Unread | Attention::Idle => {}
            }
        }
        self.derived.attention = attention;
        self.derived.waiting = waiting.filter(|_| counts.needs_you == 1);
        self.derived.counts = counts;
    }

    fn prepare_strip_offsets(&mut self) {
        let worktree_of = self
            .summaries
            .iter()
            .map(|summary| (summary.thread, summary.worktree.clone()))
            .collect::<HashMap<_, _>>();
        let mut children = HashMap::<ThreadId, Vec<ThreadId>>::new();
        for summary in &self.summaries {
            if let Some(parent) = summary.parent {
                children.entry(parent).or_default().push(summary.thread);
            }
        }
        let mut strip_offsets = HashMap::new();
        let mut next_by_worktree = HashMap::<WorktreeId, usize>::new();
        let mut roots = self
            .summaries
            .iter()
            .filter(|summary| summary.parent.is_none() && !self.closed.contains(&summary.thread))
            .map(|summary| summary.thread)
            .collect::<Vec<_>>();
        roots.extend(self.summaries.iter().filter_map(|summary| {
            (summary.parent.is_some()
                && self.attached.contains(&summary.thread)
                && !self.closed.contains(&summary.thread))
            .then_some(summary.thread)
        }));
        for root in roots {
            let mut stack = vec![root];
            while let Some(thread) = stack.pop() {
                let Some(worktree) = worktree_of.get(&thread) else {
                    continue;
                };
                if strip_offsets.contains_key(&thread) {
                    continue;
                }
                let offset = next_by_worktree.entry(worktree.clone()).or_default();
                strip_offsets.insert(thread, *offset);
                *offset += 1;
                if let Some(descendants) = children.get(&thread) {
                    for child in descendants.iter().rev() {
                        if worktree_of.get(child) == Some(worktree)
                            && self.attached.contains(child)
                            && !self.closed.contains(child)
                        {
                            stack.push(*child);
                        }
                    }
                }
            }
        }
        self.derived.strip_offsets = strip_offsets;
    }

    /// The agent tab selected in a worktree's workspace.
    #[must_use]
    pub fn active(&self, worktree: &WorktreeId) -> Option<ThreadId> {
        self.active.get(worktree).copied()
    }

    /// Selects an agent tab, replacing whichever one that worktree showed.
    pub fn activate(&mut self, worktree: WorktreeId, thread: ThreadId) {
        self.active.insert(worktree, thread);
        self.focus_composer = Some(thread);
        self.composer_focused = None;
    }

    /// Returns to the terminal tabs of a worktree.
    pub fn deactivate(&mut self, worktree: &WorktreeId) -> bool {
        let Some(thread) = self.active.remove(worktree) else {
            return false;
        };
        if self.focus_composer == Some(thread) {
            self.focus_composer = None;
        }
        if self.composer_focused == Some(thread) {
            self.composer_focused = None;
        }
        true
    }

    /// Consumes the one-shot focus request for `thread`.
    pub fn take_composer_focus(&mut self, thread: ThreadId) -> bool {
        if self.focus_composer != Some(thread) {
            return false;
        }
        self.focus_composer = None;
        true
    }

    /// Records whether the mounted composer actually owns a descendant focus handle.
    pub fn set_composer_focused(&mut self, thread: ThreadId, focused: bool) -> bool {
        let next = if focused {
            Some(thread)
        } else if self.composer_focused == Some(thread) {
            None
        } else {
            return false;
        };
        if self.composer_focused == next {
            return false;
        }
        self.composer_focused = next;
        true
    }

    /// Whether the mounted composer of `thread` most recently proved it held focus.
    #[must_use]
    pub fn composer_is_focused(&self, thread: ThreadId) -> bool {
        self.composer_focused == Some(thread)
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
    /// Returns whether the answer changed: it decides both the key context and the harness
    /// snapshot's `mode`, so a change has to reach the next frame.
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
        }) || self.derived.live_delegation_callers.contains(&thread)
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

    /// Re-opens every installed projection after a daemon connection is replaced.
    ///
    /// Each projection supplies its own `last_seq`; an older daemon-declared cursor belongs to
    /// the connection that just went away and must not override it.
    pub fn resync_installed(&mut self) {
        let installed: Vec<ThreadId> = self.mirror.projections.keys().copied().collect();
        for thread in installed {
            self.resync.insert(thread);
            self.resume_from.remove(&thread);
        }
    }

    /// Records a daemon-declared drop of this connection's tail for one thread.
    ///
    /// Backpressure is never a stall, never an OOM, and never a silent drop: the daemon names
    /// the cursor it is sure the client received, so the re-open asks from exactly there rather
    /// than from whatever the local projection happens to hold.
    pub fn mark_resync_from(&mut self, thread: ThreadId, from_seq: Seq) {
        self.resync.insert(thread);
        // The local cursor may be ahead of the daemon's belief only if a later event already
        // landed; taking the smaller of the two costs a replay and loses nothing.
        let applied = self
            .mirror
            .projections
            .get(&thread)
            .map_or(from_seq, |projection| projection.last_seq.min(from_seq));
        self.resume_from.insert(thread, applied);
    }

    /// The cursor a re-open should resume from, when the daemon named one.
    ///
    /// Only a thread with an installed projection resumes from anywhere: a cursor names what
    /// the projection already holds, and without one the open asks for the newest window.
    #[must_use]
    pub fn resume_from(&self, thread: ThreadId) -> Option<Seq> {
        let projection = self.projection(thread)?;
        Some(
            self.resume_from
                .get(&thread)
                .copied()
                .unwrap_or(projection.last_seq),
        )
    }

    /// Marks a thread's catch-up complete, which is the only transition into live.
    ///
    /// A mirror never fabricates one: a window opened against a cached mirror paints as cached
    /// until the owner confirms it.
    pub fn synchronize(&mut self, thread: ThreadId) {
        if self.mirror.synchronize(thread) {
            self.resume_from.remove(&thread);
        }
    }

    /// Whether the owner has confirmed the content this client is showing.
    #[must_use]
    pub fn is_synchronized(&self, thread: ThreadId) -> bool {
        self.mirror.is_synchronized(thread)
    }

    /// The cursor an older page of one thread's transcript reads from, when one remains.
    #[must_use]
    pub fn older_cursor(&self, thread: ThreadId) -> Option<String> {
        self.mirror.older_cursor(thread).map(str::to_owned)
    }

    /// Installs a bounded transcript window as the newest content of a thread.
    ///
    /// A window is projection *pieces*, and the client's `last_seq` becomes the sequence the
    /// **content** applied through — not the log head — so the mirror decides continuity from
    /// what is in hand rather than from the newest row the daemon has.
    pub fn install_window(&mut self, window: &AgentThreadWindow) {
        let thread = window.summary.thread;
        let repair_pending = self.resync.contains(&thread);
        if let Some(seen) = window.seen_seq {
            self.mark_seen(thread, seen);
        }
        self.apply_summary(window.summary.clone());
        self.commands
            .insert(thread, window.session.commands.clone());
        self.skills.insert(thread, window.session.skills.clone());
        self.modes.insert(
            thread,
            window
                .session
                .capabilities
                .as_ref()
                .map_or_else(Vec::new, |capabilities| capabilities.modes.clone()),
        );
        self.last_applied.insert(thread, Applied::Structural);
        self.projection_replaced.insert(thread);
        match self.mirror.install_window(window) {
            MirrorOutcome::Gap { .. } => {
                self.resync.insert(thread);
            }
            MirrorOutcome::Applied(_)
            | MirrorOutcome::Duplicate { .. }
            | MirrorOutcome::Rejected { .. } => {
                if !repair_pending {
                    self.resync.remove(&thread);
                }
                self.resume_from.remove(&thread);
            }
        }
    }

    /// Merges an older page behind the window a thread already holds.
    ///
    /// A page is not an event: it can only add history *behind* what the mirror has, so it never
    /// advances a cursor. A page read at a head the client has not applied yet is **parked**,
    /// because merging it anyway replays a streaming turn's deltas on top of page content that
    /// already contains them.
    pub fn merge_older_page(&mut self, window: &AgentThreadWindow) -> PageOutcome {
        self.mirror.merge_older_page(window)
    }

    /// The harness skills `$` completes in one thread.
    #[must_use]
    pub fn skills(&self, thread: ThreadId) -> Vec<String> {
        self.skills.get(&thread).cloned().unwrap_or_default()
    }

    /// The permission modes the live harness declared, in picker order.
    #[must_use]
    pub fn modes(&self, thread: ThreadId) -> Vec<PermissionMode> {
        self.modes.get(&thread).cloned().unwrap_or_default()
    }

    /// The prepared decision currently rendered for one mounted thread.
    #[must_use]
    pub(crate) fn decision(&self, thread: ThreadId) -> Option<&PreparedDecisionObservable> {
        self.decisions.get(&thread)
    }

    /// Mirrors a view's prepared decision and reports whether the snapshot-visible value moved.
    pub(crate) fn set_decision(
        &mut self,
        thread: ThreadId,
        decision: Option<PreparedDecisionObservable>,
    ) -> bool {
        match decision {
            Some(decision) => {
                if self.decisions.get(&thread) == Some(&decision) {
                    return false;
                }
                self.decisions.insert(thread, decision);
                true
            }
            None => self.decisions.remove(&thread).is_some(),
        }
    }

    /// Records whether a thread's transcript has a row focused.
    ///
    /// Returns whether the answer changed: it decides the `AgentRow` key context, so a change
    /// has to reach the next frame.
    pub fn set_row_focus(&mut self, thread: ThreadId, focused: bool) -> bool {
        if focused {
            self.row_focus.insert(thread)
        } else {
            self.row_focus.remove(&thread)
        }
    }

    /// Whether a thread's transcript has a row focused.
    #[must_use]
    pub fn has_row_focus(&self, thread: ThreadId) -> bool {
        self.row_focus.contains(&thread)
    }

    /// Mirrors the focused transcript row's stable harness vocabulary.
    pub(crate) fn set_focused_row(
        &mut self,
        thread: ThreadId,
        focused: Option<&'static str>,
    ) -> bool {
        match focused {
            Some(focused) => self.focused_rows.insert(thread, focused) != Some(focused),
            None => self.focused_rows.remove(&thread).is_some(),
        }
    }

    /// Focused transcript row kind, when scroll mode carries row focus.
    #[must_use]
    pub(crate) fn focused_row(&self, thread: ThreadId) -> Option<&'static str> {
        self.focused_rows.get(&thread).copied()
    }

    /// Mirrors how many delegation-result cards this view has expanded.
    pub(crate) fn set_expanded_result_cards(&mut self, thread: ThreadId, count: u32) -> bool {
        self.expanded_result_cards.insert(thread, count) != Some(count)
    }

    /// Expanded delegation-result cards in one mounted thread view.
    #[must_use]
    pub(crate) fn expanded_result_cards(&self, thread: ThreadId) -> u32 {
        self.expanded_result_cards
            .get(&thread)
            .copied()
            .unwrap_or_default()
    }

    /// Clears the flag while one open request is in flight, so a frame cannot spam the daemon.
    pub fn clear_resync(&mut self, thread: ThreadId) {
        self.resync.remove(&thread);
    }

    /// Replaces a projection with a daemon snapshot and applies its ordered tail.
    pub fn install_snapshot(&mut self, projection: ThreadProjection, events: &[SeqEvent]) {
        let thread = projection.thread;
        // A replayed projection arrives with the vocabulary its `SessionConfigured` declared,
        // and the events replayed under it never reach `apply_event`.
        self.vocabulary_revision = self.vocabulary_revision.wrapping_add(1);
        let repair_pending = self.resync.contains(&thread);
        self.last_applied.insert(thread, Applied::Structural);
        self.projection_replaced.insert(thread);
        match self.mirror.install_snapshot(projection, events) {
            MirrorOutcome::Gap { .. } => {
                self.resync.insert(thread);
            }
            // A replay is already installed, and an event the reducer refused would come back
            // from the daemon unchanged: neither is worth another open request.
            MirrorOutcome::Applied(_)
            | MirrorOutcome::Duplicate { .. }
            | MirrorOutcome::Rejected { .. } => {
                if !repair_pending {
                    self.resync.remove(&thread);
                }
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
        match &event.event {
            fleet_core::agents::AgentEvent::SessionConfigured {
                commands, skills, ..
            } => {
                self.commands.insert(thread, commands.clone());
                self.skills.insert(thread, skills.clone());
                // The one event that carries a harness's model vocabulary, and so the one that
                // can change what the Model and Effort pickers have to offer.
                self.vocabulary_revision = self.vocabulary_revision.wrapping_add(1);
            }
            fleet_core::agents::AgentEvent::MetadataChanged {
                skills: Some(skills),
                ..
            } => {
                self.skills.insert(thread, skills.clone());
            }
            _ => {}
        }
        if !self.mirror.projections.contains_key(&thread) {
            // A thread this client has not opened has no projection to advance; its tab still
            // updates from the summary the daemon broadcasts beside the event. Remember the
            // missing tail in case an open reply was already in flight when this event landed.
            self.resync.insert(thread);
            return MirrorOutcome::Applied(Applied::Structural);
        }
        let outcome = self.mirror.apply_event(thread, event);
        if let MirrorOutcome::Applied(applied) = &outcome {
            self.last_applied.insert(thread, applied.clone());
        }
        if matches!(outcome, MirrorOutcome::Gap { .. }) {
            self.resync.insert(thread);
        }
        outcome
    }

    /// Patches one broadcast summary into the mirror, preserving snapshot order.
    pub fn apply_summary(&mut self, summary: AgentThreadSummary) {
        let changed = match self
            .summaries
            .iter_mut()
            .find(|existing| existing.thread == summary.thread)
        {
            Some(existing) if *existing == summary => false,
            Some(existing) => {
                *existing = summary;
                true
            }
            None => {
                self.unconfirmed.insert(summary.thread);
                self.summaries.push(summary);
                true
            }
        };
        if changed {
            self.summaries_revision = self.summaries_revision.wrapping_add(1);
            self.prepare_attention_counts();
            self.prepare_strip_offsets();
        }
    }

    /// Records the sequence the user has actually seen in a focused tab.
    pub fn mark_seen(&mut self, thread: ThreadId, seq: Seq) {
        if self.seen(thread) != seq {
            self.mirror.mark_seen(thread, seq);
            self.seen_revision = self.seen_revision.wrapping_add(1);
            self.prepare_attention_counts();
        }
    }

    /// Seeds persisted cursors returned for this installation after Hello.
    pub fn seed_seen(&mut self, cursors: &[(ThreadId, Seq)]) {
        for (thread, seq) in cursors {
            self.mark_seen(*thread, *seq);
        }
    }

    /// Threads the daemon is showing as unread that this window has already read.
    ///
    /// §3.3 clears `NeedsYou(Finished)` when the user views the tab. The process-local override
    /// can be newer than the persisted census fetched during Hello, so it is re-reported for
    /// every thread rather than only for the tab that happens to be visible.
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

    /// Makes the next snapshot authoritative about every thread, as a fresh link's first one is.
    ///
    /// Nothing from the previous link can be newer than the snapshot a new link opens with.
    pub fn forget_unconfirmed(&mut self) {
        self.unconfirmed.clear();
    }

    /// Records current attentions without presenting them, the way a fresh link is adopted.
    ///
    /// A thread that was already blocked before this window connected is not news, so the first
    /// snapshot of a connection seeds the edge detector instead of firing every state at once.
    pub fn seed(&mut self, threads: &[AgentThreadSummary]) {
        self.notified = threads
            .iter()
            .filter(|summary| summary.parent.is_none())
            .map(|summary| (summary.thread, self.attention_in(summary.thread, threads)))
            .collect();
    }

    /// Replaces the summary list from an authoritative snapshot and forgets vanished threads.
    ///
    /// A snapshot is authoritative about every thread it has *ever* listed, but not about one a
    /// summary introduced since. The daemon assembles a snapshot asynchronously and creating a
    /// thread does not stamp it, so a snapshot assembled a moment before a thread existed can be
    /// applied after that thread's `AgentSummary` and its create reply. Its silence is not a
    /// removal: forgetting the thread there would drop the tab `^s a` had just selected, and the
    /// next summary would put it back unselected. Such a thread is kept, with its latest summary,
    /// until a snapshot lists it — or until a snapshot no longer lists its worktree, which is
    /// the one way a thread no snapshot ever listed can really be gone.
    pub fn sync_snapshot(
        &mut self,
        mut threads: Vec<AgentThreadSummary>,
        worktrees: &HashSet<&WorktreeId>,
    ) {
        let mut live: HashSet<ThreadId> = threads.iter().map(|summary| summary.thread).collect();
        self.unconfirmed.retain(|thread| !live.contains(thread));
        for summary in &self.summaries {
            if self.unconfirmed.contains(&summary.thread) && worktrees.contains(&summary.worktree) {
                live.insert(summary.thread);
                threads.push(summary.clone());
            }
        }
        self.unconfirmed.retain(|thread| live.contains(thread));
        if self.summaries != threads {
            self.summaries = threads;
            self.summaries_revision = self.summaries_revision.wrapping_add(1);
        }
        self.mirror
            .projections
            .retain(|thread, _| live.contains(thread));
        self.mirror
            .last_seen
            .retain(|thread, _| live.contains(thread));
        self.resync.retain(|thread| live.contains(thread));
        self.notified.retain(|thread, _| live.contains(thread));
        self.commands.retain(|thread, _| live.contains(thread));
        self.skills.retain(|thread, _| live.contains(thread));
        self.modes.retain(|thread, _| live.contains(thread));
        self.decisions.retain(|thread, _| live.contains(thread));
        self.resume_from.retain(|thread, _| live.contains(thread));
        self.last_applied.retain(|thread, _| live.contains(thread));
        self.projection_replaced
            .retain(|thread| live.contains(thread));
        self.row_focus.retain(|thread| live.contains(thread));
        self.focused_rows.retain(|thread, _| live.contains(thread));
        self.expanded_result_cards
            .retain(|thread, _| live.contains(thread));
        self.composing.retain(|thread| live.contains(thread));
        self.scrolling.retain(|thread| live.contains(thread));
        self.question_cursor
            .retain(|thread, _| live.contains(thread));
        self.reported.retain(|thread, _| live.contains(thread));
        self.active.retain(|_, thread| live.contains(thread));
        let attached_before = self.attached.len();
        // Attached children are snapshot-local; closed callers persist across incomplete snapshots.
        self.attached.retain(|thread| live.contains(thread));
        if self.attached.len() != attached_before {
            self.bump_attached_revision();
        }
        if self
            .focus_composer
            .is_some_and(|thread| !live.contains(&thread))
        {
            self.focus_composer = None;
        }
        if self
            .composer_focused
            .is_some_and(|thread| !live.contains(&thread))
        {
            self.composer_focused = None;
        }
        self.prepare_attention_counts();
        self.prepare_strip_offsets();
    }

    /// Replaces the installation's daemon-persisted closed set.
    pub fn seed_closed(&mut self, threads: Vec<ThreadId>) {
        let closed = threads.into_iter().collect();
        if self.closed != closed {
            self.closed = closed;
            self.bump_attached_revision();
        }
    }

    /// The threads that just entered an attention worth a notification, and their tab labels.
    ///
    /// Only the two states the design treats as signals — `needs you` and `failed` — fire, and
    /// each fires once per entry, so a thread that stays blocked does not notify on every event.
    pub fn attention_edges(&mut self) -> Vec<(String, Attention)> {
        let mut edges = Vec::new();
        for summary in self
            .summaries
            .iter()
            .filter(|summary| summary.parent.is_none())
        {
            let attention = self.attention(summary.thread);
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

    /// The `Agent > …` key contexts an active agent tab owns (§12).
    ///
    /// Every sub-context is derived from **daemon state** rather than from the view, which is
    /// what makes a decision own the keyboard in the same frame the gate appears rather than one
    /// frame later. Precedence, and each step is a rule: a frozen tail beats everything,
    /// including an open gate; then the newest open decision; then work; then idle.
    #[must_use]
    pub fn agent_context_chain(&self) -> Option<Vec<&'static str>> {
        let thread = self.active_agent_thread()?;
        // §12: `^s [` is a real mode. While the tail is frozen the transcript owns `j`/`k`,
        // half/page and `gg`/`G`, and row focus lives inside it — which is what finally makes
        // `⏎`/`u`/`o`/`y`/`d` fire instead of being bound, handled and never entered.
        if self.agents.is_scrolling(thread) {
            let mut chain = vec!["Agent", "AgentNativeScroll"];
            if self.agents.has_row_focus(thread) {
                chain.push("AgentRow");
            }
            return Some(chain);
        }
        let projection = self.agents.projection(thread);
        let gate = projection.and_then(|projection| projection.gates.last());
        // §12 routes the bare letters to the decision — but a payload being corrected after
        // `[e]`, a refinement written for `[n]` and a question's free-text answer are all text.
        // While one of those is in progress the composer keeps its own keys, or the draft could
        // not start with a `y`.
        if let Some(gate) = gate
            && !self.agents.is_composing(thread)
        {
            return Some(vec!["Agent", "AgentDecision", decision_context(gate)]);
        }
        // A plan the harness produced as an *item* has no gate, and its verbs still live on the
        // composer, so it owns `AgentPlan` exactly as a plan gate would.
        if !self.agents.is_composing(thread)
            && projection.is_some_and(|projection| {
                crate::screens::agent_thread::decisions::plan_item(projection).is_some()
            })
        {
            return Some(vec![
                "Agent",
                "AgentDecision",
                crate::screens::agent_thread::decisions::PLAN_CONTEXT,
            ]);
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

    /// Whether §12's composer — a live `TextInput` — owns the keyboard on an agent tab.
    ///
    /// `AgentThreadView::focus_composer` gives the tab's keyboard to the composer in every mode
    /// but a frozen tail and an unanswered gate, which is exactly when
    /// [`Self::agent_context_chain`] publishes `AgentIdle` or `AgentWorking`. Reading it back
    /// off the chain keeps one answer rather than two that can drift apart.
    #[must_use]
    pub fn agent_composer_owns_keys(&self) -> bool {
        self.agent_context_chain()
            .is_some_and(|chain| matches!(chain.last(), Some(&"AgentIdle" | &"AgentWorking")))
    }

    /// Applies one native-agent event to the mirror and returns its continuity and damage.
    pub fn apply_agent_event(&mut self, thread: ThreadId, event: &SeqEvent) -> MirrorOutcome {
        let outcome = self.agents.apply_event(thread, event);
        if matches!(outcome, MirrorOutcome::Gap { .. }) {
            self.agents.mark_resync(thread);
        }
        outcome
    }

    /// Applies one broadcast summary and presents whatever attention edge it created.
    pub fn apply_agent_summary(&mut self, summary: AgentThreadSummary, now: Instant) {
        self.agents.apply_summary(summary);
        self.adopt_created_threads();
        self.notify_agent_attention(now);
    }

    /// Presents every fresh `needs you` / `failed` edge through the activity notification path.
    pub(crate) fn notify_agent_attention(&mut self, now: Instant) {
        for (label, attention) in self.agents.attention_edges() {
            self.notify_agent_thread(&label, attention, now);
        }
    }
}

#[cfg(test)]
mod persisted_cursor_tests {
    use super::*;
    use fleet_core::agents::AgentKind;

    #[test]
    fn reconnect_seeds_only_the_originating_installations_cursor() {
        let thread = ThreadId::new();
        let worktree = "buk/payroll#feat"
            .parse()
            .unwrap_or_else(|error| panic!("test worktree must parse: {error}"));
        let mut projection = ThreadProjection::new(thread, worktree, AgentKind::Claude);
        projection.last_seq = Seq(9);
        projection.last_completed_seq = Some(Seq(9));
        let summary = projection.summary(Seq::default());

        let mut reconnected = AgentThreads::default();
        reconnected.apply_summary(summary.clone());
        reconnected.seed_seen(&[(thread, Seq(9))]);

        let mut other_installation = AgentThreads::default();
        other_installation.apply_summary(summary);

        assert_eq!(reconnected.attention(thread), Attention::Idle);
        assert_eq!(
            other_installation.attention(thread),
            Attention::NeedsYou(AttentionKind::Finished)
        );
    }
}

mod pending;

#[cfg(test)]
mod tests;
