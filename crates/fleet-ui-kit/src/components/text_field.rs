//! `TextField` — a single-line input with a blue caret and a zero-shift validation line.
//!
//! The module has three layers, and a caller picks exactly one:
//!
//! 1. [`TextField`] — the presentational `RenderOnce` surface. The caller owns the string and
//!    the caret index and passes both down every frame. This is what dialogs that already own
//!    their state (Create, Clone, Context) compose.
//! 2. [`TextFieldState`] — the editing model: an owned string, a byte cursor and the KEYMAP
//!    §3.8 edit set (printable, `Backspace`, `Delete`, `ctrl-w`, `ctrl-u`, `ctrl-k`, `ctrl-a`,
//!    `ctrl-e`, `←` / `→`, `Home` / `End`). It is pure — no gpui element, no theme — so it is
//!    unit-testable and reusable by [`super::FilterBar`] and [`super::Palette`] callers, which
//!    render their own chrome.
//! 3. [`TextInput`] — a gpui entity that owns a [`TextFieldState`], paints a real shaped line
//!    with a caret through a custom element, and installs an [`gpui::ElementInputHandler`] so
//!    dead keys and IME composition (marked text is underlined) work like a native field.
//!
//! The one rule the surface *enforces* is §3.8.1's: the validation line **replaces** the
//! preview line in the same 18 px slot, so a failing branch name causes zero layout shift.

use std::ops::Range;

use gpui::{
    App, Bounds, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, Hsla,
    InspectorElementId, KeyDownEvent, Keystroke, LayoutId, MouseButton, MouseDownEvent, PaintQuad,
    Pixels, Point, ShapedLine, SharedString, Style, TextAlign, TextRun, UTF16Selection,
    UnderlineStyle, Window, div, fill, point, prelude::*, px, relative, size,
};

use crate::{
    icons::{Icon, IconSize},
    text::{Text, TextRole, styled_with},
    theme::{ActiveTheme, Theme},
    tone::Tone,
};

// ---------------------------------------------------------------------------------------
// Shared chrome
// ---------------------------------------------------------------------------------------

/// Everything both the presentational [`TextField`] and the live [`TextInput`] draw around the
/// value: the label, the box, the leading glyph and the 18 px status slot.
struct FieldChrome {
    label: Option<SharedString>,
    icon: Option<Icon>,
    preview: Option<SharedString>,
    invalid: Option<SharedString>,
    focused: bool,
    mono: bool,
    height: Option<Pixels>,
    hide_status_line: bool,
}

impl FieldChrome {
    /// The type role the value renders in.
    fn role(&self) -> TextRole {
        if self.mono {
            TextRole::Data
        } else {
            TextRole::Ui
        }
    }

    /// The box border: danger beats focus, focus beats rest.
    fn border(&self, theme: &Theme) -> Hsla {
        if self.invalid.is_some() {
            theme.colors.danger
        } else if self.focused {
            theme.colors.focus_ring
        } else {
            theme.colors.border
        }
    }

    /// Wrap `value` — the caret-bearing content — in the label / box / status-line frame.
    fn wrap(self, theme: &Theme, value: impl IntoElement) -> gpui::Div {
        let role = self.role();
        let box_h = self.height.unwrap_or(theme.metrics.text_field_h);
        let border = self.border(theme);
        let icon_color = if self.focused {
            theme.colors.text_secondary
        } else {
            theme.colors.text_muted
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
                    .items_center()
                    .gap(theme.space.sm)
                    .h(box_h)
                    .w_full()
                    .px(theme.space.md)
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.bg)
                    .border_1()
                    .border_color(border)
                    .overflow_hidden()
                    .children(
                        self.icon
                            .map(|icon| icon.el().size(IconSize::Medium).color(icon_color)),
                    )
                    .child(
                        // The value area carries the role's font so a shaped caret lines up
                        // with the glyphs around it.
                        styled_with(div(), role.style(theme), theme)
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .overflow_hidden()
                            .text_color(theme.colors.text)
                            .child(value),
                    ),
            )
            .when(!self.hide_status_line, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(theme.metrics.field_status_h)
                        .w_full()
                        .overflow_hidden()
                        .child(match self.invalid {
                            Some(message) => Text::hint(message).tone(Tone::Danger).ellipsize(),
                            None => Text::hint(self.preview.unwrap_or_default())
                                .faint()
                                .ellipsize(),
                        }),
                )
            })
    }
}

/// Split `text` at a character index, saturating at the end.
fn split_at_char(text: &str, chars: usize) -> (&str, &str) {
    let byte = text
        .char_indices()
        .nth(chars)
        .map_or(text.len(), |(index, _)| index);
    text.split_at(byte)
}

// ---------------------------------------------------------------------------------------
// TextField — presentational
// ---------------------------------------------------------------------------------------

/// A single-line text input. The caller owns the string, the caret and the keys.
#[derive(IntoElement)]
pub struct TextField {
    value: SharedString,
    placeholder: Option<SharedString>,
    caret: Option<usize>,
    focused: bool,
    icon: Option<Icon>,
    preview: Option<SharedString>,
    invalid: Option<SharedString>,
    mono: bool,
    label: Option<SharedString>,
    height: Option<Pixels>,
    hide_status_line: bool,
}

impl TextField {
    /// A field showing `value`.
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            placeholder: None,
            caret: None,
            focused: false,
            icon: None,
            preview: None,
            invalid: None,
            mono: false,
            label: None,
            height: None,
            hide_status_line: false,
        }
    }

    /// A label above the box.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Text shown when the value is empty.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Caret position in characters. Drawn only while focused.
    pub fn caret(mut self, caret: usize) -> Self {
        self.caret = Some(caret);
        self
    }

    /// Whether the field has keyboard focus.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// A leading glyph, e.g. `search` in the Clone dialog.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The faint derived preview under the box (`→ buk/payroll#feat-rut-validator`).
    pub fn preview(mut self, preview: impl Into<SharedString>) -> Self {
        self.preview = Some(preview.into());
        self
    }

    /// The exact failing rule. **Replaces** the preview in the same slot.
    pub fn invalid(mut self, message: impl Into<SharedString>) -> Self {
        self.invalid = Some(message.into());
        self
    }

    /// Render the value in the data face (branches, paths).
    pub fn mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    /// Override the 36 px box height. The palette's 44 px input is the only caller.
    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    /// Drop the 18 px preview / validation slot.
    ///
    /// Only for a surface that can carry **neither** a preview nor a validation message — the
    /// palette's query line. Everywhere else the slot must stay, because that is what makes a
    /// preview turning into an error cost zero layout shift.
    pub fn hide_status_line(mut self, hide: bool) -> Self {
        self.hide_status_line = hide;
        self
    }

    /// Whether the field is currently rejecting its value.
    pub fn is_invalid(&self) -> bool {
        self.invalid.is_some()
    }

    /// The chrome this field draws around its value.
    fn chrome(&self) -> FieldChrome {
        FieldChrome {
            label: self.label.clone(),
            icon: self.icon,
            preview: self.preview.clone(),
            invalid: self.invalid.clone(),
            focused: self.focused,
            mono: self.mono,
            height: self.height,
            hide_status_line: self.hide_status_line,
        }
    }
}

/// The 2 px accent bar that marks the insertion point.
fn caret_bar(theme: &Theme, role: TextRole) -> gpui::Div {
    div()
        .flex_none()
        .w(theme.metrics.focus_ring_w)
        .h(role.style(theme).line_height)
        .bg(theme.colors.accent)
}

impl RenderOnce for TextField {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let role = self.chrome().role();
        let focused = self.focused;
        let empty = self.value.is_empty();
        let chrome = self.chrome();

        let value = if empty {
            // The placeholder never carries a caret inside it: the caret sits before it.
            div()
                .flex()
                .items_center()
                .min_w_0()
                .children(focused.then(|| caret_bar(&theme, role)))
                .child(
                    Text::new(role, self.placeholder.unwrap_or_default())
                        .faint()
                        .ellipsize(),
                )
        } else {
            let caret = self.caret.unwrap_or_else(|| self.value.chars().count());
            let (head, tail) = split_at_char(self.value.as_ref(), caret);
            div()
                .flex()
                .items_center()
                .min_w_0()
                .child(
                    Text::new(role, head.to_string())
                        .tone(Tone::Default)
                        .ellipsize(),
                )
                .children(focused.then(|| caret_bar(&theme, role)))
                .child(
                    Text::new(role, tail.to_string())
                        .tone(Tone::Default)
                        .ellipsize(),
                )
        };

        chrome.wrap(&theme, value)
    }
}

// ---------------------------------------------------------------------------------------
// TextFieldState — the editing model
// ---------------------------------------------------------------------------------------

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
        SharedString::from(self.text.clone())
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
        let insertion_len = sanitize(text).len();
        self.marked = (insertion_len > 0).then_some(start..start + insertion_len);
    }

    /// Drop the IME marked range without touching the text.
    pub fn unmark(&mut self) {
        self.marked = None;
    }

    /// Handle one non-printable edit keystroke.
    ///
    /// This is the half a [`TextInput`] binds: printable characters arrive through the platform
    /// input handler (so dead keys and IME composition keep working), and only the control keys
    /// are consumed here. Returns whether the keystroke was consumed.
    pub fn handle_edit_keystroke(&mut self, keystroke: &Keystroke) -> bool {
        let modifiers = &keystroke.modifiers;
        let control = modifiers.control && !modifiers.platform && !modifiers.alt;
        match keystroke.key.as_str() {
            "backspace" if !modifiers.control => {
                self.backspace();
            }
            "delete" if !modifiers.control => {
                self.delete_forward();
            }
            "left" if !modifiers.control => {
                self.move_left();
            }
            "right" if !modifiers.control => {
                self.move_right();
            }
            "home" => {
                self.move_to_start();
            }
            "end" => {
                self.move_to_end();
            }
            "a" if control => {
                self.move_to_start();
            }
            "e" if control => {
                self.move_to_end();
            }
            "b" if control => {
                self.move_left();
            }
            "f" if control => {
                self.move_right();
            }
            "w" if control => {
                self.delete_word_before();
            }
            "u" if control => {
                self.delete_to_start();
            }
            "k" if control => {
                self.delete_to_end();
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
    /// For callers that render the presentational [`TextField`] and receive raw key events —
    /// the Hub's filter bar and the palette, which own their own query. A field that installs
    /// the platform input handler must use [`TextFieldState::handle_edit_keystroke`] instead,
    /// or every character is inserted twice.
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

// ---------------------------------------------------------------------------------------
// TextInput — the live, IME-safe entity
// ---------------------------------------------------------------------------------------

/// What a [`TextInput`] tells its owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextInputEvent {
    /// The value changed. Re-derive the preview, the validation and any filtered list.
    Changed,
    /// The caret moved but the value did not.
    CaretMoved,
}

/// A live single-line editor: [`TextFieldState`] plus focus, painting and IME.
///
/// Unlike [`TextField`] this owns its string, because a real editor needs the platform input
/// handler and that requires an `Entity`. Compose it when the view wants a working field with
/// no key plumbing of its own; compose [`TextField`] when the view already owns the string.
///
/// ```ignore
/// let input = cx.new(|cx| TextInput::new(cx).with_placeholder("feat/rut-validator").with_mono(true));
/// cx.subscribe(&input, |_this, input, _event: &TextInputEvent, cx| {
///     let value = input.read(cx).text().to_string();
///     // re-derive the preview / validation here
/// })
/// .detach();
/// ```
pub struct TextInput {
    focus_handle: FocusHandle,
    state: TextFieldState,
    label: Option<SharedString>,
    placeholder: Option<SharedString>,
    icon: Option<Icon>,
    preview: Option<SharedString>,
    invalid: Option<SharedString>,
    mono: bool,
    height: Option<Pixels>,
    hide_status_line: bool,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    last_scroll: Pixels,
}

impl TextInput {
    /// An empty field.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            state: TextFieldState::new(),
            label: None,
            placeholder: None,
            icon: None,
            preview: None,
            invalid: None,
            mono: false,
            height: None,
            hide_status_line: false,
            last_layout: None,
            last_bounds: None,
            last_scroll: px(0.0),
        }
    }

    /// Pre-fill the value.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.state.set_text(text);
        self
    }

    /// Set the label above the box.
    pub fn with_label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the empty-value placeholder.
    pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Set the leading glyph.
    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Render the value in the data face.
    pub fn with_mono(mut self, mono: bool) -> Self {
        self.mono = mono;
        self
    }

    /// Override the box height.
    pub fn with_height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    /// Drop the 18 px status slot. See [`TextField::hide_status_line`].
    pub fn with_hidden_status_line(mut self, hide: bool) -> Self {
        self.hide_status_line = hide;
        self
    }

    /// The current value.
    pub fn text(&self) -> &str {
        self.state.text()
    }

    /// The editing model, for a caller that wants to drive it directly.
    pub fn state(&self) -> &TextFieldState {
        &self.state
    }

    /// Replace the value and notify.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.state.set_text(text);
        self.emit_changed(cx);
    }

    /// Empty the field and notify.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.state.clear();
        self.emit_changed(cx);
    }

    /// Set the faint derived preview under the box.
    pub fn set_preview(&mut self, preview: Option<SharedString>, cx: &mut Context<Self>) {
        self.preview = preview;
        cx.notify();
    }

    /// Set the validation message. `Some` replaces the preview and reddens the border.
    pub fn set_invalid(&mut self, message: Option<SharedString>, cx: &mut Context<Self>) {
        self.invalid = message;
        cx.notify();
    }

    /// Whether the field is currently rejecting its value.
    pub fn is_invalid(&self) -> bool {
        self.invalid.is_some()
    }

    /// Notify and emit [`TextInputEvent::Changed`].
    fn emit_changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    /// The control half of the edit set. Printable keys reach the field through the platform
    /// input handler, so consuming them here would insert every character twice.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let before = self.state.text().to_string();
        if !self.state.handle_edit_keystroke(&event.keystroke) {
            return;
        }
        cx.stop_propagation();
        if self.state.text() == before {
            cx.emit(TextInputEvent::CaretMoved);
            cx.notify();
        } else {
            self.emit_changed(cx);
        }
    }

    /// Place the caret where the pointer landed.
    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        if let (Some(bounds), Some(line)) = (self.last_bounds, self.last_layout.as_ref()) {
            let x = event.position.x - bounds.left() + self.last_scroll;
            self.state.set_cursor(line.closest_index_for_x(x));
        }
        cx.emit(TextInputEvent::CaretMoved);
        cx.notify();
    }

    /// The chrome this field draws around its value.
    fn chrome(&self) -> FieldChrome {
        FieldChrome {
            label: self.label.clone(),
            icon: self.icon,
            preview: self.preview.clone(),
            invalid: self.invalid.clone(),
            focused: false,
            mono: self.mono,
            height: self.height,
            hide_status_line: self.hide_status_line,
        }
    }
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let focused = self.focus_handle.is_focused(window);
        let mut chrome = self.chrome();
        chrome.focused = focused;

        chrome
            .wrap(&theme, TextInputElement { input: cx.entity() })
            .key_context(TEXT_FIELD_KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
    }
}

/// The gpui key context a [`TextInput`] pushes.
///
/// An app that binds bare letters (`h`, `q`, `y`) must shadow them in this context with
/// `gpui::NoAction`, or typing those letters fires the action instead of reaching the field.
pub const TEXT_FIELD_KEY_CONTEXT: &str = "FleetTextField";

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.state.range_from_utf16(&range_utf16);
        actual_range.replace(self.state.range_to_utf16(&range));
        self.state.text().get(range).map(str::to_string)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let caret = self.state.offset_to_utf16(self.state.cursor());
        Some(UTF16Selection {
            range: caret..caret,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.state
            .marked_range()
            .map(|range| self.state.range_to_utf16(&range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.unmark();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.resolve_range(range_utf16);
        self.state.replace_range(range, text);
        self.emit_changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.resolve_range(range_utf16);
        let start = range.start;
        self.state.replace_and_mark(range, new_text);
        if let Some(selected) = new_selected_range_utf16 {
            let selected = self.state.range_from_utf16(&selected);
            self.state.set_cursor(start + selected.end);
        }
        self.emit_changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.state.range_from_utf16(&range_utf16);
        let left = element_bounds.left() - self.last_scroll;
        Some(Bounds::from_corners(
            point(left + line.x_for_index(range.start), element_bounds.top()),
            point(left + line.x_for_index(range.end), element_bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line = self.last_layout.as_ref()?;
        let index = line.index_for_x(point.x - bounds.left() + self.last_scroll)?;
        Some(self.state.offset_to_utf16(index))
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.state.range_from_utf16(&range_utf16);
        self.state.set_cursor(range.end);
        cx.notify();
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.state.len_utf16())
    }
}

impl TextInput {
    /// The byte range an IME edit applies to: the explicit range, else the marked range, else
    /// the caret.
    fn resolve_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        match range_utf16 {
            Some(range) => self.state.range_from_utf16(&range),
            None => self
                .state
                .marked_range()
                .unwrap_or(self.state.cursor()..self.state.cursor()),
        }
    }
}

/// The painted half of a [`TextInput`]: one shaped line, one caret, one marked-text underline.
struct TextInputElement {
    input: Entity<TextInput>,
}

/// What [`TextInputElement::prepaint`] hands to `paint`.
struct TextInputPrepaint {
    line: Option<ShapedLine>,
    caret: Option<PaintQuad>,
    scroll: Pixels,
}

impl IntoElement for TextInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextInputElement {
    type RequestLayoutState = ();
    type PrepaintState = TextInputPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let theme = cx.theme().clone();
        let input = self.input.read(cx);
        let style = window.text_style();
        let empty = input.state.is_empty();
        let display: SharedString = if empty {
            input.placeholder.clone().unwrap_or_default()
        } else {
            input.state.shared_text()
        };
        let color = if empty {
            theme.colors.text_muted
        } else {
            style.color
        };
        let caret_offset = if empty { 0 } else { input.state.cursor() };
        let marked = if empty {
            None
        } else {
            input.state.marked_range()
        };

        let run = TextRun {
            len: display.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match marked {
            Some(marked) if marked.end <= display.len() => vec![
                TextRun {
                    len: marked.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display.len() - marked.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            _ => vec![run],
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display, font_size, &runs, None);

        // Keep the caret in view by sliding the line left when it runs past the box.
        let caret_x = line.x_for_index(caret_offset);
        let caret_w = theme.metrics.focus_ring_w;
        let scroll = (caret_x + caret_w - bounds.size.width).max(px(0.0));
        let caret = fill(
            Bounds::new(
                point(bounds.left() + caret_x - scroll, bounds.top()),
                size(caret_w, bounds.size.height),
            ),
            theme.colors.accent,
        );

        TextInputPrepaint {
            line: Some(line),
            caret: Some(caret),
            scroll,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        let scroll = prepaint.scroll;
        if let Some(line) = prepaint.line.take() {
            let origin = point(bounds.origin.x - scroll, bounds.origin.y);
            // A shaped single line cannot wrap, so the only error path here is a font the
            // platform withdrew mid-frame; skipping the paint beats panicking on it.
            let _ = line.paint(
                origin,
                window.line_height(),
                TextAlign::Left,
                None,
                window,
                cx,
            );
            self.input.update(cx, |input, _cx| {
                input.last_layout = Some(line);
                input.last_bounds = Some(bounds);
                input.last_scroll = scroll;
            });
        }

        if focus_handle.is_focused(window)
            && let Some(caret) = prepaint.caret.take()
        {
            window.paint_quad(caret);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(text: &str, caret_chars: usize) -> TextFieldState {
        let mut state = TextFieldState::from_text(text);
        let offset = text
            .char_indices()
            .nth(caret_chars)
            .map_or(text.len(), |(index, _)| index);
        state.set_cursor(offset);
        state
    }

    #[test]
    fn insert_puts_the_caret_after_the_insertion() {
        let mut s = TextFieldState::new();
        s.insert("feat/");
        s.insert("rut");
        assert_eq!(s.text(), "feat/rut");
        assert_eq!(s.caret_chars(), 8);
    }

    #[test]
    fn insert_strips_newlines() {
        let mut s = TextFieldState::new();
        s.insert("one\ntwo\r\tthree");
        assert_eq!(s.text(), "onetwothree");
    }

    #[test]
    fn backspace_walks_char_boundaries() {
        let mut s = state("héllo", 2);
        assert!(s.backspace());
        assert_eq!(s.text(), "hllo");
        assert_eq!(s.caret_chars(), 1);
    }

    #[test]
    fn backspace_at_the_start_is_a_no_op() {
        let mut s = state("x", 0);
        assert!(!s.backspace());
        assert_eq!(s.text(), "x");
    }

    #[test]
    fn delete_forward_removes_the_char_after_the_caret() {
        let mut s = state("abc", 1);
        assert!(s.delete_forward());
        assert_eq!(s.text(), "ac");
        assert_eq!(s.caret_chars(), 1);
    }

    #[test]
    fn ctrl_w_deletes_one_word_and_its_trailing_space() {
        let mut s = TextFieldState::from_text("origin/main feature ");
        assert!(s.delete_word_before());
        assert_eq!(s.text(), "origin/main ");
        assert!(s.delete_word_before());
        assert_eq!(s.text(), "");
        assert!(!s.delete_word_before());
    }

    #[test]
    fn ctrl_u_and_ctrl_k_cut_around_the_caret() {
        let mut s = state("abcdef", 3);
        assert!(s.delete_to_end());
        assert_eq!(s.text(), "abc");
        assert!(s.delete_to_start());
        assert_eq!(s.text(), "");
    }

    #[test]
    fn ctrl_a_and_ctrl_e_park_the_caret() {
        let mut s = state("abc", 1);
        assert!(s.move_to_start());
        assert_eq!(s.caret_chars(), 0);
        assert!(!s.move_to_start());
        assert!(s.move_to_end());
        assert_eq!(s.caret_chars(), 3);
    }

    #[test]
    fn arrows_stop_at_the_ends() {
        let mut s = state("ab", 0);
        assert!(!s.move_left());
        assert!(s.move_right());
        assert!(s.move_right());
        assert!(!s.move_right());
        assert_eq!(s.caret_chars(), 2);
    }

    #[test]
    fn set_cursor_snaps_to_a_char_boundary() {
        let mut s = TextFieldState::from_text("é");
        s.set_cursor(1);
        assert_eq!(s.cursor(), 0);
        s.set_cursor(99);
        assert_eq!(s.cursor(), 2);
    }

    #[test]
    fn utf16_offsets_round_trip() {
        let s = TextFieldState::from_text("a😀b");
        assert_eq!(s.len_utf16(), 4);
        assert_eq!(s.offset_to_utf16(s.text().len()), 4);
        assert_eq!(s.offset_from_utf16(3), 5);
    }

    #[test]
    fn replace_and_mark_tracks_the_composing_range() {
        let mut s = TextFieldState::from_text("ab");
        s.replace_and_mark(2..2, "か");
        assert_eq!(s.text(), "abか");
        assert_eq!(s.marked_range(), Some(2..5));
        s.replace_range(2..5, "か");
        assert_eq!(s.marked_range(), None);
    }

    #[test]
    fn edit_keystrokes_ignore_printable_keys() {
        let mut s = TextFieldState::from_text("ab");
        let printable = Keystroke {
            modifiers: Default::default(),
            key: "c".into(),
            key_char: Some("c".into()),
        };
        assert!(!s.handle_edit_keystroke(&printable));
        assert!(s.handle_keystroke(&printable));
        assert_eq!(s.text(), "abc");
    }

    #[test]
    fn split_at_char_saturates() {
        assert_eq!(split_at_char("abc", 1), ("a", "bc"));
        assert_eq!(split_at_char("abc", 9), ("abc", ""));
        assert_eq!(split_at_char("é!", 1), ("é", "!"));
    }
}
