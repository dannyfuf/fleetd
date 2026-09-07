//! `FilterBar` — the pane header, replaced in place.
//!
//! §3.10: 30 px, same row, no overlay, no reflow. The two-stage `Esc` (first leaves the input
//! keeping the filter, second clears it) and the retained `⌕rut` chip exist because a hidden
//! active filter is the classic "where did my rows go" bug. [D-15]: `Esc` in the Hub never
//! quits the app.
//!
//! The bar is presentational: the caller owns the query and the caret and feeds keystrokes to a
//! [`super::TextFieldState`], which implements the whole §3.10 edit set (printable, `Backspace`,
//! `ctrl-w`, `ctrl-u`). `ctrl-n` / `↓` and `ctrl-p` / `↑` never reach the bar at all — they move
//! the **list** cursor while the user keeps typing.

use gpui::{App, SharedString, Window, div, prelude::*};

use crate::{
    components::KeyHint,
    components::text_field::FieldLine,
    icons::{Icon, IconSize},
    text::{Text, TextRole, styled_with},
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
    caret: Option<usize>,
    placeholder: Option<SharedString>,
}

impl FilterBar {
    /// A filter bar with a live query and its match counts.
    pub fn new(query: impl Into<SharedString>, shown: usize, total: usize) -> Self {
        Self {
            query: query.into(),
            shown,
            total,
            focused: true,
            caret: None,
            placeholder: None,
        }
    }

    /// Whether the input still has focus (first `Esc` sets this to false).
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Caret position in characters. Defaults to the end of the query.
    pub fn caret(mut self, caret: usize) -> Self {
        self.caret = Some(caret);
        self
    }

    /// What to show while the query is still empty, e.g. `filter worktrees`.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// The unframed query content for [`super::PaneHeader::query_slot`].
    /// The containing header owns padding, borders and match counts.
    pub fn query_slot(self) -> impl IntoElement {
        FilterQuery {
            query: self.query,
            placeholder: self.placeholder,
            caret: self.caret,
            focused: self.focused,
        }
    }

    /// Whether the filter matches nothing.
    pub fn is_empty_result(&self) -> bool {
        self.shown == 0
    }

    /// The tone of the `shown/total` counter: amber when the filter hides everything, so the
    /// count itself says "your rows did not vanish, they were filtered out".
    pub fn count_tone(&self) -> Tone {
        if self.total > 0 && self.is_empty_result() {
            Tone::Warning
        } else {
            Tone::Muted
        }
    }
}

impl RenderOnce for FilterBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let count_tone = self.count_tone();
        let focused = self.focused;
        let shown = self.shown;
        let total = self.total;
        let query_area = self.query_slot();

        div()
            .flex()
            .items_center()
            .justify_between()
            .size_full()
            .h(theme.metrics.pane_header_h)
            .px(theme.space.lg)
            .gap(theme.space.md)
            .bg(theme.colors.surface)
            .border_b(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(query_area)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.md)
                    .child(Text::label(format!("{shown}/{total}")).tone(count_tone))
                    // Stage one of the two-stage `Esc`: leave the input, keep the filter.
                    // Stage two, from the retained chip, clears it.
                    .child(KeyHint::labeled(
                        "esc",
                        if focused { "leave" } else { "clear" },
                    )),
            )
    }
}

#[derive(IntoElement)]
struct FilterQuery {
    query: SharedString,
    placeholder: Option<SharedString>,
    caret: Option<usize>,
    focused: bool,
}

impl RenderOnce for FilterQuery {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            query,
            placeholder,
            caret,
            focused,
        } = self;
        let theme = cx.theme();
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(theme.space.sm)
            .child(Icon::Search.el().size(IconSize::Medium).color(if focused {
                theme.colors.text_secondary
            } else {
                theme.colors.text_muted
            }))
            .child(
                styled_with(div(), TextRole::Ui.style(theme), theme)
                    .flex_1()
                    .min_w_0()
                    .h(theme.text.ui.line_height)
                    .text_color(theme.colors.text)
                    .child(FieldLine::new(query, placeholder, caret, focused)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filter_that_hides_everything_turns_amber() {
        assert_eq!(FilterBar::new("rut", 0, 12).count_tone(), Tone::Warning);
        assert_eq!(FilterBar::new("rut", 2, 12).count_tone(), Tone::Muted);
    }

    #[test]
    fn empty_result_is_about_the_shown_count_only() {
        assert!(FilterBar::new("", 0, 0).is_empty_result());
        assert!(!FilterBar::new("", 3, 3).is_empty_result());
    }

    #[test]
    fn empty_unfiltered_source_is_neutral() {
        assert_eq!(FilterBar::new("", 0, 0).count_tone(), Tone::Muted);
    }
}
