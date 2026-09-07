use super::{host::DialogHost, notify, with_host};
use crate::{actions::dialog, state::AppState};
use fleet_ui_kit::prelude::*;
use gpui::{App, Div, Entity, KeyDownEvent};

/// The kit field presenting a dialog draft: shared text plus the caret as a character index.
pub(crate) fn field(input: &TextFieldState) -> TextField {
    TextField::new(input.shared_text()).caret(input.caret_chars())
}

/// `ctrl-u` in a dialog empties the whole buffer, not just the text before the caret.
/// Returns whether anything changed.
pub(crate) fn clear_all(input: &mut TextFieldState) -> bool {
    if input.is_empty() {
        return false;
    }
    input.clear();
    true
}

/// The printable character a keystroke types, or `None` when it is not text input.
///
/// gpui dispatches key **bindings** before `on_key_down`, so a bound key never reaches this;
/// what arrives is exactly the printable set `docs/KEYMAP.md` gives to text inputs.
#[must_use]
pub(crate) fn typed_char(event: &KeyDownEvent) -> Option<&str> {
    let modifiers = event.keystroke.modifiers;
    if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
        return None;
    }
    let text = event.keystroke.key_char.as_deref()?;
    if text.is_empty() || text.chars().any(char::is_control) {
        return None;
    }
    Some(text)
}

/// Types a printable keystroke into `input`. Returns whether the buffer changed.
pub(crate) fn type_into(input: &mut TextFieldState, event: &KeyDownEvent) -> bool {
    match typed_char(event) {
        Some(text) => {
            input.insert(text);
            true
        }
        None => false,
    }
}

/// The editing and line-motion half of KEYMAP §3.8's input keys.
///
/// Dialogs that give the arrows a second job — Create's host cycler — bind only this half and
/// handle `left` / `right` themselves.
pub(super) fn edit_actions(
    root: Div,
    state: &Entity<AppState>,
    input: fn(&mut DialogHost) -> &mut TextFieldState,
    changed: impl Fn(&Entity<AppState>, &mut App) + Clone + 'static,
) -> Div {
    macro_rules! edit {
        ($root:expr, $action:ty, $operation:expr) => {{
            let state = state.clone();
            let changed = changed.clone();
            $root.on_action(move |_: &$action, _, cx| {
                if with_host(&state, cx, |host| $operation(input(host))) {
                    changed(&state, cx);
                }
            })
        }};
    }
    macro_rules! caret {
        ($root:expr, $action:ty, $operation:expr) => {{
            let state = state.clone();
            $root.on_action(move |_: &$action, _, cx| {
                with_host(&state, cx, |host| $operation(input(host)));
                notify(&state, cx);
            })
        }};
    }
    let root = edit!(root, dialog::Backspace, TextFieldState::backspace);
    let root = edit!(root, dialog::DeleteWord, TextFieldState::delete_word_before);
    let root = edit!(root, dialog::ClearInput, clear_all);
    let root = caret!(root, dialog::LineStart, TextFieldState::move_to_start);
    caret!(root, dialog::LineEnd, TextFieldState::move_to_end)
}

/// Every input key of KEYMAP §3.8, for dialogs whose single field owns the arrows too.
pub(super) fn actions(
    root: Div,
    state: &Entity<AppState>,
    input: fn(&mut DialogHost) -> &mut TextFieldState,
    changed: impl Fn(&Entity<AppState>, &mut App) + Clone + 'static,
) -> Div {
    let left = state.clone();
    let right = state.clone();
    edit_actions(root, state, input, changed)
        .on_action(move |_: &dialog::CursorLeft, _, cx| {
            with_host(&left, cx, |host| input(host).move_left());
            notify(&left, cx);
        })
        .on_action(move |_: &dialog::CursorRight, _, cx| {
            with_host(&right, cx, |host| input(host).move_right());
            notify(&right, cx);
        })
}

#[cfg(test)]
mod tests;
