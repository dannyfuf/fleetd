//! Typed native-agent requests and the ordered client projection mirror.

mod mirror;

pub use mirror::{AgentMirror, MirrorOutcome};

use super::{Result, unexpected};
use crate::Client;
use fleet_core::{
    agents::{
        AgentKind, AgentThreadSummary, GateAnswer, GateId, ModelSelection, PermissionMode, Seq,
        SeqEvent, ThreadId, ThreadProjection, UserInput,
    },
    ids::WorktreeId,
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    event::Event,
    request::RequestBody,
    response::ResponseBody,
};
use tokio::sync::broadcast::{self, error::RecvError};

/// Projection and ordered event tail returned when opening a thread.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSnapshot {
    /// Materialized state at the snapshot cursor.
    pub projection: ThreadProjection,
    /// Persisted events strictly after that cursor.
    pub events_after: Vec<SeqEvent>,
}

/// Ordered native-agent event subscription with automatic per-thread gap recovery.
pub struct AgentEvents {
    client: Client,
    receiver: broadcast::Receiver<Event>,
    mirror: AgentMirror,
}

impl AgentEvents {
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
    pub async fn recv(&mut self) -> Result<(ThreadId, SeqEvent)> {
        loop {
            match self.receiver.recv().await {
                Ok(Event::Agent { thread, event }) => {
                    let client = self.client.clone();
                    let outcome = self
                        .mirror
                        .apply_or_resync(thread, &event, move |thread, from_seq| async move {
                            client.agent_thread_open(thread, from_seq).await
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

    async fn resync_all(&mut self) -> Result<()> {
        let cursors = self
            .mirror
            .projections
            .iter()
            .map(|(thread, projection)| (*thread, projection.last_seq))
            .collect::<Vec<_>>();
        for (thread, cursor) in cursors {
            let snapshot = self.client.agent_thread_open(thread, Some(cursor)).await?;
            let outcome = self
                .mirror
                .install_snapshot(snapshot.projection, &snapshot.events_after);
            match outcome {
                MirrorOutcome::Gap { expected, got } => {
                    return Err(out_of_sequence(thread, expected, got));
                }
                MirrorOutcome::Rejected { seq } => {
                    tracing::warn!(%thread, %seq, "agent event refused by the client projection");
                }
                MirrorOutcome::Applied | MirrorOutcome::Duplicate { .. } => {}
            }
        }
        Ok(())
    }
}

/// The one error both delivery paths report when a resync did not repair the mirror.
fn out_of_sequence(thread: ThreadId, expected: Seq, got: Seq) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unknown,
        message: format!(
            "agent thread {thread} remained out of sequence after resync: expected {expected}, got {got}"
        ),
    }
}

impl Client {
    /// Creates an ordered agent-event subscription with its own projection mirror.
    #[must_use]
    pub fn agent_events(&self) -> AgentEvents {
        AgentEvents {
            client: self.clone(),
            receiver: self.events(),
            mirror: AgentMirror::default(),
        }
    }

    /// Lists persisted and live native-agent thread summaries.
    pub async fn agent_thread_list(&self) -> Result<Vec<AgentThreadSummary>> {
        match self.request(RequestBody::AgentThreadList).await? {
            ResponseBody::AgentThreads(threads) => Ok(threads),
            response => Err(unexpected("agent_thread_list", response)),
        }
    }

    /// Creates and starts a native-agent thread.
    #[allow(clippy::too_many_arguments)]
    pub async fn agent_thread_create(
        &self,
        worktree: WorktreeId,
        provider: AgentKind,
        model: Option<ModelSelection>,
        mode: PermissionMode,
        resume_cursor: Option<String>,
        title: Option<String>,
    ) -> Result<AgentThreadSummary> {
        match self
            .request(RequestBody::AgentThreadCreate {
                worktree,
                provider,
                model,
                mode,
                resume_cursor,
                title,
            })
            .await?
        {
            ResponseBody::AgentThreadCreated(thread) => Ok(thread),
            response => Err(unexpected("agent_thread_create", response)),
        }
    }

    /// Opens a thread from an optional last-applied sequence.
    pub async fn agent_thread_open(
        &self,
        thread: ThreadId,
        from_seq: Option<Seq>,
    ) -> Result<AgentSnapshot> {
        match self
            .request(RequestBody::AgentThreadOpen { thread, from_seq })
            .await?
        {
            ResponseBody::AgentThreadSnapshot {
                projection,
                events_after,
            } => Ok(AgentSnapshot {
                projection,
                events_after,
            }),
            response => Err(unexpected("agent_thread_open", response)),
        }
    }

    /// Releases this client's open-thread lease.
    pub async fn agent_thread_close(&self, thread: ThreadId) -> Result<()> {
        expect_agent_ack(
            "agent_thread_close",
            self.request(RequestBody::AgentThreadClose { thread })
                .await?,
        )
    }

    /// Sends or steers user input in a thread.
    pub async fn agent_send(&self, thread: ThreadId, input: UserInput) -> Result<()> {
        expect_agent_ack(
            "agent_send",
            self.request(RequestBody::AgentSend { thread, input })
                .await?,
        )
    }

    /// Requests interruption of the active turn.
    pub async fn agent_interrupt(&self, thread: ThreadId) -> Result<()> {
        expect_agent_ack(
            "agent_interrupt",
            self.request(RequestBody::AgentInterrupt { thread }).await?,
        )
    }

    /// Answers an open permission, question, or plan gate.
    pub async fn agent_respond(
        &self,
        thread: ThreadId,
        gate: GateId,
        answer: GateAnswer,
    ) -> Result<()> {
        expect_agent_ack(
            "agent_respond",
            self.request(RequestBody::AgentRespond {
                thread,
                gate,
                answer,
            })
            .await?,
        )
    }

    /// Changes a thread's permission or plan mode.
    pub async fn agent_set_mode(&self, thread: ThreadId, mode: PermissionMode) -> Result<()> {
        expect_agent_ack(
            "agent_set_mode",
            self.request(RequestBody::AgentSetMode { thread, mode })
                .await?,
        )
    }

    /// Changes a thread's model and optional effort/provider.
    pub async fn agent_set_model(&self, thread: ThreadId, model: ModelSelection) -> Result<()> {
        expect_agent_ack(
            "agent_set_model",
            self.request(RequestBody::AgentSetModel { thread, model })
                .await?,
        )
    }

    /// Marks a sequence visible to this client.
    pub async fn agent_mark_seen(&self, thread: ThreadId, seq: Seq) -> Result<()> {
        expect_agent_ack(
            "agent_mark_seen",
            self.request(RequestBody::AgentMarkSeen { thread, seq })
                .await?,
        )
    }

    /// Stops the provider process while retaining the transcript.
    pub async fn agent_stop(&self, thread: ThreadId) -> Result<()> {
        expect_agent_ack(
            "agent_stop",
            self.request(RequestBody::AgentStop { thread }).await?,
        )
    }
}

fn expect_agent_ack(operation: &str, response: ResponseBody) -> Result<()> {
    match response {
        ResponseBody::AgentAck => Ok(()),
        ResponseBody::Ack => Err(ProtoError {
            kind: ErrorKind::Unknown,
            message: format!("legacy acknowledgement returned for {operation}"),
        }),
        response => Err(unexpected(operation, response)),
    }
}
