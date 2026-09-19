//! `TextInput` — Fleet's single live text editor, in single-line and multi-line modes.
//!
//! The entity owns the focus handle, selection, undo history, platform input bridge, layout
//! cache and scroll position that must survive frames. Callers hold one `Entity<TextInput>` and
//! never decode editing keys around it. The older presentational input families remain only as
//! migration shims and should not be used for new surfaces.

use gpui::{
    App, ClipboardItem, Context, CursorStyle, EventEmitter, FocusHandle, Focusable, KeyContext,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Point, ScrollWheelEvent,
    SharedString, Subscription, Window, div, prelude::*,
};

use crate::{
    icons::Icon,
    text::{Text, TextRole, styled_with},
    theme::ActiveTheme,
    tone::Tone,
};

pub mod actions;
mod buffer;
mod element;
mod history;
mod platform;

use actions::*;
pub use buffer::{InputBuffer, InputMode};
use element::{LineLayoutCache, TextInputElement};
pub use history::{HISTORY_CAP, TYPING_GROUP_WINDOW};

#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod tests;

/// The key-context identifier published by every [`TextInput`].
pub const TEXT_INPUT_KEY_CONTEXT: &str = "FleetTextInput";

const SINGLE_LINE_MODE: &str = "single_line";
const MULTILINE_MODE: &str = "multiline";

/// Events emitted by a [`TextInput`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextInputEvent {
    /// The text changed.
    Changed,
    /// A single-line owner explicitly submitted the value through [`TextInput::submit`].
    Submitted,
    /// The component's focus handle lost focus.
    Blurred,
}

/// A live single-line or logical multi-line text editor.
pub struct TextInput {
    pub(super) focus_handle: FocusHandle,
    pub(super) buffer: InputBuffer,
    pub(super) placeholder: SharedString,
    label: Option<SharedString>,
    icon: Option<Icon>,
    mono: bool,
    preview: Option<SharedString>,
    invalid: Option<SharedString>,
    hide_status_line: bool,
    pub(super) read_only: bool,
    filter: Option<fn(char) -> bool>,
    line_cache: LineLayoutCache,
    pub(super) last_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    pub(super) line_height: gpui::Pixels,
    pub(super) horizontal_scroll: gpui::Pixels,
    pub(super) scroll_row: usize,
    pub(super) reveal_caret: bool,
    drag_anchor: Option<usize>,
    composition_group_open: bool,
    blur_subscription: Option<Subscription>,
}

impl TextInput {
    /// Create an empty editor in `mode` with its own focus handle.
    #[must_use]
    pub fn new(mode: InputMode, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            buffer: InputBuffer::new(mode),
            placeholder: SharedString::default(),
            label: None,
            icon: None,
            mono: false,
            preview: None,
            invalid: None,
            hide_status_line: false,
            read_only: false,
            filter: None,
            line_cache: LineLayoutCache::default(),
            last_bounds: None,
            line_height: gpui::Pixels::ZERO,
            horizontal_scroll: gpui::Pixels::ZERO,
            scroll_row: 0,
            reveal_caret: true,
            drag_anchor: None,
            composition_group_open: false,
            blur_subscription: None,
        }
    }

    /// The current UTF-8 value.
    #[must_use]
    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// Replace the value, park the caret at its end and clear undo history.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let before = self.buffer.text().to_owned();
        self.finish_composition();
        self.buffer.set_text(text);
        self.line_cache.clear();
        self.reveal_caret = true;
        if self.buffer.text() != before {
            cx.emit(TextInputEvent::Changed);
        }
        cx.notify();
    }

    /// Empty the value as one undoable edit.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.finish_composition();
        let now = cx.background_executor().now();
        if self.buffer.clear(now) {
            self.changed(cx);
        }
    }

    /// Select the whole value.
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.buffer.select_all() {
            self.view_changed(cx);
        }
    }

    /// Move the caret to the document end.
    pub fn move_to_end(&mut self, cx: &mut Context<Self>) {
        if self.buffer.move_to_document_end(false) {
            self.view_changed(cx);
        }
    }

    /// Replace the empty-value placeholder.
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            self.line_cache.clear();
            cx.notify();
        }
    }

    /// Set or clear the label above the editor.
    pub fn set_label(&mut self, label: Option<SharedString>, cx: &mut Context<Self>) {
        if self.label != label {
            self.label = label;
            cx.notify();
        }
    }

    /// Set or clear the leading icon.
    pub fn set_icon(&mut self, icon: Option<Icon>, cx: &mut Context<Self>) {
        if self.icon != icon {
            self.icon = icon;
            cx.notify();
        }
    }

    /// Render the value in the data face when `mono` is true.
    pub fn set_mono(&mut self, mono: bool, cx: &mut Context<Self>) {
        if self.mono != mono {
            self.mono = mono;
            self.line_cache.clear();
            cx.notify();
        }
    }

    /// Set or clear the derived single-line status preview.
    pub fn set_preview(&mut self, preview: Option<SharedString>, cx: &mut Context<Self>) {
        if self.preview != preview {
            self.preview = preview;
            cx.notify();
        }
    }

    /// Show or hide the single-line status slot.
    pub fn set_hide_status_line(&mut self, hide: bool, cx: &mut Context<Self>) {
        if self.hide_status_line != hide {
            self.hide_status_line = hide;
            cx.notify();
        }
    }

    /// Enable or disable user mutations while preserving motion, selection and copy.
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only != read_only {
            self.read_only = read_only;
            cx.notify();
        }
    }

    /// Set validation state. Multi-line mode uses the border and ignores the message visually.
    pub fn set_invalid(&mut self, message: Option<SharedString>, cx: &mut Context<Self>) {
        if self.invalid != message {
            self.invalid = message;
            cx.notify();
        }
    }

    /// Filter inserted and pasted characters. Programmatic [`Self::set_text`] is not filtered.
    pub fn set_filter(&mut self, filter: Option<fn(char) -> bool>, cx: &mut Context<Self>) {
        self.filter = filter;
        cx.notify();
    }

    /// Whether the value is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Whether an input method currently owns marked text.
    #[must_use]
    pub fn is_composing(&self) -> bool {
        self.buffer.marked_range().is_some()
    }

    /// Whether user mutations are disabled.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Whether validation is currently failing.
    #[must_use]
    pub fn is_invalid(&self) -> bool {
        self.invalid.is_some()
    }

    /// Whether the anchor and caret enclose text.
    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.buffer.has_selection()
    }

    /// Clone the focus handle owned by this editor.
    #[must_use]
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// Focus this editor.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }

    /// The configured input mode.
    #[must_use]
    pub fn mode(&self) -> InputMode {
        self.buffer.mode()
    }

    /// The editing engine, for read-only inspection by an owner.
    #[must_use]
    pub fn buffer(&self) -> &InputBuffer {
        &self.buffer
    }

    /// Emit [`TextInputEvent::Submitted`] for a single-line owner.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if matches!(self.mode(), InputMode::SingleLine) {
            cx.emit(TextInputEvent::Submitted);
        }
    }

    pub(super) fn ensure_blur_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.blur_subscription.is_none() {
            let focus = self.focus_handle.clone();
            self.blur_subscription = Some(cx.on_blur(&focus, window, |input, _window, cx| {
                let was_composing = input.buffer.marked_range().is_some();
                input.buffer.unmark();
                input.finish_composition();
                if was_composing {
                    input.line_cache.clear();
                    cx.notify();
                }
                cx.emit(TextInputEvent::Blurred);
            }));
        }
    }

    fn finish_composition(&mut self) {
        if self.composition_group_open {
            self.buffer.end_history_group();
            self.composition_group_open = false;
        }
    }

    pub(super) fn filtered(&self, text: &str) -> String {
        match self.filter {
            Some(filter) => text
                .chars()
                .filter(|character| filter(*character))
                .collect(),
            None => text.to_owned(),
        }
    }

    pub(super) fn changed(&mut self, cx: &mut Context<Self>) {
        self.reveal_caret = true;
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn view_changed(&mut self, cx: &mut Context<Self>) {
        self.finish_composition();
        self.reveal_caret = true;
        cx.notify();
    }

    fn edit(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut InputBuffer, std::time::Instant) -> bool,
    ) {
        if self.read_only {
            return;
        }
        self.finish_composition();
        let now = cx.background_executor().now();
        if edit(&mut self.buffer, now) {
            self.changed(cx);
        }
    }

    fn motion(&mut self, cx: &mut Context<Self>, motion: impl FnOnce(&mut InputBuffer) -> bool) {
        if motion(&mut self.buffer) {
            self.view_changed(cx);
        }
    }

    fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_left(false));
    }

    fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_right(false));
    }

    fn move_word_left(&mut self, _: &MoveWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_word_left(false));
    }

    fn move_word_right(&mut self, _: &MoveWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_word_right(false));
    }

    fn move_line_start(&mut self, _: &MoveToLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_to_line_start(false));
    }

    fn move_line_end(&mut self, _: &MoveToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_to_line_end(false));
    }

    fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.mode(), InputMode::SingleLine) {
            cx.propagate();
        } else {
            self.motion(cx, |buffer| buffer.move_up(false));
        }
    }

    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.mode(), InputMode::SingleLine) {
            cx.propagate();
        } else {
            self.motion(cx, |buffer| buffer.move_down(false));
        }
    }

    fn move_start(&mut self, _: &MoveToStart, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_to_document_start(false));
    }

    fn move_end(&mut self, _: &MoveToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_to_document_end(false));
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_left(true));
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_right(true));
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_word_left(true));
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_word_right(true));
    }

    fn select_line_start(&mut self, _: &SelectToLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_to_line_start(true));
    }

    fn select_line_end(&mut self, _: &SelectToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_to_line_end(true));
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.mode(), InputMode::SingleLine) {
            cx.propagate();
        } else {
            self.motion(cx, |buffer| buffer.move_up(true));
        }
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.mode(), InputMode::SingleLine) {
            cx.propagate();
        } else {
            self.motion(cx, |buffer| buffer.move_down(true));
        }
    }

    fn select_start(&mut self, _: &SelectToStart, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_to_document_start(true));
    }

    fn select_end(&mut self, _: &SelectToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |buffer| buffer.move_to_document_end(true));
    }

    fn select_all_action(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all(cx);
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, InputBuffer::backspace);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, InputBuffer::delete_forward);
    }

    fn delete_word_backward(
        &mut self,
        _: &DeleteWordBackward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(cx, InputBuffer::delete_word_backward);
    }

    fn delete_word_forward(
        &mut self,
        _: &DeleteWordForward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(cx, InputBuffer::delete_word_forward);
    }

    fn delete_line_start(&mut self, _: &DeleteToLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, InputBuffer::delete_to_line_start);
    }

    fn delete_line_end(&mut self, _: &DeleteToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, InputBuffer::delete_to_line_end);
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.mode(), InputMode::SingleLine) {
            cx.propagate();
            return;
        }
        let accepted = self.filter.is_none_or(|filter| filter('\n'));
        if accepted {
            self.edit(cx, InputBuffer::insert_newline);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only || !self.copy_selection(cx) {
            return;
        }
        self.edit(cx, |buffer, now| buffer.insert("", now));
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let text = self.filtered(&text);
        if !text.is_empty() {
            self.edit(cx, |buffer, now| {
                buffer.replace_range(buffer.selected_range(), &text, now)
            });
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.finish_composition();
        if self.buffer.undo() {
            self.changed(cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.finish_composition();
        if self.buffer.redo() {
            self.changed(cx);
        }
    }

    fn copy_selection(&self, cx: &mut Context<Self>) -> bool {
        let selected = self.buffer.selected_text();
        if selected.is_empty() {
            return false;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(selected.to_owned()));
        true
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let Some(offset) = self.offset_for_point(event.position) else {
            return;
        };
        self.finish_composition();
        if event.click_count >= 3 {
            self.buffer.select_line_at(offset);
            self.drag_anchor = None;
        } else if event.click_count == 2 {
            self.buffer.select_word_at(offset);
            self.drag_anchor = None;
        } else if event.modifiers.shift {
            self.buffer.move_to(offset, true);
            self.drag_anchor = Some(self.buffer.anchor());
        } else {
            self.buffer.set_caret(offset);
            self.drag_anchor = Some(offset);
        }
        self.view_changed(cx);
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !event.dragging() {
            return;
        }
        let (Some(anchor), Some(offset)) =
            (self.drag_anchor, self.offset_for_point(event.position))
        else {
            return;
        };
        let before = self.buffer.selected_range();
        self.buffer.set_selected_range(anchor..offset);
        if self.buffer.selected_range() != before {
            self.view_changed(cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.drag_anchor = None;
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let InputMode::Multiline { max_rows, .. } = self.mode() else {
            return;
        };
        let visible = self.buffer.line_count().min(max_rows.max(1));
        let max_scroll = self.buffer.line_count().saturating_sub(visible);
        if max_scroll == 0 {
            return;
        }
        let delta = event.delta.pixel_delta(window.line_height()).y;
        let rows = (f32::from(delta.abs()) / f32::from(window.line_height())).ceil() as usize;
        let next = if delta > gpui::Pixels::ZERO {
            self.scroll_row.saturating_sub(rows.max(1))
        } else {
            self.scroll_row.saturating_add(rows.max(1)).min(max_scroll)
        };
        if next != self.scroll_row {
            self.scroll_row = next;
            self.reveal_caret = false;
            cx.notify();
        }
        cx.stop_propagation();
    }

    pub(super) fn offset_for_point(&self, position: Point<gpui::Pixels>) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line = if position.y < bounds.top() {
            0
        } else if position.y >= bounds.bottom() {
            self.buffer.line_count().saturating_sub(1)
        } else {
            let local_y = position.y - bounds.top();
            self.scroll_row + (f32::from(local_y) / f32::from(self.line_height)).floor() as usize
        }
        .min(self.buffer.line_count().saturating_sub(1));
        let (start, layout) = self.line_cache.line(line)?;
        let scroll = if matches!(self.mode(), InputMode::SingleLine) {
            self.horizontal_scroll
        } else {
            gpui::Pixels::ZERO
        };
        let local = layout.closest_index_for_x(position.x - bounds.left() + scroll);
        Some(start + local)
    }

    pub(super) fn line_for_offset(&self, offset: usize) -> Option<(usize, usize)> {
        self.line_cache.line_index_for_offset(offset)
    }

    fn key_context(&self) -> KeyContext {
        let mut context = KeyContext::default();
        context.add(TEXT_INPUT_KEY_CONTEXT);
        context.set(
            "mode",
            match self.mode() {
                InputMode::SingleLine => SINGLE_LINE_MODE,
                InputMode::Multiline { .. } => MULTILINE_MODE,
            },
        );
        context
    }

    fn border_color(&self, focused: bool, cx: &App) -> gpui::Hsla {
        let theme = cx.theme();
        if self.invalid.is_some() {
            theme.colors.danger
        } else if focused {
            theme.colors.focus_ring
        } else {
            theme.colors.border
        }
    }

    fn value_role(&self) -> TextRole {
        if self.mono {
            TextRole::Data
        } else {
            TextRole::Ui
        }
    }

    fn render_chrome(&self, focused: bool, value: impl IntoElement, cx: &App) -> gpui::Div {
        let theme = cx.theme();
        let role = self.value_role();
        let icon_color = if focused {
            theme.colors.text_secondary
        } else {
            theme.colors.text_muted
        };
        let box_element = div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .w_full()
            .px(theme.space.md)
            .rounded(theme.radii.sm)
            .bg(theme.colors.bg)
            .border(theme.metrics.hairline)
            .border_color(self.border_color(focused, cx))
            .overflow_hidden()
            .children(
                self.icon
                    .map(|icon| icon.el().size(crate::IconSize::Medium).color(icon_color)),
            )
            .child(
                styled_with(div(), role.style(theme), theme)
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h(role.style(theme).line_height)
                    .when(
                        matches!(self.mode(), InputMode::Multiline { .. }),
                        |element| element.h_auto(),
                    )
                    .overflow_hidden()
                    .text_color(theme.colors.text)
                    .child(value),
            )
            .when_else(
                matches!(self.mode(), InputMode::SingleLine),
                |element| element.h(theme.metrics.text_field_h),
                |element| element.min_h(theme.metrics.text_field_h).py(theme.space.sm),
            );

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(theme.space.xxs)
            .children(self.label.clone().map(Text::label))
            .child(box_element)
            .when(
                matches!(self.mode(), InputMode::SingleLine) && !self.hide_status_line,
                |element| {
                    element.child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(theme.metrics.field_status_h)
                            .w_full()
                            .overflow_hidden()
                            .child(match self.invalid.clone() {
                                Some(message) => Text::hint(message).tone(Tone::Danger).ellipsize(),
                                None => Text::hint(self.preview.clone().unwrap_or_default())
                                    .faint()
                                    .ellipsize(),
                            }),
                    )
                },
            )
    }
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle()
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        self.render_chrome(focused, TextInputElement { input: cx.entity() }, cx)
            .key_context(self.key_context())
            .track_focus(&self.focus_handle)
            .cursor(if self.read_only {
                CursorStyle::Arrow
            } else {
                CursorStyle::IBeam
            })
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::move_word_left))
            .on_action(cx.listener(Self::move_word_right))
            .on_action(cx.listener(Self::move_line_start))
            .on_action(cx.listener(Self::move_line_end))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::move_start))
            .on_action(cx.listener(Self::move_end))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_line_start))
            .on_action(cx.listener(Self::select_line_end))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_start))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::select_all_action))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_backward))
            .on_action(cx.listener(Self::delete_word_forward))
            .on_action(cx.listener(Self::delete_line_start))
            .on_action(cx.listener(Self::delete_line_end))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
    }
}
