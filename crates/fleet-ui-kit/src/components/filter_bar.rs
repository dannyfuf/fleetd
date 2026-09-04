//! `FilterBar` — the pane header, replaced in place.
//!
//! §3.10: 30 px, same row, no overlay, no reflow. The two-stage `Esc` (first leaves the input
//! keeping the filter, second clears it) and the retained `⌕rut` chip exist because a hidden
//! active filter is the classic "where did my rows go" bug. [D-15]: `Esc` in the Hub never
//! quits the app.

use gpui::{App, SharedString, Window, div, prelude::*, px};

use crate::{
    components::KeyHint,
    icons::{Icon, IconSize},
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The in-place filter input.
#[derive(IntoElement)]
pub struct FilterBar {
    query: SharedString,
    shown: usize,
    total: usize,
    focused: bool,
}

impl FilterBar {
    /// A filter bar with a live query and its match counts.
    pub fn new(query: impl Into<SharedString>, shown: usize, total: usize) -> Self {
        Self {
            query: query.into(),
            shown,
            total,
            focused: true,
        }
    }

    /// Whether the input still has focus (first `Esc` sets this to false).
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Whether the filter matches nothing.
    pub fn is_empty_result(&self) -> bool {
        self.shown == 0
    }
}

impl RenderOnce for FilterBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let empty_result = self.is_empty_result();
        div()
            .flex()
            .items_center()
            .justify_between()
            .size_full()
            .px(theme.space.lg)
            .gap(theme.space.md)
            .border_b(px(1.0))
            .border_color(theme.colors.border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(theme.space.sm)
                    .child(
                        Icon::Search
                            .el()
                            .size(IconSize::Medium)
                            .color(theme.colors.text_secondary),
                    )
                    .child(Text::ui(self.query).ellipsize())
                    .children(self.focused.then(|| {
                        div()
                            .w(px(1.5))
                            .h(theme.text.ui.line_height)
                            .bg(theme.colors.accent)
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.md)
                    .child(
                        Text::label(format!("{}/{}", self.shown, self.total)).tone(
                            if empty_result {
                                Tone::Warning
                            } else {
                                Tone::Muted
                            },
                        ),
                    )
                    .child(KeyHint::new("esc")),
            )
    }
}
