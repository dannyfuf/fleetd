//! `AppFrame` — context bar (36) + body (flex) + status bar (26), plus the overlay layer.
//!
//! §2.1: the Workspace replaces the body region entirely but keeps the two bars at the same
//! pixel positions, so the saccade never changes between screens. Overlays (dialogs, palette,
//! toasts, sheets) are children of the frame, not of the body, so they never reflow it.

use gpui::{AnyElement, App, Window, div, prelude::*};

use crate::theme::ActiveTheme;

/// The whole-window frame.
#[derive(IntoElement)]
pub struct AppFrame {
    context_bar: Option<AnyElement>,
    banner: Option<AnyElement>,
    body: Option<AnyElement>,
    status_bar: Option<AnyElement>,
    overlays: Vec<AnyElement>,
}

impl AppFrame {
    /// An empty frame.
    pub fn new() -> Self {
        Self {
            context_bar: None,
            banner: None,
            body: None,
            status_bar: None,
            overlays: Vec::new(),
        }
    }

    /// The 36 px top bar.
    pub fn context_bar(mut self, bar: impl IntoElement) -> Self {
        self.context_bar = Some(bar.into_any_element());
        self
    }

    /// A 28 px banner directly under the context bar (§3.12 case C).
    pub fn banner(mut self, banner: impl IntoElement) -> Self {
        self.banner = Some(banner.into_any_element());
        self
    }

    /// The flexible body: the Hub's panes, or the Workspace.
    pub fn body(mut self, body: impl IntoElement) -> Self {
        self.body = Some(body.into_any_element());
        self
    }

    /// The 26 px bottom bar.
    pub fn status_bar(mut self, bar: impl IntoElement) -> Self {
        self.status_bar = Some(bar.into_any_element());
        self
    }

    /// Add a floating layer. Order is paint order.
    pub fn overlay(mut self, overlay: impl IntoElement) -> Self {
        self.overlays.push(overlay.into_any_element());
        self
    }
}

impl Default for AppFrame {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for AppFrame {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.colors.bg)
            .text_color(theme.colors.text)
            .font_family(theme.font_ui.clone())
            .text_size(theme.text.ui.size)
            .line_height(theme.text.ui.line_height)
            .children(self.context_bar.map(|bar| {
                div()
                    .flex_none()
                    .h(theme.metrics.context_bar_h)
                    .w_full()
                    .child(bar)
            }))
            .children(self.banner.map(|banner| {
                div()
                    .flex_none()
                    .h(theme.metrics.banner_h)
                    .w_full()
                    .child(banner)
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .children(self.body),
            )
            .children(self.status_bar.map(|bar| {
                div()
                    .flex_none()
                    .h(theme.metrics.status_bar_h)
                    .w_full()
                    .child(bar)
            }))
            .children(self.overlays)
    }
}
