use fleet_ui_kit::prelude::*;

/// The style flags one span of the fake frame can carry.
///
/// This is the example's stand-in for an SGR state machine: enough to exercise every branch of
/// [`GridCell::resolve`] without pulling a VT parser into a gallery.
#[derive(Clone, Copy, Default)]
pub(crate) struct Sgr {
    fg: Option<usize>,
    bg: Option<usize>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: UnderlineStyle,
    underline_color: Option<usize>,
    strikethrough: bool,
    inverse: bool,
    blink: bool,
    invisible: bool,
}

impl Sgr {
    pub(crate) fn fg(index: usize) -> Self {
        Self {
            fg: Some(index),
            ..Self::default()
        }
    }

    pub(crate) fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub(crate) fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    pub(crate) fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub(crate) fn bg(mut self, index: usize) -> Self {
        self.bg = Some(index);
        self
    }

    pub(crate) fn underline(mut self, style: UnderlineStyle) -> Self {
        self.underline = style;
        self
    }

    pub(crate) fn underline_color(mut self, index: usize) -> Self {
        self.underline_color = Some(index);
        self
    }

    pub(crate) fn strikethrough(mut self) -> Self {
        self.strikethrough = true;
        self
    }

    pub(crate) fn inverse(mut self) -> Self {
        self.inverse = true;
        self
    }

    pub(crate) fn blink(mut self) -> Self {
        self.blink = true;
        self
    }

    pub(crate) fn invisible(mut self) -> Self {
        self.invisible = true;
        self
    }

    pub(crate) fn apply(self, cell: GridCell, theme: &Theme) -> GridCell {
        let ansi = |ix: usize| theme.terminal.ansi[ix % 16];
        let mut cell = cell
            .bold(self.bold)
            .dim(self.dim)
            .italic(self.italic)
            .underline(self.underline)
            .strikethrough(self.strikethrough)
            .inverse(self.inverse)
            .blink(self.blink)
            .invisible(self.invisible);
        if let Some(fg) = self.fg {
            cell = cell.fg(ansi(fg));
        }
        if let Some(bg) = self.bg {
            cell = cell.bg(ansi(bg));
        }
        if let Some(color) = self.underline_color {
            cell = cell.underline_color(ansi(color));
        }
        cell
    }
}

/// One span of a fake row.
pub(crate) struct Span(pub(crate) &'static str, pub(crate) Sgr);

/// Turn spans into narrow cells.
pub(crate) fn row_of(theme: &Theme, spans: &[Span]) -> GridRow {
    let mut cells = Vec::new();
    for Span(text, sgr) in spans {
        for ch in text.chars() {
            cells.push(sgr.apply(GridCell::new(ch.to_string(), theme), theme));
        }
    }
    GridRow::new(cells)
}

/// A row of two-column graphemes, each followed by its zero-column spacer.
pub(crate) fn wide_row(theme: &Theme, text: &str, sgr: Sgr) -> GridRow {
    let mut cells = Vec::new();
    for ch in text.chars() {
        cells.push(
            sgr.apply(GridCell::new(ch.to_string(), theme), theme)
                .width(CellWidth::Wide),
        );
        cells.push(GridCell::new("", theme).width(CellWidth::Spacer));
    }
    GridRow::new(cells)
}

/// The command the fake shell is "typing", one character per cursor step.
pub(crate) const TYPED: &str = "cargo run -p fleet-app";

/// How many 16 ms ticks pass between two visible changes.
pub(crate) const TICKS_PER_STEP: u64 = 12;

/// The normal-screen frame at a given step.
pub(crate) fn normal_frame(theme: &Theme, step: u64) -> (Vec<GridRow>, usize) {
    let typed_len = (step as usize) % (TYPED.chars().count() + 6);
    let typed: String = TYPED.chars().take(typed_len).collect();
    let green = Sgr::fg(2);
    let dim = Sgr::default().dim();
    let mut rows = vec![
        row_of(
            theme,
            &[
                Span("\u{276f} ", green),
                Span("cargo build", Sgr::default()),
            ],
        ),
        row_of(
            theme,
            &[
                Span("   Compiling ", Sgr::fg(2).bold()),
                Span("fleet-ui-kit ", Sgr::default()),
                Span("v0.1.0", dim),
            ],
        ),
        row_of(
            theme,
            &[
                Span("warning", Sgr::fg(3).bold()),
                Span(": unused variable: ", Sgr::default()),
                Span("`theme`", Sgr::fg(6)),
            ],
        ),
        row_of(
            theme,
            &[
                Span("  --> ", Sgr::fg(4)),
                Span("crates/fleet-app/src/main.rs:42", Sgr::default().dim()),
            ],
        ),
        row_of(
            theme,
            &[
                Span("error", Sgr::fg(1).bold()),
                Span(": could not compile ", Sgr::default()),
                Span("fleet-daemon", Sgr::fg(1)),
            ],
        ),
        row_of(
            theme,
            &[
                Span("    Finished ", Sgr::fg(2).bold()),
                Span("dev [unoptimized + debuginfo] target(s) in 4.21s", dim),
            ],
        ),
        GridRow::default(),
        wide_row(theme, "日本語の桁も揃う", Sgr::fg(5)),
        GridRow::default(),
    ];
    let prompt = row_of(theme, &[Span("\u{276f} ", green), Span("", Sgr::default())]);
    let mut prompt_cells = prompt.cells;
    for ch in typed.chars() {
        prompt_cells.push(GridCell::new(ch.to_string(), theme));
    }
    let cursor_col = prompt_cells.len();
    rows.push(GridRow::new(prompt_cells));
    (rows, cursor_col)
}

/// The alt-screen frame: a full-width inverse header, a selected row and box drawing — the
/// three things a TUI does that a naive mirror grid gets wrong.
pub(crate) fn alt_frame(theme: &Theme, step: u64) -> (Vec<GridRow>, usize) {
    let selected = (step as usize / 2) % 4;
    let header = Sgr::default().inverse().bold();
    let mut rows = vec![
        row_of(
            theme,
            &[Span(
                " lazygit  \u{2014}  fleet   \u{2502} status \u{2502} files \u{2502} branches   ",
                header,
            )],
        ),
        row_of(
            theme,
            &[Span(
                "\u{250c}\u{2500} Files \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}",
                Sgr::fg(8),
            )],
        ),
    ];
    let files = [
        " M crates/fleet-ui-kit/src/lib.rs",
        " A crates/fleet-ui-kit/examples/gallery_terminal.rs",
        " D docs/old-notes.md",
        " ? target/",
    ];
    for (ix, file) in files.iter().enumerate() {
        let base = if ix == selected {
            Sgr::default().inverse()
        } else {
            Sgr::default()
        };
        let mut cells = Vec::new();
        for ch in "\u{2502}".chars() {
            cells.push(Sgr::fg(8).apply(GridCell::new(ch.to_string(), theme), theme));
        }
        for ch in file.chars() {
            cells.push(base.apply(GridCell::new(ch.to_string(), theme), theme));
        }
        rows.push(GridRow::new(cells));
    }
    rows.push(row_of(
        theme,
        &[Span("\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}", Sgr::fg(8))],
    ));
    rows.push(GridRow::default());
    rows.push(row_of(
        theme,
        &[
            Span("no scrollback here", Sgr::default().dim().italic()),
            Span(
                "  \u{2014}  alt-screen owns the buffer",
                Sgr::default().dim(),
            ),
        ],
    ));
    (rows, 1)
}
