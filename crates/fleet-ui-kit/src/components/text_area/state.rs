use gpui::{Keystroke, SharedString};

/// How many spaces one `Tab` inserts. Fleet never stores a hard tab in a description.
pub const TAB_WIDTH: usize = 2;

/// A multi-line editing model: an owned string, a byte cursor, a preferred column and the
/// first visible row.
///
/// Every mutation keeps the cursor on a `char` boundary and inside the string, so the state can
/// never index-panic. `\r\n` and `\r` are normalized to `\n` and hard tabs become [`TAB_WIDTH`]
/// spaces on the way in, so the stored text is exactly what a markdown renderer will see.
///
/// The **preferred column** is what makes `↑` / `↓` behave like every real editor: moving down
/// through a short line and out the other side lands back on the column the run started at.
/// Every other operation clears it, because any edit redefines where "here" is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextAreaState {
    text: String,
    cursor: usize,
    preferred_col: Option<usize>,
    scroll_row: usize,
}

impl TextAreaState {
    /// An empty editor.
    pub fn new() -> Self {
        Self::default()
    }

    /// An editor pre-filled with `text`, caret at the end.
    pub fn from_text(text: impl Into<String>) -> Self {
        let text = sanitize(&text.into());
        let cursor = text.len();
        Self {
            text,
            cursor,
            preferred_col: None,
            scroll_row: 0,
        }
    }

    /// The current value.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The current value as a `SharedString`, ready for [`super::TextArea::new`].
    pub fn shared_text(&self) -> SharedString {
        SharedString::from(self.text.clone())
    }

    /// Whether the editor is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The caret as a byte offset. Always on a `char` boundary.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The caret as a zero-based `(line, column)`, the column counted in **characters**.
    pub fn line_col(&self) -> (usize, usize) {
        let start = self.line_start(self.cursor);
        let line = self.text[..start].matches('\n').count();
        (line, self.text[start..self.cursor].chars().count())
    }

    /// The lines of the value, including a trailing empty one after a final `\n`.
    ///
    /// This is `split('\n')`, not `str::lines()`: an editor whose last keystroke was `Enter`
    /// must still show the empty line the caret sits on.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.text.split('\n')
    }

    /// How many lines the value has.
    pub fn line_count(&self) -> usize {
        self.text.matches('\n').count() + 1
    }

    /// The first visible row, for a caller that scrolls the box itself.
    pub fn scroll_row(&self) -> usize {
        self.scroll_row
    }

    /// Set the first visible row.
    pub fn set_scroll_row(&mut self, row: usize) {
        self.scroll_row = row.min(self.line_count().saturating_sub(1));
    }

    /// Scroll the minimum amount that puts the caret's line inside a `visible_rows` viewport.
    ///
    /// Returns whether the scroll row changed. Call it from the handler that moved the caret,
    /// never from `render`.
    pub fn reveal_cursor(&mut self, visible_rows: usize) -> bool {
        let rows = visible_rows.max(1);
        let (line, _) = self.line_col();
        let before = self.scroll_row;
        if line < self.scroll_row {
            self.scroll_row = line;
        } else if line >= self.scroll_row + rows {
            self.scroll_row = line + 1 - rows;
        }
        self.scroll_row != before
    }

    /// Replace the value; the caret lands at the end.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = sanitize(&text.into());
        self.cursor = self.text.len();
        self.preferred_col = None;
        self.scroll_row = 0;
    }

    /// Empty the editor.
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.preferred_col = None;
        self.scroll_row = 0;
    }

    /// Move the caret to a byte offset, snapped down to a `char` boundary.
    pub fn set_cursor(&mut self, offset: usize) {
        self.cursor = self.floor_boundary(offset);
        self.preferred_col = None;
    }

    /// Insert `text` at the caret. Newlines are kept; `\r\n`, `\r` and `\t` are normalized.
    pub fn insert(&mut self, text: &str) {
        let insertion = sanitize(text);
        if insertion.is_empty() {
            return;
        }
        self.text.insert_str(self.cursor, &insertion);
        self.cursor += insertion.len();
        self.preferred_col = None;
    }

    /// `Enter`: break the line at the caret.
    pub fn insert_newline(&mut self) {
        self.insert("\n");
    }

    /// `Tab`: insert [`TAB_WIDTH`] spaces. Fleet never stores a hard tab.
    pub fn insert_tab(&mut self) {
        self.insert("\t");
    }

    /// Delete the character before the caret, joining lines at a line start. Returns whether
    /// anything changed.
    pub fn backspace(&mut self) -> bool {
        let start = self.previous_boundary(self.cursor);
        if start == self.cursor {
            return false;
        }
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.preferred_col = None;
        true
    }

    /// Delete the character after the caret. Returns whether anything changed.
    pub fn delete_forward(&mut self) -> bool {
        let end = self.next_boundary(self.cursor);
        if end == self.cursor {
            return false;
        }
        self.text.replace_range(self.cursor..end, "");
        self.preferred_col = None;
        true
    }

    /// `ctrl-w`: delete the whitespace-delimited word before the caret.
    pub fn delete_word_before(&mut self) -> bool {
        let start = self.word_start_before(self.cursor);
        if start == self.cursor {
            return false;
        }
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.preferred_col = None;
        true
    }

    /// `alt-d`: delete the whitespace-delimited word after the caret.
    pub fn delete_word_after(&mut self) -> bool {
        let end = self.word_end_after(self.cursor);
        if end == self.cursor {
            return false;
        }
        self.text.replace_range(self.cursor..end, "");
        self.preferred_col = None;
        true
    }

    /// `ctrl-u`: delete from the start of the line to the caret.
    pub fn delete_to_line_start(&mut self) -> bool {
        let start = self.line_start(self.cursor);
        if start == self.cursor {
            return false;
        }
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.preferred_col = None;
        true
    }

    /// `ctrl-k`: delete from the caret to the end of the line, or the line break itself when
    /// the caret is already there.
    pub fn delete_to_line_end(&mut self) -> bool {
        let end = self.line_end(self.cursor);
        let end = if end == self.cursor {
            self.next_boundary(self.cursor)
        } else {
            end
        };
        if end == self.cursor {
            return false;
        }
        self.text.replace_range(self.cursor..end, "");
        self.preferred_col = None;
        true
    }

    /// `←`: one character left, across a line break.
    pub fn move_left(&mut self) -> bool {
        let target = self.previous_boundary(self.cursor);
        let moved = target != self.cursor;
        self.cursor = target;
        self.preferred_col = None;
        moved
    }

    /// `→`: one character right, across a line break.
    pub fn move_right(&mut self) -> bool {
        let target = self.next_boundary(self.cursor);
        let moved = target != self.cursor;
        self.cursor = target;
        self.preferred_col = None;
        moved
    }

    /// `↑`: the same column one line up, keeping the preferred column.
    pub fn move_up(&mut self) -> bool {
        let start = self.line_start(self.cursor);
        if start == 0 {
            return false;
        }
        let column = self.preferred_column();
        let previous_break = start - 1;
        let previous_start = self.line_start(previous_break);
        self.cursor = self.offset_for_column(previous_start, previous_break, column);
        self.preferred_col = Some(column);
        true
    }

    /// `↓`: the same column one line down, keeping the preferred column.
    pub fn move_down(&mut self) -> bool {
        let end = self.line_end(self.cursor);
        if end == self.text.len() {
            return false;
        }
        let column = self.preferred_column();
        let next_start = end + 1;
        let next_end = self.line_end(next_start);
        self.cursor = self.offset_for_column(next_start, next_end, column);
        self.preferred_col = Some(column);
        true
    }

    /// `Home` / `ctrl-a`: the start of the current line.
    pub fn move_to_line_start(&mut self) -> bool {
        let start = self.line_start(self.cursor);
        let moved = start != self.cursor;
        self.cursor = start;
        self.preferred_col = None;
        moved
    }

    /// `End` / `ctrl-e`: the end of the current line.
    pub fn move_to_line_end(&mut self) -> bool {
        let end = self.line_end(self.cursor);
        let moved = end != self.cursor;
        self.cursor = end;
        self.preferred_col = None;
        moved
    }

    /// The very start of the value.
    pub fn move_to_start(&mut self) -> bool {
        let moved = self.cursor != 0;
        self.cursor = 0;
        self.preferred_col = None;
        moved
    }

    /// The very end of the value.
    pub fn move_to_end(&mut self) -> bool {
        let moved = self.cursor != self.text.len();
        self.cursor = self.text.len();
        self.preferred_col = None;
        moved
    }

    /// Handle one non-printable edit keystroke, `Enter` and `Tab` included.
    ///
    /// This is the half an IME-safe host binds: printable characters arrive through the
    /// platform input handler, and only the control keys are consumed here. Returns whether
    /// the keystroke was consumed.
    pub fn handle_edit_keystroke(&mut self, keystroke: &Keystroke) -> bool {
        let modifiers = &keystroke.modifiers;
        let control = modifiers.control && !modifiers.platform && !modifiers.alt;
        let alt = modifiers.alt && !modifiers.control && !modifiers.platform;
        match keystroke.key.as_str() {
            "enter" | "return" if !control => {
                self.insert_newline();
            }
            "tab" if !control && !modifiers.shift => {
                self.insert_tab();
            }
            "backspace" if !control => {
                self.backspace();
            }
            "delete" if !control => {
                self.delete_forward();
            }
            "left" if !control => {
                self.move_left();
            }
            "right" if !control => {
                self.move_right();
            }
            "up" if !control => {
                self.move_up();
            }
            "down" if !control => {
                self.move_down();
            }
            "home" => {
                self.move_to_line_start();
            }
            "end" => {
                self.move_to_line_end();
            }
            "a" if control => {
                self.move_to_line_start();
            }
            "e" if control => {
                self.move_to_line_end();
            }
            "b" if control => {
                self.move_left();
            }
            "f" if control => {
                self.move_right();
            }
            "p" if control => {
                self.move_up();
            }
            "n" if control => {
                self.move_down();
            }
            "w" if control => {
                self.delete_word_before();
            }
            "d" if alt => {
                self.delete_word_after();
            }
            "u" if control => {
                self.delete_to_line_start();
            }
            "k" if control => {
                self.delete_to_line_end();
            }
            "h" if control => {
                self.backspace();
            }
            "d" if control => {
                self.delete_forward();
            }
            _ => return false,
        }
        true
    }

    /// Handle a keystroke including printable insertion.
    ///
    /// For a caller that renders the presentational [`super::TextArea`] and receives raw key events,
    /// which is every card dialog. Returns whether the keystroke was consumed.
    pub fn handle_keystroke(&mut self, keystroke: &Keystroke) -> bool {
        if self.handle_edit_keystroke(keystroke) {
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

    /// The column `↑` / `↓` aim at: the remembered one, else where the caret is now.
    fn preferred_column(&self) -> usize {
        self.preferred_col.unwrap_or_else(|| self.line_col().1)
    }

    /// The byte offset of the start of the line containing `offset`.
    fn line_start(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.text[..offset].rfind('\n').map_or(0, |index| index + 1)
    }

    /// The byte offset of the end of the line containing `offset`, i.e. its `\n` or the end of
    /// the value.
    fn line_end(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.text[offset..]
            .find('\n')
            .map_or(self.text.len(), |index| offset + index)
    }

    /// The offset `column` characters into the line `start..end`, clamped to `end`.
    fn offset_for_column(&self, start: usize, end: usize, column: usize) -> usize {
        self.text[start..end]
            .char_indices()
            .nth(column)
            .map_or(end, |(index, _)| start + index)
    }

    /// The start of the whitespace-delimited word before `offset` (`ctrl-w`'s target).
    fn word_start_before(&self, offset: usize) -> usize {
        let bytes = self.text.as_bytes();
        let mut start = offset;
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
        start
    }

    /// The end of the whitespace-delimited word after `offset`.
    fn word_end_after(&self, offset: usize) -> usize {
        let bytes = self.text.as_bytes();
        let mut end = offset;
        while end < self.text.len() && bytes[end].is_ascii_whitespace() {
            end = self.next_boundary(end);
        }
        while end < self.text.len() && !bytes[end].is_ascii_whitespace() {
            end = self.next_boundary(end);
        }
        end
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
            Some(character) => offset + character.len_utf8(),
            None => offset,
        }
    }
}

/// Normalize what a multi-line field is allowed to hold: `\n` line breaks and no hard tabs.
fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\t' => out.extend(std::iter::repeat_n(' ', TAB_WIDTH)),
            other => out.push(other),
        }
    }
    out
}
