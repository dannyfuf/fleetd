//! `StatusBar` — breadcrumb · mode word · job ticker · sticky error slot.
//!
//! §2.2. The error slot replaces the ticker when present, because an error outranks progress
//! and because both live in the same 26 px row.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*, px};

use crate::{
    components::{Mode, ModeWord},
    text::Text,
    theme::ActiveTheme,
};

/// The bottom bar.
#[derive(IntoElement)]
pub struct StatusBar {
    breadcrumb: Option<SharedString>,
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
            mode: None,
            ticker: None,
            error: None,
            trailing: None,
        }
    }

    /// `context › repo › row`, truncated in the middle.
    pub fn breadcrumb(mut self, breadcrumb: impl Into<SharedString>) -> Self {
        self.breadcrumb = Some(breadcrumb.into());
        self
    }

    /// The mode word. Mandatory on every screen.
    pub fn mode(mut self, mode: Mode) -> Self {
        self.mode = Some(ModeWord::new(mode).into_any_element());
        self
    }

    /// A custom mode element.
    pub fn mode_element(mut self, mode: impl IntoElement) -> Self {
        self.mode = Some(mode.into_any_element());
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
        div()
            .flex()
            .items_center()
            .size_full()
            .px(theme.space.md)
            .gap(theme.space.md)
            .bg(theme.colors.bg)
            .border_t(px(1.0))
            .border_color(theme.colors.border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .children(self.breadcrumb.map(|b| Text::ui(b).muted().ellipsize())),
            )
            .children(self.mode)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .justify_end()
                    .gap(theme.space.sm)
                    .when(!has_error, |el| el.children(self.ticker))
                    .children(self.error)
                    .children(self.trailing),
            )
    }
}
