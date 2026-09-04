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
//! **Minimal render.** This implementation lays out one `div` per run of same-styled cells
//! inside one `div` per row, which is correct and honest but allocates per frame. The real
//! implementation is a custom `gpui::Element` that takes `relative(1.)` height, tracks its own
//! `scroll_top` and paints quads + shaped lines directly (Zed's `TerminalElement` does exactly
//! this, and uses neither `uniform_list` nor `list`). The public API below does not change
//! when that lands.

use gpui::{App, Hsla, SharedString, Window, div, prelude::*, px};

use crate::{
    text::styled_with,
    theme::{ActiveTheme, Theme},
};

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
        self.cells
            .iter()
            .map(|c| c.width.columns() as usize)
            .sum()
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

/// The painted cell grid.
#[derive(IntoElement)]
pub struct TerminalGrid {
    rows: Vec<GridRow>,
    cursor: Option<GridCursor>,
    selection: Option<GridSelection>,
    focused: bool,
    padding: Option<gpui::Pixels>,
    scrollback: Option<(usize, usize)>,
}

impl TerminalGrid {
    /// A grid over the mirror rows.
    pub fn new(rows: impl IntoIterator<Item = GridRow>) -> Self {
        Self {
            rows: rows.into_iter().collect(),
            cursor: None,
            selection: None,
            focused: true,
            padding: None,
            scrollback: None,
        }
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
    pub fn padding(mut self, padding: gpui::Pixels) -> Self {
        self.padding = Some(padding);
        self
    }

    /// `viewport { offset, scrollback_len }`. When `offset > 0` the grid paints a
    /// [`ScrollbackBadge`](crate::components::ScrollbackBadge) in its top-right corner, so a
    /// scrolled-back viewport is never mistaken for a live one.
    pub fn scrollback(mut self, offset: usize, len: usize) -> Self {
        self.scrollback = Some((offset, len));
        self
    }

    /// How many rows the grid holds.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }
}

impl RenderOnce for TerminalGrid {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let padding = self.padding.unwrap_or(px(8.0));
        let cursor = self.cursor;
        let selection = self.selection.map(GridSelection::normalized);
        let cell_w = theme.metrics.cell_w;
        let cell_h = theme.metrics.cell_h;
        let selection_color = theme.terminal.selection;
        let cursor_color = theme.terminal.cursor;
        let scrollback = self
            .scrollback
            .filter(|(offset, _)| *offset > 0)
            .map(|(offset, len)| super::ScrollbackBadge::new(offset, len));

        let rows = self.rows.into_iter().enumerate().map(|(row_ix, row)| {
            let row_columns = row.columns();
            let mut runs: Vec<(String, GridCell)> = Vec::new();
            for cell in row.cells.iter() {
                match runs.last_mut() {
                    Some((text, style)) if style.same_style(cell) => {
                        text.push_str(cell.text.as_ref());
                    }
                    _ => runs.push((cell.text.to_string(), cell.clone())),
                }
            }
            let cursor_here = cursor.filter(|c| c.visible && c.row == row_ix);
            let span = selection.and_then(|s| s.span_in_row(row_ix, row_columns));
            let theme = theme.clone();
            div()
                .relative()
                .flex()
                .flex_row()
                .h(cell_h)
                .children(span.map(|(start, end)| {
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(cell_w * (start as f32))
                        .w(cell_w * ((end - start) as f32))
                        .h(cell_h)
                        .bg(selection_color)
                }))
                .children(runs.into_iter().map(move |(text, style)| {
                    let (fg, bg) = style.resolve(&theme);
                    let underline_color = style.underline_color.unwrap_or(fg);
                    styled_with(div(), theme.text.data, &theme)
                        .relative()
                        .flex_none()
                        .h(cell_h)
                        .text_color(fg)
                        .when_some(bg, |el, bg| el.bg(bg))
                        .when(style.bold, |el| el.font_weight(gpui::FontWeight::BOLD))
                        .when(style.italic, |el| el.italic())
                        .when(style.strikethrough, |el| el.line_through())
                        .when(style.underline.is_some(), |el| {
                            el.underline()
                                .text_decoration_color(underline_color)
                                .map(|el| match style.underline {
                                    UnderlineStyle::Double => {
                                        el.text_decoration_2().text_decoration_solid()
                                    }
                                    UnderlineStyle::Curly => {
                                        el.text_decoration_1().text_decoration_wavy()
                                    }
                                    _ => el.text_decoration_1().text_decoration_solid(),
                                })
                        })
                        .child(SharedString::from(text))
                }))
                .when_some(cursor_here, |el, c| {
                    let color = cursor_color;
                    el.child(
                        div()
                            .absolute()
                            .left(cell_w * (c.col as f32))
                            .top(px(0.0))
                            .w(match c.shape {
                                CursorShape::Bar => px(2.0),
                                _ => cell_w,
                            })
                            .h(match c.shape {
                                CursorShape::Underline => px(2.0),
                                _ => cell_h,
                            })
                            .when(c.shape == CursorShape::Underline, |el| {
                                el.top(cell_h - px(2.0))
                            })
                            .map(|el| match (c.shape, self.focused) {
                                (CursorShape::Hollow, _) | (_, false) => {
                                    el.border_1().border_color(color)
                                }
                                _ => el.bg(color),
                            }),
                    )
                })
        });

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .p(padding)
            .bg(theme.terminal.background)
            .overflow_hidden()
            .children(rows)
            .children(scrollback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
