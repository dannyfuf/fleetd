//! `TerminalGrid` — the mirror cell grid.
//!
//! The kit deliberately defines its **own** cell model rather than depending on
//! `fleet-proto`: `fleet-ui-kit` has no domain dependencies, and the app converts
//! `proto::Cell` into [`GridCell`] on the way in, resolving `Palette(u8)` through
//! [`crate::theme::TerminalPalette::color`] and `Default` through the palette's `foreground` /
//! `background`.
//!
//! **Minimal render.** This implementation lays out one `div` per run of same-styled cells
//! inside one `div` per row, which is correct and honest but allocates per frame. The real
//! implementation is a custom `gpui::Element` that takes `relative(1.)` height, tracks its own
//! `scroll_top` and paints quads + shaped lines directly (Zed's `TerminalElement` does exactly
//! this, and uses neither `uniform_list` nor `list`). The public API below does not change
//! when that lands.

use gpui::{App, Hsla, SharedString, Window, div, prelude::*};

use crate::{
    text::styled_with,
    theme::{ActiveTheme, Theme},
};

/// The cursor shapes a VT can ask for.
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
    /// Italic.
    pub italic: bool,
    /// Underline.
    pub underline: bool,
    /// 1 for normal, 2 for a wide grapheme.
    pub width: u8,
}

impl GridCell {
    /// A plain cell in the palette's default foreground.
    pub fn new(text: impl Into<SharedString>, theme: &Theme) -> Self {
        Self {
            text: text.into(),
            fg: theme.terminal.foreground,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
            width: 1,
        }
    }

    /// Whether two cells can share one text run.
    pub fn same_style(&self, other: &GridCell) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
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
}

/// A rectangular text selection on the mirror grid.
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

/// The painted cell grid.
#[derive(IntoElement)]
pub struct TerminalGrid {
    rows: Vec<GridRow>,
    cursor: Option<GridCursor>,
    selection: Option<GridSelection>,
    focused: bool,
    padding: Option<gpui::Pixels>,
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
        }
    }

    /// Where the cursor is.
    pub fn cursor(mut self, cursor: GridCursor) -> Self {
        self.cursor = Some(cursor);
        self
    }

    /// The current selection, if any.
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

    /// How many rows the grid holds.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }
}

impl RenderOnce for TerminalGrid {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let padding = self.padding.unwrap_or(gpui::px(8.0));
        let cursor = self.cursor;
        let cell_w = theme.metrics.cell_w;
        let cell_h = theme.metrics.cell_h;

        let rows = self.rows.into_iter().enumerate().map(|(row_ix, row)| {
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
            div()
                .relative()
                .flex()
                .flex_row()
                .h(cell_h)
                .children(runs.into_iter().map(|(text, style)| {
                    styled_with(div(), theme.text.data, &theme)
                        .flex_none()
                        .h(cell_h)
                        .text_color(style.fg)
                        .when_some(style.bg, |el, bg| el.bg(bg))
                        .when(style.bold, |el| el.font_weight(gpui::FontWeight::BOLD))
                        .when(style.italic, |el| el.italic())
                        .when(style.underline, |el| el.underline())
                        .child(SharedString::from(text))
                }))
                .when_some(cursor_here, |el, c| {
                    let color = theme.terminal.cursor;
                    el.child(
                        div()
                            .absolute()
                            .left(cell_w * (c.col as f32))
                            .top(gpui::px(0.0))
                            .w(match c.shape {
                                CursorShape::Bar => gpui::px(2.0),
                                _ => cell_w,
                            })
                            .h(match c.shape {
                                CursorShape::Underline => gpui::px(2.0),
                                _ => cell_h,
                            })
                            .when(c.shape == CursorShape::Underline, |el| el.top(cell_h - gpui::px(2.0)))
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
            .flex()
            .flex_col()
            .size_full()
            .p(padding)
            .bg(theme.terminal.background)
            .overflow_hidden()
            .children(rows)
    }
}
