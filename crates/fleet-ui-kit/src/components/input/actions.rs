//! Editing actions handled by [`super::TextInput`].

gpui::actions!(
    text_input,
    [
        /// `left` / `ctrl-b`: move left by one grapheme.
        MoveLeft,
        /// `right` / `ctrl-f`: move right by one grapheme.
        MoveRight,
        /// `alt-left`: move to the previous word boundary.
        MoveWordLeft,
        /// `alt-right`: move to the next word boundary.
        MoveWordRight,
        /// `cmd-left` / `ctrl-a`: move to the logical line start.
        MoveToLineStart,
        /// `cmd-right` / `ctrl-e`: move to the logical line end.
        MoveToLineEnd,
        /// `home`: move to the current visual row start.
        MoveToRowStart,
        /// `end`: move to the current visual row end.
        MoveToRowEnd,
        /// `up`: move one visual row up, or one logical line without a layout.
        MoveUp,
        /// `down`: move one visual row down, or one logical line without a layout.
        MoveDown,
        /// `cmd-up`: move to the document start.
        MoveToStart,
        /// `cmd-down`: move to the document end.
        MoveToEnd,
        /// `shift-left`: extend the selection left by one grapheme.
        SelectLeft,
        /// `shift-right`: extend the selection right by one grapheme.
        SelectRight,
        /// `alt-shift-left`: extend the selection to the previous word boundary.
        SelectWordLeft,
        /// `alt-shift-right`: extend the selection to the next word boundary.
        SelectWordRight,
        /// `cmd-shift-left` / `ctrl-shift-a`: extend to the logical line start.
        SelectToLineStart,
        /// `cmd-shift-right` / `ctrl-shift-e`: extend to the logical line end.
        SelectToLineEnd,
        /// `shift-home`: extend to the current visual row start.
        SelectToRowStart,
        /// `shift-end`: extend to the current visual row end.
        SelectToRowEnd,
        /// `shift-up`: extend one visual row up, or one logical line without a layout.
        SelectUp,
        /// `shift-down`: extend one visual row down, or one logical line without a layout.
        SelectDown,
        /// `cmd-shift-up`: extend to the document start.
        SelectToStart,
        /// `cmd-shift-down`: extend to the document end.
        SelectToEnd,
        /// `cmd-a`: select the whole value.
        SelectAll,
        /// `backspace` / `ctrl-h`: delete the previous grapheme or selection.
        Backspace,
        /// `delete` / `ctrl-d`: delete the next grapheme or selection.
        Delete,
        /// `alt-backspace` / `ctrl-w`: delete the previous word run.
        DeleteWordBackward,
        /// `alt-delete`: delete the next word run.
        DeleteWordForward,
        /// `cmd-backspace` / `ctrl-u`: delete to the logical line start.
        DeleteToLineStart,
        /// `cmd-delete` / `ctrl-k`: delete to the logical line end.
        DeleteToLineEnd,
        /// `enter` in multi-line mode: insert a logical newline.
        Newline,
        /// `cmd-c`: copy the selection.
        Copy,
        /// `cmd-x`: copy and delete the selection.
        Cut,
        /// `cmd-v`: insert clipboard text.
        Paste,
        /// `cmd-z`: restore the previous editing snapshot.
        Undo,
        /// `cmd-shift-z`: restore the next editing snapshot.
        Redo,
    ]
);

/// The key context of a [`super::TextInput`] in multi-line mode.
const MULTILINE_CONTEXT: &str = "FleetTextInput && mode == multiline";
/// The key context of a multi-line [`super::TextInput`] whose owner left `enter` to the text.
const MULTILINE_NEWLINE_CONTEXT: &str = "FleetTextInput && mode == multiline && enter == newline";

/// Every binding a [`super::TextInput`] needs, for an app that has no key table of its own.
///
/// This is the whole editing vocabulary of `docs/KEYMAP.md`'s `FleetTextInput` table, in one
/// place, so the gallery, `fleet-lazygit` and the kit's own tests cannot drift from each other
/// or from the app. `fleet-app` re-states the same rows inside its `key_table!`, because that
/// macro also feeds the Help overlay and the documentation-drift test; a test there asserts the
/// two agree.
///
/// Nothing here belongs to a container: `enter`, `escape`, `tab` and every navigation key of the
/// surface around the editor stay unbound, and `ctrl-v` is never bound at all — it belongs to
/// the shells Fleet hosts.
#[must_use]
pub fn default_bindings() -> Vec<gpui::KeyBinding> {
    let context = Some(super::TEXT_INPUT_KEY_CONTEXT);
    vec![
        gpui::KeyBinding::new("left", MoveLeft, context),
        gpui::KeyBinding::new("right", MoveRight, context),
        gpui::KeyBinding::new("alt-left", MoveWordLeft, context),
        gpui::KeyBinding::new("alt-right", MoveWordRight, context),
        gpui::KeyBinding::new("home", MoveToRowStart, context),
        gpui::KeyBinding::new("end", MoveToRowEnd, context),
        gpui::KeyBinding::new("cmd-left", MoveToLineStart, context),
        gpui::KeyBinding::new("cmd-right", MoveToLineEnd, context),
        gpui::KeyBinding::new("up", MoveUp, context),
        gpui::KeyBinding::new("down", MoveDown, context),
        gpui::KeyBinding::new("cmd-up", MoveToStart, context),
        gpui::KeyBinding::new("cmd-down", MoveToEnd, context),
        gpui::KeyBinding::new("shift-left", SelectLeft, context),
        gpui::KeyBinding::new("shift-right", SelectRight, context),
        gpui::KeyBinding::new("alt-shift-left", SelectWordLeft, context),
        gpui::KeyBinding::new("alt-shift-right", SelectWordRight, context),
        gpui::KeyBinding::new("shift-home", SelectToRowStart, context),
        gpui::KeyBinding::new("shift-end", SelectToRowEnd, context),
        gpui::KeyBinding::new("cmd-shift-left", SelectToLineStart, context),
        gpui::KeyBinding::new("cmd-shift-right", SelectToLineEnd, context),
        gpui::KeyBinding::new("shift-up", SelectUp, context),
        gpui::KeyBinding::new("shift-down", SelectDown, context),
        gpui::KeyBinding::new("cmd-shift-up", SelectToStart, context),
        gpui::KeyBinding::new("cmd-shift-down", SelectToEnd, context),
        gpui::KeyBinding::new("ctrl-a", MoveToLineStart, context),
        gpui::KeyBinding::new("ctrl-e", MoveToLineEnd, context),
        gpui::KeyBinding::new("ctrl-shift-a", SelectToLineStart, context),
        gpui::KeyBinding::new("ctrl-shift-e", SelectToLineEnd, context),
        gpui::KeyBinding::new("ctrl-b", MoveLeft, context),
        gpui::KeyBinding::new("ctrl-f", MoveRight, context),
        gpui::KeyBinding::new("backspace", Backspace, context),
        gpui::KeyBinding::new("delete", Delete, context),
        gpui::KeyBinding::new("alt-backspace", DeleteWordBackward, context),
        gpui::KeyBinding::new("alt-delete", DeleteWordForward, context),
        gpui::KeyBinding::new("cmd-backspace", DeleteToLineStart, context),
        gpui::KeyBinding::new("cmd-delete", DeleteToLineEnd, context),
        gpui::KeyBinding::new("ctrl-w", DeleteWordBackward, context),
        gpui::KeyBinding::new("ctrl-u", DeleteToLineStart, context),
        gpui::KeyBinding::new("ctrl-k", DeleteToLineEnd, context),
        gpui::KeyBinding::new("ctrl-h", Backspace, context),
        gpui::KeyBinding::new("ctrl-d", Delete, context),
        gpui::KeyBinding::new("cmd-a", SelectAll, context),
        gpui::KeyBinding::new("cmd-c", Copy, context),
        gpui::KeyBinding::new("cmd-x", Cut, context),
        gpui::KeyBinding::new("cmd-v", Paste, context),
        gpui::KeyBinding::new("cmd-z", Undo, context),
        gpui::KeyBinding::new("cmd-shift-z", Redo, context),
        gpui::KeyBinding::new("enter", Newline, Some(MULTILINE_NEWLINE_CONTEXT)),
        gpui::KeyBinding::new("shift-enter", Newline, Some(MULTILINE_CONTEXT)),
    ]
}
