//! `TextInput` — Fleet's single live text editor, in single-line and multi-line modes.
//!
//! The entity owns the focus handle, selection, undo history, platform input bridge, layout
//! cache and scroll position that must survive frames. Callers hold one `Entity<TextInput>` and
//! never decode editing keys around it; it is the only input the kit has (ADR 0020).
//!
//! This file keeps the entity, its fields and its public API. The action handlers live in
//! [`handlers`], mouse and wheel input in [`pointer`], the painted-layout arithmetic in
//! [`geometry`], and the box drawn around the value in [`chrome`].

use gpui::{
    App, Context, CursorStyle, EventEmitter, FocusHandle, Focusable, KeyContext, MouseButton,
    SharedString, Subscription, Window, div, prelude::*,
};

use crate::{icons::Icon, text::styled_with, theme::ActiveTheme};

pub mod actions;
mod buffer;
mod chrome;
mod element;
mod geometry;
mod handlers;
mod history;
mod platform;
mod pointer;

pub use buffer::{InputBuffer, InputMode};
use element::{LineLayoutCache, TextInputElement};
pub use history::{HISTORY_CAP, TYPING_GROUP_WINDOW};

#[cfg(test)]
pub(crate) mod live_tests;
#[cfg(test)]
mod tests;

/// The key-context identifier published by every [`TextInput`].
pub const TEXT_INPUT_KEY_CONTEXT: &str = "FleetTextInput";

const SINGLE_LINE_MODE: &str = "single_line";
const MULTILINE_MODE: &str = "multiline";
const ENTER_NEWLINE: &str = "newline";
const ENTER_OWNER: &str = "owner";

/// Events emitted by a [`TextInput`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextInputEvent {
    /// The text changed.
    Changed,
    /// A single-line owner explicitly submitted the value through [`TextInput::submit`].
    Submitted,
    /// The component's focus handle gained focus, including from a click on the value, and on
    /// the first paint when the handle was already focused before that paint.
    ///
    /// A surface that remembers *which* of its editors owns the keyboard must mirror this
    /// event into that marker, or a pointer-driven focus change and the marker disagree and
    /// the next focus reconciliation moves the caret back.
    Focused,
    /// The component's focus handle lost focus.
    Blurred,
}

/// A live single-line or logical multi-line text editor.
pub struct TextInput {
    focus_handle: FocusHandle,
    buffer: InputBuffer,
    placeholder: SharedString,
    label: Option<SharedString>,
    icon: Option<Icon>,
    mono: bool,
    preview: Option<SharedString>,
    invalid: Option<SharedString>,
    hide_status_line: bool,
    read_only: bool,
    enter_inserts_newline: bool,
    embedded: bool,
    filter: Option<fn(char) -> bool>,
    line_cache: LineLayoutCache,
    last_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    line_height: gpui::Pixels,
    horizontal_scroll: gpui::Pixels,
    scroll_row: usize,
    reveal_caret: bool,
    drag_anchor: Option<usize>,
    vertical_goal_x: Option<gpui::Pixels>,
    composition_group_open: bool,
    focus_subscriptions: Option<[Subscription; 2]>,
    #[cfg(test)]
    last_selection_rows: Vec<usize>,
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
            enter_inserts_newline: true,
            embedded: false,
            filter: None,
            line_cache: LineLayoutCache::default(),
            last_bounds: None,
            line_height: gpui::Pixels::ZERO,
            horizontal_scroll: gpui::Pixels::ZERO,
            scroll_row: 0,
            reveal_caret: true,
            drag_anchor: None,
            vertical_goal_x: None,
            composition_group_open: false,
            focus_subscriptions: None,
            #[cfg(test)]
            last_selection_rows: Vec::new(),
        }
    }

    /// The current UTF-8 value.
    #[must_use]
    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// Replace the value, park the caret at its end and clear undo history.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.set_text_inner(text, true, cx);
    }

    /// Replace the value without emitting an owner-visible change event.
    pub(crate) fn set_text_silent(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.set_text_inner(text, false, cx);
    }

    fn set_text_inner(
        &mut self,
        text: impl Into<String>,
        emit_changed: bool,
        cx: &mut Context<Self>,
    ) {
        let before = self.buffer.text().to_owned();
        self.finish_composition();
        self.buffer.set_text(text);
        self.line_cache.clear();
        self.vertical_goal_x = None;
        self.reveal_caret = true;
        if emit_changed && self.buffer.text() != before {
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

    /// Insert user text at the caret, replacing a selection as one undoable edit.
    ///
    /// The configured character filter and the input mode's newline/tab normalization apply in
    /// the same way as platform typing and paste.
    pub fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let text = self.filtered(text);
        if text.is_empty() {
            return;
        }
        self.edit(cx, |buffer, now| buffer.insert(&text, now));
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

    /// Choose whether plain Enter inserts a newline in multi-line mode.
    ///
    /// The value is published as the `enter = newline | owner` key-context attribute. It has
    /// no effect in single-line mode, where Enter always belongs to the containing surface.
    pub fn set_enter_inserts_newline(&mut self, inserts: bool, cx: &mut Context<Self>) {
        if self.enter_inserts_newline != inserts {
            self.enter_inserts_newline = inserts;
            cx.notify();
        }
    }

    /// Render only the editing surface so the owner can supply its own chrome.
    ///
    /// An embedded editor draws no box, no border and no status line: it is one line of text
    /// with a caret, sized by its text role, for a surface that already owns the frame around
    /// it — the pane header's filter slot and the palette's query row are both 30-44 px rows
    /// that a framed field would not fit in.
    pub fn set_embedded(&mut self, embedded: bool, cx: &mut Context<Self>) {
        if self.embedded != embedded {
            self.embedded = embedded;
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

    /// Move one row vertically, using wrapped geometry when it belongs to the current text.
    ///
    /// Returns whether the caret moved. Consecutive visual moves retain a pixel goal-x; every
    /// other edit or motion clears it. Without current geometry this falls back to the engine's
    /// logical-line motion.
    pub fn move_vertical(&mut self, down: bool, select: bool, cx: &mut Context<Self>) -> bool {
        if matches!(self.mode(), InputMode::SingleLine) || self.is_composing() {
            return false;
        }
        let moved = if self.has_current_layout() {
            self.move_visual_row(down, select)
        } else if down {
            self.buffer.move_down(select)
        } else {
            self.buffer.move_up(select)
        };
        if moved {
            self.reveal_caret = true;
            cx.notify();
        }
        moved
    }

    /// Whether the caret belongs to the first visual row, or first logical line without layout.
    pub(crate) fn on_first_visual_row(&self) -> bool {
        if !self.has_current_layout() {
            return self.buffer.on_first_line();
        }
        self.position_for_offset(self.buffer.caret())
            .is_none_or(|position| position.y < self.line_height)
    }

    /// Whether the caret belongs to the last visual row, or last logical line without layout.
    pub(crate) fn on_last_visual_row(&self) -> bool {
        if !self.has_current_layout() {
            return self.buffer.on_last_line();
        }
        let caret = self.buffer.caret();
        let Some(position) = self.position_for_offset(caret) else {
            return self.buffer.on_last_line();
        };
        let wrapped = position.x == gpui::Pixels::ZERO
            && position.y > gpui::Pixels::ZERO
            && self.buffer.line_start(caret) != caret;
        let y = if wrapped {
            position.y - self.line_height
        } else {
            position.y
        };
        y + self.line_height >= self.content_height()
    }

    /// Registers the focus and blur listeners the entity reports [`TextInputEvent`] from.
    ///
    /// Both are registered from the painted element, because the handle only joins the focus
    /// tree once the element exists. `on_focus` is what makes a click on the value visible to
    /// the surface that remembers which of its editors owns the keyboard.
    ///
    /// A listener registered here only activates at the end of the current effect cycle, so it
    /// cannot see the focus change a surface made before this editor's first paint. Report that
    /// initial focus directly instead, or an owner that focuses an editor in the frame it
    /// creates it never hears [`TextInputEvent::Focused`] at all.
    pub(super) fn ensure_focus_subscriptions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.focus_subscriptions.is_some() {
            return;
        }
        let focus = self.focus_handle.clone();
        let focused = cx.on_focus(&focus, window, |_input, _window, cx| {
            cx.emit(TextInputEvent::Focused);
        });
        let blurred = cx.on_blur(&focus, window, |input, _window, cx| {
            let was_composing = input.buffer.marked_range().is_some();
            input.buffer.unmark();
            input.finish_composition();
            if was_composing {
                input.line_cache.clear();
                cx.notify();
            }
            cx.emit(TextInputEvent::Blurred);
        });
        self.focus_subscriptions = Some([focused, blurred]);
        if focus.is_focused(window) {
            cx.emit(TextInputEvent::Focused);
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
        self.vertical_goal_x = None;
        self.reveal_caret = true;
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn view_changed(&mut self, cx: &mut Context<Self>) {
        self.finish_composition();
        self.vertical_goal_x = None;
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
        context.set(
            "enter",
            if self.enter_inserts_newline {
                ENTER_NEWLINE
            } else {
                ENTER_OWNER
            },
        );
        context
    }

    #[cfg(test)]
    pub(crate) fn test_bounds(&self) -> Option<gpui::Bounds<gpui::Pixels>> {
        self.last_bounds
    }

    #[cfg(test)]
    pub(crate) fn test_line_height(&self) -> gpui::Pixels {
        self.line_height
    }

    #[cfg(test)]
    pub(crate) fn test_position_for_offset(
        &self,
        offset: usize,
    ) -> Option<gpui::Point<gpui::Pixels>> {
        self.position_for_offset(offset)
    }

    #[cfg(test)]
    pub(crate) fn test_visual_rows(&self) -> usize {
        self.line_cache.visual_rows()
    }

    #[cfg(test)]
    pub(crate) fn test_reset_shape_probe(&mut self) {
        self.line_cache.reset_probe();
    }

    #[cfg(test)]
    pub(crate) fn test_shape_miss_counts(&self) -> &[usize] {
        self.line_cache.miss_counts()
    }

    #[cfg(test)]
    pub(crate) fn test_scroll_row(&self) -> usize {
        self.scroll_row
    }

    #[cfg(test)]
    pub(crate) fn test_value_color(&self, cx: &App) -> gpui::Hsla {
        if self.buffer.is_empty() {
            cx.theme().colors.text_muted
        } else {
            cx.theme().colors.text
        }
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
        let theme = cx.theme();
        let role = self.value_role();
        let content = if self.embedded {
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
                .child(TextInputElement { input: cx.entity() })
                .into_any_element()
        } else {
            self.render_chrome(focused, TextInputElement { input: cx.entity() }, cx)
                .into_any_element()
        };
        div()
            .when(self.embedded, |element| {
                element.flex().flex_1().min_w_0().w_full()
            })
            .when(!self.embedded, |element| element.w_full())
            .child(content)
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
            .on_action(cx.listener(Self::move_row_start))
            .on_action(cx.listener(Self::move_row_end))
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
            .on_action(cx.listener(Self::select_row_start))
            .on_action(cx.listener(Self::select_row_end))
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
