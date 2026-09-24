//! `StatusBar` — the 28 px bottom row: where you are and what is running.
//!
//! Left to right:
//!
//! | Slot | Width | Content |
//! | --- | --- | --- |
//! | daemon | intrinsic | a small dot in the daemon's tone and `fleetd`, plus its state word when it is not healthy |
//! | breadcrumb | flex, truncate | `context › repo › row` |
//! | job ticker | intrinsic, muted | `⟳ <kind> <target> <pct>` with `+n` |
//! | sticky error | intrinsic, red | `⚠ <text> · !`, persists until dismissed |
//! | trailing | intrinsic | ghost buttons: `Shortcuts ?`, and in the Workspace `Fleet commands ⌃S` |
//!
//! The error **replaces** the ticker rather than joining it: an error outranks progress, and both
//! want the same place. There is no mode word (ADR 0023): the state that changes what keys do is
//! on the surface that has it — the scroll pill, the filter bar, the open overlay, the ⌃S menu.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{
    components::{DaemonState, StatusDot},
    text::Text,
    theme::ActiveTheme,
    truncate::Truncate,
};

/// The bottom bar.
#[derive(IntoElement)]
pub struct StatusBar {
    daemon: Option<(DaemonState, Option<SharedString>)>,
    breadcrumb: Option<SharedString>,
    breadcrumb_budget: Option<usize>,
    ticker: Option<AnyElement>,
    error: Option<AnyElement>,
    trailing: Vec<AnyElement>,
}

impl StatusBar {
    /// An empty bar.
    pub fn new() -> Self {
        Self {
            daemon: None,
            breadcrumb: None,
            breadcrumb_budget: None,
            ticker: None,
            error: None,
            trailing: Vec::new(),
        }
    }

    /// The daemon's liveness: a dot and `fleetd`, followed by `word` (`unreachable`, `starting`)
    /// when the daemon is not healthy. A healthy daemon never shows a word.
    pub fn daemon(mut self, state: DaemonState, word: Option<SharedString>) -> Self {
        let word = state
            .is_labelled()
            .then(|| word.or_else(|| state.word().map(SharedString::new_static)))
            .flatten();
        self.daemon = Some((state, word));
        self
    }

    /// `context › repo › row`. Ellipsizes at the end of the flex slot by default; give it a
    /// `ch` budget with [`StatusBar::breadcrumb_ch`] to truncate in the middle instead, which
    /// keeps both the outermost context and the innermost row readable.
    pub fn breadcrumb(mut self, breadcrumb: impl Into<SharedString>) -> Self {
        self.breadcrumb = Some(breadcrumb.into());
        self
    }

    /// Truncate the breadcrumb in the middle at a `ch` budget.
    pub fn breadcrumb_ch(mut self, budget: usize) -> Self {
        self.breadcrumb_budget = Some(budget);
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

    /// Append a right-aligned element, normally a compact ghost [`super::Button`].
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing.push(trailing.into_any_element());
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
        let daemon = self.daemon.map(|(state, word)| {
            let tone = state.tone();
            let name = match word {
                Some(word) => SharedString::from(format!("fleetd {word}")),
                None => SharedString::new_static("fleetd"),
            };
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(theme.space.xs)
                .child(StatusDot::small(tone))
                .child(if state.is_labelled() {
                    Text::ui(name).tone(tone)
                } else {
                    Text::ui(name).muted()
                })
        });

        div()
            .flex()
            .items_center()
            .size_full()
            .px(theme.space.md)
            .gap(theme.space.lg)
            .bg(theme.colors.chrome)
            .border_t(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .children(daemon)
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
            .child(
                div()
                    .flex()
                    .flex_none()
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
