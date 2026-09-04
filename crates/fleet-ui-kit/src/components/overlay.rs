//! `Overlay` — a centered, top-anchored floating layer.
//!
//! §3.9: the palette sits at y = 120, the "thinking position", not at screen center. The
//! overlay is `deferred` so it paints above the body regardless of sibling order, and it does
//! not ghost the base — only [`super::Dialog`] does that.

use gpui::{AnyElement, App, Pixels, Window, deferred, div, prelude::*};

use crate::theme::ActiveTheme;

/// A floating layer anchored from the top of the window.
#[derive(IntoElement)]
pub struct Overlay {
    top: Option<Pixels>,
    width: Option<Pixels>,
    scrim: bool,
    child: Option<AnyElement>,
}

impl Overlay {
    /// A layer at the default y = 120 with the default 640 px width.
    pub fn new() -> Self {
        Self {
            top: None,
            width: None,
            scrim: false,
            child: None,
        }
    }

    /// Override the distance from the top of the window.
    pub fn top(mut self, top: Pixels) -> Self {
        self.top = Some(top);
        self
    }

    /// Override the card width.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Ghost the base screen behind the layer. Off for the palette, on for dialogs.
    pub fn scrim(mut self, scrim: bool) -> Self {
        self.scrim = scrim;
        self
    }

    /// The card.
    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.child = Some(child.into_any_element());
        self
    }
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for Overlay {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let top = self.top.unwrap_or(theme.metrics.palette_top);
        let width = self.width.unwrap_or(theme.metrics.palette_w);
        deferred(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .when(self.scrim, |el| el.bg(theme.colors.overlay))
                .child(
                    div()
                        .mt(top)
                        .w(width)
                        .rounded(theme.radii.lg)
                        .bg(theme.colors.elevated)
                        .border_1()
                        .border_color(theme.colors.border_strong)
                        .shadow(theme.dialog_shadow())
                        .overflow_hidden()
                        .children(self.child),
                ),
        )
    }
}
