//! `Overlay` — a centered, top-anchored floating layer, and the paint order every floating
//! surface in Fleet shares.
//!
//! §3.9: the palette sits at y = 120, the "thinking position", not at screen center. The
//! overlay is `deferred` so it paints above the body regardless of sibling order, and it does
//! **not** ghost the base — only [`super::Dialog`] does that, because only a dialog is a
//! decision. An overlay is a jump.

use gpui::{AnyElement, App, Pixels, Window, deferred, div, prelude::*};

use crate::theme::ActiveTheme;

/// Paint order of the four floating surfaces.
///
/// gpui draws `deferred` elements after the rest of the frame, ordered by priority, so the
/// four layers that can be on screen at the same time state their order **here, once**,
/// instead of each one inventing a number. Higher paints later, i.e. on top.
///
/// The order encodes three UX-spec rules:
///
/// 1. A [`super::Sheet`] is lowest, because it is *about* the rows behind it (§3.7).
/// 2. A [`super::Dialog`] is above a palette [`Overlay`], because a dialog ghosts the base
///    screen and a palette does not (§3.8, §3.9).
/// 3. The [`super::ToastStack`] is highest, because §2.7 allows a toast while a dialog is
///    open (the "dialog-close reassurance" row) and an unreadable acknowledgement is worse
///    than none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OverlayLayer {
    /// The right-docked panel.
    Sheet,
    /// A top-anchored card: the command palette.
    #[default]
    Anchored,
    /// A centered modal with a scrim.
    Dialog,
    /// The bottom-right toast stack.
    Toast,
}

impl OverlayLayer {
    /// The `deferred` priority this layer paints at.
    pub const fn priority(self) -> usize {
        match self {
            OverlayLayer::Sheet => 100,
            OverlayLayer::Anchored => 200,
            OverlayLayer::Dialog => 300,
            OverlayLayer::Toast => 400,
        }
    }
}

/// A floating layer anchored from the top of the window.
#[derive(IntoElement)]
pub struct Overlay {
    top: Option<Pixels>,
    width: Option<Pixels>,
    scrim: bool,
    layer: OverlayLayer,
    child: Option<AnyElement>,
}

impl Overlay {
    /// A layer at the default y = 120 with the default 640 px width.
    pub fn new() -> Self {
        Self {
            top: None,
            width: None,
            scrim: false,
            layer: OverlayLayer::Anchored,
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
    ///
    /// A scrim also makes the layer swallow pointer events, so a click outside the card can
    /// never reach the frozen screen behind it.
    pub fn scrim(mut self, scrim: bool) -> Self {
        self.scrim = scrim;
        self
    }

    /// Paint at another layer's priority. The default is [`OverlayLayer::Anchored`].
    pub fn layer(mut self, layer: OverlayLayer) -> Self {
        self.layer = layer;
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
        let scrim = self.scrim;
        deferred(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .when(scrim, |el| el.bg(theme.colors.overlay).occlude())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .mt(top)
                        .mb(top)
                        .w(width)
                        .min_h_0()
                        .rounded(theme.radii.lg)
                        .bg(theme.colors.elevated)
                        .border_1()
                        .border_color(theme.colors.border_strong)
                        .shadow(theme.dialog_shadow())
                        .overflow_hidden()
                        .occlude()
                        .children(self.child),
                ),
        )
        .with_priority(self.layer.priority())
    }
}
