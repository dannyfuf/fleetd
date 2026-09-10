//! Serialized native-agent thread lifecycle manager.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::Context;
use chrono::Utc;
use fleet_core::{
    agents::{
        AbortReason, AgentEvent, AgentKind, AgentThreadSummary, CheckpointKind, GateAnswer, GateId,
        GateKind, GateResolver, ItemId, ItemKind, ItemStatus, ModelSelection, PermissionChoice,
        PermissionMode, PlanAnswer, Seq, SeqEvent, SessionState, StartRequest, ThreadId,
        ThreadProjection, TurnId, TurnOutcome, TurnState, UserInput,
    },
    config::AgentCommands,
    ids::{HostId, WorktreeId},
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    event::Event,
    response::ResponseBody,
};

use crate::{server::BroadcastBus, services::worktrees::Worktrees, stores::config::ConfigStore};

use super::{
    AgentIndex, AgentStore, AgentThreadRecord,
    providers::{AgentProvider, ProviderError, ProviderEvents, spawn_provider},
    thread::{AppliedEvent, ThreadRuntime, coalesce_deltas},
};

const DELTA_TICK: Duration = Duration::from_millis(16);

type ProviderFactory = dyn Fn(AgentKind, &StartRequest, &AgentCommands) -> anyhow::Result<Box<dyn AgentProvider>>
    + Send
    + Sync;
type RemoteHostResolver = dyn Fn(&WorktreeId) -> Option<HostId> + Send + Sync;

struct ManagerInner {
    store: AgentStore,
    events: BroadcastBus,
    worktrees: Worktrees,
    config: Option<Arc<ConfigStore>>,
    threads: RwLock<HashMap<ThreadId, ThreadRuntime>>,
    index: std::sync::Mutex<AgentIndex>,
    provider_factory: Arc<ProviderFactory>,
    remote_host_resolver: RwLock<Option<Arc<RemoteHostResolver>>>,
}

/// Owns native-agent threads, providers, sequencing, projections, and client seen cursors.
#[derive(Clone)]
pub struct AgentSessionManager {
    inner: Arc<ManagerInner>,
}

impl AgentSessionManager {
    /// Creates a manager around the agent store, daemon event bus, worktree path lookup, and
    /// the configuration that names each provider's executable.
    #[must_use]
    pub fn new(
        store: AgentStore,
        events: BroadcastBus,
        worktrees: Worktrees,
        config: Arc<ConfigStore>,
    ) -> Self {
        let manager = Self::new_with_factory(
            store,
            events,
            worktrees,
            Some(config),
            Arc::new(spawn_provider),
        );
        manager.set_remote_host_resolver(Arc::new(|_| None));
        manager
    }

    fn new_with_factory(
        store: AgentStore,
        events: BroadcastBus,
        worktrees: Worktrees,
        config: Option<Arc<ConfigStore>>,
        provider_factory: Arc<ProviderFactory>,
    ) -> Self {
        let mut index = match store.read_index() {
            Ok(index) => index,
            Err(error) => {
                tracing::warn!(%error, "could not load native-agent index");
                AgentIndex::default()
            }
        };
        let mut threads = HashMap::new();
        let mut recovered = false;
        for record in &mut index.threads {
            match load_projection(&store, record) {
                Ok(mut projection) => {
                    if orphaned(&projection) {
                        recovered = true;
                        recover_orphan(&store, record, &mut projection);
                    }
                    threads.insert(
                        record.thread,
                        ThreadRuntime::new(projection, record.clone()),
                    );
                }
                Err(error) => {
                    tracing::warn!(thread = %record.thread, %error, "could not replay native-agent thread");
                }
            }
        }
        if recovered && let Err(error) = store.write_index(&index) {
            tracing::warn!(%error, "could not persist native-agent restart recovery");
        }
        Self {
            inner: Arc::new(ManagerInner {
                store,
                events,
                worktrees,
                config,
                threads: RwLock::new(threads),
                index: std::sync::Mutex::new(index),
                provider_factory,
                remote_host_resolver: RwLock::new(None),
            }),
        }
    }

    /// Installs the router's remote-worktree ownership lookup.
    ///
    /// Routing normally prevents a remote create request from reaching this local manager. This
    /// second guard makes that invariant explicit and, crucially, runs before local path lookup,
    /// provider startup, or any write under the local agent store.
    pub(crate) fn set_remote_host_resolver(&self, resolver: Arc<RemoteHostResolver>) {
        *self
            .inner
            .remote_host_resolver
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(resolver);
    }

    /// The configured provider command lines, falling back to the packaged defaults.
    ///
    /// A configuration that cannot be read must not block a thread from starting: the default
    /// executables are what the user would have gotten anyway, and the launch failure the
    /// adapter reports next is more actionable than a config error here.
    async fn agent_commands(&self) -> AgentCommands {
        let Some(config) = self.inner.config.as_ref() else {
            return default_agent_commands();
        };
        match config.load().await {
            Ok(config) => config.agent_commands,
            Err(error) => {
                tracing::warn!(%error, "could not read agent commands; using defaults");
                default_agent_commands()
            }
        }
    }

    /// Current summaries for inclusion in the daemon's global snapshot.
    #[must_use]
    pub fn summaries(&self) -> Vec<AgentThreadSummary> {
        let threads = self
            .inner
            .threads
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut summaries = threads
            .values()
            .map(|runtime| {
                let state = runtime
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (
                    state.record.created,
                    state.projection.summary(Seq::default()),
                )
            })
            .collect::<Vec<_>>();
        summaries.sort_by_key(|(created, _)| *created);
        summaries.into_iter().map(|(_, summary)| summary).collect()
    }

    /// Handles `AgentThreadList`.
    pub async fn list(&self) -> Result<ResponseBody, ProtoError> {
        Ok(ResponseBody::AgentThreads(self.summaries()))
    }

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
        let runtime = ThreadRuntime::new(projection, record.clone());
        *runtime.provider.lock().await = Some(provider);

        let index_write_error = {
            let mut index = self
                .inner
                .index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            index.threads.push(record);
            if let Err(error) = self.inner.store.write_index(&index) {
                index.threads.retain(|entry| entry.thread != thread);
                Some(error)
            } else {
                None
            }
        };
        if let Some(error) = index_write_error {
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
                    "could not stop the provider after a failed index write",
                );
            }
            return Err(storage_error(error));
        }
        self.inner
            .threads
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(thread, runtime.clone());
        self.spawn_event_task(runtime, provider_events);
        let runtime = self.runtime(thread)?;
        let state = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let summary = state.projection.summary(Seq::default());
        drop(state);
        self.inner
            .events
            .publish(Event::AgentSummary(summary.clone()));
        Ok(ResponseBody::AgentThreadCreated(summary))
    }

    /// Handles `AgentThreadOpen`.
    pub async fn open(
        &self,
        thread: ThreadId,
        from_seq: Option<Seq>,
    ) -> Result<ResponseBody, ProtoError> {
        let runtime = self.runtime(thread)?;
        // A catch-up open is a client repairing a sequence gap, not a user opening a tab: §6
        // resumes a stopped thread lazily "the next time it is opened", which is a user action.
        // Relaunching a provider process for every mirror that lagged is not.
        if from_seq.is_none() {
            self.resume_if_stopped(&runtime).await?;
        }

        let Some(cursor) = from_seq else {
            let state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            return Ok(ResponseBody::AgentThreadSnapshot {
                projection: state.projection.clone(),
                events_after: Vec::new(),
            });
        };
        let record = {
            let state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if cursor > state.projection.last_seq {
                return Err(validation(format!(
                    "thread {thread} has no sequence {cursor}; latest is {}",
                    state.projection.last_seq
                )));
            }
            state.record.clone()
        };
        // A whole-transcript read is disk work, and a rendering client is waiting on it: it
        // runs on the blocking pool rather than on an executor worker (§3, never block on IO).
        let store = self.inner.store.clone();
        let events = tokio::task::spawn_blocking(move || store.load(thread))
            .await
            .map_err(|error| storage_error(anyhow::anyhow!(error)))?
            .map_err(storage_error)?;
        let mut projection = projection_seed(&record);
        for event in events.iter().filter(|event| event.seq <= cursor) {
            projection.apply(event).map_err(|error| {
                storage_error(anyhow::anyhow!(error).context("replay agent snapshot"))
            })?;
        }
        let events_after = events
            .into_iter()
            .filter(|event| event.seq > cursor)
            .collect();
        Ok(ResponseBody::AgentThreadSnapshot {
            projection,
            events_after,
        })
    }

    /// Handles `AgentThreadClose`.
    pub async fn close(&self, thread: ThreadId) -> Result<ResponseBody, ProtoError> {
        let _runtime = self.runtime(thread)?;
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
        let runtime = self.runtime(thread)?;
        // §6 resumes a stopped thread lazily; §7 makes `AgentSend` a first-class verb of the
        // same request set the app uses. Taken together, a send to a thread whose provider a
        // restart took away resumes it rather than refusing until something else opens the tab.
        // Resuming takes the operation lock itself, so it happens before this one does.
        self.resume_if_stopped(&runtime).await?;
        let _operation = runtime.operation.lock().await;
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
        let mut provider_slot = runtime.provider.lock().await;
        let provider = provider_slot
            .as_mut()
            .ok_or_else(|| conflict(format!("agent thread {thread} is not live")))?;
        if (projected_running || had_inflight) && !provider.capabilities().steer {
            return Err(conflict(format!(
                "{} does not support steering",
                kind.display_name()
            )));
        }
        provider
            .send(turn, input.clone())
            .await
            .map_err(provider_error)?;
        drop(provider_slot);
        {
            let mut state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.inflight_turn = Some(turn);
            if kind == AgentKind::Claude && !projected_running {
                state.pending_claude_inputs.push_back((turn, input.clone()));
            }
        }
        if kind == AgentKind::Claude && projected_running {
            self.record_user_input_locked(&runtime, turn, ItemId::new(), input)?;
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
        let runtime = self.runtime(thread)?;
        let _operation = runtime.operation.lock().await;
        let Some(turn) = runtime_inflight(&runtime) else {
            return Ok(ResponseBody::AgentAck);
        };
        let mut provider_slot = runtime.provider.lock().await;
        let provider = provider_slot
            .as_mut()
            .ok_or_else(|| conflict(format!("agent thread {thread} is not live")))?;
        if !provider.capabilities().interrupt {
            return Err(conflict(format!(
                "{} does not support interruption",
                provider.kind().display_name()
            )));
        }
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
        let runtime = self.runtime(thread)?;
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
        let runtime = self.runtime(thread)?;
        let _operation = runtime.operation.lock().await;
        let mut provider_slot = runtime.provider.lock().await;
        let provider = provider_slot
            .as_mut()
            .ok_or_else(|| conflict(format!("agent thread {thread} is not live")))?;
        if !provider.capabilities().modes {
            return Err(conflict(format!(
                "{} does not support changing mode",
                provider.kind().display_name()
            )));
        }
        // §2's mode word is a promise about behaviour, so the row records the mode the session
        // settled on, not the one the palette asked for.
        let effective = provider.set_mode(mode).await.map_err(provider_error)?;
        drop(provider_slot);
        self.update_settings_locked(&runtime, Some(effective), None)?;
        Ok(ResponseBody::AgentAck)
    }

    /// Handles `AgentSetModel`.
    pub async fn set_model(
        &self,
        thread: ThreadId,
        model: ModelSelection,
    ) -> Result<ResponseBody, ProtoError> {
        let runtime = self.runtime(thread)?;
        let _operation = runtime.operation.lock().await;
        let mut provider_slot = runtime.provider.lock().await;
        let provider = provider_slot
            .as_mut()
            .ok_or_else(|| conflict(format!("agent thread {thread} is not live")))?;
        if !provider.capabilities().models {
            return Err(conflict(format!(
                "{} does not support changing model",
                provider.kind().display_name()
            )));
        }
        provider
            .set_model(model.clone())
            .await
            .map_err(provider_error)?;
        drop(provider_slot);
        self.update_settings_locked(&runtime, None, Some(model))?;
        Ok(ResponseBody::AgentAck)
    }

    /// Handles `AgentMarkSeen`.
    ///
    /// §3.3 makes the seen cursor "per client, not persisted daemon-side", and one
    /// [`Event::AgentSummary`] reaches every subscriber: a cursor folded into it here would be
    /// one client's and would clear the amber dot on all the others. So the daemon validates
    /// the cursor and keeps none — the reading client narrows the broadcast attention with
    /// `AgentThreadSummary::attention_for` against the cursor it holds itself.
    pub async fn mark_seen(&self, thread: ThreadId, seq: Seq) -> Result<ResponseBody, ProtoError> {
        let runtime = self.runtime(thread)?;
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
        let runtime = self.runtime(thread)?;
        let _operation = runtime.operation.lock().await;
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

        for applied in self.settle_open_gates_locked(&runtime)? {
            publish_applied(&self.inner, &runtime, applied);
        }
        if let Some(turn) = runtime_inflight(&runtime) {
            for applied in self.flush_pending_claude_inputs_locked(&runtime, turn, "agent_stop")? {
                publish_applied(&self.inner, &runtime, applied);
            }
            self.apply_one_locked(
                &runtime,
                AgentEvent::TurnAborted {
                    turn,
                    reason: AbortReason::SessionStopped,
                },
                Some("agent_stop".to_owned()),
            )?;
        }
        self.apply_one_locked(
            &runtime,
            AgentEvent::SessionExited {
                code: None,
                expected: true,
            },
            Some("agent_stop".to_owned()),
        )?;
        runtime.abort_task();
        Ok(ResponseBody::AgentAck)
    }

    fn runtime(&self, thread: ThreadId) -> Result<ThreadRuntime, ProtoError> {
        self.inner
            .threads
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&thread)
            .cloned()
            .ok_or_else(|| not_found(format!("agent thread {thread}")))
    }

    /// Resumes a thread whose provider is gone but whose cursor can bring it back.
    ///
    /// A thread with no cursor, or one that already has a provider, is left exactly as it is.
    async fn resume_if_stopped(&self, runtime: &ThreadRuntime) -> Result<(), ProtoError> {
        let resumable = {
            let state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.record.resume_cursor.is_some()
                && matches!(
                    state.projection.session,
                    SessionState::Stopped | SessionState::Error | SessionState::Starting
                )
        };
        if resumable && runtime.provider.lock().await.is_none() {
            self.resume_runtime(runtime).await?;
        }
        Ok(())
    }

    async fn resume_runtime(&self, runtime: &ThreadRuntime) -> Result<(), ProtoError> {
        let _operation = runtime.operation.lock().await;
        if runtime.provider.lock().await.is_some() {
            return Ok(());
        }
        let (record, age_ms) = {
            let state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let age_ms = Utc::now()
                .signed_duration_since(state.record.last_activity)
                .num_milliseconds()
                .max(0) as u64;
            (state.record.clone(), age_ms)
        };
        let cursor = record
            .resume_cursor
            .clone()
            .ok_or_else(|| conflict("agent thread has no resume cursor"))?;
        let path = self
            .inner
            .worktrees
            .path(record.worktree.clone())
            .await
            .map(PathBuf::from)
            .map_err(daemon_error)?;
        let request = StartRequest {
            thread: record.thread,
            worktree_path: path,
            provider: record.provider,
            model: record.model,
            mode: record.mode,
            resume_cursor: Some(cursor),
            title: Some(record.title),
        };
        let commands = self.agent_commands().await;
        let mut provider = (self.inner.provider_factory)(request.provider, &request, &commands)
            .map_err(provider_factory_error)?;
        provider.start(request).await.map_err(provider_error)?;
        let provider_events = provider.events();
        // The provider process is up; `claude -p` only emits `system/init` once it is prompted,
        // so waiting for that would leave a resumed tab on a state §3.3 has no row for. The
        // resume checkpoint below is the visible marker instead.
        self.apply_one_locked(
            runtime,
            AgentEvent::SessionStateChanged(SessionState::Ready),
            Some("resume".to_owned()),
        )?;
        self.apply_one_locked(
            runtime,
            AgentEvent::Checkpoint(CheckpointKind::Resumed { age_ms }),
            Some("resume".to_owned()),
        )?;
        *runtime.provider.lock().await = Some(provider);
        self.spawn_event_task(runtime.clone(), provider_events);
        Ok(())
    }

    fn spawn_event_task(&self, runtime: ThreadRuntime, events: ProviderEvents) {
        let inner = Arc::clone(&self.inner);
        let runtime_for_task = runtime.clone();
        let task = tokio::spawn(async move {
            run_provider_events(inner, runtime_for_task, events).await;
        });
        runtime.set_task(task.abort_handle());
    }

    fn apply_provider_event_locked(
        &self,
        runtime: &ThreadRuntime,
        event: AgentEvent,
        raw: Option<String>,
    ) -> Result<Vec<AppliedEvent>, ProtoError> {
        let turn_started = match &event {
            AgentEvent::TurnStarted { turn, user_item } => Some((*turn, *user_item)),
            _ => None,
        };
        // §3.3 rule 4 closes a gate only on `GateResolved`, so a session that ends with one open
        // would strand the card at the top of `NeedsYou` for the life of the thread, with no
        // adapter left to answer it. The settlement is appended, exactly like `control_cancel`.
        let mut applied = if ends_the_session(&event) {
            self.settle_open_gates_locked(runtime)?
        } else {
            Vec::new()
        };
        // The turn is about to be cleared, so this is the last event that can still carry the
        // prompts Claude was given under it. Without this a turn whose `TurnStarted` never
        // arrived strands them, and the next turn's drain would find them ahead of its own.
        if ends_the_turn(&event)
            && let Some(turn) = pending_claude_turn(runtime)
        {
            applied.extend(self.flush_pending_claude_inputs_locked(
                runtime,
                turn,
                "pending_input",
            )?);
        }
        applied.push(self.apply_event_locked(runtime, event, raw)?);
        if let Some((turn, user_item)) = turn_started {
            let provider = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .record
                .provider;
            if provider == AgentKind::Claude {
                applied.extend(self.drain_claude_inputs_applied_locked(runtime, turn, user_item)?);
            }
        }
        Ok(applied)
    }

    /// Records the prompts Claude was already given whose `TurnStarted` never arrived.
    ///
    /// §6 makes the log the transcript, and those prompts are already on Claude's stdin: the
    /// last moment to write them down is the event that settles the turn they were sent under.
    /// Dropping them instead would make the transcript lie the other way — the model answering
    /// a question no row shows. A projection that is already running a turn drained the deque
    /// when that turn started, and the reducer refuses a second `TurnStarted` anyway, so there
    /// is nothing to do.
    fn flush_pending_claude_inputs_locked(
        &self,
        runtime: &ThreadRuntime,
        turn: TurnId,
        raw: &str,
    ) -> Result<Vec<AppliedEvent>, ProtoError> {
        if matches!(
            runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .projection
                .turn,
            TurnState::Running(_)
        ) {
            return Ok(Vec::new());
        }
        let user_item = ItemId::new();
        let mut applied = vec![self.apply_event_locked(
            runtime,
            AgentEvent::TurnStarted { turn, user_item },
            Some(raw.to_owned()),
        )?];
        applied.extend(self.drain_claude_inputs_applied_locked(runtime, turn, user_item)?);
        Ok(applied)
    }

    fn drain_claude_inputs_applied_locked(
        &self,
        runtime: &ThreadRuntime,
        turn: TurnId,
        first_item: ItemId,
    ) -> Result<Vec<AppliedEvent>, ProtoError> {
        let inputs = {
            let mut state = runtime
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Keyed by turn, not by position: an entry whose `TurnStarted` never arrived must
            // not shadow the ones behind it, or every later prompt is written to Claude and
            // never recorded, and the deque grows for the life of the daemon.
            let mut inputs = Vec::new();
            let mut kept = VecDeque::new();
            for (pending_turn, input) in std::mem::take(&mut state.pending_claude_inputs) {
                if pending_turn == turn {
                    inputs.push(input);
                } else {
                    kept.push_back((pending_turn, input));
                }
            }
            state.pending_claude_inputs = kept;
            inputs
        };
        let mut applied = Vec::new();
        for (index, input) in inputs.into_iter().enumerate() {
            let item = if index == 0 {
                first_item
            } else {
                ItemId::new()
            };
            applied.push(self.apply_event_locked(
                runtime,
                user_item_started(turn, item, input),
                None,
            )?);
            applied.push(self.apply_event_locked(
                runtime,
                AgentEvent::ItemCompleted {
                    item,
                    status: ItemStatus::Done,
                },
                None,
            )?);
        }
        Ok(applied)
    }

    fn record_user_input_locked(
        &self,
        runtime: &ThreadRuntime,
        turn: TurnId,
        item: ItemId,
        input: UserInput,
    ) -> Result<(), ProtoError> {
        self.apply_one_locked(runtime, user_item_started(turn, item, input), None)?;
        self.apply_one_locked(
            runtime,
            AgentEvent::ItemCompleted {
                item,
                status: ItemStatus::Done,
            },
            None,
        )
    }

    /// Closes every gate still open, as the appended events §6 requires.
    fn settle_open_gates_locked(
        &self,
        runtime: &ThreadRuntime,
    ) -> Result<Vec<AppliedEvent>, ProtoError> {
        let open = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .projection
            .gates
            .clone();
        open.into_iter()
            .map(|gate| {
                self.apply_event_locked(
                    runtime,
                    AgentEvent::GateResolved {
                        gate: gate.id,
                        answer: closed_gate_answer(&gate.kind),
                        by: GateResolver::ProviderClosed,
                    },
                    Some("provider_closed".to_owned()),
                )
            })
            .collect()
    }

    fn apply_one_locked(
        &self,
        runtime: &ThreadRuntime,
        event: AgentEvent,
        raw: Option<String>,
    ) -> Result<(), ProtoError> {
        let applied = self.apply_event_locked(runtime, event, raw)?;
        publish_applied(&self.inner, runtime, applied);
        Ok(())
    }

    fn apply_event_locked(
        &self,
        runtime: &ThreadRuntime,
        event: AgentEvent,
        raw: Option<String>,
    ) -> Result<AppliedEvent, ProtoError> {
        apply_event(&self.inner, runtime, event, raw).map_err(storage_error)
    }

    /// Records a mode or model change as a durable event rather than a silent projection edit.
    ///
    /// §3 makes the reducer the only writer of projected state and §6 makes the log the
    /// transcript, so a client mirror learns the new model from the same stream as everything
    /// else instead of keeping the old one until its tab is re-opened.
    fn update_settings_locked(
        &self,
        runtime: &ThreadRuntime,
        mode: Option<PermissionMode>,
        model: Option<ModelSelection>,
    ) -> Result<(), ProtoError> {
        self.apply_one_locked(
            runtime,
            AgentEvent::MetadataChanged {
                title: None,
                mode,
                model,
            },
            None,
        )
    }
}

async fn run_provider_events(
    inner: Arc<ManagerInner>,
    runtime: ThreadRuntime,
    mut receiver: ProviderEvents,
) {
    let manager = AgentSessionManager {
        inner: Arc::clone(&inner),
    };
    while let Some(first) = receiver.recv().await {
        let mut batch = vec![first];
        if matches!(batch[0].event, AgentEvent::ContentDelta { .. }) {
            tokio::time::sleep(DELTA_TICK).await;
            while let Ok(event) = receiver.try_recv() {
                batch.push(event);
            }
        }
        // §5: one event per item per tick. The run is merged *before* it is reduced, so the tick
        // costs one sequence, one stored line and one broadcast frame rather than one per token.
        for event in coalesce_deltas(batch) {
            let _operation = runtime.operation.lock().await;
            let exited = matches!(event.event, AgentEvent::SessionExited { .. });
            match manager.apply_provider_event_locked(&runtime, event.event, event.raw) {
                Ok(events) => {
                    for applied in events {
                        publish_applied(&inner, &runtime, applied);
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "dropped invalid native-agent provider event");
                }
            }
            // §6 resumes a stopped thread "lazily with a new adapter the next time it is
            // opened", and `resume_if_stopped` only does that when the slot is empty. Waiting
            // for the event channel to close never empties it: the adapter owns a sender for
            // its whole life, so a crashed child would leave its dead adapter — and its stale
            // turn bookkeeping — rejecting every later send until the daemon restarted. The
            // session is over the moment it says so, so the adapter goes with it.
            if exited {
                drop(runtime.provider.lock().await.take());
            }
        }
    }

    let _operation = runtime.operation.lock().await;
    let should_mark_exit = {
        let state = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !matches!(
            state.projection.session,
            SessionState::Stopped | SessionState::Error
        )
    };
    if should_mark_exit {
        match manager.apply_provider_event_locked(
            &runtime,
            AgentEvent::SessionExited {
                code: None,
                expected: false,
            },
            Some("provider_event_stream_closed".to_owned()),
        ) {
            Ok(events) => {
                for applied in events {
                    publish_applied(&inner, &runtime, applied);
                }
            }
            Err(error) => tracing::warn!(%error, "could not record provider event-stream exit"),
        }
    }
    *runtime.provider.lock().await = None;
}

fn apply_event(
    inner: &ManagerInner,
    runtime: &ThreadRuntime,
    event: AgentEvent,
    raw: Option<String>,
) -> anyhow::Result<AppliedEvent> {
    let (applied, record, persist_index) = {
        let mut state = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = state.projection.summary(Seq::default());
        let sequenced = SeqEvent {
            seq: state.projection.last_seq.next(),
            at: Utc::now(),
            raw: raw.or_else(|| Some(event_name(&event).to_owned())),
            event,
        };
        // §6 makes the transcript what the reducer wrote *before* the event was broadcast, so
        // memory, log and broadcast must not diverge. The event is settled against the
        // projection first, made durable second, and only then applied: a write that fails
        // therefore leaves `last_seq` where it was, instead of advancing memory past a sequence
        // the log never received and making every later append discontinuous on disk.
        state
            .projection
            .accepts(&sequenced)
            .with_context(|| format!("apply native-agent event {}", sequenced.seq))?;
        inner
            .store
            .append(state.record.thread, &sequenced)
            .context("persist native-agent event")?;
        state
            .projection
            .apply(&sequenced)
            .with_context(|| format!("apply native-agent event {}", sequenced.seq))?;
        // Reborrowed so `record` and `title` are two disjoint field borrows: taking them both
        // through the guard would need a clone of the whole transcript per applied event.
        let state = &mut *state;
        let previous = state.record.clone();
        update_record(&mut state.record, &sequenced, &state.projection.title);
        if ends_the_turn(&sequenced.event) {
            state.inflight_turn = None;
        }
        // The gate is settled everywhere now, so the answered-once guard can forget it.
        if let AgentEvent::GateResolved { gate, .. } = &sequenced.event {
            state.answered_gates.remove(gate);
        }
        let after = state.projection.summary(Seq::default());
        let persist_index = index_metadata_changed(&previous, &state.record);
        (
            AppliedEvent {
                event: sequenced,
                summary: summary_transition(&before, &after).then_some(after),
            },
            state.record.clone(),
            persist_index,
        )
    };
    // The index is thread *metadata*, not a second event log: writing it costs two fsyncs and a
    // rename, which a streaming turn must not pay per delta. `last_activity` alone rides along
    // with the next real metadata transition, and every consumer reads the log for the rest.
    replace_index_record(inner, &record);
    if persist_index && let Err(error) = write_index(inner) {
        tracing::warn!(%error, "could not update native-agent index");
    }
    Ok(applied)
}

/// Whether a record changed in a way the durable index has to learn about now.
///
/// `last_activity` moves on every event and is deliberately excluded: it is a listing nicety,
/// and it is persisted anyway by the next transition that matters.
fn index_metadata_changed(before: &AgentThreadRecord, after: &AgentThreadRecord) -> bool {
    before.title != after.title
        || before.resume_cursor != after.resume_cursor
        || before.model != after.model
        || before.mode != after.mode
        || before.last_outcome != after.last_outcome
}

fn publish_applied(inner: &ManagerInner, runtime: &ThreadRuntime, applied: AppliedEvent) {
    let thread = runtime
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record
        .thread;
    inner.events.publish(Event::Agent {
        thread,
        event: applied.event,
    });
    if let Some(summary) = applied.summary {
        inner.events.publish(Event::AgentSummary(summary));
    }
}

fn user_item_started(turn: TurnId, item: ItemId, input: UserInput) -> AgentEvent {
    AgentEvent::ItemStarted {
        turn,
        item,
        kind: ItemKind::UserMessage {
            text: input.text,
            attachments: input.attachments,
        },
        parent: None,
    }
}

fn runtime_inflight(runtime: &ThreadRuntime) -> Option<TurnId> {
    let state = runtime
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match state.projection.turn {
        TurnState::Running(turn) => Some(turn),
        _ => state.inflight_turn,
    }
}

fn update_record(record: &mut AgentThreadRecord, event: &SeqEvent, title: &str) {
    if record.title != title {
        record.title = title.to_owned();
    }
    record.last_activity = event.at;
    match &event.event {
        AgentEvent::SessionStarted {
            resume_cursor,
            model,
            mode,
            ..
        } => {
            if resume_cursor.is_some() {
                record.resume_cursor.clone_from(resume_cursor);
            }
            record.model.clone_from(model);
            record.mode = *mode;
        }
        AgentEvent::MetadataChanged { mode, model, .. } => {
            if let Some(mode) = mode {
                record.mode = *mode;
            }
            if model.is_some() {
                record.model.clone_from(model);
            }
        }
        AgentEvent::TurnCompleted { outcome, .. } => {
            record.last_outcome = Some(outcome.clone());
        }
        AgentEvent::TurnAborted { .. } => {
            record.last_outcome = Some(TurnOutcome::Interrupted);
        }
        AgentEvent::SessionExited {
            expected: false, ..
        }
        | AgentEvent::RuntimeError { fatal: true, .. } => {
            record.last_outcome = Some(TurnOutcome::Error {
                message: Some("provider exited unexpectedly".to_owned()),
            });
        }
        _ => {}
    }
}

fn replace_index_record(inner: &ManagerInner, record: &AgentThreadRecord) {
    let mut index = inner
        .index
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = index
        .threads
        .iter_mut()
        .find(|candidate| candidate.thread == record.thread)
    {
        *existing = record.clone();
    }
}

fn write_index(inner: &ManagerInner) -> anyhow::Result<()> {
    let index = inner
        .index
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    inner.store.write_index(&index)
}

/// Replays a thread's log as far as the reducer accepts it, quarantining anything it does not.
///
/// §6 replays every thread's whole log at start and keeps threads "browsable read-only
/// regardless". One event the reducer rejects — a shape an older daemon wrote, a tail a crash
/// tore — must therefore cost that event and the ones behind it, never the whole thread: a
/// thread that fails to load is absent from `summaries()` and answers `NotFound` for good. The
/// log is trimmed to what replayed, so the next append continues from the sequence the
/// projection actually holds instead of colliding with the events it skipped.
fn load_projection(
    store: &AgentStore,
    record: &AgentThreadRecord,
) -> anyhow::Result<ThreadProjection> {
    let mut projection = projection_seed(record);
    let events = store.load(record.thread)?;
    let logged = events.last().map(|event| event.seq);
    let mut replayed = None;
    for event in &events {
        if let Err(error) = projection.apply(event) {
            tracing::warn!(
                thread = %record.thread,
                seq = %event.seq,
                %error,
                "quarantining a native-agent event the reducer rejected on replay"
            );
            break;
        }
        replayed = Some(event.seq);
    }
    if replayed != logged
        && let Err(error) = store.truncate_after(record.thread, replayed)
    {
        tracing::warn!(thread = %record.thread, %error, "could not trim a native-agent log");
    }
    Ok(projection)
}

fn projection_seed(record: &AgentThreadRecord) -> ThreadProjection {
    let mut projection =
        ThreadProjection::new(record.thread, record.worktree.clone(), record.provider);
    projection.title.clone_from(&record.title);
    projection.model.clone_from(&record.model);
    projection.mode = record.mode;
    projection
}

/// Whether the log leaves this thread claiming a provider the restart already killed.
///
/// §6 settles an orphan explicitly. `Ready` is as much a live-provider state as `Running` is —
/// the child is gone either way — so leaving it out would strand the thread advertising a
/// session it does not have, with no state `open` is willing to resume from.
fn orphaned(projection: &ThreadProjection) -> bool {
    matches!(
        projection.session,
        SessionState::Starting | SessionState::Ready | SessionState::Running
    ) || matches!(projection.turn, TurnState::Running(_))
}

fn recover_orphan(
    store: &AgentStore,
    record: &mut AgentThreadRecord,
    projection: &mut ThreadProjection,
) {
    /// Appends one recovery event in the same order the reducer uses: settle, persist, apply.
    ///
    /// Returns whether the caller may append another one. Applying before persisting would put
    /// `last_seq` ahead of the log, so the next append would leave a hole on disk that the
    /// following start quarantines — taking the whole tail of the transcript with it (§6).
    fn append_recovery(
        store: &AgentStore,
        record: &mut AgentThreadRecord,
        projection: &mut ThreadProjection,
        event: AgentEvent,
    ) -> bool {
        let sequenced = SeqEvent {
            seq: projection.last_seq.next(),
            at: Utc::now(),
            raw: Some("daemon_restart_recovery".to_owned()),
            event,
        };
        if let Err(error) = projection.accepts(&sequenced) {
            tracing::warn!(thread = %record.thread, %error, "could not reduce restart recovery");
            return true;
        }
        if let Err(error) = store.append(record.thread, &sequenced) {
            tracing::warn!(thread = %record.thread, %error, "could not persist restart recovery");
            return false;
        }
        if let Err(error) = projection.apply(&sequenced) {
            tracing::warn!(thread = %record.thread, %error, "could not reduce restart recovery");
            return false;
        }
        update_record(record, &sequenced, &projection.title);
        true
    }

    // §3.3 rule 4 and §6: a gate the restart orphaned is settled explicitly, as appended
    // events. Otherwise the card outlives the adapter that could answer it and the thread stays
    // on the highest-priority `NeedsYou` forever.
    for gate in projection.gates.clone() {
        if !append_recovery(
            store,
            record,
            projection,
            AgentEvent::GateResolved {
                gate: gate.id,
                answer: closed_gate_answer(&gate.kind),
                by: GateResolver::ProviderClosed,
            },
        ) {
            return;
        }
    }
    if record.resume_cursor.is_some() {
        if let TurnState::Running(turn) = projection.turn
            && !append_recovery(
                store,
                record,
                projection,
                AgentEvent::TurnAborted {
                    turn,
                    reason: AbortReason::ProviderExited,
                },
            )
        {
            return;
        }
        append_recovery(
            store,
            record,
            projection,
            AgentEvent::SessionStateChanged(SessionState::Stopped),
        );
    } else {
        append_recovery(
            store,
            record,
            projection,
            AgentEvent::SessionExited {
                code: None,
                expected: false,
            },
        );
    }
}

/// The turn the deque is still holding prompts for, when it holds any.
fn pending_claude_turn(runtime: &ThreadRuntime) -> Option<TurnId> {
    runtime
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pending_claude_inputs
        .front()
        .map(|(turn, _)| *turn)
}

/// The events that clear `inflight_turn`, and with it the turn a queued prompt belongs to.
fn ends_the_turn(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::TurnCompleted { .. }
            | AgentEvent::TurnAborted { .. }
            | AgentEvent::SessionExited { .. }
            | AgentEvent::RuntimeError { fatal: true, .. }
    )
}

/// Whether this event ends the provider session, so its open gates go with it.
fn ends_the_session(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::SessionExited { .. }
            | AgentEvent::RuntimeError { fatal: true, .. }
            | AgentEvent::SessionStateChanged(SessionState::Stopped)
    )
}

/// The answer a gate nobody can respond to any more settles with.
///
/// It mirrors the one `control_cancel_request` already writes: the request is gone, so the safe
/// reading is that nothing was allowed.
fn closed_gate_answer(kind: &GateKind) -> GateAnswer {
    match kind {
        GateKind::Permission { .. } => GateAnswer::Permission {
            choice: PermissionChoice::Deny,
            edited_payload: None,
        },
        GateKind::Question { .. } => GateAnswer::Question {
            answers: Vec::new(),
        },
        GateKind::Plan { .. } => GateAnswer::Plan(PlanAnswer::AskForChanges {
            note: String::new(),
        }),
    }
}

fn summary_transition(before: &AgentThreadSummary, after: &AgentThreadSummary) -> bool {
    before.attention != after.attention
        || before.session != after.session
        || before.turn != after.turn
        || before.title != after.title
        || before.exit_code != after.exit_code
}

fn event_name(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::SessionStarted { .. } => "session_started",
        AgentEvent::MetadataChanged { .. } => "metadata_changed",
        AgentEvent::SessionStateChanged(_) => "session_state_changed",
        AgentEvent::SessionExited { .. } => "session_exited",
        AgentEvent::TurnStarted { .. } => "turn_started",
        AgentEvent::TurnCompleted { .. } => "turn_completed",
        AgentEvent::TurnAborted { .. } => "turn_aborted",
        AgentEvent::ItemStarted { .. } => "item_started",
        AgentEvent::ContentDelta { .. } => "content_delta",
        AgentEvent::ItemUpdated { .. } => "item_updated",
        AgentEvent::ItemCompleted { .. } => "item_completed",
        AgentEvent::GateOpened { .. } => "gate_opened",
        AgentEvent::GateResolved { .. } => "gate_resolved",
        AgentEvent::TokenUsage { .. } => "token_usage",
        AgentEvent::Checkpoint(_) => "checkpoint",
        AgentEvent::Retrying { .. } => "retrying",
        AgentEvent::RuntimeError { .. } => "runtime_error",
        AgentEvent::Notice(_) => "notice",
    }
}

fn default_agent_commands() -> AgentCommands {
    AgentCommands {
        claude: AgentKind::Claude.executable().to_owned(),
        opencode: AgentKind::OpenCode.executable().to_owned(),
    }
}

fn provider_factory_error(error: anyhow::Error) -> ProtoError {
    match error.downcast::<ProviderError>() {
        Ok(error) => provider_error(error),
        Err(error) => ProtoError {
            kind: ErrorKind::Unknown,
            message: one_line(&format!("could not construct agent provider: {error:#}")),
        },
    }
}

fn provider_error(error: ProviderError) -> ProtoError {
    match error {
        ProviderError::Unavailable { reason } => ProtoError {
            kind: ErrorKind::Unsupported,
            message: one_line(&format!(
                "{reason}. Open a terminal fallback and run the configured agent command."
            )),
        },
        ProviderError::Protocol { message } => conflict(message),
        ProviderError::Exited { code } => conflict(format!("agent provider exited: {code:?}")),
        ProviderError::Timeout { what } => ProtoError {
            kind: ErrorKind::Unknown,
            message: one_line(&format!("agent provider timed out waiting for {what}")),
        },
    }
}

fn daemon_error(error: crate::DaemonError) -> ProtoError {
    error.into()
}

fn storage_error(error: anyhow::Error) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Fs,
        message: one_line(&format!("native-agent storage failed: {error:#}")),
    }
}

fn not_found(message: impl Into<String>) -> ProtoError {
    ProtoError {
        kind: ErrorKind::NotFound,
        message: one_line(&message.into()),
    }
}

fn conflict(message: impl Into<String>) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Conflict,
        message: one_line(&message.into()),
    }
}

fn validation(message: impl Into<String>) -> ProtoError {
    ProtoError {
        kind: ErrorKind::Validation,
        message: one_line(&message.into()),
    }
}

fn one_line(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests;
