//! Orthogonal session, turn, attention, mode, and harness-capability state.

use std::{cmp::Ordering, collections::BTreeSet, fmt};

use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Serialize};

use super::{TurnId, TurnOutcome};

/// A supported native-agent harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    /// Anthropic Claude Code.
    Claude,
    /// OpenAI Codex app-server.
    Codex,
}

impl AgentKind {
    /// Permission modes this harness accepts, in native-picker order.
    #[must_use]
    pub const fn supported_modes(self) -> &'static [PermissionMode] {
        match self {
            Self::Claude => &[
                PermissionMode::Ask,
                PermissionMode::AcceptEdits,
                PermissionMode::Plan,
                PermissionMode::Auto,
                PermissionMode::DontAsk,
                PermissionMode::FullAccess,
            ],
            Self::Codex => &[
                PermissionMode::Ask,
                PermissionMode::AcceptEdits,
                PermissionMode::Plan,
                PermissionMode::FullAccess,
            ],
        }
    }

    /// The default executable name for this harness.
    #[must_use]
    pub const fn executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The harness name used in native UI copy.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }
}

/// Why a ready harness is temporarily unable to make progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum WaitingReason {
    /// A provider usage window rejected further work.
    UsageLimit {
        /// Provider-native window name.
        window: String,
        /// Time at which the provider says the window resets.
        resets_at: DateTime<Utc>,
    },
}

/// Whole-harness-process lifecycle, independent of turn and gate state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum SessionState {
    /// A harness process is starting or resuming.
    Starting,
    /// The harness is ready for input.
    #[default]
    Ready,
    /// The harness reports active work.
    Running,
    /// Progress is parked on a provider-controlled wait.
    Waiting(WaitingReason),
    /// The harness was stopped intentionally.
    Stopped,
    /// The harness session failed.
    Error,
}

/// Why a thread stopped, kept on the record so delivery can tell a user's Stop from a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopCause {
    /// The user intentionally stopped the thread.
    User,
    /// The provider process exited unexpectedly.
    ProviderExit,
}

/// Lifecycle of the current or most recently settled turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum TurnState {
    /// No turn has been submitted.
    #[default]
    None,
    /// The harness accepted and is executing this turn.
    Running(TurnId),
    /// The harness authoritatively settled this turn.
    Settled(TurnId, TurnOutcome),
}

/// Why a thread currently needs user attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// An open tool permission.
    Permission,
    /// One or more blocking harness questions.
    Question,
    /// A settled plan awaiting a decision.
    Plan,
    /// A newly completed turn.
    Finished,
}

/// Derived user-attention state, ordered by the native-agents attention table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Attention {
    /// A decision or finished turn requires the user.
    NeedsYou(AttentionKind),
    /// Provider, turn, or background work remains active.
    Working,
    /// The provider is parked on a usage window.
    Waiting,
    /// The turn or session failed.
    Failed,
    /// Non-terminal output arrived since the client last viewed the thread.
    Unread,
    /// No attention signal is active.
    Idle,
}

impl Attention {
    /// Returns the table priority; larger ranks take precedence.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::NeedsYou(AttentionKind::Permission) => 8,
            Self::NeedsYou(AttentionKind::Question) => 7,
            Self::NeedsYou(AttentionKind::Plan) => 6,
            Self::Working => 5,
            Self::Waiting => 4,
            Self::Failed => 3,
            Self::NeedsYou(AttentionKind::Finished) => 2,
            Self::Unread => 1,
            Self::Idle => 0,
        }
    }
}

impl Ord for Attention {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl PartialOrd for Attention {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Permission policy selected for harness tool calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Ask before protected operations.
    #[default]
    Ask,
    /// Apply file edits without asking, while retaining other gates.
    AcceptEdits,
    /// Produce and approve a plan before execution.
    Plan,
    /// Let Claude approve actions it classifies as safe.
    Auto,
    /// Deny tools that are not already allowed instead of asking.
    DontAsk,
    /// Auto-allow supported harness operations.
    FullAccess,
}

impl PermissionMode {
    /// The word the CLI's `--mode` takes and refusals print, e.g. `full-access`.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::AcceptEdits => "accept-edits",
            Self::Plan => "plan",
            Self::Auto => "auto",
            Self::DontAsk => "dont-ask",
            Self::FullAccess => "full-access",
        }
    }
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.word())
    }
}

/// Provider-neutral model and reasoning-effort selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    /// Harness-native model identifier, or **the empty string for "the configured default".**
    ///
    /// The empty string is the sentinel a caller uses to select a reasoning effort without
    /// naming a model — `fleet subagent run --effort high` with no `--model`. The field stays a
    /// required `String` rather than becoming an `Option` because it is on the wire with
    /// byte-exact goldens, and an absent selection already means something else: `None` for the
    /// whole `ModelSelection` is "no opinion at all", while `Some` with an empty `model` is "this
    /// effort, whatever model the harness would have used".
    ///
    /// `AgentSessionManager::create_with` resolves it against the per-harness configured default
    /// and leaves it empty only when there is no configured model either. Every adapter must
    /// therefore treat an empty `model` as "say nothing about the model" and still honour
    /// `effort` (`docs/NATIVE-AGENTS.md` §15).
    pub model: String,
    /// Optional harness-native reasoning effort or variant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Optional model-provider identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// One harness-declared reasoning-effort option for a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffortDescriptor {
    /// Harness-native effort identifier sent back unchanged.
    pub id: String,
    /// Harness-authored explanation shown beside the option.
    pub description: String,
}

/// One model and the reasoning-effort vocabulary the harness declares for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDescriptor {
    /// Harness-native model identifier sent back unchanged.
    pub id: String,
    /// Harness-authored model name for presentation.
    pub display_name: String,
    /// Legal reasoning efforts for this model, in harness order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<ReasoningEffortDescriptor>,
    /// Harness-declared default effort, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

/// Cost of changing one runtime control.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlCost {
    /// The live harness can accept the change for a subsequent turn.
    #[default]
    InPlace,
    /// The harness must restart and resume before the change is truthful.
    RestartWithResume,
    /// The harness cannot express this control.
    NotSupported,
}

/// Harness support for reopening an existing conversation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ResumeSupport {
    /// This harness cannot resume a prior conversation.
    #[default]
    None,
    /// The harness resumes from an opaque cursor and may also support forking.
    ByCursor { fork: bool },
}

/// Harness support for steering an active turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum SteerSupport {
    /// Steering is unavailable.
    #[default]
    None,
    /// A second message is implicitly folded into the current turn.
    Implicit,
    /// A dedicated steering operation is available.
    Explicit { compare_and_swap: bool },
}

/// Harness support for stopping an active turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum InterruptSupport {
    /// Interrupt closes the process because no structured operation exists.
    #[default]
    HardClose,
    /// Interrupt has a receipt and may cancel queued input.
    Receipted { cancel_queued: bool },
}

/// Number of independent sandbox/access axes exposed by a harness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxAxes {
    /// Claude's one permission-mode axis.
    #[default]
    One,
    /// Codex's approval, sandbox, and permission-profile axes.
    Three,
}

/// Number of independent reasoning streams exposed by a harness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningChannels {
    /// One reasoning stream, used by Claude thinking.
    #[default]
    One,
    /// Separate summary and raw streams, used by Codex.
    Two,
}

/// Negotiated capabilities for one harness process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HarnessCapabilities {
    /// Parsed harness version.
    pub version: Version,
    /// Resume and fork support.
    pub resume: ResumeSupport,
    /// In-flight steering support.
    pub steer: SteerSupport,
    /// Interrupt protocol support.
    pub interrupt: InterruptSupport,
    /// Whether the harness can read its own history back.
    pub history_readback: bool,
    /// Whether compaction is a native operation.
    pub native_compaction: bool,
    /// Whether the harness emits turn-level diffs.
    pub turn_diff: bool,
    /// Whether the harness emits attention flags.
    pub attention_flags: bool,
    /// Whether questions may outlive their turn.
    pub async_questions: bool,
    /// Whether questions may contain secret answers.
    pub secret_answers: bool,
    /// Harness sandbox/access shape.
    pub sandbox_axes: SandboxAxes,
    /// Harness reasoning-stream shape.
    pub reasoning_channels: ReasoningChannels,
    /// Whether context utilization is reported while a turn runs.
    pub live_context_meter: bool,
    /// Cost of switching models.
    pub model_switch: ControlCost,
    /// Cost of switching reasoning effort.
    pub effort_switch: ControlCost,
    /// Cost of switching interaction/access mode.
    pub mode_switch: ControlCost,
    /// Permission modes this harness supports, in picker order.
    pub modes: Vec<PermissionMode>,
    /// Raw capability strings published by the harness.
    pub declared: BTreeSet<String>,
}

impl Default for HarnessCapabilities {
    fn default() -> Self {
        Self {
            version: Version::new(0, 0, 0),
            resume: ResumeSupport::None,
            steer: SteerSupport::None,
            interrupt: InterruptSupport::HardClose,
            history_readback: false,
            native_compaction: false,
            turn_diff: false,
            attention_flags: false,
            async_questions: false,
            secret_answers: false,
            sandbox_axes: SandboxAxes::One,
            reasoning_channels: ReasoningChannels::One,
            live_context_meter: false,
            model_switch: ControlCost::NotSupported,
            effort_switch: ControlCost::NotSupported,
            mode_switch: ControlCost::NotSupported,
            modes: Vec::new(),
            declared: BTreeSet::new(),
        }
    }
}

/// Whether the harness holds a usable account, as the harness itself reports it.
///
/// `None` on a projection means **never reported** — Claude publishes no account signal at all —
/// and is not the same thing as [`AccountStatus::SignedOut`], which is the harness saying it
/// looked and found nothing. A surface that collapsed the two would draw `signed out` on every
/// Claude thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AccountStatus {
    /// The harness has no account and cannot run a turn until one is added.
    SignedOut,
    /// The harness is signed in.
    SignedIn(AccountInfo),
}

/// What is known about the signed-in account.
///
/// A struct rather than a bare [`AccountKind`] so a later addition — an organization name, a
/// credit balance — is an additive field rather than a re-shaped event (`rust-ipc-protocol`
/// Rule 7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    /// How the account authenticates.
    pub kind: AccountKind,
}

impl AccountInfo {
    /// The shortest honest label for this account, or `None` when it has no distinguishing name.
    ///
    /// The email when there is one, the plan when there is not, and nothing at all rather than a
    /// placeholder: `docs/UX-SPEC.md` §3.6.0 forbids inventing a metadata segment.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        match &self.kind {
            AccountKind::ChatGpt { email, plan } => email
                .as_deref()
                .or(plan.as_deref())
                .filter(|label| !label.is_empty()),
            AccountKind::ApiKey => Some("api key"),
            AccountKind::Other(name) => Some(name.as_str()).filter(|name| !name.is_empty()),
        }
    }
}

/// How a signed-in harness account authenticates.
///
/// `Other` carries the harness's own word for a mode this build does not model, so a new Codex
/// auth mode is a label rather than a lost event (§4.5 rule 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AccountKind {
    /// A ChatGPT subscription, with whatever the harness published about it.
    #[serde(rename = "chatgpt")]
    ChatGpt {
        /// Account email, when the harness reports one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        /// Plan name, when the harness reports one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<String>,
    },
    /// A raw API key.
    ApiKey,
    /// An auth mode this build does not model, named by the harness.
    Other(String),
}
