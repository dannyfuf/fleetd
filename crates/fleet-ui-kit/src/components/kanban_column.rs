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
    AnyElement, App, ElementId, Hsla, ListAlignment, ListState, Pixels, ScrollHandle, SharedString,
    Window, div, prelude::*, px,
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

/// How far above and below the viewport a virtualized column measures its tiles.
///
/// One tile is ~60-100 px, so this is between two and four rows of runway on each side: enough
/// that a held `j` never lands on an unmeasured row, and far short of measuring the column.
const COLUMN_OVERDRAW: Pixels = px(200.0);

/// Builds the tile at `index`. Only the rows a virtualized column can show are asked for.
type RowRenderer = Box<dyn FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static>;

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
    rows: Option<(ListState, usize, RowRenderer)>,
    scroll: Option<ScrollHandle>,
}

impl KanbanColumn {
    /// The [`ListState`] a virtualized column body wants: top-aligned, with the column's own
    /// overdraw.
    ///
    /// The caller owns it, because it carries the scroll offset and the measured tile heights
    /// from one frame to the next and the column itself is a `RenderOnce` that owns nothing.
    #[must_use]
    pub fn list_state() -> ListState {
        ListState::new(0, ListAlignment::Top, COLUMN_OVERDRAW)
    }

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
            rows: None,
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

    /// The tiles this column holds, built only for the rows the viewport can show.
    ///
    /// The lazy form of [`Self::tiles`], and the one a board of any size has to use: a column
    /// given 300 finished elements builds, boxes and measures 300 of them every frame to show
    /// twelve. `list` is the caller-owned [`ListState`] — it carries the scroll offset and the
    /// measured heights, so the caller splices it when `count` changes and reveals the cursor
    /// through it. [`gpui::list`] rather than `uniform_list` because a tile's height is not
    /// uniform: its title wraps to one line or two and its meta row is zero-suppressed.
    pub fn rows(
        mut self,
        list: ListState,
        count: usize,
        render_row: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.rows = Some((list, count, Box::new(render_row)));
        self
    }

    /// Track the body's scroll offset, so the screen can reveal the cursor card after a move.
    ///
    /// Only the eager [`Self::tiles`] form scrolls this way; a column built from [`Self::rows`]
    /// scrolls through its own [`ListState`].
    pub fn scroll_handle(mut self, scroll: ScrollHandle) -> Self {
        self.scroll = Some(scroll);
        self
    }
}

impl RenderOnce for KanbanColumn {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let width = self.width.unwrap_or_else(|| ch(COLUMN_WIDTH_CH));
        let (gap, pad) = (theme.space.sm, theme.space.sm);
        let empty = self
            .rows
            .as_ref()
            .map_or_else(|| self.tiles.is_empty(), |(_, count, _)| *count == 0);
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

        let hint = self
            .empty_hint
            .map(|hint| div().h_full().w_full().child(EmptyState::new(hint)));
        let body = div()
            .id(body_id)
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .p(pad)
            .when(empty, |el| el.children(hint));
        let body = match self.rows {
            // The gap lives on each row, not on the body: a virtualized list lays its items out
            // itself, so a container gap would apply to nothing.
            Some((list, count, mut render_row)) if count > 0 => body.child(
                gpui::list(list, move |index, window, cx| {
                    div()
                        .pb(gap)
                        .child(render_row(index, window, cx))
                        .into_any_element()
                })
                .size_full(),
            ),
            Some(_) => body,
            None => body
                .gap(gap)
                .overflow_y_scroll()
                .when_some(self.scroll, |el, scroll| el.track_scroll(&scroll))
                .children(self.tiles),
        };

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

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{AppContext, Entity, Point, size};

    use super::*;

    /// A backlog nobody would page through by hand, and the shape the finding names.
    const ROWS: usize = 500;
    /// Tall enough for a handful of tiles, which is what a column gets in the shell.
    const VIEWPORT_H: f32 = 400.0;

    /// A root view for the virtualization test: [`gpui::list`] only lays out inside a rendered
    /// entity, which `VisualTestContext::draw` provides through a view.
    struct ColumnHarness {
        list: ListState,
        built: Rc<Cell<usize>>,
    }

    impl gpui::Render for ColumnHarness {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            let built = Rc::clone(&self.built);
            div()
                .size_full()
                .child(KanbanColumn::new("column", "Backlog").count(ROWS).rows(
                    self.list.clone(),
                    ROWS,
                    move |index, _, _| {
                        built.set(built.get() + 1);
                        div()
                            .h(px(60.0))
                            .child(Text::label(format!("row {index}")))
                            .into_any_element()
                    },
                ))
        }
    }

    #[gpui::test]
    fn a_column_builds_only_the_tiles_its_viewport_can_show(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(crate::Theme::dark()));
        let list = KanbanColumn::list_state();
        list.splice(0..0, ROWS);
        let built = Rc::new(Cell::new(0));
        let cx = cx.add_empty_window();
        let harness: Entity<ColumnHarness> = cx.new(|_| ColumnHarness {
            list: list.clone(),
            built: Rc::clone(&built),
        });
        let view = harness.clone();
        cx.draw(
            Point::default(),
            size(ch(COLUMN_WIDTH_CH), px(VIEWPORT_H)),
            |_, _| view.into_any_element(),
        );

        assert!(built.get() > 0, "the visible tiles must be built");
        assert!(
            built.get() < 40,
            "a {VIEWPORT_H} px viewport built {} of {ROWS} tiles",
            built.get()
        );
        assert!(
            list.bounds_for_item(ROWS - 1).is_none(),
            "a tile below the overdraw must not be laid out at all"
        );
    }
}
