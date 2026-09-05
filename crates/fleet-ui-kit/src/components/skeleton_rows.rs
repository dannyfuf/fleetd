//! `SkeletonRows` — 30 % placeholder rows. Cold load only.
//!
//! §3.5: used exactly once, for a cold PR fetch. Everything else in Fleet renders from
//! `state.json` immediately and never shows a skeleton — a placeholder where cached truth
//! exists is a lie, and it costs the user the one thing the cache was for.

use gpui::{App, Pixels, Window, div, prelude::*, relative};

use crate::theme::ActiveTheme;

/// The four widths the bars cycle through, as a fraction of the row, so a block of them reads
/// as a list of rows rather than as a progress bar.
const BAR_WIDTHS: [f32; 4] = [0.55, 0.65, 0.75, 0.85];

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
        // A bar is one line of body text tall, so the block occupies exactly the space the real
        // rows will and nothing shifts when they arrive.
        let bar_h = theme.text.ui.size;
        let pad = theme.space.md;
        let radius = theme.radii.xs;
        div().flex().flex_col().w_full().children(
            (0..self.count)
                .map(|ix| {
                    let width = BAR_WIDTHS[ix % BAR_WIDTHS.len()];
                    div().h(height).flex().items_center().px(pad).child(
                        div()
                            .h(bar_h)
                            .w(relative(width))
                            .rounded(radius)
                            .bg(color)
                            .opacity(theme.metrics.skeleton_opacity),
                    )
                })
                .collect::<Vec<_>>(),
        )
    }
}
