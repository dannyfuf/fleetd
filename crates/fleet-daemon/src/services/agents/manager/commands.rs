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
        AbortReason, AgentEvent, AgentKind, ApprovalPolicy, ControlCost, GateAnswer, GateId,
        ModelSelection, PermissionMode, SandboxPolicy, Seq, StartRequest, SteerSupport, ThreadId,
        ThreadProjection, TurnId, TurnState, UserInput,
    },
    ids::WorktreeId,
};
use fleet_proto::{
    agents::{snapshot_ceiling_error, snapshot_fits, wire_bytes},
    error::{ErrorKind, ProtoError},
    event::Event,
    request::RequestBody,
    response::ResponseBody,
};

use crate::agents::harness::RuntimeChange;

use super::{
    AgentSessionManager, AgentThreadRecord, ThreadRuntime,
    apply::{publish_applied, runtime_inflight},
    conflict, daemon_error, hydrate, not_found, provider_error, provider_factory_error,
    storage_error, validation,
    window::OpenRequest,
};

impl AgentSessionManager {
    /// Handles `AgentThreadCreate`.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        worktree: WorktreeId,
        provider_kind: AgentKind,
        model: Option<ModelSelection>,
        mode: PermissionMode,
        resume_cursor: Option<String>,
        title: Option<String>,
    ) -> Result<ResponseBody, ProtoError> {
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
        let thread = ThreadId::new();
        let request = StartRequest {
            thread,
            worktree_path: path,
            provider: provider_kind,
            model: model.clone(),
            mode,
            resume_cursor: resume_cursor.clone(),
            fork: false,
            env: BTreeMap::new(),
            sandbox: SandboxPolicy::default(),
            approval_policy: ApprovalPolicy::default(),
            permission_profile: None,
            title: title.clone(),
        };
        let commands = self.agent_commands().await;
        let mut provider = (self.inner.provider_factory)(provider_kind, &request, &commands)
            .map_err(provider_factory_error)?;
        provider.start(request).await.map_err(provider_error)?;
        let provider_events = provider.events();
        let created = Utc::now();
        let resolved_title = title.unwrap_or_else(|| provider_kind.display_name().to_owned());
        let record = AgentThreadRecord {
            thread,
            worktree: worktree.clone(),
            provider: provider_kind,
            title: resolved_title.clone(),
            created,
            last_activity: created,
            resume_cursor,
            model: model.clone(),
            mode,
            last_outcome: None,
        };
        let mut projection = ThreadProjection::new(thread, worktree, provider_kind);
        projection.title = resolved_title;
        projection.model = model;
        projection.mode = mode;
        let runtime = ThreadRuntime::new(projection, record.clone(), None);
        *runtime.provider.lock().await = Some(provider);

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
        self.spawn_event_task(runtime.clone(), provider_events);
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
        Ok(ResponseBody::AgentAck)
    }

    /// Handles `AgentSend`.
    pub async fn send(
        &self,
        thread: ThreadId,
        input: UserInput,
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
        let turn = submitted.turn;
        {
            let mut state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.inflight_turn = Some(turn);
            if !submitted.queued {
                state.pending_inputs.push_back((turn, input.clone()));
            }
        }
        if submitted.queued {
            // A steer: the turn is already running, so there is no announcement to wait for and
            // the bubble is recorded now, marked as having joined it (§7.2).
            let item = input.item.unwrap_or_default();
            self.record_user_input(&runtime, &operation, turn, item, input, true)
                .await?;
        }
        Ok(ResponseBody::AgentAck)
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

    /// Handles `AgentMarkSeen`.
    ///
    /// **Validates the cursor and records nothing**, which is not what §3.3 asks for: the design
    /// wants `last_seen_seq` persisted daemon-side *per client*, and phase 2's `seen` table is
    /// waiting for it. Two things are missing before it can be, and neither is guessable —
    /// a stable client identity on the handshake to key the row by, and a field on a read
    /// response to hand a client its own cursor back. Until then the cursor lives in client
    /// memory: the reading client narrows the broadcast attention with
    /// `AgentThreadSummary::attention_for` against what it holds, and the amber dot returns
    /// after an app restart. `docs/NATIVE-AGENTS.md` §13 phase 4 records it as owed.
    ///
    /// What the daemon must *not* do is fold a cursor into [`Event::AgentSummary`]: one summary
    /// reaches every subscriber, so one client's read would clear the dot on all the others.
    pub async fn mark_seen(&self, thread: ThreadId, seq: Seq) -> Result<ResponseBody, ProtoError> {
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
        Ok(ResponseBody::AgentAck)
    }

    /// Handles `AgentStop`.
    pub async fn stop(&self, thread: ThreadId) -> Result<ResponseBody, ProtoError> {
        self.refuse_if_mirrored(thread, "stop").await?;
        let runtime = self.runtime(thread).await?;
        let operation = runtime.operation.lock().await;
        let mut provider_slot = runtime.provider.lock().await;
        let Some(mut provider) = provider_slot.take() else {
            return Ok(ResponseBody::AgentAck);
        };
        // A provider that will not die is a diagnostic, not a reason to leave the transcript
        // claiming a turn is still running: the child that exits from its own stdin close
        // answers `stop` with `Exited`, and returning here skipped every settlement below, so
        // §2's tab spun on a dead process until the daemon restarted. The handle is dropped
        // either way — nothing can reach that session again.
        if let Err(error) = provider.stop().await {
            tracing::warn!(
                target: "fleet::agents",
                %error,
                %thread,
                "the native-agent provider did not stop cleanly",
            );
        }
        drop(provider);
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
