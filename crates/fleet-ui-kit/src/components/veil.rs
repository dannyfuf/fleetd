//! `Veil` — a 55 % scrim over terminal grids only, with key dropping.
//!
//! §3.12 case C: while the daemon is gone, lists stay at 100 % and stay navigable — they are
//! true, just frozen — but a terminal grid is a live surface, so it is veiled and keys typed
//! into it are **dropped, not buffered**. The component renders the scrim; the caller is
//! responsible for actually dropping the keys, and [`Veil::drops_keys`] states that contract.

use gpui::{AnyElement, App, Window, div, prelude::*};

use crate::theme::ActiveTheme;

/// A scrim over a live surface.
#[derive(IntoElement)]
pub struct Veil {
    active: bool,
    opacity: f32,
    child: Option<AnyElement>,
}

impl Veil {
    /// A veil over `child`.
    pub fn new(active: bool) -> Self {
        Self {
            active,
            opacity: 0.55,
            child: None,
        }
    }

    /// Override the dim factor.
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    /// The surface being veiled.
    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.child = Some(child.into_any_element());
        self
    }

    /// Whether the caller must drop keystrokes for this surface. Always true while active.
    pub fn drops_keys(&self) -> bool {
        self.active
    }
}

impl RenderOnce for Veil {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let bg = cx.theme().colors.bg;
        div()
            .relative()
            .size_full()
            .children(self.child)
            .when(self.active, |el| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(bg)
                        .opacity(self.opacity)
                        .occlude(),
                )
            })
    }
}
