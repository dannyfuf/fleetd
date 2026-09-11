//! Serialized native-agent thread lifecycle manager.
//!
//! It owns providers, sequencing and the hydrated reducer state of the threads something is
//! actually using. It owns neither the transcript nor the thread list: both live in
//! [`super::store::SqliteAgentStore`], which is why a daemon start replays nothing and
//! `AgentThreadList` is one `SELECT` (`docs/NATIVE-AGENTS.md` §8). See [`hydrate`] for how a
//! thread's reducer state appears on first use and [`apply`] for the write path's ordering.

mod apply;
mod bodies;
mod checkpoints;
mod commands;
mod controls;
mod hydrate;
mod mirror;
#[cfg(test)]
mod tests;
mod window;

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
};

use crate::{server::BroadcastBus, services::worktrees::Worktrees, stores::config::ConfigStore};
use anyhow::anyhow;
use chrono::Utc;
use fleet_core::{
    agents::{
        AgentEvent, AgentKind, AgentThreadSummary, ApprovalPolicy, CheckpointKind, GateResolver,
        HarnessCapabilities, ItemId, ItemStatus, SandboxPolicy, SessionState, StartRequest,
        ThreadId, TurnId, TurnState, UserInput,
    },
    config::AgentCommands,
    ids::{HostId, WorktreeId},
};
use fleet_proto::{
    error::{ErrorKind, ProtoError},
    response::ResponseBody,
};

use super::{
    AgentThreadRecord,
    providers::{AgentProvider, ProviderError, ProviderEvents, spawn_provider},
    store::SqliteAgentStore,
    thread,
};
use apply::{
    Serialized, apply_event, closed_gate_answer, edited_paths, ends_the_session, ends_the_turn,
    pending_input_turn, publish_applied, runtime_inflight, user_item_started,
};
use thread::{AppliedEvent, ThreadRuntime, coalesce_deltas};

const DELTA_TICK: Duration = Duration::from_millis(16);

/// Emission-to-read skew worth a log line.
///
/// Codex stamps `emittedAtMs` on every notification and it is the only server-side clock Fleet
/// gets (§9.4). It is not put on the wire — `SeqEvent` has no reader for it — so the one use it
/// has is here: a frame read this far behind its own emission means the harness, the pipe or the
/// remote link is the source of a stall, which is precisely the question a latency budget asks.
/// Well above the 16 ms merge tick so a normal turn logs nothing.
const EMISSION_SKEW_FLOOR: Duration = Duration::from_millis(250);

type ProviderFactory = dyn Fn(AgentKind, &StartRequest, &AgentCommands) -> anyhow::Result<Box<dyn AgentProvider>>
    + Send
    + Sync;
type RemoteHostResolver = dyn Fn(&WorktreeId) -> Option<HostId> + Send + Sync;

/// The store, or the reason there is none.
///
/// A database this build cannot open or migrate is fatal *for the agent service* and for nothing
/// else. `Services::build` is infallible and is constructed from twenty call sites, and taking
/// terminals, jobs and worktrees down over an agent transcript database would be a worse failure
/// than refusing agent work: every agent request answers with the reason instead, once, loudly.
enum StoreSlot {
    Ready(SqliteAgentStore),
    Unavailable(String),
}

struct ManagerInner {
    store: StoreSlot,
    events: BroadcastBus,
    worktrees: Worktrees,
    config: Option<Arc<ConfigStore>>,
    /// The threads whose reducer state is currently in memory. Absence means "not hydrated yet",
    /// never "does not exist": existence is a question for the database.
    threads: RwLock<HashMap<ThreadId, ThreadRuntime>>,
    /// Serializes first-use hydration, so one thread is never built twice concurrently.
    ///
    /// One gate for the whole manager rather than one per thread, which is what makes the
    /// background repair pass safe to run while a user is working: `tokio::sync::Mutex` hands the
    /// gate out in FIFO order, so a request waiting behind the pass is served after the current
    /// thread rather than after all of them.
    hydration: tokio::sync::Mutex<()>,
    provider_factory: Arc<ProviderFactory>,
    remote_host_resolver: RwLock<Option<Arc<RemoteHostResolver>>>,
    /// Fleet-owned worktree checkpoints, or `None` on a manager built without them.
    ///
    /// Installed after construction, like the remote-host resolver: `Services::build` creates
    /// both services and neither can take the other as a constructor argument. `None` means
    /// nothing is captured, so `[u]` is simply not drawn — never that a turn is refused.
    checkpoints: RwLock<Option<crate::services::checkpoints::Checkpoints>>,
}

impl ManagerInner {
    /// The store, or the open failure that stands in for it.
    fn store(&self) -> anyhow::Result<&SqliteAgentStore> {
        match &self.store {
            StoreSlot::Ready(store) => Ok(store),
            StoreSlot::Unavailable(reason) => Err(anyhow!(
                "the native-agent database is unavailable: {reason}"
            )),
        }
    }
}

/// Owns native-agent threads, providers, sequencing, and projections.
#[derive(Clone)]
pub struct AgentSessionManager {
    inner: Arc<ManagerInner>,
}

impl AgentSessionManager {
    /// Creates a manager over the agent database, daemon event bus, worktree path lookup, and
    /// the configuration that names each provider's executable.
    ///
    /// `database` is `FleetHome::agents_db_path()`.
    #[must_use]
    pub fn new(
        database: PathBuf,
        events: BroadcastBus,
        worktrees: Worktrees,
        config: Arc<ConfigStore>,
    ) -> Self {
        let manager = Self::new_with_factory(
            database,
            events,
            worktrees,
            Some(config),
            Arc::new(spawn_provider),
        );
        manager.set_remote_host_resolver(Arc::new(|_| None));
        manager
    }

    fn new_with_factory(
        database: PathBuf,
        events: BroadcastBus,
        worktrees: Worktrees,
        config: Option<Arc<ConfigStore>>,
        provider_factory: Arc<ProviderFactory>,
    ) -> Self {
        // Opening the database migrates it and runs the one-shot NDJSON import. It reads no
        // transcript: the boot census it takes is two index lookups, and the replay it may imply
        // happens in `repair`, in the background, one thread at a time.
        let store = match SqliteAgentStore::open(database) {
            Ok(store) => StoreSlot::Ready(store),
            Err(error) => {
                let reason = format!("{error:#}");
                tracing::error!(
                    target: "fleet::agents",
                    error = %reason,
                    "the native-agent database could not be opened; agent requests will be refused"
                );
                StoreSlot::Unavailable(reason)
            }
        };
        let manager = Self {
            inner: Arc::new(ManagerInner {
                store,
                events,
                worktrees,
                config,
                threads: RwLock::new(HashMap::new()),
                hydration: tokio::sync::Mutex::new(()),
                provider_factory,
                remote_host_resolver: RwLock::new(None),
                checkpoints: RwLock::new(None),
            }),
        };
        manager.spawn_repair();
        manager
    }

    /// Starts the background boot repair, if this manager was built inside a runtime.
    ///
    /// `Services::build` is synchronous and a handful of tests construct a manager with no
    /// runtime at all, so the spawn is conditional rather than assumed. Nothing is lost when it
    /// does not happen: the census is recomputed at the next start, and a thread reached before
    /// the repair gets to it is hydrated — and therefore settled — by that request instead.
    fn spawn_repair(&self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::debug!(
                target: "fleet::agents",
                "no runtime to repair native-agent threads on; deferring to first use"
            );
            return;
        };
        let manager = self.clone();
        handle.spawn(async move { manager.repair().await });
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

    /// Installs the checkpoint service a capture runs against.
    ///
    /// Separate from the constructor for the same reason the remote-host resolver is: both
    /// services are built by `Services::build`, and one cannot be an argument to the other.
    pub(crate) fn set_checkpoints(&self, checkpoints: crate::services::checkpoints::Checkpoints) {
        *self
            .inner
            .checkpoints
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(checkpoints);
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
    ///
    /// One `SELECT` against `threads`, whatever the transcript history is. It deliberately does
    /// *not* consult the hydrated map: every append commits its projection rows before the reply
    /// resolves, so the database is never behind memory, and reading one source keeps a hydrated
    /// thread and a cold one from being described differently.
    pub async fn summaries(&self) -> Vec<AgentThreadSummary> {
        match self.inner.store() {
            Ok(store) => match store.summaries().await {
                Ok(summaries) => summaries,
                Err(error) => {
                    tracing::warn!(%error, "could not list the native-agent threads");
                    Vec::new()
                }
            },
            Err(error) => {
                tracing::warn!(%error, "could not list the native-agent threads");
                Vec::new()
            }
        }
    }

    /// The capabilities the live harness negotiated for this thread, when one is running.
    ///
    /// Read off the adapter rather than a column: a capability set belongs to one *process*
    /// (§3.1 freezes it for that process's life), and a thread with no process has no capability
    /// set to report. `None` gates every control off, which is the safe direction — a button that
    /// silently means something weaker than it says is worse than no button (§4.5).
    async fn harness_capabilities(&self, thread: ThreadId) -> Option<HarnessCapabilities> {
        let runtime = self.hydrated(thread)?;
        let provider = runtime.provider.lock().await;
        provider.as_ref().map(|provider| provider.capabilities())
    }

    /// Handles `AgentThreadList`.
    pub async fn list(&self) -> Result<ResponseBody, ProtoError> {
        let store = self.inner.store().map_err(storage_error)?;
        store
            .summaries()
            .await
            .map(ResponseBody::AgentThreads)
            .map_err(storage_error)
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

    /// Starts a provider for a thread whose cursor can bring it back.
    ///
    /// The owner check is the enforcement point of §9.3's second authority rule: with
    /// `create`, this is one of exactly two places in the daemon that starts a harness process,
    /// and a mirrored thread must never be one of them. The owner is running that session; a
    /// second process on this machine would be a second writer on the owner's sequence.
    async fn resume_runtime(&self, runtime: &ThreadRuntime) -> Result<(), ProtoError> {
        if let Some(owner) = runtime.owner() {
            return Err(ProtoError {
                kind: ErrorKind::Remote,
                message: one_line(&format!(
                    "agent thread is owned by host {owner}; its provider runs there, not here"
                )),
            });
        }
        let operation = runtime.operation.lock().await;
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
            fork: false,
            env: BTreeMap::new(),
            sandbox: SandboxPolicy::default(),
            approval_policy: ApprovalPolicy::default(),
            permission_profile: None,
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
        self.apply_one(
            runtime,
            &operation,
            AgentEvent::SessionStateChanged(SessionState::Ready),
            Some("resume".to_owned()),
        )
        .await?;
        self.apply_one(
            runtime,
            &operation,
            AgentEvent::Compacted(CheckpointKind::Resumed { age_ms }),
            Some("resume".to_owned()),
        )
        .await?;
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

    async fn apply_provider_event(
        &self,
        runtime: &ThreadRuntime,
        operation: Serialized<'_>,
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
            self.settle_open_gates(runtime, operation).await?
        } else {
            Vec::new()
        };
        // The turn is about to be cleared, so this is the last event that can still carry the
        // prompts Claude was given under it. Without this a turn whose `TurnStarted` never
        // arrived strands them, and the next turn's drain would find them ahead of its own.
        if ends_the_turn(&event)
            && let Some(turn) = pending_input_turn(runtime)
        {
            applied.extend(
                self.flush_pending_inputs(runtime, operation, turn, "pending_input")
                    .await?,
            );
        }
        // Before the event is applied, because this is the earliest the daemon can know an edit
        // is coming. It is **best effort and says so**: neither harness waits for Fleet before
        // running an auto-approved tool, so a file-scope checkpoint can land after the write and
        // then restore what is already there. The turn-scope checkpoint taken in `send` is the
        // guarantee; this one is the finer-grained revert when the race goes Fleet's way — which
        // it always does for a gated edit, the case where the user is watching.
        if let Some((turn, paths)) = edited_paths(&event) {
            let (thread, worktree) = {
                let state = runtime
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (state.record.thread, state.record.worktree.clone())
            };
            self.capture_file_checkpoint(thread, turn, &worktree, paths)
                .await;
        }
        applied.push(self.apply(runtime, operation, event, raw).await?);
        // Both harnesses announce the turn on the stream rather than as the submit's return
        // value, so this is the moment the prompt that opened it becomes a transcript row. It is
        // not Claude-only: Codex suppresses the echo of a message Fleet itself sent (it carries
        // Fleet's own `clientId`), so without this the user's bubble would exist in no log at
        // all and vanish on the next open.
        if let Some((turn, user_item)) = turn_started {
            applied.extend(
                self.drain_inputs_applied(runtime, operation, turn, user_item)
                    .await?,
            );
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
    async fn flush_pending_inputs(
        &self,
        runtime: &ThreadRuntime,
        operation: Serialized<'_>,
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
        // The prompt already parked for this turn carries the identity the client drew its
        // optimistic bubble under, so the synthesized announcement names that item rather than a
        // second one the client cannot recognise.
        let user_item = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_inputs
            .front()
            .and_then(|(_, input)| input.item)
            .unwrap_or_default();
        let mut applied = vec![
            self.apply(
                runtime,
                operation,
                AgentEvent::TurnStarted { turn, user_item },
                Some(raw.to_owned()),
            )
            .await?,
        ];
        applied.extend(
            self.drain_inputs_applied(runtime, operation, turn, user_item)
                .await?,
        );
        Ok(applied)
    }

    async fn drain_inputs_applied(
        &self,
        runtime: &ThreadRuntime,
        operation: Serialized<'_>,
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
            for (pending_turn, input) in std::mem::take(&mut state.pending_inputs) {
                if pending_turn == turn {
                    inputs.push(input);
                } else {
                    kept.push_back((pending_turn, input));
                }
            }
            state.pending_inputs = kept;
            inputs
        };
        let mut applied = Vec::new();
        for (index, input) in inputs.into_iter().enumerate() {
            // Position zero is the prompt the announcement named, so it keeps that identity —
            // which is the client's own when it sent one, because the adapter adopts it. Every
            // later prompt in the same turn keeps the id its own client drew it under.
            let item = if index == 0 {
                first_item
            } else {
                input.item.unwrap_or_else(ItemId::new)
            };
            applied.push(
                self.apply(
                    runtime,
                    operation,
                    // Nothing parked here was a steer: a submission the harness folded into a
                    // running turn is recorded by `send` itself and never queued.
                    user_item_started(turn, item, input, false),
                    None,
                )
                .await?,
            );
            applied.push(
                self.apply(
                    runtime,
                    operation,
                    AgentEvent::ItemCompleted {
                        item,
                        status: ItemStatus::Completed,
                    },
                    None,
                )
                .await?,
            );
        }
        Ok(applied)
    }

    async fn record_user_input(
        &self,
        runtime: &ThreadRuntime,
        operation: Serialized<'_>,
        turn: TurnId,
        item: ItemId,
        input: UserInput,
        steered: bool,
    ) -> Result<(), ProtoError> {
        self.apply_one(
            runtime,
            operation,
            user_item_started(turn, item, input, steered),
            None,
        )
        .await?;
        self.apply_one(
            runtime,
            operation,
            AgentEvent::ItemCompleted {
                item,
                status: ItemStatus::Completed,
            },
            None,
        )
        .await
    }

    /// Closes every gate still open, as the appended events §6 requires.
    async fn settle_open_gates(
        &self,
        runtime: &ThreadRuntime,
        operation: Serialized<'_>,
    ) -> Result<Vec<AppliedEvent>, ProtoError> {
        let open = runtime
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .projection
            .gates
            .clone();
        let mut applied = Vec::with_capacity(open.len());
        for gate in open {
            applied.push(
                self.apply(
                    runtime,
                    operation,
                    AgentEvent::GateResolved {
                        gate: gate.id,
                        answer: closed_gate_answer(&gate.kind),
                        by: GateResolver::ProviderClosed,
                    },
                    Some("provider_closed".to_owned()),
                )
                .await?,
            );
        }
        Ok(applied)
    }

    async fn apply_one(
        &self,
        runtime: &ThreadRuntime,
        operation: Serialized<'_>,
        event: AgentEvent,
        raw: Option<String>,
    ) -> Result<(), ProtoError> {
        let applied = self.apply(runtime, operation, event, raw).await?;
        publish_applied(&self.inner, runtime, applied);
        Ok(())
    }

    async fn apply(
        &self,
        runtime: &ThreadRuntime,
        operation: Serialized<'_>,
        event: AgentEvent,
        raw: Option<String>,
    ) -> Result<AppliedEvent, ProtoError> {
        apply_event(&self.inner, runtime, operation, event, raw)
            .await
            .map_err(storage_error)
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
            let operation = runtime.operation.lock().await;
            let exited = matches!(event.event, AgentEvent::SessionExited { .. });
            if let Some(skew) = event.emission_skew
                && skew >= EMISSION_SKEW_FLOOR
            {
                tracing::debug!(
                    target: "fleet::agents",
                    skew_ms = skew.as_millis(),
                    frame = event.raw.as_deref().unwrap_or("unnamed"),
                    "a harness frame was read well after the harness says it emitted it",
                );
            }
            match manager
                .apply_provider_event(&runtime, &operation, event.event, event.raw)
                .await
            {
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

    let operation = runtime.operation.lock().await;
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
        match manager
            .apply_provider_event(
                &runtime,
                &operation,
                AgentEvent::SessionExited {
                    code: None,
                    expected: false,
                },
                Some("provider_event_stream_closed".to_owned()),
            )
            .await
        {
            Ok(events) => {
                for applied in events {
                    publish_applied(&inner, &runtime, applied);
                }
            }
            Err(error) => tracing::warn!(%error, "could not record provider event-stream exit"),
        }
    }
    // The gate is still held: the slot is emptied under the same serialization every settlement
    // above ran under, so nothing can pick up a provider this loop is retiring.
    *runtime.provider.lock().await = None;
    drop(operation);
}

fn default_agent_commands() -> AgentCommands {
    AgentCommands {
        claude: AgentKind::Claude.executable().to_owned(),
        codex: AgentKind::Codex.executable().to_owned(),
        // ADR 0014 keeps the legacy field readable for one release, so the terminal an
        // existing config still points at OpenCode with keeps a command to run.
        opencode: "opencode".to_owned(),
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
