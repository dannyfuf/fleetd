//! Ordered native-agent projection mirror with explicit sequence-gap recovery.
//!
//! The mirror is a read-through cache and never a replica: it is never consulted for a write,
//! never merged field by field, and every disagreement resolves in favour of the daemon. What it
//! owns is *continuity* — the four-way answer to "did this event land?" that lets a client tell a
//! replay from a hole from a refusal, and repair only the one of the three that a round trip can
//! repair.

use std::collections::HashMap;

use fleet_core::agents::{Seq, SeqEvent, ThreadId, ThreadProjection};
use fleet_proto::agents::{AgentThreadWindow, TranscriptPage};

use super::{AgentSnapshot, Result, window::projection_from_window};

/// Result of applying a sequenced native-agent event to the local mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorOutcome {
    /// The event was applied to an installed projection.
    Applied,
    /// The event is already in the projection; the mirror is ahead of it, not behind.
    ///
    /// A cursored open reads the log outside the daemon's state lock, so its tail and the live
    /// broadcast overlap by construction (§4.3: duplicate settlements are idempotent). A replay
    /// is not a discontinuity, and re-opening the thread over it would only fetch the same
    /// overlap again.
    Duplicate {
        /// Sequence the projection has already applied through.
        applied: Seq,
    },
    /// The projection is missing or the per-thread sequence is discontinuous.
    Gap {
        /// Required next sequence.
        expected: Seq,
        /// Received sequence.
        got: Seq,
    },
    /// The sequence was continuous but the projection refused the event on its own terms.
    ///
    /// A resync cannot repair this — the daemon would hand back the very same event — so it is
    /// reported apart from [`MirrorOutcome::Gap`], which a resync is expected to close.
    Rejected {
        /// Sequence the projection refused.
        seq: Seq,
    },
}

/// What merging an older transcript page did.
///
/// A page is not an event: it can only ever add history *behind* what the mirror already holds,
/// so it never advances a cursor and never decides continuity. The three outcomes are the three
/// things that can be true of one, and none of them is a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageOutcome {
    /// The page was merged behind the content the mirror already had.
    Merged {
        /// Turns the page contributed that the mirror did not already hold.
        turns: usize,
        /// Items the page contributed that the mirror did not already hold.
        items: usize,
    },
    /// The page was read at a head the mirror has not applied yet, so it is parked.
    ///
    /// Merging it anyway is the duplicate-delta bug: a turn streaming *outside* the window can
    /// have its deltas replayed on top of page content that already contains them, and the
    /// transcript grows a doubled message nobody can explain. The caller applies live events up
    /// to `thread_seq` and offers the page again.
    Parked {
        /// Thread head the page was read at.
        thread_seq: Seq,
        /// Sequence the mirror has applied.
        applied: Seq,
    },
    /// The mirror holds no projection for this thread; a page cannot precede its window.
    Unknown,
}

/// Where a thread's window ends and whether older history remains.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowState {
    /// Page metadata from the most recent window response.
    pub page: Option<TranscriptPage>,
    /// Log head the window was read at.
    pub head_seq: Seq,
    /// Whether the owner has confirmed this content.
    ///
    /// A mirrored thread opens `false` and paints as cached. Only
    /// [`Event::AgentSynchronized`](fleet_proto::event::Event::AgentSynchronized) turns it true —
    /// the mirror never fabricates one, which is what keeps "live" from meaning "probably live".
    pub synchronized: bool,
}

/// Client-side materialized projections and per-thread last-seen cursors.
#[derive(Debug, Default)]
pub struct AgentMirror {
    /// Installed thread projections.
    pub projections: HashMap<ThreadId, ThreadProjection>,
    /// Latest event the user has viewed for each thread.
    pub last_seen: HashMap<ThreadId, Seq>,
    /// Window and synchronization state per thread, for threads opened with a window.
    pub windows: HashMap<ThreadId, WindowState>,
}

impl AgentMirror {
    /// Applies one ordered event, reporting a gap without mutating on discontinuity.
    pub fn apply_event(&mut self, thread: ThreadId, event: &SeqEvent) -> MirrorOutcome {
        let Some(projection) = self.projections.get_mut(&thread) else {
            return MirrorOutcome::Gap {
                expected: Seq(1),
                got: event.seq,
            };
        };
        if event.seq <= projection.last_seq {
            return MirrorOutcome::Duplicate {
                applied: projection.last_seq,
            };
        }
        let expected = projection.last_seq.next();
        if event.seq != expected {
            return MirrorOutcome::Gap {
                expected,
                got: event.seq,
            };
        }
        if projection.apply(event).is_err() {
            return MirrorOutcome::Rejected { seq: event.seq };
        }
        MirrorOutcome::Applied
    }

    /// Replaces a projection and applies its ordered event tail.
    pub fn install_snapshot(
        &mut self,
        projection: ThreadProjection,
        events_after: &[SeqEvent],
    ) -> MirrorOutcome {
        let thread = projection.thread;
        self.projections.insert(thread, projection);
        for event in events_after {
            match self.apply_event(thread, event) {
                MirrorOutcome::Applied | MirrorOutcome::Duplicate { .. } => {}
                outcome => return outcome,
            }
        }
        MirrorOutcome::Applied
    }

    /// Replaces a projection from a bounded window and applies its live tail.
    ///
    /// The outcome is the tail's, not the window's: a window always installs. A
    /// [`MirrorOutcome::Gap`] here means the daemon is mid-rebuild — its window applied fewer
    /// events than its tail assumes — and the caller repairs it by resuming from the projection's
    /// own cursor, exactly as it repairs a gap in live delivery.
    pub fn install_window(&mut self, window: &AgentThreadWindow) -> MirrorOutcome {
        let thread = window.summary.thread;
        self.windows.insert(
            thread,
            WindowState {
                page: window.page.clone(),
                head_seq: window.head_seq,
                synchronized: window.synchronized,
            },
        );
        self.install_snapshot(projection_from_window(window), &window.events_after)
    }

    /// Merges an older page of history behind what the mirror already holds.
    ///
    /// Never advances `last_seq`, never replaces the open gates — a live
    /// [`GateWithdrawn`](fleet_core::agents::AgentEvent::GateWithdrawn) the client has already
    /// applied must not be undone by a page read before it — and never re-adds a turn or item
    /// the projection already carries.
    pub fn merge_older_page(&mut self, window: &AgentThreadWindow) -> PageOutcome {
        let thread = window.summary.thread;
        let Some(projection) = self.projections.get_mut(&thread) else {
            return PageOutcome::Unknown;
        };
        if let Some(page) = &window.page
            && page.thread_seq > projection.last_seq
        {
            return PageOutcome::Parked {
                thread_seq: page.thread_seq,
                applied: projection.last_seq,
            };
        }

        let known_turns = projection
            .turns
            .iter()
            .map(|turn| turn.id)
            .collect::<std::collections::HashSet<_>>();
        let known_items = projection
            .items
            .iter()
            .map(|item| item.id)
            .collect::<std::collections::HashSet<_>>();
        let turns = window
            .window
            .turns
            .iter()
            .filter(|turn| !known_turns.contains(&turn.id))
            .cloned()
            .collect::<Vec<_>>();
        let items = window
            .window
            .items
            .iter()
            .filter(|item| !known_items.contains(&item.id))
            .cloned()
            .collect::<Vec<_>>();
        let merged = PageOutcome::Merged {
            turns: turns.len(),
            items: items.len(),
        };

        // Prepend, so display order stays chronological and the indexes the projection rebuilds
        // stay consistent with the vectors they describe.
        projection.turns.splice(0..0, turns);
        projection.items.splice(0..0, items);
        let checkpoints = window.window.checkpoints.clone();
        let notices = window.window.notices.clone();
        projection.checkpoints.splice(0..0, checkpoints);
        projection.notices.splice(0..0, notices);

        if let Some(state) = self.windows.get_mut(&thread) {
            state.page = window.page.clone();
        }
        merged
    }

    /// Marks a thread live after the daemon said catch-up is complete.
    ///
    /// A marker counts for any thread this mirror holds, including one opened through the legacy
    /// unbounded snapshot: the marker is a fact about the *stream*, not about how the transcript
    /// was read. Returns whether the thread was known — a marker for a thread this client never
    /// opened is a stale broadcast, not an error.
    pub fn synchronize(&mut self, thread: ThreadId) -> bool {
        if !self.projections.contains_key(&thread) && !self.windows.contains_key(&thread) {
            return false;
        }
        self.windows.entry(thread).or_default().synchronized = true;
        true
    }

    /// Whether this thread's content has been confirmed by its owner.
    #[must_use]
    pub fn is_synchronized(&self, thread: ThreadId) -> bool {
        self.windows
            .get(&thread)
            .is_some_and(|state| state.synchronized)
    }

    /// The cursor for the next older page of a thread, when one exists.
    #[must_use]
    pub fn older_cursor(&self, thread: ThreadId) -> Option<&str> {
        self.windows
            .get(&thread)
            .and_then(|state| state.page.as_ref())
            .filter(|page| page.has_more)
            .and_then(|page| page.before_cursor.as_deref())
    }

    /// The sequence this mirror has applied for a thread, or zero when it holds none.
    #[must_use]
    pub fn applied_seq(&self, thread: ThreadId) -> Seq {
        self.projections
            .get(&thread)
            .map_or(Seq::default(), |projection| projection.last_seq)
    }

    /// Records the latest sequence visible to the user for a thread.
    pub fn mark_seen(&mut self, thread: ThreadId, seq: Seq) {
        self.last_seen.insert(thread, seq);
    }

    /// Applies an event and fetches an ordered snapshot tail when delivery has a gap.
    pub async fn apply_or_resync<F, Fut>(
        &mut self,
        thread: ThreadId,
        event: &SeqEvent,
        resync: F,
    ) -> Result<MirrorOutcome>
    where
        F: FnOnce(ThreadId, Option<Seq>) -> Fut,
        Fut: std::future::Future<Output = Result<AgentSnapshot>>,
    {
        let outcome = self.apply_event(thread, event);
        // Only a gap is worth a round-trip: a replay is already in the projection, and an event
        // the reducer refused would come back from the daemon unchanged.
        if !matches!(outcome, MirrorOutcome::Gap { .. }) {
            return Ok(outcome);
        }
        // A resync is a client repairing a gap, never a user opening a tab. The daemon reads a
        // `None` cursor as the §6 lazy resume and would relaunch a provider process nobody asked
        // for, so a mirror with no projection asks for the transcript from the beginning.
        let last_applied = self
            .projections
            .get(&thread)
            .map_or(Seq(0), |projection| projection.last_seq);
        let snapshot = resync(thread, Some(last_applied)).await?;
        Ok(self.install_snapshot(snapshot.projection, &snapshot.events_after))
    }
}

#[cfg(test)]
mod tests {
    use fleet_core::{
        agents::{AgentEvent, AgentKind, ItemId, Seq, SeqEvent, ThreadId, ThreadProjection},
        ids::WorktreeId,
    };

    use super::*;

    #[tokio::test]
    async fn a_gap_resyncs_from_the_last_applied_sequence() {
        let thread = ThreadId::new();
        let worktree = WorktreeId::try_from("acme/api#feature").expect("worktree");
        let projection = ThreadProjection::new(thread, worktree.clone(), AgentKind::Claude);
        let first = event(Seq(1), "first");
        let second = event(Seq(2), "second");
        let third = event(Seq(3), "third");
        let third_tail = third.clone();
        let mut mirror = AgentMirror::default();
        mirror.install_snapshot(projection, std::slice::from_ref(&first));

        let outcome = mirror
            .apply_or_resync(thread, &third, |requested, from_seq| async move {
                assert_eq!(requested, thread);
                assert_eq!(from_seq, Some(Seq(1)));
                let mut base = ThreadProjection::new(thread, worktree.clone(), AgentKind::Claude);
                base.apply(&first).expect("first event");
                Ok(AgentSnapshot {
                    projection: base,
                    events_after: vec![second, third_tail],
                })
            })
            .await
            .expect("resync");

        assert_eq!(outcome, MirrorOutcome::Applied);
        assert_eq!(mirror.projections[&thread].last_seq, Seq(3));
    }

    #[tokio::test]
    async fn a_gap_the_resync_did_not_repair_is_reported_not_swallowed() {
        let thread = ThreadId::new();
        let worktree = WorktreeId::try_from("acme/api#feature").expect("worktree");
        let projection = ThreadProjection::new(thread, worktree.clone(), AgentKind::Claude);
        let mut mirror = AgentMirror::default();
        mirror.install_snapshot(projection, &[]);

        // The daemon answers with a snapshot that is still behind the event being applied.
        let outcome = mirror
            .apply_or_resync(thread, &event(Seq(4), "fourth"), |_, from_seq| async move {
                assert_eq!(
                    from_seq,
                    Some(Seq(0)),
                    "a catch-up never resumes a provider"
                );
                Ok(AgentSnapshot {
                    projection: ThreadProjection::new(thread, worktree.clone(), AgentKind::Claude),
                    events_after: vec![event(Seq(2), "second")],
                })
            })
            .await
            .expect("resync completes");
        assert_eq!(
            outcome,
            MirrorOutcome::Gap {
                expected: Seq(1),
                got: Seq(2)
            }
        );
    }

    #[tokio::test]
    async fn an_event_the_mirror_already_applied_is_a_duplicate_not_a_gap() {
        let thread = ThreadId::new();
        let worktree = WorktreeId::try_from("acme/api#feature").expect("worktree");
        let projection = ThreadProjection::new(thread, worktree, AgentKind::Claude);
        let first = event(Seq(1), "first");
        let mut mirror = AgentMirror::default();
        mirror.install_snapshot(projection, std::slice::from_ref(&first));

        // The overlap a cursored open and the live broadcast share must cost neither a resync
        // request nor a fatal out-of-sequence error.
        let outcome = mirror
            .apply_or_resync(thread, &first, |_, _| async {
                panic!("a replayed event must never trigger a resync");
            })
            .await
            .expect("duplicate");

        assert_eq!(outcome, MirrorOutcome::Duplicate { applied: Seq(1) });
        assert_eq!(mirror.projections[&thread].last_seq, Seq(1));
    }

    #[tokio::test]
    async fn an_event_the_projection_refuses_is_reported_as_a_rejection() {
        let thread = ThreadId::new();
        let worktree = WorktreeId::try_from("acme/api#feature").expect("worktree");
        let projection = ThreadProjection::new(thread, worktree, AgentKind::Claude);
        let mut mirror = AgentMirror::default();
        mirror.install_snapshot(projection, &[]);

        // A turn nobody saw start cannot be aborted: NATIVE-AGENTS.md §3.3 accepts a settlement
        // for an unknown turn because it recovers a lost start, and refuses an abort because a
        // delayed stop must not clobber a newer pending start. The sequence is continuous, so
        // this is a reducer rejection and not a discontinuity a snapshot could close.
        let refused = SeqEvent {
            seq: Seq(1),
            at: "2026-09-07T12:00:00Z".parse().expect("timestamp"),
            raw: None,
            event: AgentEvent::TurnAborted {
                turn: fleet_core::agents::TurnId::new(),
                reason: fleet_core::agents::AbortReason::User,
            },
        };
        let outcome = mirror
            .apply_or_resync(thread, &refused, |_, _| async {
                panic!("a rejection must never trigger a resync");
            })
            .await
            .expect("rejection");

        assert_eq!(outcome, MirrorOutcome::Rejected { seq: Seq(1) });
        assert_eq!(mirror.projections[&thread].last_seq, Seq(0));
    }

    #[tokio::test]
    async fn a_window_installs_the_sequence_its_content_applied_not_the_log_head() {
        // A daemon mid-rebuild answers a window that is a prefix: `head_seq` is ahead of what the
        // projection could apply. Trusting the head would skip every event in between, and they
        // would never be sent and never replayed.
        let thread = ThreadId::new();
        let window = window_response(thread, Seq(12), Seq(9), true);

        let mut mirror = AgentMirror::default();
        let outcome = mirror.install_window(&window);

        assert_eq!(outcome, MirrorOutcome::Applied);
        assert_eq!(mirror.applied_seq(thread), Seq(9));
        assert!(mirror.is_synchronized(thread));
        assert_eq!(
            mirror.older_cursor(thread),
            Some("fat.1.cursor.4"),
            "the page cursor is what makes `load earlier` possible at all"
        );
    }

    #[tokio::test]
    async fn a_mirrored_window_stays_cached_until_its_owner_confirms_it() {
        let thread = ThreadId::new();
        let window = window_response(thread, Seq(4), Seq(4), false);
        let mut mirror = AgentMirror::default();
        mirror.install_window(&window);

        assert!(
            !mirror.is_synchronized(thread),
            "a mirror never fabricates a synchronization"
        );
        assert!(mirror.synchronize(thread));
        assert!(mirror.is_synchronized(thread));
        // A marker for a thread this client never opened is a stale broadcast, not an error.
        assert!(!mirror.synchronize(ThreadId::new()));
    }

    #[tokio::test]
    async fn an_older_page_merges_behind_the_window_without_advancing_the_cursor() {
        let thread = ThreadId::new();
        let mut mirror = AgentMirror::default();
        mirror.install_window(&window_response(thread, Seq(9), Seq(9), true));
        let applied = mirror.applied_seq(thread);

        let mut older = window_response(thread, Seq(9), Seq(9), true);
        older.window.items = vec![item(ItemId::new()), item(ItemId::new())];
        older.window.turns = Vec::new();
        older.page = Some(TranscriptPage {
            before_cursor: None,
            has_more: false,
            thread_seq: Seq(9),
        });

        let outcome = mirror.merge_older_page(&older);

        assert_eq!(outcome, PageOutcome::Merged { turns: 0, items: 2 });
        assert_eq!(
            mirror.applied_seq(thread),
            applied,
            "a page is not an event"
        );
        assert_eq!(mirror.projections[&thread].items.len(), 3);
        assert_eq!(
            mirror.older_cursor(thread),
            None,
            "a fully loaded thread stops offering a cursor"
        );
    }

    #[tokio::test]
    async fn a_page_read_ahead_of_the_mirror_is_parked_rather_than_merged() {
        // Merging it would replay a streaming turn's deltas on top of page content that already
        // contains them, and the transcript grows a doubled message nobody can explain.
        let thread = ThreadId::new();
        let mut mirror = AgentMirror::default();
        mirror.install_window(&window_response(thread, Seq(4), Seq(4), true));

        let mut ahead = window_response(thread, Seq(20), Seq(20), true);
        ahead.page = Some(TranscriptPage {
            before_cursor: Some("fat.1.cursor.1".to_owned()),
            has_more: true,
            thread_seq: Seq(20),
        });

        assert_eq!(
            mirror.merge_older_page(&ahead),
            PageOutcome::Parked {
                thread_seq: Seq(20),
                applied: Seq(4)
            }
        );
        assert_eq!(mirror.projections[&thread].items.len(), 1);
        assert_eq!(
            mirror.older_cursor(thread),
            Some("fat.1.cursor.4"),
            "a parked page leaves the window's own cursor in place"
        );
    }

    #[tokio::test]
    async fn a_page_for_a_thread_with_no_window_is_reported_rather_than_installed() {
        let thread = ThreadId::new();
        let mut mirror = AgentMirror::default();

        assert_eq!(
            mirror.merge_older_page(&window_response(thread, Seq(4), Seq(4), true)),
            PageOutcome::Unknown
        );
        assert!(mirror.projections.is_empty());
    }

    #[tokio::test]
    async fn a_page_never_re_adds_an_item_the_projection_already_holds() {
        let thread = ThreadId::new();
        let mut mirror = AgentMirror::default();
        let window = window_response(thread, Seq(9), Seq(9), true);
        mirror.install_window(&window);

        // The same page served twice — a retried "load earlier" — must not double the transcript.
        assert_eq!(
            mirror.merge_older_page(&window),
            PageOutcome::Merged { turns: 0, items: 0 }
        );
        assert_eq!(mirror.projections[&thread].items.len(), 1);
    }

    /// One window response with a single item, a page cursor, and explicit watermarks.
    fn window_response(
        thread: ThreadId,
        head_seq: Seq,
        projected_seq: Seq,
        synchronized: bool,
    ) -> AgentThreadWindow {
        use fleet_core::{
            agents::{AgentKind, Attention, SessionState, TurnState},
            ids::WorktreeId,
        };

        AgentThreadWindow {
            summary: fleet_core::agents::AgentThreadSummary {
                thread,
                worktree: WorktreeId::try_from("acme/api#feature").expect("worktree"),
                host: None,
                provider: AgentKind::Claude,
                title: "Claude".to_owned(),
                attention: Attention::Idle,
                session: SessionState::Ready,
                turn: TurnState::None,
                last_seq: head_seq,
                last_activity: None,
                last_completed_seq: None,
                last_nonterminal_seq: None,
                exit_code: None,
            },
            session: fleet_proto::agents::AgentSessionView::default(),
            window: fleet_proto::agents::TranscriptWindow {
                items: vec![item(
                    "bbbbbbbb-2222-4333-8444-555555555555"
                        .parse()
                        .expect("item id"),
                )],
                ..fleet_proto::agents::TranscriptWindow::default()
            },
            page: Some(TranscriptPage {
                before_cursor: Some("fat.1.cursor.4".to_owned()),
                has_more: true,
                thread_seq: head_seq,
            }),
            head_seq,
            projected_seq,
            events_after: Vec::new(),
            synchronized,
        }
    }

    fn item(id: fleet_core::agents::ItemId) -> fleet_core::agents::Item {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "turn": fleet_core::agents::TurnId::new(),
            "kind": {"type": "assistant_text", "data": {"text": "hello"}},
            "status": "completed",
            "started": "2026-09-07T12:00:00Z",
        }))
        .unwrap_or_else(|error| panic!("{error}"))
    }

    fn event(seq: Seq, message: &str) -> SeqEvent {
        SeqEvent {
            seq,
            at: "2026-09-07T12:00:00Z".parse().expect("timestamp"),
            raw: None,
            event: AgentEvent::Notice(message.to_owned()),
        }
    }
}
