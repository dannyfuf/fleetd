//! The request handlers: one method per `Agent*` verb the wire carries.
//!
//! They are the only public surface of the manager besides the list, and they share one shape:
//! resolve the thread (hydrating it on first use), take its serialized-operation gate, talk to the
//! provider, and record what happened as appended events. Nothing here writes projected state
//! directly — §3 makes the reducer the only writer of it, and [`super::apply`] is the only path to
//! the reducer.

use std::{collections::BTreeMap, path::PathBuf};

use chrono::Utc;
use fleet_core::{
    agents::{
        AbortReason, AgentEvent, AgentKind, ControlCost, DelegationId, GateAnswer, GateId,
        ModelSelection, PermissionMode, Seq, StartRequest, SteerSupport, ThreadId,
        ThreadProjection, TurnId, TurnState, UserInput,
    },
    ids::WorktreeId,
};
use fleet_proto::{
    agents::{AgentSeenCursor, snapshot_ceiling_error, snapshot_fits, wire_bytes},
    error::{ErrorKind, ProtoError},
    event::Event,
    request::RequestBody,
    response::ResponseBody,
};

use crate::agents::harness::{AccountOp, AccountOutcome, RuntimeChange};

use super::{
    AgentSessionManager, AgentThreadRecord, SubmissionState, ThreadRuntime,
    apply::{publish_applied, runtime_inflight},
    conflict, controls_for_mode, daemon_error, hydrate, not_found, provider_error,
    provider_factory_error, provider_start_error, storage_error, validation,
    window::OpenRequest,
};

/// Everything one new thread needs, including the delegation facts a child carries.
///
/// A struct rather than six more positional arguments: `create` grew a parent, a delegation and
/// an environment the moment a thread could be spawned by another thread rather than by a user,
/// and `docs/NATIVE-AGENTS.md` §15 adds more of those than a call site can read positionally.
#[derive(Debug, Clone)]
pub struct CreateOptions {
    /// Preallocated identity, used when a delegation must be durable before provider events run.
    pub thread: Option<ThreadId>,
    /// Worktree the child runs in.
    pub worktree: WorktreeId,
    /// Harness to start.
    pub provider: AgentKind,
    /// Model selection, or the harness default.
    pub model: Option<ModelSelection>,
    /// Permission-mode override, or the selected harness's configured default.
    pub mode: Option<PermissionMode>,
    /// Cursor to resume an existing harness session from.
    pub resume_cursor: Option<String>,
    /// Title, or the provider's display name.
    pub title: Option<String>,
    /// Caller thread, when this thread is a delegated child.
    pub parent: Option<ThreadId>,
    /// Delegation that spawned this thread, when one did.
    pub delegation: Option<DelegationId>,
    /// Extra environment for the child process, merged before `FLEET_SESSION`.
    ///
    /// This is how `FLEET_DELEGATION` and `FLEET_DELEGATION_TOKEN` reach the child: both adapters
    /// already extend their overrides with `StartRequest::env`, so nothing harness-specific is
    /// needed to carry a secret the child alone may use.
    pub extra_env: BTreeMap<String, String>,
    /// Directory prepended to the child's `PATH`, when the daemon resolved one.
    ///
    /// Carried separately from `extra_env` on purpose: both adapters treat an `env` entry as a
    /// whole-value override, so a `PATH` there would discard the login shell's own.
    pub path_prepend: Option<PathBuf>,
}

impl CreateOptions {
    /// The options a plain `AgentThreadCreate` carries: no parent, no delegation, no environment.
    #[must_use]
    pub fn new(worktree: WorktreeId, provider: AgentKind, mode: Option<PermissionMode>) -> Self {
        Self {
            thread: None,
            worktree,
            provider,
            model: None,
            mode,
            resume_cursor: None,
            title: None,
            parent: None,
            delegation: None,
            extra_env: BTreeMap::new(),
            path_prepend: None,
        }
    }
}

impl AgentSessionManager {
    /// Handles `AgentThreadCreate`.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        worktree: WorktreeId,
        provider_kind: AgentKind,
        model: Option<ModelSelection>,
        mode: Option<PermissionMode>,
        resume_cursor: Option<String>,
        title: Option<String>,
    ) -> Result<ResponseBody, ProtoError> {
        self.create_with(CreateOptions {
            model,
            resume_cursor,
            title,
            ..CreateOptions::new(worktree, provider_kind, mode)
        })
        .await
    }

    /// Creates a thread from the full option set, which is what a delegated child needs.
    pub async fn create_with(&self, options: CreateOptions) -> Result<ResponseBody, ProtoError> {
        let CreateOptions {
            thread,
            worktree,
            provider: provider_kind,
            model,
            mode,
            resume_cursor,
            title,
            parent,
            delegation,
            extra_env,
            path_prepend,
        } = options;
        let remote_host = self
            .inner
            .remote_host_resolver
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|resolver| resolver(&worktree));
        if let Some(host) = remote_host {
            return Err(ProtoError {
                kind: ErrorKind::Remote,
                message: format!(
                    "agent thread for worktree {worktree} belongs to remote host {host}; local manager refused it"
                ),
            });
        }
        // Before the path lookup and before the provider: a manager with no database must not
        // start a child it cannot record.
        let store = self.inner.store().map_err(storage_error)?.clone();
        let path = self
            .inner
            .worktrees
            .path(worktree.clone())
            .await
            .map(PathBuf::from)
            .map_err(daemon_error)?;
        let thread = thread.unwrap_or_else(ThreadId::new);
        let (binaries, native_agents, config_notice) = self.native_agent_settings().await;
        let defaults = native_agents.for_kind(provider_kind);
        let mode = mode.unwrap_or(defaults.mode);
        if !provider_kind.supported_modes().contains(&mode) {
            return Err(validation(format!(
                "{} does not support permission mode {mode:?}",
                provider_kind.display_name()
            )));
        }
        let model = resolve_model_selection(model, defaults);
        let (sandbox, approval_policy) = controls_for_mode(mode);
        let request = StartRequest {
            thread,
            worktree_path: path,
            provider: provider_kind,
            model: model.clone(),
            mode,
            resume_cursor: resume_cursor.clone(),
            fork: false,
            env: extra_env,
            sandbox,
            approval_policy,
            permission_profile: None,
            title: title.clone(),
            path_prepend,
        };
        let command = binaries.binary(provider_kind).to_owned();
        let worktree_path = request.worktree_path.clone();
        let mut provider = (self.inner.provider_factory)(provider_kind, &request, &binaries)
            .map_err(|error| {
                provider_factory_error(provider_kind, &command, &worktree_path, error)
            })?;
        provider.start(request).await.map_err(|error| {
            provider_start_error(provider_kind, &command, &worktree_path, error)
        })?;
        let provider_events = provider.events();
        // The adapter's own cursor, durable before the harness has said anything: Claude only
        // publishes `system/init` with the first prompt, and a thread the daemon loses before
        // then must still be the same thread when it comes back.
        let resume_cursor = provider.resume_cursor().or(resume_cursor);
        let created = Utc::now();
        let resolved_title = title.unwrap_or_else(|| provider_kind.display_name().to_owned());
        tracing::info!(
            target: "fleet::agents",
            %thread,
            provider = %provider_kind.display_name(),
            worktree = %worktree_path.display(),
            has_cursor = resume_cursor.is_some(),
            "agent thread created"
        );
        let record = AgentThreadRecord {
            thread,
            parent,
            delegation,
            worktree: worktree.clone(),
            provider: provider_kind,
            title: resolved_title.clone(),
            created,
            last_activity: created,
            resume_cursor,
            model: model.clone(),
            mode,
            last_outcome: None,
            stop_cause: None,
        };
        let mut projection = ThreadProjection::new(thread, worktree, provider_kind);
        // §1.5: the caller travels on the projection so the summary a client lists a child under
        // carries it from the thread's very first frame, not from the delegation record it would
        // have to join against.
        projection.parent = parent;
        projection.title = resolved_title;
        projection.model = model;
        projection.mode = mode;
        let runtime = ThreadRuntime::new(projection, record.clone(), None);
        *runtime.provider.lock().await = Some(provider);
        let provider_generation = runtime.next_provider_generation();

        // The row is written before the runtime is published, so a thread a client can see is a
        // thread the next start will find. One upsert, not a whole-index rewrite.
        if let Err(error) = store.write_record(&record).await {
            if let Some(mut provider) = runtime.provider.lock().await.take()
                && let Err(stop_error) = provider.stop().await
            {
                // The orphan outlives this log line, but nothing else names it: the thread was
                // never inserted, so the discarded `Err` was the only trace of the child still
                // holding the worktree and its port.
                tracing::warn!(
                    target: "fleet::agents",
                    error = %stop_error,
                    %thread,
                    "could not stop the provider after a failed thread record write",
                );
            }
            return Err(storage_error(error));
        }
        self.inner
            .threads
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(thread, runtime.clone());
        if let Some(notice) = config_notice {
            let operation = runtime.operation.lock().await;
            if let Err(error) = self
                .apply_one(
                    &runtime,
                    &operation,
                    AgentEvent::Notice(notice.to_owned()),
                    Some("config_fallback".to_owned()),
                )
                .await
            {
                tracing::warn!(
                    target: "fleet::agents",
                    %error,
                    %thread,
                    "could not record the native-agent configuration fallback notice",
                );
            }
        }
        drop(self.spawn_event_task(runtime.clone(), provider_events, provider_generation));
        let summary = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .projection
            .summary(Seq::default());
        self.inner
            .events
            .publish(Event::AgentSummary(summary.clone()));
        Ok(ResponseBody::AgentThreadCreated(summary))
    }

    /// Handles `AgentThreadOpen`.
    ///
    /// Three answers, and the request chooses between them. A thread another host owns and this
    /// daemon has mirrored is answered from the mirror with `synchronized = false` — the caller
    /// brings the mirror up to the owner's head in parallel, so no transcript crosses the link
    /// (`docs/NATIVE-AGENTS.md` §9.3). A peer that asked for a window gets one, bounded by turn
    /// count and by the admission ladder. A version-6 peer gets the unbounded snapshot whose
    /// meaning never changes — or, past the frame it would not survive, the typed refusal that
    /// names the capability which fixes it.
    pub async fn open(&self, body: &RequestBody) -> Result<ResponseBody, ProtoError> {
        let request = OpenRequest::from_body(body)
            .ok_or_else(|| validation("that request is not an agent thread open"))?;
        // Before `runtime`, which hydrates: a mirrored thread must not reach a path that could
        // resume a provider for it (§9.3, authority rule 2).
        if let Some(answer) = self.mirror_open(&request).await {
            return answer;
        }
        let runtime = self.runtime(request.thread).await?;
        // A catch-up open is a client repairing a sequence gap, not a user opening a tab: §6
        // resumes a stopped thread lazily "the next time it is opened", which is a user action.
        // Relaunching a provider process for every mirror that lagged is not.
        if request.resume.is_none() {
            self.resume_if_stopped(&runtime).await?;
        }

        if request.windowed {
            let projection = {
                let state = runtime
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                self.validate_resume(&request, state.projection.last_seq)?;
                state.projection.clone()
            };
            let answer = self.window_for(&request, &projection, true).await?;
            if request.sync_marker {
                // This daemon owns the thread, so it is the thing whose word `Live` means. The
                // marker follows the catch-up the response carries.
                self.publish_synchronized(request.thread);
            }
            return Ok(answer);
        }

        let Some(cursor) = request.resume else {
            let state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            return snapshot_response(state.projection.clone(), Vec::new());
        };
        let record = {
            let state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.validate_resume(&request, state.projection.last_seq)?;
            state.record.clone()
        };
        // A whole-transcript read is disk work, and a rendering client is waiting on it: the
        // store runs it on the blocking pool, on a read-only connection, under a snapshot.
        let events = self
            .inner
            .store()
            .map_err(storage_error)?
            .load(request.thread)
            .await
            .map_err(storage_error)?;
        let mut projection = hydrate::projection_seed(&record);
        for event in events.iter().filter(|event| event.seq <= cursor) {
            projection.apply(event).map_err(|error| {
                storage_error(anyhow::anyhow!(error).context("replay agent snapshot"))
            })?;
        }
        let events_after = events
            .into_iter()
            .filter(|event| event.seq > cursor)
            .collect();
        snapshot_response(projection, events_after)
    }

    /// Refuses a resume cursor no event ever carried.
    fn validate_resume(&self, request: &OpenRequest, last_seq: Seq) -> Result<(), ProtoError> {
        match request.resume {
            Some(cursor) if cursor > last_seq => Err(validation(format!(
                "thread {} has no sequence {cursor}; latest is {last_seq}",
                request.thread
            ))),
            _ => Ok(()),
        }
    }

    /// Handles `AgentThreadClose`.
    ///
    /// It deliberately does not hydrate: closing a tab is the one verb that has no use for the
    /// reducer, and building a projection to answer an ack would be the O(whole thread) read this
    /// store exists to avoid.
    pub async fn close(&self, thread: ThreadId) -> Result<ResponseBody, ProtoError> {
        self.ensure_thread_exists(thread).await?;
        Ok(ResponseBody::AgentAck)
    }

    /// Handles `AgentThreadReopen` without hydrating the thread.
    pub async fn reopen(&self, thread: ThreadId) -> Result<ResponseBody, ProtoError> {
        self.ensure_thread_exists(thread).await?;
        Ok(ResponseBody::AgentAck)
    }

    async fn ensure_thread_exists(&self, thread: ThreadId) -> Result<(), ProtoError> {
        if self.hydrated(thread).is_none()
            && self
                .inner
                .store()
                .map_err(storage_error)?
                .read_record(thread)
                .await
                .map_err(storage_error)?
                .is_none()
        {
            return Err(not_found(format!("agent thread {thread}")));
        }
        Ok(())
    }

    /// Handles `AgentSend`.
    pub async fn send(
        &self,
        thread: ThreadId,
        input: UserInput,
    ) -> Result<ResponseBody, ProtoError> {
        self.send_inner(thread, input, false).await
    }

    /// Sends a durable outbox-owned input and commits its stable item before acknowledging it.
    ///
    /// Delegation workers cannot treat a provider return as durable: a database failure after the
    /// provider accepted the input would otherwise make the open row submit it again. Ordinary UI
    /// sends still wait for the provider's `TurnStarted`; outbox sends use this tighter boundary.
    pub(crate) async fn send_durable(
        &self,
        thread: ThreadId,
        input: UserInput,
    ) -> Result<ResponseBody, ProtoError> {
        self.send_inner(thread, input, true).await
    }

    async fn send_inner(
        &self,
        thread: ThreadId,
        input: UserInput,
        durable: bool,
    ) -> Result<ResponseBody, ProtoError> {
        if input.text.trim().is_empty() && input.attachments.is_empty() {
            return Err(validation("agent input cannot be empty"));
        }
        self.refuse_if_mirrored(thread, "send").await?;
        let runtime = self.runtime(thread).await?;
        // §6 resumes a stopped thread lazily; §7 makes `AgentSend` a first-class verb of the
        // same request set the app uses. Taken together, a send to a thread whose provider a
        // restart took away resumes it rather than refusing until something else opens the tab.
        // Resuming takes the operation lock itself, so it happens before this one does.
        self.resume_if_stopped(&runtime).await?;
        let operation = runtime.operation.lock().await;
        let (turn, projected_running, kind) = {
            let state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let projected = match state.projection.turn {
                TurnState::Running(turn) => Some(turn),
                _ => None,
            };
            (
                projected
                    .or(state.inflight_turn)
                    .unwrap_or_else(TurnId::new),
                projected.is_some(),
                state.record.provider,
            )
        };
        let had_inflight = runtime_inflight(&runtime).is_some();
        // Before the harness is asked to do anything, and only for a turn that is actually
        // starting: a steer joins a turn whose checkpoint already exists, and taking a second
        // one would make `[u] revert turn` restore the middle of the turn rather than its start.
        if !projected_running && !had_inflight {
            let worktree = {
                let state = runtime
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.record.worktree.clone()
            };
            self.capture_turn_checkpoint(thread, turn, &worktree).await;
        }
        let mut provider_slot = runtime.provider.lock().await;
        let provider = provider_slot
            .as_mut()
            .ok_or_else(|| conflict(format!("agent thread {thread} is not live")))?;
        if (projected_running || had_inflight)
            && matches!(provider.capabilities().steer, SteerSupport::None)
        {
            return Err(conflict(format!(
                "{} does not support steering",
                kind.display_name()
            )));
        }
        // The harness answers both halves the manager must not guess: which turn the message
        // landed in, and whether it joined one that was already running.
        let submitted = provider
            .send(turn, input.clone())
            .await
            .map_err(provider_error)?;
        drop(provider_slot);
        let turn = submitted.turn();
        {
            let mut state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.inflight_turn = Some(turn);
            if !submitted.joined_active() {
                state.pending_inputs.push_back((turn, input.clone()));
            }
        }
        if submitted.joined_active() {
            // A steer: the turn is already running, so there is no announcement to wait for and
            // the bubble is recorded now, marked as having joined it (§7.2).
            let item = input.item.unwrap_or_default();
            self.record_user_input(&runtime, &operation, turn, item, input, true)
                .await?;
        } else if durable && !projected_running && !had_inflight {
            // The stable item is the durable acknowledgement keyed by the outbox row. Provider
            // `TurnStarted` remains useful, but if it arrives later it is a harmless duplicate;
            // the worker may reconcile this committed item instead of resubmitting the prompt.
            for applied in self
                .flush_pending_inputs(&runtime, &operation, turn, "durable_outbox_submission")
                .await?
            {
                publish_applied(&self.inner, &runtime, applied);
            }
        }
        Ok(ResponseBody::AgentAck)
    }

    /// Reconciles a stable outbox item with the durable transcript or accepted-input queue.
    pub(crate) async fn submission_state(
        &self,
        thread: ThreadId,
        item: fleet_core::agents::ItemId,
    ) -> Result<SubmissionState, ProtoError> {
        let runtime = self.runtime(thread).await?;
        let state = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .projection
            .items
            .iter()
            .any(|candidate| candidate.id == item)
        {
            return Ok(SubmissionState::Committed);
        }
        if state
            .pending_inputs
            .iter()
            .any(|(_, input)| input.item == Some(item))
        {
            return Ok(SubmissionState::Pending);
        }
        Ok(SubmissionState::Unknown)
    }

    /// Commits an input the provider accepted when the first transcript write failed.
    pub(crate) async fn reconcile_submission(
        &self,
        thread: ThreadId,
        item: fleet_core::agents::ItemId,
    ) -> Result<SubmissionState, ProtoError> {
        let runtime = self.runtime(thread).await?;
        let operation = runtime.operation.lock().await;
        let pending_turn = {
            let state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state
                .projection
                .items
                .iter()
                .any(|candidate| candidate.id == item)
            {
                return Ok(SubmissionState::Committed);
            }
            state
                .pending_inputs
                .iter()
                .find_map(|(turn, input)| (input.item == Some(item)).then_some(*turn))
        };
        let Some(turn) = pending_turn else {
            return Ok(SubmissionState::Unknown);
        };
        for applied in self
            .flush_pending_inputs(&runtime, &operation, turn, "durable_outbox_reconcile")
            .await?
        {
            publish_applied(&self.inner, &runtime, applied);
        }
        Ok(SubmissionState::Committed)
    }

    /// Resumes a stopped provider and drains the history it returned during open before an
    /// outbox row decides whether its stable item needs to be retried.
    pub(crate) async fn reconcile_provider_history(
        &self,
        thread: ThreadId,
        item: fleet_core::agents::ItemId,
    ) -> Result<SubmissionState, ProtoError> {
        let runtime = self.runtime(thread).await?;
        self.resume_if_stopped(&runtime).await?;
        self.submission_state(thread, item).await
    }

    /// Handles `AgentInterrupt`.
    ///
    /// Interrupting a turn that has already settled is a no-op, not a conflict: `esc` and the
    /// `result` frame that ends the turn race by milliseconds, and the client cannot see the
    /// settle coming. Reporting that race as an error put a sticky `conflict: agent thread …
    /// has no active turn` in the status bar for something the user did nothing wrong to cause.
    pub async fn interrupt(&self, thread: ThreadId) -> Result<ResponseBody, ProtoError> {
        self.refuse_if_mirrored(thread, "interrupt").await?;
        let runtime = self.runtime(thread).await?;
        let _operation = runtime.operation.lock().await;
        let Some(turn) = runtime_inflight(&runtime) else {
            return Ok(ResponseBody::AgentAck);
        };
        let mut provider_slot = runtime.provider.lock().await;
        let provider = provider_slot
            .as_mut()
            .ok_or_else(|| conflict(format!("agent thread {thread} is not live")))?;
        provider.interrupt(turn).await.map_err(provider_error)?;
        Ok(ResponseBody::AgentAck)
    }

    /// Handles `AgentRespond`.
    pub async fn respond(
        &self,
        thread: ThreadId,
        gate: GateId,
        answer: GateAnswer,
    ) -> Result<ResponseBody, ProtoError> {
        // The owner's reducer is the only thing that decides whether a gate answer was accepted,
        // so a mirrored gate is never resolved here on optimism (§9.3, authority rule 3).
        self.refuse_if_mirrored(thread, "respond").await?;
        let runtime = self.runtime(thread).await?;
        let _operation = runtime.operation.lock().await;
        // The projection only drops the gate when the provider's `GateResolved` is drained, so
        // two answers issued back to back would both pass an open-gate check and both write a
        // control response for one request id. §4.3: duplicate settlements are idempotent.
        {
            let mut state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.answered_gates.contains(&gate) {
                return Ok(ResponseBody::AgentAck);
            }
            if !state
                .projection
                .gates
                .iter()
                .any(|candidate| candidate.id == gate)
            {
                return Err(conflict(format!(
                    "gate {gate} is not open in agent thread {thread}"
                )));
            }
            state.answered_gates.insert(gate);
        }
        let answered = runtime
            .provider
            .lock()
            .await
            .as_mut()
            .ok_or_else(|| conflict(format!("agent thread {thread} is not live")))?
            .respond(gate, answer)
            .await;
        if let Err(error) = answered {
            runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .answered_gates
                .remove(&gate);
            return Err(provider_error(error));
        }
        Ok(ResponseBody::AgentAck)
    }

    /// Handles `AgentSetMode`.
    pub async fn set_mode(
        &self,
        thread: ThreadId,
        mode: PermissionMode,
    ) -> Result<ResponseBody, ProtoError> {
        self.refuse_if_mirrored(thread, "set mode").await?;
        let runtime = self.runtime(thread).await?;
        let operation = runtime.operation.lock().await;
        let mut provider_slot = runtime.provider.lock().await;
        let provider = provider_slot
            .as_mut()
            .ok_or_else(|| conflict(format!("agent thread {thread} is not live")))?;
        if provider.capabilities().mode_switch == ControlCost::NotSupported {
            return Err(conflict(format!(
                "{} does not support changing mode",
                provider.kind().display_name()
            )));
        }
        if !provider.capabilities().modes.contains(&mode) {
            return Err(validation(format!(
                "{} does not support permission mode {mode:?}",
                provider.kind().display_name()
            )));
        }
        drop(provider_slot);
        self.apply_control(
            &runtime,
            &operation,
            RuntimeChange {
                mode: Some(mode),
                ..RuntimeChange::default()
            },
        )
        .await?;
        // §2's mode word is a promise about behaviour, so the row records the mode only once the
        // session is actually honouring it — which, on a harness that needs a restart, is after
        // that restart succeeded.
        self.update_settings(&runtime, &operation, Some(mode), None)
            .await?;
        Ok(ResponseBody::AgentAck)
    }

    /// Handles `AgentSetModel`.
    pub async fn set_model(
        &self,
        thread: ThreadId,
        model: ModelSelection,
    ) -> Result<ResponseBody, ProtoError> {
        self.refuse_if_mirrored(thread, "set model").await?;
        let runtime = self.runtime(thread).await?;
        let operation = runtime.operation.lock().await;
        let mut provider_slot = runtime.provider.lock().await;
        let provider = provider_slot
            .as_mut()
            .ok_or_else(|| conflict(format!("agent thread {thread} is not live")))?;
        if provider.capabilities().model_switch == ControlCost::NotSupported {
            return Err(conflict(format!(
                "{} does not support changing model",
                provider.kind().display_name()
            )));
        }
        drop(provider_slot);
        self.apply_control(
            &runtime,
            &operation,
            RuntimeChange {
                model: Some(model.clone()),
                ..RuntimeChange::default()
            },
        )
        .await?;
        self.update_settings(&runtime, &operation, None, Some(model))
            .await?;
        Ok(ResponseBody::AgentAck)
    }

    /// Handles `AgentAccountLogin`.
    ///
    /// Refused on a mirror rather than forwarded: the sign-in Codex starts is a loopback callback
    /// on the **owner** host, so a browser opened here would come back to the wrong machine.
    pub async fn account_login(&self, thread: ThreadId) -> Result<ResponseBody, ProtoError> {
        self.refuse_if_mirrored(thread, "sign in").await?;
        match self.account(thread, AccountOp::Login).await? {
            AccountOutcome::Browser { auth_url } => {
                Ok(ResponseBody::AgentAccountLogin { auth_url })
            }
            // A harness that signed in without a browser has nothing for the caller to open, and
            // an empty URL would be a link to nowhere.
            AccountOutcome::Settled => Ok(ResponseBody::AgentAck),
        }
    }

    /// Handles `AgentAccountLogout`.
    pub async fn account_logout(&self, thread: ThreadId) -> Result<ResponseBody, ProtoError> {
        self.refuse_if_mirrored(thread, "sign out").await?;
        self.account(thread, AccountOp::Logout).await?;
        Ok(ResponseBody::AgentAck)
    }

    /// The shared half of both account verbs: the live provider, under the operation gate.
    ///
    /// The gate is held for the same reason `set_model` holds it — the call talks to the harness
    /// process — and released before the response is built.
    async fn account(&self, thread: ThreadId, op: AccountOp) -> Result<AccountOutcome, ProtoError> {
        let runtime = self.runtime(thread).await?;
        let _operation = runtime.operation.lock().await;
        let mut provider_slot = runtime.provider.lock().await;
        let provider = provider_slot
            .as_mut()
            .ok_or_else(|| conflict(format!("agent thread {thread} is not live")))?;
        provider.account(op).await.map_err(provider_error)
    }

    /// Handles `AgentMarkSeen` from a peer with no stable identity.
    ///
    /// Compatibility peers keep the historical validate-only behaviour. A negotiated connection
    /// calls [`Self::mark_seen_for`] with its Hello identity before dispatch reaches this method.
    pub async fn mark_seen(&self, thread: ThreadId, seq: Seq) -> Result<ResponseBody, ProtoError> {
        self.mark_seen_for(None, thread, seq).await
    }

    /// Validates and monotonically persists one installation's read cursor.
    pub async fn mark_seen_for(
        &self,
        client_id: Option<String>,
        thread: ThreadId,
        seq: Seq,
    ) -> Result<ResponseBody, ProtoError> {
        let runtime = self.runtime(thread).await?;
        let _operation = runtime.operation.lock().await;
        let last_seq = {
            let state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.projection.last_seq
        };
        if seq > last_seq {
            return Err(validation(format!(
                "cannot mark unseen sequence {seq}; latest is {last_seq}"
            )));
        }
        if let Some(client_id) = client_id {
            self.inner
                .store()
                .map_err(storage_error)?
                .mark_seen(client_id, thread, seq, Utc::now().timestamp_millis())
                .await
                .map_err(storage_error)?;
        }
        Ok(ResponseBody::AgentAck)
    }

    /// Reads one installation's cursor for a window response.
    pub async fn seen_seq(
        &self,
        client_id: String,
        thread: ThreadId,
    ) -> Result<Option<Seq>, ProtoError> {
        self.inner
            .store()
            .map_err(storage_error)?
            .seen_seq(client_id, thread)
            .await
            .map_err(storage_error)
    }

    /// Reads the one-shot post-Hello cursor census for an installation.
    pub async fn seen_cursors(
        &self,
        client_id: String,
    ) -> Result<Vec<AgentSeenCursor>, ProtoError> {
        self.inner
            .store()
            .map_err(storage_error)?
            .seen_cursors(client_id)
            .await
            .map_err(storage_error)
    }

    /// Persists one installation's closed marker after validating the thread exists.
    pub async fn mark_closed_for(
        &self,
        client_id: String,
        thread: ThreadId,
    ) -> Result<(), ProtoError> {
        self.ensure_thread_exists(thread).await?;
        self.inner
            .store()
            .map_err(storage_error)?
            .mark_closed(client_id, thread, Utc::now().timestamp_millis())
            .await
            .map_err(storage_error)
    }

    /// Clears one installation's closed marker after validating the thread exists.
    pub async fn clear_closed_for(
        &self,
        client_id: String,
        thread: ThreadId,
    ) -> Result<(), ProtoError> {
        self.ensure_thread_exists(thread).await?;
        self.inner
            .store()
            .map_err(storage_error)?
            .clear_closed(client_id, thread)
            .await
            .map_err(storage_error)
    }

    /// Reads the one-shot post-Hello closed-thread census for an installation.
    pub async fn closed_threads(&self, client_id: String) -> Result<Vec<ThreadId>, ProtoError> {
        self.inner
            .store()
            .map_err(storage_error)?
            .closed_threads(client_id)
            .await
            .map_err(storage_error)
    }

    /// Handles `AgentStop`.
    pub async fn stop(&self, thread: ThreadId) -> Result<ResponseBody, ProtoError> {
        self.refuse_if_mirrored(thread, "stop").await?;
        let runtime = self.runtime(thread).await?;
        let operation = runtime.operation.lock().await;
        let mut provider_slot = runtime.provider.lock().await;
        let provider = provider_slot.take();
        runtime.invalidate_provider();
        // A provider that will not die is a diagnostic, not a reason to leave the transcript
        // claiming a turn is still running: the child that exits from its own stdin close
        // answers `stop` with `Exited`, and returning here skipped every settlement below, so
        // §2's tab spun on a dead process until the daemon restarted. The handle is dropped
        // either way — nothing can reach that session again.
        if let Some(mut provider) = provider {
            if let Err(error) = provider.stop().await {
                tracing::warn!(
                    target: "fleet::agents",
                    %error,
                    %thread,
                    "the native-agent provider did not stop cleanly",
                );
            }
            drop(provider);
        }
        drop(provider_slot);

        for applied in self.settle_open_gates(&runtime, &operation).await? {
            publish_applied(&self.inner, &runtime, applied);
        }
        if let Some(turn) = runtime_inflight(&runtime) {
            for applied in self
                .flush_pending_inputs(&runtime, &operation, turn, "agent_stop")
                .await?
            {
                publish_applied(&self.inner, &runtime, applied);
            }
            self.apply_one(
                &runtime,
                &operation,
                AgentEvent::TurnAborted {
                    turn,
                    reason: AbortReason::SessionStopped,
                },
                Some("agent_stop".to_owned()),
            )
            .await?;
        }
        self.apply_one(
            &runtime,
            &operation,
            AgentEvent::SessionExited {
                code: None,
                expected: true,
            },
            Some("agent_stop".to_owned()),
        )
        .await?;
        runtime.abort_task();
        Ok(ResponseBody::AgentAck)
    }
}

/// The version-6 unbounded answer, or the typed refusal when it would not survive a frame.
///
/// Past the ceiling the snapshot is not slow, it is *undecodable*: `serde_json` fails on the
/// peer and the thread becomes permanently unopenable for it. The refusal names the capability
/// that fixes it instead, which is a sentence a user can act on.
fn snapshot_response(
    projection: ThreadProjection,
    events_after: Vec<fleet_core::agents::SeqEvent>,
) -> Result<ResponseBody, ProtoError> {
    let thread = projection.thread;
    let body = ResponseBody::AgentThreadSnapshot {
        projection,
        events_after,
    };
    let bytes = wire_bytes(&body).map_err(|error| storage_error(anyhow::anyhow!(error)))?;
    if snapshot_fits(bytes) {
        Ok(body)
    } else {
        Err(snapshot_ceiling_error(thread, bytes))
    }
}

/// Resolves a requested model selection against the harness's configured defaults.
///
/// Two sentinels meet here and they mean different things. A selection of `None` is "no opinion
/// at all", and becomes the configured default model and effort, or nothing when neither is
/// configured. A `Some` whose `model` is empty is "this effort, whatever model the harness would
/// have used" — the shape `fleet subagent run --effort high` produces with no `--model` — and
/// keeps the caller's effort while borrowing only the model from the defaults. Blank-but-present
/// models are normalised to the empty string so that every adapter can test `is_empty()` alone.
fn resolve_model_selection(
    model: Option<ModelSelection>,
    defaults: &fleet_core::config::NativeAgentDefaults,
) -> Option<ModelSelection> {
    match model {
        Some(mut selection) => {
            if selection.effort.is_none() {
                selection.effort.clone_from(&defaults.effort);
            }
            if selection.model.trim().is_empty() {
                match defaults.model.as_ref() {
                    Some(model) => selection.model.clone_from(model),
                    None => selection.model.clear(),
                }
            }
            Some(selection)
        }
        None => defaults.model.as_ref().map(|model| ModelSelection {
            model: model.clone(),
            effort: defaults.effort.clone(),
            provider: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use fleet_core::config::NativeAgentDefaults;

    use super::{ModelSelection, resolve_model_selection};

    fn defaults(model: Option<&str>, effort: Option<&str>) -> NativeAgentDefaults {
        NativeAgentDefaults {
            model: model.map(str::to_owned),
            effort: effort.map(str::to_owned),
            ..NativeAgentDefaults::default()
        }
    }

    fn selection(model: &str, effort: Option<&str>) -> ModelSelection {
        ModelSelection {
            model: model.to_owned(),
            effort: effort.map(str::to_owned),
            provider: None,
        }
    }

    /// The four shapes `--model` / `--effort` can arrive in, against configured defaults.
    #[test]
    fn a_model_selection_resolves_against_the_configured_defaults() {
        let defaults = defaults(Some("gpt-5"), Some("medium"));
        // No opinion at all: both come from the defaults.
        assert_eq!(
            resolve_model_selection(None, &defaults),
            Some(selection("gpt-5", Some("medium")))
        );
        // An explicit model keeps the default effort.
        assert_eq!(
            resolve_model_selection(Some(selection("opus", None)), &defaults),
            Some(selection("opus", Some("medium")))
        );
        // Both explicit: neither default applies.
        assert_eq!(
            resolve_model_selection(Some(selection("opus", Some("high"))), &defaults),
            Some(selection("opus", Some("high")))
        );
        // Effort only: the caller's effort survives, the model comes from the defaults.
        assert_eq!(
            resolve_model_selection(Some(selection("", Some("high"))), &defaults),
            Some(selection("gpt-5", Some("high")))
        );
    }

    /// Effort only with nothing configured leaves an empty model for the adapters to skip.
    #[test]
    fn an_effort_only_selection_survives_a_harness_with_no_default_model() {
        let resolved = resolve_model_selection(
            Some(selection("   ", Some("high"))),
            &defaults(None, Some("medium")),
        );
        // Blank is normalised to empty so an adapter only ever tests `is_empty()`.
        assert_eq!(resolved, Some(selection("", Some("high"))));
    }
}
