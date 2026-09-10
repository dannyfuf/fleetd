use fleet_core::{ids::TerminalId, watches::WatchId};
use fleet_proto::event::Event;

/// Conservative affected regions, not evidence that a reducer accepted an event or changed data.
/// Ordered terminal deltas must still all be applied before notifying these regions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventDamage {
    pub domain: bool,
    pub jobs: bool,
    pub sessions: bool,
    pub chrome: bool,
    pub notifications: bool,
    pub connection: bool,
    pub terminal: Option<TerminalId>,
    pub watch: Option<WatchId>,
    pub watch_structure: bool,
}

pub fn event_damage(event: &Event) -> EventDamage {
    match event {
        // The board mirror is not part of the Hub's domain projection: `AppState` marks it
        // stale and the board screen reloads it, so no cached row here is invalidated.
        Event::BoardChanged { .. } => EventDamage::default(),
        Event::SnapshotChanged(_) => EventDamage {
            domain: true,
            jobs: true,
            sessions: true,
            chrome: true,
            notifications: true,
            ..EventDamage::default()
        },
        Event::JobUpdated(_) => EventDamage {
            jobs: true,
            chrome: true,
            notifications: true,
            ..EventDamage::default()
        },
        Event::SessionChanged(_) => EventDamage {
            sessions: true,
            chrome: true,
            ..EventDamage::default()
        },
        Event::AgentActivityChanged { terminal_id, .. } => EventDamage {
            sessions: true,
            chrome: true,
            notifications: true,
            terminal: Some(*terminal_id),
            ..EventDamage::default()
        },
        Event::Agent { .. } | Event::AgentSummary(_) => EventDamage {
            domain: true,
            chrome: true,
            notifications: true,
            ..EventDamage::default()
        },
        Event::TerminalFrame(frame) => EventDamage {
            terminal: Some(frame.terminal),
            sessions: frame.title.is_some(),
            ..EventDamage::default()
        },
        Event::TerminalExited { terminal, .. } | Event::TerminalTitle { terminal, .. } => {
            EventDamage {
                terminal: Some(*terminal),
                sessions: true,
                chrome: true,
                ..EventDamage::default()
            }
        }
        Event::TerminalReattach { terminal } => EventDamage {
            terminal: Some(*terminal),
            sessions: true,
            ..EventDamage::default()
        },
        Event::HostLinkChanged { .. } => EventDamage {
            domain: true,
            chrome: true,
            connection: true,
            ..EventDamage::default()
        },
        Event::WatchStarted(watch) | Event::WatchExited(watch) => EventDamage {
            watch: Some(watch.id),
            watch_structure: true,
            ..EventDamage::default()
        },
        Event::WatchOutput { watch, .. } => EventDamage {
            watch: Some(*watch),
            ..EventDamage::default()
        },
        Event::WatchDismissed(watch) => EventDamage {
            watch: Some(*watch),
            watch_structure: true,
            ..EventDamage::default()
        },
        Event::Toast { .. } => EventDamage {
            notifications: true,
            ..EventDamage::default()
        },
        Event::DaemonShuttingDown => EventDamage {
            connection: true,
            chrome: true,
            ..EventDamage::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn watch_output_does_not_rebuild_domain_lists() {
        let damage = event_damage(&Event::WatchOutput {
            watch: WatchId(7),
            chunks: Vec::new(),
        });
        assert_eq!(damage.watch, Some(WatchId(7)));
        assert!(!damage.domain && !damage.sessions && !damage.jobs && !damage.watch_structure);
        let damage = event_damage(&Event::TerminalTitle {
            terminal: TerminalId(4),
            title: "vim".into(),
        });
        assert!(damage.sessions && damage.chrome);
        assert!(!damage.domain && !damage.jobs);
    }
}
