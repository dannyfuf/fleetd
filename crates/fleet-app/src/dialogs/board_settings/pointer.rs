//! What a click does to the Board settings draft (ADR 0023).
//!
//! Every verb here is the keyboard's own, reached from where the pointer landed: a click on a
//! row is the `j` / `k` that would put the cursor there, a click on an option is the `h` / `l`
//! that would reach it, a switch is `space`, a column's double-click is `⏎`. So the draft moves
//! through exactly the states the keys move it through, and nothing a key refuses (a locked
//! automation row, a save in flight) is reachable by clicking instead.

use super::*;

/// A step far below the first value of every closed choice this dialog cycles. The steps are
/// clamped, so this lands on the first option from anywhere — including a stored value that is
/// off the grid — and a second step of `n` then lands on option `n`.
const TO_FIRST: isize = -1024;

/// A click on the rail: the section `⇥` would reach after this many presses.
pub(super) fn select_section(
    state: &Entity<AppState>,
    bridge: &Bridge,
    index: usize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let current = read_host(state, cx, |host, _| {
        host.board_settings
            .sections()
            .iter()
            .position(|section| *section == host.board_settings.section)
            .unwrap_or(0)
    });
    let delta = index as isize - current as isize;
    if delta == 0 {
        return;
    }
    if !cycle_section(state, delta, cx) {
        return;
    }
    materialize_input(state, Some(window), Some(focus), cx);
    load_board_schedules(state, bridge, cx);
}

/// A click on row `row` of the open pane: the cursor moves there, as `j` / `k` would move it.
///
/// A click on the row that already has the cursor does nothing, so a click inside the open
/// editor places its caret instead of rebuilding it. An armed delete keeps its notice: the
/// cursor is how the user points at the column the cards move to.
pub(super) fn select_row(
    state: &Entity<AppState>,
    row: usize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let moved = with_host(state, cx, |host| {
        let draft = &mut host.board_settings;
        if draft.saving || draft.row == row || row >= draft.rows().len() {
            return false;
        }
        draft.row = row;
        draft.editing = false;
        draft.discard_armed = false;
        if draft.pending_delete.is_none() {
            draft.notice = None;
        }
        // A schedule's armed delete belongs to the row it was asked on (§3.13).
        if draft.schedules.pending_delete.take().is_some() {
            draft.notice = None;
        }
        true
    });
    if moved {
        materialize_input(state, Some(window), Some(focus), cx);
    }
}

/// A click on option `option` of the closed choice on row `row`, which shows option `current`
/// (`None` for a stored value off the grid).
pub(super) fn pick(
    state: &Entity<AppState>,
    row: usize,
    option: usize,
    current: Option<usize>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    select_row(state, row, focus, window, cx);
    if current == Some(option) {
        return;
    }
    cycle(state, TO_FIRST, cx);
    if option > 0 {
        cycle(state, option as isize, cx);
    }
}

/// A click on the switch of the flag on row `row`: `l` turns it on and `h` off.
pub(super) fn switch(
    state: &Entity<AppState>,
    row: usize,
    on: bool,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    select_row(state, row, focus, window, cx);
    cycle(state, if on { 1 } else { -1 }, cx);
}

/// A double-click on a column in the list: `⏎`, which drills into it.
pub(super) fn open_column(
    state: &Entity<AppState>,
    row: usize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    select_row(state, row, focus, window, cx);
    confirm_column(state, window, focus, cx);
}

/// A click on a column while a delete waits for a target: the cards move there, as `⏎` on that
/// column would move them.
pub(super) fn choose_target(
    state: &Entity<AppState>,
    bridge: &Bridge,
    row: usize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    select_row(state, row, focus, window, cx);
    delete_with_cards(state, bridge, cx);
}
