//! `TerminalGrid` — the mirror cell grid.
//!
//! The kit deliberately defines its **own** cell model rather than depending on
//! `fleet-proto`: `fleet-ui-kit` has no domain dependencies, and the app converts
//! `proto::Cell` into [`GridCell`] on the way in, resolving `Palette(u8)` through
//! [`crate::theme::TerminalPalette::color`] and `Default` through the palette's `foreground` /
//! `background`.
//!
//! **The attribute set is complete on purpose.** `INVERSE` and `DIM` are not cosmetic:
//! lazygit, nvim status lines and `fzf` draw their selection with reverse video and dim, so a
//! reduced cell model visibly corrupts exactly the apps the default `nvim | cc | lg` layout
//! runs. [`GridCell`] therefore carries all ten VT flags plus the underline style and color,
//! and [`GridCell::resolve`] applies `inverse` / `dim` / `invisible` for the painter.
//!
//! # How it paints
//!
//! The grid is **not** a tree of `div`s — one element per cell (or even per row) is far too
//! much layout for a 200 x 60 terminal at 60 fps. It is a single [`gpui::canvas`] that owns
//! the whole cell area and replays a flat display list, following Zed's `TerminalElement`
//! (`docs/research/gpui.md` §4.8):
//!
//! 1. **Measure.** [`CellMetrics::measure`] shapes one reference glyph of the theme's mono
//!    face, so the column width is the font's real advance and not a guess. Both axes are
//!    snapped to whole device pixels, which is what keeps a full-screen grid from shimmering.
//! 2. **Batch.** Runs of adjacent cells with the same style become one string; blanks,
//!    invisible cells, wide graphemes and style changes flush the batch.
//! 3. **Merge.** Cell backgrounds are collected as horizontal runs and then coalesced
//!    vertically, so a full-width selection or a `bat` header is one quad, not 200.
//! 4. **Paint.** Backgrounds first, then the selection overlay, then one
//!    [`gpui::WindowTextSystem::shape_line`] per batch with `force_width = cell_width` (the
//!    monospace-grid trick: it locks every base glyph onto a column), then the cursor.
//!
//! # Redraw policy
//!
//! The grid **never** asks for an animation frame: it has no blink timer and no spinner, so a
//! terminal that produces no output costs zero frames. The caller drives repaints from the
//! daemon's `FrameUpdate` stream — [`GridRow`] is `PartialEq` precisely so a 60 fps producer
//! can compare the new rows against the painted ones and notify only when something changed.

use gpui::{
    App, Bounds, ContentMask, ElementId, FontFeatures, FontStyle, FontWeight, Hsla, Pixels,
    SharedString, StrikethroughStyle, TextAlign, TextRun, Window, canvas, div, fill, font, outline,
    point, prelude::*, px, size,
};

use crate::{
    components::{ScrollPill, ScrollbackBadge, TerminalMode, TerminalModes},
    theme::{ActiveTheme, Theme},
};

/// The reference glyph whose advance defines one column.
///
/// `M` is the widest ASCII letter in most faces; in a true monospace face every advance is
/// identical, so the choice only matters when the configured family is not actually monospaced
/// — and there the widest letter is the safe direction to be wrong in.
const REFERENCE_GLYPH: char = 'M';

/// The cursor shapes a VT can ask for.
///
/// `Block` / `Bar` / `Underline` mirror `proto::CursorShape` one-for-one. `Hollow` exists
/// **only** in the kit: the client derives it from focus, so it must never be added to the
/// wire enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// A filled block.
    #[default]
    Block,
    /// A vertical bar.
    Bar,
    /// An underline.
    Underline,
    /// A hollow block: the terminal does not have focus.
    Hollow,
}

/// How wide a cell is, mirroring `proto::CellWidth`.
///
/// The mapping is fixed here so nothing has to guess it: `Spacer` occupies **zero** columns —
/// it is the continuation cell that follows a `Wide` grapheme and it paints only its
/// background.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CellWidth {
    /// One column.
    #[default]
    Narrow,
    /// Two columns: a CJK or emoji grapheme.
    Wide,
    /// Zero columns: the continuation cell of the preceding `Wide` grapheme.
    Spacer,
}

impl CellWidth {
    /// How many terminal columns the cell advances: 1, 2 or 0.
    pub fn columns(self) -> u8 {
        match self {
            CellWidth::Narrow => 1,
            CellWidth::Wide => 2,
            CellWidth::Spacer => 0,
        }
    }
}

/// The underline styles a VT can ask for, mirroring the `UNDERLINE` / `DOUBLE_UNDERLINE` /
/// `CURLY_UNDERLINE` proto flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UnderlineStyle {
    /// No underline.
    #[default]
    None,
    /// A single 1 px line.
    Single,
    /// A 2 px line, the closest honest rendering of a double underline.
    Double,
    /// A wavy line — what LSP diagnostics inside `nvim` draw.
    Curly,
}

impl UnderlineStyle {
    /// Whether anything is drawn.
    pub fn is_some(self) -> bool {
        !matches!(self, UnderlineStyle::None)
    }

    /// The gpui decoration for this style, in the given color.
    ///
    /// gpui has no double-underline primitive, so `Double` is a 2 px solid rule — the closest
    /// honest rendering, and the one the design system documents.
    fn decoration(self, color: Hsla) -> Option<gpui::UnderlineStyle> {
        let (thickness, wavy) = match self {
            UnderlineStyle::None => return None,
            UnderlineStyle::Single => (px(1.0), false),
            UnderlineStyle::Double => (px(2.0), false),
            UnderlineStyle::Curly => (px(1.0), true),
        };
        Some(gpui::UnderlineStyle {
            thickness,
            color: Some(color),
            wavy,
        })
    }
}

/// Where the cursor is and what it looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridCursor {
    /// Row index, 0-based, in the visible viewport.
    pub row: usize,
    /// Column index, 0-based.
    pub col: usize,
    /// Whether to draw it at all.
    pub visible: bool,
    /// Shape.
    pub shape: CursorShape,
}

/// One cell of the mirror grid, with colors already resolved from the theme palette.
#[derive(Clone, Debug, PartialEq)]
pub struct GridCell {
    /// The grapheme. Empty for a continuation cell of a wide grapheme.
    pub text: SharedString,
    /// Resolved foreground.
    pub fg: Hsla,
    /// Resolved background.
    pub bg: Option<Hsla>,
    /// Bold.
    pub bold: bool,
    /// Dim (SGR 2). Rendered as 55 % foreground opacity, not as a different color.
    pub dim: bool,
    /// Italic.
    pub italic: bool,
    /// Underline style.
    pub underline: UnderlineStyle,
    /// An explicit underline color (SGR 58). `None` means "use the foreground".
    pub underline_color: Option<Hsla>,
    /// Strikethrough.
    pub strikethrough: bool,
    /// Reverse video: foreground and background swap in [`GridCell::resolve`].
    pub inverse: bool,
    /// Blink. The kit does not animate; it renders blinking cells at 70 % opacity so they are
    /// distinguishable without costing a frame timer.
    pub blink: bool,
    /// Invisible (SGR 8): the glyph is not painted, the background still is.
    pub invisible: bool,
    /// Narrow, wide or the zero-column spacer that follows a wide grapheme.
    pub width: CellWidth,
}

impl GridCell {
    /// A plain cell in the palette's default foreground.
    pub fn new(text: impl Into<SharedString>, theme: &Theme) -> Self {
        Self {
            text: text.into(),
            fg: theme.terminal.foreground,
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            underline: UnderlineStyle::None,
            underline_color: None,
            strikethrough: false,
            inverse: false,
            blink: false,
            invisible: false,
            width: CellWidth::Narrow,
        }
    }

    /// Set the foreground.
    pub fn fg(mut self, fg: Hsla) -> Self {
        self.fg = fg;
        self
    }

    /// Set the background.
    pub fn bg(mut self, bg: Hsla) -> Self {
        self.bg = Some(bg);
        self
    }

    /// Bold.
    pub fn bold(mut self, bold: bool) -> Self {
        self.bold = bold;
        self
    }

    /// Dim.
    pub fn dim(mut self, dim: bool) -> Self {
        self.dim = dim;
        self
    }

    /// Italic.
    pub fn italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }

    /// Underline style.
    pub fn underline(mut self, style: UnderlineStyle) -> Self {
        self.underline = style;
        self
    }

    /// Explicit underline color (SGR 58).
    pub fn underline_color(mut self, color: Hsla) -> Self {
        self.underline_color = Some(color);
        self
    }

    /// Strikethrough.
    pub fn strikethrough(mut self, strikethrough: bool) -> Self {
        self.strikethrough = strikethrough;
        self
    }

    /// Reverse video.
    pub fn inverse(mut self, inverse: bool) -> Self {
        self.inverse = inverse;
        self
    }

    /// Blink.
    pub fn blink(mut self, blink: bool) -> Self {
        self.blink = blink;
        self
    }

    /// Invisible.
    pub fn invisible(mut self, invisible: bool) -> Self {
        self.invisible = invisible;
        self
    }

    /// Narrow, wide or spacer.
    pub fn width(mut self, width: CellWidth) -> Self {
        self.width = width;
        self
    }

    /// The colors the painter actually uses, with `inverse`, `dim`, `blink` and `invisible`
    /// applied against the palette's default background.
    ///
    /// This is the one place reverse video is resolved, so `fleet-app` never has to.
    pub fn resolve(&self, theme: &Theme) -> (Hsla, Option<Hsla>) {
        let default_bg = theme.terminal.background;
        let (mut fg, mut bg) = if self.inverse {
            (self.bg.unwrap_or(default_bg), Some(self.fg))
        } else {
            (self.fg, self.bg)
        };
        if self.dim {
            fg.a *= 0.55;
        }
        if self.blink {
            fg.a *= 0.7;
        }
        if self.invisible {
            fg.a = 0.0;
        }
        if bg == Some(default_bg) {
            bg = None;
        }
        (fg, bg)
    }

    /// Whether two cells can share one text run.
    pub fn same_style(&self, other: &GridCell) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.bold == other.bold
            && self.dim == other.dim
            && self.italic == other.italic
            && self.underline == other.underline
            && self.underline_color == other.underline_color
            && self.strikethrough == other.strikethrough
            && self.inverse == other.inverse
            && self.blink == other.blink
            && self.invisible == other.invisible
    }

    /// Whether the cell contributes anything to the text pass.
    ///
    /// A spacer and an `invisible` cell paint their background and nothing else, and so does a
    /// plain blank — a run of blanks is the most common thing on a terminal row, and shaping
    /// it costs glyphs that draw nothing. A *decorated* blank is different: the underline
    /// below a hyperlink's trailing space is real, so it keeps its run.
    fn paints_glyph(&self) -> bool {
        if self.invisible || self.width == CellWidth::Spacer {
            return false;
        }
        let blank = self.text.is_empty() || self.text.as_ref() == " ";
        !blank || self.underline.is_some() || self.strikethrough
    }

    /// The string this cell shapes: a decorated empty cell still needs one blank to hang its
    /// underline on.
    fn glyph_text(&self) -> &str {
        if self.text.is_empty() {
            " "
        } else {
            self.text.as_ref()
        }
    }

    /// The gpui text run for this cell, with `inverse` / `dim` / `blink` already applied.
    fn text_run(&self, theme: &Theme, len: usize) -> TextRun {
        let (fg, _) = self.resolve(theme);
        let underline_color = self.underline_color.unwrap_or(fg);
        TextRun {
            len,
            font: cell_font(theme, self.bold, self.italic),
            color: fg,
            // Backgrounds are painted as merged quads before the text pass, so the run must
            // not paint its own: a per-run quad reintroduces exactly the seams the merge
            // exists to remove.
            background_color: None,
            underline: self.underline.decoration(underline_color),
            strikethrough: self.strikethrough.then(|| StrikethroughStyle {
                thickness: px(1.0),
                color: Some(fg),
            }),
        }
    }
}

/// One row of the mirror grid.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridRow {
    /// The cells, left to right.
    pub cells: Vec<GridCell>,
}

impl GridRow {
    /// A row from cells.
    pub fn new(cells: impl IntoIterator<Item = GridCell>) -> Self {
        Self {
            cells: cells.into_iter().collect(),
        }
    }

    /// How many terminal columns the row occupies, honouring wide cells and spacers.
    pub fn columns(&self) -> usize {
        self.cells.iter().map(|c| c.width.columns() as usize).sum()
    }
}

/// A text selection on the mirror grid, in stream order (not a rectangle).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSelection {
    /// Anchor row.
    pub start_row: usize,
    /// Anchor column.
    pub start_col: usize,
    /// Head row.
    pub end_row: usize,
    /// Head column.
    pub end_col: usize,
}

impl GridSelection {
    /// A selection between two points, in either order.
    pub fn new(start_row: usize, start_col: usize, end_row: usize, end_col: usize) -> Self {
        Self {
            start_row,
            start_col,
            end_row,
            end_col,
        }
    }

    /// The same selection with the anchor before the head.
    pub fn normalized(self) -> Self {
        let a = (self.start_row, self.start_col);
        let b = (self.end_row, self.end_col);
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        Self {
            start_row: s.0,
            start_col: s.1,
            end_row: e.0,
            end_col: e.1,
        }
    }

    /// The half-open column span selected on `row`, given how many columns that row holds.
    /// `None` when the row is outside the selection.
    pub fn span_in_row(self, row: usize, row_columns: usize) -> Option<(usize, usize)> {
        let s = self.normalized();
        if row < s.start_row || row > s.end_row {
            return None;
        }
        let start = if row == s.start_row {
            s.start_col.min(row_columns)
        } else {
            0
        };
        let end = if row == s.end_row {
            s.end_col.min(row_columns)
        } else {
            row_columns
        };
        (end > start).then_some((start, end))
    }
}

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

/// The mono [`gpui::Font`] a cell of the given weight and slant is shaped with.
///
/// Ligatures are disabled: a terminal that renders `!=` as `≠` has silently moved the column
/// grid, which is the one thing a mirror grid may never do.
fn cell_font(theme: &Theme, bold: bool, italic: bool) -> gpui::Font {
    let mut face = font(theme.font_mono.clone());
    face.features = FontFeatures::disable_ligatures();
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
struct CellRect {
    row: usize,
    rows: usize,
    col: usize,
    cols: usize,
    color: Hsla,
}

/// One shaped run of same-styled cells.
struct TextBatch {
    row: usize,
    col: usize,
    /// How many terminal **columns** the batch covers. `run.len` counts utf-8 bytes instead,
    /// and the two are only equal for pure ASCII.
    cells: usize,
    text: String,
    run: TextRun,
}

/// Everything the paint pass replays, computed once in prepaint.
struct GridLayout {
    metrics: CellMetrics,
    font_size: Pixels,
    backgrounds: Vec<CellRect>,
    selection: Vec<CellRect>,
    text: Vec<TextBatch>,
    cursor: Option<CursorLayout>,
}

/// The cursor quad plus the glyph that has to be redrawn on top of it.
struct CursorLayout {
    row: usize,
    col: usize,
    cols: usize,
    shape: CursorShape,
    hollow: bool,
    color: Hsla,
    glyph: Option<(SharedString, TextRun)>,
}

/// The painted cell grid.
#[derive(IntoElement)]
pub struct TerminalGrid {
    id: Option<ElementId>,
    rows: Vec<GridRow>,
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
}

impl TerminalGrid {
    /// A grid over the mirror rows.
    pub fn new(rows: impl IntoIterator<Item = GridRow>) -> Self {
        Self {
            id: None,
            rows: rows.into_iter().collect(),
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
        }
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

    /// The VT modes this frame reports, drawn as [`TerminalModes`] badges in the grid's
    /// top-left corner. Zero-suppressed: a plain shell shows nothing.
    pub fn modes(mut self, modes: impl IntoIterator<Item = TerminalMode>) -> Self {
        self.modes = modes.into_iter().collect();
        self
    }

    /// The daemon's current frame size, when it differs from what the rows imply.
    ///
    /// [`TerminalGrid::on_resize`] compares this against what the painted area can hold, so a
    /// frame that has not caught up with the window yet does not re-report the same size every
    /// frame.
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
    /// This is how the daemon learns the PTY has to be resized. It fires during prepaint and
    /// only on an actual change, so wiring it straight to a `Request::ResizeTerminal` does not
    /// loop: the next frame arrives with the new size and the comparison goes quiet.
    pub fn on_resize(
        mut self,
        on_resize: impl Fn(usize, usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_resize = Some(Box::new(on_resize));
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

/// Collect the horizontal background runs of one row.
fn row_backgrounds(row_ix: usize, row: &GridRow, theme: &Theme, out: &mut Vec<CellRect>) {
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
fn merge_vertically(rects: Vec<CellRect>) -> Vec<CellRect> {
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
fn row_text_batches<'row>(
    row_ix: usize,
    row: &'row GridRow,
    theme: &Theme,
    out: &mut Vec<TextBatch>,
) {
    let mut col = 0usize;
    let mut open: Option<(TextBatch, &'row GridCell)> = None;
    for cell in &row.cells {
        let span = cell.width.columns() as usize;
        if !cell.paints_glyph() {
            if let Some((batch, _)) = open.take() {
                out.push(batch);
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
            batch.text.push_str(cell.glyph_text());
            batch.run.len = batch.text.len();
            batch.cells += span;
            col += span;
            continue;
        }
        if let Some((batch, _)) = open.take() {
            out.push(batch);
        }
        let text = cell.glyph_text().to_string();
        let run = cell.text_run(theme, text.len());
        open = Some((
            TextBatch {
                row: row_ix,
                col,
                cells: span,
                text,
                run,
            },
            cell,
        ));
        col += span;
    }
    if let Some((batch, _)) = open.take() {
        out.push(batch);
    }
}

impl RenderOnce for TerminalGrid {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let padding = self.padding.unwrap_or(theme.space.sm);
        let declared = self.declared_size();
        let alt_screen = self.modes.contains(&TerminalMode::AltScreen);
        let modes = TerminalModes::new(self.modes.clone());
        let show_modes = modes.is_visible();

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
        let cursor = self.cursor;
        let selection = self.selection.map(GridSelection::normalized);
        let focused = self.focused;
        let on_resize = self.on_resize;
        let selection_color = theme.terminal.selection;
        let cursor_color = theme.terminal.cursor;
        let background = theme.terminal.background;
        let inset = theme.space.md;

        let painter = canvas(
            move |bounds, window, cx| {
                let metrics = CellMetrics::measure(&theme, window, cx);
                if let Some(on_resize) = on_resize.as_ref() {
                    let (cols, rows) = metrics.fit(bounds.size);
                    if cols > 0 && rows > 0 && (cols, rows) != declared {
                        on_resize(cols, rows, window, cx);
                    }
                }

                let mut backgrounds = Vec::new();
                let mut text = Vec::new();
                let mut selection_rects = Vec::new();
                for (row_ix, row) in rows.iter().enumerate() {
                    row_backgrounds(row_ix, row, &theme, &mut backgrounds);
                    row_text_batches(row_ix, row, &theme, &mut text);
                    if let Some((start, end)) =
                        selection.and_then(|s| s.span_in_row(row_ix, row.columns()))
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

                let cursor = cursor.filter(|c| c.visible).map(|c| {
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
                    backgrounds: merge_vertically(backgrounds),
                    selection: merge_vertically(selection_rects),
                    text,
                    cursor,
                }
            },
            paint_grid,
        )
        .size_full();

        div()
            .relative()
            .size_full()
            .bg(background)
            .overflow_hidden()
            .when(self.dimmed, |el| el.opacity(0.55))
            .child(div().size_full().p(padding).child(painter))
            // The mode badges take the free corner: the scroll overlays own the top right and
            // the prefix hint owns the bottom left.
            .when(show_modes, |el| {
                el.child(div().absolute().top(inset).left(inset).child(modes))
            })
            // Both overlays position themselves 12 px inside this box; only one of them ever
            // exists at a time.
            .children(pill)
            .children(badge)
    }
}

/// The cell that occupies terminal column `col` of `row`, honouring wide cells and spacers.
fn cell_at(row: Option<&GridRow>, col: usize) -> Option<&GridCell> {
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
            return Some(cell);
        }
        cursor += span;
    }
    None
}

/// Replay the display list. Backgrounds, then selection, then text, then the cursor.
fn paint_grid(bounds: Bounds<Pixels>, layout: GridLayout, window: &mut Window, cx: &mut App) {
    let metrics = layout.metrics;
    let origin = bounds.origin;
    // `floor` the left edge and `ceil` the width so two horizontally adjacent quads overlap by
    // a sub-pixel instead of leaving a hairline of background between them.
    let rect_bounds = |rect: &CellRect| {
        let x = (f32::from(origin.x) + rect.col as f32 * f32::from(metrics.width)).floor();
        let y = f32::from(origin.y) + rect.row as f32 * f32::from(metrics.height);
        let w = (rect.cols as f32 * f32::from(metrics.width)).ceil();
        let h = rect.rows as f32 * f32::from(metrics.height);
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    };

    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for rect in &layout.backgrounds {
            window.paint_quad(fill(rect_bounds(rect), rect.color));
        }
        for rect in &layout.selection {
            window.paint_quad(fill(rect_bounds(rect), rect.color));
        }
        for batch in &layout.text {
            let position = point(
                origin.x + metrics.width * (batch.col as f32),
                origin.y + metrics.height * (batch.row as f32),
            );
            let shaped = window.text_system().shape_line(
                SharedString::from(batch.text.clone()),
                layout.font_size,
                std::slice::from_ref(&batch.run),
                Some(metrics.width),
            );
            // A shaping failure is a font problem, not a program error: the cell stays blank
            // rather than taking the window down mid-frame.
            let _ = shaped.paint(position, metrics.height, TextAlign::Left, None, window, cx);
        }
        if let Some(cursor) = &layout.cursor {
            paint_cursor(cursor, origin, metrics, layout.font_size, window, cx);
        }
    });
}

/// Paint the cursor quad and, under a filled block, the glyph it covers.
fn paint_cursor(
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
    let bar_w = px(2.0);
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
        let _ = shaped.paint(
            point(left, top),
            metrics.height,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::dark()
    }

    #[test]
    fn spacer_occupies_zero_columns() {
        assert_eq!(CellWidth::Narrow.columns(), 1);
        assert_eq!(CellWidth::Wide.columns(), 2);
        assert_eq!(CellWidth::Spacer.columns(), 0);
    }

    #[test]
    fn selection_spans_are_clipped_per_row() {
        let sel = GridSelection::new(2, 4, 1, 2).normalized();
        assert_eq!((sel.start_row, sel.start_col), (1, 2));
        assert_eq!(sel.span_in_row(0, 80), None);
        assert_eq!(sel.span_in_row(1, 80), Some((2, 80)));
        assert_eq!(sel.span_in_row(2, 80), Some((0, 4)));
        assert_eq!(sel.span_in_row(3, 80), None);
    }

    #[test]
    fn equal_styles_merge_into_one_batch() {
        let t = theme();
        let row = GridRow::new("hello".chars().map(|c| GridCell::new(c.to_string(), &t)));
        let mut out = Vec::new();
        row_text_batches(0, &row, &t, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "hello");
        assert_eq!(out[0].col, 0);
    }

    #[test]
    fn a_style_change_and_a_blank_both_flush_the_batch() {
        let t = theme();
        let row = GridRow::new([
            GridCell::new("a", &t),
            GridCell::new("b", &t).bold(true),
            GridCell::new(" ", &t),
            GridCell::new("c", &t),
        ]);
        let mut out = Vec::new();
        row_text_batches(0, &row, &t, &mut out);
        let batches: Vec<_> = out.iter().map(|b| (b.col, b.text.as_str())).collect();
        assert_eq!(batches, vec![(0, "a"), (1, "b"), (3, "c")]);
    }

    #[test]
    fn a_wide_grapheme_is_its_own_batch_and_advances_two_columns() {
        let t = theme();
        let row = GridRow::new([
            GridCell::new("a", &t),
            GridCell::new("漢", &t).width(CellWidth::Wide),
            GridCell::new("", &t).width(CellWidth::Spacer),
            GridCell::new("b", &t),
        ]);
        let mut out = Vec::new();
        row_text_batches(0, &row, &t, &mut out);
        let batches: Vec<_> = out.iter().map(|b| (b.col, b.text.as_str())).collect();
        assert_eq!(batches, vec![(0, "a"), (1, "漢"), (3, "b")]);
    }

    #[test]
    fn invisible_cells_keep_their_background_and_drop_their_glyph() {
        let t = theme();
        let row = GridRow::new([GridCell::new("x", &t)
            .bg(t.terminal.ansi[1])
            .invisible(true)]);
        let mut text = Vec::new();
        row_text_batches(0, &row, &t, &mut text);
        assert!(text.is_empty());
        let mut bg = Vec::new();
        row_backgrounds(0, &row, &t, &mut bg);
        assert_eq!(bg.len(), 1);
        assert_eq!(bg[0].cols, 1);
    }

    #[test]
    fn a_wide_cell_and_its_spacer_paint_exactly_two_columns() {
        let t = theme();
        let red = t.terminal.ansi[1];
        let row = GridRow::new([
            GridCell::new("\u{6f22}", &t).bg(red).width(CellWidth::Wide),
            GridCell::new("", &t).bg(red).width(CellWidth::Spacer),
            GridCell::new("X", &t).bg(red),
        ]);
        let mut rects = Vec::new();
        row_backgrounds(0, &row, &t, &mut rects);
        assert_eq!(rects.len(), 1, "one colour, one run");
        assert_eq!(
            (rects[0].col, rects[0].cols),
            (0, 3),
            "two columns for the wide cell, one for the narrow one that follows"
        );
    }

    #[test]
    fn a_spacer_never_extends_a_run_past_the_cell_that_follows() {
        let t = theme();
        let red = t.terminal.ansi[1];
        let blue = t.terminal.ansi[4];
        let row = GridRow::new([
            GridCell::new("\u{6f22}", &t).bg(red).width(CellWidth::Wide),
            GridCell::new("", &t).bg(red).width(CellWidth::Spacer),
            GridCell::new("X", &t).bg(blue),
        ]);
        let mut rects = Vec::new();
        row_backgrounds(0, &row, &t, &mut rects);
        let spans: Vec<_> = rects.iter().map(|rect| (rect.col, rect.cols)).collect();
        assert_eq!(
            spans,
            vec![(0, 2), (2, 1)],
            "the runs must tile the row, never overlap"
        );
    }

    #[test]
    fn adjacent_backgrounds_merge_horizontally_then_vertically() {
        let t = theme();
        let red = t.terminal.ansi[1];
        let make = || GridRow::new((0..4).map(|_| GridCell::new("x", &t).bg(red)));
        let mut rects = Vec::new();
        row_backgrounds(0, &make(), &t, &mut rects);
        row_backgrounds(1, &make(), &t, &mut rects);
        assert_eq!(rects.len(), 2);
        let merged = merge_vertically(rects);
        assert_eq!(merged.len(), 1);
        assert_eq!((merged[0].cols, merged[0].rows), (4, 2));
    }

    #[test]
    fn inverse_swaps_the_pair_and_dim_lowers_the_foreground() {
        let t = theme();
        let cell = GridCell::new("x", &t)
            .fg(t.terminal.ansi[2])
            .bg(t.terminal.ansi[4])
            .inverse(true);
        let (fg, bg) = cell.resolve(&t);
        assert_eq!(fg, t.terminal.ansi[4]);
        assert_eq!(bg, Some(t.terminal.ansi[2]));

        let dim = GridCell::new("x", &t).dim(true);
        let (dim_fg, _) = dim.resolve(&t);
        assert!(dim_fg.a < t.terminal.foreground.a);
    }

    #[test]
    fn cell_at_resolves_through_wide_cells() {
        let t = theme();
        let row = GridRow::new([
            GridCell::new("a", &t),
            GridCell::new("漢", &t).width(CellWidth::Wide),
            GridCell::new("", &t).width(CellWidth::Spacer),
            GridCell::new("b", &t),
        ]);
        assert_eq!(cell_at(Some(&row), 0).map(|c| c.text.as_ref()), Some("a"));
        assert_eq!(cell_at(Some(&row), 1).map(|c| c.text.as_ref()), Some("漢"));
        assert_eq!(cell_at(Some(&row), 2).map(|c| c.text.as_ref()), Some("漢"));
        assert_eq!(cell_at(Some(&row), 3).map(|c| c.text.as_ref()), Some("b"));
        assert_eq!(cell_at(Some(&row), 9), None);
    }

    #[test]
    fn metrics_fit_whole_cells_only() {
        let metrics = CellMetrics {
            width: px(8.0),
            height: px(18.0),
        };
        assert_eq!(metrics.fit(size(px(83.0), px(55.0))), (10, 3));
        assert_eq!(metrics.fit(size(px(0.0), px(0.0))), (0, 0));
    }
}
