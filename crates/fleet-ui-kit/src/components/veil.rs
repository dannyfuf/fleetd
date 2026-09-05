//! `Veil` — a 55 % scrim over **terminal grids only**, while the daemon is gone.
//!
//! §3.12 case C draws a sharp line: lists stay at 100 % opacity and stay navigable — they are
//! *true*, just frozen — while a terminal grid is a live surface whose contents stopped being
//! true the moment the daemon died. Only the live surface is veiled.
//!
//! Keys typed into a veiled grid are **dropped, not buffered**. Replaying a buffer into a
//! restarted shell would run commands the user typed at a different prompt, which is the
//! failure mode this rule exists to prevent. The component renders the scrim and blocks the
//! pointer; the caller must honour the key contract, and [`Veil::drops_keys`] states it.
//!
//! ## States
//!
//! inactive (renders the child untouched, zero cost) · active (scrim + pointer block).

use gpui::{AnyElement, App, Window, div, prelude::*};

use crate::theme::ActiveTheme;

/// A scrim over a live surface.
#[derive(IntoElement)]
pub struct Veil {
    active: bool,
    opacity: Option<f32>,
    child: Option<AnyElement>,
}

impl Veil {
    /// A veil over `child`.
    pub fn new(active: bool) -> Self {
        Self {
            active,
            opacity: None,
            child: None,
        }
    }

    /// Override the dim factor.
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = Some(opacity.clamp(0.0, 1.0));
        self
    }

    /// The surface being veiled.
    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.child = Some(child.into_any_element());
        self
    }

    /// Whether the caller must drop keystrokes for this surface. True exactly while the veil
    /// is active.
    pub fn drops_keys(&self) -> bool {
        self.active
    }
}

impl RenderOnce for Veil {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let ground = theme.colors.bg;
        let opacity = self.opacity.unwrap_or(theme.metrics.veil_opacity);
        div()
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .children(self.child)
            .when(self.active, |el| {
                el.child(
                    // The scrim is an alpha *fill*, not an element opacity: an opacity layer
                    // would also fade anything a caller stacks above the veil.
                    div()
                        .absolute()
                        .inset_0()
                        .bg(ground.opacity(opacity))
                        .occlude(),
                )
            })
    }
}
