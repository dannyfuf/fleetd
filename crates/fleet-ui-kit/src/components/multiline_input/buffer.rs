use std::ops::Range;

use gpui::SharedString;
use unicode_segmentation::UnicodeSegmentation;

use super::HISTORY_LIMIT;

/// A completion surface the composer asked for, and what has been typed into it.
///
/// The symbol is reported, never swallowed: `@`, `$` and `/` all stay in the buffer, so a
/// picker that opens over one filters on [`Trigger::query`] and a picker that never opens
/// leaves ordinary prose behind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trigger {
    /// Which surface: `@` files, `$` skills, `/` commands.
    pub symbol: char,
    /// The byte offset of the symbol itself, so a replacement can be guarded against the text
    /// it expects to replace.
    pub at: usize,
    /// Everything typed between the symbol and the caret.
    pub query: SharedString,
}

/// A multi-line editing model: an owned string, a byte cursor and a selection anchor.
///
/// This is [`super::MultilineInput`]'s half that never touches gpui, so the composer's edit set
/// is unit-testable without a window. Every mutation keeps both ends of the selection on a
/// grapheme boundary and inside the string, so no operation can split a user-perceived
/// character.
///
/// Unlike [`crate::TextFieldState`] a newline is *content*: `\r\n` and `\r` are normalised to
/// `\n` on the way in, a tab becomes one space (the design system defines no tab advance), and
/// every other control character is dropped.
///
/// Vertical motion is **logical**: `↑` and `↓` walk `\n`-delimited lines and preserve a goal
/// column. [`super::MultilineInput`] overrides that with visual rows when it has a layout to
/// consult; the logical walk is the fallback and the tested contract.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MultilineBuffer {
    text: String,
    cursor: usize,
    anchor: usize,
    marked: Option<Range<usize>>,
    goal_column: Option<usize>,
}

impl MultilineBuffer {
    /// An empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// A buffer pre-filled with `text`, caret at the end.
    pub fn from_text(text: impl Into<String>) -> Self {
        let mut buffer = Self::new();
        buffer.set_text(text);
        buffer
    }

    /// The current value.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The current value as a `SharedString`, ready to shape.
    pub fn shared_text(&self) -> SharedString {
        SharedString::new(self.text.as_str())
    }

    /// Whether the buffer holds nothing.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Whether the buffer holds nothing a provider could act on.
    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// The caret as a byte offset.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The selection anchor as a byte offset. Equals the cursor when nothing is selected.
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// The selection, low end first.
    pub fn selected_range(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    /// The selected text, empty when the caret is collapsed.
    pub fn selected_text(&self) -> &str {
        &self.text[self.selected_range()]
    }

    /// The IME's marked (composing) range, as byte offsets.
    pub fn marked_range(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// Replace the value; the caret lands at the end.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = sanitize(&text.into());
        self.place(self.text.len(), false);
    }

    /// Empty the buffer.
    pub fn clear(&mut self) {
        self.text.clear();
        self.place(0, false);
    }

    /// Move the caret to a byte offset, snapped down to a grapheme boundary, collapsing the
    /// selection.
    pub fn set_cursor(&mut self, offset: usize) {
        self.place(offset, false);
    }

    /// Move the caret to a byte offset, extending the selection when `select`.
    pub fn move_to(&mut self, offset: usize, select: bool) {
        self.place(offset, select);
    }

    /// Set the platform selection, snapping both ends to grapheme boundaries.
    ///
    /// The marked range survives, because the IME sets the selection *inside* the text it is
    /// still composing.
    pub fn set_selected_range(&mut self, range: Range<usize>) {
        self.anchor = self.floor_boundary(range.start);
        self.cursor = self.floor_boundary(range.end);
        self.goal_column = None;
    }

    /// Select the whole buffer, caret at the end.
    pub fn select_all(&mut self) -> bool {
        if self.text.is_empty() {
            return false;
        }
        self.set_selected_range(0..self.text.len());
        true
    }

    /// Insert `text` at the caret, replacing the selection. Newlines are kept.
    pub fn insert(&mut self, text: &str) {
        let range = self.selected_range();
        self.replace_range(range, text);
    }

    /// `⇧⏎`: break the line at the caret.
    pub fn insert_newline(&mut self) {
        self.insert("\n");
    }

    /// Delete the grapheme before the caret, or the selection. Reports whether it changed.
    pub fn backspace(&mut self) -> bool {
        if !self.selected_range().is_empty() {
            self.insert("");
            return true;
        }
        let start = self.previous_boundary(self.cursor);
        self.cut(start..self.cursor)
    }

    /// Delete the grapheme after the caret, or the selection. Reports whether it changed.
    pub fn delete_forward(&mut self) -> bool {
        if !self.selected_range().is_empty() {
            self.insert("");
            return true;
        }
        let end = self.next_boundary(self.cursor);
        self.cut(self.cursor..end)
    }

    /// `⌥⌫` / `^w`: delete the word before the caret, or join with the line above.
    pub fn delete_word_before(&mut self) -> bool {
        if !self.selected_range().is_empty() {
            self.insert("");
            return true;
        }
        let start = self.word_start_before(self.cursor);
        if start == self.cursor {
            return self.backspace();
        }
        self.cut(start..self.cursor)
    }

    /// `⌥⌦`: delete the word after the caret, or join with the line below.
    pub fn delete_word_after(&mut self) -> bool {
        if !self.selected_range().is_empty() {
            self.insert("");
            return true;
        }
        let end = self.word_end_after(self.cursor);
        if end == self.cursor {
            return self.delete_forward();
        }
        self.cut(self.cursor..end)
    }

    /// `^u`: delete from the start of the current line to the caret.
    pub fn delete_to_line_start(&mut self) -> bool {
        let start = self.line_start(self.cursor);
        self.cut(start..self.cursor)
    }

    /// `^k`: delete from the caret to the end of the current line.
    pub fn delete_to_line_end(&mut self) -> bool {
        let end = self.line_end(self.cursor);
        self.cut(self.cursor..end)
    }

    /// `←`: one grapheme left, or to the low end of the selection.
    pub fn move_left(&mut self, select: bool) -> bool {
        if !select && !self.selected_range().is_empty() {
            self.place(self.selected_range().start, false);
            return true;
        }
        let target = self.previous_boundary(self.cursor);
        self.step(target, select)
    }

    /// `→`: one grapheme right, or to the high end of the selection.
    pub fn move_right(&mut self, select: bool) -> bool {
        if !select && !self.selected_range().is_empty() {
            self.place(self.selected_range().end, false);
            return true;
        }
        let target = self.next_boundary(self.cursor);
        self.step(target, select)
    }

    /// `⌥←`: to the start of the word before the caret.
    pub fn move_word_left(&mut self, select: bool) -> bool {
        let target = self.word_start_before(self.cursor);
        if target == self.cursor {
            return self.move_left(select);
        }
        self.step(target, select)
    }

    /// `⌥→`: to the end of the word after the caret.
    pub fn move_word_right(&mut self, select: bool) -> bool {
        let target = self.word_end_after(self.cursor);
        if target == self.cursor {
            return self.move_right(select);
        }
        self.step(target, select)
    }

    /// `↑`: the same column one logical line up.
    pub fn move_up(&mut self, select: bool) -> bool {
        self.move_vertical(false, select)
    }

    /// `↓`: the same column one logical line down.
    pub fn move_down(&mut self, select: bool) -> bool {
        self.move_vertical(true, select)
    }

    /// `home` / `^a`: to the start of the current line.
    pub fn move_to_line_start(&mut self, select: bool) -> bool {
        let target = self.line_start(self.cursor);
        self.step(target, select)
    }

    /// `end` / `^e`: to the end of the current line.
    pub fn move_to_line_end(&mut self, select: bool) -> bool {
        let target = self.line_end(self.cursor);
        self.step(target, select)
    }

    /// `⌘↑`: to the start of the buffer.
    pub fn move_to_start(&mut self, select: bool) -> bool {
        self.step(0, select)
    }

    /// `⌘↓`: to the end of the buffer.
    pub fn move_to_end(&mut self, select: bool) -> bool {
        self.step(self.text.len(), select)
    }

    /// Whether the caret sits on the first logical line — the condition §9's `↑` history walk
    /// needs.
    pub fn on_first_line(&self) -> bool {
        !self.text[..self.cursor].contains('\n')
    }

    /// Whether the caret sits on the last logical line.
    pub fn on_last_line(&self) -> bool {
        !self.text[self.cursor..].contains('\n')
    }

    /// The byte offset of the start of the line holding `offset`.
    pub fn line_start(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.text[..offset].rfind('\n').map_or(0, |index| index + 1)
    }

    /// The byte offset of the end of the line holding `offset`, before its `\n`.
    pub fn line_end(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.text[offset..]
            .find('\n')
            .map_or(self.text.len(), |index| offset + index)
    }

    /// Whether inserting `text` at `offset` opens a completion surface, and which one.
    ///
    /// The composer **reports** a trigger rather than consuming the key (`spec-B` §B5.8), so
    /// all three characters stay typable and the picker filters on what follows. Where each
    /// one fires is not symmetric:
    ///
    /// | Trigger | Where | Query |
    /// | --- | --- | --- |
    /// | `/` | **line start only** | commands |
    /// | `$` | anywhere a token starts | skills |
    /// | `@` | anywhere a token starts | files and folders |
    ///
    /// `/` is at line start only because *a harness expands a slash command only when it opens
    /// the whole message*; anywhere else it reaches the agent as literal text, and offering it
    /// there is a whole class of "why didn't my command run?" bugs. `feature/branch` and
    /// `me@host` therefore do not fire either.
    pub fn trigger_for(&self, offset: usize, text: &str) -> Option<Trigger> {
        let mut chars = text.chars();
        let (Some(symbol), None) = (chars.next(), chars.next()) else {
            return None;
        };
        if !matches!(symbol, '@' | '/' | '$') {
            return None;
        }
        let at = self.floor_boundary(offset);
        let token_start = self.text[..at]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        let allowed = if symbol == '/' {
            self.line_start(at) == at
        } else {
            token_start
        };
        allowed.then(|| Trigger {
            symbol,
            at,
            query: SharedString::default(),
        })
    }

    /// The trigger the caret is currently inside, with everything typed after it.
    ///
    /// This is what lets a picker filter as the user keeps typing: the owner reads it on every
    /// [`super::MultilineInputEvent::Changed`] rather than tracking the keystrokes itself. It
    /// answers `None` as soon as the token gains whitespace or the caret leaves it.
    pub fn active_trigger(&self) -> Option<Trigger> {
        let cursor = self.cursor;
        let line_start = self.line_start(cursor);
        let token_start = self.text[line_start..cursor]
            .rfind(char::is_whitespace)
            .map_or(line_start, |index| line_start + index + 1);
        let token = &self.text[token_start..cursor];
        let symbol = token.chars().next()?;
        if !matches!(symbol, '@' | '/' | '$') {
            return None;
        }
        if symbol == '/' && token_start != line_start {
            return None;
        }
        Some(Trigger {
            symbol,
            at: token_start,
            query: SharedString::from(token[symbol.len_utf8()..].to_owned()),
        })
    }

    /// Replace a byte range with `text` and leave the caret after the insertion.
    ///
    /// The range is snapped to grapheme boundaries, so an IME range that is stale by a frame
    /// cannot panic the app.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str) {
        let start = self.floor_boundary(range.start);
        let end = self.floor_boundary(range.end.max(start));
        let insertion = sanitize(text);
        self.text.replace_range(start..end, &insertion);
        self.place(start + insertion.len(), false);
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
        self.anchor = self.cursor;
    }

    /// Convert a byte offset to a UTF-16 offset, for the platform input handler.
    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.text[..offset].chars().map(char::len_utf16).sum()
    }

    /// Convert a UTF-16 offset to a byte offset, for the platform input handler.
    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        offset_from_utf16(&self.text, offset)
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

    /// Park the caret, optionally keeping the anchor to extend a selection.
    fn place(&mut self, offset: usize, select: bool) {
        self.cursor = self.floor_boundary(offset);
        if !select {
            self.anchor = self.cursor;
        }
        self.marked = None;
        self.goal_column = None;
    }

    /// [`Self::place`] that reports whether the caret actually moved.
    fn step(&mut self, offset: usize, select: bool) -> bool {
        let had_selection = !select && !self.selected_range().is_empty();
        let moved = offset != self.cursor || had_selection;
        self.place(offset, select);
        moved
    }

    /// Delete a byte range, reporting whether it held anything.
    fn cut(&mut self, range: Range<usize>) -> bool {
        if range.is_empty() {
            return false;
        }
        self.text.replace_range(range.clone(), "");
        self.place(range.start, false);
        true
    }

    /// The shared half of [`Self::move_up`] and [`Self::move_down`].
    fn move_vertical(&mut self, down: bool, select: bool) -> bool {
        let start = self.line_start(self.cursor);
        let column = self
            .goal_column
            .unwrap_or_else(|| self.text[start..self.cursor].chars().count());
        let target_start = if down {
            let end = self.line_end(self.cursor);
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
            .char_indices()
            .nth(column)
            .map_or(line.len(), |(index, _)| index);
        self.place(target_start + local, select);
        self.goal_column = Some(column);
        true
    }

    /// The start of the word before `offset`, never crossing the line start.
    fn word_start_before(&self, offset: usize) -> usize {
        let floor = self.line_start(offset);
        let bytes = self.text.as_bytes();
        let mut start = self.floor_boundary(offset);
        while start > floor {
            let previous = self.previous_boundary(start);
            if bytes[previous].is_ascii_whitespace() {
                start = previous;
            } else {
                break;
            }
        }
        while start > floor {
            let previous = self.previous_boundary(start);
            if bytes[previous].is_ascii_whitespace() {
                break;
            }
            start = previous;
        }
        start
    }

    /// The end of the word after `offset`, never crossing the line end.
    fn word_end_after(&self, offset: usize) -> usize {
        let ceiling = self.line_end(offset);
        let bytes = self.text.as_bytes();
        let mut end = self.floor_boundary(offset);
        while end < ceiling && bytes[end].is_ascii_whitespace() {
            end = self.next_boundary(end);
        }
        while end < ceiling && !bytes[end].is_ascii_whitespace() {
            end = self.next_boundary(end);
        }
        end
    }

    /// The nearest grapheme boundary at or before `offset`, clamped to the string.
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

    /// The grapheme boundary before `offset`.
    fn previous_boundary(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .take_while(|index| *index < offset)
            .last()
            .unwrap_or(0)
    }

    /// The grapheme boundary after `offset`.
    fn next_boundary(&self, offset: usize) -> usize {
        let offset = self.floor_boundary(offset);
        self.text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .find(|index| *index > offset)
            .unwrap_or(self.text.len())
    }
}

/// The submitted prompts `↑` and `↓` walk, plus the draft they were opened from.
///
/// Navigation is a cursor into [`Self::entries`]: `None` means "editing the live draft", and
/// walking back off the end of the list restores that draft rather than emptying the composer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptHistory {
    entries: Vec<String>,
    cursor: Option<usize>,
    draft: Option<String>,
}

impl PromptHistory {
    /// An empty history.
    pub fn new() -> Self {
        Self::default()
    }

    /// The recorded prompts, oldest first.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Whether the composer is currently showing a recalled prompt.
    pub fn is_active(&self) -> bool {
        self.cursor.is_some()
    }

    /// Record a submitted prompt and leave navigation at the live draft.
    ///
    /// Blank prompts and an immediate repeat of the newest entry are dropped, and the list is
    /// capped at [`HISTORY_LIMIT`] with the oldest entry falling off.
    pub fn push(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.reset();
        if text.trim().is_empty() || self.entries.last() == Some(&text) {
            return;
        }
        self.entries.push(text);
        if self.entries.len() > HISTORY_LIMIT {
            self.entries.remove(0);
        }
    }

    /// Forget where the walk was; the next `↑` starts from the newest entry again.
    pub fn reset(&mut self) {
        self.cursor = None;
        self.draft = None;
    }

    /// `↑`: the previous prompt, remembering `current` as the draft on the first step.
    pub fn previous(&mut self, current: &str) -> Option<String> {
        let index = match self.cursor {
            None => {
                self.draft = Some(current.to_owned());
                self.entries.len().checked_sub(1)?
            }
            Some(0) => return None,
            Some(index) => index - 1,
        };
        self.cursor = Some(index);
        Some(self.entries[index].clone())
    }

    /// `↓`: the next prompt, or the draft the walk started from.
    ///
    /// Not `next`: the type is public, and a bare `next` on something that is not an
    /// iterator reads as one.
    pub fn newer(&mut self) -> Option<String> {
        let index = self.cursor?;
        if index + 1 < self.entries.len() {
            self.cursor = Some(index + 1);
            return Some(self.entries[index + 1].clone());
        }
        self.cursor = None;
        Some(self.draft.take().unwrap_or_default())
    }
}

/// Convert a UTF-16 offset relative to `text` into a byte offset in that same text.
pub(super) fn offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut utf16 = 0;
    for (byte, ch) in text.char_indices() {
        if utf16 >= offset {
            return byte;
        }
        utf16 += ch.len_utf16();
    }
    text.len()
}

/// Normalise text on the way into the buffer.
///
/// `\r\n` and a lone `\r` become `\n`, a tab becomes one space, and every other control
/// character is dropped: the composer shapes one run per logical line, and a glyph with no
/// defined advance would put the caret off the grid.
fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' => out.push('\n'),
            '\t' => out.push(' '),
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }
    out
}
