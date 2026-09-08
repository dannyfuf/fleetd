use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, Pixels, Point,
    SharedString, UTF16Selection, Window, WrappedLine, div, point, prelude::*,
};

use super::{
    MULTILINE_INPUT_KEY_CONTEXT, MultilineBuffer, MultilineInputEvent, PromptHistory,
    buffer::offset_from_utf16,
    element::{MultilineInputElement, visual_rows},
};

use crate::{
    text::{Text, TextRole, styled_with},
    theme::ActiveTheme,
};

/// The `❯` the canvas puts in front of the composer value.
const PROMPT_GLYPH: &str = "❯";

/// How tall the composer is allowed to grow, in lines (`NATIVE-AGENTS.md` §2). Past this the
/// block scrolls under the caret instead of pushing the transcript up.
const MAX_VISIBLE_LINES: usize = 8;

/// The native-agent composer: a wrapping, IME-aware, multi-line editor with prompt history.
///
/// It is [`crate::TextInput`]'s multi-line sibling. The editing model is [`MultilineBuffer`],
/// which is pure and unit-tested; this entity adds focus, keys, the clipboard, the platform
/// input handler and painting, and reports intent through [`MultilineInputEvent`] rather than
/// acting on the thread itself.
///
/// ```ignore
/// let composer = cx.new(|cx| {
///     MultilineInput::new(cx, "Message claude… (@ file · / command)".into())
/// });
/// cx.subscribe(&composer, |this, composer, event: &MultilineInputEvent, cx| match event {
///     MultilineInputEvent::Submit(text) => this.send(text.clone(), cx),
///     MultilineInputEvent::Trigger(ch) => this.open_completions(*ch, cx),
///     MultilineInputEvent::Escape => this.interrupt(cx),
/// })
/// .detach();
/// ```
pub struct MultilineInput {
    pub(super) focus_handle: FocusHandle,
    pub(super) placeholder: SharedString,
    pub(super) buffer: MultilineBuffer,
    pub(super) display_text: SharedString,
    pub(super) history: PromptHistory,
    pub(super) line_layout: Vec<WrappedLine>,
    pub(super) line_height: Pixels,
    pub(super) last_bounds: Option<Bounds<Pixels>>,
    pub(super) scroll: Pixels,
    goal_x: Option<Pixels>,
    /// Whether the composer draws the focus affordance while it holds the focus handle.
    focus_visible: bool,
}

impl MultilineInput {
    /// Creates an empty composer with its own focus handle.
    #[must_use]
    pub fn new(cx: &mut Context<Self>, placeholder: SharedString) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            placeholder,
            buffer: MultilineBuffer::new(),
            display_text: SharedString::default(),
            history: PromptHistory::new(),
            line_layout: Vec::new(),
            line_height: Pixels::ZERO,
            last_bounds: None,
            scroll: Pixels::ZERO,
            goal_x: None,
            focus_visible: true,
        }
    }

    /// Shows or hides the focus affordance without giving the focus handle away.
    ///
    /// DESIGN-SYSTEM §3 allows exactly two blue affordances and NATIVE-AGENTS §2 makes blue
    /// "where you are": while an open decision card owns the bare keys, the composer is not
    /// where the user is, even though it still holds the handle so typing a note works.
    pub fn set_focus_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.focus_visible != visible {
            self.focus_visible = visible;
            cx.notify();
        }
    }

    /// Returns the current composer text.
    #[must_use]
    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// Whether the composer holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// The editing model, for a caller that wants to drive or inspect it directly.
    #[must_use]
    pub fn buffer(&self) -> &MultilineBuffer {
        &self.buffer
    }

    /// The prompt history `↑` walks.
    #[must_use]
    pub fn history(&self) -> &PromptHistory {
        &self.history
    }

    /// Replaces the composer text.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.buffer.set_text(text);
        self.history.reset();
        self.goal_x = None;
        self.sync(cx);
    }

    /// Clears the composer text.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.buffer.clear();
        self.history.reset();
        self.goal_x = None;
        self.sync(cx);
    }

    /// Replaces the empty-value placeholder.
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            cx.notify();
        }
    }

    /// Returns the focus handle used by the composer element.
    #[must_use]
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Adds a submitted entry to history, ignoring blank and adjacent-duplicate entries.
    ///
    /// [`Self::submit`] already records what it emits; this is for an owner that queues or
    /// rewrites a prompt before sending it.
    pub fn push_history(&mut self, text: impl Into<String>) {
        self.history.push(text);
    }

    /// `⏎`: emit the current text and clear, unless it is blank.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.buffer.is_blank() {
            return;
        }
        let text = self.buffer.text().to_owned();
        self.buffer.clear();
        self.history.push(text.clone());
        self.goal_x = None;
        self.sync(cx);
        cx.emit(MultilineInputEvent::Submit(text));
    }

    /// The caret's position inside the shaped block, relative to the text origin.
    pub(super) fn position_for_offset(&self, offset: usize) -> Option<Point<Pixels>> {
        let mut top = Pixels::ZERO;
        let mut start = 0usize;
        for line in &self.line_layout {
            let end = start + line.text.len();
            if offset <= end {
                let local = line.position_for_index(offset - start, self.line_height)?;
                return Some(point(local.x, top + local.y));
            }
            top += self.line_height * (line.wrap_boundaries.len() + 1) as f32;
            start = end + '\n'.len_utf8();
        }
        None
    }

    /// The byte offset closest to a position inside the shaped block.
    pub(super) fn offset_for_position(&self, position: Point<Pixels>) -> Option<usize> {
        let mut top = Pixels::ZERO;
        let mut start = 0usize;
        let mut last = None;
        for line in &self.line_layout {
            let height = self.line_height * (line.wrap_boundaries.len() + 1) as f32;
            let end = start + line.text.len();
            if position.y < top + height {
                let local = point(position.x, position.y - top);
                let index = match line.closest_index_for_position(local, self.line_height) {
                    Ok(index) | Err(index) => index,
                };
                return Some(start + index.min(line.text.len()));
            }
            top += height;
            start = end + '\n'.len_utf8();
            last = Some(end);
        }
        last
    }

    /// The height every shaped line occupies together.
    pub(super) fn content_height(&self) -> Pixels {
        self.line_height * visual_rows(&self.line_layout).max(1) as f32
    }

    /// Refresh the shaped string and schedule a repaint.
    fn sync(&mut self, cx: &mut Context<Self>) {
        self.display_text = self.buffer.shared_text();
        cx.notify();
    }

    /// Run a text-changing edit: history navigation ends where typing begins.
    fn edit(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut MultilineBuffer)) -> bool {
        edit(&mut self.buffer);
        self.history.reset();
        self.goal_x = None;
        self.sync(cx);
        true
    }

    /// Run a caret motion, dropping the vertical goal column.
    fn motion(
        &mut self,
        cx: &mut Context<Self>,
        motion: impl FnOnce(&mut MultilineBuffer),
    ) -> bool {
        motion(&mut self.buffer);
        self.goal_x = None;
        self.sync(cx);
        true
    }

    /// Recalls the previous submitted prompt, whatever the caret is doing.
    ///
    /// An owner that binds `↑` to an action of its own consumes the key before the composer
    /// sees it (`NATIVE-AGENTS.md` §9 binds it to history in an agent tab), so the recall the
    /// composer owns is exposed as a call rather than only as a keystroke.
    pub fn recall_previous(&mut self, cx: &mut Context<Self>) -> bool {
        let current = self.buffer.text().to_owned();
        let Some(entry) = self.history.previous(&current) else {
            return false;
        };
        self.buffer.set_text(entry);
        self.goal_x = None;
        self.sync(cx);
        true
    }

    /// `↑` routed back from an owner that bound the key: the composer's own resolution of it.
    ///
    /// On macOS a keymap binding always wins over the input's key handler, so an agent tab that
    /// binds `↑` (to advance an open picker) would otherwise replace a `⇧⏎` multi-line draft
    /// with a recalled prompt instead of moving the caret up a line. Delegating here keeps the
    /// two `↑` behaviours identical whether or not an owner claimed the key.
    pub fn caret_up(&mut self, cx: &mut Context<Self>) -> bool {
        self.on_up(false, cx)
    }

    /// `↓` routed back from an owner that bound the key, for the same reason as
    /// [`Self::caret_up`]: an agent tab binds `↓` to move an open completion picker, and a
    /// draft with no picker over it must still move its caret down a row.
    pub fn caret_down(&mut self, cx: &mut Context<Self>) -> bool {
        self.on_down(false, cx)
    }

    /// `↑`: the previous prompt when the caret is parked at the top of an untouched buffer,
    /// otherwise one row up.
    fn on_up(&mut self, select: bool, cx: &mut Context<Self>) -> bool {
        if self.buffer.on_first_line() && (self.buffer.is_empty() || self.history.is_active()) {
            self.recall_previous(cx);
            return true;
        }
        self.vertical(false, select, cx)
    }

    /// `↓`: the next prompt while walking history, otherwise one row down.
    fn on_down(&mut self, select: bool, cx: &mut Context<Self>) -> bool {
        if self.history.is_active() && self.buffer.on_last_line() {
            if let Some(entry) = self.history.newer() {
                self.buffer.set_text(entry);
                self.goal_x = None;
                self.sync(cx);
            }
            return true;
        }
        self.vertical(true, select, cx)
    }

    /// One row up or down, visually when there is a layout to walk and logically otherwise.
    fn vertical(&mut self, down: bool, select: bool, cx: &mut Context<Self>) -> bool {
        match self.visual_row(down, select, self.goal_x) {
            Some(x) => self.goal_x = Some(x),
            None => {
                if down {
                    self.buffer.move_down(select);
                } else {
                    self.buffer.move_up(select);
                }
                self.goal_x = None;
            }
        }
        self.sync(cx);
        true
    }

    /// Move the caret one wrapped row, keeping `goal` as the column to land on.
    fn visual_row(&mut self, down: bool, select: bool, goal: Option<Pixels>) -> Option<Pixels> {
        if self.line_height <= Pixels::ZERO || self.line_layout.is_empty() {
            return None;
        }
        let position = self.position_for_offset(self.buffer.cursor())?;
        let x = goal.unwrap_or(position.x);
        let y = if down {
            position.y + self.line_height
        } else {
            position.y - self.line_height
        };
        if y < Pixels::ZERO || y >= self.content_height() {
            return None;
        }
        let offset = self.offset_for_position(point(x, y))?;
        self.buffer.move_to(offset, select);
        Some(x)
    }

    /// `⌘c`: put the selection on the clipboard.
    fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        let selected = self.buffer.selected_text();
        if selected.is_empty() {
            return false;
        }
        let text = selected.to_owned();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    /// `⌘x`: copy the selection, then delete it.
    fn cut(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.copy(cx) {
            return false;
        }
        self.edit(cx, |buffer| buffer.insert(""))
    }

    /// `⌘v`: insert the clipboard at the caret.
    fn paste(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return false;
        };
        if text.is_empty() {
            return false;
        }
        self.edit(cx, |buffer| buffer.insert(&text))
    }

    /// The control half of the edit set. Printable characters arrive through the platform input
    /// handler, so consuming them here would insert every character twice.
    ///
    /// Returns whether the composer owned the key; an unowned key keeps propagating so the
    /// `^s` prefix and the thread's own bindings still fire.
    fn handle_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let modifiers = &keystroke.modifiers;
        let plain = !modifiers.modified();
        let select = modifiers.shift;
        let shift = modifiers.shift
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.platform
            && !modifiers.function;
        let motion = plain || shift;
        let alt = modifiers.alt && !modifiers.control && !modifiers.platform && !modifiers.function;
        let command =
            modifiers.platform && !modifiers.control && !modifiers.alt && !modifiers.function;
        let control = modifiers.control
            && !modifiers.alt
            && !modifiers.shift
            && !modifiers.platform
            && !modifiers.function;

        match keystroke.key.as_str() {
            "enter" if plain => {
                self.submit(cx);
                true
            }
            "enter" if shift => self.edit(cx, MultilineBuffer::insert_newline),
            "escape" if plain => {
                cx.emit(MultilineInputEvent::Escape);
                true
            }
            "up" if motion => self.on_up(select, cx),
            "down" if motion => self.on_down(select, cx),
            "left" if motion => self.motion(cx, |buffer| {
                buffer.move_left(select);
            }),
            "right" if motion => self.motion(cx, |buffer| {
                buffer.move_right(select);
            }),
            "left" if alt => self.motion(cx, |buffer| {
                buffer.move_word_left(select);
            }),
            "right" if alt => self.motion(cx, |buffer| {
                buffer.move_word_right(select);
            }),
            "left" | "home" if command => self.motion(cx, |buffer| {
                buffer.move_to_line_start(select);
            }),
            "right" | "end" if command => self.motion(cx, |buffer| {
                buffer.move_to_line_end(select);
            }),
            "up" if command => self.motion(cx, |buffer| {
                buffer.move_to_start(select);
            }),
            "down" if command => self.motion(cx, |buffer| {
                buffer.move_to_end(select);
            }),
            "home" if motion => self.motion(cx, |buffer| {
                buffer.move_to_line_start(select);
            }),
            "end" if motion => self.motion(cx, |buffer| {
                buffer.move_to_line_end(select);
            }),
            "backspace" if plain => self.edit(cx, |buffer| {
                buffer.backspace();
            }),
            "backspace" if alt => self.edit(cx, |buffer| {
                buffer.delete_word_before();
            }),
            "backspace" if command => self.edit(cx, |buffer| {
                buffer.delete_to_line_start();
            }),
            "delete" if plain => self.edit(cx, |buffer| {
                buffer.delete_forward();
            }),
            "delete" if alt => self.edit(cx, |buffer| {
                buffer.delete_word_after();
            }),
            "delete" if command => self.edit(cx, |buffer| {
                buffer.delete_to_line_end();
            }),
            "a" if command => self.motion(cx, |buffer| {
                buffer.select_all();
            }),
            "c" if command => self.copy(cx),
            "x" if command => self.cut(cx),
            "v" if command => self.paste(cx),
            "a" if control => self.motion(cx, |buffer| {
                buffer.move_to_line_start(false);
            }),
            "e" if control => self.motion(cx, |buffer| {
                buffer.move_to_line_end(false);
            }),
            "b" if control => self.motion(cx, |buffer| {
                buffer.move_left(false);
            }),
            "f" if control => self.motion(cx, |buffer| {
                buffer.move_right(false);
            }),
            "w" if control => self.edit(cx, |buffer| {
                buffer.delete_word_before();
            }),
            "u" if control => self.edit(cx, |buffer| {
                buffer.delete_to_line_start();
            }),
            "k" if control => self.edit(cx, |buffer| {
                buffer.delete_to_line_end();
            }),
            "h" if control => self.edit(cx, |buffer| {
                buffer.backspace();
            }),
            "d" if control => self.edit(cx, |buffer| {
                buffer.delete_forward();
            }),
            _ => false,
        }
    }

    /// gpui's key-down listener: consume only what the composer owns.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.handle_keystroke(&event.keystroke, cx) {
            cx.stop_propagation();
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
        if let Some(bounds) = self.last_bounds {
            let position = point(
                event.position.x - bounds.left(),
                event.position.y - bounds.top() + self.scroll,
            );
            if let Some(offset) = self.offset_for_position(position) {
                self.buffer.set_cursor(offset);
            }
        }
        self.goal_x = None;
        cx.notify();
    }

    /// The byte range an IME edit applies to: the explicit range, else the marked range, else
    /// the platform selection.
    fn resolve_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        match range_utf16 {
            Some(range) => self.buffer.range_from_utf16(&range),
            None => self
                .buffer
                .marked_range()
                .unwrap_or_else(|| self.buffer.selected_range()),
        }
    }
}

impl EventEmitter<MultilineInputEvent> for MultilineInput {}

impl Focusable for MultilineInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MultilineInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let focused = self.focus_handle.is_focused(window) && self.focus_visible;
        let style = TextRole::Ui.style(&theme);
        let border = if focused {
            theme.colors.focus_ring
        } else {
            theme.colors.border
        };
        let glyph = if focused {
            theme.colors.text_secondary
        } else {
            theme.colors.text_muted
        };

        div()
            .key_context(MULTILINE_INPUT_KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .flex()
            .items_start()
            .gap(theme.space.sm)
            .w_full()
            // 8 + 18 + 8 + two hairlines is the canvas' 36 px minimum; the element grows the
            // box from there, one line at a time, up to `MAX_VISIBLE_LINES`.
            .min_h(theme.metrics.text_field_h)
            .px(theme.space.md)
            .py(theme.space.sm)
            .rounded(theme.radii.sm)
            .bg(theme.colors.bg)
            .border(theme.metrics.hairline)
            .border_color(border)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .h(style.line_height)
                    .child(Text::data(PROMPT_GLYPH).color(glyph)),
            )
            .child(
                // The value area carries the role's font so the shaped caret lines up with the
                // glyphs around it.
                styled_with(div(), style, &theme)
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .text_color(theme.colors.text)
                    .child(MultilineInputElement {
                        input: cx.entity(),
                        max_lines: MAX_VISIBLE_LINES,
                    }),
            )
    }
}

impl EntityInputHandler for MultilineInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        actual_range.replace(self.buffer.range_to_utf16(&range));
        self.buffer.text().get(range).map(str::to_string)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.range_to_utf16(&self.buffer.selected_range()),
            reversed: self.buffer.cursor() < self.buffer.anchor(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.buffer
            .marked_range()
            .map(|range| self.buffer.range_to_utf16(&range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.buffer.unmark();
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
        let trigger = self.buffer.trigger_for(range.start, text);
        self.buffer.replace_range(range, text);
        self.history.reset();
        self.goal_x = None;
        self.sync(cx);
        if let Some(trigger) = trigger {
            cx.emit(MultilineInputEvent::Trigger(trigger));
        }
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
        self.buffer.replace_and_mark(range, new_text);
        if let Some(selected) = new_selected_range_utf16 {
            let inserted = self.buffer.marked_range().unwrap_or_else(|| {
                let cursor = self.buffer.cursor();
                cursor..cursor
            });
            let insertion = self.buffer.text().get(inserted.clone()).unwrap_or("");
            let relative_start = offset_from_utf16(insertion, selected.start);
            let relative_end = offset_from_utf16(insertion, selected.end);
            self.buffer
                .set_selected_range(inserted.start + relative_start..inserted.start + relative_end);
        }
        self.history.reset();
        self.goal_x = None;
        self.sync(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        let start = self.position_for_offset(range.start)?;
        let end = self.position_for_offset(range.end)?;
        let origin = point(element_bounds.left(), element_bounds.top() - self.scroll);
        Some(Bounds::from_corners(
            point(origin.x + start.x, origin.y + start.y),
            point(origin.x + end.x, origin.y + end.y + self.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point_in_window: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let position = point(
            point_in_window.x - bounds.left(),
            point_in_window.y - bounds.top() + self.scroll,
        );
        let offset = self.offset_for_position(position)?;
        Some(self.buffer.offset_to_utf16(offset))
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.buffer.range_from_utf16(&range_utf16);
        self.buffer.set_selected_range(range);
        cx.notify();
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.buffer.len_utf16())
    }
}
