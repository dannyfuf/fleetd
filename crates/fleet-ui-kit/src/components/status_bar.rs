//! `StatusBar` — breadcrumb · mode word · job ticker · sticky error slot.
//!
//! §2.2 fixes four slots in one 26 px row, left to right:
//!
//! | Slot | Width | Content |
//! | --- | --- | --- |
//! | breadcrumb | flex, truncate-**middle** | `context › repo › row` |
//! | mode word | fixed 84 px, centered | §2.8, mandatory on every screen |
//! | job ticker | flex, muted | `⟳ <kind> <target> <pct>` with `+n` |
//! | sticky error | right, red | `⚠ <text> · !`, persists until dismissed |
//!
//! The error **replaces** the ticker rather than joining it: an error outranks progress, both
//! want the same half of the row, and a row that grows a fifth slot under pressure is a row
//! whose mode word stops being centered — which is the one thing §2.8 does not allow.
//!
//! The two flex halves are equal, which is what keeps the 84 px word optically centered on
//! every screen including the Workspace and including zoom.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{
    components::{Mode, ModeWord},
    text::Text,
    theme::ActiveTheme,
    truncate::Truncate,
};

/// The bottom bar.
#[derive(IntoElement)]
pub struct StatusBar {
    breadcrumb: Option<SharedString>,
    breadcrumb_budget: Option<usize>,
    mode: Option<AnyElement>,
    ticker: Option<AnyElement>,
    error: Option<AnyElement>,
    trailing: Option<AnyElement>,
}

impl StatusBar {
    /// An empty bar.
    pub fn new() -> Self {
        Self {
            breadcrumb: None,
            breadcrumb_budget: None,
            mode: None,
            ticker: None,
            error: None,
            trailing: None,
        }
    }

    /// `context › repo › row`. Ellipsizes at the end of the flex slot by default; give it a
    /// `ch` budget with [`StatusBar::breadcrumb_ch`] to get §2.2's middle truncation, which
    /// keeps both the outermost context and the innermost row readable.
    pub fn breadcrumb(mut self, breadcrumb: impl Into<SharedString>) -> Self {
        self.breadcrumb = Some(breadcrumb.into());
        self
    }

    /// Truncate the breadcrumb in the middle at a `ch` budget (§2.2).
    pub fn breadcrumb_ch(mut self, budget: usize) -> Self {
        self.breadcrumb_budget = Some(budget);
        self
    }

    /// The mode word. Mandatory on every screen (§2.8).
    pub fn mode(mut self, mode: Mode) -> Self {
        self.mode = Some(ModeWord::new(mode).into_any_element());
        self
    }

    /// The job ticker. Hidden while an error is present.
    pub fn ticker(mut self, ticker: impl IntoElement) -> Self {
        self.ticker = Some(ticker.into_any_element());
        self
    }

    /// The sticky error slot. Outranks the ticker.
    pub fn error(mut self, error: impl IntoElement) -> Self {
        self.error = Some(error.into_any_element());
        self
    }

    /// An extra right-aligned element, e.g. the Workspace's daemon dot.
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for StatusBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let has_error = self.error.is_some();
        let budget = self.breadcrumb_budget;

        div()
            .flex()
            .items_center()
            .size_full()
            .px(theme.space.md)
            .gap(theme.space.md)
            .bg(theme.colors.bg)
            .border_t(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .overflow_hidden()
                    .children(self.breadcrumb.map(|b| {
                        let text = Text::ui(b).muted();
                        match budget {
                            Some(budget) => text.truncate_at(budget, Truncate::Middle),
                            None => text.ellipsize(),
                        }
                    })),
            )
            .children(self.mode)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .justify_end()
                    .overflow_hidden()
                    .gap(theme.space.sm)
                    .when(!has_error, |el| el.children(self.ticker))
                    .children(self.error)
                    .children(self.trailing),
            )
    }
}
