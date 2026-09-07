//! `FocusRing` — the only two places blue appears.
//!
//! §2.2: a 2 px inset ring on the focused pane, and a 2 px left bar on the cursor row.
//! Nothing else in Fleet is blue, so no component may draw the accent color as a border on
//! its own; it wraps itself in a `FocusRing` instead.

use gpui::{AnyElement, App, Window, div, prelude::*};

use crate::theme::ActiveTheme;

/// Which of the two rings to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusRingKind {
    /// 2 px inset ring around a pane.
    Pane,
    /// 2 px bar on the leading edge of a row.
    CursorRow,
}

/// Wraps a child and draws the focus affordance when `focused` is true.
#[derive(IntoElement)]
pub struct FocusRing {
    kind: FocusRingKind,
    focused: bool,
    child: Option<AnyElement>,
}

impl FocusRing {
    /// A pane ring.
    pub fn pane(focused: bool) -> Self {
        Self {
            kind: FocusRingKind::Pane,
            focused,
            child: None,
        }
    }

    /// A cursor-row bar.
    pub fn cursor_row(active: bool) -> Self {
        Self {
            kind: FocusRingKind::CursorRow,
            focused: active,
            child: None,
        }
    }

    /// The element the ring wraps.
    pub fn content(mut self, child: impl IntoElement) -> Self {
        self.child = Some(child.into_any_element());
        self
    }
}

impl RenderOnce for FocusRing {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let width = theme.metrics.focus_ring_w;
        let color = theme.colors.focus_ring;
        let bar = theme.colors.cursor_bar;
        let child = self.child.unwrap_or_else(|| div().into_any_element());

        match self.kind {
            FocusRingKind::Pane => div()
                .size_full()
                .border(width)
                .border_color(if self.focused {
                    color
                } else {
                    gpui::transparent_black()
                })
                .child(child),
            FocusRingKind::CursorRow => div()
                .flex()
                .flex_row()
                .size_full()
                .border_l(width)
                .border_color(if self.focused {
                    bar
                } else {
                    gpui::transparent_black()
                })
                .child(child),
        }
    }
}
