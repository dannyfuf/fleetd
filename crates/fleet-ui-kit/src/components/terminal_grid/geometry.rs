//! Cell measurement and the row/column window a set of bounds can show.
//!
//! [`CellMetrics::measure`] shapes one reference glyph of the theme's mono face, so the column
//! width is the font's real advance and not a guess. Both axes are snapped to whole device
//! pixels, which is what keeps a full-screen grid from shimmering.

use gpui::{App, Bounds, Pixels, Window, px};

use super::{GridCell, GridRow, REFERENCE_GLYPH, cell_font};
use crate::theme::Theme;

/// The measured geometry of one terminal cell.
///
/// The width comes from the mono face's real advance, so a user who swaps `SF Mono` for
/// `Menlo` gets a grid that still lines up; the height is the design system's fixed
/// `data` line height, because a terminal's rows must match the rest of the app's rhythm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    /// One column.
    pub width: Pixels,
    /// One row.
    pub height: Pixels,
}

impl CellMetrics {
    /// Measure a cell by shaping the reference glyph in the theme's mono face.
    ///
    /// Falls back to `theme.metrics.cell_w` when the font system cannot report an advance
    /// (an unresolvable family), so a broken font configuration misaligns the grid instead of
    /// collapsing it to zero-width columns.
    pub fn measure(theme: &Theme, window: &Window, cx: &App) -> Self {
        let font_size = theme.text.data.size;
        let font_id = cx
            .text_system()
            .resolve_font(&cell_font(theme, false, false));
        let width = cx
            .text_system()
            .advance(font_id, font_size, REFERENCE_GLYPH)
            .map(|advance| advance.width)
            .unwrap_or(theme.metrics.cell_w);
        let snap = |value: Pixels| {
            let snapped = window.pixel_snap(value);
            if snapped > px(0.0) { snapped } else { value }
        };
        Self {
            width: snap(width),
            height: snap(theme.text.data.line_height),
        }
    }

    /// How many whole columns and rows fit in `area`.
    pub fn fit(self, area: gpui::Size<Pixels>) -> (usize, usize) {
        let cols = (f32::from(area.width) / f32::from(self.width)).floor();
        let rows = (f32::from(area.height) / f32::from(self.height)).floor();
        (cols.max(0.0) as usize, rows.max(0.0) as usize)
    }
}

/// The owning column and cell that occupy terminal column `col` of `row`.
pub(super) fn cell_at(row: Option<&GridRow>, col: usize) -> Option<(usize, &GridCell)> {
    let row = row?;
    let mut cursor = 0usize;
    for cell in &row.cells {
        let span = cell.width.columns() as usize;
        // A spacer occupies no column of its own: the wide grapheme before it already claimed
        // both, so looking one up must never land here.
        if span == 0 {
            continue;
        }
        if col >= cursor && col < cursor + span {
            return Some((cursor, cell));
        }
        cursor += span;
    }
    None
}

/// Clip rows before visiting cells. The range includes partially visible rows.
pub(super) fn visible_rows(
    bounds: Bounds<Pixels>,
    mask: Bounds<Pixels>,
    metrics: CellMetrics,
    count: usize,
) -> std::ops::Range<usize> {
    let visible = bounds.intersect(&mask);
    if visible.size.width <= px(0.0) || visible.size.height <= px(0.0) {
        return 0..0;
    }
    let first = ((visible.top() - bounds.top()) / metrics.height)
        .floor()
        .max(0.0) as usize;
    let end = ((visible.bottom() - bounds.top()) / metrics.height)
        .ceil()
        .max(0.0) as usize;
    first.min(count)..end.min(count)
}
