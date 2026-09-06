//! Asynchronous daemon events broadcast to subscribed clients.

use fleet_core::{
    ids::{SessionId, TerminalId},
    sessions::{AgentActivity, Session},
};
use serde::{Deserialize, Serialize};

use crate::{job::JobRecord, snapshot::Snapshot, terminal::FrameUpdate};

/// Event families a client may subscribe to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Watch registration.
    WatchStarted,
    /// Coalesced watch output.
    WatchOutput,
    /// Watch completion.
    WatchExited,
    /// Watch removal, including terminal close and TTL.
    WatchDismissed,
    /// Complete domain snapshot changes.
    SnapshotChanged,
    /// Background job changes.
    JobUpdated,
    /// Session metadata changes.
    SessionChanged,
    /// Coding-agent activity transitions.
    AgentActivityChanged,
    /// Terminal frame updates.
    TerminalFrame,
    /// Terminal exits.
    TerminalExited,
    /// Terminal title changes.
    TerminalTitle,
    /// Transient daemon messages.
    Toast,
    /// Daemon shutdown notification.
    DaemonShuttingDown,
}

/// Severity of a transient client notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastLevel {
    /// Informational message.
    Info,
    /// Non-fatal warning.
    Warning,
    /// Operation failure.
    Error,
}

/// Asynchronous notification emitted by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    /// A child watch was registered.
    WatchStarted(fleet_core::watches::Watch),
    /// Retained output batched every 50 ms; sequence gaps require TailWatch.
    WatchOutput {
        /// Watch identifier.
        watch: fleet_core::watches::WatchId,
        /// Immutable sequenced output chunks.
        chunks: Vec<fleet_core::watches::WatchChunk>,
    },
    /// A child exited or its owning connection disappeared.
    WatchExited(fleet_core::watches::Watch),
    /// A watch was removed; this never kills a process.
    WatchDismissed(fleet_core::watches::WatchId),
    /// The authoritative domain snapshot changed.
    SnapshotChanged(Snapshot),
    /// A job was created or changed.
    JobUpdated(JobRecord),
    /// A session was created or changed.
    SessionChanged(Session),
    /// A recognized agent terminal changed between unknown, working, and idle.
    AgentActivityChanged {
        /// Owning session.
        session: SessionId,
        /// Changed terminal.
        terminal_id: TerminalId,
        /// Recognized executable, when currently present.
        agent: Option<String>,
        /// New activity state.
        activity: AgentActivity,
        /// ISO-8601 transition time.
        changed_at: String,
    },
    /// A terminal's rendered grid changed.
    TerminalFrame(FrameUpdate),
    /// A terminal PTY exited.
    TerminalExited {
        /// Exited terminal.
        terminal: TerminalId,
        /// Process exit code when available.
        code: Option<i32>,
    },
    /// A terminal title changed.
    TerminalTitle {
        /// Updated terminal.
        terminal: TerminalId,
        /// New title.
        title: String,
    },
    /// A transient client-facing message.
    Toast {
        /// Message severity.
        level: ToastLevel,
        /// Human-readable message.
        message: String,
    },
    /// The daemon is about to stop accepting work.
    DaemonShuttingDown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_and_kind_round_trip() {
        let event = Event::TerminalExited {
            terminal: TerminalId(3),
            code: Some(0),
        };
        let json = serde_json::to_string(&event).unwrap_or_else(|error| panic!("{error}"));
        let decoded: Event = serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, event);

        let json =
            serde_json::to_string(&EventKind::Toast).unwrap_or_else(|error| panic!("{error}"));
        let decoded: EventKind =
            serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, EventKind::Toast);

        let event = Event::AgentActivityChanged {
            session: SessionId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
            terminal_id: TerminalId(3),
            agent: Some("claude".to_owned()),
            activity: AgentActivity::Idle,
            changed_at: "2026-09-05T12:00:00Z".to_owned(),
        };
        let json = serde_json::to_string(&event).unwrap_or_else(|error| panic!("{error}"));
        let decoded: Event = serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, event);

        let json =
            serde_json::to_string(&ToastLevel::Warning).unwrap_or_else(|error| panic!("{error}"));
        let decoded: ToastLevel =
            serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, ToastLevel::Warning);
    }
}
