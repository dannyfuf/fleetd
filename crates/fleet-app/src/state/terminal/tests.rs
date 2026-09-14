use super::*;
use crate::state::test_support::*;

#[test]
fn forward_sequence_gap_requires_full_recovery() {
    let mut grid = MirrorGrid::new(4, 2);
    grid.apply(&frame(1, true, vec![row(0, "old"), row(1, "keep")]));
    assert!(!grid.apply(&frame(3, false, vec![row(0, "lost")])));
    assert!(grid.desynced);
    assert_eq!(grid.row_text(0), "old");
    assert!(!grid.apply(&frame(4, false, vec![row(1, "bad")])));
    assert!(grid.apply(&frame(5, true, vec![row(0, "new"), row(1, "good")])));
    assert!(!grid.desynced);
    assert!(!grid.apply(&frame(2, true, vec![row(0, "stale")])));
    assert_eq!(grid.row_text(0), "new");
}

#[test]
fn shifted_rows_move_before_replacements_in_both_directions() {
    let mut grid = MirrorGrid::new(4, 2);
    let mut wrapped = row(1, "bbb");
    wrapped.wrapped = true;
    grid.apply(&frame(1, true, vec![row(0, "aaa"), wrapped]));
    let mut down = frame(2, false, vec![row(1, "ccc")]);
    down.shift = Some(1);
    assert!(grid.apply(&down));
    assert_eq!(grid.wrapped, vec![true, false]);
    assert_eq!(
        (grid.row_text(0), grid.row_text(1)),
        ("bbb".into(), "ccc".into())
    );
    let mut up = frame(3, false, vec![row(0, "aaa")]);
    up.shift = Some(-1);
    assert!(grid.apply(&up));
    assert_eq!(grid.wrapped, vec![false, true]);
    assert_eq!(
        (grid.row_text(0), grid.row_text(1)),
        ("aaa".into(), "bbb".into())
    );
}

#[test]
fn shift_across_history_epochs_freezes_rows_until_full_recovery() {
    let mut grid = MirrorGrid::new(4, 2);
    grid.apply(&frame(1, true, vec![row(0, "old"), row(1, "keep")]));
    let before = grid.lines.clone();
    let mut shifted = frame(2, false, vec![row(1, "new")]);
    shifted.shift = Some(1);
    shifted.viewport.history_epoch = 1;
    assert!(!grid.apply(&shifted));
    assert!(grid.desynced);
    assert_eq!(grid.lines, before);
    let mut recovery = frame(3, true, vec![row(0, "new"), row(1, "live")]);
    recovery.viewport.history_epoch = 1;
    assert!(grid.apply(&recovery));
    assert!(!grid.desynced);
    assert_eq!(grid.viewport.history_epoch, 1);
    assert_eq!(grid.row_text(0), "new");
}

#[test]
fn a_dropped_frame_refuses_diffs_until_a_full_frame_repairs_the_mirror() {
    // quality-F2: the client's broadcast buffer overflowed. The rows that changed inside
    // the gap are never re-sent, so applying the *next* diff leaves them permanently wrong.
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.apply_frame(&frame(1, true, vec![row(0, "abcd"), row(1, "efgh")]));

    state.desync_grids();
    assert!(
        state.grids[&TerminalId(1)].desynced,
        "the shell re-primes every desynced mirror"
    );

    // The last good frame stays on screen — it is still the best answer available.
    let grid = state
        .grids
        .get(&TerminalId(1))
        .unwrap_or_else(|| panic!("no grid"));
    assert!(grid.primed, "the painted rows are not blanked");
    assert_eq!(grid.row_text(0), "abcd");

    // A diff that skipped the gap is refused.
    state.apply_frame(&frame(9, false, vec![row(1, "zzzz")]));
    let grid = state
        .grids
        .get(&TerminalId(1))
        .unwrap_or_else(|| panic!("no grid"));
    assert_eq!(grid.row_text(1), "efgh", "a post-gap diff is not applied");

    // The full frame the shell asked for repairs it, and diffs flow again.
    state.apply_frame(&frame(10, true, vec![row(0, "wxyz"), row(1, "1234")]));
    state.apply_frame(&frame(11, false, vec![row(1, "5678")]));
    let grid = state
        .grids
        .get(&TerminalId(1))
        .unwrap_or_else(|| panic!("no grid"));
    assert!(!grid.desynced);
    assert_eq!(grid.row_text(0), "wxyz");
    assert_eq!(grid.row_text(1), "5678");
}

#[test]
fn a_lagged_bridge_event_desyncs_every_mirror() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.apply_frame(&frame(1, true, vec![row(0, "abcd")]));
    state.apply_bridge_event(BridgeEvent::EventsLagged { dropped: 12 }, now);
    assert!(
        state
            .grids
            .values()
            .all(|grid| grid.desynced && grid.primed)
    );
}

#[test]
fn mirror_grid_applies_full_then_diff_frames() {
    let mut grid = MirrorGrid::new(4, 2);
    let mut first = row(0, "abcd");
    first.wrapped = true;
    assert!(grid.apply(&frame(1, true, vec![first, row(1, "efgh")])));
    assert_eq!(grid.row_text(0), "abcd");
    assert_eq!(grid.row_text(1), "efgh");
    assert_eq!(grid.wrapped, vec![true, false]);

    assert!(grid.apply(&frame(2, false, vec![row(1, "zzzz")])));
    assert_eq!(grid.row_text(0), "abcd", "untouched rows survive a diff");
    assert_eq!(grid.row_text(1), "zzzz");
    assert_eq!(grid.wrapped, vec![true, false]);
    assert_eq!(grid.seq, 2);
}

#[test]
fn mirror_grid_drops_diffs_before_the_first_full_frame() {
    let mut grid = MirrorGrid::new(4, 2);
    assert!(!grid.apply(&frame(1, false, vec![row(0, "abcd")])));
    assert_eq!(grid.row_text(0), "");
    assert!(!grid.primed);
}

#[test]
fn mirror_grid_drops_out_of_order_diffs() {
    let mut grid = MirrorGrid::new(4, 2);
    grid.apply(&frame(5, true, vec![row(0, "abcd")]));
    assert!(!grid.apply(&frame(4, false, vec![row(0, "zzzz")])));
    assert_eq!(grid.row_text(0), "abcd");
}

#[test]
fn mirror_grid_resizes_on_a_full_frame() {
    let mut grid = MirrorGrid::new(4, 2);
    let mut wider = frame(1, true, vec![row(2, "xy")]);
    wider.cols = 8;
    wider.rows = 3;
    assert!(grid.apply(&wider));
    assert_eq!((grid.cols, grid.rows), (8, 3));
    assert_eq!(grid.lines.len(), 3);
    assert_eq!(grid.row_text(2), "xy");
}

#[test]
fn a_daemon_restart_drops_the_mirror_grids_and_says_so() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.apply_frame(&frame(1, true, vec![row(0, "hi")]));
    assert_eq!(state.grids.len(), 1);

    state.apply_bridge_event(
        BridgeEvent::Disconnected { attempt: 2 },
        now + Duration::from_secs(1),
    );
    assert!(state.daemon.is_lost());
    assert!(state.refuses_mutations());
    assert!(state.drops_terminal_keys());

    state.apply_bridge_event(
        BridgeEvent::Reconnected {
            restarted: true,
            snapshot: Box::new(snapshot()),
        },
        now + Duration::from_secs(2),
    );
    assert!(
        state.grids.is_empty(),
        "a restarted daemon rebuilds every grid from its holder's replay, so the mirrors are stale"
    );
    assert!(matches!(
        state.daemon,
        DaemonLink::Reconnected {
            restarted: true,
            ..
        }
    ));
}

#[test]
fn terminal_exit_touches_only_that_grid() {
    let now = Instant::now();
    let mut state = AppState::new("/tmp/fleet", now);
    state.apply_frame(&frame(1, true, vec![row(0, "hi")]));
    assert!(state.grids.contains_key(&TerminalId(1)));
    state.apply_terminal_exit(TerminalId(1), Some(1));
    assert_eq!(
        state.grids.get(&TerminalId(1)).and_then(|g| g.exit_code),
        Some(Some(1))
    );
    assert_eq!(state.grids[&TerminalId(1)].row_text(0), "hi");
}
