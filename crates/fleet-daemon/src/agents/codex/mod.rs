//! The Codex harness: `codex app-server` over stdio.
//!
//! Fleet uses fifteen of Codex's 99 client methods and answers five of its eleven server
//! requests. Everything it does **not** use is a deliberate omission with a reason: `fs/*`,
//! `command/exec/*` and `process/*` because Fleet's daemon owns the filesystem, the PTYs and the
//! search and proxying them adds a hop and a second truth; `thread/shellCommand` because it
//! *"runs unsandboxed with full access"*; `thread/rollback` because it is deprecated and *"does
//! not revert local file changes"*, and Fleet's revert is git-based.

pub mod approvals;
pub mod envelope;
pub(crate) mod map;
pub mod methods;
mod params;
pub mod routing;
mod session;
pub mod tolerant;
pub mod transport;
pub mod wire;

#[cfg(test)]
mod tests;

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use fleet_core::agents::{
    AgentEvent, AgentKind, GateAnswer, GateId, HarnessCapabilities, PermissionMode, SessionState,
    TurnId,
};
use semver::Version;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use self::{
    params::{compact_params, interrupt_params, settings_params},
    session::{ApprovalShape, CodexSession, TurnControls},
    transport::Transport,
};
use crate::agents::harness::{
    Harness, HarnessConfig, HarnessError, HarnessEvents, HarnessResult, HarnessSink,
    InterruptReason, OpenSession, RuntimeApplied, RuntimeChange, RuntimeField, SessionOpened,
    ShutdownReason, Submit, Submitted, closed_events, probe::Probed, process,
};

/// Deadlines. The protocol has none, so every one of these is Fleet's, and each is strictly less
/// than the `fleet-proto` request timeout of the client call that triggers it.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);
const THREAD_DEADLINE: Duration = Duration::from_secs(20);
const TURN_DEADLINE: Duration = Duration::from_secs(15);
const STEER_DEADLINE: Duration = Duration::from_secs(5);
const CHILD_INTERRUPT_DEADLINE: Duration = Duration::from_secs(3);
const INTERRUPT_FANOUT_DEADLINE: Duration = Duration::from_secs(10);
const PARENT_INTERRUPT_DEADLINE: Duration = Duration::from_secs(5);
const SETTINGS_DEADLINE: Duration = Duration::from_secs(5);
const COMPACT_DEADLINE: Duration = Duration::from_secs(10);

/// How many child interrupts run at once.
const CHILD_INTERRUPT_CONCURRENCY: usize = 8;

/// The narrow "thread not found" set a resume may fall back to `thread/start` for.
///
/// A **malformed** resume response must *not* fall back: silently starting a fresh thread loses
/// the conversation.
const NOT_FOUND_PHRASES: [&str; 6] = [
    "not found",
    "missing thread",
    "no such thread",
    "unknown thread",
    "does not exist",
    "no rollout found",
];

/// One Codex session.
pub struct CodexHarness {
    config: HarnessConfig,
    probed: Probed,
    session: Arc<Mutex<CodexSession>>,
    transport: Option<Transport>,
    events: HarnessSink,
    receiver: Option<HarnessEvents>,
    capabilities: HarnessCapabilities,
}

impl CodexHarness {
    /// Builds an adapter for a probed Codex install. Nothing is spawned yet.
    #[must_use]
    pub fn new(config: HarnessConfig, probed: Probed) -> Self {
        let (events, receiver) = tokio::sync::mpsc::unbounded_channel();
        let session = CodexSession {
            version: Some(probed.version.clone()),
            ..CodexSession::default()
        };
        let capabilities = session.capabilities();
        Self {
            config,
            probed,
            session: Arc::new(Mutex::new(session)),
            transport: None,
            events,
            receiver: Some(receiver),
            capabilities,
        }
    }

    fn transport(&self) -> HarnessResult<&Transport> {
        self.transport.as_ref().ok_or(HarnessError::Exited {
            code: None,
            signal: None,
        })
    }

    /// The child environment. `CODEX_HOME` selects which `config.toml` Codex reads.
    async fn environment(
        &self,
        request: &OpenSession,
    ) -> std::collections::HashMap<std::ffi::OsString, std::ffi::OsString> {
        let inherited = process::login_environment(&request.start.worktree_path).await;
        let mut overrides: BTreeMap<String, String> = BTreeMap::new();
        if let Some(home) = &self.config.home {
            // Tilde-expanded manually: `spawn` performs no shell expansion, and
            // `CODEX_HOME=~/.codex_work` reaching Codex verbatim trips "that path does not
            // exist".
            overrides.insert(
                "CODEX_HOME".to_owned(),
                process::expand_home(home).to_string_lossy().into_owned(),
            );
        }
        overrides.extend(self.config.env.clone());
        overrides.extend(request.start.env.clone());
        overrides.insert("FLEET_SESSION".to_owned(), request.start.thread.to_string());
        overrides.insert("FLEET_TERMINAL".to_owned(), "codex".to_owned());
        process::filter_environment(
            inherited,
            crate::agents::harness::probe::strip_list(AgentKind::Codex),
            &overrides,
        )
    }

    /// Interrupts every live child first, then the parent.
    ///
    /// Interrupting only the parent leaves a subagent fleet running, and every deadline here
    /// exists because the transport awaits an unbounded response: a wedged child would otherwise
    /// block Stop forever — precisely during the runaway fleet where Stop matters most.
    async fn interrupt_fanout(&self, provider_turn: &str) -> HarnessResult<()> {
        let (children, root) = {
            let session = self.session.lock().await;
            (session.subagents.live_turns(), session.root.clone())
        };
        let transport = self.transport()?;
        let fanout = async {
            for batch in children.chunks(CHILD_INTERRUPT_CONCURRENCY) {
                for (thread, turn) in batch {
                    // Errors are ignored: a false-positive entry costs one ignored RPC, and a
                    // missing one costs a runaway agent.
                    if let Err(error) = transport
                        .request(
                            "turn/interrupt",
                            Some(interrupt_params(thread, turn)),
                            CHILD_INTERRUPT_DEADLINE,
                        )
                        .await
                    {
                        tracing::debug!(
                            target: "fleet::agents::codex",
                            %error,
                            "a Codex child did not answer its interrupt"
                        );
                    }
                }
            }
        };
        if tokio::time::timeout(INTERRUPT_FANOUT_DEADLINE, fanout)
            .await
            .is_err()
        {
            tracing::warn!(
                target: "fleet::agents::codex",
                "the Codex child interrupt fan-out exceeded its budget; interrupting the parent"
            );
        }
        let Some(root) = root else {
            return Ok(());
        };
        transport
            .request(
                "turn/interrupt",
                Some(interrupt_params(&root, provider_turn)),
                PARENT_INTERRUPT_DEADLINE,
            )
            .await
            .map(|_| ())
    }
}

#[async_trait]
impl Harness for CodexHarness {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn capabilities(&self) -> &HarnessCapabilities {
        &self.capabilities
    }

    async fn open(&mut self, req: OpenSession) -> HarnessResult<SessionOpened> {
        if self.transport.is_some() {
            return Err(HarnessError::Handshake {
                harness: AgentKind::Codex,
                detail: "the session is already open".to_owned(),
            });
        }
        if req.start.provider != AgentKind::Codex {
            return Err(HarnessError::Handshake {
                harness: AgentKind::Codex,
                detail: "the thread was created for another harness".to_owned(),
            });
        }
        let controls = TurnControls::from_mode(req.start.mode);
        {
            let mut session = self.session.lock().await;
            session.controls = TurnControls {
                model: req.start.model.as_ref().map(|model| model.model.clone()),
                effort: req
                    .start
                    .model
                    .as_ref()
                    .and_then(|model| model.effort.clone()),
                approval_policy: req.start.approval_policy.clone(),
                sandbox: req.start.sandbox,
                permission_profile: req.start.permission_profile.clone(),
                ..controls
            };
        }
        // `app-server` is always argv[1]; the user's own launch args come next, and Fleet's own
        // `-c key=value` overrides would go **last**, so last-write-wins config semantics favour
        // Fleet. Fleet never writes `config.toml`: all configuration is argv, and `CODEX_HOME`
        // selects which file Codex reads. There are no Fleet-injected overrides yet, which is
        // why the list below is the user's alone.
        let mut args = vec!["app-server".to_owned()];
        args.extend(process::tokenize_user_args("")?);
        let environment = self.environment(&req).await;
        let peer = process::spawn_child(
            AgentKind::Codex,
            &self.config.command,
            &args,
            &req.start.worktree_path,
            environment,
        )
        .await?;
        // The reader is running before `initialize` is sent, which is what satisfies "all
        // server-request handlers must be registered before `initialize`": the server may issue
        // a request as early as its reply to the first call.
        let transport = Transport::start(peer, Arc::clone(&self.session), self.events.clone());
        let mut suppress = true;
        let result = match transport
            .request(
                "initialize",
                Some(self.initialize_params(true)),
                HANDSHAKE_DEADLINE,
            )
            .await
        {
            Ok(result) => result,
            // A **timeout** is a handshake failure, not a capability rejection: retrying it
            // would double every dead-peer wait. A child that has already died says so more
            // precisely than the deadline does.
            Err(error @ HarnessError::Timeout { .. }) => {
                if !transport.is_alive().await {
                    return Err(HarnessError::Unavailable {
                        reason: "Codex is installed but exited during startup.".to_owned(),
                    });
                }
                return Err(HarnessError::Handshake {
                    harness: AgentKind::Codex,
                    detail: error.to_string(),
                });
            }
            Err(error) => {
                // A server that rejects the suppression capability is an older CLI: Fleet drops
                // the field and filters daemon-side, with one warning about event volume.
                tracing::warn!(
                    target: "fleet::agents::codex",
                    %error,
                    "Codex refused the notification suppression capability; \
                     filtering in the daemon instead, which costs event volume over a remote link"
                );
                suppress = false;
                transport
                    .request(
                        "initialize",
                        Some(self.initialize_params(false)),
                        HANDSHAKE_DEADLINE,
                    )
                    .await
                    .map_err(|error| HarnessError::Handshake {
                        harness: AgentKind::Codex,
                        detail: error.to_string(),
                    })?
            }
        };
        {
            let mut session = self.session.lock().await;
            session.suppression_accepted = suppress;
            session.user_agent = result
                .get("userAgent")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            // **There is no protocol version.** The CLI version is scraped from `userAgent`.
            session.version = session
                .user_agent
                .as_deref()
                .and_then(user_agent_version)
                .or_else(|| Some(self.probed.version.clone()));
            session.codex_home = result
                .get("codexHome")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
        }
        transport.notify("initialized", None).await?;

        let start_params = self.start_params(&req, &self.session.lock().await.controls.clone());
        let thread = match req.start.resume_cursor.as_deref() {
            Some(cursor) => {
                let mut params = start_params.clone();
                if let Some(object) = params.as_object_mut() {
                    object.insert("threadId".to_owned(), json!(cursor));
                    // Not in the generated params schema, so it goes through the raw request
                    // path; an older server may return history anyway, which the minimal-subset
                    // decode below survives.
                    object.insert("excludeTurns".to_owned(), Value::Bool(true));
                }
                match transport
                    .request("thread/resume", Some(params), THREAD_DEADLINE)
                    .await
                {
                    Ok(result) => result,
                    Err(error) if is_thread_missing(&error) => {
                        tracing::warn!(
                            target: "fleet::agents::codex",
                            "Codex no longer has this thread; starting a fresh one"
                        );
                        transport
                            .request("thread/start", Some(start_params), THREAD_DEADLINE)
                            .await?
                    }
                    Err(error) => return Err(error),
                }
            }
            None => {
                transport
                    .request("thread/start", Some(start_params), THREAD_DEADLINE)
                    .await?
            }
        };
        // Decoded through a **minimal** schema — `{cwd, model, thread:{id}}` — so an unrecognised
        // item in some historical turn cannot block resuming a perfectly valid thread.
        let Some(thread_id) = thread
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
        else {
            return Err(HarnessError::Handshake {
                harness: AgentKind::Codex,
                detail: "the thread response named no thread id".to_owned(),
            });
        };
        let model = thread
            .pointer("/thread/model")
            .or_else(|| thread.get("model"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        self.transport = Some(transport);
        let mut session = self.session.lock().await;
        session.root = Some(thread_id.clone());
        // Adopt `cwd` and `model` **from the response**: Codex may normalise or override what
        // the request asked for, and Fleet records what came back.
        if model.is_some() {
            session.controls.model = model;
        }
        self.capabilities = session.capabilities();
        let selection = session.model_selection();
        drop(session);
        transport::emit(
            &self.events,
            AgentEvent::SessionConfigured {
                provider: AgentKind::Codex,
                resume_cursor: Some(thread_id.clone()),
                model: selection.clone(),
                mode: req.start.mode,
                tools: Vec::new(),
                commands: Vec::new(),
                skills: Vec::new(),
            },
            Some("thread/start"),
        );
        transport::emit(
            &self.events,
            AgentEvent::SessionStateChanged(SessionState::Ready),
            Some("thread/start"),
        );
        Ok(SessionOpened {
            resume_cursor: Some(thread_id),
            model: selection,
            mode: Some(req.start.mode),
        })
    }

    async fn submit(&mut self, req: Submit) -> HarnessResult<Submitted> {
        let (thread, controls, active_provider_turn) = {
            let session = self.session.lock().await;
            (
                session.root.clone(),
                session.controls.clone(),
                session.active_provider_turn.clone(),
            )
        };
        let Some(thread) = thread else {
            return Err(HarnessError::Handshake {
                harness: AgentKind::Codex,
                detail: "the thread is not open".to_owned(),
            });
        };
        // The client's own id when it sent one: `clientUserMessageId` carries it to Codex, the
        // echo comes back under it, and the optimistic bubble the app already drew is the same
        // row for its whole life (§9.4).
        let user_item = req.input.item.unwrap_or_default();
        {
            let mut session = self.session.lock().await;
            session.remember_user_item(req.turn, user_item);
        }
        let transport = self.transport()?;
        let params = self.turn_params(&thread, &req.input, user_item, &controls);

        // The decision procedure is **answerable by the harness rather than guessed**: whenever a
        // turn is running, try to steer it, and fall back to a queued `turn/start` only on the
        // errors that say steering is impossible. The caller's `intent` records what it *meant*;
        // the wire form is the adapter's to choose, which is why a plain `Fresh` submit into a
        // running turn still steers rather than queueing behind it.
        if let Some(active) = active_provider_turn {
            tracing::debug!(
                target: "fleet::agents::codex",
                intent = ?req.intent,
                "steering the running Codex turn"
            );
            let steer = self.steer_params(&thread, &req.input, user_item, &active);
            match transport
                .request("turn/steer", Some(steer), STEER_DEADLINE)
                .await
            {
                Ok(_) => {
                    let mut session = self.session.lock().await;
                    let turn = session.turn_for(&active);
                    session.remember_user_item(turn, user_item);
                    return Ok(Submitted { turn, queued: true });
                }
                Err(error) if is_not_steerable(&error) => {
                    // Silently: the user never sees this one.
                    tracing::debug!(
                        target: "fleet::agents::codex",
                        "the running Codex turn is not steerable; queueing a turn instead"
                    );
                }
                Err(error) => return Err(error),
            }
        }

        let result = transport
            .request("turn/start", Some(params), TURN_DEADLINE)
            .await?;
        let provider_turn = result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let mut session = self.session.lock().await;
        // The caller's own id stays the turn's identity, whatever Codex named it.
        if !provider_turn.is_empty() {
            session.alias_turn(&provider_turn, req.turn);
        }
        let queued = session.active_turn.is_some() && session.active_turn != Some(req.turn);
        if !queued {
            session.begin_turn(req.turn);
            session.adopt_turn(req.turn, &provider_turn);
        }
        drop(session);
        // `turn/started` follows and is authoritative; announcing the turn here is what keeps the
        // composer honest when the notification is a few milliseconds behind the response.
        if !queued {
            transport::emit(
                &self.events,
                AgentEvent::TurnStarted {
                    turn: req.turn,
                    user_item,
                },
                Some("turn/start"),
            );
        }
        Ok(Submitted {
            turn: req.turn,
            queued,
        })
    }

    async fn interrupt(&mut self, turn: TurnId, reason: InterruptReason) -> HarnessResult<()> {
        let provider_turn = {
            let session = self.session.lock().await;
            match session.active_turn {
                Some(active) if active == turn => session.active_provider_turn.clone(),
                // `turn/interrupt` **only accepts the id that is active now**, so a Stop for a
                // queued turn is not an error the caller can act on.
                _ => None,
            }
        };
        let Some(provider_turn) = provider_turn else {
            tracing::debug!(
                target: "fleet::agents::codex",
                ?reason,
                "ignoring an interrupt for a Codex turn that is not running"
            );
            return Ok(());
        };
        self.interrupt_fanout(&provider_turn).await
    }

    async fn answer_gate(&mut self, gate: GateId, answer: GateAnswer) -> HarnessResult<()> {
        let pending = self.session.lock().await.take_gate(gate)?;
        // `approvalId`, `itemId` and `threadId` are **display correlation only** — the gate's own
        // key is the JSON-RPC request id — so they are logged and never keyed on.
        tracing::debug!(
            target: "fleet::agents::codex",
            thread = %pending.thread,
            item = pending.item.as_deref().unwrap_or("-"),
            "answering a Codex gate"
        );
        // An async question has no request behind it: its answer is a message, and the caller
        // sends it with `submit(intent: AnswerAsMessage)`.
        if matches!(pending.shape, ApprovalShape::AsyncQuestions { .. }) {
            transport::emit(
                &self.events,
                AgentEvent::GateResolved {
                    gate,
                    answer,
                    by: fleet_core::agents::GateResolver::User,
                },
                Some("answer_gate"),
            );
            return Ok(());
        }
        let result = approvals::answer(&pending, gate, &answer)?;
        let transport = self.transport()?;
        transport::write_result(transport.writer(), &pending.request_id, result).await;
        transport::emit(
            &self.events,
            AgentEvent::GateResolved {
                gate,
                answer,
                by: fleet_core::agents::GateResolver::User,
            },
            Some("answer_gate"),
        );
        Ok(())
    }

    async fn apply_runtime(&mut self, change: RuntimeChange) -> HarnessResult<RuntimeApplied> {
        let mut applied = std::collections::BTreeSet::new();
        let thread = {
            let mut session = self.session.lock().await;
            if let Some(model) = &change.model {
                session.controls.model = Some(model.model.clone());
                applied.insert(RuntimeField::Model);
                if let Some(effort) = &model.effort {
                    session.controls.effort = Some(effort.clone());
                    applied.insert(RuntimeField::Effort);
                }
            }
            if let Some(mode) = change.mode {
                let controls = TurnControls::from_mode(mode);
                session.controls.approval_policy = controls.approval_policy;
                session.controls.sandbox = controls.sandbox;
                session.controls.reviewer = controls.reviewer;
                applied.insert(RuntimeField::Mode);
                applied.insert(RuntimeField::Sandbox);
                applied.insert(RuntimeField::ApprovalPolicy);
            }
            if let Some(sandbox) = change.sandbox {
                session.controls.sandbox = sandbox;
                applied.insert(RuntimeField::Sandbox);
            }
            if let Some(policy) = change.approval_policy.clone() {
                session.controls.approval_policy = policy;
                applied.insert(RuntimeField::ApprovalPolicy);
            }
            if let Some(profile) = change.permission_profile.clone() {
                session.controls.permission_profile = profile;
                applied.insert(RuntimeField::PermissionProfile);
            }
            session.root.clone()
        };
        if applied.is_empty() {
            return Ok(RuntimeApplied::default());
        }
        // `thread/settings/update` is explicitly "for subsequent turns", and every override is
        // re-sent on each `turn/start` anyway, so nothing restarts and nothing applies mid-turn.
        if let Some(thread) = thread {
            let controls = self.session.lock().await.controls.clone();
            if let Err(error) = self
                .transport()?
                .request(
                    "thread/settings/update",
                    Some(settings_params(&thread, &controls)),
                    SETTINGS_DEADLINE,
                )
                .await
            {
                tracing::warn!(
                    target: "fleet::agents::codex",
                    %error,
                    "Codex refused a settings update; the next turn re-asserts it"
                );
            }
        }
        Ok(RuntimeApplied {
            applies_next_turn: applied,
            applied_now: std::collections::BTreeSet::new(),
            restart: None,
        })
    }

    async fn compact(&mut self) -> HarnessResult<()> {
        let Some(thread) = self.session.lock().await.root.clone() else {
            return Ok(());
        };
        // Native, unlike Claude: completion arrives as a `contextCompaction` item, so there is
        // nothing to synthesise and nothing to watch for.
        self.transport()?
            .request(
                "thread/compact/start",
                Some(compact_params(&thread)),
                COMPACT_DEADLINE,
            )
            .await
            .map(|_| ())
    }

    async fn shutdown(&mut self, reason: ShutdownReason) -> HarnessResult<()> {
        let Some(transport) = self.transport.take() else {
            return Ok(());
        };
        transport.expect_stop();
        // 1. Settle every pending gate **before** killing the process, so the in-flight request
        //    handlers actually write a JSON-RPC response instead of being interrupted mid-flight.
        let pending = self.session.lock().await.drain_gates();
        for (gate, approval) in pending {
            if !matches!(approval.shape, ApprovalShape::AsyncQuestions { .. }) {
                let cancel = match approval.shape {
                    ApprovalShape::Questions { .. } => json!({"answers": {}}),
                    ApprovalShape::Elicitation => json!({"action": "cancel"}),
                    ApprovalShape::Permissions { .. } => {
                        json!({"permissions": {}, "scope": "turn"})
                    }
                    _ => json!({"decision": "cancel"}),
                };
                transport::write_result(transport.writer(), &approval.request_id, cancel).await;
            }
            transport::emit(
                &self.events,
                AgentEvent::GateResolved {
                    gate,
                    answer: CodexSession::closed_answer(&approval.shape),
                    by: fleet_core::agents::GateResolver::ProviderClosed,
                },
                Some("shutdown"),
            );
        }
        // 2. Mark the session closed and clear the active turn.
        let mut events = Vec::new();
        {
            let mut session = self.session.lock().await;
            if let Some(turn) = session.active_turn {
                session.close_turn_items(
                    turn,
                    fleet_core::agents::ItemStatus::Stopped,
                    &mut events,
                );
                events.push(AgentEvent::TurnAborted {
                    turn,
                    reason: match reason {
                        ShutdownReason::Restart { .. } => {
                            fleet_core::agents::AbortReason::Superseded
                        }
                        ShutdownReason::Fatal(_) => fleet_core::agents::AbortReason::ProviderExited,
                        ShutdownReason::User | ShutdownReason::Replaced => {
                            fleet_core::agents::AbortReason::SessionStopped
                        }
                    },
                });
                session.settle_turn(turn);
            }
        }
        events.push(AgentEvent::SessionStateChanged(SessionState::Stopped));
        for event in events {
            transport::emit(&self.events, event, Some("shutdown"));
        }
        // 3-4. Announce, then close the transport scope, which kills the process.
        let code = transport.terminate().await;
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

impl CodexHarness {
    /// How many unroutable lines this connection has seen.
    ///
    /// An unroutable line is a **counted warning, not a session kill**: t3code terminates the
    /// connection on one, and that is wrong when the protocol shares stdout with anything that
    /// might ever write a stray line.
    #[cfg(test)]
    pub(super) fn unroutable_lines(&self) -> u64 {
        self.transport
            .as_ref()
            .map_or(0, Transport::unroutable_lines)
    }
}

/// The semver inside a `userAgent` string.
#[must_use]
pub fn user_agent_version(user_agent: &str) -> Option<Version> {
    let after_slash = user_agent.split('/').nth(1)?;
    let token = after_slash.split_whitespace().next()?;
    Version::parse(token).ok()
}

/// Whether an error is the narrow "thread not found" set a resume may fall back for.
fn is_thread_missing(error: &HarnessError) -> bool {
    let HarnessError::Request { detail, .. } = error else {
        return false;
    };
    let lower = detail.to_ascii_lowercase();
    lower.contains("thread")
        && NOT_FOUND_PHRASES
            .iter()
            .any(|phrase| lower.contains(phrase))
}

/// Whether an error says the running turn cannot be steered.
fn is_not_steerable(error: &HarnessError) -> bool {
    let HarnessError::Request { detail, .. } = error else {
        return false;
    };
    let lower = detail.to_ascii_lowercase();
    lower.contains("activeturnnotsteerable")
        || lower.contains("not steerable")
        || lower.contains("already finished")
        || lower.contains("no active turn")
}

/// The Fleet mode a Codex approval policy and sandbox pair present as.
#[must_use]
pub fn mode_of(controls_policy: &str, sandbox: &str) -> PermissionMode {
    match (controls_policy, sandbox) {
        ("never", _) | (_, "danger-full-access") => PermissionMode::FullAccess,
        ("on-request", _) => PermissionMode::AcceptEdits,
        _ => PermissionMode::Ask,
    }
}
