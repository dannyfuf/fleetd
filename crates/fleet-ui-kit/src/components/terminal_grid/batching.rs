//! Turning rows of cells into a flat display list, and retaining it across frames.
//!
//! The grid is **not** a tree of `div`s — one element per cell (or even per row) is far too
//! much layout for a 200 x 60 terminal at 60 fps. It is a single [`gpui::canvas`] that owns the
//! whole cell area and replays a display list built here, following Zed's `TerminalElement`
//! (`docs/research/gpui.md` §4.8):
//!
//! 1. **Batch.** Runs of adjacent cells with the same style become one string; blanks,
//!    invisible cells, wide graphemes and style changes flush the batch.
//! 2. **Merge.** Cell backgrounds are collected as horizontal runs and then coalesced
//!    vertically, so a full-width selection or a `bat` header is one quad, not 200.
//!
//! [`TerminalGridCache`] keeps the batched content of unchanged rows, so an overlay-only update
//! (cursor move, selection drag) re-batches nothing.

use std::{cell::RefCell, rc::Rc, sync::Arc};

use gpui::{FontFeatures, FontStyle, FontWeight, Hsla, Pixels, SharedString, TextRun, font};

use super::{CellMetrics, CellWidth, CursorShape, GridCell, GridRow};
use crate::theme::Theme;

/// The mono [`gpui::Font`] a cell of the given weight and slant is shaped with.
///
/// Ligatures are disabled: a terminal that renders `!=` as `≠` has silently moved the column
/// grid, which is the one thing a mirror grid may never do.
pub(super) fn cell_font(theme: &Theme, bold: bool, italic: bool) -> gpui::Font {
    let mut face = font(theme.font_mono.clone());
    static FEATURES: std::sync::OnceLock<FontFeatures> = std::sync::OnceLock::new();
    face.features = FEATURES
        .get_or_init(FontFeatures::disable_ligatures)
        .clone();
    face.weight = if bold {
        FontWeight::BOLD
    } else {
        theme.text.data.weight
    };
    face.style = if italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    face
}

/// A merged rectangle of equal-colored cells, in cell coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CellRect {
    pub(super) row: usize,
    pub(super) rows: usize,
    pub(super) col: usize,
    pub(super) cols: usize,
    pub(super) color: Hsla,
}

/// One shaped run of same-styled cells.
pub(super) struct TextBatch {
    pub(super) row: usize,
    pub(super) col: usize,
    /// How many terminal **columns** the batch covers. `run.len` counts utf-8 bytes instead,
    /// and the two are only equal for pure ASCII.
    pub(super) cells: usize,
    pub(super) text: SharedString,
    pub(super) run: TextRun,
}

/// Everything the paint pass replays, computed once in prepaint.
pub(super) struct GridLayout {
    pub(super) metrics: CellMetrics,
    pub(super) font_size: Pixels,
    pub(super) content: Rc<GridContent>,
    pub(super) selection: Vec<CellRect>,
    pub(super) cursor: Option<CursorLayout>,
}

/// The cursor quad plus the glyph that has to be redrawn on top of it.
pub(super) struct CursorLayout {
    pub(super) row: usize,
    pub(super) col: usize,
    pub(super) cols: usize,
    pub(super) shape: CursorShape,
    pub(super) hollow: bool,
    pub(super) color: Hsla,
    pub(super) glyph: Option<(SharedString, TextRun)>,
}

/// Collect the horizontal background runs of one row.
pub(super) fn row_backgrounds(
    row_ix: usize,
    row: &GridRow,
    theme: &Theme,
    out: &mut Vec<CellRect>,
) {
    let mut col = 0usize;
    let mut open: Option<CellRect> = None;
    for cell in &row.cells {
        let span = cell.width.columns() as usize;
        if span == 0 {
            // A `CellWidth::Spacer` is the trailing half of the wide cell before it: it owns
            // no column of its own, so it must neither open, extend nor close a run. Counting
            // it as one column would paint a third column for a two-column grapheme and make
            // that rectangle overlap the next cell.
            continue;
        }
        let (_, bg) = cell.resolve(theme);
        match (bg, open.as_mut()) {
            (Some(color), Some(rect)) if rect.color == color && rect.col + rect.cols == col => {
                rect.cols += span;
            }
            (Some(color), _) => {
                if let Some(rect) = open.take() {
                    out.push(rect);
                }
                open = Some(CellRect {
                    row: row_ix,
                    rows: 1,
                    col,
                    cols: span,
                    color,
                });
            }
            (None, _) => {
                if let Some(rect) = open.take() {
                    out.push(rect);
                }
            }
        }
        col += span;
    }
    if let Some(rect) = open.take() {
        out.push(rect);
    }
}

/// Coalesce vertically adjacent rectangles that share a column span and a color.
///
/// Rows arrive in order, so one pass over the accumulated rectangles is enough: a full-width
/// `bat` background or a multi-line selection collapses from one quad per row to one quad.
pub(super) fn merge_vertically(rects: Vec<CellRect>) -> Vec<CellRect> {
    let mut merged: Vec<CellRect> = Vec::with_capacity(rects.len());
    for rect in rects {
        let joined = merged.iter_mut().rev().take(64).find(|candidate| {
            candidate.col == rect.col
                && candidate.cols == rect.cols
                && candidate.color == rect.color
                && candidate.row + candidate.rows == rect.row
        });
        match joined {
            Some(candidate) => candidate.rows += rect.rows,
            None => merged.push(rect),
        }
    }
    merged
}

/// Build the text batches of one row.
///
/// A batch is extended while the next cell is narrow, contiguous, and styled identically.
/// Everything else — a blank, a spacer, a style change, a column gap, and every wide grapheme
/// — closes it.
pub(super) fn row_text_batches<'row>(
    row_ix: usize,
    row: &'row GridRow,
    theme: &Theme,
    out: &mut Vec<TextBatch>,
) {
    let mut col = 0usize;
    let mut open: Option<(TextBatch, &'row GridCell)> = None;
    let mut buffer = String::new();
    for cell in &row.cells {
        let span = cell.width.columns() as usize;
        if !cell.paints_glyph() {
            if let Some((batch, _)) = open.take() {
                out.push(finish_batch(batch, &mut buffer));
            }
            col += span;
            continue;
        }
        // A wide grapheme is always its own batch: `force_width` positions one glyph per
        // column, so letting a two-column grapheme share a run would pull every glyph after
        // it one cell to the left.
        let can_append = cell.width == CellWidth::Narrow
            && match open.as_ref() {
                Some((batch, style)) => {
                    style.width == CellWidth::Narrow
                        && style.same_style(cell)
                        && batch.col + batch.cells == col
                }
                None => false,
            };
        if can_append && let Some((batch, _)) = open.as_mut() {
            buffer.push_str(cell.glyph_text());
            batch.run.len = buffer.len();
            batch.cells += span;
            col += span;
            continue;
        }
        if let Some((batch, _)) = open.take() {
            out.push(finish_batch(batch, &mut buffer));
        }
        buffer.push_str(cell.glyph_text());
        let run = cell.text_run(theme, buffer.len());
        open = Some((
            TextBatch {
                row: row_ix,
                col,
                cells: span,
                text: SharedString::default(),
                run,
            },
            cell,
        ));
        col += span;
    }
    if let Some((batch, _)) = open.take() {
        out.push(finish_batch(batch, &mut buffer));
    }
}

fn finish_batch(mut batch: TextBatch, buffer: &mut String) -> TextBatch {
    batch.text = SharedString::new(buffer.as_str());
    buffer.clear();
    batch
}

pub(super) struct GridContent {
    pub(super) backgrounds: Vec<CellRect>,
    pub(super) text: Vec<TextBatch>,
}

struct CachedContent {
    rows: Arc<[GridRow]>,
    theme: Theme,
    visible: std::ops::Range<usize>,
    content: Rc<GridContent>,
}

/// Retained terminal batches. Rows, theme and visible row range invalidate content;
/// geometry is applied in paint, while cursor and selection always remain separate.
#[derive(Clone, Default)]
pub struct TerminalGridCache(Rc<RefCell<Option<CachedContent>>>);

impl TerminalGridCache {
    pub(super) fn content(
        &self,
        rows: &Arc<[GridRow]>,
        theme: &Theme,
        visible: std::ops::Range<usize>,
    ) -> Rc<GridContent> {
        let mut cached = self.0.borrow_mut();
        if let Some(previous) = cached.as_ref()
            && Arc::ptr_eq(&previous.rows, rows)
            && previous.theme == *theme
            && previous.visible == visible
        {
            return previous.content.clone();
        }
        let mut backgrounds = Vec::new();
        let mut text = Vec::new();
        for row_ix in visible.clone() {
            row_backgrounds(row_ix, &rows[row_ix], theme, &mut backgrounds);
            row_text_batches(row_ix, &rows[row_ix], theme, &mut text);
        }
        let content = Rc::new(GridContent {
            backgrounds: merge_vertically(backgrounds),
            text,
        });
        *cached = Some(CachedContent {
            rows: rows.clone(),
            theme: theme.clone(),
            visible,
            content: content.clone(),
        });
        content
    }
}
