use std::{ops::Range, time::Instant};

use unicode_segmentation::UnicodeSegmentation;

use super::history::{EditKind, InputHistory, Snapshot};

/// Whether an input holds one line or logical newline-delimited lines.
///
/// Explicitly inserted or pasted tabs become one space in [`SingleLine`](Self::SingleLine) and
/// remain hard tabs in [`Multiline`](Self::Multiline). A live single-line input should let the
/// Tab key propagate for field navigation; this rule only governs text arriving through insert
/// or paste. Single-line CRLF, CR, and LF line endings each become one space. Multiline line
/// endings are normalized to LF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    /// A value with no line breaks.
    SingleLine,
    /// A logical-line editor whose rendered component grows within the row bounds.
    Multiline {
        /// Minimum rendered row count.
        min_rows: usize,
        /// Maximum rendered row count before scrolling.
        max_rows: usize,
    },
}

impl InputMode {
    fn is_multiline(self) -> bool {
        matches!(self, Self::Multiline { .. })
    }
}

/// A pure UTF-8 text-editing engine with grapheme-safe selection and snapshot history.
///
/// The caret and anchor are byte offsets, but every public operation snaps them to extended
/// grapheme boundaries. User text mutations take an [`Instant`] supplied by the caller so undo
/// grouping is deterministic and the engine never reads a wall clock.
#[derive(Clone, Debug)]
pub struct InputBuffer {
    mode: InputMode,
    text: String,
    caret: usize,
    anchor: usize,
    marked: Option<Range<usize>>,
    goal_column: Option<usize>,
    revision: u64,
    history: InputHistory,
}

impl InputBuffer {
    /// Create an empty buffer in `mode`.
    pub fn new(mode: InputMode) -> Self {
        Self {
            mode,
            text: String::new(),
            caret: 0,
            anchor: 0,
            marked: None,
            goal_column: None,
            revision: 0,
            history: InputHistory::default(),
        }
    }

    /// Create a buffer containing `text`, with the caret at its end and empty history.
    pub fn from_text(mode: InputMode, text: impl Into<String>) -> Self {
        let mut buffer = Self::new(mode);
        buffer.set_text(text);
        buffer
    }

    /// The configured editing mode.
    pub fn mode(&self) -> InputMode {
        self.mode
    }

    /// The current UTF-8 value.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the buffer contains no text.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Whether the buffer contains only whitespace.
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// The caret as a UTF-8 byte offset.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selection anchor as a UTF-8 byte offset.
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// The monotonically increasing text revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The selected byte range, low end first.
    pub fn selected_range(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    /// Whether the anchor and caret enclose text.
    pub fn has_selection(&self) -> bool {
        self.anchor != self.caret
    }

    /// The selected text, or an empty string for a collapsed selection.
    pub fn selected_text(&self) -> &str {
        &self.text[self.selected_range()]
    }

    /// The IME marked range as UTF-8 byte offsets.
    pub fn marked_range(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// Whether an undo snapshot is available.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Whether a redo snapshot is available.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Replace the value programmatically, park the caret at the end, and clear history.
    pub fn set_text(&mut self, text: impl Into<String>) {
        let text = sanitize(self.mode, &text.into());
        if self.text != text {
            self.text = text;
            self.bump_revision();
        }
        self.caret = self.text.len();
        self.anchor = self.caret;
        self.marked = None;
        self.goal_column = None;
        self.history.clear();
    }

    /// Delete the whole value as one undoable edit.
    pub fn clear(&mut self, now: Instant) -> bool {
        self.replace_range_with_kind(0..self.text.len(), "", EditKind::Other, now)
    }

    /// Move the caret to `offset`, snapping down to a grapheme boundary.
    pub fn set_caret(&mut self, offset: usize) {
        self.place(offset, false);
    }

    /// Move the caret to `offset`, extending from the current anchor when `select` is true.
    pub fn move_to(&mut self, offset: usize, select: bool) -> bool {
        self.step(offset, select)
    }

    /// Set a platform selection, snapping both endpoints down to grapheme boundaries.
    ///
    /// The IME marked range remains intact because a platform can select within its preedit.
    pub fn set_selected_range(&mut self, range: Range<usize>) {
        self.anchor = self.floor_boundary(range.start);
        self.caret = self.floor_boundary(range.end);
        self.goal_column = None;
        self.history.break_typing_group();
    }

    /// Select the entire value, with the caret at the end.
    pub fn select_all(&mut self) -> bool {
        if self.text.is_empty() {
            return false;
        }
        self.set_selected_range(0..self.text.len());
        true
    }

    /// Select the class run at `offset`: word, whitespace, or punctuation.
    pub fn select_word_at(&mut self, offset: usize) -> bool {
        if self.text.is_empty() {
            return false;
        }
        let start = self.floor_boundary(offset.min(self.text.len().saturating_sub(1)));
        let class = self.class_at(start);
        let mut low = start;
        while low > self.line_start(start) {
            let previous = self.previous_boundary(low);
            if self.class_at(previous) != class {
                break;
            }
            low = previous;
        }
        let ceiling = self.line_end(start);
        let mut high = self.next_boundary(start);
        while high < ceiling && self.class_at(high) == class {
            high = self.next_boundary(high);
        }
        self.set_selected_range(low..high);
        true
    }

    /// Select the logical line at `offset`, including its trailing newline when present.
    pub fn select_line_at(&mut self, offset: usize) -> bool {
        let start = self.line_start(offset);
        let end = self.line_end(offset);
        let end = if end < self.text.len() {
            self.next_boundary(end)
        } else {
            end
        };
        if start == end {
            return false;
        }
        self.set_selected_range(start..end);
        true
    }

    /// Insert text at the caret, replacing a selection.
    pub fn insert(&mut self, text: &str, now: Instant) -> bool {
        let kind = if self.has_selection() {
            EditKind::Other
        } else {
            EditKind::Typing
        };
        self.replace_range_with_kind(self.selected_range(), text, kind, now)
    }

    /// Insert a logical newline in multiline mode; single-line mode inserts one space.
    pub fn insert_newline(&mut self, now: Instant) -> bool {
        self.insert("\n", now)
    }

    /// Insert a hard tab in multiline mode or one space in single-line mode.
    pub fn insert_tab(&mut self, now: Instant) -> bool {
        self.insert("\t", now)
    }

    /// Delete the grapheme before the caret, or the current selection.
    pub fn backspace(&mut self, now: Instant) -> bool {
        let range = if self.has_selection() {
            self.selected_range()
        } else {
            self.previous_boundary(self.caret)..self.caret
        };
        self.replace_range_with_kind(range, "", EditKind::Other, now)
    }

    /// Delete the grapheme after the caret, or the current selection.
    pub fn delete_forward(&mut self, now: Instant) -> bool {
        let range = if self.has_selection() {
            self.selected_range()
        } else {
            self.caret..self.next_boundary(self.caret)
        };
        self.replace_range_with_kind(range, "", EditKind::Other, now)
    }

    /// Delete one class run backward, including one adjacent whitespace grapheme.
    ///
    /// A whitespace run of two or more graphemes is deleted by itself, making deletion less
    /// greedy than word motion.
    pub fn delete_word_backward(&mut self, now: Instant) -> bool {
        if self.has_selection() {
            return self.backspace(now);
        }
        let start = self.word_deletion_start(self.caret);
        if start == self.caret {
            return self.backspace(now);
        }
        self.replace_range_with_kind(start..self.caret, "", EditKind::Other, now)
    }

    /// Delete one class run forward, including one leading whitespace grapheme.
    ///
    /// A whitespace run of two or more graphemes is deleted by itself.
    pub fn delete_word_forward(&mut self, now: Instant) -> bool {
        if self.has_selection() {
            return self.delete_forward(now);
        }
        let end = self.word_deletion_end(self.caret);
        if end == self.caret {
            return self.delete_forward(now);
        }
        self.replace_range_with_kind(self.caret..end, "", EditKind::Other, now)
    }

    /// Delete from the logical line start to the caret.
    ///
    /// In single-line mode the logical line is the entire value.
    pub fn delete_to_line_start(&mut self, now: Instant) -> bool {
        if self.has_selection() {
            return self.backspace(now);
        }
        let start = self.line_start(self.caret);
        self.replace_range_with_kind(start..self.caret, "", EditKind::Other, now)
    }

    /// Delete from the caret to the logical line end.
    ///
    /// When already at a multiline line end, this removes the newline and joins the next line.
    pub fn delete_to_line_end(&mut self, now: Instant) -> bool {
        if self.has_selection() {
            return self.delete_forward(now);
        }
        let end = self.line_end(self.caret);
        let end = if self.mode.is_multiline() && end == self.caret && end < self.text.len() {
            self.next_boundary(end)
        } else {
            end
        };
        self.replace_range_with_kind(self.caret..end, "", EditKind::Other, now)
    }

    /// Move left by one grapheme, or collapse a selection to its low end.
    pub fn move_left(&mut self, select: bool) -> bool {
        if !select && self.has_selection() {
            return self.step(self.selected_range().start, false);
        }
        self.step(self.previous_boundary(self.caret), select)
    }

    /// Move right by one grapheme, or collapse a selection to its high end.
    pub fn move_right(&mut self, select: bool) -> bool {
        if !select && self.has_selection() {
            return self.step(self.selected_range().end, false);
        }
        self.step(self.next_boundary(self.caret), select)
    }

    /// Move to the start of the previous word or punctuation run, skipping whitespace.
    pub fn move_word_left(&mut self, select: bool) -> bool {
        let target = self.word_start_before(self.caret);
        if target == self.caret {
            return self.move_left(select);
        }
        self.step(target, select)
    }

    /// Move to the end of the next word or punctuation run, skipping whitespace first.
    pub fn move_word_right(&mut self, select: bool) -> bool {
        let target = self.word_end_after(self.caret);
        if target == self.caret {
            return self.move_right(select);
        }
        self.step(target, select)
    }

    /// Move one logical line up while retaining a grapheme goal column.
    ///
    /// Returns false in single-line mode so the containing surface can handle navigation.
    pub fn move_up(&mut self, select: bool) -> bool {
        self.move_vertical(false, select)
    }

    /// Move one logical line down while retaining a grapheme goal column.
    ///
    /// Returns false in single-line mode so the containing surface can handle navigation.
    pub fn move_down(&mut self, select: bool) -> bool {
        self.move_vertical(true, select)
    }

    /// Move to the logical line start, or the document start in single-line mode.
    pub fn move_to_line_start(&mut self, select: bool) -> bool {
        self.step(self.line_start(self.caret), select)
    }

    /// Move to the logical line end, or the document end in single-line mode.
    pub fn move_to_line_end(&mut self, select: bool) -> bool {
        self.step(self.line_end(self.caret), select)
    }

    /// Move to the document start.
    pub fn move_to_document_start(&mut self, select: bool) -> bool {
        self.step(0, select)
    }

    /// Move to the document end.
    pub fn move_to_document_end(&mut self, select: bool) -> bool {
        self.step(self.text.len(), select)
    }

    /// Whether the caret is on the first logical line.
    pub fn on_first_line(&self) -> bool {
        !self.text[..self.caret].contains('\n')
    }

    /// Whether the caret is on the last logical line.
    pub fn on_last_line(&self) -> bool {
        !self.text[self.caret..].contains('\n')
    }

    /// Number of logical lines, including an empty line after a trailing newline.
    pub fn line_count(&self) -> usize {
        self.text.matches('\n').count() + 1
    }

    /// Logical lines, including an empty line after a trailing newline.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.text.split('\n')
    }

    /// Byte offset of the logical line start containing `offset`.
    pub fn line_start(&self, offset: usize) -> usize {
        if !self.mode.is_multiline() {
            return 0;
        }
        let offset = self.floor_boundary(offset);
        self.text[..offset].rfind('\n').map_or(0, |index| index + 1)
    }

    /// Byte offset of the logical line end containing `offset`, before its newline.
    pub fn line_end(&self, offset: usize) -> usize {
        if !self.mode.is_multiline() {
            return self.text.len();
        }
        let offset = self.floor_boundary(offset);
        self.text[offset..]
            .find('\n')
            .map_or(self.text.len(), |index| offset + index)
    }

    /// Replace a byte range and leave the caret after the insertion.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str, now: Instant) -> bool {
        self.replace_range_with_kind(range, text, EditKind::Other, now)
    }

    /// Replace a byte range and mark the inserted text as an IME preedit.
    pub fn replace_and_mark(&mut self, range: Range<usize>, text: &str, now: Instant) -> bool {
        let start = self.floor_boundary(range.start);
        let changed = self.replace_range_with_kind(range, text, EditKind::Other, now);
        let insertion_len = self.caret.saturating_sub(start);
        self.marked = (insertion_len > 0).then_some(start..start + insertion_len);
        changed
    }

    /// Drop the IME marked range without changing text.
    pub fn unmark(&mut self) {
        self.marked = None;
        self.anchor = self.caret;
    }

    /// Begin one undo group that remains open until [`Self::end_history_group`].
    pub fn begin_history_group(&mut self) {
        self.history.begin_group();
    }

    /// End an explicit undo group, normally after an IME composition commits or cancels.
    pub fn end_history_group(&mut self) {
        self.history.end_group();
    }

    /// Restore the previous text, caret, and anchor snapshot.
    pub fn undo(&mut self) -> bool {
        let current = self.snapshot();
        let Some(snapshot) = self.history.undo(current) else {
            return false;
        };
        self.restore(snapshot);
        true
    }

    /// Restore the next text, caret, and anchor snapshot.
    pub fn redo(&mut self) -> bool {
        let current = self.snapshot();
        let Some(snapshot) = self.history.redo(current) else {
            return false;
        };
        self.restore(snapshot);
        true
    }

    /// Convert a grapheme-safe UTF-8 byte offset to a UTF-16 code-unit offset.
    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.text[..offset].chars().map(char::len_utf16).sum()
    }

    /// Convert a UTF-16 code-unit offset to a grapheme-safe UTF-8 byte offset.
    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        self.floor_boundary(byte_offset_from_utf16(&self.text, offset))
    }

    /// Length of the value in UTF-16 code units.
    pub fn len_utf16(&self) -> usize {
        self.text.chars().map(char::len_utf16).sum()
    }

    /// Convert a UTF-8 byte range to a UTF-16 code-unit range.
    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    /// Convert a UTF-16 code-unit range to a grapheme-safe UTF-8 byte range.
    pub fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn replace_range_with_kind(
        &mut self,
        range: Range<usize>,
        text: &str,
        kind: EditKind,
        now: Instant,
    ) -> bool {
        let start = self.floor_boundary(range.start);
        let end = self.floor_boundary(range.end.max(start));
        let insertion = sanitize(self.mode, text);
        if self.text.get(start..end) == Some(insertion.as_str()) {
            self.place(start + insertion.len(), false);
            return false;
        }
        let before = self.snapshot();
        self.text.replace_range(start..end, &insertion);
        self.caret = start + insertion.len();
        self.anchor = self.caret;
        self.marked = None;
        self.goal_column = None;
        self.bump_revision();
        self.history.record(before, kind, now);
        true
    }

    fn place(&mut self, offset: usize, select: bool) {
        self.caret = self.floor_boundary(offset);
        if !select {
            self.anchor = self.caret;
        }
        self.marked = None;
        self.goal_column = None;
        self.history.break_typing_group();
    }

    fn step(&mut self, offset: usize, select: bool) -> bool {
        let had_selection = !select && self.has_selection();
        let target = self.floor_boundary(offset);
        let moved = target != self.caret || had_selection;
        self.place(target, select);
        moved
    }

    fn move_vertical(&mut self, down: bool, select: bool) -> bool {
        if !self.mode.is_multiline() {
            return false;
        }
        let start = self.line_start(self.caret);
        let column = self
            .goal_column
            .unwrap_or_else(|| self.text[start..self.caret].graphemes(true).count());
        let target_start = if down {
            let end = self.line_end(self.caret);
            if end == self.text.len() {
                return false;
            }
            end + 1
        } else {
            if start == 0 {
                return false;
            }
            self.line_start(start - 1)
        };
        let target_end = self.line_end(target_start);
        let line = &self.text[target_start..target_end];
        let local = line
            .grapheme_indices(true)
            .nth(column)
            .map_or(line.len(), |(index, _)| index);
        self.place(target_start + local, select);
        self.goal_column = Some(column);
        true
    }

    fn word_start_before(&self, offset: usize) -> usize {
        let floor = self.line_start(offset);
        let mut start = self.floor_boundary(offset);
        while start > floor {
            let previous = self.previous_boundary(start);
            if self.class_at(previous) != CharacterClass::Whitespace {
                break;
            }
            start = previous;
        }
        if start == floor {
            return start;
        }
        let class = self.class_at(self.previous_boundary(start));
        while start > floor {
            let previous = self.previous_boundary(start);
            if self.class_at(previous) != class {
                break;
            }
            start = previous;
        }
        start
    }

    fn word_end_after(&self, offset: usize) -> usize {
        let ceiling = self.line_end(offset);
        let mut end = self.floor_boundary(offset);
        while end < ceiling && self.class_at(end) == CharacterClass::Whitespace {
            end = self.next_boundary(end);
        }
        if end == ceiling {
            return end;
        }
        let class = self.class_at(end);
        while end < ceiling && self.class_at(end) == class {
            end = self.next_boundary(end);
        }
        end
    }

    fn word_deletion_start(&self, offset: usize) -> usize {
        let floor = self.line_start(offset);
        let mut start = self.floor_boundary(offset);
        let mut whitespace = 0;
        while start > floor {
            let previous = self.previous_boundary(start);
            if self.class_at(previous) != CharacterClass::Whitespace {
                break;
            }
            whitespace += 1;
            start = previous;
        }
        if whitespace >= 2 || start == floor {
            return start;
        }
        let class = self.class_at(self.previous_boundary(start));
        while start > floor {
            let previous = self.previous_boundary(start);
            if self.class_at(previous) != class {
                break;
            }
            start = previous;
        }
        start
    }

    fn word_deletion_end(&self, offset: usize) -> usize {
        let ceiling = self.line_end(offset);
        let mut end = self.floor_boundary(offset);
        let mut whitespace = 0;
        while end < ceiling && self.class_at(end) == CharacterClass::Whitespace {
            whitespace += 1;
            end = self.next_boundary(end);
        }
        if whitespace >= 2 || end == ceiling {
            return end;
        }
        let class = self.class_at(end);
        while end < ceiling && self.class_at(end) == class {
            end = self.next_boundary(end);
        }
        end
    }

    fn class_at(&self, offset: usize) -> CharacterClass {
        let end = self.next_boundary(offset);
        let grapheme = self.text.get(offset..end).unwrap_or_default();
        if grapheme.chars().all(char::is_whitespace) {
            CharacterClass::Whitespace
        } else if grapheme
            .chars()
            .any(|character| character.is_alphanumeric() || character == '_')
        {
            CharacterClass::Word
        } else {
            CharacterClass::Punctuation
        }
    }

    fn floor_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.text.len());
        if offset == self.text.len() {
            return offset;
        }
        self.text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .take_while(|index| *index <= offset)
            .last()
            .unwrap_or(0)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .take_while(|index| *index < offset)
            .last()
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .find(|index| *index > offset)
            .unwrap_or(self.text.len())
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            caret: self.caret,
            anchor: self.anchor,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        let text_changed = self.text != snapshot.text;
        self.text = snapshot.text;
        self.caret = snapshot.caret;
        self.anchor = snapshot.anchor;
        self.marked = None;
        self.goal_column = None;
        if text_changed {
            self.bump_revision();
        }
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharacterClass {
    Word,
    Whitespace,
    Punctuation,
}

fn byte_offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut utf16 = 0;
    for (byte, character) in text.char_indices() {
        if utf16 >= offset {
            return byte;
        }
        utf16 += character.len_utf16();
    }
    text.len()
}

fn sanitize(mode: InputMode, text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                output.push(if mode.is_multiline() { '\n' } else { ' ' });
            }
            '\n' => output.push(if mode.is_multiline() { '\n' } else { ' ' }),
            '\t' if mode.is_multiline() => output.push('\t'),
            '\t' => output.push(' '),
            other if other.is_control() => {}
            other => output.push(other),
        }
    }
    output
}
