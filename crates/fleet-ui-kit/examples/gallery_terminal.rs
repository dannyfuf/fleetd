//! The visual test bench for the **jobs and terminal** half of `fleet-ui-kit`.
//!
//! `TerminalGrid`, `TerminalTabStrip`, `ScrollPill`, `ScrollbackBadge`, `PrefixHint`,
//! `ExitStrip`, `JobRow`, `JobTicker`, `StickyErrorSlot` and `LogView`, each in every state
//! the design system names, in both themes.
//!
//! The first section is **live**: a fake VT frame generator ticks at 60 Hz and the grid is
//! notified only when the frame it would paint actually changed — which is the redraw contract
//! `TerminalGrid` is built for, demonstrated rather than asserted. The frame counter in the
//! header shows how few repaints a busy-looking terminal really costs.
//!
//! ```sh
//! cargo run -p fleet-ui-kit --example gallery_terminal
//! ```
//!
//! | key | |
//! | --- | --- |
//! | `t` | light / dark |
//! | `a` | alt-screen (suppresses the scroll overlays) |
//! | `s` | Scroll mode (`ScrollPill`) / scrolled-back (`ScrollbackBadge`) / live |
//! | `v` | selection |
//! | `u` | focus the terminal / unfocus it (hollow cursor) |
//! | `c` | cursor shape |
//! | `p` | prefix hint |
//! | `L` | focus the log (then `f`, `j`, `k`, `G`); `esc` returns |
//! | `q` | quit |

use std::time::Duration;

use fleet_ui_kit::KitAssets;
use fleet_ui_kit::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, Menu, MenuItem,
    SharedString, TitlebarOptions, UniformListScrollHandle, Window, WindowBounds, WindowOptions,
    actions, div, px, size,
};

actions!(
    gallery_terminal,
    [
        ToggleTheme,
        ToggleAltScreen,
        CycleScroll,
        ToggleSelection,
        ToggleFocus,
        CycleCursor,
        TogglePrefix,
        FocusLog,
        FocusRoot,
        Quit,
    ]
);

// ---------------------------------------------------------------- fake VT frames

/// The style flags one span of the fake frame can carry.
///
/// This is the example's stand-in for an SGR state machine: enough to exercise every branch of
/// [`GridCell::resolve`] without pulling a VT parser into a gallery.
#[derive(Clone, Copy, Default)]
struct Sgr {
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
    fn fg(index: usize) -> Self {
        Self {
            fg: Some(index),
            ..Self::default()
        }
    }

    fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    fn bg(mut self, index: usize) -> Self {
        self.bg = Some(index);
        self
    }

    fn underline(mut self, style: UnderlineStyle) -> Self {
        self.underline = style;
        self
    }

    fn underline_color(mut self, index: usize) -> Self {
        self.underline_color = Some(index);
        self
    }

    fn strikethrough(mut self) -> Self {
        self.strikethrough = true;
        self
    }

    fn inverse(mut self) -> Self {
        self.inverse = true;
        self
    }

    fn blink(mut self) -> Self {
        self.blink = true;
        self
    }

    fn invisible(mut self) -> Self {
        self.invisible = true;
        self
    }

    fn apply(self, cell: GridCell, theme: &Theme) -> GridCell {
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
struct Span(&'static str, Sgr);

/// Turn spans into narrow cells.
fn row_of(theme: &Theme, spans: &[Span]) -> GridRow {
    let mut cells = Vec::new();
    for Span(text, sgr) in spans {
        for ch in text.chars() {
            cells.push(sgr.apply(GridCell::new(ch.to_string(), theme), theme));
        }
    }
    GridRow::new(cells)
}

/// A row of two-column graphemes, each followed by its zero-column spacer.
fn wide_row(theme: &Theme, text: &str, sgr: Sgr) -> GridRow {
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
const TYPED: &str = "cargo run -p fleet-app";

/// How many 16 ms ticks pass between two visible changes.
///
/// The whole point of the section: at 60 Hz the generator runs 60 times a second and the grid
/// repaints five times, because nothing else changed.
const TICKS_PER_STEP: u64 = 12;

/// The normal-screen frame at a given step.
fn normal_frame(theme: &Theme, step: u64) -> (Vec<GridRow>, usize) {
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
fn alt_frame(theme: &Theme, step: u64) -> (Vec<GridRow>, usize) {
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

// ---------------------------------------------------------------- the gallery entity

/// What the scroll overlays are showing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScrollState {
    Live,
    ScrolledBack,
    ScrollMode,
    Selecting,
}

impl ScrollState {
    fn next(self) -> Self {
        match self {
            ScrollState::Live => ScrollState::ScrolledBack,
            ScrollState::ScrolledBack => ScrollState::ScrollMode,
            ScrollState::ScrollMode => ScrollState::Selecting,
            ScrollState::Selecting => ScrollState::Live,
        }
    }

    fn label(self) -> &'static str {
        match self {
            ScrollState::Live => "live",
            ScrollState::ScrolledBack => "scrolled back",
            ScrollState::ScrollMode => "scroll mode",
            ScrollState::Selecting => "selecting",
        }
    }

    fn offset(self) -> usize {
        match self {
            ScrollState::Live => 0,
            _ => 412,
        }
    }
}

struct Gallery {
    focus_handle: FocusHandle,
    log_focus: FocusHandle,
    log_scroll: UniformListScrollHandle,

    tick: u64,
    step: u64,
    /// The rows the grid last painted, so the 60 Hz generator can notify only on a real change.
    painted: Vec<GridRow>,
    painted_cursor: usize,
    repaints: u64,

    alt_screen: bool,
    scroll: ScrollState,
    selection: bool,
    focused: bool,
    cursor_shape: usize,
    prefix: bool,
    reported: (usize, usize),

    log_following: bool,
    log_top: usize,
    log_lines: Vec<SharedString>,

    _ticker: gpui::Task<()>,
}

impl Gallery {
    fn new(cx: &mut Context<Self>) -> Self {
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let alive = this
                    .update(cx, |this, cx| {
                        // 60 generator runs a second; `cx.notify` only when the frame the grid
                        // would paint is not the frame it already painted.
                        if this.advance(cx) {
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        });

        Self {
            focus_handle: cx.focus_handle(),
            log_focus: cx.focus_handle(),
            log_scroll: UniformListScrollHandle::new(),
            tick: 0,
            step: 0,
            painted: Vec::new(),
            painted_cursor: 0,
            repaints: 0,
            alt_screen: false,
            scroll: ScrollState::Live,
            selection: false,
            focused: true,
            cursor_shape: 0,
            prefix: false,
            reported: (0, 0),
            log_following: true,
            log_top: 0,
            log_lines: sample_log(),
            _ticker: ticker,
        }
    }

    /// Advance the generator by one 16 ms tick. Returns whether the painted frame changed.
    fn advance(&mut self, cx: &mut App) -> bool {
        self.tick += 1;
        if !self.tick.is_multiple_of(TICKS_PER_STEP) {
            return false;
        }
        self.step += 1;
        let theme = cx.theme();
        let (rows, cursor) = self.frame(theme);
        if rows == self.painted && cursor == self.painted_cursor {
            return false;
        }
        self.painted = rows;
        self.painted_cursor = cursor;
        self.repaints += 1;
        true
    }

    fn frame(&self, theme: &Theme) -> (Vec<GridRow>, usize) {
        if self.alt_screen {
            alt_frame(theme, self.step)
        } else {
            normal_frame(theme, self.step)
        }
    }

    fn cursor_shape(&self) -> CursorShape {
        const SHAPES: [CursorShape; 3] =
            [CursorShape::Block, CursorShape::Bar, CursorShape::Underline];
        SHAPES[self.cursor_shape % SHAPES.len()]
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        Theme::toggle(cx);
        // The palette changed, so every cached cell color did too.
        self.painted.clear();
        cx.notify();
    }

    fn toggle_alt_screen(
        &mut self,
        _: &ToggleAltScreen,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.alt_screen = !self.alt_screen;
        self.painted.clear();
        cx.notify();
    }

    fn cycle_scroll(&mut self, _: &CycleScroll, _window: &mut Window, cx: &mut Context<Self>) {
        self.scroll = self.scroll.next();
        cx.notify();
    }

    fn toggle_selection(
        &mut self,
        _: &ToggleSelection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection = !self.selection;
        cx.notify();
    }

    fn toggle_focus(&mut self, _: &ToggleFocus, _window: &mut Window, cx: &mut Context<Self>) {
        self.focused = !self.focused;
        cx.notify();
    }

    fn cycle_cursor(&mut self, _: &CycleCursor, _window: &mut Window, cx: &mut Context<Self>) {
        self.cursor_shape += 1;
        cx.notify();
    }

    fn toggle_prefix(&mut self, _: &TogglePrefix, _window: &mut Window, cx: &mut Context<Self>) {
        self.prefix = !self.prefix;
        cx.notify();
    }

    fn focus_log(&mut self, _: &FocusLog, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.log_focus, cx);
        cx.notify();
    }

    fn focus_root(&mut self, _: &FocusRoot, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }
}

impl Focusable for Gallery {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn sample_log() -> Vec<SharedString> {
    let mut lines = Vec::new();
    for ix in 0..48 {
        lines.push(SharedString::from(format!(
            "remote: Counting objects: {}% ({}/202), done.",
            (ix * 2).min(100),
            ix * 4
        )));
    }
    lines.push(SharedString::new_static(
        "fatal: unable to access 'https://github.com/…': HTTP 502",
    ));
    lines
}

// ---------------------------------------------------------------- layout helpers

fn section(title: &str, t: &Theme, children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(t.space.md)
        .pb(t.space.xl)
        .child(SectionHeader::new(title.to_string()))
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(t.space.md)
                .children(children),
        )
        .into_any_element()
}

fn labeled(label: &str, t: &Theme, child: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .items_start()
        .w_full()
        .gap(t.space.md)
        .child(Text::hint(label.to_string()).faint().w(px(168.0)))
        .child(div().flex().flex_1().min_w_0().items_center().child(child))
        .into_any_element()
}

/// A bordered, `relative` box: the overlays position themselves inside one of these.
fn stage(t: &Theme, height: gpui::Pixels, child: impl IntoElement) -> AnyElement {
    div()
        .relative()
        .w_full()
        .h(height)
        .rounded(t.radii.sm)
        .bg(t.terminal.background)
        .border_1()
        .border_color(t.colors.border)
        .overflow_hidden()
        .child(child)
        .into_any_element()
}

// ---------------------------------------------------------------- sections

impl Gallery {
    fn live_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let (rows, cursor_col) = self.frame(&t);
        let row_count = rows.len();
        let cursor = GridCursor {
            row: row_count.saturating_sub(1),
            col: cursor_col,
            visible: true,
            shape: self.cursor_shape(),
        };
        let weak = cx.weak_entity();

        let mut grid = TerminalGrid::new(rows)
            .id("live-grid")
            .cursor(cursor)
            .focused(self.focused)
            .scrollback(self.scroll.offset(), 2000)
            .modes(if self.alt_screen {
                vec![
                    TerminalMode::AltScreen,
                    TerminalMode::MouseReporting,
                    TerminalMode::ApplicationCursor,
                ]
            } else {
                vec![TerminalMode::BracketedPaste]
            })
            .on_resize(move |cols, rows, _window, cx| {
                weak.update(cx, |this, cx| {
                    if this.reported != (cols, rows) {
                        this.reported = (cols, rows);
                        cx.notify();
                    }
                })
                .ok();
            });
        if matches!(
            self.scroll,
            ScrollState::ScrollMode | ScrollState::Selecting
        ) {
            grid = grid.scroll_pill(
                ScrollPill::new(self.scroll.offset(), 2000)
                    .selecting(self.scroll == ScrollState::Selecting),
            );
        }
        if self.selection {
            grid = grid.selection(GridSelection::new(2, 0, 4, 28));
        }

        let (cols, rows_reported) = self.reported;
        let header = div()
            .flex()
            .items_center()
            .gap(t.space.md)
            .child(Text::label("live frame generator"))
            .child(Text::hint(format!("{} ticks", self.tick)).faint())
            .child(Text::hint(format!("{} repaints", self.repaints)).faint())
            .child(Text::hint(format!("{cols}x{rows_reported} reported")).faint())
            .child(Text::hint(self.scroll.label()).faint())
            .child(Text::hint(if self.focused { "focused" } else { "unfocused" }).faint());

        let terminal = div()
            .relative()
            .w_full()
            .h(px(320.0))
            .rounded(t.radii.sm)
            .border_1()
            .border_color(t.colors.border)
            .overflow_hidden()
            .child(grid)
            .child(PrefixHint::new(self.prefix));

        let children = vec![
            header.into_any_element(),
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(t.space.xs)
                .child(
                    TerminalTabStrip::new([
                        TerminalTab::new(1, "nvim").keep_alive(Icon::FilePen),
                        TerminalTab::new(2, "cc")
                            .activity(true)
                            .keep_alive(Icon::Bot),
                        TerminalTab::new(3, "lg"),
                        TerminalTab::new(4, "test").exited(1),
                    ])
                    .id("live-tabs")
                    .active(1),
                )
                .child(terminal)
                .child(ExitStrip::new(1))
                .into_any_element(),
        ];
        section("live terminal", &t, children)
    }
}

fn attributes_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let row = |label: &'static str, sgr: Sgr| {
        row_of(
            &t,
            &[
                Span("  ", Sgr::default()),
                Span(label, sgr),
                Span("  the quick brown fox", sgr),
            ],
        )
    };
    let rows = vec![
        row("plain     ", Sgr::default()),
        row("bold      ", Sgr::default().bold()),
        row("dim       ", Sgr::default().dim()),
        row("italic    ", Sgr::default().italic()),
        row(
            "underline ",
            Sgr::default().underline(UnderlineStyle::Single),
        ),
        row(
            "double    ",
            Sgr::default().underline(UnderlineStyle::Double),
        ),
        row(
            "curly     ",
            Sgr::default()
                .underline(UnderlineStyle::Curly)
                .underline_color(1),
        ),
        row("strike    ", Sgr::default().strikethrough()),
        row("inverse   ", Sgr::default().inverse()),
        row("blink     ", Sgr::default().blink()),
        row("invisible ", Sgr::default().invisible().bg(4)),
        row("on bg     ", Sgr::fg(0).bg(3)),
        row("256 color ", Sgr::fg(13).bold()),
        wide_row(&t, "  幅の広い文字", Sgr::fg(6)),
    ];

    let palette = {
        let mut cells = Vec::new();
        for ix in 0..16 {
            for ch in " \u{2588}\u{2588} ".chars() {
                cells.push(Sgr::fg(ix).apply(GridCell::new(ch.to_string(), &t), &t));
            }
        }
        GridRow::new(cells)
    };

    let cursor_stage = |label: &'static str, shape: CursorShape, focused: bool| {
        let rows = vec![row_of(
            &t,
            &[
                Span("  ", Sgr::default()),
                Span("cursor here", Sgr::default()),
            ],
        )];
        div()
            .flex()
            .flex_col()
            .gap(t.space.xxs)
            .child(Text::hint(label).faint())
            .child(
                div()
                    .w(px(180.0))
                    .h(px(38.0))
                    .rounded(t.radii.sm)
                    .border_1()
                    .border_color(t.colors.border)
                    .overflow_hidden()
                    .child(
                        TerminalGrid::new(rows)
                            .padding(t.space.xs)
                            .focused(focused)
                            .cursor(GridCursor {
                                row: 0,
                                col: 9,
                                visible: true,
                                shape,
                            }),
                    ),
            )
            .into_any_element()
    };

    let children = vec![
        labeled(
            "attributes",
            &t,
            stage(
                &t,
                px(f32::from(t.text.data.line_height) * 15.0),
                TerminalGrid::new(rows),
            ),
        ),
        labeled(
            "ansi 0-15",
            &t,
            stage(&t, px(40.0), TerminalGrid::new(vec![palette])),
        ),
        labeled(
            "selection",
            &t,
            stage(
                &t,
                px(80.0),
                TerminalGrid::new(vec![
                    row_of(&t, &[Span("  selected across", Sgr::default())]),
                    row_of(&t, &[Span("  three whole rows", Sgr::default())]),
                    row_of(&t, &[Span("  and part of this", Sgr::default())]),
                ])
                .selection(GridSelection::new(0, 2, 2, 8)),
            ),
        ),
        div()
            .flex()
            .flex_wrap()
            .gap(t.space.md)
            .children(vec![
                cursor_stage("block", CursorShape::Block, true),
                cursor_stage("bar", CursorShape::Bar, true),
                cursor_stage("underline", CursorShape::Underline, true),
                cursor_stage("hollow (unfocused)", CursorShape::Block, false),
            ])
            .into_any_element(),
    ];
    section("terminal grid \u{b7} states", &t, children)
}

fn overlays_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let children = vec![
        labeled(
            "scroll pill",
            &t,
            stage(&t, px(64.0), ScrollPill::new(412, 2000)),
        ),
        labeled(
            "scroll pill \u{b7} selecting",
            &t,
            stage(&t, px(78.0), ScrollPill::new(412, 2000).selecting(true)),
        ),
        labeled(
            "scroll pill \u{b7} alt-screen (suppressed)",
            &t,
            stage(&t, px(48.0), ScrollPill::new(412, 2000).alt_screen(true)),
        ),
        labeled(
            "scrollback badge",
            &t,
            stage(&t, px(56.0), ScrollbackBadge::new(412, 2000)),
        ),
        labeled(
            "scrollback badge \u{b7} live (suppressed)",
            &t,
            stage(&t, px(48.0), ScrollbackBadge::new(0, 2000)),
        ),
        labeled(
            "prefix hint \u{b7} hidden",
            &t,
            stage(&t, px(48.0), PrefixHint::new(false)),
        ),
        labeled(
            "prefix hint \u{b7} visible",
            &t,
            stage(&t, px(64.0), PrefixHint::new(true)),
        ),
        labeled(
            "prefix hint \u{b7} custom keys",
            &t,
            stage(
                &t,
                px(64.0),
                PrefixHint::new(true).prefix("^A").hints(
                    KeyHintRow::new()
                        .key("s", "hub")
                        .key("]", "paste")
                        .key("z", "zoom"),
                ),
            ),
        ),
        labeled("exit strip \u{b7} failure", &t, ExitStrip::new(1)),
        labeled("exit strip \u{b7} clean", &t, ExitStrip::new(0)),
        labeled("exit strip \u{b7} killed", &t, ExitStrip::new(None)),
        labeled(
            "exit strip \u{b7} custom keys",
            &t,
            ExitStrip::new(127).hints(KeyHintRow::new().key("^s r", "restart")),
        ),
    ];
    section("terminal overlays", &t, children)
}

fn tabs_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let children = vec![
        labeled(
            "default \u{b7} three terminals",
            &t,
            TerminalTabStrip::new([
                TerminalTab::new(1, "nvim"),
                TerminalTab::new(2, "cc"),
                TerminalTab::new(3, "lg"),
            ])
            .id("tabs-default"),
        ),
        labeled(
            "every mark",
            &t,
            TerminalTabStrip::new([
                TerminalTab::new(1, "nvim").keep_alive(Icon::FilePen),
                TerminalTab::new(2, "cc")
                    .activity(true)
                    .keep_alive(Icon::Bot),
                TerminalTab::new(3, "server").keep_alive(Icon::Server),
                TerminalTab::new(4, "test").exited(1),
                TerminalTab::new(5, "build").exited(None),
                TerminalTab::new(6, "a-very-long-terminal-name").activity(true),
            ])
            .id("tabs-marks")
            .active(2),
        ),
        labeled(
            "waking \u{b7} every PTY still spawning",
            &t,
            TerminalTabStrip::new([
                TerminalTab::new(1, "nvim").starting(true),
                TerminalTab::new(2, "cc").starting(true),
                TerminalTab::new(3, "lg").starting(true),
            ])
            .id("tabs-waking"),
        ),
        labeled(
            "no new-tab affordance",
            &t,
            TerminalTabStrip::new([TerminalTab::new(1, "shell")])
                .id("tabs-no-plus")
                .show_plus(false),
        ),
    ];
    section("terminal tab strip", &t, children)
}

fn jobs_section(cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let rows = div()
        .flex()
        .flex_col()
        .w_full()
        .rounded(t.radii.sm)
        .bg(t.colors.surface)
        .border_1()
        .border_color(t.colors.border)
        .overflow_hidden()
        .child(
            JobRow::new(JobStatus::Running, "clone", "nixos")
                .id("job-running")
                .elapsed("0:42")
                .percent(40)
                .progress("Receiving objects: 40% (81/202)")
                .selected(true)
                .cursor(true),
        )
        .child(
            JobRow::new(JobStatus::Running, "hooks", "buk/payroll#feat-rut")
                .id("job-hooks")
                .elapsed("0:08")
                .progress("pnpm install (2/3)"),
        )
        .child(
            JobRow::new(JobStatus::Cancelling, "pool", "dannyfuf/fleetd")
                .id("job-cancelling")
                .elapsed("1:03")
                .progress("waiting for the current copy to finish"),
        )
        .child(JobRow::new(JobStatus::Queued, "prune", "buk/www").id("job-queued"))
        .child(
            JobRow::new(JobStatus::Failed, "prs", "review")
                .id("job-failed")
                .elapsed("1m")
                .trailing_key("R")
                .progress("gh: HTTP 502 upstream connect error"),
        )
        .child(
            JobRow::new(JobStatus::Done, "prune", "buk/www")
                .id("job-done")
                .elapsed("12s"),
        )
        .child(JobRow::new(JobStatus::Cancelled, "fetch", "dannyfuf/fleetd").id("job-cancelled"));

    let confirm_rows = div()
        .flex()
        .flex_col()
        .w_full()
        .rounded(t.radii.sm)
        .bg(t.colors.surface)
        .border_1()
        .border_color(t.colors.border)
        .overflow_hidden()
        .child(
            JobRow::new(JobStatus::Running, "clone", "nixos")
                .id("confirm-1")
                .retryable(true),
        )
        .child(
            JobRow::new(JobStatus::Running, "hooks", "buk/payroll")
                .id("confirm-2")
                .retryable(false),
        );

    let children = vec![
        labeled("every status", &t, rows),
        labeled("quit-and-stop confirm", &t, confirm_rows),
        labeled(
            "ticker",
            &t,
            JobTicker::new("clone", "nixos").id("ticker-1"),
        ),
        labeled(
            "ticker \u{b7} percent + elapsed + others",
            &t,
            JobTicker::new("clone", "nixos")
                .id("ticker-2")
                .elapsed("0:42")
                .percent(40)
                .extra(2),
        ),
        labeled(
            "sticky error",
            &t,
            StickyErrorSlot::new("gh: HTTP 502 upstream connect error").id("err-1"),
        ),
        labeled(
            "sticky error \u{b7} repeated, prefixed key",
            &t,
            StickyErrorSlot::new("gh: HTTP 502 upstream connect error")
                .id("err-2")
                .count(7)
                .key("^s !"),
        ),
    ];
    section("jobs", &t, children)
}

impl Gallery {
    fn log_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let weak = cx.weak_entity();
        let log = LogView::new("log", self.log_lines.clone())
            .following(self.log_following)
            .top(self.log_top)
            .track_scroll(&self.log_scroll)
            .focus(&self.log_focus)
            .on_command(move |command, _window, cx| {
                weak.update(cx, |this, cx| {
                    match command {
                        LogCommand::ToggleFollow => this.log_following = !this.log_following,
                        LogCommand::Follow => {
                            this.log_following = true;
                            this.log_top = this.log_lines.len().saturating_sub(1);
                        }
                        LogCommand::ScrollTo(top) => {
                            this.log_following = false;
                            this.log_top = top;
                        }
                    }
                    cx.notify();
                })
                .ok();
            });

        let children = vec![
            labeled(
                "log \u{b7} press L to focus, then f / j / k / G",
                &t,
                div()
                    .w_full()
                    .h(px(180.0))
                    .rounded(t.radii.sm)
                    .bg(t.colors.surface)
                    .border_1()
                    .border_color(t.colors.border)
                    .overflow_hidden()
                    .child(log),
            ),
            labeled(
                "log \u{b7} empty",
                &t,
                div()
                    .w_full()
                    .h(px(72.0))
                    .rounded(t.radii.sm)
                    .bg(t.colors.surface)
                    .border_1()
                    .border_color(t.colors.border)
                    .child(LogView::new("log-empty", Vec::new())),
            ),
        ];
        section("log view", &t, children)
    }
}

// ---------------------------------------------------------------- render

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = cx.theme().mode;
        let pad = cx.theme().space.xl;

        let sections = vec![
            self.live_section(cx),
            attributes_section(cx),
            overlays_section(cx),
            tabs_section(cx),
            jobs_section(cx),
            self.log_section(cx),
        ];

        AppFrame::new()
            .context_bar(
                ContextBar::new([ContextTab::new("jobs + terminal gallery", 1)])
                    .leading_inset(px(84.0))
                    .chip(Chip::labeled(
                        if mode.is_dark() {
                            Icon::Moon
                        } else {
                            Icon::CircleArrowUp
                        },
                        if mode.is_dark() { "dark" } else { "light" },
                    ))
                    .daemon(DaemonState::Healthy),
            )
            .body(
                div()
                    .id("gallery-terminal-scroll")
                    .track_focus(&self.focus_handle)
                    .key_context("GalleryTerminal")
                    .on_action(cx.listener(Self::toggle_theme))
                    .on_action(cx.listener(Self::toggle_alt_screen))
                    .on_action(cx.listener(Self::cycle_scroll))
                    .on_action(cx.listener(Self::toggle_selection))
                    .on_action(cx.listener(Self::toggle_focus))
                    .on_action(cx.listener(Self::cycle_cursor))
                    .on_action(cx.listener(Self::toggle_prefix))
                    .on_action(cx.listener(Self::focus_log))
                    .on_action(cx.listener(Self::focus_root))
                    .on_action(cx.listener(Self::quit))
                    .size_full()
                    .overflow_y_scroll()
                    .p(pad)
                    .flex()
                    .flex_col()
                    .children(sections),
            )
            .status_bar(
                StatusBar::new()
                    .breadcrumb("fleet-ui-kit \u{b7} jobs and terminal")
                    .mode(Mode::Terminal)
                    .ticker(
                        KeyHintRow::new()
                            .key("t", "theme")
                            .key("a", "alt")
                            .key("s", "scroll")
                            .key("v", "select")
                            .key("u", "focus")
                            .key("c", "cursor")
                            .key("p", "prefix")
                            .key("L", "log")
                            .key("q", "quit"),
                    ),
            )
    }
}

fn main() {
    gpui_platform::application()
        .with_assets(KitAssets)
        .run(|cx: &mut App| {
            Theme::init(ThemeMode::Dark, cx);
            cx.bind_keys([
                KeyBinding::new("t", ToggleTheme, None),
                KeyBinding::new("a", ToggleAltScreen, None),
                KeyBinding::new("s", CycleScroll, None),
                KeyBinding::new("v", ToggleSelection, None),
                KeyBinding::new("u", ToggleFocus, None),
                KeyBinding::new("c", CycleCursor, None),
                KeyBinding::new("p", TogglePrefix, None),
                KeyBinding::new("shift-l", FocusLog, None),
                KeyBinding::new("escape", FocusRoot, None),
                KeyBinding::new("q", Quit, None),
                KeyBinding::new("cmd-q", Quit, None),
            ]);
            cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
            cx.set_menus(vec![Menu {
                name: "fleet-ui-kit".into(),
                items: vec![MenuItem::action("Quit", Quit)],
                disabled: false,
            }]);
            cx.on_window_closed(|cx: &mut App, _window_id| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1180.0), px(860.0)), cx);
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("fleet-ui-kit \u{b7} jobs + terminal".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |_window, cx| {
                        let view: Entity<Gallery> = cx.new(Gallery::new);
                        view
                    },
                )
                .expect("failed to open the gallery window");

            window
                .update(cx, |view, window, cx| {
                    window.focus(&view.focus_handle(cx), cx);
                })
                .ok();

            cx.activate(true);
        });
}
