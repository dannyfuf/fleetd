//! `FilterBar` — the pane header, replaced in place.
//!
//! §3.10: 30 px, same row, no overlay, no reflow. The two-stage `Esc` (first leaves the input
//! keeping the filter, second clears it) and the retained `⌕rut` chip exist because a hidden
//! active filter is the classic "where did my rows go" bug. [D-15]: `Esc` in the Hub never
//! quits the app.
//!
//! The bar is the **editing** half of that pair: it is drawn only while the input owns the
//! keyboard, and the query it draws is a live [`TextInput`] the caller owns, so the whole
//! §3.10 edit set is the `FleetTextInput` table and the bar decodes no keys at all. Stage two
//! of the `Esc` is drawn by [`super::PaneHeader::filter_chip`], not here. `ctrl-n` / `↓` and
//! `ctrl-p` / `↑` never reach the editor — they move the **list** cursor while the user keeps
//! typing.
//!
//! The editor belongs in the header's 30 px row, so its owner builds it embedded
//! ([`TextInput::set_embedded`]): one line of text and a caret, with no box of its own.

use gpui::{App, Entity, Window, div, prelude::*};

use crate::{
    components::{KeyHint, TextInput},
    icons::{Icon, IconSize},
    text::{Text, TextRole, styled_with},
    theme::ActiveTheme,
    tone::Tone,
};

/// The in-place filter input.
#[derive(IntoElement)]
pub struct FilterBar {
    input: Entity<TextInput>,
    shown: usize,
    total: usize,
}

impl FilterBar {
    /// A filter bar over a live query editor and its match counts.
    pub fn new(input: Entity<TextInput>, shown: usize, total: usize) -> Self {
        Self {
            input,
            shown,
            total,
        }
    }

    /// The unframed query content for [`super::PaneHeader::query_slot`].
    /// The containing header owns padding, borders and match counts.
    pub fn query_slot(self) -> impl IntoElement {
        FilterQuery { input: self.input }
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
                    // Stage one of the two-stage `Esc`: leave the input, keep the filter. The
                    // bar exists only in that stage, so the hint is not a state of its own.
                    .child(KeyHint::labeled("esc", "leave")),
            )
    }
}

#[derive(IntoElement)]
struct FilterQuery {
    input: Entity<TextInput>,
}

impl RenderOnce for FilterQuery {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
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
            .child(
                styled_with(div(), TextRole::Ui.style(theme), theme)
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h(theme.text.ui.line_height)
                    .text_color(theme.colors.text)
                    .child(self.input),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::InputMode;

    fn bar(cx: &mut gpui::TestAppContext, shown: usize, total: usize) -> FilterBar {
        let input = cx.new(|cx| TextInput::new(InputMode::SingleLine, cx));
        FilterBar::new(input, shown, total)
    }

    #[gpui::test]
    fn a_filter_that_hides_everything_turns_amber(cx: &mut gpui::TestAppContext) {
        assert_eq!(bar(cx, 0, 12).count_tone(), Tone::Warning);
        assert_eq!(bar(cx, 2, 12).count_tone(), Tone::Muted);
    }

    #[gpui::test]
    fn empty_result_is_about_the_shown_count_only(cx: &mut gpui::TestAppContext) {
        assert!(bar(cx, 0, 0).is_empty_result());
        assert!(!bar(cx, 3, 3).is_empty_result());
    }

    #[gpui::test]
    fn empty_unfiltered_source_is_neutral(cx: &mut gpui::TestAppContext) {
        assert_eq!(bar(cx, 0, 0).count_tone(), Tone::Muted);
    }
}
