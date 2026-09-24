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
//!
//! A clear ✕ sits at the end of the query while it holds text (ADR 0023). It empties the
//! editor the bar was given, which is the same edit `ctrl-u` makes, so the owner hears it as an
//! ordinary change and the input keeps the keyboard: the two-stage `Esc` is untouched.

use gpui::{App, Entity, Window, div, prelude::*};

use crate::{
    components::{ButtonSize, IconButton, TextInput},
    harness::HarnessTargetExt,
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
                    .child(Text::label(format!("{shown}/{total}")).tone(count_tone)),
            )
    }
}

#[derive(IntoElement)]
struct FilterQuery {
    input: Entity<TextInput>,
}

impl RenderOnce for FilterQuery {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let has_query = !self.input.read(cx).text().is_empty();
        let clear = has_query.then(|| {
            let input = self.input.clone();
            IconButton::new("filter-clear", Icon::X, "Clear filter")
                .size(ButtonSize::Compact)
                .on_click(move |_, _, cx| input.update(cx, |input, cx| input.set_text("", cx)))
                .harness_target("filter.clear")
        });
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
            .children(clear)
    }
}

#[cfg(test)]
mod tests {
    use gpui::Render;

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

    struct ClearHost {
        input: Entity<TextInput>,
    }

    impl Render for ClearHost {
        fn render(&mut self, window: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            crate::harness::begin_frame(window);
            div()
                .size_full()
                .child(FilterBar::new(self.input.clone(), 1, 3))
        }
    }

    #[gpui::test]
    fn the_clear_button_empties_the_query_and_shows_only_with_text(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, point, px};

        crate::harness::set_recording(true);
        cx.update(|cx| cx.set_global(crate::Theme::dark()));
        let input = cx.new(|cx| {
            let mut input = TextInput::new(InputMode::SingleLine, cx);
            input.set_text("rut", cx);
            input
        });
        let window = cx.add_window(|_, _| ClearHost {
            input: input.clone(),
        });
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let clear = visual
            .update(|window, _| crate::harness::painted(window))
            .into_iter()
            .find(|target| target.name == "filter.clear")
            .unwrap_or_else(|| panic!("a query with text shows its clear button"))
            .rect;
        let at = point(px(clear.x + clear.w / 2.0), px(clear.y + clear.h / 2.0));
        visual.simulate_mouse_move(at, None, Modifiers::none());
        visual.simulate_click(at, Modifiers::none());
        visual.run_until_parked();
        input.read_with(&visual, |input, _| assert_eq!(input.text(), ""));

        visual.update(|window, cx| window.draw(cx).clear(cx));
        let names: Vec<String> = visual
            .update(|window, _| crate::harness::painted(window))
            .into_iter()
            .map(|target| target.name.to_string())
            .collect();
        crate::harness::set_recording(false);
        assert!(
            !names.iter().any(|name| name == "filter.clear"),
            "an empty query has nothing to clear"
        );
    }
}
