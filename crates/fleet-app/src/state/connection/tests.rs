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
