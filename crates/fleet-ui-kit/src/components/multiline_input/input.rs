use std::ops::Range;

use gpui::{
    App, Context, CursorStyle, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, SharedString, Subscription, Window, div, prelude::*,
};

use super::{
    MULTILINE_INPUT_KEY_CONTEXT, MultilineInputEvent, PromptHistory, Trigger,
    triggers::active_trigger,
};
use crate::{
    InputBuffer, InputMode, Text, TextInput, TextInputEvent, TextRole, theme::ActiveTheme,
};

const PROMPT_GLYPH: &str = "❯";
const MAX_VISIBLE_LINES: usize = 8;

/// The native-agent composer: shared multi-line editing plus history and completion reports.
pub struct MultilineInput {
    input: Entity<TextInput>,
    focus_handle: FocusHandle,
    history: PromptHistory,
    focus_visible: bool,
    read_only: bool,
    active_trigger_id: Option<(char, usize)>,
    _subscriptions: Vec<Subscription>,
}

impl MultilineInput {
    /// Create an empty composer with its own shared multi-line input.
    #[must_use]
    pub fn new(cx: &mut Context<Self>, placeholder: SharedString) -> Self {
        let input = cx.new(|cx| {
            let mut input = TextInput::new(
                InputMode::Multiline {
                    min_rows: 1,
                    max_rows: MAX_VISIBLE_LINES,
                },
                cx,
            );
            input.set_placeholder(placeholder, cx);
            input.set_enter_inserts_newline(false, cx);
            input.set_embedded(true, cx);
            input
        });
        let focus_handle = input.read(cx).focus_handle();
        let subscription = cx.subscribe(&input, |this, input, event, cx| {
            if !matches!(event, TextInputEvent::Changed) {
                return;
            }
            this.history.reset();
            let trigger = active_trigger(input.read(cx).buffer());
            let trigger_id = trigger.as_ref().map(|trigger| (trigger.symbol, trigger.at));
            if trigger_id != this.active_trigger_id
                && let Some(trigger) = trigger.filter(|trigger| trigger.query.is_empty())
            {
                cx.emit(MultilineInputEvent::Trigger(trigger));
            }
            this.active_trigger_id = trigger_id;
            cx.emit(MultilineInputEvent::Changed);
        });
        Self {
            input,
            focus_handle,
            history: PromptHistory::new(),
            focus_visible: true,
            read_only: false,
            active_trigger_id: None,
            _subscriptions: vec![subscription],
        }
    }

    /// Show or hide the focus affordance without giving the focus handle away.
    pub fn set_focus_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.focus_visible != visible {
            self.focus_visible = visible;
            cx.notify();
        }
    }

    /// Prevent user edits and submit events while preserving the current draft.
    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only != read_only {
            self.read_only = read_only;
            self.input
                .update(cx, |input, cx| input.set_read_only(read_only, cx));
            cx.notify();
        }
    }

    /// Whether user interaction can mutate this composer.
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Return the current composer text.
    #[must_use]
    pub fn text<'a>(&'a self, cx: &'a App) -> &'a str {
        self.input.read(cx).text()
    }

    /// Whether the composer holds nothing.
    #[must_use]
    pub fn is_empty(&self, cx: &App) -> bool {
        self.input.read(cx).is_empty()
    }

    /// Whether the platform input method currently owns marked text.
    #[must_use]
    pub fn is_composing(&self, cx: &App) -> bool {
        self.input.read(cx).is_composing()
    }

    /// The shared editing engine, for read-only inspection by an owner.
    #[must_use]
    pub fn buffer<'a>(&'a self, cx: &'a App) -> &'a InputBuffer {
        self.input.read(cx).buffer()
    }

    /// The prompt history `↑` walks.
    #[must_use]
    pub const fn history(&self) -> &PromptHistory {
        &self.history
    }

    /// The completion surface containing the caret, including its current query.
    #[must_use]
    pub fn active_trigger(&self, cx: &App) -> Option<Trigger> {
        active_trigger(self.input.read(cx).buffer())
    }

    /// Replace the composer text without reporting a user edit.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_text_silent(text, cx));
        self.history.reset();
        self.sync_trigger_id(cx);
    }

    /// Clear the composer text without reporting a user edit.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_text_silent("", cx));
        self.history.reset();
        self.active_trigger_id = None;
    }

    /// Replace the empty-value placeholder.
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.input
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
    }

    /// Return the focus handle used by the inner input.
    #[must_use]
    pub const fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Add a submitted entry to history, ignoring blanks and adjacent duplicates.
    pub fn push_history(&mut self, text: impl Into<String>) {
        self.history.push(text);
    }

    /// Emit the current text and clear it, unless it is blank or composing.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.read_only || self.is_composing(cx) {
            return;
        }
        let text = self.text(cx).to_owned();
        if text.trim().is_empty() {
            return;
        }
        self.input
            .update(cx, |input, cx| input.set_text_silent("", cx));
        self.history.push(text.clone());
        self.active_trigger_id = None;
        cx.emit(MultilineInputEvent::Submit(text));
    }

    /// Forward a marked-text update to the shared input's platform bridge.
    ///
    /// Normal platform input reaches the inner entity directly; this forwarding surface keeps
    /// deterministic owner tests able to model a live composition through the composer API.
    pub fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(
                range_utf16,
                new_text,
                new_selected_range_utf16,
                window,
                cx,
            );
        });
    }

    /// Recall the previous submitted prompt.
    pub fn recall_previous(&mut self, cx: &mut Context<Self>) -> bool {
        let current = self.text(cx).to_owned();
        let Some(entry) = self.history.previous(&current) else {
            return false;
        };
        self.input
            .update(cx, |input, cx| input.set_text_silent(entry, cx));
        self.sync_trigger_id(cx);
        true
    }

    /// Route `↑` back through the composer's history/visual-row policy.
    pub fn caret_up(&mut self, cx: &mut Context<Self>) -> bool {
        if self.recall_allowed(cx)
            && self.input.read(cx).on_first_visual_row()
            && (self.input.read(cx).is_empty() || self.history.is_active())
        {
            self.recall_previous(cx);
            return true;
        }
        self.input
            .update(cx, |input, cx| input.move_vertical(false, false, cx))
    }

    /// Route `↓` back through the composer's history/visual-row policy.
    pub fn caret_down(&mut self, cx: &mut Context<Self>) -> bool {
        if self.recall_allowed(cx)
            && self.history.is_active()
            && self.input.read(cx).on_last_visual_row()
        {
            if let Some(entry) = self.history.newer() {
                self.input
                    .update(cx, |input, cx| input.set_text_silent(entry, cx));
                self.sync_trigger_id(cx);
            }
            return true;
        }
        self.input
            .update(cx, |input, cx| input.move_vertical(true, false, cx))
    }

    fn recall_allowed(&self, cx: &App) -> bool {
        !self.input.read(cx).is_composing()
    }

    fn sync_trigger_id(&mut self, cx: &App) {
        self.active_trigger_id = active_trigger(self.input.read(cx).buffer())
            .map(|trigger| (trigger.symbol, trigger.at));
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            cx.stop_propagation();
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.modified() {
            return;
        }
        let handled = match event.keystroke.key.as_str() {
            "enter" => {
                self.submit(cx);
                true
            }
            "escape" => {
                cx.emit(MultilineInputEvent::Escape);
                true
            }
            "up" => self.caret_up(cx),
            "down" => self.caret_down(cx),
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    #[cfg(test)]
    pub(super) fn inner(&self) -> Entity<TextInput> {
        self.input.clone()
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
        let theme = cx.theme();
        let focused = self.focus_handle.is_focused(window) && self.focus_visible;
        let style = TextRole::Ui.style(theme);
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
            .on_key_down(cx.listener(Self::on_key_down))
            .cursor(if self.read_only {
                CursorStyle::Arrow
            } else {
                CursorStyle::IBeam
            })
            .flex()
            .items_start()
            .gap(theme.space.sm)
            .w_full()
            .min_h(theme.metrics.text_field_h)
            .px(theme.space.md)
            .py(theme.space.sm)
            .rounded(theme.radii.sm)
            .bg(theme.colors.bg)
            .border(theme.metrics.hairline)
            .border_color(border)
            .when(self.read_only, |element| {
                element.opacity(theme.metrics.dimmed_opacity)
            })
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .h(style.line_height)
                    .child(Text::data(PROMPT_GLYPH).color(glyph)),
            )
            .child(self.input.clone())
    }
}
