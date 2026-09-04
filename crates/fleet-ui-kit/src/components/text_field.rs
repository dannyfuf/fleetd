//! `TextField` — a single-line input with a blue caret and a zero-shift validation line.
//!
//! **Minimal render.** The component is presentational: the caller owns the string and the
//! caret index and handles keys (printable, `Backspace`, `ctrl-w`, `ctrl-u`, `ctrl-a`,
//! `ctrl-e`, `←`/`→` — KEYMAP, §3.8). That split is deliberate: gpui `RenderOnce` components
//! cannot own state, and the dialogs already own theirs.
//!
//! The one rule the component *does* enforce is §3.8.1's: the validation line **replaces** the
//! preview line in the same box, so a failing branch name causes zero layout shift.

use gpui::{App, SharedString, Window, div, prelude::*, px};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// A single-line text input.
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

    /// Whether the field is currently rejecting its value.
    pub fn is_invalid(&self) -> bool {
        self.invalid.is_some()
    }
}

impl RenderOnce for TextField {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let empty = self.value.is_empty();
        let shown = if empty {
            self.placeholder.clone().unwrap_or_default()
        } else {
            self.value.clone()
        };
        let border = if self.invalid.is_some() {
            theme.colors.danger
        } else if self.focused {
            theme.colors.focus_ring
        } else {
            theme.colors.border
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
                    .h(px(36.0))
                    .w_full()
                    .px(theme.space.md)
                    .rounded(theme.radii.sm)
                    .bg(theme.colors.bg)
                    .border_1()
                    .border_color(border)
                    .children(self.icon.map(|icon| {
                        icon.el()
                            .size(IconSize::Medium)
                            .color(theme.colors.text_secondary)
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .child(if self.mono {
                                Text::data(shown).tone(if empty {
                                    Tone::Muted
                                } else {
                                    Tone::Default
                                })
                            } else {
                                Text::ui(shown).tone(if empty {
                                    Tone::Muted
                                } else {
                                    Tone::Default
                                })
                            }),
                    )
                    .children(self.focused.then(|| {
                        div()
                            .w(px(1.5))
                            .h(theme.text.ui.line_height)
                            .bg(theme.colors.accent)
                    })),
            )
            .child(
                div()
                    .h(px(18.0))
                    .flex()
                    .items_center()
                    .child(match self.invalid {
                        Some(message) => Text::hint(message).tone(Tone::Danger),
                        None => Text::hint(self.preview.unwrap_or_default()).faint(),
                    }),
            )
    }
}
