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
        /// `home` / `cmd-left` / `ctrl-a`: move to the logical line start.
        MoveToLineStart,
        /// `end` / `cmd-right` / `ctrl-e`: move to the logical line end.
        MoveToLineEnd,
        /// `up`: move one logical line up.
        MoveUp,
        /// `down`: move one logical line down.
        MoveDown,
        /// `cmd-up`: move to the document start.
        MoveToStart,
        /// `cmd-down`: move to the document end.
        MoveToEnd,
        /// `shift-left`: extend the selection left by one grapheme.
        SelectLeft,
        /// `shift-right`: extend the selection right by one grapheme.
        SelectRight,
        /// `shift-alt-left`: extend the selection to the previous word boundary.
        SelectWordLeft,
        /// `shift-alt-right`: extend the selection to the next word boundary.
        SelectWordRight,
        /// `shift-home` / `shift-cmd-left`: extend to the logical line start.
        SelectToLineStart,
        /// `shift-end` / `shift-cmd-right`: extend to the logical line end.
        SelectToLineEnd,
        /// `shift-up`: extend one logical line up.
        SelectUp,
        /// `shift-down`: extend one logical line down.
        SelectDown,
        /// `shift-cmd-up`: extend to the document start.
        SelectToStart,
        /// `shift-cmd-down`: extend to the document end.
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
