//! `TerminalGrid` — the mirror cell grid.
//!
//! The element itself is thin: it measures with [`geometry`], batches with [`batching`], and
//! paints with [`painter`]; [`model`] defines the cell vocabulary and carries the redraw
//! contract the caller must honour. The caller owns frame updates, focus and resize
//! deduplication.

use std::sync::Arc;

use gpui::{App, Bounds, ElementId, Pixels, Window, canvas, div, prelude::*};

use crate::{
    components::{ScrollPill, ScrollbackBadge, TerminalMode},
    theme::ActiveTheme,
};

mod batching;
mod geometry;
mod model;
mod painter;

pub use batching::TerminalGridCache;
use batching::*;
pub use geometry::CellMetrics;
use geometry::*;
pub use model::{
    CellWidth, CursorShape, GridCell, GridCursor, GridRow, GridSelection, UnderlineStyle,
};
use painter::paint_grid;

const REFERENCE_GLYPH: char = 'M';

/// The painted cell grid.
#[derive(IntoElement)]
pub struct TerminalGrid {
    id: Option<ElementId>,
    rows: Arc<[GridRow]>,
    cache: TerminalGridCache,
    cursor: Option<GridCursor>,
    selection: Option<GridSelection>,
    focused: bool,
    padding: Option<Pixels>,
    scrollback: Option<(usize, usize)>,
    scroll_pill: Option<ScrollPill>,
    modes: Vec<TerminalMode>,
    frame_size: Option<(usize, usize)>,
    dimmed: bool,
    #[allow(clippy::type_complexity)]
    on_resize: Option<Box<dyn Fn(usize, usize, &mut Window, &mut App) + 'static>>,
    #[allow(clippy::type_complexity)]
    on_geometry: Option<Box<dyn Fn(Bounds<Pixels>, CellMetrics) + 'static>>,
}

impl TerminalGrid {
    /// Compose immutable rows. An `Arc<[GridRow]>` snapshot is taken as is, without copying
    /// cells or collecting again.
    pub fn from_shared(rows: impl Into<Arc<[GridRow]>>) -> Self {
        let rows = rows.into();
        Self {
            id: None,
            rows,
            cache: TerminalGridCache::default(),
            cursor: None,
            selection: None,
            focused: true,
            padding: None,
            scrollback: None,
            scroll_pill: None,
            modes: Vec::new(),
            frame_size: None,
            dimmed: false,
            on_resize: None,
            on_geometry: None,
        }
    }

    /// Retain unchanged content batches across cursor and selection updates.
    /// Keep this handle with the view owning the row snapshot.
    pub fn cache(mut self, cache: &TerminalGridCache) -> Self {
        self.cache = cache.clone();
        self
    }

    /// A stable id, so the overlays this grid owns keep their element state across frames.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Where the cursor is.
    pub fn cursor(mut self, cursor: GridCursor) -> Self {
        self.cursor = Some(cursor);
        self
    }

    /// The current selection, if any. Painted as a `terminal.selection` overlay behind the
    /// text, one quad per selected row span.
    pub fn selection(mut self, selection: GridSelection) -> Self {
        self.selection = Some(selection);
        self
    }

    /// Whether the terminal has focus. An unfocused terminal draws a hollow cursor.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Override the 8 px inner padding.
    pub fn padding(mut self, padding: Pixels) -> Self {
        self.padding = Some(padding);
        self
    }

    /// `viewport { offset, scrollback_len }`. When `offset > 0` the grid paints a
    /// [`ScrollbackBadge`] in its top-right corner, so a scrolled-back viewport is never
    /// mistaken for a live one.
    pub fn scrollback(mut self, offset: usize, len: usize) -> Self {
        self.scrollback = Some((offset, len));
        self
    }

    /// Show the Scroll-mode [`ScrollPill`] in the top-right corner instead of the badge.
    ///
    /// The badge is the *state* affordance and the pill is the *mode* affordance; only one of
    /// them can own the 12 px inset corner, and the pill wins while Scroll mode is active.
    pub fn scroll_pill(mut self, pill: ScrollPill) -> Self {
        self.scroll_pill = Some(pill);
        self
    }

    /// The VT modes this frame reports.
    ///
    /// Nothing is *drawn* for them here — the badges belong to the Workspace header
    /// ([`crate::components::TerminalModes`]), because the grid is live content and a badge
    /// over it hides
    /// output. The grid only needs `AltScreen`, which suppresses the scroll overlays.
    pub fn modes(mut self, modes: impl IntoIterator<Item = TerminalMode>) -> Self {
        self.modes = modes.into_iter().collect();
        self
    }

    /// The daemon's current frame size, when it differs from what the rows imply.
    ///
    /// [`TerminalGrid::on_resize`] compares this against what the painted area can hold, so a
    /// caller can compare geometry with the last acknowledged frame.
    pub fn frame_size(mut self, cols: usize, rows: usize) -> Self {
        self.frame_size = Some((cols, rows));
        self
    }

    /// Dim the whole grid to 55 %: the daemon was lost and this frame is stale (§3.12).
    pub fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self
    }

    /// Report `(cols, rows)` whenever the painted area stops matching the frame.
    ///
    /// Fires during prepaint until the declared frame matches. Callers must deduplicate
    /// against the last requested size while a resize is awaiting acknowledgement.
    pub fn on_resize(
        mut self,
        on_resize: impl Fn(usize, usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_resize = Some(Box::new(on_resize));
        self
    }

    /// Reports the content bounds (padding already removed) and measured cell metrics.
    pub fn on_geometry(mut self, report: impl Fn(Bounds<Pixels>, CellMetrics) + 'static) -> Self {
        self.on_geometry = Some(Box::new(report));
        self
    }

    /// How many rows the grid holds.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The widest row, in terminal columns.
    pub fn column_count(&self) -> usize {
        self.rows.iter().map(GridRow::columns).max().unwrap_or(0)
    }

    /// The frame size this grid believes it is painting.
    fn declared_size(&self) -> (usize, usize) {
        self.frame_size
            .unwrap_or_else(|| (self.column_count(), self.row_count()))
    }
}

impl RenderOnce for TerminalGrid {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let padding = self.padding.unwrap_or(theme.space.sm);
        let declared = self.declared_size();
        let alt_screen = self.modes.contains(&TerminalMode::AltScreen);

        // The pill is the mode affordance and wins the corner; the badge is the state
        // affordance and only appears when nothing else claims it.
        let pill = self
            .scroll_pill
            .map(|pill| pill.alt_screen(alt_screen))
            .filter(ScrollPill::is_visible);
        let badge = pill.is_none().then_some(()).and_then(|()| {
            self.scrollback
                .map(|(offset, len)| ScrollbackBadge::new(offset, len).alt_screen(alt_screen))
                .filter(ScrollbackBadge::is_visible)
        });

        let rows = self.rows;
        let cache = self.cache;
        let dimmed_opacity = theme.metrics.stale_opacity;
        let cursor = self.cursor;
        let selection = self.selection.map(GridSelection::normalized);
        let focused = self.focused;
        let on_resize = self.on_resize;
        let on_geometry = self.on_geometry;
        let selection_color = theme.terminal.selection;
        let cursor_color = theme.terminal.cursor;
        let background = theme.terminal.background;

        let painter = canvas(
            move |bounds, window, cx| {
                let metrics = CellMetrics::measure(&theme, window, cx);
                if let Some(report) = &on_geometry {
                    report(bounds, metrics);
                }
                if let Some(on_resize) = on_resize.as_ref() {
                    let (cols, rows) = metrics.fit(bounds.size);
                    if cols > 0 && rows > 0 && (cols, rows) != declared {
                        on_resize(cols, rows, window, cx);
                    }
                }

                let visible =
                    visible_rows(bounds, window.content_mask().bounds, metrics, rows.len());
                let content = cache.content(&rows, &theme, visible.clone());
                let mut selection_rects = Vec::new();
                if let Some(selection) = selection {
                    for row_ix in visible.clone() {
                        if let Some((start, end)) =
                            selection.span_in_row(row_ix, rows[row_ix].columns())
                        {
                            selection_rects.push(CellRect {
                                row: row_ix,
                                rows: 1,
                                col: start,
                                cols: end - start,
                                color: selection_color,
                            });
                        }
                    }
                }

                let cursor = cursor
                    .filter(|c| c.visible && visible.contains(&c.row))
                    .map(|c| {
                        let cell = cell_at(rows.get(c.row), c.col);
                        let hollow = !focused || c.shape == CursorShape::Hollow;
                        let shape = if c.shape == CursorShape::Hollow {
                            CursorShape::Block
                        } else {
                            c.shape
                        };
                        // A filled block hides the glyph underneath it, so the glyph is redrawn
                        // in the terminal background color — otherwise the character under the
                        // cursor silently disappears, which is how "my shell ate my prompt" bugs
                        // are reported.
                        let glyph = (!hollow && shape == CursorShape::Block)
                            .then(|| cell.filter(|cell| cell.paints_glyph()))
                            .flatten()
                            .map(|cell| {
                                let mut run = cell.text_run(&theme, cell.text.len());
                                run.color = background;
                                run.underline = None;
                                run.strikethrough = None;
                                (cell.text.clone(), run)
                            });
                        CursorLayout {
                            row: c.row,
                            col: c.col,
                            cols: cell
                                .map(|cell| cell.width.columns().max(1) as usize)
                                .unwrap_or(1),
                            shape,
                            hollow,
                            color: cursor_color,
                            glyph,
                        }
                    });

                GridLayout {
                    metrics,
                    font_size: theme.text.data.size,
                    content,
                    selection: merge_vertically(selection_rects),
                    cursor,
                }
            },
            paint_grid,
        )
        .size_full();

        let grid = div()
            .relative()
            .size_full()
            .bg(background)
            .overflow_hidden()
            .when(self.dimmed, |el| el.opacity(dimmed_opacity))
            .child(div().size_full().p(padding).child(painter))
            // Nothing else is drawn over the cells: the grid is live content and §3.6 allows
            // only the two scroll overlays and the prefix hint on top of it. The VT modes are
            // rendered by the Workspace *header* (`TerminalModes`), never here — a badge over
            // the grid permanently hides the first rows of output.
            // Both overlays position themselves 12 px inside this box; only one of them ever
            // exists at a time.
            .children(pill)
            .children(badge);
        match self.id {
            Some(id) => grid.id(id).into_any_element(),
            None => grid.into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests;
