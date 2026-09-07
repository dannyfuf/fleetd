//! `KanbanColumn` and `KanbanBoard` — the two containers of the board screen.
//!
//! A column is a [`super::Pane`] that scrolls in the other axis: same hairline, same
//! `surface` ground, same 2 px focus ring, and a 30 px header that carries the status name and
//! the count. It is a separate component rather than a `Pane` variant because its body is a
//! **gapped stack of tiles**, not a list of flush rows, and because its header owns a category
//! accent bar that no pane has.
//!
//! Neither container owns a key. `h` / `l` / `j` / `k` move a cursor the screen owns, exactly
//! as they do for [`super::ListView`]; these two only draw.

use std::sync::Arc;

use gpui::{
    AnyElement, App, ElementId, Hsla, Pixels, ScrollHandle, SharedString, Window, div, prelude::*,
};

use crate::{
    components::{Badge, EmptyState},
    focus::FocusRing,
    text::Text,
    theme::{ActiveTheme, ch},
};

/// The default column width, in `ch` of the data face — wide enough for a `FLT-123` key, a
/// two-line title at a readable measure, and a meta row of three chips.
pub const COLUMN_WIDTH_CH: f32 = 34.0;

/// One board column.
#[derive(IntoElement)]
pub struct KanbanColumn {
    id: ElementId,
    title: SharedString,
    count: Option<usize>,
    accent: Option<Hsla>,
    focused: bool,
    width: Option<Pixels>,
    empty_hint: Option<SharedString>,
    tiles: Vec<AnyElement>,
    scroll: Option<ScrollHandle>,
}

impl KanbanColumn {
    /// A column titled `title`.
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            count: None,
            accent: None,
            focused: false,
            width: None,
            empty_hint: None,
            tiles: Vec::new(),
            scroll: None,
        }
    }

    /// The card count shown next to the title. Rendered even when it is `0`, because a column
    /// header is a ledger, not a chip: a missing count reads as "unknown", not as "empty".
    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    /// The status-category accent, as a 2 px bar above the header.
    ///
    /// The parameter is an `Hsla` and the call site is expected to pass a **token**
    /// (`cx.theme().colors.success`), never a literal: the category → token mapping is domain
    /// knowledge and lives in the app.
    pub fn accent(mut self, accent: Option<Hsla>) -> Self {
        self.accent = accent;
        self
    }

    /// Draw the 2 px focus ring: the keyboard is in this column.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Override the [`COLUMN_WIDTH_CH`] width.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// What an empty column says. Nothing is drawn when it is unset.
    pub fn empty_hint(mut self, empty_hint: impl Into<SharedString>) -> Self {
        self.empty_hint = Some(empty_hint.into());
        self
    }

    /// The tiles this column holds, normally [`super::CardTile`]s, in board order.
    pub fn tiles(mut self, tiles: impl IntoIterator<Item = AnyElement>) -> Self {
        self.tiles = tiles.into_iter().collect();
        self
    }

    /// Track the body's scroll offset, so the screen can reveal the cursor card after a move.
    pub fn scroll_handle(mut self, scroll: ScrollHandle) -> Self {
        self.scroll = Some(scroll);
        self
    }
}

impl RenderOnce for KanbanColumn {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let width = self.width.unwrap_or_else(|| ch(COLUMN_WIDTH_CH));
        let empty = self.tiles.is_empty();
        let body_id =
            ElementId::NamedChild(Arc::new(self.id.clone()), SharedString::new_static("body"));

        let header = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(theme.space.sm)
            .h(theme.metrics.pane_header_h)
            .w_full()
            .px(theme.space.sm)
            .border_b(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .child(Text::label(self.title).ellipsize()),
            )
            .children(self.count.map(|count| Badge::new(count.to_string())));

        let body = div()
            .id(body_id)
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .gap(theme.space.sm)
            .p(theme.space.sm)
            .overflow_y_scroll()
            .when_some(self.scroll, |el, scroll| el.track_scroll(&scroll))
            .when(empty, |el| {
                el.children(
                    self.empty_hint
                        .clone()
                        .map(|hint| div().h_full().w_full().child(EmptyState::new(hint))),
                )
            })
            .children(self.tiles);

        let content = div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            // The category bar sits above the header and spans the column, so a board scanned
            // left to right reads its statuses before it reads a single card.
            .children(self.accent.map(|accent| {
                div()
                    .flex_none()
                    .h(theme.metrics.focus_ring_w)
                    .w_full()
                    .bg(accent)
            }))
            .child(header)
            .child(body);

        div()
            .id(self.id)
            .flex()
            .flex_col()
            .flex_none()
            .h_full()
            .w(width)
            .overflow_hidden()
            .rounded(theme.radii.md)
            .bg(theme.colors.surface)
            .border(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(FocusRing::pane(self.focused).content(content))
    }
}

/// The horizontal scroller the columns live in.
#[derive(IntoElement)]
pub struct KanbanBoard {
    id: ElementId,
    columns: Vec<AnyElement>,
    scroll: Option<ScrollHandle>,
}

impl KanbanBoard {
    /// An empty board.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            columns: Vec::new(),
            scroll: None,
        }
    }

    /// The columns, in board order.
    pub fn columns(mut self, columns: impl IntoIterator<Item = AnyElement>) -> Self {
        self.columns = columns.into_iter().collect();
        self
    }

    /// Track the horizontal scroll offset, so the screen can reveal the focused column.
    pub fn scroll_handle(mut self, scroll: ScrollHandle) -> Self {
        self.scroll = Some(scroll);
        self
    }
}

impl RenderOnce for KanbanBoard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_stretch()
            .size_full()
            .min_h_0()
            .gap(theme.space.md)
            .p(theme.space.md)
            .overflow_x_scroll()
            .when_some(self.scroll, |el, scroll| el.track_scroll(&scroll))
            .children(self.columns)
    }
}
