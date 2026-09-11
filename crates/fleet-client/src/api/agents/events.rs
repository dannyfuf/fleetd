//! The ordered agent-event subscription: gap repair, budget resync, and the sync marker.
//!
//! Every repair this loop makes is invisible when it works, which is why each one has a name on
//! the wire rather than a heuristic here:
//!
//! - a **gap** is a discontinuity in one thread's sequence, and the only one of the four a round
//!   trip can repair;
//! - a **resync** is the daemon saying it dropped events for this connection on purpose, with the
//!   cursor to resume from — never a stall, never an OOM, never a silent drop;
//! - a **synchronization marker** is the only transition into live, so a mirrored thread stops
//!   reading as cached exactly when its owner has confirmed it and not a moment earlier;
//! - a **window notice** says the local daemon refilled a mirrored thread from its owner, and the
//!   window in hand is now behind.

use fleet_core::agents::{Seq, SeqEvent, ThreadId};
use fleet_proto::{
    AGENT_WINDOW_CAPABILITY,
    error::{ErrorKind, ProtoError},
    event::Event,
};
use tokio::sync::broadcast::{self, error::RecvError};

use super::{
    AgentMirror, AgentSnapshot, AgentWindowRequest, MirrorOutcome, Result, out_of_sequence,
};
use crate::Client;

/// Ordered native-agent event subscription with automatic per-thread gap recovery.
pub struct AgentEvents {
    client: Client,
    receiver: broadcast::Receiver<Event>,
    mirror: AgentMirror,
}

impl AgentEvents {
    /// Subscribes to the daemon's agent events with an empty mirror.
    pub(crate) fn new(client: Client) -> Self {
        Self {
            receiver: client.events(),
            client,
            mirror: AgentMirror::default(),
        }
    }

    /// Returns the live projection mirror maintained by this subscription.
    #[must_use]
    pub fn mirror(&self) -> &AgentMirror {
        &self.mirror
    }

    /// Installs an initial thread snapshot before consuming live events.
    pub fn install(&mut self, snapshot: AgentSnapshot) -> MirrorOutcome {
        self.mirror
            .install_snapshot(snapshot.projection, &snapshot.events_after)
    }

    /// Waits for the next thread event, repairing gaps from the last applied sequence.
    ///
    /// The three stream-control events are handled here and never returned: they carry no
    /// transcript content, so handing them to a caller that expects a [`SeqEvent`] would make
    /// every caller re-implement the repair this loop already did.
    pub async fn recv(&mut self) -> Result<(ThreadId, SeqEvent)> {
        loop {
            match self.receiver.recv().await {
                Ok(Event::Agent { thread, event }) => {
                    let client = self.client.clone();
                    // A gap is repaired through the *windowed* open where the daemon has one.
                    // A cursored legacy open bounds only the event tail: its `projection` is the
                    // whole thread whatever the cursor says, so on a large transcript the repair
                    // is exactly the frame the daemon has to refuse.
                    let windowed = client.supports_capability(AGENT_WINDOW_CAPABILITY);
                    let outcome = self
                        .mirror
                        .apply_or_resync(thread, &event, move |thread, from_seq| async move {
                            resume(&client, thread, from_seq, windowed).await
                        })
                        .await?;
                    // A gap that survived its own resync leaves a stale projection: reporting the
                    // event as delivered would render it as if it had been applied. `resync_all`
                    // already refuses that, and both paths have to agree (§6). A rejection is
                    // not that: the sequence is intact, one event did not reduce, and the next
                    // one gaps and repairs it — killing the subscription over it would be worse
                    // than the stale row it leaves behind.
                    match outcome {
                        MirrorOutcome::Gap { expected, got } => {
                            return Err(out_of_sequence(thread, expected, got));
                        }
                        MirrorOutcome::Rejected { seq } => {
                            tracing::warn!(%thread, %seq, "agent event refused by the client projection");
                        }
                        MirrorOutcome::Applied | MirrorOutcome::Duplicate { .. } => {}
                    }
                    return Ok((thread, event));
                }
                Ok(Event::AgentResync { thread, from_seq }) => {
                    self.resync_thread(thread, from_seq).await?;
                }
                Ok(Event::AgentSynchronized { thread }) => {
                    if !self.mirror.synchronize(thread) {
                        tracing::debug!(
                            %thread,
                            "ignored a synchronization marker for a thread this client never opened"
                        );
                    }
                }
                Ok(Event::AgentWindow { thread }) => self.refill_window(thread).await?,
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => self.resync_all().await?,
                Err(RecvError::Closed) => {
                    return Err(ProtoError {
                        kind: ErrorKind::Unknown,
                        message: "Fleet daemon event stream is closed".to_owned(),
                    });
                }
            }
        }
    }

    /// Re-reads one thread from the cursor the daemon said it dropped events after.
    ///
    /// The daemon's cursor is a claim about what this connection received; the mirror's own
    /// cursor is a fact about what it applied. Resuming from the smaller of the two is what makes
    /// an overflow lossless rather than merely reported.
    async fn resync_thread(&mut self, thread: ThreadId, from_seq: Seq) -> Result<()> {
        let applied = self.mirror.applied_seq(thread);
        let resume = from_seq.min(applied);
        tracing::debug!(
            %thread, %from_seq, %applied,
            "the daemon overflowed this connection's agent budget; resuming the thread"
        );
        if self.client.supports_capability(AGENT_WINDOW_CAPABILITY) {
            let window = self
                .client
                .agent_thread_open_window(thread, AgentWindowRequest::resume(resume))
                .await?;
            let outcome = self.mirror.install_window(&window);
            return self.settle(thread, outcome);
        }
        let snapshot = self.client.agent_thread_open(thread, Some(resume)).await?;
        let outcome = self
            .mirror
            .install_snapshot(snapshot.projection, &snapshot.events_after);
        self.settle(thread, outcome)
    }

    /// Records this client's read cursor in the daemon's `seen` table and in the mirror.
    ///
    /// The daemon is written first and the local cursor only on success: clearing the amber dot
    /// locally while the daemon still holds the old cursor is how an app restart resurrects every
    /// dot the user just cleared.
    ///
    /// # Errors
    ///
    /// Returns the daemon's own refusal; the local cursor is left untouched.
    pub async fn mark_seen(&mut self, thread: ThreadId, seq: Seq) -> Result<()> {
        self.client.agent_mark_seen(thread, seq).await?;
        self.mirror.mark_seen(thread, seq);
        Ok(())
    }

    /// Re-reads a mirrored thread whose stored window the local daemon just refilled.
    async fn refill_window(&mut self, thread: ThreadId) -> Result<()> {
        if !self.client.supports_capability(AGENT_WINDOW_CAPABILITY) {
            // Only a windowing daemon emits this, so reaching here means the capability was
            // reset by a reconnect between the emission and the delivery.
            tracing::debug!(%thread, "ignored a window notice on a connection without the capability");
            return Ok(());
        }
        if !self.mirror.projections.contains_key(&thread) {
            return Ok(());
        }
        let resume = AgentWindowRequest::resume(self.mirror.applied_seq(thread));
        let window = self.client.agent_thread_open_window(thread, resume).await?;
        let outcome = self.mirror.install_window(&window);
        self.settle(thread, outcome)
    }

    /// Re-reads every mirrored thread after the local broadcast channel lagged.
    ///
    /// This is the client's own overflow, not the daemon's: a subscriber that fell behind its
    /// bounded channel. It repairs the same way and through the same window, so a lag costs a
    /// bounded read per open thread rather than one whole transcript per open thread.
    async fn resync_all(&mut self) -> Result<()> {
        let windowed = self.client.supports_capability(AGENT_WINDOW_CAPABILITY);
        let cursors = self
            .mirror
            .projections
            .iter()
            .map(|(thread, projection)| (*thread, projection.last_seq))
            .collect::<Vec<_>>();
        for (thread, cursor) in cursors {
            let snapshot = resume(&self.client, thread, Some(cursor), windowed).await?;
            let outcome = self
                .mirror
                .install_snapshot(snapshot.projection, &snapshot.events_after);
            self.settle(thread, outcome)?;
        }
        Ok(())
    }

    /// The one place a repair's outcome is judged, so every path agrees on what is fatal.
    fn settle(&self, thread: ThreadId, outcome: MirrorOutcome) -> Result<()> {
        match outcome {
            MirrorOutcome::Gap { expected, got } => Err(out_of_sequence(thread, expected, got)),
            MirrorOutcome::Rejected { seq } => {
                tracing::warn!(%thread, %seq, "agent event refused by the client projection");
                Ok(())
            }
            MirrorOutcome::Applied | MirrorOutcome::Duplicate { .. } => Ok(()),
        }
    }
}

/// Re-reads one thread from `from_seq`, through the window when the daemon serves one.
///
/// Both shapes reduce to the same [`AgentSnapshot`], which is what lets the mirror keep one
/// continuity rule instead of one per response variant.
async fn resume(
    client: &Client,
    thread: ThreadId,
    from_seq: Option<Seq>,
    windowed: bool,
) -> Result<AgentSnapshot> {
    if !windowed {
        return client.agent_thread_open(thread, from_seq).await;
    }
    let request = AgentWindowRequest::resume(from_seq.unwrap_or_default());
    let window = client.agent_thread_open_window(thread, request).await?;
    Ok(AgentSnapshot {
        projection: super::projection_from_window(&window),
        events_after: window.events_after,
    })
}
