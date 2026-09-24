use std::time::Instant;

use super::*;
use crate::{bridge::BridgeEvent, state::AppState};

fn board() -> BoardId {
    "brd-reviews"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn a_changed_board_is_stale_once_and_an_unknown_board_is_ignored() {
    let mut mirror = SchedulesMirror::default();
    assert!(!mirror.mark_stale(&board()), "a board never asked about");
    assert!(mirror.stale_boards().is_empty());

    assert!(mirror.begin_load(&board()));
    assert!(!mirror.begin_load(&board()), "one load in flight at a time");
    mirror.apply(&board(), Vec::new());

    assert!(mirror.mark_stale(&board()));
    assert_eq!(mirror.stale_boards(), [board()]);
    assert!(mirror.begin_load(&board()));
    assert!(mirror.stale_boards().is_empty(), "the reload claimed it");
}

#[test]
fn a_change_during_a_load_survives_its_answer() {
    let mut mirror = SchedulesMirror::default();
    assert!(mirror.begin_load(&board()));
    assert!(mirror.mark_stale(&board()));
    assert!(
        mirror.stale_boards().is_empty(),
        "a board already loading is not asked twice"
    );
    mirror.apply(&board(), Vec::new());
    assert_eq!(
        mirror.stale_boards(),
        [board()],
        "the answer may predate the change, so the entry stays stale"
    );
}

#[test]
fn a_lost_connection_forgets_every_boards_schedules() {
    let mut state = AppState::new("/tmp/fleet-schedules-mirror", Instant::now());
    assert!(state.schedules.begin_load(&board()));
    state.schedules.apply(&board(), Vec::new());
    let revision = state.schedules.revision;

    state.apply_bridge_event(BridgeEvent::Disconnected { attempt: 1 }, Instant::now());

    assert!(state.schedules.entry(&board()).is_none());
    assert_ne!(state.schedules.revision, revision, "the strip repaints");
}

#[test]
fn a_failed_board_is_asked_again_once_per_activation_and_never_per_notify() {
    let other: BoardId = "brd-other"
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    let mut mirror = SchedulesMirror::default();
    assert!(
        mirror.wants_header_load(&board()),
        "a board never asked about"
    );
    assert!(mirror.begin_load(&board()));
    mirror.load_failed(&board(), "schedules.json is unreadable".to_owned());

    // The failure notifies, and the observation asks again with the same board shown.
    assert!(!mirror.wants_header_load(&board()));
    assert!(!mirror.wants_header_load(&board()));

    // Another board shown, then this one again: a new activation retries the failure once.
    assert!(mirror.wants_header_load(&other));
    assert!(mirror.wants_header_load(&board()));
    assert!(!mirror.wants_header_load(&board()));

    // A loaded board is not asked again from the header, even on a new activation.
    mirror.apply(&board(), Vec::new());
    assert!(mirror.wants_header_load(&other), "never asked about");
    assert!(!mirror.wants_header_load(&board()));
}
