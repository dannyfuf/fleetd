use std::ops::Range;

use gpui::{Keystroke, SharedString};

/// A single-line editing model: an owned string plus a byte cursor.
///
/// Every mutation keeps the cursor on a `char` boundary and inside the string, so the state can
/// never index-panic. `\n` and `\r` are stripped on insert — this is a single-line field.
///
/// The edit set is KEYMAP §3.8's, and nothing else: no selection, no undo, no word motion
/// beyond `ctrl-w`. Selection is deliberately absent — Fleet's fields are short, and the two
/// blue affordances of the design system are the focus ring and the caret.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextFieldState {
    text: String,
    cursor: usize,
    marked: Option<Range<usize>>,
}

impl TextFieldState {
    /// An empty field.
    pub fn new() -> Self {
        Self::default()
    }

    /// A field pre-filled with `text`, caret at the end.
    pub fn from_text(text: impl Into<String>) -> Self {
        let text = sanitize(&text.into());
        let cursor = text.len();
        Self {
            text,
            cursor,
            marked: None,
        }
    }

    /// The current value.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The current value as a `SharedString`, ready for [`TextField::new`].
    pub fn shared_text(&self) -> SharedString {
        SharedString::new(self.text.as_str())
    }

    /// Whether the field is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The caret as a byte offset.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The caret as a character index — what [`TextField::caret`] wants.
    pub fn caret_chars(&self) -> usize {
        self.text[..self.cursor].chars().count()
    }

    /// The IME's marked (composing) range, as byte offsets.
    pub fn marked_range(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// Replace the value; the caret lands at the end.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = sanitize(&text.into());
        self.cursor = self.text.len();
        self.marked = None;
    }

    /// Empty the field.
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.marked = None;
    }

    /// Move the caret to a byte offset, snapped down to a `char` boundary.
    pub fn set_cursor(&mut self, offset: usize) {
        self.cursor = self.floor_boundary(offset);
    }

    /// Insert `text` at the caret. Newlines are stripped.
    pub fn insert(&mut self, text: &str) {
        let insertion = sanitize(text);
        if insertion.is_empty() {
            return;
        }
        self.text.insert_str(self.cursor, &insertion);
        self.cursor += insertion.len();
        self.marked = None;
    }

    /// Delete the character before the caret. Returns whether anything changed.
    pub fn backspace(&mut self) -> bool {
        let start = self.previous_boundary(self.cursor);
        if start == self.cursor {
            return false;
        }
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.marked = None;
        true
    }

    /// Delete the character after the caret. Returns whether anything changed.
    pub fn delete_forward(&mut self) -> bool {
        let end = self.next_boundary(self.cursor);
        if end == self.cursor {
            return false;
        }
        self.text.replace_range(self.cursor..end, "");
        self.marked = None;
        true
    }

    /// `ctrl-w`: delete the whitespace-delimited word before the caret.
    pub fn delete_word_before(&mut self) -> bool {
        let bytes = self.text.as_bytes();
        let mut start = self.cursor;
        while start > 0 {
            let previous = self.previous_boundary(start);
            if bytes[previous].is_ascii_whitespace() {
                start = previous;
            } else {
                break;
            }
        }
        while start > 0 {
            let previous = self.previous_boundary(start);
            if bytes[previous].is_ascii_whitespace() {
                break;
            }
            start = previous;
        }
        if start == self.cursor {
            return false;
        }
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.marked = None;
        true
    }

    /// `ctrl-u`: delete everything before the caret.
    pub fn delete_to_start(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.text.replace_range(..self.cursor, "");
        self.cursor = 0;
        self.marked = None;
        true
    }

    /// `ctrl-k`: delete everything after the caret.
    pub fn delete_to_end(&mut self) -> bool {
        if self.cursor == self.text.len() {
            return false;
        }
        self.text.truncate(self.cursor);
        self.marked = None;
        true
    }

    /// `←`: one character left.
    pub fn move_left(&mut self) -> bool {
        let target = self.previous_boundary(self.cursor);
        let moved = target != self.cursor;
        self.cursor = target;
        moved
    }

    /// `→`: one character right.
    pub fn move_right(&mut self) -> bool {
        let target = self.next_boundary(self.cursor);
        let moved = target != self.cursor;
        self.cursor = target;
        moved
    }

    /// `ctrl-a` / `Home`.
    pub fn move_to_start(&mut self) -> bool {
        let moved = self.cursor != 0;
        self.cursor = 0;
        moved
    }

    /// `ctrl-e` / `End`.
    pub fn move_to_end(&mut self) -> bool {
        let moved = self.cursor != self.text.len();
        self.cursor = self.text.len();
        moved
    }

    /// Replace a byte range with `text` and leave the caret after the insertion.
    ///
    /// The range is snapped to `char` boundaries, so an IME range that is stale by a frame
    /// cannot panic the app.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str) {
        let start = self.floor_boundary(range.start);
        let end = self.floor_boundary(range.end.max(start));
        let insertion = sanitize(text);
        self.text.replace_range(start..end, &insertion);
        self.cursor = start + insertion.len();
        self.marked = None;
    }

    /// Replace a byte range and mark the insertion as IME-composing text.
    pub fn replace_and_mark(&mut self, range: Range<usize>, text: &str) {
        let start = self.floor_boundary(range.start);
        self.replace_range(range, text);
        let insertion_len = self.cursor - start;
        self.marked = (insertion_len > 0).then_some(start..start + insertion_len);
    }

    /// Drop the IME marked range without touching the text.
    pub fn unmark(&mut self) {
        self.marked = None;
    }

    /// Apply one non-printable editing key, reporting what it changed without copying the
    /// buffer to detect it.
    ///
    /// This is the half a [`TextInput`] binds: printable characters arrive through the platform
    /// input handler (so dead keys and IME composition keep working), and only the control keys
    /// are consumed here.
    pub fn edit_keystroke(&mut self, keystroke: &Keystroke) -> EditEffect {
        let modifiers = &keystroke.modifiers;
        let control = modifiers.control && !modifiers.platform && !modifiers.alt;
        let (changed, text) = match keystroke.key.as_str() {
            "backspace" if !modifiers.control => (self.backspace(), true),
            "delete" if !modifiers.control => (self.delete_forward(), true),
            "left" if !modifiers.control => (self.move_left(), false),
            "right" if !modifiers.control => (self.move_right(), false),
            "home" => (self.move_to_start(), false),
            "end" => (self.move_to_end(), false),
            "a" if control => (self.move_to_start(), false),
            "e" if control => (self.move_to_end(), false),
            "b" if control => (self.move_left(), false),
            "f" if control => (self.move_right(), false),
            "w" if control => (self.delete_word_before(), true),
            "u" if control => (self.delete_to_start(), true),
            "k" if control => (self.delete_to_end(), true),
            "h" if control => (self.backspace(), true),
            "d" if control => (self.delete_forward(), true),
            _ => return EditEffect::Ignored,
        };
        match (changed, text) {
            (false, _) => EditEffect::Unchanged,
            (true, true) => EditEffect::Changed,
            (true, false) => EditEffect::CaretMoved,
        }
    }

    /// Handle a keystroke including printable insertion.
    ///
    /// For callers that render the presentational [`TextField`] and receive raw key events —
    /// the Hub's filter bar and the palette, which own their own query. A field that installs
    /// the platform input handler must use [`TextFieldState::edit_keystroke`] instead, or every
    /// character is inserted twice.
    pub fn handle_keystroke(&mut self, keystroke: &Keystroke) -> bool {
        if self.edit_keystroke(keystroke) != EditEffect::Ignored {
            return true;
        }
        let modifiers = &keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.function {
            return false;
        }
        match keystroke.key_char.as_deref() {
            Some(text) if !text.is_empty() && !text.chars().any(char::is_control) => {
                self.insert(text);
                true
            }
            _ => false,
        }
    }

    /// Convert a byte offset to a UTF-16 offset, for the platform input handler.
    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.text[..offset].chars().map(char::len_utf16).sum()
    }

    /// Convert a UTF-16 offset to a byte offset, for the platform input handler.
    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        for (byte, ch) in self.text.char_indices() {
            if utf16 >= offset {
                return byte;
            }
            utf16 += ch.len_utf16();
        }
        self.text.len()
    }

    /// The whole value's length in UTF-16 code units.
    pub fn len_utf16(&self) -> usize {
        self.text.chars().map(char::len_utf16).sum()
    }

    /// Byte range to UTF-16 range.
    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    /// UTF-16 range to byte range.
    pub fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    /// The nearest `char` boundary at or before `offset`, clamped to the string.
    fn floor_boundary(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while offset > 0 && !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    /// The `char` boundary before `offset`.
    fn previous_boundary(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.text[..offset]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    /// The `char` boundary after `offset`.
    fn next_boundary(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        match self.text[offset..].chars().next() {
            Some(ch) => offset + ch.len_utf8(),
            None => offset,
        }
    }
}

/// Strip the characters a single-line field must never hold.
fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|ch| *ch != '\n' && *ch != '\r' && *ch != '\t')
        .collect()
}

/// The observable effect of a non-printable editing key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditEffect {
    /// This key belongs to another handler.
    Ignored,
    /// A recognized key at an editing boundary changed nothing.
    Unchanged,
    /// The value changed.
    Changed,
    /// Only the caret moved.
    CaretMoved,
}
