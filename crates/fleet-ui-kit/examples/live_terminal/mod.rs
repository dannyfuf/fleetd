use std::{sync::Arc, time::Duration};

use fleet_ui_kit::prelude::*;

use crate::LAYOUT;

pub(crate) mod frames;
use frames::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrollState {
    Live,
    ScrolledBack,
    ScrollMode,
    Selecting,
}

impl ScrollState {
    pub(crate) fn next(self) -> Self {
        match self {
            ScrollState::Live => ScrollState::ScrolledBack,
            ScrollState::ScrolledBack => ScrollState::ScrollMode,
            ScrollState::ScrollMode => ScrollState::Selecting,
            ScrollState::Selecting => ScrollState::Live,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            ScrollState::Live => "live",
            ScrollState::ScrolledBack => "scrolled back",
            ScrollState::ScrollMode => "scroll mode",
            ScrollState::Selecting => "selecting",
        }
    }

    pub(crate) fn offset(self) -> usize {
        match self {
            ScrollState::Live => 0,
            _ => 412,
        }
    }
}

pub(crate) struct LiveTerminal {
    tick: u64,
    step: u64,
    /// Prepared immutable rows from the last generated frame.
    painted: Arc<[GridRow]>,
    painted_cursor: usize,
    generated_frames: u64,

    pub(crate) alt_screen: bool,
    pub(crate) scroll: ScrollState,
    pub(crate) selection: bool,
    pub(crate) focused: bool,
    pub(crate) cursor_shape: usize,
    pub(crate) prefix: bool,
    reported: (usize, usize),

    cache: TerminalGridCache,
    _ticker: gpui::Task<()>,
}

impl LiveTerminal {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let alive = this
                    .update(cx, |this, cx| {
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

        let (rows, cursor) = normal_frame(cx.theme(), 0);
        Self {
            tick: 0,
            step: 0,
            painted: rows.into(),
            painted_cursor: cursor,
            generated_frames: 0,
            alt_screen: false,
            scroll: ScrollState::Live,
            selection: false,
            focused: true,
            cursor_shape: 0,
            prefix: false,
            reported: (0, 0),
            cache: TerminalGridCache::default(),
            _ticker: ticker,
        }
    }

    pub(crate) fn refresh(&mut self, cx: &App) {
        let (rows, cursor) = self.frame(cx.theme());
        self.painted = rows.into();
        self.painted_cursor = cursor;
    }

    fn advance(&mut self, cx: &mut App) -> bool {
        self.tick += 1;
        if !self.tick.is_multiple_of(TICKS_PER_STEP) {
            return false;
        }
        self.step += 1;
        let theme = cx.theme();
        let (rows, cursor) = self.frame(theme);
        if rows.as_slice() == self.painted.as_ref() && cursor == self.painted_cursor {
            return false;
        }
        self.painted = rows.into();
        self.painted_cursor = cursor;
        self.generated_frames += 1;
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

    fn live_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let row_count = self.painted.len();
        let cursor_col = self.painted_cursor;
        let cursor = GridCursor {
            row: row_count.saturating_sub(1),
            col: cursor_col,
            visible: true,
            shape: self.cursor_shape(),
        };
        let weak = cx.weak_entity();

        let mut grid = TerminalGrid::from_shared(self.painted.clone())
            .cache(&self.cache)
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
            .child(Text::hint(format!("{} generated frames", self.generated_frames)).faint())
            .child(Text::hint(format!("{cols}x{rows_reported} reported")).faint())
            .child(Text::hint(self.scroll.label()).faint())
            .child(Text::hint(if self.focused { "focused" } else { "unfocused" }).faint());

        let terminal = div()
            .relative()
            .w_full()
            .h(px(320.0))
            .rounded(t.radii.sm)
            .border(t.metrics.hairline)
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
        LAYOUT.section("live terminal", &t, children)
    }
}

impl Render for LiveTerminal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.live_section(cx)
    }
}
