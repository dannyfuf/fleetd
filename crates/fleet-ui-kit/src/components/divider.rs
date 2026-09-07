//! `Divider` — a 1 px hairline. The only separator in the system.

use gpui::{App, Window, div, prelude::*};

use crate::theme::ActiveTheme;

/// Which way the hairline runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DividerAxis {
    /// A full-width rule.
    #[default]
    Horizontal,
    /// A full-height rule, used between split panes.
    Vertical,
}

/// A 1 px `border` hairline.
#[derive(IntoElement)]
pub struct Divider {
    axis: DividerAxis,
    inset: bool,
}

impl Divider {
    /// A full-width horizontal rule.
    pub fn horizontal() -> Self {
        Self {
            axis: DividerAxis::Horizontal,
            inset: false,
        }
    }

    /// A full-height vertical rule.
    pub fn vertical() -> Self {
        Self {
            axis: DividerAxis::Vertical,
            inset: false,
        }
    }

    /// Inset the rule by the pane padding, so it does not touch the pane border.
    pub fn inset(mut self, inset: bool) -> Self {
        self.inset = inset;
        self
    }
}

impl RenderOnce for Divider {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let pad = theme.space.lg;
        let rule = div().bg(theme.colors.border).size_full();
        match self.axis {
            DividerAxis::Horizontal => div()
                .flex_none()
                .h(theme.metrics.hairline)
                .w_full()
                .when(self.inset, |el| el.px(pad))
                .child(rule),
            DividerAxis::Vertical => div()
                .flex_none()
                .w(theme.metrics.hairline)
                .h_full()
                .when(self.inset, |el| el.py(pad))
                .child(rule),
        }
    }
}
