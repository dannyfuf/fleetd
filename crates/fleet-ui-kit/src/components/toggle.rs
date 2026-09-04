//! `Toggle` — `[x]` / `[ ]`, toggled with `Space`.
//!
//! Settings uses brackets rather than a switch because the whole dialog is a keyboard list and
//! a switch implies a pointer.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{text::Text, theme::ActiveTheme, tone::Tone};

/// A checkbox.
#[derive(IntoElement)]
pub struct Toggle {
    label: Option<SharedString>,
    checked: bool,
    focused: bool,
    disabled: bool,
    detail: Option<SharedString>,
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
}

impl RenderOnce for Toggle {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let box_text = if self.checked { "[x]" } else { "[ ]" };
        let tone = if self.disabled {
            Tone::Muted
        } else if self.checked {
            Tone::Default
        } else {
            Tone::Secondary
        };
        div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .h(theme.metrics.row_h)
            .w_full()
            .when(self.focused, |el| el.bg(theme.colors.row_selected))
            .child(Text::data(box_text).tone(tone))
            .children(self.label.map(|label| {
                Text::ui(label).tone(if self.disabled {
                    Tone::Muted
                } else {
                    Tone::Default
                })
            }))
            .children(self.detail.map(|detail| Text::ui(detail).faint()))
    }
}
