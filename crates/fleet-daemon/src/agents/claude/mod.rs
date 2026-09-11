//! The Claude Code harness: `claude -p` over bidirectional stream-json.
//!
//! There is no Rust Agent SDK and there will be no Node sidecar: `fleetd` has no Node dependency
//! and is not acquiring one for a single NDJSON format, so this module is the ~600 lines the SDK
//! was hiding.

pub mod argv;
pub mod frames;
pub(crate) mod map;
mod session;
mod transport;

#[cfg(test)]
mod tests;

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use fleet_core::agents::{
    AbortReason, AgentEvent, AgentKind, AttachmentSource, GateAnswer, GateId, HarnessCapabilities,
    ItemId, ItemStatus, SessionState, TurnId, UserInput,
};
use serde_json::{Value, json};
use tokio::{sync::Mutex, task::JoinHandle};
use uuid::Uuid;

use self::{session::ClaudeSession, transport::Transport};
use crate::agents::harness::{
    Harness, HarnessConfig, HarnessError, HarnessEvents, HarnessResult, HarnessSink,
    InterruptReason, OpenSession, RestartPlan, RuntimeApplied, RuntimeChange, RuntimeField,
    SessionOpened, ShutdownReason, Submit, SubmitIntent, Submitted, closed_events, probe::Probed,
    process,
};

/// How long `open` waits for the process to prove it can run.
///
/// **A divergence from spec A.2.3, deliberately.** The spec's barrier is `system/init`, but
/// `claude -p --input-format stream-json` publishes `system/init` only once it has been
/// *prompted*: waiting for it would stall every thread start until the user's first message.
/// So the barrier is "the child survived its own startup", and `system/init` is mapped normally
/// when it arrives — including its capability gate. A child that dies inside this window is a
/// handshake failure with the terminal fallback, which is the case the spec's deadline exists for.
const SPAWN_WINDOW: Duration = Duration::from_millis(750);

/// How long an interrupt receipt is worth waiting for. A nicety, never a barrier.
const RECEIPT_DEADLINE: Duration = Duration::from_secs(2);

/// How long the interrupted turn's own `result` is waited for before escalating.
const RESULT_DEADLINE: Duration = Duration::from_secs(10);

/// The composer limits, copied verbatim from the numbers a real deployment settled on.
const MAX_INPUT_CHARS: usize = 120_000;
const MAX_ATTACHMENTS: usize = 8;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// The image media types the CLI accepts. Anything else is a request error and the turn never
/// starts, rather than a turn that silently drops the attachment.
const IMAGE_TYPES: [&str; 4] = ["image/gif", "image/jpeg", "image/png", "image/webp"];

/// One Claude Code session.
pub struct ClaudeHarness {
    config: HarnessConfig,
    probed: Probed,
    session: Arc<Mutex<ClaudeSession>>,
    transport: Option<Transport>,
    events: HarnessSink,
    receiver: Option<HarnessEvents>,
    capabilities: HarnessCapabilities,
    /// The interrupt escalation watchdog, held so it is never a bare detach.
    escalation: Option<JoinHandle<()>>,
    /// The session id Fleet minted for a fresh session.
    minted_session: Uuid,
}

impl ClaudeHarness {
    /// Builds an adapter for a probed Claude install. Nothing is spawned yet.
    #[must_use]
    pub fn new(config: HarnessConfig, probed: Probed) -> Self {
        let (events, receiver) = tokio::sync::mpsc::unbounded_channel();
        let session = ClaudeSession::default();
        let capabilities = session.capabilities(probed.version.clone());
        Self {
            config,
            probed,
            session: Arc::new(Mutex::new(session)),
            transport: None,
            events,
            receiver: Some(receiver),
            capabilities,
            escalation: None,
            minted_session: Uuid::new_v4(),
        }
    }

    fn transport(&self) -> HarnessResult<&Transport> {
        self.transport.as_ref().ok_or(HarnessError::Exited {
            code: None,
            signal: None,
        })
    }

    /// The child environment: the login shell's, minus Claude's own inherited variables.
    async fn environment(
        &self,
        request: &OpenSession,
    ) -> std::collections::HashMap<std::ffi::OsString, std::ffi::OsString> {
        let inherited = process::login_environment(&request.start.worktree_path).await;
        let mut overrides: BTreeMap<String, String> = BTreeMap::new();
        // Per-instance config isolates through `CLAUDE_CONFIG_DIR`, **never** by overriding
        // `HOME`: on macOS that relocates the login keychain and the CLI reports "Not logged in".
        if let Some(home) = &self.config.home {
            overrides.insert(
                "CLAUDE_CONFIG_DIR".to_owned(),
                process::expand_home(home).to_string_lossy().into_owned(),
            );
        }
        overrides.extend(self.config.env.clone());
        overrides.extend(request.start.env.clone());
        overrides.insert("FLEET_SESSION".to_owned(), request.start.thread.to_string());
        overrides.insert("FLEET_TERMINAL".to_owned(), "claude".to_owned());
        process::filter_environment(
            inherited,
            crate::agents::harness::probe::strip_list(AgentKind::Claude),
            &overrides,
        )
    }

    /// Writes the interrupt control request, respecting the declared capabilities.
    async fn write_interrupt(&self) -> HarnessResult<()> {
        let (cancel_queued, expect_receipt) = {
            let session = self.session.lock().await;
            (
                session.can_cancel_queued(),
                session.expects_interrupt_receipt(),
            )
        };
        let request_id = Uuid::new_v4().to_string();
        let mut request = json!({"subtype": "interrupt"});
        // Without `interrupt_cancel_queued_v1` the flag is omitted and the Stop affordance says
        // that queued messages will still run.
        if cancel_queued && let Some(object) = request.as_object_mut() {
            object.insert("cancel_queued".to_owned(), Value::Bool(true));
        }
        let frame = json!({
            "type": "control_request",
            "request_id": request_id,
            "request": request,
        });
        let receipt = self.transport()?.control_request(request_id, frame).await?;
        if expect_receipt {
            // A receipt is a nicety: absent one, Fleet carries on and relies on the `result`.
            match tokio::time::timeout(RECEIPT_DEADLINE, receipt).await {
                Ok(Ok(response)) if response.subtype == "error" => tracing::warn!(
                    target: "fleet::agents::claude",
                    "Claude refused the interrupt; escalating on the result deadline"
                ),
                Ok(_) => {}
                Err(_) => tracing::debug!(
                    target: "fleet::agents::claude",
                    "no interrupt receipt inside the deadline; relying on the result"
                ),
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Harness for ClaudeHarness {
    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn capabilities(&self) -> &HarnessCapabilities {
        &self.capabilities
    }

    async fn open(&mut self, req: OpenSession) -> HarnessResult<SessionOpened> {
        if self.transport.is_some() {
            return Err(HarnessError::Handshake {
                harness: AgentKind::Claude,
                detail: "the session is already open".to_owned(),
            });
        }
        if req.start.provider != AgentKind::Claude {
            return Err(HarnessError::Handshake {
                harness: AgentKind::Claude,
                detail: "the thread was created for another harness".to_owned(),
            });
        }
        {
            let mut session = self.session.lock().await;
            session.launched_effort = req
                .start
                .model
                .as_ref()
                .and_then(|model| model.effort.clone());
            session.cursor = argv::resume_cursor(req.start.resume_cursor.as_ref())
                // The cursor is durable *before the CLI speaks*: a crash between spawn and
                // `system/init` still leaves a resumable thread.
                .or_else(|| Some(self.minted_session.to_string()));
        }
        let args = argv::launch_args(&argv::Launch {
            start: &req.start,
            session_id: self.minted_session,
            attachments_dir: self.config.attachments_dir.as_deref(),
            user_args: "",
        })?;
        let environment = self.environment(&req).await;
        let peer = process::spawn_child(
            AgentKind::Claude,
            &self.config.command,
            &args,
            &req.start.worktree_path,
            environment,
        )
        .await?;
        let transport = Transport::start(peer, Arc::clone(&self.session), self.events.clone());
        // The barrier: the child either publishes `system/init` or at least survives its own
        // startup. A child that dies here is a handshake failure, not a failed turn.
        let deadline = std::time::Instant::now() + SPAWN_WINDOW;
        loop {
            if self.session.lock().await.initialized {
                break;
            }
            if !transport.is_alive().await {
                return Err(HarnessError::Handshake {
                    harness: AgentKind::Claude,
                    detail: "the process exited during startup".to_owned(),
                });
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        self.transport = Some(transport);
        let session = self.session.lock().await;
        // `system/init` may not have landed yet (it lands with the first prompt), so the gate is
        // enforced both here and, when it lands later, by the mapper.
        if session.initialized && !session.declared.contains(session::CAPABILITY_MSG_LIFECYCLE) {
            return Err(HarnessError::Unavailable {
                reason: format!(
                    "Claude Code {} is too old for Fleet's agent tab. Upgrade to a build that declares `{}`.",
                    self.probed.version,
                    session::CAPABILITY_MSG_LIFECYCLE
                ),
            });
        }
        self.capabilities = session.capabilities(self.probed.version.clone());
        Ok(SessionOpened {
            resume_cursor: session.cursor.clone(),
            model: req.start.model.clone(),
            mode: None,
        })
    }

    async fn submit(&mut self, req: Submit) -> HarnessResult<Submitted> {
        let frame = user_frame(&req.input)?;
        let user_item = req.input.item.unwrap_or_default();
        // Resolved before the turn is announced: a harness that is not running must fail the
        // submit without having started anything.
        let writer = self.transport()?.writer.clone();
        let announced = {
            let mut session = self.session.lock().await;
            match req.intent {
                // A steer joins the running turn; the CLI coalesces it and reports how many were
                // folded in on the settling `result`.
                SubmitIntent::Steer | SubmitIntent::AnswerAsMessage => {
                    session.record_steer(req.turn);
                    None
                }
                // The client's own id when it sent one, so the optimistic bubble it already
                // drew and the transcript row are one item for the row's whole life (§9.4).
                SubmitIntent::Fresh => session.begin_turn(req.turn, user_item)?,
            }
        };
        // `TurnStarted` reaches the channel *before* the write, or the opening item of a fast
        // turn can overtake it and be rejected for naming a turn nothing has started.
        let queued = announced.is_none();
        if let Some(event) = announced {
            transport::emit(&self.events, event, Some("submit"));
        }
        if let Err(error) = writer.write(frame).await {
            if !queued {
                let mut session = self.session.lock().await;
                session.rollback_turn_start(req.turn);
                drop(session);
                transport::emit(
                    &self.events,
                    AgentEvent::TurnAborted {
                        turn: req.turn,
                        reason: AbortReason::Other(
                            "the prompt could not be written to Claude Code".to_owned(),
                        ),
                    },
                    Some("submit"),
                );
            }
            return Err(error);
        }
        Ok(Submitted {
            turn: req.turn,
            queued,
        })
    }

    async fn interrupt(&mut self, turn: TurnId, reason: InterruptReason) -> HarnessResult<()> {
        {
            let mut session = self.session.lock().await;
            // Idempotent: interrupting a turn that is not running is not an error the caller can
            // act on, and Stop is pressed twice all the time.
            if session.active_turn() != Some(turn) {
                return Ok(());
            }
            session.mark_interrupted(turn)?;
        }
        self.write_interrupt().await?;
        // The ladder: the interrupted turn still emits its own `result`, and that is what settles
        // it. Only when it does not does Fleet escalate — and it settles the turn itself, so the
        // thread never spins on a process that stopped answering.
        let session = Arc::clone(&self.session);
        let events = self.events.clone();
        let writer = self.transport()?.writer.clone();
        if let Some(previous) = self.escalation.take() {
            previous.abort();
        }
        self.escalation = Some(tokio::spawn(async move {
            tokio::time::sleep(RESULT_DEADLINE).await;
            let still_running = session.lock().await.active_turn() == Some(turn);
            if !still_running {
                return;
            }
            tracing::warn!(
                target: "fleet::agents::claude",
                %turn,
                ?reason,
                "Claude did not answer the interrupt inside its deadline; escalating"
            );
            if let Err(error) = writer.close().await {
                tracing::debug!(
                    target: "fleet::agents::claude",
                    %error,
                    "could not close Claude's stdin while escalating"
                );
            }
            let mut locked = session.lock().await;
            let mut settled = Vec::new();
            let open = locked
                .open_items
                .iter()
                .filter(|(_, item)| item.turn == turn)
                .map(|(item, _)| *item)
                .collect::<Vec<_>>();
            for item in open {
                locked.complete_item(item, ItemStatus::Stopped, &mut settled);
            }
            locked.active_turn = None;
            locked.pending_start = None;
            drop(locked);
            for event in settled {
                transport::emit(&events, event, Some("interrupt_escalation"));
            }
            transport::emit(
                &events,
                AgentEvent::TurnAborted {
                    turn,
                    reason: AbortReason::User,
                },
                Some("interrupt_escalation"),
            );
        }));
        Ok(())
    }

    async fn answer_gate(&mut self, gate: GateId, answer: GateAnswer) -> HarnessResult<()> {
        let response = {
            let session = self.session.lock().await;
            map::gates::response_for(&session, gate, &answer)?
        };
        if let Some(response) = response {
            self.transport()?.writer.write(response).await?;
        }
        let resolved = self.session.lock().await.resolve_gate(gate, answer)?;
        transport::emit(&self.events, resolved, Some("answer_gate"));
        Ok(())
    }

    async fn apply_runtime(&mut self, change: RuntimeChange) -> HarnessResult<RuntimeApplied> {
        // Claude's model, effort and permission mode are **launch flags**, and the in-session
        // control-request subtypes are not proven by any capture Fleet holds: shipping a guess
        // means a silently-ignored control. Every one of them is therefore a restart with the
        // resume cursor, executed by the manager at a turn boundary.
        let mut fields = std::collections::BTreeSet::new();
        if let Some(model) = &change.model {
            fields.insert(RuntimeField::Model);
            if model.effort.is_some() {
                fields.insert(RuntimeField::Effort);
            }
        }
        if change.mode.is_some() {
            fields.insert(RuntimeField::Mode);
        }
        // Codex's three axes have no Claude expression at all: `--add-dir` grants scope and there
        // is no sandbox. Reporting them as applied would be a lie.
        if change.sandbox.is_some() || change.approval_policy.is_some() {
            tracing::debug!(
                target: "fleet::agents::claude",
                "Claude has no sandbox or approval-policy axis; ignoring that part of the change"
            );
        }
        if fields.is_empty() {
            return Ok(RuntimeApplied::default());
        }
        Ok(RuntimeApplied {
            applied_now: std::collections::BTreeSet::new(),
            applies_next_turn: std::collections::BTreeSet::new(),
            restart: Some(RestartPlan {
                resume: true,
                fields,
            }),
        })
    }

    async fn compact(&mut self) -> HarnessResult<()> {
        // Claude has no compaction RPC: `/compact` is an ordinary turn. The boundary frame is
        // watched for, and a turn that settles without one gets a synthesised boundary so the
        // thread stops claiming it is compacting.
        let turn = TurnId::new();
        let announced = {
            let mut session = self.session.lock().await;
            let announced = session.begin_turn(turn, ItemId::new())?;
            session.compacting = Some(session.active_turn().unwrap_or(turn));
            announced
        };
        if let Some(event) = announced {
            transport::emit(&self.events, event, Some("compact"));
        }
        self.transport()?
            .writer
            .write(user_frame(&UserInput {
                text: "/compact".to_owned(),
                ..UserInput::default()
            })?)
            .await
    }

    async fn shutdown(&mut self, reason: ShutdownReason) -> HarnessResult<()> {
        let Some(transport) = self.transport.take() else {
            return Ok(());
        };
        if let Some(escalation) = self.escalation.take() {
            escalation.abort();
        }
        transport.expect_stop();
        // 1. Write the interrupt and close stdin **first**: termination is scheduled before any
        //    cleanup that could itself wait on the harness.
        let active = self.session.lock().await.active_turn();
        if active.is_some()
            && let Err(error) = self.write_interrupt_on(&transport).await
        {
            tracing::debug!(
                target: "fleet::agents::claude",
                %error,
                "could not interrupt Claude while shutting down"
            );
        }
        if let Err(error) = transport.writer.close().await {
            tracing::debug!(
                target: "fleet::agents::claude",
                %error,
                "could not close Claude's stdin while shutting down"
            );
        }
        // 2-5. Settle every gate, then every open item, then the open turn.
        let mut events = Vec::new();
        {
            let mut session = self.session.lock().await;
            events.extend(
                session.settle_open_gates(fleet_core::agents::GateResolver::ProviderClosed),
            );
            let open = session.open_items.keys().copied().collect::<Vec<_>>();
            for item in open {
                session.complete_item(item, ItemStatus::Stopped, &mut events);
            }
            if let Some(turn) = session.active_turn() {
                events.push(AgentEvent::TurnAborted {
                    turn,
                    reason: match reason {
                        ShutdownReason::Restart { .. } => AbortReason::Superseded,
                        ShutdownReason::User | ShutdownReason::Replaced => {
                            AbortReason::SessionStopped
                        }
                        ShutdownReason::Fatal(_) => AbortReason::ProviderExited,
                    },
                });
            }
            session.active_turn = None;
            session.pending_start = None;
        }
        events.push(AgentEvent::SessionStateChanged(SessionState::Stopped));
        for event in events {
            transport::emit(&self.events, event, Some("shutdown"));
        }
        // 6. SIGTERM, then SIGKILL.
        let code = transport.terminate().await;
        // 7. `SessionExited` last. Nothing follows it.
        transport::emit(
            &self.events,
            AgentEvent::SessionExited {
                code,
                expected: true,
            },
            Some("shutdown"),
        );
        Ok(())
    }

    fn events(&mut self) -> HarnessEvents {
        self.receiver.take().unwrap_or_else(closed_events)
    }
}

impl ClaudeHarness {
    /// The interrupt write, against a transport this call already owns.
    async fn write_interrupt_on(&self, transport: &Transport) -> HarnessResult<()> {
        let cancel_queued = self.session.lock().await.can_cancel_queued();
        let request_id = Uuid::new_v4().to_string();
        let mut request = json!({"subtype": "interrupt"});
        if cancel_queued && let Some(object) = request.as_object_mut() {
            object.insert("cancel_queued".to_owned(), Value::Bool(true));
        }
        transport
            .writer
            .write(json!({
                "type": "control_request",
                "request_id": request_id,
                "request": request,
            }))
            .await
    }
}

/// The `user` frame one submission becomes.
///
/// **Block order is load-bearing and is not negotiable:** `[images] … [final text]`. The final
/// text block goes **last** because the CLI only reads a streamed user message as a slash-command
/// invocation when the last content block is text; leading with the text made every
/// image-carrying turn fall back to a plain prompt and a hand-typed `/skill args` reach the model
/// unexpanded.
pub(crate) fn user_frame(input: &UserInput) -> HarnessResult<Value> {
    let request_error = |detail: String| HarnessError::Request {
        method: "user".to_owned(),
        code: None,
        detail,
    };
    if input.text.chars().count() > MAX_INPUT_CHARS {
        return Err(request_error(format!(
            "The message is longer than {MAX_INPUT_CHARS} characters."
        )));
    }
    if input.attachments.len() > MAX_ATTACHMENTS {
        return Err(request_error(format!(
            "A message may carry at most {MAX_ATTACHMENTS} attachments."
        )));
    }
    let mut text = input.text.clone();
    let mut images = Vec::new();
    for attachment in &input.attachments {
        let is_image = IMAGE_TYPES.contains(&attachment.media_type.as_str());
        match (&attachment.source, is_image) {
            (AttachmentSource::Base64(data), true) => {
                // Base64 is 4 bytes per 3 bytes of payload.
                if data.len() / 4 * 3 > MAX_IMAGE_BYTES {
                    return Err(request_error(format!(
                        "An image attachment is larger than {} MiB.",
                        MAX_IMAGE_BYTES / (1024 * 1024)
                    )));
                }
                images.push(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": attachment.media_type,
                        "data": data,
                    }
                }));
            }
            // Non-image attachments reach the agent as absolute paths in the prompt text, which
            // is why the attachments directory has to be in `--add-dir`.
            (AttachmentSource::Path(path), _) => {
                text.push_str(&format!("\n{}", path.display()));
            }
            (AttachmentSource::Url(url), _) => {
                text.push_str(&format!("\n{url}"));
            }
            (AttachmentSource::Base64(_), false) => {
                return Err(request_error(format!(
                    "Claude Code cannot read a `{}` attachment inline; only {} are accepted as images.",
                    attachment.media_type,
                    IMAGE_TYPES.join(", ")
                )));
            }
        }
    }
    let content = if images.is_empty() {
        Value::String(text)
    } else {
        let mut blocks = images;
        blocks.push(json!({"type": "text", "text": text}));
        Value::Array(blocks)
    };
    Ok(json!({
        "type": "user",
        "session_id": "",
        "parent_tool_use_id": null,
        "message": {"role": "user", "content": content},
    }))
}
