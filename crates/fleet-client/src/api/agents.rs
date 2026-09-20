//! Typed native-agent requests and the ordered client projection mirror.

mod events;
mod mirror;
mod window;

pub use events::AgentEvents;
pub use mirror::{AgentMirror, MirrorOutcome, PageOutcome, WindowState};
pub use window::{AgentWindowRequest, applied_seq, projection_from_window};

use std::collections::BTreeMap;

use super::{Result, unexpected};
use crate::Client;
use fleet_core::{
    agents::{
        AgentKind, AgentThreadSummary, Delegation, DelegationId, GateAnswer, GateId, ItemId,
        ModelSelection, PermissionMode, Seq, SeqEvent, StreamKind, ThreadId, ThreadProjection,
        UserInput,
    },
    ids::WorktreeId,
};
use fleet_proto::{
    AGENT_CHECKPOINTS_CAPABILITY, AGENT_ITEM_BODY_CAPABILITY, AGENT_SEEN_CAPABILITY,
    AGENT_WINDOW_CAPABILITY,
    agents::{
        AgentRevertReport, AgentSeenCursor, AgentThreadWindow, CheckpointId, TurnCheckpoint,
        checkpoints_capability_error, clamp_item_body_limit, window_capability_error,
    },
    error::{ErrorKind, ProtoError},
    request::RequestBody,
    response::ResponseBody,
};

/// Projection and ordered event tail returned when opening a thread.
///
/// The version-6 unbounded shape. A daemon advertising `agent.window` is opened with
/// [`Client::agent_thread_open_window`] instead, which is bounded and pages.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSnapshot {
    /// Materialized state at the snapshot cursor.
    pub projection: ThreadProjection,
    /// Persisted events strictly after that cursor.
    pub events_after: Vec<SeqEvent>,
}

/// One slice of a stored item body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentItemBody {
    /// Byte offset this slice starts at.
    pub offset: u64,
    /// Total stored bytes in the stream, so a caller knows when it has read it all.
    pub total: u64,
    /// The slice itself.
    pub text: String,
}

/// Parameters for starting a delegated child thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationRunRequest {
    /// Calling thread.
    pub caller: ThreadId,
    /// Provider implementation to start.
    pub provider: AgentKind,
    /// Work assigned to the child.
    pub brief: String,
    /// Completion criteria for the child.
    pub expectation: String,
    /// Optional published worktree override.
    pub worktree: Option<WorktreeId>,
    /// Optional permission-mode override.
    pub mode: Option<PermissionMode>,
    /// Optional model override.
    pub model: Option<ModelSelection>,
    /// Optional child-thread title.
    pub title: Option<String>,
    /// Absolute path to the caller's own `fleet` executable, if it could resolve one.
    ///
    /// Advisory: see [`RequestBody::DelegationRun`]'s field of the same name for what the daemon
    /// may and may not assume about it. A `String` rather than a `PathBuf` because that is the
    /// wire shape, and converting at one end only keeps a non-UTF-8 path from being silently
    /// mangled somewhere in the middle — the caller decides what to do about one, and sends
    /// `None` if it cannot express it.
    pub fleet_path: Option<String>,
    /// Extra environment variables for the child's provider process.
    ///
    /// Merged under the daemon's own `FLEET_DELEGATION` and `FLEET_DELEGATION_TOKEN`: see
    /// [`RequestBody::DelegationRun`]'s field of the same name for what the daemon guarantees.
    /// Empty is the shape every caller sent before this field existed.
    pub env: BTreeMap<String, String>,
    /// Deliver completion as soon as possible.
    pub eager: bool,
}

impl AgentItemBody {
    /// The offset to ask for next, or `None` once the whole body has been read.
    #[must_use]
    pub fn next_offset(&self) -> Option<u64> {
        let next = self.offset.saturating_add(self.text.len() as u64);
        (next < self.total).then_some(next)
    }
}

/// The one error both delivery paths report when a resync did not repair the mirror.
pub(crate) fn out_of_sequence(thread: ThreadId, expected: Seq, got: Seq) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Unknown,
        message: format!(
            "agent thread {thread} remained out of sequence after resync: expected {expected}, got {got}"
        ),
    }
}

impl Client {
    /// Starts a delegated child thread.
    pub async fn delegation_run(
        &self,
        request: DelegationRunRequest,
    ) -> Result<(Delegation, Option<String>)> {
        match self
            .request(RequestBody::DelegationRun {
                caller: request.caller,
                provider: request.provider,
                brief: request.brief,
                expectation: request.expectation,
                worktree: request.worktree,
                mode: request.mode,
                model: request.model,
                title: request.title,
                fleet_path: request.fleet_path,
                env: request.env,
                eager: request.eager,
            })
            .await?
        {
            ResponseBody::DelegationStarted {
                delegation,
                warning,
            } => Ok((delegation, warning)),
            response => Err(unexpected("delegation_run", response)),
        }
    }

    /// Reports a delegated child's completion.
    pub async fn delegation_complete(
        &self,
        delegation: DelegationId,
        child: ThreadId,
        token: String,
        result: String,
        blocked: bool,
    ) -> Result<Delegation> {
        match self
            .request(RequestBody::DelegationComplete {
                delegation,
                child,
                token,
                result,
                blocked,
            })
            .await?
        {
            ResponseBody::Delegation(delegation) => Ok(delegation),
            response => Err(unexpected("delegation_complete", response)),
        }
    }

    /// Lists delegations, optionally for one caller.
    pub async fn delegation_list(&self, caller: Option<ThreadId>) -> Result<Vec<Delegation>> {
        match self.request(RequestBody::DelegationList { caller }).await? {
            ResponseBody::Delegations(delegations) => Ok(delegations),
            response => Err(unexpected("delegation_list", response)),
        }
    }

    /// Reads one delegation.
    pub async fn delegation_get(&self, delegation: DelegationId) -> Result<Delegation> {
        match self
            .request(RequestBody::DelegationGet { delegation })
            .await?
        {
            ResponseBody::Delegation(delegation) => Ok(delegation),
            response => Err(unexpected("delegation_get", response)),
        }
    }

    /// Cancels one delegation and returns its current record.
    pub async fn delegation_cancel(&self, delegation: DelegationId) -> Result<Delegation> {
        match self
            .request(RequestBody::DelegationCancel { delegation })
            .await?
        {
            ResponseBody::Delegation(delegation) => Ok(delegation),
            response => Err(unexpected("delegation_cancel", response)),
        }
    }

    /// Waits for one delegation to become terminal or the deadline to expire.
    ///
    /// `caller` names the thread this wait is issued for, when the waiter knows its own. It is
    /// advisory identity, not authorisation: passing the delegation's own caller is what marks a
    /// terminal result read, so the daemon never injects it into that thread a second time.
    /// `None` — a shell with no session of its own — waits exactly as this call always has.
    pub async fn delegation_wait(
        &self,
        delegation: DelegationId,
        timeout_ms: u64,
        caller: Option<ThreadId>,
    ) -> Result<Delegation> {
        match self
            .request(RequestBody::DelegationWait {
                delegation,
                timeout_ms,
                caller,
            })
            .await?
        {
            ResponseBody::Delegation(delegation) => Ok(delegation),
            response => Err(unexpected("delegation_wait", response)),
        }
    }

    /// Reads the stable installation's persisted agent cursors after capability negotiation.
    pub async fn agent_seen_cursors(&self) -> Result<Vec<AgentSeenCursor>> {
        if !self.supports_capability(AGENT_SEEN_CAPABILITY) {
            return Ok(Vec::new());
        }
        match self.request(RequestBody::AgentSeenCursors).await? {
            ResponseBody::AgentSeenCursors(cursors) => Ok(cursors),
            response => Err(unexpected("agent_seen_cursors", response)),
        }
    }

    /// Reads the stable installation's closed thread set after capability negotiation.
    pub async fn agent_closed_threads(&self) -> Result<Vec<ThreadId>> {
        if !self.supports_capability(fleet_proto::AGENT_CLOSED_CAPABILITY) {
            return Ok(Vec::new());
        }
        match self.request(RequestBody::AgentClosedThreads).await? {
            ResponseBody::AgentClosedThreads(threads) => Ok(threads),
            response => Err(unexpected("agent_closed_threads", response)),
        }
    }

    /// Creates an ordered agent-event subscription with its own projection mirror.
    #[must_use]
    pub fn agent_events(&self) -> AgentEvents {
        AgentEvents::new(self.clone())
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
        mode: Option<PermissionMode>,
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
            .request(RequestBody::AgentThreadOpen {
                thread,
                from_seq,
                after_seq: None,
                turn_limit: None,
                before_cursor: None,
                request_sync_marker: false,
            })
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

    /// Opens a thread and reads a bounded window of its transcript.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Unsupported`] *without sending anything* when the negotiated daemon
    /// did not advertise `agent.window`. A window field on an older daemon is not a harmless
    /// unknown key: it ignores the field and answers the whole transcript, which is the frame
    /// the window exists to avoid — and above the ceiling that frame is undecodable. Capabilities
    /// reset on disconnect, so this is asked per request and never cached.
    pub async fn agent_thread_open_window(
        &self,
        thread: ThreadId,
        request: AgentWindowRequest,
    ) -> Result<AgentThreadWindow> {
        if !self.supports_capability(AGENT_WINDOW_CAPABILITY) {
            return Err(window_capability_error());
        }
        match self.request(request.into_body(thread)).await? {
            ResponseBody::AgentThreadWindow(window) => Ok(*window),
            response => Err(unexpected("agent_thread_open_window", response)),
        }
    }

    /// Reads one slice of a stored item body, for an output the window elided.
    ///
    /// `limit` is clamped to 256 KiB before it is sent, so the answer is one page and the caller
    /// walks `offset` against the returned `total`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Unsupported`] without sending anything when the daemon cannot serve
    /// body ranges.
    pub async fn agent_item_body(
        &self,
        thread: ThreadId,
        item: ItemId,
        stream: StreamKind,
        offset: u64,
        limit: u32,
    ) -> Result<AgentItemBody> {
        if !self.supports_capability(AGENT_ITEM_BODY_CAPABILITY) {
            return Err(ProtoError {
                kind: ErrorKind::Unsupported,
                message: format!(
                    "this daemon cannot serve stored item bodies (capability `{AGENT_ITEM_BODY_CAPABILITY}`)"
                ),
            });
        }
        match self
            .request(RequestBody::AgentItemBody {
                thread,
                item,
                stream,
                offset,
                limit: clamp_item_body_limit(limit),
            })
            .await?
        {
            ResponseBody::AgentItemBodyChunk {
                offset,
                total,
                text,
                ..
            } => Ok(AgentItemBody {
                offset,
                total,
                text,
            }),
            response => Err(unexpected("agent_item_body", response)),
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

    /// Re-registers this installation's interest in a native-agent thread.
    pub async fn agent_thread_reopen(&self, thread: ThreadId) -> Result<()> {
        expect_agent_ack(
            "agent_thread_reopen",
            self.request(RequestBody::AgentThreadReopen { thread })
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

    /// Lists the Fleet-owned checkpoints a thread's worktree can be reverted to, oldest first.
    ///
    /// This is what `[u]` is drawn from, so the capability check matters: a daemon without the
    /// checkpoint service must answer "unsupported", never an empty list, or the surface would
    /// read "nothing to revert to" and silently stop offering an undo that exists.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Unsupported`] without sending anything when the daemon does not keep
    /// checkpoints.
    pub async fn agent_checkpoints(&self, thread: ThreadId) -> Result<Vec<TurnCheckpoint>> {
        if !self.supports_capability(AGENT_CHECKPOINTS_CAPABILITY) {
            return Err(checkpoints_capability_error());
        }
        match self
            .request(RequestBody::AgentCheckpoints { thread })
            .await?
        {
            ResponseBody::AgentCheckpoints(checkpoints) => Ok(checkpoints),
            response => Err(unexpected("agent_checkpoints", response)),
        }
    }

    /// Restores a thread's worktree from one of its checkpoints, and reports what changed.
    ///
    /// Files only: the harness's conversation is untouched (`docs/NATIVE-AGENTS.md` §5).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Unsupported`] without sending anything when the daemon does not keep
    /// checkpoints, and [`ErrorKind::NotFound`] when the checkpoint is gone — which is what a
    /// `[u]` drawn from a stale listing gets, and is distinguishable from a Git failure.
    pub async fn agent_revert(
        &self,
        thread: ThreadId,
        checkpoint: CheckpointId,
    ) -> Result<AgentRevertReport> {
        if !self.supports_capability(AGENT_CHECKPOINTS_CAPABILITY) {
            return Err(checkpoints_capability_error());
        }
        match self
            .request(RequestBody::AgentRevert { thread, checkpoint })
            .await?
        {
            ResponseBody::AgentReverted(report) => Ok(report),
            response => Err(unexpected("agent_revert", response)),
        }
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
