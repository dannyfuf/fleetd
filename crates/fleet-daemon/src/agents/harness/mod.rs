//! The adapter seam: one live harness process bound to one thread.
//!
//! `NATIVE-AGENTS.md` §3.1 and spec A.1 own this file. The trait is deliberately narrow and
//! forbids four things:
//!
//! - **no method returns transcript state** — that would give the daemon two orderings of one
//!   truth. Everything observable arrives on [`Harness::events`];
//! - **no method blocks on a human** — [`Harness::answer_gate`] writes the answer and returns;
//! - **no adapter holds a GPUI handle or reads configuration at call time** — configuration is
//!   captured in [`HarnessConfig`] at [`spawn`];
//! - **no adapter retries a turn** — both harnesses retry internally and say so, and a
//!   Fleet-level retry would double-charge the user and duplicate side effects.
//!
//! Construction is the free function [`spawn`] rather than a trait method, because the failure
//! modes before a process exists (binary missing, too old, signed out) are a different taxonomy
//! from "the turn failed".

#[cfg(test)]
pub(crate) mod capture;
pub mod fingerprint;
#[cfg(test)]
pub(crate) mod mockpeer;
pub mod ndjson;
pub mod probe;
pub mod process;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::SystemTime,
};

use async_trait::async_trait;
use fleet_core::agents::{
    AgentEvent, AgentKind, ApprovalPolicy, GateAnswer, GateId, HarnessCapabilities, ModelSelection,
    PermissionMode, SandboxPolicy, StartRequest, TurnId, UserInput,
};
use thiserror::Error;
use tokio::sync::mpsc;

use fingerprint::SchemaFingerprint;
use probe::ProbeCache;

/// Which harness an adapter drives.
///
/// The daemon has no second vocabulary for this: the harness kind *is* the provider kind the
/// thread record, the protocol and the UI already carry.
pub type HarnessKind = AgentKind;

/// A pointer into the per-thread raw NDJSON log, never an inline payload.
///
/// Inlining raw frames into events is what makes a snapshot undecodable at 16 MiB (§3.1), so the
/// event carries the harness's own name for the frame plus where to find it, and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRef {
    /// The harness's own name for the frame (`system/init`, `turn/completed`).
    pub method: String,
    /// Byte offset in the thread's raw log, when raw logging is enabled.
    pub offset: Option<u64>,
}

impl RawRef {
    /// A reference that names the frame kind but no location.
    #[must_use]
    pub fn method(method: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            offset: None,
        }
    }
}

/// One normalized event as the adapter observed it.
#[derive(Debug, Clone, PartialEq)]
pub struct HarnessEvent {
    /// When the daemon read the frame.
    pub observed_at: SystemTime,
    /// When the harness says it emitted the frame.
    ///
    /// Codex stamps every notification with a top-level `emittedAtMs` and it is the only
    /// server-side clock Fleet gets — what makes ordering measurable across a remote link.
    /// Claude publishes none, so this is `None` there.
    pub emitted_at: Option<SystemTime>,
    /// The harness-neutral event.
    pub event: AgentEvent,
    /// Where the frame behind it can be read, for debugging.
    pub raw: Option<RawRef>,
}

impl HarnessEvent {
    /// An event observed now, named by the frame it was mapped from.
    #[must_use]
    pub fn now(event: AgentEvent, raw: Option<RawRef>) -> Self {
        Self {
            observed_at: SystemTime::now(),
            emitted_at: None,
            event,
            raw,
        }
    }

    /// Records the harness's own emission time.
    #[must_use]
    pub fn emitted_at(mut self, at: Option<SystemTime>) -> Self {
        self.emitted_at = at;
        self
    }
}

/// Send side of one harness's event stream.
///
/// Unbounded on purpose. Every manager verb holds the thread's operation gate across the adapter
/// call and the only consumer takes that same gate to reduce an event, so a bounded channel that
/// filled during a streaming turn would park the adapter inside the verb that holds the gate and
/// park the drain on the gate that verb holds — a deadlock no timeout recovers from.
pub type HarnessSink = mpsc::UnboundedSender<HarnessEvent>;

/// Receive side of one harness's event stream.
pub type HarnessEvents = mpsc::UnboundedReceiver<HarnessEvent>;

/// Everything an adapter needs before a process exists.
#[derive(Debug, Clone, Default)]
pub struct HarnessConfig {
    /// The configured shell command line, quote-tokenized by the adapter (`claude --model opus`).
    pub command: String,
    /// Per-instance harness home (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`), tilde-expanded already.
    ///
    /// Never `HOME`: overriding that relocates the macOS login keychain and the CLI reports
    /// "Not logged in" (spec A.2.2).
    pub home: Option<PathBuf>,
    /// Explicit child-environment additions, applied after the inherited environment is filtered.
    pub env: BTreeMap<String, String>,
    /// The leaf directory holding this thread's attachments, granted with `--add-dir`.
    pub attachments_dir: Option<PathBuf>,
    /// Fleet's own version, reported to the harness as client info.
    pub client_version: String,
    /// Directory for the per-thread raw NDJSON debug log, when raw logging is enabled.
    pub raw_log_dir: Option<PathBuf>,
}

/// Open (or resume) one harness session.
#[derive(Debug, Clone)]
pub struct OpenSession {
    /// The thread, worktree, model, mode and resume cursor to open with.
    pub start: StartRequest,
}

/// What the harness published about itself once the handshake completed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionOpened {
    /// The harness-native resume cursor, persisted before any turn runs.
    pub resume_cursor: Option<String>,
    /// The model the harness says is in force, which may not be the one requested.
    pub model: Option<ModelSelection>,
    /// The permission policy the harness reported. Advisory on Claude (§4.1).
    pub mode: Option<PermissionMode>,
}

/// Why a message is being submitted, which decides its wire form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitIntent {
    /// Start a new turn.
    Fresh,
    /// Steer the turn already running.
    Steer,
    /// Answer a non-blocking question by sending it as a message (Codex async questions).
    AnswerAsMessage,
}

/// One user submission.
#[derive(Debug, Clone)]
pub struct Submit {
    /// The turn Fleet minted for a fresh turn, or the running turn for a steer.
    pub turn: TurnId,
    /// The user's message.
    pub input: UserInput,
    /// Whether this is a fresh turn, a steer, or an answer routed as a message.
    pub intent: SubmitIntent,
}

/// What the harness did with a submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Submitted {
    /// The turn the message landed in.
    pub turn: TurnId,
    /// Whether the harness folded it into a running turn (or queued it behind one).
    pub queued: bool,
}

/// Why a turn is being stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptReason {
    /// A person pressed Stop.
    User,
    /// The thread is being closed.
    ThreadClosing,
    /// The daemon is shutting down.
    DaemonShutdown,
    /// A gate answer asked for the turn to stop as well.
    GateCancelled,
}

/// Why a harness process is being torn down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownReason {
    /// A person stopped the thread.
    User,
    /// A newer process replaced this one.
    Replaced,
    /// A control change needs a restart; `resume` says whether the cursor is reused.
    Restart {
        /// Whether the replacement reopens the same harness session.
        resume: bool,
    },
    /// The session is being torn down because of a failure.
    Fatal(String),
}

/// One runtime control Fleet can ask a harness to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimeField {
    /// The model id, including Claude's `[1m]` context-window suffix.
    Model,
    /// Reasoning effort.
    Effort,
    /// The access/permission mode.
    Mode,
    /// The filesystem sandbox (Codex).
    Sandbox,
    /// The approval policy (Codex).
    ApprovalPolicy,
    /// The permission profile (Codex).
    PermissionProfile,
}

/// A requested runtime change. Absent fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeChange {
    /// New model and effort.
    pub model: Option<ModelSelection>,
    /// New access mode.
    pub mode: Option<PermissionMode>,
    /// New sandbox policy.
    pub sandbox: Option<SandboxPolicy>,
    /// New approval policy.
    pub approval_policy: Option<ApprovalPolicy>,
    /// New permission profile, where `Some(None)` clears it.
    pub permission_profile: Option<Option<String>>,
}

/// What a restart would take, handed back to the manager rather than performed by the adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartPlan {
    /// Whether the replacement process resumes the same harness session.
    pub resume: bool,
    /// The fields that only a restart can make truthful.
    pub fields: BTreeSet<RuntimeField>,
}

/// The outcome of [`Harness::apply_runtime`].
///
/// Returning this instead of `()` moves the restart decision to the manager, the only thing that
/// knows whether a turn is running (§3.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeApplied {
    /// Fields the live process accepted immediately.
    pub applied_now: BTreeSet<RuntimeField>,
    /// Fields that take effect at the next turn.
    pub applies_next_turn: BTreeSet<RuntimeField>,
    /// Set when the manager must restart the process at a turn boundary.
    pub restart: Option<RestartPlan>,
}

/// Which side of the wire a protocol failure happened on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolOp {
    /// A frame could not be classified as a request, response or notification.
    Classify,
    /// A frame's params or result could not be decoded.
    Decode,
    /// A frame Fleet was writing could not be encoded.
    Encode,
    /// A gate answer could not be mapped back onto the harness protocol.
    Respond,
}

impl std::fmt::Display for ProtocolOp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Classify => "classify",
            Self::Decode => "decode",
            Self::Encode => "encode",
            Self::Respond => "respond",
        };
        formatter.write_str(name)
    }
}

/// Every way an adapter can fail.
///
/// `Protocol` carries a [`SchemaFingerprint`] and **never** a payload, a field value, or a string
/// derived from one — see the module docs of [`fingerprint`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HarnessError {
    /// The binary is missing, unrunnable, signed out, or too old. The reason is user-facing copy.
    #[error("{reason}")]
    Unavailable {
        /// User-facing sentence naming the `^s F` terminal fallback's cause.
        reason: String,
    },
    /// The handshake did not complete.
    #[error("the {} handshake did not complete: {detail}", harness.display_name())]
    Handshake {
        /// Which harness.
        harness: HarnessKind,
        /// Structural detail, never a payload.
        detail: String,
    },
    /// A frame could not be classified, decoded, encoded or answered.
    #[error("{op} failed for {}: {}", method.as_deref().unwrap_or("an unnamed frame"), fingerprint.summary())]
    Protocol {
        /// Which side of the wire.
        op: ProtocolOp,
        /// The harness's own method or frame name, which is not payload data.
        method: Option<String>,
        /// The structural description of the failure.
        fingerprint: SchemaFingerprint,
    },
    /// A harness request answered with an error.
    #[error("{method} failed: {detail}")]
    Request {
        /// The method Fleet called.
        method: String,
        /// The harness's own error code, when it published one.
        code: Option<i64>,
        /// The harness's own error message. Harness-authored copy, never a Fleet payload echo.
        detail: String,
    },
    /// The gate was already answered or withdrawn.
    #[error("gate {gate} is no longer open")]
    GateGone {
        /// The gate Fleet tried to answer.
        gate: GateId,
    },
    /// A bounded operation exceeded its Fleet-side deadline.
    #[error("timed out waiting for {what} after {}ms", after.as_millis())]
    Timeout {
        /// What Fleet was waiting for.
        what: &'static str,
        /// The deadline that expired.
        after: std::time::Duration,
    },
    /// The harness process is gone.
    #[error("the harness exited (code {code:?}, signal {signal:?})")]
    Exited {
        /// Exit code, when readable.
        code: Option<i32>,
        /// Terminating signal, when readable.
        signal: Option<i32>,
    },
}

impl HarnessError {
    /// A decode failure over a payload, described structurally.
    #[must_use]
    pub fn decode(
        method: Option<&str>,
        error: &serde_json::Error,
        payload: &serde_json::Value,
    ) -> Self {
        Self::Protocol {
            op: ProtocolOp::Decode,
            method: method.map(ToOwned::to_owned),
            fingerprint: SchemaFingerprint::of(error, payload),
        }
    }

    /// Whether this failure means the session can no longer make progress.
    #[must_use]
    pub const fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::Unavailable { .. } | Self::Handshake { .. } | Self::Exited { .. }
        )
    }
}

/// Result of a harness operation.
pub type HarnessResult<T> = Result<T, HarnessError>;

/// One live harness process bound to one thread.
///
/// Owned by the daemon's thread runtime; never shared, never `Clone`, never reachable from a
/// client.
#[async_trait]
pub trait Harness: Send + 'static {
    /// Which harness this adapter drives.
    fn kind(&self) -> HarnessKind;

    /// Capabilities negotiated at probe/handshake time and frozen for the life of the process.
    ///
    /// The UI reads this to decide what it may offer: a button that silently means something
    /// weaker than it says is worse than no button.
    fn capabilities(&self) -> &HarnessCapabilities;

    /// Spawns (or re-attaches) and completes the handshake.
    ///
    /// Returns only once the harness published its own identity — Claude's `system/init`,
    /// Codex's `thread/start`/`thread/resume` response. Emits nothing itself: every observable
    /// consequence arrives on [`Harness::events`]. Returning is the barrier a `submit` racing the
    /// handshake waits on.
    async fn open(&mut self, req: OpenSession) -> HarnessResult<SessionOpened>;

    /// Delivers user input, in whatever wire form the intent implies.
    ///
    /// The adapter knows the wire form; the caller must never guess it from a stale mirror of
    /// the active turn.
    async fn submit(&mut self, req: Submit) -> HarnessResult<Submitted>;

    /// Stops the named turn. Idempotent, and returns **before** the turn is settled: settlement
    /// arrives on [`Harness::events`], never as this call's return value.
    async fn interrupt(&mut self, turn: TurnId, reason: InterruptReason) -> HarnessResult<()>;

    /// Answers an open gate. Answering an unknown or resolved gate is
    /// [`HarnessError::GateGone`], never a panic.
    async fn answer_gate(&mut self, gate: GateId, answer: GateAnswer) -> HarnessResult<()>;

    /// Applies what it can and reports what a restart would take.
    async fn apply_runtime(&mut self, change: RuntimeChange) -> HarnessResult<RuntimeApplied>;

    /// Asks the harness to compact its own context now.
    async fn compact(&mut self) -> HarnessResult<()>;

    /// Ordered teardown: settle every gate, force-complete every item and the open turn, then
    /// kill the process, and emit `SessionExited` last. Idempotent.
    async fn shutdown(&mut self, reason: ShutdownReason) -> HarnessResult<()>;

    /// Takes the event stream, exactly once, so a second consumer cannot exist.
    fn events(&mut self) -> HarnessEvents;
}

/// Constructs an adapter for `kind`, probing the binary first.
///
/// Nothing is spawned for the session here: [`Harness::open`] owns the process, so a launch
/// failure arrives as a typed [`HarnessError`] the manager can turn into the terminal fallback.
/// The probe *may* run the binary (`--version`), which is why this is `async` and cached.
pub async fn spawn(
    kind: HarnessKind,
    cfg: &HarnessConfig,
    probe: &ProbeCache,
) -> HarnessResult<Box<dyn Harness>> {
    let probed = probe.probe(kind, cfg).await?;
    match kind {
        HarnessKind::Claude => Ok(Box::new(super::claude::ClaudeHarness::new(
            cfg.clone(),
            probed,
        ))),
        HarnessKind::Codex => Ok(Box::new(super::codex::CodexHarness::new(
            cfg.clone(),
            probed,
        ))),
    }
}

/// An event stream with no producer, for an adapter whose receiver was already taken.
#[must_use]
pub fn closed_events() -> HarnessEvents {
    let (sender, receiver) = mpsc::unbounded_channel();
    drop(sender);
    receiver
}
