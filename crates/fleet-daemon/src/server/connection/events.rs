//! Which events one connection is allowed to see.
//!
//! Two filters, and the order matters. The subscription filter is the client's own request — it
//! asked for these families and no others. The **capability** filter is a decode-safety rule and
//! not a preference: [`Event`] is adjacently tagged, so a variant a peer has no arm for does not
//! cost it one event, it fails the decode of the whole frame. Every event family added after a
//! client shipped therefore reaches only a connection whose handshake named the capability that
//! defines it (`fleet_proto::AGENT_CAPABILITIES`), and a peer that named nothing keeps exactly
//! the behaviour it had.

use std::collections::HashSet;

use fleet_proto::{
    event::{Event, EventKind},
    request::HelloClient,
};

pub(super) fn event_visible(
    event: &Event,
    subscriptions: &HashSet<EventKind>,
    attached: &HashSet<fleet_core::ids::TerminalId>,
    client: &HelloClient,
) -> bool {
    if !subscriptions.contains(&event_kind(event)) {
        return false;
    }
    match event {
        Event::TerminalFrame(frame) => attached.contains(&frame.terminal),
        Event::TerminalExited { terminal, .. } | Event::TerminalTitle { terminal, .. } => {
            attached.contains(terminal)
        }
        // `Event` is adjacently tagged, so a variant this peer has no arm for does not degrade
        // to "one event lost" — it fails the decode of the whole frame. The three agent
        // stream-control events postdate the original set, so each is sent only to a connection
        // whose handshake named the capability that defines it. An older client keeps the
        // recovery it already had: a sequence gap in `Event::Agent`.
        Event::AgentResync { .. } => client.supports(fleet_proto::AGENT_RESYNC_CAPABILITY),
        Event::AgentSynchronized { .. } => {
            client.supports(fleet_proto::AGENT_SYNC_MARKER_CAPABILITY)
        }
        Event::AgentWindow { .. } => client.supports(fleet_proto::AGENT_WINDOW_CAPABILITY),
        // Never re-broadcast: it is a tag *this* build could not name, and forwarding it tells
        // no peer anything it can act on.
        Event::Unknown => false,
        _ => true,
    }
}

pub(super) fn event_kind(event: &Event) -> EventKind {
    match event {
        // The window/resync/synchronized trio are control events on one thread's agent stream,
        // so they ride the family a client already subscribes to for that stream.
        Event::Agent { .. }
        | Event::AgentResync { .. }
        | Event::AgentSynchronized { .. }
        | Event::AgentWindow { .. } => EventKind::Agent,
        Event::AgentSummary(_) => EventKind::AgentSummary,
        Event::WatchStarted(_) => EventKind::WatchStarted,
        Event::WatchOutput { .. } => EventKind::WatchOutput,
        Event::WatchExited(_) => EventKind::WatchExited,
        Event::WatchDismissed(_) => EventKind::WatchDismissed,
        Event::SnapshotChanged(_) => EventKind::SnapshotChanged,
        Event::BoardChanged { .. } => EventKind::BoardChanged,
        Event::JobUpdated(_) => EventKind::JobUpdated,
        Event::SessionChanged(_) => EventKind::SessionChanged,
        Event::AgentActivityChanged { .. } => EventKind::AgentActivityChanged,
        Event::TerminalFrame(_) => EventKind::TerminalFrame,
        Event::TerminalExited { .. } => EventKind::TerminalExited,
        Event::TerminalTitle { .. } => EventKind::TerminalTitle,
        Event::HostLinkChanged { .. } => EventKind::HostLinkChanged,
        Event::TerminalReattach { .. } => EventKind::TerminalReattach,
        Event::Toast { .. } => EventKind::Toast,
        Event::DaemonShuttingDown => EventKind::DaemonShuttingDown,
        // A payload-free unknown tag from a newer peer: subscribed clients see it as a plain
        // daemon notice rather than being dropped silently.
        Event::Unknown => EventKind::Toast,
    }
}

#[cfg(test)]
mod tests {
    use fleet_core::ids::TerminalId;

    use super::*;

    #[test]
    fn watch_events_are_global_and_require_their_subscription() {
        let event = Event::WatchDismissed(fleet_core::watches::WatchId(1));
        assert!(event_visible(
            &event,
            &HashSet::from([EventKind::WatchDismissed]),
            &HashSet::new(),
            &HelloClient::default(),
        ));
        assert!(!event_visible(
            &event,
            &HashSet::new(),
            &HashSet::new(),
            &HelloClient::default(),
        ));
    }

    /// An agent stream-control event is withheld from a peer that never named the capability:
    /// `Event` is adjacently tagged, so sending one to a client with no arm for it would fail
    /// the decode of that whole frame rather than lose one event.
    #[test]
    fn agent_stream_control_events_need_the_peer_to_have_named_them() {
        let subscriptions = HashSet::from([EventKind::Agent]);
        let thread = fleet_core::agents::ThreadId::new();
        let capable = HelloClient {
            capabilities: fleet_proto::AGENT_CAPABILITIES
                .iter()
                .map(|capability| (*capability).to_owned())
                .collect(),
            ..HelloClient::default()
        };
        for event in [
            Event::AgentResync {
                thread,
                from_seq: fleet_core::agents::Seq(7),
            },
            Event::AgentSynchronized { thread },
            Event::AgentWindow { thread },
        ] {
            assert!(
                !event_visible(
                    &event,
                    &subscriptions,
                    &HashSet::new(),
                    &HelloClient::default()
                ),
                "{event:?} must not reach a peer that named nothing"
            );
            assert!(
                event_visible(&event, &subscriptions, &HashSet::new(), &capable),
                "{event:?} reaches a peer that named the capability"
            );
        }
        // A tag this build could not name is never forwarded, however capable the peer is.
        assert!(!event_visible(
            &Event::Unknown,
            &HashSet::from([EventKind::Toast]),
            &HashSet::new(),
            &capable,
        ));
    }

    #[test]
    fn terminal_events_are_visible_only_to_attached_subscribers() {
        let terminal = TerminalId(9);
        let event = Event::TerminalExited {
            terminal,
            code: Some(0),
        };
        let subscriptions = HashSet::from([EventKind::TerminalExited]);

        assert!(!event_visible(
            &event,
            &subscriptions,
            &HashSet::new(),
            &HelloClient::default(),
        ));
        assert!(event_visible(
            &event,
            &subscriptions,
            &HashSet::from([terminal]),
            &HelloClient::default(),
        ));
        assert!(!event_visible(
            &event,
            &HashSet::new(),
            &HashSet::from([terminal]),
            &HelloClient::default(),
        ));
    }
}
