//! GPUI element that paints the mirrored terminal cell grid.
//!
//! The painting itself belongs to [`fleet_ui_kit::TerminalGrid`]; what lives here is the
//! translation between the daemon's wire model and the kit's, plus the geometry the Workspace
//! needs in order to tell the daemon how big its PTY should be.
//!
//! Three things happen on the way from a [`MirrorGrid`] to a painted frame:
//!
//! 1. **Colors are resolved.** `fleet-ui-kit` has no domain dependency, so a `Palette(u8)` is
//!    resolved through [`fleet_ui_kit::theme::TerminalPalette::color`] and a `Default` through
//!    the palette's `foreground` / `background` here, once per cell.
//! 2. **Attributes are unpacked.** The proto keeps ten VT flags in a bitfield; the kit keeps
//!    them as named fields, because `INVERSE` and `DIM` decide whether `nvim`'s status line and
//!    `fzf`'s selection are legible at all.
//! 3. **The area is measured.** The PTY size is a function of the painted area, not of the
//!    window: [`measure`] reports the terminal area's pixel bounds every frame and
//!    [`grid_size`] turns them into the `cols × rows` the daemon is asked to resize to.
//!
//! Everything except [`measure`] is a pure function of its arguments and is unit tested.

use std::collections::BTreeMap;

use fleet_proto::terminal::{
    Cell as ProtoCell, CellAttrs, CellWidth as ProtoWidth, Color, CursorShape as ProtoShape,
    TerminalModes,
};
use fleet_ui_kit::{
    CellWidth, CursorShape, GridCell, GridCursor, GridRow, GridSelection,
    TerminalMode as KitTerminalMode, Theme, UnderlineStyle,
};
use gpui::{Bounds, Hsla, IntoElement, Pixels, Point, Rgba, Size, canvas, prelude::*, px};

use crate::state::MirrorGrid;

/// The inner padding [`fleet_ui_kit::TerminalGrid`] paints with (§3.6: 8 px, no border).
///
/// It is subtracted on both axes before the area is divided into cells, so the grid the daemon
/// renders is exactly the grid that fits.
pub const GRID_PADDING: f32 = 8.0;

/// The smallest grid Fleet ever asks a PTY for.
///
/// A zero-sized PTY is not a valid `TIOCSWINSZ` argument and a one-column one makes every
/// full-screen app misbehave, so a window squeezed below the minimum still gets a usable shell.
pub const MIN_COLS: u16 = 2;
/// The smallest row count Fleet ever asks a PTY for. See [`MIN_COLS`].
pub const MIN_ROWS: u16 = 1;

/// The grid size that fits `area`, in cells.
///
/// `cell` is `theme.metrics.cell_w` × `theme.metrics.cell_h`. The result is clamped to
/// [`MIN_COLS`] × [`MIN_ROWS`] so a collapsed window never asks the daemon for an empty PTY.
#[must_use]
pub fn grid_size(area: Size<Pixels>, cell: Size<Pixels>) -> (u16, u16) {
    let usable = |extent: Pixels| f32::from(extent) - 2.0 * GRID_PADDING;
    let fit = |extent: f32, unit: Pixels| {
        let unit = f32::from(unit);
        if unit <= 0.0 || !extent.is_finite() || extent <= 0.0 {
            return 0;
        }
        // `as` saturates at u16::MAX for a NaN-free positive float, and the floor is what fits.
        (extent / unit).floor().max(0.0).min(f32::from(u16::MAX)) as u16
    };
    (
        fit(usable(area.width), cell.width).max(MIN_COLS),
        fit(usable(area.height), cell.height).max(MIN_ROWS),
    )
}

/// An invisible element that reports the pixel bounds it was laid out into.
///
/// The Workspace fills the terminal area with it and turns the reported size into a
/// `ResizeTerminal` request. It draws nothing: only the layout pass is interesting.
pub fn measure(report: impl 'static + FnOnce(Bounds<Pixels>)) -> impl IntoElement {
    canvas(
        move |bounds, _window, _cx| report(bounds),
        |_bounds, (), _window, _cx| {},
    )
    .absolute()
    .size_full()
}

/// One cell in the visible terminal viewport.
///
/// Columns are terminal columns rather than indices into [`MirrorGrid::lines`]: a wide cell
/// occupies two columns while its spacer cell occupies none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CellPoint {
    /// Viewport row, zero based.
    pub row: usize,
    /// Terminal column, zero based.
    pub col: usize,
}

impl CellPoint {
    /// A viewport cell.
    #[must_use]
    pub const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// One terminal cell addressed in the absolute scrollback coordinate space.
///
/// Unlike a viewport row, `line` continues to identify the same terminal line when new output
/// moves the viewport. Mouse selections use this type so repaints do not retarget their ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AbsoluteCellPoint {
    /// Absolute scrollback line, zero based.
    pub line: u64,
    /// Terminal column, zero based.
    pub col: usize,
}

impl AbsoluteCellPoint {
    /// An absolute terminal cell.
    #[must_use]
    pub const fn new(line: u64, col: usize) -> Self {
        Self { line, col }
    }
}

/// Unit a mouse gesture expands by after its initiating click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionGranularity {
    /// Individual terminal cells.
    Cell,
    /// Terminal-friendly word runs.
    Word,
    /// Complete visual rows.
    Line,
}

/// Inclusive absolute endpoints of a stream selection, normalized in reading order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbsoluteCellSelection {
    /// First selected cell.
    pub start: AbsoluteCellPoint,
    /// Last selected cell.
    pub end: AbsoluteCellPoint,
}

impl AbsoluteCellSelection {
    /// Builds a normalized inclusive selection.
    #[must_use]
    pub fn new(first: AbsoluteCellPoint, second: AbsoluteCellPoint) -> Self {
        let (start, end) = if first <= second {
            (first, second)
        } else {
            (second, first)
        };
        Self { start, end }
    }
}

/// A stream selection containing both the anchor cell and the head cell.
///
/// [`GridSelection`] uses half-open column ranges for painting. Mouse selection is naturally
/// inclusive at both ends, so the later cell is advanced by one column before the range is
/// returned. This also makes dragging backwards produce exactly the same range as dragging
/// forwards.
#[must_use]
pub fn cell_selection(anchor: CellPoint, head: CellPoint) -> GridSelection {
    let (first, last) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    GridSelection::new(first.row, first.col, last.row, last.col.saturating_add(1))
}

/// Selects the cell, word, or line at one viewport point in absolute coordinates.
#[must_use]
pub fn absolute_selection_at(
    grid: &MirrorGrid,
    point: CellPoint,
    granularity: SelectionGranularity,
) -> Option<AbsoluteCellSelection> {
    let viewport = match granularity {
        SelectionGranularity::Cell => cell_selection(point, point),
        SelectionGranularity::Word => word_selection(grid, point)?,
        SelectionGranularity::Line => {
            GridSelection::new(point.row, 0, point.row, usize::from(grid.cols))
        }
    }
    .normalized();
    let base = viewport_base(grid);
    Some(AbsoluteCellSelection::new(
        AbsoluteCellPoint::new(base + viewport.start_row as u64, viewport.start_col),
        AbsoluteCellPoint::new(
            base + viewport.end_row as u64,
            viewport.end_col.saturating_sub(1),
        ),
    ))
}

/// Extends an initial mouse selection according to its click granularity.
///
/// Pointer jitter inside the initiating cell returns the initial word or line unchanged. Once the
/// pointer reaches another cell, the hovered word or line is included as a whole.
#[must_use]
pub fn extend_absolute_selection(
    grid: &MirrorGrid,
    initial: AbsoluteCellSelection,
    initiating: AbsoluteCellPoint,
    hovered: CellPoint,
    granularity: SelectionGranularity,
) -> Option<AbsoluteCellSelection> {
    let hovered_cell =
        AbsoluteCellPoint::new(viewport_base(grid) + hovered.row as u64, hovered.col);
    if hovered_cell == initiating {
        return Some(initial);
    }
    let hovered_range = absolute_selection_at(grid, hovered, granularity)?;
    Some(if hovered_cell < initiating {
        AbsoluteCellSelection::new(hovered_range.start, initial.end)
    } else {
        AbsoluteCellSelection::new(initial.start, hovered_range.end)
    })
}

/// Converts an absolute cell selection into the currently visible viewport rows.
///
/// The returned selection is clipped at both viewport edges. `None` means every selected row is
/// currently off-screen; callers must retain the absolute endpoints so it can become visible
/// again after the viewport moves. Copy uses a separate bounded row cache for the off-screen part.
#[must_use]
pub fn viewport_cell_selection(
    grid: &MirrorGrid,
    anchor: AbsoluteCellPoint,
    head: AbsoluteCellPoint,
) -> Option<GridSelection> {
    let (first, last) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    let base = viewport_base(grid);
    let bottom = viewport_last(grid)?;
    if last.line < base || first.line > bottom {
        return None;
    }

    let visible_first = first.line.max(base);
    let visible_last = last.line.min(bottom);
    let start_col = if first.line < base { 0 } else { first.col };
    let end_col = if last.line > bottom {
        usize::from(grid.cols)
    } else {
        last.col.saturating_add(1)
    };
    Some(GridSelection::new(
        usize::try_from(visible_first - base).unwrap_or(0),
        start_col,
        usize::try_from(visible_last - base).unwrap_or(0),
        end_col,
    ))
}

/// The word under `point`, using terminal-friendly word boundaries.
///
/// Letters, numbers and `_` form words; adjacent whitespace forms a blank run; punctuation forms
/// a third run. This keeps paths pleasantly predictable without inventing shell-specific parsing.
#[must_use]
pub fn word_selection(grid: &MirrorGrid, point: CellPoint) -> Option<GridSelection> {
    let line = grid.lines.get(point.row)?;
    let mut cells = Vec::new();
    let mut col = 0usize;
    for cell in line {
        let width = cell_columns(cell);
        if width == 0 {
            continue;
        }
        let end = col.saturating_add(width);
        cells.push((col, end, selection_class(cell.text.as_str())));
        col = end;
    }
    let index = cells
        .iter()
        .position(|(start, end, _)| point.col >= *start && point.col < *end)?;
    let class = cells[index].2;
    let first = (0..=index)
        .rev()
        .take_while(|candidate| cells[*candidate].2 == class)
        .last()
        .unwrap_or(index);
    let last = (index..cells.len())
        .take_while(|candidate| cells[*candidate].2 == class)
        .last()
        .unwrap_or(index);
    Some(GridSelection::new(
        point.row,
        cells[first].0,
        point.row,
        cells[last].1,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectionClass {
    Word,
    Whitespace,
    Punctuation,
}

fn selection_class(text: &str) -> SelectionClass {
    let Some(character) = text.chars().next() else {
        return SelectionClass::Whitespace;
    };
    if character.is_whitespace() {
        SelectionClass::Whitespace
    } else if character.is_alphanumeric() || character == '_' {
        SelectionClass::Word
    } else {
        SelectionClass::Punctuation
    }
}

/// The viewport cell under a window position, clamped to the visible terminal grid.
///
/// `bounds` includes the grid's padding. The returned point therefore subtracts
/// [`GRID_PADDING`] before dividing by the measured cell size. Clamping lets a drag that ends
/// in the padding still select the first or last cell instead of losing the mouse-up event.
#[must_use]
pub fn cell_at_position(
    bounds: Bounds<Pixels>,
    position: Point<Pixels>,
    cell: Size<Pixels>,
    cols: u16,
    rows: u16,
) -> Option<CellPoint> {
    if cols == 0
        || rows == 0
        || bounds.size.width <= px(0.0)
        || bounds.size.height <= px(0.0)
        || cell.width <= px(0.0)
        || cell.height <= px(0.0)
    {
        return None;
    }
    let x = f32::from(position.x - bounds.origin.x) - GRID_PADDING;
    let y = f32::from(position.y - bounds.origin.y) - GRID_PADDING;
    let col = (x / f32::from(cell.width)).floor() as isize;
    let row = (y / f32::from(cell.height)).floor() as isize;
    Some(CellPoint::new(
        usize::try_from(row.clamp(0, isize::try_from(rows - 1).unwrap_or(isize::MAX))).unwrap_or(0),
        usize::try_from(col.clamp(0, isize::try_from(cols - 1).unwrap_or(isize::MAX))).unwrap_or(0),
    ))
}

/// The text inside a visible cell selection.
///
/// Rows are joined in reading order. A soft-wrapped row is joined directly to its continuation
/// and keeps trailing cells because they are part of the logical line; hard rows are separated by
/// `\n` and have terminal padding trimmed. Wide graphemes are emitted once when either of their
/// two columns intersects the selection; their zero-column spacer is never emitted.
#[must_use]
pub fn grid_selection_text(grid: &MirrorGrid, selection: GridSelection) -> String {
    if grid.lines.is_empty() || selection.start_row.min(selection.end_row) >= grid.lines.len() {
        return String::new();
    }
    let selection = selection.normalized();
    let mut text = String::new();
    let mut previous_row = None;
    for row in selection.start_row..=selection.end_row.min(grid.lines.len().saturating_sub(1)) {
        let Some((start, end)) = selection.span_in_row(row, usize::from(grid.cols)) else {
            continue;
        };
        let previous_wrapped = previous_row.and_then(|row| grid.wrapped.get(row)).copied();
        let wrapped = grid.wrapped.get(row).copied().unwrap_or(false);
        append_selected_row(
            &mut text,
            previous_wrapped,
            &grid.lines[row],
            start,
            end,
            wrapped,
        );
        previous_row = Some(row);
    }
    text
}

/// A mirrored viewport row retained for an absolute mouse selection.
///
/// Cells, rather than an already-trimmed string, are cached while a selection covers the row so
/// partial first/last rows and wide cells retain the same extraction semantics off-screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedGridRow {
    cells: Vec<ProtoCell>,
    wrapped: bool,
}

/// Clones one visible row into the bounded selection cache owned by the Workspace.
#[must_use]
pub fn cached_grid_row(grid: &MirrorGrid, row: usize) -> Option<CachedGridRow> {
    Some(CachedGridRow {
        cells: grid.lines.get(row)?.clone(),
        wrapped: grid.wrapped.get(row).copied().unwrap_or(false),
    })
}

/// Extracts an absolute mouse selection from live viewport rows and retained cached rows.
///
/// Live rows take precedence over cached copies. Every selected line must be available: returning
/// `None` prevents a partially scrolled-away selection from silently replacing the clipboard with
/// truncated text. The Workspace bounds the selection-scoped cache and resets it whenever terminal
/// coordinates become unrelated (terminal/screen/column changes or a history-epoch advance).
#[must_use]
pub fn absolute_selection_text(
    grid: &MirrorGrid,
    cache: &BTreeMap<u64, CachedGridRow>,
    selection: AbsoluteCellSelection,
) -> Option<String> {
    let selection = AbsoluteCellSelection::new(selection.start, selection.end);
    let line_count = selection
        .end
        .line
        .checked_sub(selection.start.line)?
        .checked_add(1)?;
    if usize::try_from(line_count).ok()? > cache.len().saturating_add(grid.lines.len()) {
        return None;
    }

    let base = viewport_base(grid);
    let bottom = viewport_last(grid);
    let mut text = String::new();
    let mut previous_wrapped = None;
    for line in selection.start.line..=selection.end.line {
        let live_row = bottom
            .filter(|bottom| line >= base && line <= *bottom)
            .and_then(|_| usize::try_from(line - base).ok());
        let (cells, wrapped) = if let Some(row) = live_row {
            (
                grid.lines.get(row)?,
                grid.wrapped.get(row).copied().unwrap_or(false),
            )
        } else {
            let cached = cache.get(&line)?;
            (&cached.cells, cached.wrapped)
        };
        let start = if line == selection.start.line {
            selection.start.col
        } else {
            0
        };
        let end = if line == selection.end.line {
            selection.end.col.saturating_add(1)
        } else {
            usize::from(grid.cols)
        };
        append_selected_row(&mut text, previous_wrapped, cells, start, end, wrapped);
        previous_wrapped = Some(wrapped);
    }
    Some(text)
}

fn append_selected_row(
    text: &mut String,
    previous_wrapped: Option<bool>,
    cells: &[ProtoCell],
    start: usize,
    end: usize,
    wrapped: bool,
) {
    if previous_wrapped == Some(false) {
        text.push('\n');
    }
    text.push_str(&selected_row_text(cells, start, end, !wrapped));
}

fn selected_row_text(line: &[ProtoCell], start: usize, end: usize, trim_end: bool) -> String {
    let mut text = String::new();
    let mut col = 0usize;
    for cell in line {
        let width = cell_columns(cell);
        if width == 0 {
            continue;
        }
        let cell_end = col.saturating_add(width);
        if col < end && cell_end > start {
            text.push_str(cell.text.as_str());
        }
        col = cell_end;
        if col >= end {
            break;
        }
    }
    if trim_end {
        text.truncate(text.trim_end().len());
    }
    text
}

/// The kit badges a `FrameUpdate`'s VT modes map onto (§3.6, DESIGN-SYSTEM "Terminal modes").
///
/// `TerminalModes` is zero-suppressed, so a plain shell produces an empty list and no badges.
/// The Kitty keyboard flags and focus-event reporting have no badge: neither changes what a
/// documented Fleet key does, which is the bar the badge row is drawn to.
#[must_use]
pub fn grid_modes(modes: &TerminalModes) -> Vec<KitTerminalMode> {
    let mut active = Vec::with_capacity(4);
    if modes.alt_screen {
        active.push(KitTerminalMode::AltScreen);
    }
    if modes.mouse_reporting {
        active.push(KitTerminalMode::MouseReporting);
    }
    if modes.bracketed_paste {
        active.push(KitTerminalMode::BracketedPaste);
    }
    if modes.app_cursor_keys {
        active.push(KitTerminalMode::ApplicationCursor);
    }
    active
}

/// Resolves a foreground color against the theme's terminal palette.
#[must_use]
pub fn foreground(color: Color, theme: &Theme) -> Hsla {
    match color {
        Color::Default => theme.terminal.foreground,
        Color::Palette(index) => theme.terminal.color(index),
        Color::Rgb { r, g, b } => rgb(r, g, b),
    }
}

/// Resolves a background color, mapping `Default` to "paint nothing" as the kit expects.
#[must_use]
pub fn background(color: Color, theme: &Theme) -> Option<Hsla> {
    match color {
        Color::Default => None,
        Color::Palette(index) => Some(theme.terminal.color(index)),
        Color::Rgb { r, g, b } => Some(rgb(r, g, b)),
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Hsla {
    Hsla::from(Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    })
}

/// The underline style the three mutually exclusive underline flags encode.
#[must_use]
pub fn underline_style(attrs: CellAttrs) -> UnderlineStyle {
    if attrs.contains(CellAttrs::CURLY_UNDERLINE) {
        UnderlineStyle::Curly
    } else if attrs.contains(CellAttrs::DOUBLE_UNDERLINE) {
        UnderlineStyle::Double
    } else if attrs.contains(CellAttrs::UNDERLINE) {
        UnderlineStyle::Single
    } else {
        UnderlineStyle::None
    }
}

/// Converts one wire cell into the kit's painted cell.
#[must_use]
pub fn grid_cell(cell: &ProtoCell, theme: &Theme) -> GridCell {
    let attrs = cell.attrs;
    GridCell {
        text: cell.text.as_str().to_owned().into(),
        fg: foreground(cell.fg, theme),
        bg: background(cell.bg, theme),
        bold: attrs.contains(CellAttrs::BOLD),
        dim: attrs.contains(CellAttrs::DIM),
        italic: attrs.contains(CellAttrs::ITALIC),
        underline: underline_style(attrs),
        underline_color: cell
            .underline_color
            .and_then(|color| background(color, theme)),
        strikethrough: attrs.contains(CellAttrs::STRIKETHROUGH),
        inverse: attrs.contains(CellAttrs::INVERSE),
        blink: attrs.contains(CellAttrs::BLINK),
        invisible: attrs.contains(CellAttrs::INVISIBLE),
        width: match cell.width {
            ProtoWidth::Narrow => CellWidth::Narrow,
            ProtoWidth::Wide => CellWidth::Wide,
            ProtoWidth::Spacer => CellWidth::Spacer,
        },
    }
}

/// Converts the whole mirror into painted rows.
///
/// A row the daemon has never sent is painted as an empty row rather than as blanks: the kit
/// lays rows out at a fixed cell height, so an empty row already occupies the right space and
/// costs no text shaping.
#[must_use]
pub fn grid_rows(grid: &MirrorGrid, theme: &Theme) -> Vec<GridRow> {
    grid.lines
        .iter()
        .map(|line| GridRow::new(line.iter().map(|cell| grid_cell(cell, theme))))
        .collect()
}

/// The kit cursor for a mirror grid.
///
/// `focused` is the app's own focus, not the PTY's: an unfocused terminal draws the hollow
/// cursor so a window that is not taking keys cannot look like one that is.
#[must_use]
pub fn grid_cursor(grid: &MirrorGrid, focused: bool) -> GridCursor {
    GridCursor {
        row: grid.cursor.row as usize,
        col: grid.cursor.col as usize,
        visible: grid.cursor.visible,
        shape: if focused {
            match grid.cursor.shape {
                ProtoShape::Block => CursorShape::Block,
                ProtoShape::Bar => CursorShape::Bar,
                ProtoShape::Underline => CursorShape::Underline,
            }
        } else {
            CursorShape::Hollow
        },
    }
}

/// The absolute scrollback line the viewport's top row is showing.
///
/// `ViewportInfo::scrollback_len` counts history above the screen and `offset` how far back the
/// viewport was pulled, so the top row sits at `scrollback_len - offset`. This coordinate remains
/// stable within one `history_epoch`; the daemon advances that epoch when its tracked oldest row is
/// discarded, history shrinks, or output arrives at the nominal bound. Viewport-only frames keep
/// the epoch stable, so scrolling cannot invalidate a Scroll-mode selection.
///
/// Scroll-mode selections are anchored in this space and never in viewport rows: a viewport row
/// number means a different line after every scroll, so an anchor expressed that way silently
/// re-points at whatever moved under it — which is why a `v`, `PageUp`, `y` used to yank the
/// wrong single line.
#[must_use]
pub fn viewport_base(grid: &MirrorGrid) -> u64 {
    let scrollback = grid.viewport.scrollback_len as u64;
    let offset = grid.viewport.offset as u64;
    scrollback.saturating_sub(offset)
}

/// The last absolute line the viewport is showing, or `None` for an empty grid.
#[must_use]
pub fn viewport_last(grid: &MirrorGrid) -> Option<u64> {
    (grid.rows > 0).then(|| viewport_base(grid) + u64::from(grid.rows - 1))
}

/// A line-wise selection between two **absolute** scrollback lines, in either order.
///
/// Fleet's copy mode selects whole lines: `v` anchors a line, movement extends the range and
/// `y` yanks it. A column-wise selection would need a caret the spec does not draw, so the head
/// column is simply the end of the head row.
///
/// The result is in viewport rows, clipped to what is on screen — the selection itself may run
/// far past both edges — and is `None` when none of it is visible.
#[must_use]
pub fn line_selection(grid: &MirrorGrid, anchor: u64, head: u64) -> Option<GridSelection> {
    let base = viewport_base(grid);
    let bottom = viewport_last(grid)?;
    let (first, last) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    if last < base || first > bottom {
        return None;
    }
    let start_row = first.max(base) - base;
    let end_row = last.min(bottom) - base;
    let end = grid
        .lines
        .get(usize::try_from(end_row).unwrap_or(usize::MAX))
        .map_or(0, |line| line.iter().map(cell_columns).sum());
    Some(GridSelection::new(
        usize::try_from(start_row).unwrap_or(0),
        0,
        usize::try_from(end_row).unwrap_or(0),
        end,
    ))
}

fn cell_columns(cell: &ProtoCell) -> usize {
    match cell.width {
        ProtoWidth::Narrow => 1,
        ProtoWidth::Wide => 2,
        ProtoWidth::Spacer => 0,
    }
}

/// The text a selection yanks, one line per selected scrollback line.
///
/// `history` is the client's record of every line it has painted since the selection was
/// anchored, keyed by absolute scrollback line. The daemon mirrors only the *viewport*, so a
/// selection that spans more than one screen can only be assembled from what this client saw —
/// which is every line the user scrolled the selection over.
///
/// Trailing blanks are what a terminal pads short rows with; keeping them would paste a
/// rectangle of spaces into the next shell prompt, so they are stripped on the way in.
#[must_use]
pub fn selection_text(history: &BTreeMap<u64, String>, anchor: u64, head: u64) -> String {
    let (first, last) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    history
        .range(first..=last)
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The pixel size of one cell, from the theme metrics.
#[must_use]
pub fn cell_size(theme: &Theme) -> Size<Pixels> {
    gpui::size(theme.metrics.cell_w, theme.metrics.cell_h)
}

/// The 2 px amber reminder that `ctrl-s z` hid the header and the tab strip (§3.6).
pub fn zoom_bar(theme: &Theme) -> impl IntoElement {
    gpui::div()
        .h(px(2.0))
        .w_full()
        .flex_none()
        .bg(theme.colors.warning)
}

#[cfg(test)]
mod tests {

    #[test]
    fn frame_modes_become_zero_suppressed_badges() {
        let quiet = TerminalModes::default();
        assert!(
            grid_modes(&quiet).is_empty(),
            "a plain shell costs no badge row"
        );

        let vim = TerminalModes {
            alt_screen: true,
            mouse_reporting: false,
            bracketed_paste: true,
            focus_events: true,
            kitty_keyboard_flags: 1,
            app_cursor_keys: true,
        };
        assert_eq!(
            grid_modes(&vim),
            vec![
                KitTerminalMode::AltScreen,
                KitTerminalMode::BracketedPaste,
                KitTerminalMode::ApplicationCursor,
            ],
            "focus events and Kitty flags change no documented Fleet key, so they get no badge"
        );

        let pager = TerminalModes {
            mouse_reporting: true,
            ..TerminalModes::default()
        };
        assert_eq!(grid_modes(&pager), vec![KitTerminalMode::MouseReporting]);
    }
    use fleet_proto::terminal::{CursorState, RowUpdate, TerminalModes, ViewportInfo};
    use fleet_ui_kit::ThemeMode;

    use super::*;

    fn theme() -> Theme {
        Theme::for_mode(ThemeMode::Dark)
    }

    fn cell(text: &str) -> ProtoCell {
        ProtoCell {
            text: text.into(),
            fg: Color::Default,
            bg: Color::Default,
            underline_color: None,
            attrs: CellAttrs::empty(),
            width: ProtoWidth::Narrow,
        }
    }

    fn wide_cell(text: &str) -> ProtoCell {
        ProtoCell {
            width: ProtoWidth::Wide,
            ..cell(text)
        }
    }

    fn spacer() -> ProtoCell {
        ProtoCell {
            width: ProtoWidth::Spacer,
            ..cell("")
        }
    }

    fn grid_with(rows: &[&str]) -> MirrorGrid {
        grid_scrolled(rows, 0, 0)
    }

    /// A grid whose viewport sits `scrollback_len - offset` lines into the scrollback.
    fn grid_scrolled(rows: &[&str], scrollback_len: usize, offset: usize) -> MirrorGrid {
        let cols = rows
            .iter()
            .map(|row| row.chars().count())
            .max()
            .unwrap_or(0);
        let mut grid = MirrorGrid::new(cols as u16, rows.len() as u16);
        grid.apply(&fleet_proto::terminal::FrameUpdate {
            terminal: fleet_core::ids::TerminalId(1),
            seq: 1,
            cols: cols as u16,
            rows: rows.len() as u16,
            full: true,
            rows_changed: rows
                .iter()
                .enumerate()
                .map(|(index, row)| RowUpdate {
                    index: index as u16,
                    cells: row.chars().map(|c| cell(&c.to_string())).collect(),
                    wrapped: false,
                })
                .collect(),
            cursor: CursorState {
                row: 0,
                col: 0,
                visible: true,
                shape: ProtoShape::Block,
            },
            viewport: ViewportInfo {
                scrollback_len,
                offset,
                history_epoch: 0,
            },
            modes: TerminalModes::default(),
            title: None,
        });
        grid
    }

    fn cache_row(text: &str, wrapped: bool) -> CachedGridRow {
        let mut grid = grid_with(&[text]);
        grid.wrapped[0] = wrapped;
        cached_grid_row(&grid, 0).unwrap_or_else(|| panic!("fixture row must exist"))
    }

    #[test]
    fn grid_size_subtracts_the_padding_on_both_axes() {
        let cell = gpui::size(px(10.0), px(20.0));
        // 216 px wide minus 2 × 8 px of padding is exactly 20 cells.
        let (cols, rows) = grid_size(gpui::size(px(216.0), px(416.0)), cell);
        assert_eq!((cols, rows), (20, 20));
    }

    #[test]
    fn grid_size_never_returns_an_empty_pty() {
        let cell = gpui::size(px(10.0), px(20.0));
        let (cols, rows) = grid_size(gpui::size(px(0.0), px(0.0)), cell);
        assert_eq!((cols, rows), (MIN_COLS, MIN_ROWS));
    }

    #[test]
    fn grid_size_survives_a_degenerate_cell() {
        let (cols, rows) = grid_size(
            gpui::size(px(100.0), px(100.0)),
            gpui::size(px(0.0), px(0.0)),
        );
        assert_eq!((cols, rows), (MIN_COLS, MIN_ROWS));
    }

    #[test]
    fn underline_flags_are_mutually_exclusive_in_priority_order() {
        assert_eq!(underline_style(CellAttrs::empty()), UnderlineStyle::None);
        assert_eq!(
            underline_style(CellAttrs::UNDERLINE),
            UnderlineStyle::Single
        );
        assert_eq!(
            underline_style(CellAttrs::UNDERLINE | CellAttrs::DOUBLE_UNDERLINE),
            UnderlineStyle::Double
        );
        assert_eq!(
            underline_style(CellAttrs::DOUBLE_UNDERLINE | CellAttrs::CURLY_UNDERLINE),
            UnderlineStyle::Curly
        );
    }

    #[test]
    fn default_background_paints_nothing_and_default_foreground_uses_the_palette() {
        let theme = theme();
        assert_eq!(background(Color::Default, &theme), None);
        assert_eq!(
            foreground(Color::Default, &theme),
            theme.terminal.foreground
        );
        assert_eq!(
            foreground(Color::Palette(1), &theme),
            theme.terminal.color(1)
        );
    }

    #[test]
    fn attributes_survive_the_round_trip_into_the_kit() {
        let theme = theme();
        let mut source = cell("x");
        source.attrs = CellAttrs::BOLD | CellAttrs::INVERSE | CellAttrs::DIM;
        source.width = ProtoWidth::Wide;
        let converted = grid_cell(&source, &theme);
        assert!(converted.bold && converted.inverse && converted.dim);
        assert!(!converted.italic);
        assert_eq!(converted.width, CellWidth::Wide);
    }

    #[test]
    fn an_unfocused_terminal_draws_a_hollow_cursor() {
        let grid = grid_with(&["ab"]);
        assert_eq!(grid_cursor(&grid, true).shape, CursorShape::Block);
        assert_eq!(grid_cursor(&grid, false).shape, CursorShape::Hollow);
    }

    #[test]
    fn a_line_selection_covers_whole_rows_in_either_direction() {
        let grid = grid_with(&["one", "two", "three"]);
        let down = line_selection(&grid, 0, 2).unwrap_or_else(|| panic!("off screen"));
        let up = line_selection(&grid, 2, 0).unwrap_or_else(|| panic!("off screen"));
        assert_eq!(down, up);
        assert_eq!((down.start_row, down.start_col), (0, 0));
        assert_eq!((down.end_row, down.end_col), (2, 5));
    }

    #[test]
    fn cell_selection_is_inclusive_and_order_independent() {
        let forward = cell_selection(CellPoint::new(1, 2), CellPoint::new(2, 4));
        let backward = cell_selection(CellPoint::new(2, 4), CellPoint::new(1, 2));
        assert_eq!(forward, backward);
        assert_eq!(
            forward,
            GridSelection::new(1, 2, 2, 5),
            "the head cell is included by the half-open painted range"
        );
    }

    #[test]
    fn absolute_cell_selection_clips_at_both_viewport_edges() {
        let grid = grid_scrolled(&["aaaaaa", "bbbbbb", "cccccc"], 110, 10);
        assert_eq!(viewport_base(&grid), 100);

        assert_eq!(
            viewport_cell_selection(
                &grid,
                AbsoluteCellPoint::new(99, 4),
                AbsoluteCellPoint::new(101, 2),
            ),
            Some(GridSelection::new(0, 0, 1, 3)),
            "an anchor above the viewport clips to its first cell"
        );
        assert_eq!(
            viewport_cell_selection(
                &grid,
                AbsoluteCellPoint::new(101, 2),
                AbsoluteCellPoint::new(103, 4),
            ),
            Some(GridSelection::new(1, 2, 2, 6)),
            "a head below the viewport clips to its last cell"
        );
        assert_eq!(
            viewport_cell_selection(
                &grid,
                AbsoluteCellPoint::new(103, 4),
                AbsoluteCellPoint::new(101, 2),
            ),
            Some(GridSelection::new(1, 2, 2, 6)),
            "backwards selections clip identically"
        );
    }

    #[test]
    fn absolute_cell_selection_survives_a_viewport_base_shift() {
        let anchor = AbsoluteCellPoint::new(101, 1);
        let head = AbsoluteCellPoint::new(102, 3);
        let before = grid_scrolled(&["aaaaaa", "bbbbbb", "cccccc"], 110, 10);
        let after = grid_scrolled(&["bbbbbb", "cccccc", "dddddd"], 111, 10);

        assert_eq!(
            viewport_cell_selection(&before, anchor, head),
            Some(GridSelection::new(1, 1, 2, 4))
        );
        assert_eq!(
            viewport_cell_selection(&after, anchor, head),
            Some(GridSelection::new(0, 1, 1, 4)),
            "the same absolute endpoints move up when output advances the viewport"
        );
    }

    #[test]
    fn off_screen_absolute_cell_selection_paints_nothing_without_losing_its_endpoints() {
        let grid = grid_scrolled(&["aaaaaa", "bbbbbb", "cccccc"], 110, 10);
        let selection = (AbsoluteCellPoint::new(90, 1), AbsoluteCellPoint::new(91, 3));

        assert_eq!(
            viewport_cell_selection(&grid, selection.0, selection.1),
            None
        );
        assert_eq!(selection.0, AbsoluteCellPoint::new(90, 1));
        assert_eq!(selection.1, AbsoluteCellPoint::new(91, 3));
    }

    #[test]
    fn word_selection_stops_at_character_classes() {
        let grid = grid_with(&["one_two:: three"]);
        assert_eq!(
            word_selection(&grid, CellPoint::new(0, 4)),
            Some(GridSelection::new(0, 0, 0, 7))
        );
        assert_eq!(
            word_selection(&grid, CellPoint::new(0, 7)),
            Some(GridSelection::new(0, 7, 0, 9))
        );
        assert_eq!(
            word_selection(&grid, CellPoint::new(0, 9)),
            Some(GridSelection::new(0, 9, 0, 10))
        );
    }

    #[test]
    fn word_drag_ignores_jitter_then_extends_to_the_hovered_word_boundary() {
        let grid = grid_with(&["one two"]);
        let initiating = AbsoluteCellPoint::new(0, 1);
        let initial =
            absolute_selection_at(&grid, CellPoint::new(0, 1), SelectionGranularity::Word)
                .unwrap_or_else(|| panic!("word missing"));

        assert_eq!(
            extend_absolute_selection(
                &grid,
                initial,
                initiating,
                CellPoint::new(0, 1),
                SelectionGranularity::Word,
            ),
            Some(initial)
        );
        assert_eq!(
            extend_absolute_selection(
                &grid,
                initial,
                initiating,
                CellPoint::new(0, 5),
                SelectionGranularity::Word,
            ),
            Some(AbsoluteCellSelection::new(
                AbsoluteCellPoint::new(0, 0),
                AbsoluteCellPoint::new(0, 6),
            ))
        );
    }

    #[test]
    fn line_drag_extends_by_whole_visual_rows() {
        let grid = grid_with(&["aaaa", "bbbb", "cccc"]);
        let initiating = AbsoluteCellPoint::new(1, 2);
        let initial =
            absolute_selection_at(&grid, CellPoint::new(1, 2), SelectionGranularity::Line)
                .unwrap_or_else(|| panic!("line missing"));

        assert_eq!(
            extend_absolute_selection(
                &grid,
                initial,
                initiating,
                CellPoint::new(2, 1),
                SelectionGranularity::Line,
            ),
            Some(AbsoluteCellSelection::new(
                AbsoluteCellPoint::new(1, 0),
                AbsoluteCellPoint::new(2, 3),
            ))
        );
    }

    #[test]
    fn mouse_position_maps_through_grid_padding_and_clamps() {
        let bounds = Bounds::new(
            gpui::point(px(100.0), px(200.0)),
            gpui::size(px(116.0), px(76.0)),
        );
        let cell = gpui::size(px(10.0), px(20.0));
        assert_eq!(
            cell_at_position(bounds, gpui::point(px(133.0), px(249.0)), cell, 10, 3),
            Some(CellPoint::new(2, 2))
        );
        assert_eq!(
            cell_at_position(bounds, gpui::point(px(100.0), px(200.0)), cell, 10, 3),
            Some(CellPoint::new(0, 0)),
            "padding maps to the nearest grid cell"
        );
    }

    #[test]
    fn grid_selection_text_orders_rows_and_trims_each_one() {
        let mut grid = MirrorGrid::new(6, 2);
        grid.lines = vec![
            "abcdef".chars().map(|c| cell(&c.to_string())).collect(),
            "gh    ".chars().map(|c| cell(&c.to_string())).collect(),
        ];
        let selection = cell_selection(CellPoint::new(1, 1), CellPoint::new(0, 2));
        assert_eq!(grid_selection_text(&grid, selection), "cdef\ngh");
    }

    #[test]
    fn grid_selection_text_joins_three_soft_wrapped_rows_as_one_line() {
        let mut grid = MirrorGrid::new(4, 3);
        grid.lines = ["abc ", "def ", "ghi "]
            .map(|row| row.chars().map(|c| cell(&c.to_string())).collect())
            .to_vec();
        grid.wrapped = vec![true, true, false];

        assert_eq!(
            grid_selection_text(&grid, GridSelection::new(0, 0, 2, 4)),
            "abc def ghi"
        );
    }

    #[test]
    fn grid_selection_text_emits_a_wide_cell_once() {
        let mut grid = MirrorGrid::new(6, 1);
        grid.lines = vec![vec![
            cell("a"),
            wide_cell("漢"),
            spacer(),
            cell("b"),
            cell(" "),
            cell(" "),
        ]];
        assert_eq!(
            grid_selection_text(&grid, GridSelection::new(0, 0, 0, 6)),
            "a漢b"
        );
        assert_eq!(
            grid_selection_text(&grid, GridSelection::new(0, 2, 0, 3)),
            "漢",
            "selecting the second half of a wide cell still emits one grapheme"
        );
    }

    #[test]
    fn absolute_selection_uses_cache_for_the_offscreen_prefix() {
        let grid = grid_scrolled(&["ef  ", "gh  "], 11, 0);
        let cache = BTreeMap::from([(10, cache_row("abcd", true))]);
        let selection = AbsoluteCellSelection::new(
            AbsoluteCellPoint::new(10, 1),
            AbsoluteCellPoint::new(12, 3),
        );

        assert_eq!(
            absolute_selection_text(&grid, &cache, selection),
            Some("bcdef\ngh".to_owned())
        );
    }

    #[test]
    fn absolute_selection_can_copy_fully_offscreen_cached_rows() {
        let grid = grid_scrolled(&["live"], 20, 0);
        let cache = BTreeMap::from([
            (10, cache_row("abcd", false)),
            (11, cache_row("efgh", false)),
        ]);
        let selection = AbsoluteCellSelection::new(
            AbsoluteCellPoint::new(10, 1),
            AbsoluteCellPoint::new(11, 1),
        );

        assert_eq!(
            absolute_selection_text(&grid, &cache, selection),
            Some("bcd\nef".to_owned())
        );
    }

    #[test]
    fn absolute_selection_fails_instead_of_truncating_a_missing_row() {
        let grid = grid_scrolled(&["live"], 20, 0);
        let cache = BTreeMap::from([
            (10, cache_row("abcd", false)),
            (12, cache_row("ijkl", false)),
        ]);
        let selection = AbsoluteCellSelection::new(
            AbsoluteCellPoint::new(10, 0),
            AbsoluteCellPoint::new(12, 3),
        );

        assert_eq!(absolute_selection_text(&grid, &cache, selection), None);
    }

    #[test]
    fn the_viewport_base_is_where_the_top_row_sits_in_the_scrollback() {
        // 900 lines of history, scrolled back 10: the top row is line 890.
        let grid = grid_scrolled(&["a", "b", "c"], 900, 10);
        assert_eq!(viewport_base(&grid), 890);
        assert_eq!(viewport_last(&grid), Some(892));
        // At the live bottom the viewport starts right after the history.
        let live = grid_scrolled(&["a", "b", "c"], 900, 0);
        assert_eq!(viewport_base(&live), 900);
    }

    #[test]
    fn a_selection_anchored_before_the_viewport_still_paints_what_is_visible() {
        // terminal-008: `v` on line 890, then a page up — the anchor is above the screen now.
        let grid = grid_scrolled(&["d", "e", "f"], 900, 13);
        assert_eq!(viewport_base(&grid), 887);
        let painted = line_selection(&grid, 890, 887).unwrap_or_else(|| panic!("off screen"));
        assert_eq!(
            (painted.start_row, painted.end_row),
            (0, 2),
            "the visible part of the selection is clipped to the viewport, not lost"
        );

        // A selection entirely above or below the viewport paints nothing at all.
        assert_eq!(line_selection(&grid, 400, 401), None);
        assert_eq!(line_selection(&grid, 1_000, 1_001), None);
    }

    #[test]
    fn yanked_text_spans_every_line_the_selection_covers() {
        // terminal-008: the yank reads the client's scrollback record, not the viewport, so a
        // selection that started three pages up still yields all of its lines.
        let mut history = BTreeMap::new();
        for (offset, line) in ["one", "two", "three", "four"].into_iter().enumerate() {
            history.insert(890 + offset as u64, line.to_owned());
        }
        assert_eq!(selection_text(&history, 890, 893), "one\ntwo\nthree\nfour");
        assert_eq!(
            selection_text(&history, 893, 890),
            "one\ntwo\nthree\nfour",
            "the range is the same in either direction"
        );
        assert_eq!(selection_text(&history, 891, 892), "two\nthree");
        assert_eq!(selection_text(&history, 500, 501), "");
    }

    #[test]
    fn rows_are_painted_one_kit_row_per_mirror_row() {
        let theme = theme();
        let grid = grid_with(&["ab", "c"]);
        let rows = grid_rows(&grid, &theme);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].columns(), 2);
        assert_eq!(rows[1].columns(), 1);
    }
}
