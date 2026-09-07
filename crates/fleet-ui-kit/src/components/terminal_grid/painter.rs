//! Replaying the display list onto the canvas.
//!
//! Backgrounds first, then the selection overlay, then one
//! [`gpui::WindowTextSystem::shape_line`] per batch with `force_width = cell_width` — the
//! monospace-grid trick that locks every base glyph onto its column regardless of the advance
//! the shaper would otherwise pick — and finally the cursor.

use gpui::{App, Bounds, ContentMask, Pixels, TextAlign, Window, fill, outline, point, size};

use super::{CellMetrics, CellRect, CursorLayout, CursorShape, GridLayout};
use crate::theme::ActiveTheme;

/// Replay the display list. Backgrounds, then selection, then text, then the cursor.
pub(super) fn paint_grid(
    bounds: Bounds<Pixels>,
    layout: GridLayout,
    window: &mut Window,
    cx: &mut App,
) {
    let metrics = layout.metrics;
    let origin = bounds.origin;

    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for rect in &layout.content.backgrounds {
            window.paint_quad(fill(
                cell_rect_bounds(rect, origin, metrics, window),
                rect.color,
            ));
        }
        for rect in &layout.selection {
            window.paint_quad(fill(
                cell_rect_bounds(rect, origin, metrics, window),
                rect.color,
            ));
        }
        for batch in &layout.content.text {
            let position = point(
                origin.x + metrics.width * (batch.col as f32),
                origin.y + metrics.height * (batch.row as f32),
            );
            let shaped = window.text_system().shape_line(
                batch.text.clone(),
                layout.font_size,
                std::slice::from_ref(&batch.run),
                Some(metrics.width),
            );
            // A shaping failure is a font problem, not a program error: the cell stays blank
            // rather than taking the window down mid-frame.
            if let Err(error) =
                shaped.paint(position, metrics.height, TextAlign::Left, None, window, cx)
            {
                crate::paint_error::log_once(&error);
            }
        }
        if let Some(cursor) = &layout.cursor {
            paint_cursor(cursor, origin, metrics, layout.font_size, window, cx);
        }
    });
}

/// Resolve cell coordinates by snapping their shared edges, not each rectangle's width.
pub(super) fn cell_rect_bounds(
    rect: &CellRect,
    origin: gpui::Point<Pixels>,
    metrics: CellMetrics,
    window: &Window,
) -> Bounds<Pixels> {
    let left = window.pixel_snap(origin.x + metrics.width * rect.col as f32);
    let top = window.pixel_snap(origin.y + metrics.height * rect.row as f32);
    let right = window.pixel_snap(origin.x + metrics.width * (rect.col + rect.cols) as f32);
    let bottom = window.pixel_snap(origin.y + metrics.height * (rect.row + rect.rows) as f32);
    Bounds::from_corners(point(left, top), point(right, bottom))
}

/// Paint the cursor quad and, under a filled block, the glyph it covers.
pub(super) fn paint_cursor(
    cursor: &CursorLayout,
    origin: gpui::Point<Pixels>,
    metrics: CellMetrics,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let left = origin.x + metrics.width * (cursor.col as f32);
    let top = origin.y + metrics.height * (cursor.row as f32);
    let block_w = metrics.width * (cursor.cols as f32);
    let bar_w = cx.theme().metrics.focus_ring_w;
    let quad_bounds = match cursor.shape {
        CursorShape::Bar => Bounds::new(point(left, top), size(bar_w, metrics.height)),
        CursorShape::Underline => Bounds::new(
            point(left, top + metrics.height - bar_w),
            size(block_w, bar_w),
        ),
        CursorShape::Block | CursorShape::Hollow => {
            Bounds::new(point(left, top), size(block_w, metrics.height))
        }
    };
    let quad_bounds = window.pixel_snap_bounds(quad_bounds);
    if cursor.hollow {
        // An unfocused terminal keeps the cursor findable without claiming the keyboard.
        window.paint_quad(outline(quad_bounds, cursor.color, gpui::BorderStyle::Solid));
        return;
    }
    window.paint_quad(fill(quad_bounds, cursor.color));
    if let Some((text, run)) = &cursor.glyph {
        let shaped = window.text_system().shape_line(
            text.clone(),
            font_size,
            std::slice::from_ref(run),
            Some(metrics.width),
        );
        if let Err(error) = shaped.paint(
            point(left, top),
            metrics.height,
            TextAlign::Left,
            None,
            window,
            cx,
        ) {
            crate::paint_error::log_once(&error);
        }
    }
}
