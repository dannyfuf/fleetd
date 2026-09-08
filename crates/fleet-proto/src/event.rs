//! Asynchronous daemon events broadcast to subscribed clients.

use fleet_core::{
    agents::{AgentThreadSummary, SeqEvent, ThreadId},
    ids::{BoardId, HostId, SessionId, TerminalId},
    sessions::{AgentActivity, Session},
    watches::{Watch, WatchChunk, WatchId},
};
use serde::{Deserialize, Serialize};

use crate::{
    job::JobRecord,
    snapshot::{LinkState, Snapshot},
    terminal::FrameUpdate,
};

/// Event families a client may subscribe to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Sequenced native-agent transcript and lifecycle events.
    Agent,
    /// Compact native-agent summary changes.
    AgentSummary,
    /// Board or card mutations.
    BoardChanged,
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
    /// A remote daemon link changed state.
    HostLinkChanged,
    /// An attached terminal should be reattached after recovery.
    TerminalReattach,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    /// One normalized, persisted native-agent event.
    Agent {
        /// Owning thread.
        thread: ThreadId,
        /// Sequenced event payload.
        event: SeqEvent,
    },
    /// A native-agent thread's compact state changed.
    AgentSummary(AgentThreadSummary),
    /// A board or its cards changed.
    BoardChanged {
        /// Changed board identifier.
        board_id: BoardId,
        /// Nature of the mutation.
        reason: BoardChangeReason,
    },
    /// A child watch was registered.
    WatchStarted(Watch),
    /// Retained output batched every 50 ms; sequence gaps require TailWatch.
    WatchOutput {
        /// Watch identifier.
        watch: WatchId,
        /// Immutable sequenced output chunks.
        chunks: Vec<WatchChunk>,
    },
    /// A child exited or its owning connection disappeared.
    WatchExited(Watch),
    /// A watch was removed; this never kills a process.
    WatchDismissed(WatchId),
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
    /// A configured machine's remote daemon link changed state.
    HostLinkChanged {
        /// Configured host.
        host: HostId,
        /// New link state.
        link: LinkState,
        /// Remote daemon version when known.
        version: Option<String>,
        /// Link failure detail when down.
        error: Option<String>,
    },
    /// A previously attached terminal should be attached again after recovery.
    TerminalReattach {
        /// Local terminal identifier.
        terminal: TerminalId,
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

/// Compatibility name for the asynchronous event payload enum.
pub type EventBody = Event;
/// Why a board changed; clients reload its authoritative view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoardChangeReason {
    /// Board creation.
    Created,
    /// Board configuration changed.
    Updated,
    /// Board deletion.
    Deleted,
    /// Card content or membership changed.
    CardChanged,
    /// Synchronization completed.
    Synced,
    /// Synchronization failed.
    SyncFailed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_round_trip;

    #[test]
    fn events_kinds_and_toast_levels_round_trip() {
        assert_round_trip(Event::TerminalExited {
            terminal: TerminalId(3),
            code: Some(0),
        });
        assert_round_trip(Event::AgentActivityChanged {
            session: SessionId::try_from("acme/api").unwrap_or_else(|error| panic!("{error}")),
            terminal_id: TerminalId(3),
            agent: Some("claude".to_owned()),
            activity: AgentActivity::Idle,
            changed_at: "2026-09-05T12:00:00Z".to_owned(),
        });
        assert_round_trip(EventKind::Toast);
        assert_round_trip(ToastLevel::Warning);
        let projection = fleet_core::agents::ThreadProjection::new(
            ThreadId::new(),
            fleet_core::ids::WorktreeId::try_from("acme/api#native-agents")
                .unwrap_or_else(|error| panic!("{error}")),
            fleet_core::agents::AgentKind::Claude,
        );
        assert_round_trip(Event::AgentSummary(projection.summary(Default::default())));
        assert_round_trip(Event::Agent {
            thread: projection.thread,
            event: SeqEvent {
                seq: fleet_core::agents::Seq(9),
                at: "2026-09-07T12:00:00Z"
                    .parse()
                    .unwrap_or_else(|error| panic!("{error}")),
                raw: Some("system.init".to_owned()),
                event: fleet_core::agents::AgentEvent::Notice("provider ready".to_owned()),
            },
        });
        assert_round_trip(EventKind::Agent);
        assert_round_trip(EventKind::AgentSummary);
        assert_round_trip(Event::HostLinkChanged {
            host: HostId::try_from("dev-box").expect("host"),
            link: LinkState::Ready,
            version: Some("fleetd 0.1.0".to_owned()),
            error: None,
        });
        assert_round_trip(Event::TerminalReattach {
            terminal: TerminalId(9),
        });
    }

    #[test]
    fn board_change_preserves_event_tag_convention() {
        let event = Event::BoardChanged {
            board_id: "work".parse().unwrap_or_else(|error| panic!("{error}")),
            reason: BoardChangeReason::CardChanged,
        };
        assert_round_trip(event.clone());
        let json = serde_json::to_value(&event).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            json,
            serde_json::json!({"type":"board_changed", "data":{"board_id":"work", "reason":"card_changed"}})
        );
        assert_eq!(
            serde_json::to_value(EventKind::BoardChanged).unwrap_or_else(|error| panic!("{error}")),
            "board_changed"
        );
    }
}
