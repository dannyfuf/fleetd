//! `Toggle` — `[x]` / `[ ]`, toggled with `Space`.
//!
//! Settings uses brackets rather than a switch because the whole dialog is a keyboard list and
//! a switch implies a pointer. The brackets are drawn in the data face so a column of them
//! lines up on the same monospace grid as every other value in the dialog.

use gpui::{App, Pixels, SharedString, Window, div, prelude::*};

use crate::{components::FocusRing, text::Text, theme::ActiveTheme, tone::Tone};

/// A checkbox.
#[derive(IntoElement)]
pub struct Toggle {
    label: Option<SharedString>,
    checked: bool,
    focused: bool,
    disabled: bool,
    detail: Option<SharedString>,
    label_width: Option<Pixels>,
}

impl Toggle {
    /// A toggle.
    pub fn new(checked: bool) -> Self {
        Self {
            label: None,
            checked,
            focused: false,
            disabled: false,
            detail: None,
            label_width: None,
        }
    }

    /// A labelled toggle.
    pub fn labeled(label: impl Into<SharedString>, checked: bool) -> Self {
        Self::new(checked).label(label)
    }

    /// Set the label.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Fix the label column so a stack of settings rows aligns on one gutter.
    pub fn label_width(mut self, width: Pixels) -> Self {
        self.label_width = Some(width);
        self
    }

    /// A trailing muted detail, e.g. a live match count.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Whether the row carries the settings cursor.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Whether the toggle can be changed here.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The literal box, so a test can assert the state without rendering.
    pub fn box_text(&self) -> &'static str {
        if self.checked { "[x]" } else { "[ ]" }
    }

    /// The tone of the box: a checked box is a value, an unchecked one is chrome.
    pub fn box_tone(&self) -> Tone {
        if self.disabled {
            Tone::Muted
        } else if self.checked {
            Tone::Default
        } else {
            Tone::Secondary
        }
    }
}

impl RenderOnce for Toggle {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let box_text = self.box_text();
        let box_tone = self.box_tone();
        let disabled = self.disabled;
        let focused = self.focused && !disabled;

        let body = div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .h_full()
            .w_full()
            .px(theme.space.md)
            .child(Text::data(box_text).tone(box_tone))
            .children(self.label.map(|label| {
                let text = Text::ui(label).tone(if disabled { Tone::Muted } else { Tone::Default });
                match self.label_width {
                    Some(width) => text.w(width),
                    None => text.ellipsize(),
                }
            }))
            .children(
                self.detail
                    .map(|detail| Text::ui(detail).faint().ellipsize()),
            );

        div()
            .w_full()
            .h(theme.metrics.row_h)
            .when(focused, |el| el.bg(theme.colors.row_selected))
            .when(disabled, |el| el.opacity(0.4))
            .child(FocusRing::cursor_row(focused).child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_states_the_value() {
        assert_eq!(Toggle::new(true).box_text(), "[x]");
        assert_eq!(Toggle::new(false).box_text(), "[ ]");
    }

    #[test]
    fn a_disabled_toggle_never_reaches_full_contrast() {
        assert_eq!(Toggle::new(true).disabled(true).box_tone(), Tone::Muted);
        assert_eq!(Toggle::new(true).box_tone(), Tone::Default);
        assert_eq!(Toggle::new(false).box_tone(), Tone::Secondary);
    }
}
