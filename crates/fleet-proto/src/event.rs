//! Asynchronous daemon events broadcast to subscribed clients.

use fleet_core::{
    agents::{AgentThreadSummary, AttentionKind, Seq, SeqEvent, ThreadId},
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
    /// The live-stream budget for this connection overflowed for one thread.
    ///
    /// Everything after `from_seq` was dropped **for this connection only**; the client re-opens
    /// the thread with that cursor. Backpressure is never a stall, never an OOM, and never a
    /// silent drop — this event is what makes the third impossible. Emitted only to connections
    /// that advertised [`AGENT_RESYNC_CAPABILITY`](crate::AGENT_RESYNC_CAPABILITY), because an
    /// adjacently tagged variant an older peer cannot decode kills its whole frame.
    AgentResync {
        /// Affected thread.
        thread: ThreadId,
        /// Last sequence this connection is known to have received.
        from_seq: Seq,
    },
    /// Catch-up for one thread is complete; everything after this is live.
    ///
    /// Emitted only when the open asked for it and only to connections that advertised
    /// [`AGENT_SYNC_MARKER_CAPABILITY`](crate::AGENT_SYNC_MARKER_CAPABILITY). It is the *only*
    /// transition into live: a mirror never fabricates one.
    AgentSynchronized {
        /// Synchronized thread.
        thread: ThreadId,
    },
    /// The local daemon refilled or extended a mirrored thread's window from its owner.
    ///
    /// The client re-reads the window it has open. Emitted only to connections that advertised
    /// [`AGENT_WINDOW_CAPABILITY`](crate::AGENT_WINDOW_CAPABILITY).
    AgentWindow {
        /// Thread whose stored window changed.
        thread: ThreadId,
    },
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
        /// Authoritative hook-supplied reason this terminal needs the user.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attention: Option<AttentionKind>,
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
    /// An event family this build does not understand.
    ///
    /// `Event` is adjacently tagged, so without this arm one unknown `type` from a newer daemon
    /// is a decode error that drops the frame and logs a warning on every emission. With it,
    /// every future event family is survivable: the client ignores what it cannot read and keeps
    /// the ones it can. Emission is still gated on capabilities — this is the decode side of the
    /// same contract, and it is never *sent* deliberately.
    #[serde(other)]
    Unknown,
}

/// A frame this build could not decode strictly, and the reason it could not.
#[derive(Debug)]
pub struct UnknownEvent {
    /// The `type` tag exactly as it arrived, when the frame carried one.
    pub tag: Option<String>,
    /// Why the strict decoder refused the frame.
    pub error: serde_json::Error,
}

impl std::fmt::Display for UnknownEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.tag {
            Some(tag) => write!(formatter, "event `{tag}`: {}", self.error),
            None => write!(formatter, "untagged event: {}", self.error),
        }
    }
}

impl Event {
    /// Decodes one broadcast frame, reporting an undecodable family instead of failing the read.
    ///
    /// [`Event::Unknown`] and its `#[serde(other)]` cover only a *payload-free* unknown tag:
    /// `Event` is adjacently tagged, so a newer daemon's `{"type":"…","data":{…}}` still fails
    /// the strict decoder with `invalid type: map, expected unit variant`, and every event family
    /// worth adding carries a payload. This is the other half of that contract — the caller keeps
    /// its subscription and says what it skipped, rather than tearing down a connection over one
    /// frame it was never meant to understand.
    ///
    /// It costs nothing extra on the paths that use it: both the client and the remote link
    /// already decode every frame to a [`serde_json::Value`] before an `Event` is ever built.
    ///
    /// # Errors
    ///
    /// Returns the tag and the strict decoder's own error. A corrupt frame of a *known* family is
    /// indistinguishable from a new one here, which is why the error travels with the tag: the
    /// caller logs a known tag louder than an unknown one.
    pub fn from_wire(value: serde_json::Value) -> Result<Self, UnknownEvent> {
        let tag = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        serde_json::from_value(value).map_err(|error| UnknownEvent { tag, error })
    }
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
            attention: Some(AttentionKind::Finished),
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
    fn the_three_agent_stream_events_round_trip() {
        let thread = ThreadId::new();
        for event in [
            Event::AgentResync {
                thread,
                from_seq: Seq(41),
            },
            Event::AgentSynchronized { thread },
            Event::AgentWindow { thread },
        ] {
            assert_round_trip(event);
        }
    }

    #[test]
    fn an_event_family_this_build_never_heard_of_decodes_instead_of_killing_the_frame() {
        // A payload-free unknown tag is absorbed by `#[serde(other)]` itself.
        let bare: Event = serde_json::from_str(r#"{"type":"quantum_entangled"}"#)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(bare, Event::Unknown);
        assert_round_trip(Event::Unknown);

        // One that carries a payload does not, and this is the reason `from_wire` exists: the
        // adjacently tagged strict decoder refuses the content it has no variant to put it in.
        let payload = r#"{"type":"agent_checkpointed","data":{"thread":"x"}}"#;
        assert!(serde_json::from_str::<Event>(payload).is_err());
        let reported = Event::from_wire(
            serde_json::from_str(payload).unwrap_or_else(|error| panic!("{error}")),
        )
        .expect_err("an unknown family is reported, not decoded");
        assert_eq!(reported.tag.as_deref(), Some("agent_checkpointed"));
        assert!(reported.to_string().contains("agent_checkpointed"));

        // A known tag still decodes to its own arm through both doors, so nothing is shadowed.
        let known: Event = serde_json::from_str(r#"{"type":"daemon_shutting_down"}"#)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(known, Event::DaemonShuttingDown);
        assert_eq!(
            Event::from_wire(serde_json::json!({"type": "daemon_shutting_down"}))
                .unwrap_or_else(|error| panic!("{error}")),
            Event::DaemonShuttingDown
        );
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
