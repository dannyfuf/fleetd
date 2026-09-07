use std::ops::Range;

use gpui::{
    App, Bounds, Context, CursorStyle, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point, ShapedLine, SharedString,
    UTF16Selection, Window, point, prelude::*, px,
};

use super::{EditEffect, FieldChrome, TextFieldState, TextInputElement};

use crate::{icons::Icon, theme::ActiveTheme};

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
    pub(super) focus_handle: FocusHandle,
    pub(super) state: TextFieldState,
    pub(super) display_text: SharedString,
    pub(super) label: Option<SharedString>,
    pub(super) placeholder: Option<SharedString>,
    pub(super) icon: Option<Icon>,
    pub(super) preview: Option<SharedString>,
    pub(super) invalid: Option<SharedString>,
    pub(super) mono: bool,
    pub(super) height: Option<Pixels>,
    pub(super) hide_status_line: bool,
    pub(super) last_layout: Option<ShapedLine>,
    pub(super) last_bounds: Option<Bounds<Pixels>>,
    pub(super) last_scroll: Pixels,
}

impl TextInput {
    /// An empty field.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            state: TextFieldState::new(),
            display_text: SharedString::default(),
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
        self.display_text = self.state.shared_text();
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
        if self.preview != preview {
            self.preview = preview;
            cx.notify();
        }
    }

    /// Set the validation message. `Some` replaces the preview and reddens the border.
    pub fn set_invalid(&mut self, message: Option<SharedString>, cx: &mut Context<Self>) {
        if self.invalid != message {
            self.invalid = message;
            cx.notify();
        }
    }

    /// Whether the field is currently rejecting its value.
    pub fn is_invalid(&self) -> bool {
        self.invalid.is_some()
    }

    /// Notify and emit [`TextInputEvent::Changed`].
    fn emit_changed(&mut self, cx: &mut Context<Self>) {
        self.display_text = self.state.shared_text();
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    /// The control half of the edit set. Printable keys reach the field through the platform
    /// input handler, so consuming them here would insert every character twice.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match self.state.edit_keystroke(&event.keystroke) {
            EditEffect::Ignored => return,
            EditEffect::Unchanged => {}
            EditEffect::Changed => self.emit_changed(cx),
            EditEffect::CaretMoved => {
                cx.emit(TextInputEvent::CaretMoved);
                cx.notify();
            }
        }
        cx.stop_propagation();
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
            prefix: None,
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
