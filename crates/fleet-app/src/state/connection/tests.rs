use super::*;
use crate::state::test_support::*;

#[test]
fn reconnect_backoff_matches_the_spec() {
    let seconds: Vec<u64> = (0..6).map(|n| reconnect_backoff(n).as_secs()).collect();
    assert_eq!(seconds, vec![1, 2, 4, 8, 8, 8]);
}

#[test]
fn a_failed_link_takes_the_window_and_drops_the_overlay() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.open_overlay(Overlay::Dialog(Dialogs::Help));
    state.apply_bridge_event(
        BridgeEvent::ConnectFailed {
            message: "no socket".to_owned(),
            log_tail: Vec::new(),
            stale_socket: true,
        },
        now,
    );
    assert!(
        state.overlay.is_none(),
        "§3.12 B draws no overlay layer, so nothing may stay open behind it"
    );
    assert_eq!(state.context_chain(), vec!["Daemon", "Down"]);
}

#[test]
fn an_overlay_opened_after_the_link_failed_still_owns_the_keyboard() {
    // KM-01: `apply_bridge_event` closes what was open when the link fails, but nothing
    // stops one being opened *afterwards* — `ctrl-q` does exactly that. An overlay whose
    // keys are not in the chain answers nothing, and `Esc` cannot leave it.
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.apply_bridge_event(
        BridgeEvent::ConnectFailed {
            message: "no socket".to_owned(),
            log_tail: Vec::new(),
            stale_socket: true,
        },
        now,
    );
    assert_eq!(state.context_chain(), vec!["Daemon", "Down"]);
    state.open_overlay(Overlay::Dialog(Dialogs::Quit));
    assert_eq!(state.context_chain(), vec!["Dialog", "Quit"]);
    state.close_overlay();
    assert_eq!(state.context_chain(), vec!["Daemon", "Down"]);
}

#[test]
fn every_new_link_bumps_the_generation_screens_re_attach_on() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    let start = state.link_generation;
    state.apply_bridge_event(BridgeEvent::Connected(Box::new(snapshot())), now);
    assert_eq!(state.link_generation, start + 1);

    // A disconnect alone changes nothing: the attachment is still notionally held.
    state.apply_bridge_event(BridgeEvent::Disconnected { attempt: 1 }, now);
    assert_eq!(state.link_generation, start + 1);

    // Both reconnect shapes replace the socket, so both invalidate every attachment.
    state.apply_bridge_event(
        BridgeEvent::Reconnected {
            restarted: false,
            snapshot: Box::new(snapshot()),
        },
        now,
    );
    assert_eq!(state.link_generation, start + 2);
    state.apply_bridge_event(
        BridgeEvent::Reconnected {
            restarted: true,
            snapshot: Box::new(snapshot()),
        },
        now,
    );
    assert_eq!(state.link_generation, start + 3);
}

#[test]
fn the_reconnect_banner_expires_on_a_tick() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.daemon = DaemonLink::Reconnected {
        restarted: false,
        reattached: 0,
        since: now,
    };
    assert!(state.tick(now));
    assert!(matches!(state.daemon, DaemonLink::Reconnected { .. }));
    assert!(state.tick(now + Duration::from_secs(1)));
    assert_eq!(state.daemon, DaemonLink::Connected);
    assert!(
        !state.tick(now + Duration::from_secs(2)),
        "a connected, idle app never repaints on a tick"
    );
}

#[test]
fn a_daemon_that_will_not_start_owns_the_whole_window() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.apply_bridge_event(
        BridgeEvent::ConnectFailed {
            message: "no socket".to_owned(),
            log_tail: vec!["boom".to_owned()],
            stale_socket: true,
        },
        now,
    );
    assert_eq!(state.context_chain(), vec!["Daemon", "Down"]);
}

#[test]
fn restart_clears_daemon_local_identity() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    let session: SessionId = "payroll/feat"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    state.renamed_terminals.insert(TerminalId(7));
    state.touch_terminal(&session, TerminalId(7));

    let mut restarted_snapshot = snapshot();
    restarted_snapshot.sessions = vec![session_with(session.as_str(), &[7])];
    state.apply_bridge_event(
        BridgeEvent::Reconnected {
            restarted: true,
            snapshot: Box::new(restarted_snapshot),
        },
        now,
    );

    assert!(state.renamed_terminals.is_empty());
    assert!(state.terminal_mru.is_empty());
}

/// A remote host's status entry, as the daemon reports it before any link event.
fn host_status(link: LinkState) -> fleet_proto::snapshot::HostStatus {
    fleet_proto::snapshot::HostStatus {
        id: "dev-box".parse().unwrap_or_else(|error| panic!("{error}")),
        provider: "tailscale".to_owned(),
        version: None,
        link,
        address: Some("100.64.0.2".to_owned()),
        agent_binaries: None,
        reachable: link == LinkState::Ready,
        checked_at: "2026-09-04T12:00:00Z".to_owned(),
        error: Some("connect refused".to_owned()),
    }
}

fn link_event(link: LinkState, version: Option<&str>, error: Option<&str>) -> Event {
    Event::HostLinkChanged {
        host: "dev-box".parse().unwrap_or_else(|error| panic!("{error}")),
        link,
        version: version.map(str::to_owned),
        error: error.map(str::to_owned),
    }
}

fn mirrored_host(state: &AppState) -> fleet_proto::snapshot::HostStatus {
    state
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.hosts.first().cloned())
        .unwrap_or_else(|| panic!("the mirror holds the host the snapshot listed"))
}

/// §3: between two snapshots the event is the only news about a machine, so it is applied to
/// the mirror the header, the thread badge and the status glyph all read.
#[test]
fn a_host_link_event_patches_the_mirrored_host_status() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    let mut snapshot = snapshot();
    snapshot.hosts = vec![host_status(LinkState::Down)];
    state.apply_snapshot(snapshot, now);
    let before = state.snapshot_revision;

    state.apply_daemon_event(link_event(LinkState::Ready, Some("0.1.0"), None), now);

    let host = mirrored_host(&state);
    assert_eq!(host.link, LinkState::Ready);
    assert!(host.reachable, "a ready link is a reached machine");
    assert_eq!(host.version.as_deref(), Some("0.1.0"));
    assert_eq!(host.error, None, "the failure that is over is cleared");
    assert_ne!(
        state.snapshot_revision, before,
        "every projection keyed on the mirror has to be invalidated"
    );

    state.apply_daemon_event(
        link_event(LinkState::Down, None, Some("tailscale is down")),
        now,
    );
    let host = mirrored_host(&state);
    assert_eq!(host.link, LinkState::Down);
    assert!(!host.reachable);
    assert_eq!(host.error.as_deref(), Some("tailscale is down"));
    assert_eq!(
        host.version.as_deref(),
        Some("0.1.0"),
        "a dropped link has not made the daemon it handshook with forget its version"
    );

    // A connect attempt in flight decides nothing, so it may not claim the machine is gone.
    state.apply_daemon_event(link_event(LinkState::Connecting, None, None), now);
    let host = mirrored_host(&state);
    assert_eq!(host.link, LinkState::Connecting);
    assert!(!host.reachable);
}

/// A host the snapshot has not introduced yet is left alone rather than half-invented.
#[test]
fn a_link_event_for_an_unknown_host_changes_nothing() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.apply_snapshot(snapshot(), now);
    let before = state.snapshot_revision;

    state.apply_daemon_event(link_event(LinkState::Ready, None, None), now);

    assert!(
        state
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.hosts.is_empty())
    );
    assert_eq!(state.snapshot_revision, before);
}

/// P2-T08: the reattach ask is recorded per terminal and dies with the terminal itself.
#[test]
fn a_reattach_event_is_recorded_until_a_snapshot_forgets_the_terminal() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    let mut listed = snapshot();
    listed.sessions = vec![session_with("payroll/feat", &[1])];
    state.apply_snapshot(listed.clone(), now);

    state.apply_daemon_event(
        Event::TerminalReattach {
            terminal: TerminalId(1),
        },
        now,
    );
    state.apply_daemon_event(
        Event::TerminalReattach {
            terminal: TerminalId(7),
        },
        now,
    );
    assert!(state.reattach_pending.contains(&TerminalId(1)));
    assert!(state.reattach_pending.contains(&TerminalId(7)));

    // The next authoritative snapshot lists neither terminal 7 nor, later, terminal 1.
    state.apply_snapshot(listed, now);
    assert_eq!(
        state.reattach_pending.iter().copied().collect::<Vec<_>>(),
        vec![TerminalId(1)],
        "a request for a terminal the daemon no longer lists is not worth keeping"
    );
    state.apply_snapshot(snapshot(), now);
    assert!(state.reattach_pending.is_empty());
}
