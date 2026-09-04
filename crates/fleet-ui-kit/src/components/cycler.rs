//! `Cycler` — `◂ value ▸`, changed with `←` / `→`.
//!
//! Used for the host selector and every Settings choice. It is a cycler and not a dropdown
//! because the option sets are two to five items long and a dropdown would cost a second key.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// A left/right value cycler.
#[derive(IntoElement)]
pub struct Cycler {
    label: Option<SharedString>,
    value: SharedString,
    has_prev: bool,
    has_next: bool,
    focused: bool,
}

impl Cycler {
    /// A cycler showing `value`.
    pub fn new(value: impl Into<SharedString>) -> Self {
        Self {
            label: None,
            value: value.into(),
            has_prev: true,
            has_next: true,
            focused: false,
        }
    }

    /// A labelled cycler.
    pub fn labeled(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self::new(value).label(label)
    }

    /// Set the label.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Whether `←` does anything. Off dims the arrow.
    pub fn has_prev(mut self, has_prev: bool) -> Self {
        self.has_prev = has_prev;
        self
    }

    /// Whether `→` does anything.
    pub fn has_next(mut self, has_next: bool) -> Self {
        self.has_next = has_next;
        self
    }

    /// Whether the row carries the cursor.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }
}

impl RenderOnce for Cycler {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let arrow = |enabled: bool, icon: Icon| {
            icon.el().size(IconSize::Small).color(if enabled {
                theme.colors.text_secondary
            } else {
                theme.colors.text_muted
            })
        };
        div()
            .flex()
            .items_center()
            .gap(theme.space.sm)
            .h(theme.metrics.row_h)
            .when(self.focused, |el| el.bg(theme.colors.row_selected))
            .children(self.label.map(|label| Text::ui(label).muted()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.space.xs)
                    .child(arrow(self.has_prev, Icon::ChevronLeft))
                    .child(Text::ui(self.value).tone(Tone::Default))
                    .child(arrow(self.has_next, Icon::ChevronRight)),
            )
    }
}
