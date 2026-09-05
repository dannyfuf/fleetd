//! `AppFrame` — the fixed chrome of every Fleet window.
//!
//! §2.1 / §2.2 pin four heights and never move them:
//!
//! | Region | Height | Token |
//! | --- | --- | --- |
//! | context bar | 36 px | `metrics.context_bar_h` |
//! | banner (§3.12 case C, optional) | 28 px | `metrics.banner_h` |
//! | body | flexible | — |
//! | status bar | 26 px | `metrics.status_bar_h` |
//!
//! The Workspace replaces the body region entirely but keeps the two bars at the same pixel
//! positions, so the saccade never changes between screens — that is why the heights live on
//! the frame and not on the bars themselves, and why the body is the only flexible band.
//!
//! ## Two overlay layers, not one
//!
//! Floating surfaces are children of the **frame**, never of the body, so opening one never
//! reflows what the user was looking at. They come in two flavours, because §2.2 places them
//! differently:
//!
//! - [`AppFrame::overlay`] spans the whole window: a [`super::Dialog`] ghosts the base screen
//!   including both bars, and a palette [`super::Overlay`] is anchored at y = 120 from the
//!   window's top.
//! - [`AppFrame::body_overlay`] spans only the band between the bars: the Jobs
//!   [`super::Sheet`] is "full height between the context bar and the status bar" (§3.7), and
//!   the [`super::ToastStack`] sits "bottom-right, **above the status bar**" (§2.2).
//!
//! Both layers paint in [`super::OverlayLayer`] order, so a toast is legible over a dialog and
//! a dialog covers a sheet no matter which slot they were passed to.

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
    body_overlays: Vec<AnyElement>,
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
            body_overlays: Vec::new(),
        }
    }

    /// The 36 px top bar.
    pub fn context_bar(mut self, bar: impl IntoElement) -> Self {
        self.context_bar = Some(bar.into_any_element());
        self
    }

    /// A 28 px banner directly under the context bar (§3.12 case C).
    ///
    /// The banner pushes the body down rather than floating over it: it is a *state* of the
    /// window, not a layer, and a terminal that keeps its rows under a floating strip would
    /// hide the line the user is reading.
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

    /// Add a window-wide floating layer: a [`super::Dialog`] or a palette
    /// [`super::Overlay`]. Order within a layer is paint order.
    pub fn overlay(mut self, overlay: impl IntoElement) -> Self {
        self.overlays.push(overlay.into_any_element());
        self
    }

    /// Add a floating layer clipped to the band between the two bars: the Jobs
    /// [`super::Sheet`] and the [`super::ToastStack`].
    pub fn body_overlay(mut self, overlay: impl IntoElement) -> Self {
        self.body_overlays.push(overlay.into_any_element());
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
            .overflow_hidden()
            .bg(theme.colors.bg)
            .text_color(theme.colors.text)
            .font_family(theme.font_ui.clone())
            .text_size(theme.text.ui.size)
            .line_height(theme.text.ui.line_height)
            .children(self.context_bar.map(|bar| {
                div()
                    .flex()
                    .flex_none()
                    .h(theme.metrics.context_bar_h)
                    .w_full()
                    .overflow_hidden()
                    .child(bar)
            }))
            .children(self.banner.map(|banner| {
                div()
                    .flex()
                    .flex_none()
                    .h(theme.metrics.banner_h)
                    .w_full()
                    .overflow_hidden()
                    .child(banner)
            }))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .children(
                        self.body
                            .map(|body| div().flex().size_full().min_w_0().min_h_0().child(body)),
                    )
                    .children(self.body_overlays),
            )
            .children(self.status_bar.map(|bar| {
                div()
                    .flex()
                    .flex_none()
                    .h(theme.metrics.status_bar_h)
                    .w_full()
                    .overflow_hidden()
                    .child(bar)
            }))
            .children(self.overlays)
    }
}
