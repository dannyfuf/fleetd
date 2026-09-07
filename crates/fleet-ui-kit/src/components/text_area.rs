//! `TextArea` — the multi-line sibling of [`super::text_field`], for the board's card
//! description and its comments.
//!
//! The module has the same three layers as `text_field.rs`, and a caller picks exactly one:
//!
//! 1. [`TextArea`] — the presentational `RenderOnce` surface. The caller owns the string and
//!    the caret byte offset and passes both down every frame.
//! 2. [`TextAreaState`] — the editing model: an owned string, a byte cursor, a preferred
//!    column and a scroll row, plus the KEYMAP §3.8 edit set widened to two dimensions
//!    (`↑` / `↓` keeping the column, `Enter` inserting a newline, `Tab` inserting
//!    [`TAB_WIDTH`] spaces). It is pure — no gpui element, no theme — so it is unit-testable
//!    and reusable by any dialog that renders its own chrome.
//! 3. There is deliberately no live `TextInput`-style entity here yet: the card dialogs own
//!    their focus and route keys through [`TextAreaState::handle_keystroke`]. When one needs
//!    IME composition, it grows the same way `TextField` did — around
//!    [`TextAreaState::handle_edit_keystroke`], never around `handle_keystroke`, or every
//!    printable character is inserted twice.
//!
//! The surface keeps `TextField`'s visual language: the same box, the same border ladder
//! (danger beats focus beats rest) and the same 2 px accent caret bar. What it does not keep
//! is the 18 px status slot — a multi-line body has no derived preview, and a description
//! that fails validation fails on the field that names it.

use gpui::{App, Div, ElementId, Keystroke, ScrollHandle, SharedString, Window, div, prelude::*};

use crate::{
    text::{Text, TextRole, styled_with},
    theme::{ActiveTheme, Theme},
};

/// How many spaces one `Tab` inserts. Fleet never stores a hard tab in a description.
pub const TAB_WIDTH: usize = 2;

/// How many rows a [`TextArea`] shows before it grows, unless [`TextArea::rows`] says otherwise.
pub const TEXT_AREA_ROWS: u32 = 6;

// ---------------------------------------------------------------------------------------
// TextAreaState — the editing model
// ---------------------------------------------------------------------------------------

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

    /// The current value as a `SharedString`, ready for [`TextArea::new`].
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
    /// For a caller that renders the presentational [`TextArea`] and receives raw key events,
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

// ---------------------------------------------------------------------------------------
// TextArea — presentational
// ---------------------------------------------------------------------------------------

/// A multi-line text input. The caller owns the string, the caret and the keys.
#[derive(IntoElement)]
pub struct TextArea {
    value: SharedString,
    cursor: Option<usize>,
    focused: bool,
    placeholder: Option<SharedString>,
    label: Option<SharedString>,
    rows: u32,
    max_rows: Option<u32>,
    scroll_row: usize,
    scroll: Option<(ElementId, ScrollHandle)>,
    mono: bool,
    invalid: bool,
}

impl TextArea {
    /// An area showing `value`.
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            cursor: None,
            focused: false,
            placeholder: None,
            label: None,
            rows: TEXT_AREA_ROWS,
            max_rows: None,
            scroll_row: 0,
            scroll: None,
            mono: false,
            invalid: false,
        }
    }

    /// The caret position as a **byte** offset — [`TextAreaState::cursor`] verbatim. Drawn
    /// only while focused, and snapped to a `char` boundary before use.
    pub fn cursor(mut self, byte_offset: usize) -> Self {
        self.cursor = Some(byte_offset);
        self
    }

    /// Whether the area has keyboard focus.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Text shown when the value is empty.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// A label above the box.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The minimum number of visible rows. [`TEXT_AREA_ROWS`] by default.
    pub fn rows(mut self, rows: u32) -> Self {
        self.rows = rows.max(1);
        self
    }

    /// Cap the box at `rows` rows and clip the rest.
    ///
    /// Additive to the §7 contract: a description editor inside a dialog cannot be allowed to
    /// push the dialog's footer off screen. The caller keeps the caret visible with
    /// [`TextAreaState::reveal_cursor`] **and** [`TextArea::scroll`]: the row window is counted
    /// in logical lines, the cap is measured in pixels, and the body word-wraps, so a capped box
    /// without a scroll handle clips whatever the wrapping pushed past the cap — the caret
    /// included.
    pub fn max_rows(mut self, rows: u32) -> Self {
        self.max_rows = Some(rows.max(1));
        self
    }

    /// Own the box's pixel scrolling, so the caret stays visible when lines wrap.
    ///
    /// [`TextAreaState::scroll_row`] moves a window of whole logical lines, which is what bounds
    /// the work per keystroke; it cannot say how tall those lines became once the box wrapped
    /// them. The handle closes that gap: the element measures the caret's line and scrolls to it,
    /// so a paragraph that occupies six visual rows never hides the row being typed on.
    ///
    /// `id` must be unique among the caller's elements, and the handle must outlive the frame —
    /// keep it in the dialog's draft, next to the [`TextAreaState`].
    pub fn scroll(mut self, id: impl Into<ElementId>, handle: ScrollHandle) -> Self {
        self.scroll = Some((id.into(), handle));
        self
    }

    /// First visible logical line, from `TextAreaState::scroll_row`.
    pub fn scroll_row(mut self, row: usize) -> Self {
        self.scroll_row = row;
        self
    }

    /// Render the value in the data face (descriptions that are really code, commit bodies).
    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    /// Draw the box in the danger color: the value is rejected.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// The type role the value renders in.
    fn role(&self) -> TextRole {
        if self.mono {
            TextRole::Data
        } else {
            TextRole::Ui
        }
    }
}

/// One wrap chunk of a line: a word plus the spaces that follow it, so a flex-wrapped row
/// breaks where a text renderer would.
fn wrap_chunks(line: &str) -> impl Iterator<Item = &str> {
    line.split_inclusive(' ')
}

/// A chunk of a line. `flex_none` + `whitespace_nowrap` is what makes the parent's `flex_wrap`
/// behave like word wrapping instead of shrinking every word.
fn chunk(text: &str) -> Div {
    div()
        .flex_none()
        .whitespace_nowrap()
        .child(SharedString::from(text.to_string()))
}

/// Scrolls `handle` the minimum amount that puts the caret inside the visible box.
///
/// A logical line that fits the viewport is handed to gpui, which measures the row it actually
/// laid out — the only thing that knows how many visual rows the wrapping made of it. A line
/// taller than the whole box has no row gpui can reveal, so the caret's position inside it is
/// estimated from how far along the line the caret sits, which is exact for the uniformly
/// wrapped paragraph that produces this case.
fn reveal_caret(handle: &ScrollHandle, row: usize, progress: f32, line_height: gpui::Pixels) {
    let container = handle.bounds();
    let Some(child) = handle.bounds_for_item(row) else {
        handle.scroll_to_item(row);
        return;
    };
    if child.size.height <= container.size.height {
        handle.scroll_to_item(row);
        return;
    }
    let mut offset = handle.offset();
    let top = child.top() + child.size.height * progress;
    let bottom = top + line_height;
    if top + offset.y < container.top() {
        offset.y = container.top() - top;
    } else if bottom + offset.y > container.bottom() {
        offset.y = container.bottom() - bottom;
    }
    handle.set_offset(offset);
}

/// The 2 px accent bar that marks the insertion point — the same caret [`super::TextField`]
/// draws, at the line height of the role.
fn caret_bar(theme: &Theme, role: TextRole) -> Div {
    div()
        .flex_none()
        .w(theme.metrics.focus_ring_w)
        .h(role.style(theme).line_height)
        .bg(theme.colors.accent)
}

/// One rendered line: its chunks, with the caret spliced in when it lands on this line.
fn line_row(theme: &Theme, role: TextRole, line: &str, caret: Option<usize>) -> Div {
    let line_height = role.style(theme).line_height;
    let row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .w_full()
        .min_h(line_height);
    match caret {
        Some(offset) => {
            let (head, tail) = line.split_at(offset);
            row.children(wrap_chunks(head).map(chunk))
                .child(caret_bar(theme, role))
                .children(wrap_chunks(tail).map(chunk))
        }
        None => row.children(wrap_chunks(line).map(chunk)),
    }
}

impl RenderOnce for TextArea {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let role = self.role();
        let line_height = role.style(&theme).line_height;
        let padding = theme.space.sm;
        // Danger beats focus, focus beats rest — the ladder `TextField` uses.
        let border = if self.invalid {
            theme.colors.danger
        } else if self.focused {
            theme.colors.focus_ring
        } else {
            theme.colors.border
        };

        let value = self.value.clone();
        let empty = value.is_empty();
        // The caret is a byte offset into the whole value; find its line, its offset inside that
        // line and how far along the line it sits, once, instead of per rendered row.
        let caret = self.focused.then(|| {
            let mut offset = self.cursor.unwrap_or(value.len()).min(value.len());
            while offset > 0 && !value.is_char_boundary(offset) {
                offset -= 1;
            }
            let line_start = value[..offset].rfind('\n').map_or(0, |index| index + 1);
            let line_end = value[line_start..]
                .find('\n')
                .map_or(value.len(), |index| line_start + index);
            let progress = if line_end > line_start {
                (offset - line_start) as f32 / (line_end - line_start) as f32
            } else {
                0.0
            };
            (
                value[..line_start].matches('\n').count(),
                offset - line_start,
                progress,
            )
        });

        let content: Vec<Div> = if empty {
            // The placeholder never carries a caret inside it: the caret sits before it.
            vec![
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .min_h(line_height)
                    .children(self.focused.then(|| caret_bar(&theme, role)))
                    .children(
                        self.placeholder
                            .map(|placeholder| Text::new(role, placeholder).faint()),
                    ),
            ]
        } else {
            // `max_rows` caps the box; without the take, a pasted thousand-line description
            // still builds a thousand rows, and a word div per word in each, on every keystroke.
            value
                .split('\n')
                .enumerate()
                .skip(self.scroll_row)
                .take(self.max_rows.map_or(usize::MAX, |rows| rows as usize))
                .map(|(index, line)| {
                    let caret = caret.and_then(|(line_index, offset, _)| {
                        (line_index == index).then_some(offset)
                    });
                    line_row(&theme, role, line, caret)
                })
                .collect()
        };

        // The rows are the scroll container's own children, so gpui measures each one and can
        // reveal the caret's line whatever the wrapping did to it.
        let base = styled_with(div(), role.style(&theme), &theme)
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .min_h(line_height * (self.rows as f32))
            .text_color(theme.colors.text);
        let body = match (self.max_rows, self.scroll) {
            (Some(rows), Some((id, handle))) => {
                if let Some((line, _, progress)) = caret {
                    reveal_caret(
                        &handle,
                        line.saturating_sub(self.scroll_row),
                        progress,
                        line_height,
                    );
                }
                base.id(id)
                    .max_h(line_height * (rows as f32))
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .children(content)
                    .into_any_element()
            }
            (Some(rows), None) => base
                .max_h(line_height * (rows as f32))
                .overflow_hidden()
                .children(content)
                .into_any_element(),
            (None, _) => base.children(content).into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(theme.space.xxs)
            .children(self.label.map(Text::label))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .px(theme.space.md)
                    .py(padding)
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.bg)
                    .border_1()
                    .border_color(border)
                    .overflow_hidden()
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(text: &str, cursor: usize) -> TextAreaState {
        let mut state = TextAreaState::from_text(text);
        state.set_cursor(cursor);
        state
    }

    fn key(key: &str) -> Keystroke {
        Keystroke {
            modifiers: Default::default(),
            key: key.into(),
            key_char: None,
        }
    }

    fn ctrl(key: &str) -> Keystroke {
        Keystroke {
            modifiers: gpui::Modifiers {
                control: true,
                ..Default::default()
            },
            key: key.into(),
            key_char: None,
        }
    }

    #[test]
    fn from_text_parks_the_caret_at_the_end() {
        let state = TextAreaState::from_text("one\ntwo");
        assert_eq!(state.cursor(), 7);
        assert_eq!(state.line_col(), (1, 3));
        assert_eq!(state.line_count(), 2);
    }

    #[test]
    fn insert_normalizes_line_breaks_and_tabs() {
        let mut state = TextAreaState::new();
        state.insert("a\r\nb\rc\td");
        assert_eq!(state.text(), "a\nb\nc  d");
        assert_eq!(state.cursor(), state.text().len());
    }

    #[test]
    fn enter_inserts_a_newline_and_tab_inserts_two_spaces() {
        let mut state = TextAreaState::new();
        state.insert("fix");
        assert!(state.handle_keystroke(&key("enter")));
        assert!(state.handle_keystroke(&key("tab")));
        assert_eq!(state.text(), "fix\n  ");
        assert_eq!(state.line_col(), (1, 2));
    }

    #[test]
    fn line_col_counts_characters_not_bytes() {
        let state = state("héllo\nwörld", 10);
        // Line 1 starts at byte 7 and "wö" spans three bytes: that is column 2, not 3.
        assert_eq!(state.line_col(), (1, 2));
        // An offset inside the ö snaps back to the character that owns it.
        assert_eq!(super::TextAreaState::from_text("wö").line_col(), (0, 2));
    }

    #[test]
    fn cursor_snaps_to_a_char_boundary() {
        let mut state = TextAreaState::from_text("é\n😀");
        state.set_cursor(1);
        assert_eq!(state.cursor(), 0);
        state.set_cursor(4);
        assert_eq!(state.cursor(), 3);
        state.set_cursor(999);
        assert_eq!(state.cursor(), state.text().len());
    }

    #[test]
    fn arrows_cross_line_breaks_by_characters() {
        let mut state = state("añ\nb", 0);
        assert!(state.move_right());
        assert!(state.move_right());
        assert_eq!(state.cursor(), 3); // past the two-byte ñ
        assert!(state.move_right()); // over the newline
        assert_eq!(state.line_col(), (1, 0));
        assert!(state.move_left());
        assert_eq!(state.line_col(), (0, 2));
        assert!(state.move_to_start());
        assert!(!state.move_left());
    }

    #[test]
    fn up_and_down_preserve_the_preferred_column() {
        let mut state = state("abcdef\nxy\nlmnopq", 5);
        assert_eq!(state.line_col(), (0, 5));
        assert!(state.move_down());
        assert_eq!(state.line_col(), (1, 2)); // clamped to the short line
        assert!(state.move_down());
        assert_eq!(state.line_col(), (2, 5)); // …and back to column 5
        assert!(state.move_up());
        assert_eq!(state.line_col(), (1, 2));
    }

    #[test]
    fn an_edit_forgets_the_preferred_column() {
        let mut state = state("abcdef\nxy\nlmnopq", 5);
        assert!(state.move_down());
        state.insert("!");
        assert!(state.move_down());
        assert_eq!(state.line_col(), (2, 3));
    }

    #[test]
    fn up_at_the_first_line_and_down_at_the_last_do_nothing() {
        let mut state = state("one\ntwo", 1);
        assert!(!state.move_up());
        assert_eq!(state.line_col(), (0, 1));
        state.move_to_end();
        assert!(!state.move_down());
    }

    #[test]
    fn up_and_down_land_on_char_boundaries_in_multibyte_lines() {
        let mut state = state("ñññññ\nabcde", 10);
        assert_eq!(state.line_col(), (0, 5));
        assert!(state.move_down());
        assert_eq!(state.cursor(), 16);
        assert!(state.move_up());
        assert_eq!(state.cursor(), 10);
    }

    #[test]
    fn backspace_joins_lines_and_walks_char_boundaries() {
        let mut state = state("añ\nb", 3);
        assert!(state.backspace()); // deletes the whole ñ
        assert_eq!(state.text(), "a\nb");
        state.set_cursor(2);
        assert!(state.backspace()); // deletes the line break
        assert_eq!(state.text(), "ab");
        state.set_cursor(0);
        assert!(!state.backspace());
    }

    #[test]
    fn delete_forward_removes_the_char_after_the_caret() {
        let mut state = state("a😀b", 1);
        assert!(state.delete_forward());
        assert_eq!(state.text(), "ab");
        state.move_to_end();
        assert!(!state.delete_forward());
    }

    #[test]
    fn word_ops_cut_around_the_caret() {
        let mut state = TextAreaState::from_text("origin/main feature ");
        assert!(state.delete_word_before());
        assert_eq!(state.text(), "origin/main ");
        state.set_cursor(0);
        assert!(state.delete_word_after());
        assert_eq!(state.text(), " ");
        assert!(state.delete_word_after());
        assert_eq!(state.text(), "");
        assert!(!state.delete_word_before());
        assert!(!state.delete_word_after());
    }

    #[test]
    fn ctrl_u_and_ctrl_k_are_line_scoped() {
        let mut state = state("one\ntwo three\nfour", 7);
        assert!(state.delete_to_line_end());
        assert_eq!(state.text(), "one\ntwo\nfour");
        assert!(state.delete_to_line_start());
        assert_eq!(state.text(), "one\n\nfour");
        // At the end of a line `ctrl-k` eats the break itself.
        assert!(state.delete_to_line_end());
        assert_eq!(state.text(), "one\nfour");
    }

    #[test]
    fn home_and_end_stay_on_the_current_line() {
        let mut state = state("one\ntwo", 5);
        assert!(state.handle_keystroke(&key("home")));
        assert_eq!(state.cursor(), 4);
        // The keystroke is still consumed once the caret is parked; only the motion reports
        // that it had nowhere to go.
        assert!(state.handle_keystroke(&key("home")));
        assert!(!state.move_to_line_start());
        assert!(state.handle_keystroke(&ctrl("e")));
        assert_eq!(state.cursor(), 7);
        assert!(state.move_to_start());
        assert_eq!(state.cursor(), 0);
    }

    #[test]
    fn printable_keystrokes_insert_and_control_ones_do_not() {
        let mut state = TextAreaState::new();
        let printable = Keystroke {
            modifiers: Default::default(),
            key: "ñ".into(),
            key_char: Some("ñ".into()),
        };
        assert!(!state.handle_edit_keystroke(&printable));
        assert!(state.handle_keystroke(&printable));
        assert_eq!(state.text(), "ñ");
        assert!(!state.handle_keystroke(&ctrl("s")));
        assert_eq!(state.text(), "ñ");
    }

    #[test]
    fn reveal_cursor_scrolls_the_minimum_amount() {
        let mut state = TextAreaState::from_text("0\n1\n2\n3\n4\n5\n6\n7");
        assert!(state.reveal_cursor(3)); // caret on line 7
        assert_eq!(state.scroll_row(), 5);
        state.set_cursor(0);
        assert!(state.reveal_cursor(3));
        assert_eq!(state.scroll_row(), 0);
        assert!(!state.reveal_cursor(3));
    }

    #[test]
    fn set_text_and_clear_reset_the_view() {
        let mut state = TextAreaState::from_text("a\nb\nc");
        state.set_scroll_row(2);
        state.set_text("x");
        assert_eq!(state.text(), "x");
        assert_eq!(state.cursor(), 1);
        assert_eq!(state.scroll_row(), 0);
        state.clear();
        assert!(state.is_empty());
        assert_eq!(state.cursor(), 0);
    }

    #[test]
    fn lines_keeps_the_empty_line_after_a_trailing_break() {
        let state = TextAreaState::from_text("a\n");
        assert_eq!(state.lines().collect::<Vec<_>>(), vec!["a", ""]);
        assert_eq!(state.line_count(), 2);
    }

    #[test]
    fn wrap_chunks_keep_their_trailing_space() {
        assert_eq!(
            wrap_chunks("two words here").collect::<Vec<_>>(),
            vec!["two ", "words ", "here"]
        );
        assert_eq!(wrap_chunks("").next(), None);
    }
}
