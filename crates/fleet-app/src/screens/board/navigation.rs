use super::*;

/// The selected unarchived card, in the contract's column order and behind the filter.
#[must_use]
pub(crate) fn selected_card(state: &AppState) -> Option<&Card> {
    let view = state.board()?;
    let status = view.board.statuses.get(state.board.focus.column)?;
    board_screen::visible_cards(view, &status.id, &state.board.filter)
        .get(state.board.focus.row)
        .copied()
}

/// The board being shown, when one is loaded.
#[must_use]
pub(super) fn board_id(state: &AppState) -> Option<BoardId> {
    state.board().map(|view| view.board.id.clone())
}

/// Puts the focus on `card`, wherever the daemon's answer put it.
///
/// Every mutation lands here: a card that moved column, gained a label or was just created is
/// still the card the user is working on, and a selection that stays behind is a selection that
/// silently points at somebody else's card.
pub(crate) fn focus_card(app: &mut AppState, card: &CardId) {
    let found = app.board.view.as_ref().and_then(|view| {
        view.board
            .statuses
            .iter()
            .enumerate()
            .find_map(|(column, status)| {
                board_screen::visible_cards(view, &status.id, &app.board.filter)
                    .iter()
                    .position(|candidate| &candidate.id == card)
                    .map(|row| (column, row))
            })
    });
    if let Some((column, row)) = found {
        app.board.focus.column = column;
        app.board.focus.row = row;
    }
    app.clamp_board_focus();
}

/// Moves the focus by whole columns and rows, clamped at both ends (§5.11: never wraps).
pub(super) fn step_focus(state: &Entity<AppState>, columns: isize, rows: isize, cx: &mut App) {
    let next = {
        let app = state.read(cx);
        let Some(view) = app.board() else {
            return;
        };
        let mut column = app.board.focus.column;
        let mut row = app.board.focus.row;
        if columns != 0 {
            column = crate::state::move_cursor(column, columns, view.board.statuses.len());
        }
        if rows != 0 {
            let len = view.board.statuses.get(column).map_or(0, |status| {
                board_screen::visible_cards(view, &status.id, &app.board.filter).len()
            });
            row = crate::state::move_cursor(row, rows, len);
        }
        (column, row)
    };
    state.update(cx, |app, cx| {
        app.board.focus.column = next.0;
        app.board.focus.row = next.1;
        app.clamp_board_focus();
        cx.notify();
    });
}

/// A mouse click on a tile or a column header.
pub(super) fn on_click(click: BoardClick, state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let (column, row, open) = match click {
        BoardClick::Column(column) => (column, None, false),
        BoardClick::Card(column, row) => (column, Some(row), false),
        BoardClick::OpenCard(column, row) => (column, Some(row), true),
    };
    state.update(cx, |app, cx| {
        app.board.focus.column = column;
        if let Some(row) = row {
            app.board.focus.row = row;
        }
        app.clamp_board_focus();
        cx.notify();
    });
    if open {
        open_card(state, bridge, cx);
    }
}

/// Runs `edit` against the filter query while the input owns the keyboard.
pub(super) fn edit_filter(state: &Entity<AppState>, cx: &mut App, edit: impl FnOnce(&mut String)) {
    let edited = state.update(cx, |app, cx| {
        if !app.board.filter_editing {
            return false;
        }
        edit(&mut app.board.filter);
        app.board.focus.row = 0;
        app.clamp_board_focus();
        cx.notify();
        true
    });
    if edited {
        cx.stop_propagation();
    } else {
        // The board is not in its filter input, so the same key belongs to the screen behind
        // this handler rather than to a text edit nobody asked for.
        cx.propagate();
    }
}

/// Leaves the input but keeps the filter, which is stage one of the §3.10 `Esc`.
pub(super) fn leave_filter_input(state: &Entity<AppState>, cx: &mut App) {
    state.update(cx, |app, cx| {
        app.board.filter_editing = false;
        cx.notify();
    });
}

/// Moves the card focus by whole rows; the Hub's shared cursor keys land here on this tab.
pub(crate) fn move_rows(state: &Entity<AppState>, rows: isize, cx: &mut App) {
    step_focus(state, 0, rows, cx);
}

/// Jumps the card focus to the first or last card of the focused column.
pub(crate) fn jump_rows(state: &Entity<AppState>, bottom: bool, cx: &mut App) {
    let len = {
        let app = state.read(cx);
        app.board().map_or(0, |view| {
            view.board
                .statuses
                .get(app.board.focus.column)
                .map_or(0, |status| {
                    board_screen::visible_cards(view, &status.id, &app.board.filter).len()
                })
        })
    };
    state.update(cx, |app, cx| {
        app.board.focus.row = if bottom { len.saturating_sub(1) } else { 0 };
        app.clamp_board_focus();
        cx.notify();
    });
}

/// `h` / `←` — previous column.
pub(crate) fn prev_column(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    step_focus(state, -1, 0, cx);
}

/// `l` / `→` — next column.
pub(crate) fn next_column(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    step_focus(state, 1, 0, cx);
}

/// `j` / `↓` — next card.
pub(crate) fn next_card(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    step_focus(state, 0, 1, cx);
}

/// `k` / `↑` — previous card.
pub(crate) fn prev_card(state: &Entity<AppState>, _bridge: &Bridge, cx: &mut App) {
    step_focus(state, 0, -1, cx);
}
