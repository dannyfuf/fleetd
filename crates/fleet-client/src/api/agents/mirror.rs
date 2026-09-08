//! Ordered native-agent projection mirror with explicit sequence-gap recovery.

use std::collections::HashMap;

use fleet_core::agents::{Seq, SeqEvent, ThreadId, ThreadProjection};

use super::{AgentSnapshot, Result};

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

/// Client-side materialized projections and per-thread last-seen cursors.
#[derive(Debug, Default)]
pub struct AgentMirror {
    /// Installed thread projections.
    pub projections: HashMap<ThreadId, ThreadProjection>,
    /// Latest event the user has viewed for each thread.
    pub last_seen: HashMap<ThreadId, Seq>,
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
        agents::{AgentEvent, AgentKind, Seq, SeqEvent, ThreadId, ThreadProjection},
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

        // A turn that was never started cannot complete: the sequence is continuous, so this is
        // a reducer rejection and not a discontinuity a snapshot could close.
        let refused = SeqEvent {
            seq: Seq(1),
            at: "2026-09-07T12:00:00Z".parse().expect("timestamp"),
            raw: None,
            event: AgentEvent::TurnCompleted {
                turn: fleet_core::agents::TurnId::new(),
                outcome: fleet_core::agents::TurnOutcome::Completed,
                usage: fleet_core::agents::Usage::default(),
                duration_ms: 0,
                files_changed: Vec::new(),
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

    fn event(seq: Seq, message: &str) -> SeqEvent {
        SeqEvent {
            seq,
            at: "2026-09-07T12:00:00Z".parse().expect("timestamp"),
            raw: None,
            event: AgentEvent::Notice(message.to_owned()),
        }
    }
}
