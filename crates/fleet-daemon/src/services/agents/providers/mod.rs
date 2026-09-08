//! Native-agent provider adapter boundary and provider factory.

use async_trait::async_trait;
use fleet_core::{
    agents::{
        AgentEvent, AgentKind, Capabilities, GateAnswer, GateId, ModelSelection, PermissionMode,
        StartRequest, TurnId, UserInput,
    },
    config::AgentCommands,
};
use thiserror::Error;

pub mod claude;
pub mod opencode;

use claude::ClaudeProvider;
use opencode::OpenCodeProvider;

/// One normalized event and the provider wire type that produced it.
///
/// §11 names `SeqEvent.raw` — "the provider's own event type name for every stored event" — as
/// the mitigation for protocol drift, so the adapter has to carry that name across the channel;
/// the reducer is the only writer of the stored record and cannot invent it afterwards.
#[derive(Debug, Clone)]
pub struct ProviderEvent {
    /// The normalized event.
    pub event: AgentEvent,
    /// The provider's own name for the message this event was mapped from.
    pub raw: Option<String>,
}

impl ProviderEvent {
    /// One event mapped from a named provider message.
    #[must_use]
    pub fn new(event: AgentEvent, raw: Option<&str>) -> Self {
        Self {
            event,
            raw: raw.map(ToOwned::to_owned),
        }
    }
}

impl From<AgentEvent> for ProviderEvent {
    /// An event the adapter produced itself, with no provider message behind it.
    fn from(event: AgentEvent) -> Self {
        Self { event, raw: None }
    }
}

/// The exit code of a finished child, including one a signal killed.
///
/// `ExitStatus::code()` is `None` for a signalled process, which is exactly the crash §2 asks
/// the tab to identify (`exited 137` in red). `128 + signal` is the shell's own spelling of it.
/// Both adapters report `SessionExited` from the same reading, so a killed `opencode serve` and
/// a killed `claude` describe themselves the same way.
#[must_use]
pub fn exit_code(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt as _;

    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
}

/// Send side of one provider's normalized event stream.
///
/// The stream is unbounded on purpose. Every manager verb (`send`, `respond`, `stop`, …) holds
/// the thread's operation gate across the adapter call, and the only consumer of this channel
/// has to take that same gate to reduce an event: a bounded channel that filled up during a
/// streaming turn would park the adapter inside the verb that holds the gate and park the drain
/// on the gate the verb holds — a deadlock no timeout recovers from. §3 keeps the adapters
/// drainable instead, and the drain coalesces deltas, so the queue is transient by construction.
pub type ProviderSink = tokio::sync::mpsc::UnboundedSender<ProviderEvent>;

/// Receive side of one provider's normalized event stream.
pub type ProviderEvents = tokio::sync::mpsc::UnboundedReceiver<ProviderEvent>;

/// Result returned by provider control operations.
pub type ProviderResult<T> = Result<T, ProviderError>;

/// Provider lifecycle and command adapter defined by Native Agents §3.1.
#[async_trait]
pub trait AgentProvider: Send {
    /// Provider represented by this adapter.
    fn kind(&self) -> AgentKind;
    /// Operations implemented by this adapter/version pair.
    fn capabilities(&self) -> Capabilities;
    /// Starts a fresh or resumed provider session.
    async fn start(&mut self, req: StartRequest) -> ProviderResult<()>;
    /// Sends or steers user input for a turn.
    async fn send(&mut self, turn: TurnId, input: UserInput) -> ProviderResult<()>;
    /// Requests interruption of a turn; the later terminal provider event remains authoritative.
    async fn interrupt(&mut self, turn: TurnId) -> ProviderResult<()>;
    /// Maps a normalized gate answer back to the provider protocol.
    async fn respond(&mut self, gate: GateId, answer: GateAnswer) -> ProviderResult<()>;
    /// Changes permission or plan mode.
    async fn set_mode(&mut self, mode: PermissionMode) -> ProviderResult<()>;
    /// Changes the active model and optional effort/provider.
    async fn set_model(&mut self, model: ModelSelection) -> ProviderResult<()>;
    /// Stops the provider process or server.
    async fn stop(&mut self) -> ProviderResult<()>;
    /// Takes the next normalized event receiver.
    fn events(&mut self) -> ProviderEvents;
}

/// Constructs the adapter for a provider kind.
///
/// `commands` are the configured shell command lines (`config.agentCommands`); each adapter
/// splits its own with shell words so `claude --model opus` keeps working. Nothing is spawned
/// here — [`AgentProvider::start`] owns the transport, so a failure to launch arrives as a
/// typed [`ProviderError`] the manager can turn into the terminal fallback.
///
/// A resumed thread resumes in place rather than forking: `StartRequest` carries no fork flag,
/// and silently branching a session would strand the transcript the user is looking at.
pub fn spawn_provider(
    kind: AgentKind,
    _req: &StartRequest,
    commands: &AgentCommands,
) -> anyhow::Result<Box<dyn AgentProvider>> {
    Ok(match kind {
        AgentKind::Claude => Box::new(ClaudeProvider::new(commands.claude.clone())),
        AgentKind::OpenCode => Box::new(OpenCodeProvider::new(commands.opencode.clone())),
    })
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

pub(crate) fn empty_events() -> ProviderEvents {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    drop(sender);
    receiver
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use fleet_core::agents::ThreadId;

    use super::*;

    fn request(provider: AgentKind) -> StartRequest {
        StartRequest {
            thread: ThreadId::new(),
            worktree_path: PathBuf::from("/tmp/fleet-agent-factory"),
            provider,
            model: None,
            mode: PermissionMode::Ask,
            resume_cursor: None,
            title: None,
        }
    }

    fn commands() -> AgentCommands {
        AgentCommands {
            claude: "claude --model opus".to_owned(),
            opencode: "opencode".to_owned(),
        }
    }

    /// R5: both adapters report a signalled child the same way, so §2's `exited 137` is red in
    /// the tab whichever provider was killed.
    #[test]
    fn a_signalled_child_reports_the_shells_own_spelling_of_its_death() {
        use std::os::unix::process::ExitStatusExt as _;

        // A raw wait status of 9 is "terminated by SIGKILL"; `ExitStatus::code()` answers `None`.
        let killed = std::process::ExitStatus::from_raw(9);
        assert_eq!(killed.code(), None);
        assert_eq!(exit_code(&killed), Some(137));
        // A normal exit is untouched.
        assert_eq!(
            exit_code(&std::process::ExitStatus::from_raw(0x0100)),
            Some(1)
        );
        assert_eq!(exit_code(&std::process::ExitStatus::from_raw(0)), Some(0));
    }

    #[test]
    fn both_providers_are_selectable_and_nothing_is_spawned() {
        for kind in [AgentKind::Claude, AgentKind::OpenCode] {
            let provider = spawn_provider(kind, &request(kind), &commands())
                .unwrap_or_else(|error| panic!("construct {kind:?} provider: {error}"));
            assert_eq!(provider.kind(), kind);
            assert!(provider.capabilities().interrupt);
        }
    }
}
