//! `SkeletonRows` — 30 % placeholder rows. Cold load only.
//!
//! §3.5: used exactly once, for a cold PR fetch. Everything else in Fleet renders from
//! `state.json` immediately and never shows a skeleton.

use gpui::{App, Pixels, Window, div, prelude::*};

use crate::theme::ActiveTheme;

/// N placeholder rows.
#[derive(IntoElement)]
pub struct SkeletonRows {
    count: usize,
    row_height: Option<Pixels>,
}

impl SkeletonRows {
    /// `count` rows at the standard row height.
    pub fn new(count: usize) -> Self {
        Self {
            count,
            row_height: None,
        }
    }

    /// Override the row height (job rows are 44 px).
    pub fn row_height(mut self, height: Pixels) -> Self {
        self.row_height = Some(height);
        self
    }
}

impl RenderOnce for SkeletonRows {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let height = self.row_height.unwrap_or(theme.metrics.row_h);
        let color = theme.colors.skeleton;
        div().flex().flex_col().w_full().children(
            (0..self.count)
                .map(|ix| {
                    let width = 0.55 + ((ix % 4) as f32) * 0.1;
                    div()
                        .h(height)
                        .flex()
                        .items_center()
                        .px(theme.space.md)
                        .child(
                            div()
                                .h(gpui::px(10.0))
                                .w(gpui::relative(width))
                                .rounded(theme.radii.xs)
                                .bg(color)
                                .opacity(0.3),
                        )
                })
                .collect::<Vec<_>>(),
        )
    }
}
