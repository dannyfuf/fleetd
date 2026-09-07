use super::*;
use std::collections::BTreeMap;

/// One cell in the visible terminal viewport.
///
/// Columns are terminal columns rather than indices into [`MirrorGrid::lines`]: a wide cell
/// occupies two columns while its spacer cell occupies none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CellPoint {
    /// Viewport row, zero based.
    pub row: usize,
    /// Terminal column, zero based.
    pub col: usize,
}

impl CellPoint {
    /// A viewport cell.
    #[must_use]
    pub(crate) const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// One terminal cell addressed in the absolute scrollback coordinate space.
///
/// Unlike a viewport row, `line` continues to identify the same terminal line when new output
/// moves the viewport. Mouse selections use this type so repaints do not retarget their ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct AbsoluteCellPoint {
    /// Absolute scrollback line, zero based.
    pub line: u64,
    /// Terminal column, zero based.
    pub col: usize,
}

impl AbsoluteCellPoint {
    /// An absolute terminal cell.
    #[must_use]
    pub(crate) const fn new(line: u64, col: usize) -> Self {
        Self { line, col }
    }
}

/// Unit a mouse gesture expands by after its initiating click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionGranularity {
    /// Individual terminal cells.
    Cell,
    /// Terminal-friendly word runs.
    Word,
    /// Complete visual rows.
    Line,
}

/// Inclusive absolute endpoints of a stream selection, normalized in reading order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AbsoluteCellSelection {
    /// First selected cell.
    pub start: AbsoluteCellPoint,
    /// Last selected cell.
    pub end: AbsoluteCellPoint,
}

impl AbsoluteCellSelection {
    /// Builds a normalized inclusive selection.
    #[must_use]
    pub(crate) fn new(first: AbsoluteCellPoint, second: AbsoluteCellPoint) -> Self {
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
pub(crate) fn cell_selection(anchor: CellPoint, head: CellPoint) -> GridSelection {
    let (first, last) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    GridSelection::new(first.row, first.col, last.row, last.col.saturating_add(1))
}

/// Selects the cell, word, or line at one viewport point in absolute coordinates.
#[must_use]
pub(crate) fn absolute_selection_at(
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
pub(crate) fn extend_absolute_selection(
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
pub(crate) fn viewport_cell_selection(
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
pub(crate) fn word_selection(grid: &MirrorGrid, point: CellPoint) -> Option<GridSelection> {
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

/// A mirrored viewport row retained for an absolute mouse selection.
///
/// Cells, rather than an already-trimmed string, are cached while a selection covers the row so
/// partial first/last rows and wide cells retain the same extraction semantics off-screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CachedGridRow {
    cells: Vec<ProtoCell>,
    wrapped: bool,
}

impl CachedGridRow {
    pub(crate) fn matches(&self, grid: &MirrorGrid, row: usize) -> bool {
        grid.lines.get(row) == Some(&self.cells)
            && self.wrapped == grid.wrapped.get(row).copied().unwrap_or(false)
    }
}

/// Clones one visible row into the bounded selection cache owned by the Workspace.
#[must_use]
pub(crate) fn cached_grid_row(grid: &MirrorGrid, row: usize) -> Option<CachedGridRow> {
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
pub(crate) fn absolute_selection_text(
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
    let row_start = text.len();
    let mut col = 0usize;
    for cell in cells {
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
    if !wrapped {
        text.truncate(row_start + text[row_start..].trim_end().len());
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
pub(crate) fn viewport_base(grid: &MirrorGrid) -> u64 {
    let scrollback = grid.viewport.scrollback_len as u64;
    let offset = grid.viewport.offset as u64;
    scrollback.saturating_sub(offset)
}

/// The last absolute line the viewport is showing, or `None` for an empty grid.
#[must_use]
pub(crate) fn viewport_last(grid: &MirrorGrid) -> Option<u64> {
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
pub(crate) fn line_selection(grid: &MirrorGrid, anchor: u64, head: u64) -> Option<GridSelection> {
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

/// The text a selection yanks, preserving soft wraps between selected scrollback rows.
///
/// `history` is the client's record of every line it has painted since the selection was
/// anchored, keyed by absolute scrollback line. The daemon mirrors only the *viewport*, so a
/// selection that spans more than one screen can only be assembled from what this client saw —
/// which is every line the user scrolled the selection over.
///
/// Every row must still be retained. A missing row returns `None` instead of silently joining
/// the rows around a gap and claiming that truncated text was copied.
#[must_use]
pub(crate) fn try_selection_text(
    history: &BTreeMap<u64, CachedGridRow>,
    anchor: u64,
    head: u64,
) -> Option<String> {
    let (first, last) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    let line_count = last.checked_sub(first)?.checked_add(1)?;
    if usize::try_from(line_count).ok()? > history.len() {
        return None;
    }
    let mut text = String::new();
    let mut previous_wrapped = None;
    for line in first..=last {
        let row = history.get(&line)?;
        append_selected_row(
            &mut text,
            previous_wrapped,
            &row.cells,
            0,
            usize::MAX,
            row.wrapped,
        );
        previous_wrapped = Some(row.wrapped);
    }
    Some(text)
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::terminal::surface::{TerminalRowCache, cache_selected_grid_rows};

    #[test]
    fn cursor_frames_do_not_clone_retained_selection_cells() {
        let mut grid = MirrorGrid::new(1, 1);
        grid.lines[0] = vec![ProtoCell {
            text: "x".into(),
            fg: Color::Default,
            bg: Color::Default,
            attrs: CellAttrs::empty(),
            underline_color: None,
            width: ProtoWidth::Narrow,
        }];
        grid.seq = 1;
        let mut cache = TerminalRowCache {
            cols: 1,
            alt_screen: false,
            history_epoch: 0,
            last_seq: 0,
            last_viewport_base: u64::MAX,
            rows: BTreeMap::new(),
        };
        cache_selected_grid_rows(&mut cache, &grid, 0, Some((0, 0)));
        let cells = cache.rows[&0].cells.as_ptr();
        grid.seq = 2;
        grid.cursor.visible = false;
        cache_selected_grid_rows(&mut cache, &grid, 0, Some((0, 0)));
        assert_eq!(cache.rows[&0].cells.as_ptr(), cells);
        grid.seq = 3;
        grid.lines[0][0].text = "y".into();
        cache_selected_grid_rows(&mut cache, &grid, 0, Some((0, 0)));
        assert_eq!(cache.rows[&0].cells[0].text.as_str(), "y");
        assert_ne!(cache.rows[&0].cells.as_ptr(), cells);
    }
}
