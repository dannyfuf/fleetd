//! OpenCode HTTP/SSE native-agent provider.

mod http;
mod map;
mod server;
mod sse;

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use fleet_core::agents::{
    AbortReason, AgentEvent, AgentKind, Capabilities, GateAnswer, GateId, GateResolver,
    ModelSelection, PermissionChoice, PermissionMode, PlanAnswer, SessionState, StartRequest,
    TurnId, UserInput,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};

use self::{
    http::HttpClient,
    map::{MapAction, Mapper, PendingGateKind, plan_answer_note},
    server::{ManagedServer, free_port},
    sse::{SseBatch, SseParser, WireEvent},
};
use super::{
    AgentProvider, ProviderError, ProviderEvent, ProviderEvents, ProviderResult, ProviderSink,
    empty_events,
};

const READY_BUDGET: Duration = Duration::from_secs(30);
const READY_INTERVAL: Duration = Duration::from_millis(100);
const SSE_SILENCE: Duration = Duration::from_secs(5);
const RECONNECT_MIN: Duration = Duration::from_millis(250);
const RECONNECT_MAX: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeCursor {
    base_url: String,
    directory: String,
    session_id: String,
}

/// OpenCode adapter backed by one managed HTTP server per thread.
#[derive(Debug)]
pub struct OpenCodeProvider {
    command: String,
    events_tx: ProviderSink,
    events_rx: Option<ProviderEvents>,
    http: Option<HttpClient>,
    server: Option<ManagedServer>,
    mapper: Option<Arc<Mutex<Mapper>>>,
    session_id: Option<String>,
    mode: PermissionMode,
    model: Option<ModelSelection>,
    stopping: Arc<AtomicBool>,
    plan_approved: bool,
}

impl Default for OpenCodeProvider {
    fn default() -> Self {
        Self::new(AgentKind::OpenCode.executable())
    }
}

impl OpenCodeProvider {
    /// Creates a provider that launches `opencode serve` from the configured command line.
    #[must_use]
    pub fn new(command: impl Into<String>) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        Self {
            command: command.into(),
            events_tx,
            events_rx: Some(events_rx),
            http: None,
            server: None,
            mapper: None,
            session_id: None,
            mode: PermissionMode::Ask,
            model: None,
            stopping: Arc::new(AtomicBool::new(false)),
            plan_approved: false,
        }
    }
}

impl Drop for OpenCodeProvider {
    /// Ends the spawned event loop with the adapter.
    ///
    /// §3 leaves no runtime behind a finished session, but the manager retires a provider by
    /// dropping its slot (a `SessionExited` never calls [`AgentProvider::stop`]), and the loop
    /// owns a clone of the sink, so without this the loop would reconnect to a dead port every
    /// five seconds for the life of the daemon and hold the manager's receiver open with it.
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
    }
}

#[async_trait]
impl AgentProvider for OpenCodeProvider {
    fn kind(&self) -> AgentKind {
        AgentKind::OpenCode
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            resume: true,
            fork: true,
            steer: true,
            interrupt: true,
            modes: true,
            models: true,
        }
    }

    async fn start(&mut self, req: StartRequest) -> ProviderResult<()> {
        if self.http.is_some() {
            return Err(ProviderError::Protocol {
                message: "OpenCode provider is already started".to_owned(),
            });
        }
        if req.provider != AgentKind::OpenCode {
            return Err(ProviderError::Protocol {
                message: format!("OpenCode provider cannot start {:?}", req.provider),
            });
        }
        let directory = tokio::fs::canonicalize(&req.worktree_path)
            .await
            .map_err(|error| ProviderError::Unavailable {
                reason: format!(
                    "canonicalize OpenCode worktree `{}`: {error}",
                    req.worktree_path.display()
                ),
            })?;
        let port = free_port()?;
        let base_url = format!("http://127.0.0.1:{}", port.port());
        let server =
            ManagedServer::spawn(&self.command, &directory, port, self.events_tx.clone()).await?;
        let http = HttpClient::new(
            &base_url,
            &directory,
            std::env::var("OPENCODE_SERVER_PASSWORD").ok(),
        )?;
        if let Err(error) = wait_until_ready(&http).await {
            // The child's own last word — `EADDRINUSE`, a config error — is what a reader needs;
            // a bare readiness timeout names the symptom and hides the cause.
            let error = match (error, server.last_error()) {
                (ProviderError::Timeout { what }, Some(detail)) => ProviderError::Unavailable {
                    reason: format!("{what}; `opencode serve` reported: {detail}"),
                },
                (error, _) => error,
            };
            let _ignored = server.stop(&self.events_tx).await;
            return Err(error);
        }

        let initialized = initialize_session(&http, &directory, &req).await;
        let (session_id, commands, cursor) = match initialized {
            Ok(initialized) => initialized,
            Err(error) => {
                let _ignored = server.stop(&self.events_tx).await;
                return Err(error);
            }
        };
        // §2's mode word is a promise about behaviour, and for OpenCode the promise is the
        // server's to keep: permission is decided by `opencode.json`, not by the agent the
        // adapter picks. A server configured to allow everything would still render
        // `asks before edits` — a false statement of scope — so the effective mode is what the
        // session reports (BH9).
        let mode = effective_mode(&http, req.mode).await;
        // §2's metadata row states the model *before* the first turn too. OpenCode publishes
        // one only on the first assistant message, so an idle tab read `build agent · full
        // access` with no model at all; the server's configured default is what that turn will
        // use, and a server that will not answer simply leaves the segment out as before.
        let model = match req.model.clone() {
            Some(model) => Some(model),
            None => http.default_model().await.unwrap_or_else(|error| {
                tracing::debug!(
                    target: "fleet::agents::opencode",
                    %error,
                    "OpenCode did not report its default model"
                );
                None
            }),
        };
        if self
            .events_tx
            .send(
                AgentEvent::SessionStarted {
                    provider: AgentKind::OpenCode,
                    resume_cursor: Some(cursor),
                    model,
                    mode,
                    tools: Vec::new(),
                    commands,
                    skills: Vec::new(),
                }
                .into(),
            )
            .is_err()
        {
            let _ignored = server.stop(&self.events_tx).await;
            return Err(ProviderError::Exited { code: None });
        }
        if self
            .events_tx
            .send(AgentEvent::SessionStateChanged(SessionState::Ready).into())
            .is_err()
        {
            let _ignored = server.stop(&self.events_tx).await;
            return Err(ProviderError::Exited { code: None });
        }

        self.mode = mode;
        self.model = req.model;
        self.plan_approved = mode != PermissionMode::Plan;
        let mut mapper_state = Mapper::new(session_id.clone(), mode);
        // §2's `context 34%` needs the model's window; `GET /config/providers` is the only place
        // OpenCode publishes it. A server that will not answer leaves the percentage unknown,
        // which the metadata row zero-suppresses, rather than failing the session over it.
        match http.model_context_limits().await {
            Ok(limits) => mapper_state.set_context_limits(limits.into_iter().collect()),
            Err(error) => {
                tracing::debug!(%error, "could not read OpenCode model context limits");
            }
        }
        let mapper = Arc::new(Mutex::new(mapper_state));
        self.stopping.store(false, Ordering::Release);
        tokio::spawn(run_event_loop(
            http.clone(),
            Arc::clone(&mapper),
            self.events_tx.clone(),
            Arc::clone(&self.stopping),
        ));
        self.http = Some(http);
        self.server = Some(server);
        self.mapper = Some(mapper);
        self.session_id = Some(session_id);
        Ok(())
    }

    async fn send(&mut self, turn: TurnId, input: UserInput) -> ProviderResult<()> {
        if !input.attachments.is_empty() {
            return Err(ProviderError::Protocol {
                message: "OpenCode native adapter does not yet support attachments".to_owned(),
            });
        }
        self.prompt(turn, input).await
    }

    async fn interrupt(&mut self, turn: TurnId) -> ProviderResult<()> {
        let http = self.http()?.clone();
        let session = self.session_id()?.to_owned();
        let mapper = Arc::clone(self.mapper()?);
        mapper
            .lock()
            .await
            .mark_abort(turn)
            .map_err(|message| ProviderError::Protocol { message })?;
        let accepted = match http.abort(&session).await {
            Ok(accepted) => accepted,
            Err(error) => {
                mapper.lock().await.clear_abort(turn);
                return Err(error);
            }
        };
        if !accepted {
            mapper.lock().await.clear_abort(turn);
            return Err(ProviderError::Protocol {
                message: "OpenCode refused the session abort".to_owned(),
            });
        }
        Ok(())
    }

    async fn respond(&mut self, gate: GateId, answer: GateAnswer) -> ProviderResult<()> {
        let mapper = Arc::clone(self.mapper()?);
        let pending = mapper
            .lock()
            .await
            .gate(gate)
            .ok_or_else(|| ProviderError::Protocol {
                message: format!("unknown OpenCode gate {gate}"),
            })?;
        match pending.kind {
            PendingGateKind::Permission => {
                let GateAnswer::Permission { choice, .. } = &answer else {
                    return Err(wrong_gate_answer("permission"));
                };
                let reply = match choice {
                    PermissionChoice::AllowOnce | PermissionChoice::Edit => "once",
                    PermissionChoice::AllowSession | PermissionChoice::AllowDirectory => "always",
                    PermissionChoice::Deny | PermissionChoice::DenyAndStop => "reject",
                };
                if !self
                    .http()?
                    .permission_reply(&pending.provider_id, reply)
                    .await?
                {
                    return Err(ProviderError::Protocol {
                        message: format!(
                            "OpenCode refused permission reply `{reply}` for {}",
                            pending.provider_id
                        ),
                    });
                }
            }
            PendingGateKind::Question => {
                let GateAnswer::Question { answers } = &answer else {
                    return Err(wrong_gate_answer("question"));
                };
                if !self
                    .http()?
                    .question_reply(&pending.provider_id, answers)
                    .await?
                {
                    return Err(ProviderError::Protocol {
                        message: format!(
                            "OpenCode refused question reply for {}",
                            pending.provider_id
                        ),
                    });
                }
            }
            PendingGateKind::Plan => match &answer {
                GateAnswer::Plan(PlanAnswer::Approve) => {
                    self.plan_approved = true;
                }
                GateAnswer::Plan(PlanAnswer::AskForChanges { .. }) => {
                    self.plan_approved = false;
                }
                _ => return Err(wrong_gate_answer("plan")),
            },
        }
        let note = plan_answer_note(&answer)
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .map(ToOwned::to_owned);
        let event = mapper
            .lock()
            .await
            .resolve_gate(gate, answer, GateResolver::User)
            .ok_or_else(|| ProviderError::Protocol {
                message: format!("OpenCode gate {gate} was already resolved"),
            })?;
        emit_all(&self.events_tx, vec![event])?;
        // §4.2: `ask for changes` feeds the model the user's note. It is its own prompt, so the
        // next thing the user types in the composer is still the next thing that gets sent.
        if let Some(note) = note {
            self.prompt(
                TurnId::new(),
                UserInput {
                    text: note,
                    attachments: Vec::new(),
                },
            )
            .await?;
        }
        Ok(())
    }

    async fn set_mode(&mut self, mode: PermissionMode) -> ProviderResult<()> {
        self.mode = mode;
        self.plan_approved = mode != PermissionMode::Plan;
        if let Some(mapper) = &self.mapper {
            mapper.lock().await.set_mode(mode);
        }
        Ok(())
    }

    async fn set_model(&mut self, model: ModelSelection) -> ProviderResult<()> {
        validate_model(&model)?;
        self.model = Some(model);
        Ok(())
    }

    async fn stop(&mut self) -> ProviderResult<()> {
        self.stopping.store(true, Ordering::Release);
        if let Some(server) = self.server.take() {
            server.stop(&self.events_tx).await?;
        }
        self.http = None;
        self.mapper = None;
        self.session_id = None;
        Ok(())
    }

    fn events(&mut self) -> ProviderEvents {
        self.events_rx.take().unwrap_or_else(empty_events)
    }
}

impl OpenCodeProvider {
    /// Admits one turn and submits its prompt.
    ///
    /// The turn is admitted *before* the request, not after it: §4.2 makes `session.status`
    /// busy/idle the authoritative execution level, and every SSE frame the server emits while
    /// `prompt_async` is in flight — text, tool parts, deltas, todos — is dropped when the
    /// mapper has no active turn, while an `idle` landing there settles a turn that does not
    /// exist yet and leaves the real one to settle on the *next* idle. A request that fails
    /// takes the admission back and settles the turn it announced.
    ///
    /// The mapper lock is taken to admit and again to withdraw, never across the HTTP call:
    /// `run_event_loop` needs the same lock, and holding it for the request timeout would stall
    /// idle detection and gate opening for as long as the prompt takes.
    async fn prompt(&mut self, turn: TurnId, input: UserInput) -> ProviderResult<()> {
        let http = self.http()?.clone();
        let session = self.session_id()?.to_owned();
        let mapper = Arc::clone(self.mapper()?);
        let agent = agent_for_mode(self.mode, self.plan_approved).to_owned();
        let body = prompt_body(&input.text, &agent, self.model.as_ref())?;
        let (announced, events) = {
            let mut mapper = mapper.lock().await;
            let announced = mapper.active_turn().is_none();
            let events = mapper
                .admit(turn, input, agent)
                .map_err(|message| ProviderError::Protocol { message })?;
            (announced, events)
        };
        emit_all(&self.events_tx, events)?;
        if let Err(error) = http.prompt_async(&session, body).await {
            if announced && mapper.lock().await.withdraw(turn) {
                emit_all(
                    &self.events_tx,
                    vec![AgentEvent::TurnAborted {
                        turn,
                        reason: AbortReason::Other(
                            "the prompt could not be sent to OpenCode".to_owned(),
                        ),
                    }],
                )?;
            }
            return Err(error);
        }
        Ok(())
    }

    fn http(&self) -> ProviderResult<&HttpClient> {
        self.http
            .as_ref()
            .ok_or_else(|| ProviderError::Unavailable {
                reason: "OpenCode provider has not started".to_owned(),
            })
    }

    fn mapper(&self) -> ProviderResult<&Arc<Mutex<Mapper>>> {
        self.mapper
            .as_ref()
            .ok_or_else(|| ProviderError::Unavailable {
                reason: "OpenCode provider has not started".to_owned(),
            })
    }

    fn session_id(&self) -> ProviderResult<&str> {
        self.session_id
            .as_deref()
            .ok_or_else(|| ProviderError::Unavailable {
                reason: "OpenCode provider has not started".to_owned(),
            })
    }
}

async fn initialize_session(
    http: &HttpClient,
    directory: &Path,
    req: &StartRequest,
) -> ProviderResult<(String, Vec<String>, String)> {
    let initial_agent = agent_for_mode(req.mode, false);
    let session = match req.resume_cursor.as_deref() {
        Some(cursor) => {
            let cursor = decode_cursor(cursor)?;
            if cursor.directory != directory.to_string_lossy() {
                http.fork_session(&cursor.session_id).await?
            } else {
                match http.session(&cursor.session_id).await? {
                    Some(session)
                        if session.get("directory").and_then(Value::as_str)
                            != Some(http.directory()) =>
                    {
                        http.fork_session(&cursor.session_id).await?
                    }
                    Some(session) => session,
                    None => {
                        http.create_session(create_session_body(
                            req.title.as_deref(),
                            initial_agent,
                            req.model.as_ref(),
                        )?)
                        .await?
                    }
                }
            }
        }
        None => {
            http.create_session(create_session_body(
                req.title.as_deref(),
                initial_agent,
                req.model.as_ref(),
            )?)
            .await?
        }
    };
    let session_id = session
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ProviderError::Protocol {
            message: "OpenCode session response omitted `id`".to_owned(),
        })?
        .to_owned();
    let commands = session_commands(http).await;
    let cursor = encode_cursor(&ResumeCursor {
        base_url: http.base_url().to_owned(),
        directory: http.directory().to_owned(),
        session_id: session_id.clone(),
    })?;
    Ok((session_id, commands, cursor))
}

/// The slash commands the composer's `/` picker may offer for this server (§3.2, §4.1).
///
/// `GET /agent` answers `build`/`plan` — the *modes* §4.2 puts on the metadata row, not
/// commands: splicing `/build` into a prompt runs nothing. The command list is its own
/// endpoint. It is undocumented in harness-protocols.md, so a server that does not answer it
/// leaves the picker empty rather than failing the session.
async fn session_commands(http: &HttpClient) -> Vec<String> {
    match http.commands().await {
        Ok(commands) => commands
            .into_iter()
            .filter_map(|command| {
                command
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .filter(|name| !name.is_empty())
            .collect(),
        Err(error) => {
            tracing::debug!(
                target: "fleet::agents::opencode",
                %error,
                "OpenCode did not list its slash commands"
            );
            Vec::new()
        }
    }
}

/// The mode the tab may honestly advertise, given what the server will actually do.
///
/// `plan` is the adapter's own (it selects the `plan` agent) and survives untouched. `ask` and
/// `accept edits` are the server's, and a server whose `permission` config allows everything
/// will never open a gate: reporting `full access` is the only true word for it. A server that
/// will not answer `GET /config` keeps the requested mode rather than failing the session.
async fn effective_mode(http: &HttpClient, requested: PermissionMode) -> PermissionMode {
    if matches!(requested, PermissionMode::Plan | PermissionMode::FullAccess) {
        return requested;
    }
    match http.permission_allows_everything().await {
        Ok(true) => {
            tracing::info!(
                target: "fleet::agents::opencode",
                ?requested,
                "OpenCode is configured to allow every permission; reporting full access"
            );
            PermissionMode::FullAccess
        }
        Ok(false) => requested,
        Err(error) => {
            tracing::debug!(%error, "could not read the OpenCode permission configuration");
            requested
        }
    }
}

async fn wait_until_ready(http: &HttpClient) -> ProviderResult<()> {
    let deadline = tokio::time::Instant::now() + READY_BUDGET;
    loop {
        let error = match http.readiness().await {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(ProviderError::Timeout {
                what: format!("OpenCode server readiness ({error})"),
            });
        }
        tokio::time::sleep(READY_INTERVAL).await;
    }
}

/// True once nothing can come of another poll: the adapter is stopping, or nobody is listening.
fn finished(stopping: &AtomicBool, events: &ProviderSink) -> bool {
    stopping.load(Ordering::Acquire) || events.is_closed()
}

async fn run_event_loop(
    http: HttpClient,
    mapper: Arc<Mutex<Mapper>>,
    events: ProviderSink,
    stopping: Arc<AtomicBool>,
) {
    let mut reconnect_delay = RECONNECT_MIN;
    while !finished(&stopping, &events) {
        match http.event_stream().await {
            Ok(response) => {
                reconnect_delay = RECONNECT_MIN;
                let mut stream = response.bytes_stream();
                let mut parser = SseParser::default();
                loop {
                    // The loop is the only thing keeping the session's transport alive, so it
                    // has to notice both ways a session can end: `stop()` (and `Drop`) raising
                    // `stopping`, and the manager dropping the receiver when `SessionExited` is
                    // applied. Without the second check a crashed `opencode serve` would be
                    // reconnected to forever and the manager's stream would never close.
                    if finished(&stopping, &events) {
                        return;
                    }
                    // §4.2: SSE silence is never idle. The timeout is applied unconditionally,
                    // because a prompt admitted while the loop was parked on an idle stream
                    // would otherwise never be reconciled — `reconcile_status` itself is the
                    // one that decides whether there is a running turn to poll for.
                    let next = match tokio::time::timeout(SSE_SILENCE, stream.next()).await {
                        Ok(next) => next,
                        Err(_) => {
                            reconcile_status(&http, &mapper, &events).await;
                            // §4.2 recovers a *quiet* stream exactly like a dropped one: a
                            // half-open connection never errors, so gates asked behind it would
                            // otherwise only surface once the socket finally broke. Idle threads
                            // are left alone — OpenCode only asks inside a running turn.
                            if mapper.lock().await.is_running() {
                                reconcile_gates(&http, &mapper, &events).await;
                            }
                            continue;
                        }
                    };
                    match next {
                        Some(Ok(chunk)) => {
                            drain_batch(&http, &mapper, &events, parser.push(&chunk)).await;
                        }
                        Some(Err(error)) => {
                            tracing::warn!(%error, "OpenCode SSE stream disconnected");
                            break;
                        }
                        None => {
                            drain_batch(&http, &mapper, &events, parser.finish()).await;
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "failed to subscribe to OpenCode SSE");
            }
        }
        if finished(&stopping, &events) {
            break;
        }
        reconcile_status(&http, &mapper, &events).await;
        reconcile_gates(&http, &mapper, &events).await;
        tokio::time::sleep(reconnect_delay).await;
        reconnect_delay = reconnect_delay.saturating_mul(2).min(RECONNECT_MAX);
    }
}

/// Delivers one decoded chunk: every event it carried, then every payload it could not decode.
async fn drain_batch(
    http: &HttpClient,
    mapper: &Arc<Mutex<Mapper>>,
    events: &ProviderSink,
    batch: SseBatch,
) {
    for wire in batch.events {
        process_wire(http, mapper, events, wire).await;
    }
    for diagnostic in batch.diagnostics {
        tracing::warn!(target: "fleet::agents::opencode", diagnostic, "undecodable OpenCode SSE payload");
        emit_runtime_error(events, false, diagnostic);
    }
}

async fn process_wire(
    http: &HttpClient,
    mapper: &Arc<Mutex<Mapper>>,
    events: &ProviderSink,
    wire: WireEvent,
) {
    let mapped = mapper.lock().await.handle(wire);
    let raw = mapped.raw.clone();
    let _ignored = emit_named(events, mapped.events, raw.as_deref());
    for action in mapped.actions {
        match action {
            MapAction::AutoPermission {
                gate,
                provider_id,
                answer,
            } => match http.permission_reply(&provider_id, "once").await {
                Ok(true) => {
                    if let Some(event) =
                        mapper
                            .lock()
                            .await
                            .resolve_gate(gate, answer, GateResolver::Auto)
                    {
                        let _ignored = events.send(event.into());
                    }
                }
                Ok(false) => {
                    emit_runtime_error(
                        events,
                        false,
                        format!("OpenCode refused automatic permission reply {provider_id}"),
                    );
                }
                Err(error) => emit_runtime_error(events, false, error.to_string()),
            },
        }
    }
}

async fn reconcile_status(http: &HttpClient, mapper: &Arc<Mutex<Mapper>>, events: &ProviderSink) {
    // The epoch is read with the session id, *before* the status round trip, and travels with
    // the snapshot through `reconcile_messages`. `settle_from_status_map` refuses a snapshot the
    // mapper has outrun, so a prompt admitted while these two requests were in flight is not
    // settled by an `idle` that was true only of the turn before it.
    let (session_id, observed_at) = {
        let mapper = mapper.lock().await;
        let Some(session_id) = mapper.is_running().then(|| mapper.session_id().to_owned()) else {
            return;
        };
        (session_id, mapper.admissions())
    };
    match http.status().await {
        Ok(statuses) => {
            let status = statuses.get(&session_id);
            reconcile_messages(http, mapper, events, &session_id).await;
            let mapped = mapper
                .lock()
                .await
                .settle_from_status_map(status, observed_at);
            let _ignored = emit_named(events, mapped.events, Some("session.status"));
        }
        Err(error) => tracing::debug!(%error, "OpenCode status reconciliation failed"),
    }
}

async fn reconcile_messages(
    http: &HttpClient,
    mapper: &Arc<Mutex<Mapper>>,
    events: &ProviderSink,
    session_id: &str,
) {
    let Ok(messages) = http.messages(session_id).await else {
        return;
    };
    let start = messages
        .iter()
        .rposition(|message| message.pointer("/info/role").and_then(Value::as_str) == Some("user"))
        .unwrap_or_default();
    for message in messages.into_iter().skip(start) {
        if let Some(info) = message.get("info") {
            process_wire(
                http,
                mapper,
                events,
                WireEvent {
                    id: "reconcile-message".to_owned(),
                    kind: "message.updated".to_owned(),
                    properties: json!({ "sessionID": session_id, "info": info }),
                },
            )
            .await;
        }
        if let Some(parts) = message.get("parts").and_then(Value::as_array) {
            for part in parts {
                process_wire(
                    http,
                    mapper,
                    events,
                    WireEvent {
                        id: "reconcile-part".to_owned(),
                        kind: "message.part.updated".to_owned(),
                        properties: json!({ "sessionID": session_id, "part": part }),
                    },
                )
                .await;
            }
        }
    }
}

/// Replays the gates the server still holds, and closes the ones it does not.
///
/// §4.2 makes `GET /permission` and `GET /question` recovery truth after a dropped stream.
/// Truth that can only add is not truth: a gate answered in the OpenCode TUI, by another
/// client, or expired while Fleet was disconnected has to close here too, or its card pins the
/// thread at `NeedsYou(Permission)` and every reply to it returns 404.
async fn reconcile_gates(http: &HttpClient, mapper: &Arc<Mutex<Mapper>>, events: &ProviderSink) {
    let mut live = Vec::new();
    let mut complete = true;
    match http.permissions().await {
        Ok(permissions) => {
            for properties in permissions {
                if let Some(id) = properties.get("id").and_then(Value::as_str) {
                    live.push(id.to_owned());
                }
                process_wire(
                    http,
                    mapper,
                    events,
                    WireEvent {
                        id: "reconcile-permission".to_owned(),
                        kind: "permission.asked".to_owned(),
                        properties,
                    },
                )
                .await;
            }
        }
        Err(error) => {
            complete = false;
            tracing::debug!(%error, "could not read OpenCode pending permissions");
        }
    }
    match http.questions().await {
        Ok(questions) => {
            for properties in questions {
                if let Some(id) = properties.get("id").and_then(Value::as_str) {
                    live.push(id.to_owned());
                }
                process_wire(
                    http,
                    mapper,
                    events,
                    WireEvent {
                        id: "reconcile-question".to_owned(),
                        kind: "question.asked".to_owned(),
                        properties,
                    },
                )
                .await;
            }
        }
        Err(error) => {
            complete = false;
            tracing::debug!(%error, "could not read OpenCode pending questions");
        }
    }
    // A partial read is not evidence a gate is gone, so nothing is closed on it.
    if !complete {
        return;
    }
    let stale = {
        let mapper = mapper.lock().await;
        mapper
            .provider_gates()
            .into_iter()
            .filter(|(_, provider_id)| !live.contains(provider_id))
            .map(|(_, provider_id)| provider_id)
            .collect::<Vec<_>>()
    };
    for provider_id in stale {
        let closed = mapper
            .lock()
            .await
            .close_provider_gate(&provider_id, GateResolver::ProviderClosed);
        if let Some(event) = closed {
            let _ignored = emit_named(events, vec![event], Some("permission.replied"));
        }
    }
}

fn emit_all(sender: &ProviderSink, events: Vec<AgentEvent>) -> ProviderResult<()> {
    emit_named(sender, events, None)
}

/// Emits every event mapped from one named OpenCode wire event (§11).
fn emit_named(
    sender: &ProviderSink,
    events: Vec<AgentEvent>,
    raw: Option<&str>,
) -> ProviderResult<()> {
    for event in events {
        sender
            .send(ProviderEvent::new(event, raw))
            .map_err(|_| ProviderError::Exited { code: None })?;
    }
    Ok(())
}

fn emit_runtime_error(sender: &ProviderSink, fatal: bool, message: String) {
    let _ignored = sender.send(AgentEvent::RuntimeError { fatal, message }.into());
}

fn create_session_body(
    title: Option<&str>,
    agent: &str,
    model: Option<&ModelSelection>,
) -> ProviderResult<Value> {
    let mut body = json!({ "agent": agent });
    if let Some(title) = title {
        body["title"] = Value::String(title.to_owned());
    }
    if let Some(model) = model {
        validate_model(model)?;
        body["model"] = json!({
            "id": model.model,
            "providerID": model.provider.as_deref().unwrap_or_default(),
        });
        if let Some(variant) = &model.effort {
            body["model"]["variant"] = Value::String(variant.clone());
        }
    }
    Ok(body)
}

fn prompt_body(text: &str, agent: &str, model: Option<&ModelSelection>) -> ProviderResult<Value> {
    let mut body = json!({
        "parts": [{ "type": "text", "text": text }],
        "agent": agent,
    });
    if let Some(model) = model {
        validate_model(model)?;
        body["model"] = json!({
            "providerID": model.provider.as_deref().unwrap_or_default(),
            "modelID": model.model,
        });
        if let Some(variant) = &model.effort {
            body["variant"] = Value::String(variant.clone());
        }
    }
    Ok(body)
}

fn validate_model(model: &ModelSelection) -> ProviderResult<()> {
    if model.model.is_empty() || model.provider.as_deref().is_none_or(str::is_empty) {
        Err(ProviderError::Protocol {
            message: "OpenCode model selection requires non-empty provider and model ids"
                .to_owned(),
        })
    } else {
        Ok(())
    }
}

fn agent_for_mode(mode: PermissionMode, plan_approved: bool) -> &'static str {
    if mode == PermissionMode::Plan && !plan_approved {
        "plan"
    } else {
        "build"
    }
}

fn wrong_gate_answer(kind: &str) -> ProviderError {
    ProviderError::Protocol {
        message: format!("answer does not match OpenCode {kind} gate"),
    }
}

fn encode_cursor(cursor: &ResumeCursor) -> ProviderResult<String> {
    serde_json::to_string(cursor).map_err(|error| ProviderError::Protocol {
        message: format!("encode OpenCode resume cursor: {error}"),
    })
}

fn decode_cursor(cursor: &str) -> ProviderResult<ResumeCursor> {
    let cursor: ResumeCursor =
        serde_json::from_str(cursor).map_err(|error| ProviderError::Protocol {
            message: format!("decode OpenCode resume cursor: {error}"),
        })?;
    if cursor.base_url.is_empty() || cursor.directory.is_empty() || cursor.session_id.is_empty() {
        return Err(ProviderError::Protocol {
            message: "OpenCode resume cursor contains an empty routing coordinate".to_owned(),
        });
    }
    Ok(cursor)
}

#[cfg(test)]
mod tests;
