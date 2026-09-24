//! The pointer's half of §3.8.6 and the header's search.
//!
//! Every click lands on the same update its key makes — a rail item is `Tab`, a row is `j`/`k`,
//! a switch is `Space`, a segment or a dropdown option is `h`/`l` landing on that option, and a
//! text or number box is `Enter` — and first puts the cursor on the row it touched, so the
//! keyboard carries on from where the pointer left it.

use fleet_ui_kit::{InputMode, TextInput, TextInputEvent};
use gpui::ClipboardItem;

use super::*;

/// A click on row `row`: the cursor moves there, and a text or number row opens its editor.
pub(super) fn click_row(
    state: &Entity<AppState>,
    row: usize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    put_cursor(state, row, focus, window, cx);
    begin_editing(state, window, cx);
}

/// Moves the cursor to `row` of the shown section, closing any editor and leaving the search
/// field, as `j`/`k` would.
fn put_cursor(
    state: &Entity<AppState>,
    row: usize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let moved = read_host(state, cx, |host, _| host.settings.row != row);
    if moved {
        end_editing(state, focus, window, cx);
    }
    leave_search_field(state, focus, window, cx);
    let len = focused_len(state, cx);
    with_host(state, cx, |host| {
        host.settings.row = row.min(len.saturating_sub(1));
    });
    notify(state, cx);
}

/// A click on option `option` of the choice in row `row`.
pub(super) fn select_option(
    state: &Entity<AppState>,
    row: usize,
    option: usize,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    put_cursor(state, row, focus, window, cx);
    let Some(focused) = focused_row(state, cx) else {
        return;
    };
    with_host(state, cx, |host| {
        let efforts = host.settings.efforts.clone();
        if let Some(config) = host.settings.config.as_mut() {
            select(config, &focused.id, option, &efforts);
        }
        host.settings.update_selected();
    });
    notify(state, cx);
}

/// A click on the switch in row `row`, asking for `on`.
pub(super) fn set_row_switch(
    state: &Entity<AppState>,
    row: usize,
    on: bool,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    put_cursor(state, row, focus, window, cx);
    let Some(focused) = focused_row(state, cx) else {
        return;
    };
    with_host(state, cx, |host| {
        if let Some(config) = host.settings.config.as_mut() {
            set_switch(config, &focused.id, on);
        }
        host.settings.update_selected();
    });
    notify(state, cx);
}

/// A copy button beside a read-only value: the value goes to the clipboard.
pub(super) fn copy_value(text: &str, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
}

/// Builds the header's search field for one opening of the dialog.
pub(super) fn seed_search(state: &Entity<AppState>, cx: &mut App) {
    let input = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_icon(Some(fleet_ui_kit::Icon::Search), cx);
        input.set_placeholder("Search settings", cx);
        input.set_hide_status_line(true, cx);
        input
    });
    // Weak, like every dialog editor's subscription: the field lives on `DialogHost`, which
    // `AppState` owns, so a strong capture would be a cycle holding the app alive.
    let watched = state.downgrade();
    let subscription = cx.subscribe(&input, move |input, event: &TextInputEvent, cx| {
        let Some(state) = watched.upgrade() else {
            return;
        };
        match event {
            TextInputEvent::Changed => {
                let query = input.read(cx).text().to_owned();
                with_host(&state, cx, |host| {
                    let search = host
                        .settings
                        .search
                        .get_or_insert_with(SearchState::default);
                    search.query = query;
                    search.cursor = 0;
                });
                refresh_rows(&state, cx);
                notify(&state, cx);
            }
            // A click on the field hands it the keyboard; the dialog's context word follows,
            // or the next focus reconciliation would move the caret back out.
            TextInputEvent::Focused => {
                let changed = with_host(&state, cx, |host| {
                    let changed = !host.settings.search_focused;
                    host.settings.search_focused = true;
                    // A row editor the click took the keyboard from is closed, as `j` would.
                    host.settings.editing = None;
                    host.settings_input_subscription = None;
                    host.settings_input = None;
                    changed
                });
                if changed {
                    notify(&state, cx);
                }
            }
            TextInputEvent::Blurred | TextInputEvent::Submitted => {}
        }
    });
    with_host(state, cx, |host| {
        host.settings_search = Some(input);
        host.settings_search_subscription = Some(subscription);
    });
}

/// `/`: the search field takes the keyboard.
pub(super) fn begin_search(
    state: &Entity<AppState>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    end_editing(state, focus, window, cx);
    let input = with_host(state, cx, |host| {
        host.settings.search_focused = true;
        host.settings_search.clone()
    });
    if let Some(input) = input {
        input.update(cx, |input, cx| input.focus(window, cx));
    }
    notify(state, cx);
}

/// Hands the keyboard back to the dialog from the search field, keeping what it holds.
fn leave_search_field(
    state: &Entity<AppState>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let was_focused = with_host(state, cx, |host| {
        std::mem::replace(&mut host.settings.search_focused, false)
    });
    if was_focused {
        window.focus(focus, cx);
    }
}

/// `Esc` in the search field, or a jump to a hit: the search is cleared and the dialog has the
/// keyboard again.
pub(super) fn end_search(
    state: &Entity<AppState>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    leave_search_field(state, focus, window, cx);
    let input = with_host(state, cx, |host| {
        host.settings.search = None;
        host.settings_search.clone()
    });
    // Clearing reports a change, which re-creates an empty search; an empty one shows the
    // section, and the next typed letter starts from it.
    if let Some(input) = input.filter(|input| !input.read(cx).text().is_empty()) {
        input.update(cx, |input, cx| input.clear(cx));
    }
    with_host(state, cx, |host| host.settings.search = None);
    notify(state, cx);
}

/// Whether a typed query is showing hits in place of the section.
pub(super) fn searching(state: &Entity<AppState>, cx: &mut App) -> bool {
    read_host(state, cx, |host, _| {
        host.settings
            .search
            .as_ref()
            .is_some_and(SearchState::active)
    })
}

/// `↓` / `↑` over the hits.
pub(super) fn move_hit(state: &Entity<AppState>, delta: isize, cx: &mut App) {
    with_host(state, cx, |host| {
        if let Some(search) = host.settings.search.as_mut() {
            search.cursor = step(search.cursor, delta, search.hits.len());
            host.settings.hit_scroll.scroll_to_item(search.cursor);
        }
    });
    notify(state, cx);
}

/// `Enter` on a hit, or a click on one: its section opens with the cursor on it.
pub(super) fn jump_to_hit(
    state: &Entity<AppState>,
    hit: Option<usize>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let target = read_host(state, cx, |host, _| {
        let search = host.settings.search.as_ref()?;
        let hit = search.hits.get(hit.unwrap_or(search.cursor))?;
        Some((hit.section.index(), hit.row))
    });
    if let Some((section, row)) = target {
        goto_row(state, section, row, focus, window, cx);
    }
}
