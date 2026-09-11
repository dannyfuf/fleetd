//! The manager's view of a harness: the `AgentProvider` seam, and the bridge onto `Harness`.
//!
//! The real adapters live in [`crate::agents`] behind the `Harness` trait of
//! `NATIVE-AGENTS.md` §3.1. This module is the one place the two shapes meet: the manager still
//! speaks the verbs it was written against (`start`, `send`, `respond`, `apply_runtime`,
//! `restart`, `stop`), and [`HarnessProvider`] maps each onto the harness contract.
//!
//! Three answers the harness gives are carried through rather than collapsed, because the
//! manager is the only thing that can act on them (§3.1):
//!
//! - [`RuntimeApplied::restart`] is **reported, not performed**. Claude's model, effort and mode
//!   are launch flags whose cost is a restart with the resume cursor, and only the manager knows
//!   whether a turn is running — §7's "nothing applies mid-turn". So `apply_runtime` answers what
//!   the change would cost and [`AgentProvider::restart`] is a separate verb the manager calls at
//!   a turn boundary.
//! - [`Submitted`] is returned to the caller, so the turn the message actually landed in and
//!   whether it joined a running one are the harness's answers rather than the manager's guess.
//! - [`HarnessEvent::emitted_at`] — Codex's `emittedAtMs`, the only server-side clock Fleet gets —
//!   is not on [`fleet_core::agents::SeqEvent`] and would be a wire field with no reader, so it is
//!   turned into the one thing it can be used for here: a skew measurement logged against the
//!   moment the daemon read the frame (§9.4).

use std::time::Duration;

use async_trait::async_trait;
use fleet_core::{
    agents::{
        AgentEvent, AgentKind, GateAnswer, GateId, HarnessCapabilities, StartRequest, TurnId,
        UserInput,
    },
    config::AgentCommands,
};
use thiserror::Error;
use tokio::task::JoinHandle;

use crate::agents::harness::{
    self, Harness, HarnessConfig, HarnessError, HarnessEvent, HarnessEvents, InterruptReason,
    OpenSession, RestartPlan, RuntimeApplied, RuntimeChange, ShutdownReason, Submit, SubmitIntent,
    Submitted, probe::ProbeCache,
};

/// One normalized event and the provider wire type that produced it.
///
/// The stored `SeqEvent.raw` is the mitigation for protocol drift, so the adapter has to carry
/// the harness's own name for the frame across the channel; the reducer is the only writer of the
/// stored record and cannot invent it afterwards.
#[derive(Debug, Clone)]
pub struct ProviderEvent {
    /// The normalized event.
    pub event: AgentEvent,
    /// The provider's own name for the message this event was mapped from.
    pub raw: Option<String>,
    /// How far behind the harness's own emission clock this frame was read, when it published
    /// one. `None` for Claude, which publishes no clock, and for an adapter-authored event.
    pub emission_skew: Option<Duration>,
}

impl ProviderEvent {
    /// One event mapped from a named provider message.
    #[must_use]
    pub fn new(event: AgentEvent, raw: Option<&str>) -> Self {
        Self {
            event,
            raw: raw.map(ToOwned::to_owned),
            emission_skew: None,
        }
    }
}

impl From<AgentEvent> for ProviderEvent {
    /// An event the adapter produced itself, with no provider message behind it.
    fn from(event: AgentEvent) -> Self {
        Self {
            event,
            raw: None,
            emission_skew: None,
        }
    }
}

/// Send side of one provider's normalized event stream.
///
/// The stream is unbounded on purpose. Every manager verb holds the thread's operation gate
/// across the adapter call and the only consumer takes that same gate to reduce an event: a
/// bounded channel that filled up during a streaming turn would park the adapter inside the verb
/// that holds the gate and park the drain on the gate the verb holds — a deadlock no timeout
/// recovers from.
pub type ProviderSink = tokio::sync::mpsc::UnboundedSender<ProviderEvent>;

/// Receive side of one provider's normalized event stream.
pub type ProviderEvents = tokio::sync::mpsc::UnboundedReceiver<ProviderEvent>;

/// Result returned by provider control operations.
pub type ProviderResult<T> = Result<T, ProviderError>;

/// Provider lifecycle and command adapter.
#[async_trait]
pub trait AgentProvider: Send {
    /// Provider represented by this adapter.
    fn kind(&self) -> AgentKind;
    /// Operations implemented by this adapter/version pair.
    fn capabilities(&self) -> HarnessCapabilities;
    /// Starts a fresh or resumed provider session.
    async fn start(&mut self, req: StartRequest) -> ProviderResult<()>;
    /// Sends or steers user input for a turn, answering what the harness did with it.
    async fn send(&mut self, turn: TurnId, input: UserInput) -> ProviderResult<Submitted>;
    /// Requests interruption of a turn; the later terminal provider event remains authoritative.
    async fn interrupt(&mut self, turn: TurnId) -> ProviderResult<()>;
    /// Maps a normalized gate answer back to the provider protocol.
    async fn respond(&mut self, gate: GateId, answer: GateAnswer) -> ProviderResult<()>;
    /// Applies what the live process can take and reports what a restart would still cost.
    ///
    /// Never restarts: the manager owns that decision, because it is the only thing that knows
    /// whether a turn is running.
    async fn apply_runtime(&mut self, change: RuntimeChange) -> ProviderResult<RuntimeApplied>;
    /// Replaces the harness process so the fields [`RestartPlan`] named become truthful.
    ///
    /// `change` is re-applied to the launch request, so the replacement opens with the values
    /// the live process could not take.
    async fn restart(&mut self, plan: &RestartPlan, change: &RuntimeChange) -> ProviderResult<()>;
    /// Stops the provider process or server.
    async fn stop(&mut self) -> ProviderResult<()>;
    /// Takes the next normalized event receiver.
    fn events(&mut self) -> ProviderEvents;
}

/// The process-probe cache, shared by every thread in this daemon.
///
/// A probe runs a binary, so it is cached per `(binary, home)` with a five-minute TTL: an agent
/// tab that re-probed per keystroke would be a fork bomb with a spinner.
fn probes() -> &'static ProbeCache {
    static PROBES: std::sync::OnceLock<ProbeCache> = std::sync::OnceLock::new();
    PROBES.get_or_init(ProbeCache::new)
}

/// The `AgentProvider` the manager drives, implemented over a `Harness`.
pub struct HarnessProvider {
    kind: AgentKind,
    config: HarnessConfig,
    harness: Option<Box<dyn Harness>>,
    /// The request the session was opened with, so a restart can reopen the same session.
    opened_with: Option<StartRequest>,
    /// The resume cursor the harness published, so a restart continues rather than forks.
    cursor: Option<String>,
    capabilities: HarnessCapabilities,
    sender: ProviderSink,
    receiver: Option<ProviderEvents>,
    /// The task that re-frames harness events for the manager. Held, never detached.
    forwarder: Option<JoinHandle<()>>,
}

impl HarnessProvider {
    /// Builds a provider for one harness kind. Nothing is spawned yet.
    #[must_use]
    pub fn new(kind: AgentKind, config: HarnessConfig) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            kind,
            config,
            harness: None,
            opened_with: None,
            cursor: None,
            capabilities: HarnessCapabilities::default(),
            sender,
            receiver: Some(receiver),
            forwarder: None,
        }
    }

    fn harness(&mut self) -> ProviderResult<&mut Box<dyn Harness>> {
        self.harness
            .as_mut()
            .ok_or_else(|| ProviderError::Unavailable {
                reason: "the agent session is not running".to_owned(),
            })
    }

    /// Forwards the harness's events onto the manager's channel.
    fn forward(&mut self, mut events: HarnessEvents) {
        let sink = self.sender.clone();
        if let Some(previous) = self.forwarder.take() {
            previous.abort();
        }
        self.forwarder = Some(tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let raw = event.raw.as_ref().map(|raw| raw.method.clone());
                let emission_skew = emission_skew(&event);
                if sink
                    .send(ProviderEvent {
                        event: event.event,
                        raw,
                        emission_skew,
                    })
                    .is_err()
                {
                    // The thread runtime is gone, which happens only during shutdown.
                    return;
                }
            }
        }));
    }

    /// Opens the harness for `request`, taking its event stream.
    async fn open(&mut self, request: StartRequest) -> ProviderResult<()> {
        let mut harness = harness::spawn(self.kind, &self.config, probes()).await?;
        let events = harness.events();
        let opened = harness
            .open(OpenSession {
                start: request.clone(),
            })
            .await?;
        self.capabilities = harness.capabilities().clone();
        self.cursor = opened.resume_cursor;
        self.opened_with = Some(request);
        self.harness = Some(harness);
        self.forward(events);
        Ok(())
    }

    /// Replaces the process so a reported restart plan becomes truthful.
    async fn restart_with(
        &mut self,
        plan: &RestartPlan,
        change: &RuntimeChange,
    ) -> ProviderResult<()> {
        let resume = plan.resume;
        let Some(mut request) = self.opened_with.clone() else {
            return Err(ProviderError::Unavailable {
                reason: "the agent session cannot be restarted before it has started".to_owned(),
            });
        };
        // The restart is invisible in the transcript because Fleet's store is the system of
        // record and the resume cursor is durable.
        if let Some(model) = change.model.clone() {
            request.model = Some(model);
        }
        if let Some(mode) = change.mode {
            request.mode = mode;
        }
        if let Some(sandbox) = change.sandbox {
            request.sandbox = sandbox;
        }
        if let Some(policy) = change.approval_policy.clone() {
            request.approval_policy = policy;
        }
        if let Some(profile) = change.permission_profile.clone() {
            request.permission_profile = profile;
        }
        request.resume_cursor = if resume { self.cursor.clone() } else { None };
        request.fork = false;
        if let Some(harness) = self.harness.as_mut() {
            harness.shutdown(ShutdownReason::Restart { resume }).await?;
        }
        self.harness = None;
        self.open(request).await
    }
}

#[async_trait]
impl AgentProvider for HarnessProvider {
    fn kind(&self) -> AgentKind {
        self.kind
    }

    fn capabilities(&self) -> HarnessCapabilities {
        self.capabilities.clone()
    }

    async fn start(&mut self, req: StartRequest) -> ProviderResult<()> {
        if self.harness.is_some() {
            return Err(ProviderError::Protocol {
                message: "the agent session was started more than once".to_owned(),
            });
        }
        if req.provider != self.kind {
            return Err(ProviderError::Protocol {
                message: format!(
                    "a {} thread cannot be started on the {} harness",
                    req.provider.display_name(),
                    self.kind.display_name()
                ),
            });
        }
        self.open(req).await
    }

    async fn send(&mut self, turn: TurnId, input: UserInput) -> ProviderResult<Submitted> {
        // The **adapter** decides whether this is a fresh turn or a steer: the caller must not
        // guess it from a stale mirror of the active turn, and its answer comes back so the
        // manager records the turn the message actually landed in.
        self.harness()?
            .submit(Submit {
                turn,
                input,
                intent: SubmitIntent::Fresh,
            })
            .await
            .map_err(ProviderError::from)
    }

    async fn interrupt(&mut self, turn: TurnId) -> ProviderResult<()> {
        self.harness()?
            .interrupt(turn, InterruptReason::User)
            .await
            .map_err(ProviderError::from)
    }

    async fn respond(&mut self, gate: GateId, answer: GateAnswer) -> ProviderResult<()> {
        self.harness()?
            .answer_gate(gate, answer)
            .await
            .map_err(ProviderError::from)
    }

    async fn apply_runtime(&mut self, change: RuntimeChange) -> ProviderResult<RuntimeApplied> {
        self.harness()?
            .apply_runtime(change)
            .await
            .map_err(ProviderError::from)
    }

    async fn restart(&mut self, plan: &RestartPlan, change: &RuntimeChange) -> ProviderResult<()> {
        self.restart_with(plan, change).await
    }

    async fn stop(&mut self) -> ProviderResult<()> {
        let Some(mut harness) = self.harness.take() else {
            return Ok(());
        };
        let result = harness.shutdown(ShutdownReason::User).await;
        // The forwarder is kept alive until the harness has emitted `SessionExited`, which
        // `shutdown` does before returning.
        if let Some(forwarder) = self.forwarder.take() {
            forwarder.abort();
        }
        result.map_err(ProviderError::from)
    }

    fn events(&mut self) -> ProviderEvents {
        self.receiver.take().unwrap_or_else(empty_events)
    }
}

/// Constructs the adapter for a provider kind.
///
/// `commands` are the configured shell command lines (`config.agentCommands`); each adapter
/// tokenizes its own with shell words, so `claude --model opus` keeps working. Nothing is spawned
/// here — [`AgentProvider::start`] owns the process, so a launch failure arrives as a typed
/// [`ProviderError`] the manager can turn into the terminal fallback.
pub fn spawn_provider(
    kind: AgentKind,
    req: &StartRequest,
    commands: &AgentCommands,
) -> anyhow::Result<Box<dyn AgentProvider>> {
    let command = match kind {
        AgentKind::Claude => commands.claude.clone(),
        AgentKind::Codex => commands.codex.clone(),
    };
    let config = HarnessConfig {
        command,
        home: None,
        env: req.env.clone(),
        // The attachments directory is granted with `--add-dir` and is owned by the thread's
        // store; until that lands, no directory is granted and a pasted image needs an approval.
        attachments_dir: None,
        client_version: env!("CARGO_PKG_VERSION").to_owned(),
        raw_log_dir: None,
    };
    Ok(Box::new(HarnessProvider::new(kind, config)))
}

/// How far behind the harness's own emission clock one frame was read.
///
/// Codex stamps `emittedAtMs` on every notification and it is the only server-side clock Fleet
/// gets. Clocks disagree, so a negative difference is not an error and not a measurement either:
/// it is dropped rather than reported as zero, which would make a skewed machine look punctual.
fn emission_skew(event: &HarnessEvent) -> Option<Duration> {
    event
        .observed_at
        .duration_since(event.emitted_at?)
        .ok()
        .filter(|skew| !skew.is_zero())
}

/// A typed provider transport, protocol, lifecycle, or deadline failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderError {
    /// The executable, server, or required provider feature is unavailable.
    #[error("provider unavailable: {reason}")]
    Unavailable {
        /// Human-readable availability failure.
        reason: String,
    },
    /// Provider wire data violated the adapter contract.
    #[error("provider protocol error: {message}")]
    Protocol {
        /// Human-readable protocol detail.
        message: String,
    },
    /// Provider process exited.
    #[error("provider exited with code {code:?}")]
    Exited {
        /// Process exit code when available.
        code: Option<i32>,
    },
    /// A bounded provider operation exceeded its deadline.
    #[error("provider timed out waiting for {what}")]
    Timeout {
        /// Operation or signal that timed out.
        what: String,
    },
}

impl From<HarnessError> for ProviderError {
    /// Re-frames a harness failure, keeping it structural.
    ///
    /// A `Protocol` failure carries only its fingerprint, so the message that reaches the manager
    /// is counts and field names — never a payload.
    fn from(error: HarnessError) -> Self {
        match error {
            HarnessError::Unavailable { reason } => Self::Unavailable { reason },
            HarnessError::Handshake { detail, .. } => Self::Unavailable { reason: detail },
            HarnessError::Protocol { .. } => Self::Protocol {
                message: error.to_string(),
            },
            HarnessError::Request { .. } | HarnessError::GateGone { .. } => Self::Protocol {
                message: error.to_string(),
            },
            HarnessError::Timeout { what, .. } => Self::Timeout {
                what: what.to_owned(),
            },
            HarnessError::Exited { code, .. } => Self::Exited { code },
        }
    }
}

pub(crate) fn empty_events() -> ProviderEvents {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    drop(sender);
    receiver
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use fleet_core::agents::{PermissionMode, ThreadId};

    use super::*;

    fn request(provider: AgentKind) -> StartRequest {
        StartRequest {
            thread: ThreadId::new(),
            worktree_path: PathBuf::from("/tmp/fleet-agent-factory"),
            provider,
            model: None,
            mode: PermissionMode::Ask,
            resume_cursor: None,
            fork: false,
            env: std::collections::BTreeMap::new(),
            sandbox: fleet_core::agents::SandboxPolicy::default(),
            approval_policy: fleet_core::agents::ApprovalPolicy::default(),
            permission_profile: None,
            title: None,
        }
    }

    fn commands() -> AgentCommands {
        AgentCommands {
            claude: "claude --model opus".to_owned(),
            codex: "codex".to_owned(),
            opencode: "opencode".to_owned(),
        }
    }

    /// Both harnesses are selectable, and nothing is spawned by the factory.
    #[test]
    fn both_harnesses_are_selectable_and_nothing_is_spawned() {
        for kind in [AgentKind::Claude, AgentKind::Codex] {
            let provider = spawn_provider(kind, &request(kind), &commands())
                .unwrap_or_else(|error| panic!("construct {kind:?}: {error}"));
            assert_eq!(provider.kind(), kind);
        }
    }

    /// A thread created for one harness is never started on the other: that would put one
    /// harness's transcript under the other's name.
    #[tokio::test]
    async fn a_thread_is_never_started_on_the_other_harness() {
        let mut provider =
            spawn_provider(AgentKind::Claude, &request(AgentKind::Claude), &commands())
                .unwrap_or_else(|error| panic!("{error}"));
        let error = provider
            .start(request(AgentKind::Codex))
            .await
            .err()
            .unwrap_or_else(|| panic!("a Codex thread must not start on Claude"));
        assert!(error.to_string().contains("cannot be started"), "{error}");
    }

    /// A missing binary is user-facing copy naming the harness, not a stack trace.
    #[tokio::test]
    async fn a_missing_binary_is_user_facing_copy() {
        let mut provider = spawn_provider(
            AgentKind::Codex,
            &request(AgentKind::Codex),
            &AgentCommands {
                codex: "fleet-no-such-agent-binary".to_owned(),
                ..commands()
            },
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let error = provider
            .start(request(AgentKind::Codex))
            .await
            .err()
            .unwrap_or_else(|| panic!("a missing binary cannot start"));
        assert!(
            error.to_string().contains("was not found on PATH")
                || error.to_string().contains("failed to run"),
            "{error}"
        );
    }

    /// Every verb refuses before `start`, rather than panicking on a missing process.
    #[tokio::test]
    async fn every_verb_refuses_before_the_session_is_open() {
        let mut provider =
            spawn_provider(AgentKind::Claude, &request(AgentKind::Claude), &commands())
                .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            provider
                .send(
                    TurnId::new(),
                    UserInput {
                        text: "hi".to_owned(),
                        attachments: Vec::new(),
                        item: None,
                    },
                )
                .await
                .is_err()
        );
        assert!(provider.interrupt(TurnId::new()).await.is_err());
        assert!(
            provider
                .respond(
                    GateId::new(),
                    GateAnswer::Question {
                        answers: Vec::new()
                    },
                )
                .await
                .is_err()
        );
        // Stopping a session that never started is a no-op, not an error: the manager stops
        // threads it is not sure about all the time.
        assert!(provider.stop().await.is_ok());
    }

    /// A harness protocol failure reaches the manager as counts and field names.
    #[test]
    fn a_protocol_failure_reaches_the_manager_without_a_payload() {
        let error = ProviderError::from(HarnessError::Protocol {
            op: crate::agents::harness::ProtocolOp::Decode,
            method: Some("turn/completed".to_owned()),
            fingerprint: crate::agents::harness::fingerprint::SchemaFingerprint::of_value(
                crate::agents::harness::fingerprint::IssueKind::MissingField,
                &serde_json::json!({"token": "sk-live-DEADBEEF-not-a-real-token"}),
            ),
        });
        let message = error.to_string();
        assert!(message.contains("turn/completed"), "{message}");
        assert!(message.contains("issues=1"), "{message}");
        assert!(!message.contains("sk-live"), "{message}");
    }
}
