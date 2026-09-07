//! The cell model: what a mirrored terminal cell is, and how it resolves to paint colors.
//!
//! The kit deliberately defines its **own** cell model rather than depending on `fleet-proto`:
//! `fleet-ui-kit` has no domain dependencies, and the app converts `proto::Cell` into
//! [`GridCell`] on the way in, resolving `Palette(u8)` through
//! [`crate::theme::TerminalPalette::color`] and `Default` through the palette's `foreground` /
//! `background`.
//!
//! **The attribute set is complete on purpose.** `INVERSE` and `DIM` are not cosmetic: lazygit,
//! nvim status lines and `fzf` draw their selection with reverse video and dim, so a reduced
//! cell model visibly corrupts exactly the apps the default `nvim | cc | lg` layout runs.
//! [`GridCell`] therefore carries all ten VT flags plus the underline style and color, and
//! [`GridCell::resolve`] applies `inverse` / `dim` / `invisible` for the painter.
//!
//! # Redraw policy
//!
//! The grid **never** asks for an animation frame: it has no blink timer and no spinner, so a
//! terminal that produces no output costs zero frames. The caller drives repaints from the
//! daemon's `FrameUpdate` stream — [`GridRow`] is `PartialEq` precisely so a 60 fps producer
//! can compare the new rows against the painted ones and notify only when something changed.

use gpui::{Hsla, Pixels, SharedString, StrikethroughStyle, TextRun};

use super::cell_font;
use crate::theme::Theme;

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
    pub(super) fn decoration(self, color: Hsla, hairline: Pixels) -> Option<gpui::UnderlineStyle> {
        let (thickness, wavy) = match self {
            UnderlineStyle::None => return None,
            UnderlineStyle::Single => (hairline, false),
            UnderlineStyle::Double => (hairline * 2.0, false),
            UnderlineStyle::Curly => (hairline, true),
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
            fg.a *= theme.metrics.stale_opacity;
        }
        if self.blink {
            fg.a *= theme.metrics.terminal_blink_opacity;
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
    pub(super) fn paints_glyph(&self) -> bool {
        if self.invisible || self.width == CellWidth::Spacer {
            return false;
        }
        let blank = self.text.is_empty() || self.text.as_ref() == " ";
        !blank || self.underline.is_some() || self.strikethrough
    }

    /// The string this cell shapes: a decorated empty cell still needs one blank to hang its
    /// underline on.
    pub(super) fn glyph_text(&self) -> &str {
        if self.text.is_empty() {
            " "
        } else {
            self.text.as_ref()
        }
    }

    /// The gpui text run for this cell, with `inverse` / `dim` / `blink` already applied.
    pub(super) fn text_run(&self, theme: &Theme, len: usize) -> TextRun {
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
            underline: self
                .underline
                .decoration(underline_color, theme.metrics.hairline),
            strikethrough: self.strikethrough.then_some(StrikethroughStyle {
                thickness: theme.metrics.hairline,
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
