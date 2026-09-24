//! `KanbanColumn` and `KanbanBoard` — the two containers of the board screen.
//!
//! A column is a rounded, hairlined well that scrolls in the other axis. Its header is one row:
//! a coloured dot for the column's category, the name as written (sentence case), the card count,
//! and an [`KanbanColumn::add_button`] slot at the right end — normally a `+` [`super::IconButton`] that
//! opens *New card* in this column. Under the header, a column whose entry starts something wears
//! a pill naming it in words ([`KanbanColumn::automation`]: `On enter: codex implements`). The
//! body is a **gapped stack of tiles**, not a list of flush rows, which is why this is not a
//! [`super::Pane`], and it ends in an optional [`KanbanColumn::footer`] — the `Add card` row.
//!
//! Neither container owns a key. `h` / `l` / `j` / `k` move a cursor the screen owns, exactly
//! as they do for [`super::ListView`]; these two only draw. Every pointer affordance is a slot
//! the screen fills, so a drop target or a drag handle can be added to a column without the
//! column knowing what a card is.
//!
//! Dragging a tile is the screen's too: it owns the drag, and tells the column two facts —
//! [`KanbanColumn::drop_target`], that the drag is over it (an accent hairline), and
//! [`KanbanColumn::drop_slot`], where the tile would land and what landing there does (a
//! [`super::DropSlot`] painted over the gap at that boundary without changing layout).

use std::sync::Arc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, Hsla, ListAlignment, ListState, Pixels, ScrollHandle,
    SharedString, Window, div, prelude::*, px,
};

use super::DropSlot;
use crate::{
    focus::FocusRing,
    icons::{Icon, IconSize},
    text::Text,
    theme::{ActiveTheme, ch},
    tone::Tone,
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

/// What a click on the automation pill runs.
type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// One board column.
#[derive(IntoElement)]
pub struct KanbanColumn {
    id: ElementId,
    title: SharedString,
    count: Option<usize>,
    automation: Option<SharedString>,
    on_automation: Option<ClickHandler>,
    accent: Option<Hsla>,
    focused: bool,
    drop_target: bool,
    drop_slot: Option<(usize, SharedString)>,
    width: Option<Pixels>,
    empty_hint: Option<SharedString>,
    add: Option<AnyElement>,
    footer: Option<AnyElement>,
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
    /// A column with a [`Self::footer`] draws it as the list's last item, so its state holds
    /// one item more than the column has tiles: [`Self::list_state_with_footer`] starts there.
    #[must_use]
    pub fn list_state() -> ListState {
        ListState::new(0, ListAlignment::Top, COLUMN_OVERDRAW)
    }

    /// A [`Self::list_state`] already holding the footer's item, for a column built with
    /// [`Self::footer`]. Tiles are spliced in before it: index `n` is always the footer.
    #[must_use]
    pub fn list_state_with_footer() -> ListState {
        let list = Self::list_state();
        list.splice(0..0, 1);
        list
    }

    /// A column titled `title`, written as the board names it.
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            count: None,
            automation: None,
            on_automation: None,
            accent: None,
            focused: false,
            drop_target: false,
            drop_slot: None,
            width: None,
            empty_hint: None,
            add: None,
            footer: None,
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

    /// What entering this column starts, in words: a pill under the header led by a `⚡`
    /// (`On enter: codex implements`). Unset, nothing is drawn.
    pub fn automation(mut self, label: impl Into<SharedString>) -> Self {
        self.automation = Some(label.into());
        self
    }

    /// Make the automation pill clickable: normally it opens the column's settings.
    pub fn on_automation_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_automation = Some(Box::new(handler));
        self
    }

    /// The column's category colour, as the dot before its name.
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

    /// Draw the column as the place a dragged tile is over: an accent hairline around the well.
    pub fn drop_target(mut self, drop_target: bool) -> Self {
        self.drop_target = drop_target;
        self
    }

    /// Draw a [`DropSlot`] saying `label` at the boundary before the tile at `index`; `index`
    /// equal to the tile count puts it after the last tile, above the footer. The marker is an
    /// absolute child of a relative tile wrapper, so it never changes a tile's measured height
    /// or the body's scroll extent. An empty column draws it inside its fixed-height body.
    pub fn drop_slot(mut self, index: usize, label: impl Into<SharedString>) -> Self {
        self.drop_slot = Some((index, label.into()));
        self
    }

    /// Override the [`COLUMN_WIDTH_CH`] width.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// What an empty column says, in muted text. Nothing is drawn when it is unset.
    pub fn empty_hint(mut self, empty_hint: impl Into<SharedString>) -> Self {
        self.empty_hint = Some(empty_hint.into());
        self
    }

    /// The header's trailing slot: normally a `+` [`super::IconButton`] that adds a card here.
    pub fn add_button(mut self, add: impl IntoElement) -> Self {
        self.add = Some(add.into_any_element());
        self
    }

    /// The row under the last tile: normally a ghost `Add card` button. In the virtualized
    /// [`Self::rows`] form it is the list's last item, so the caller's list holds `count + 1`.
    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
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
        let pill_id =
            ElementId::NamedChild(Arc::new(self.id.clone()), SharedString::new_static("pill"));

        let header = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(theme.space.sm)
            .h(theme.metrics.pane_header_h)
            .w_full()
            .px(theme.space.xs)
            .children(self.accent.map(|accent| {
                div()
                    .flex_none()
                    .size(theme.metrics.dot_size_small)
                    .rounded(theme.radii.pill)
                    .bg(accent)
            }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(theme.space.sm)
                    .child(Text::ui_strong(self.title).ellipsize())
                    .children(
                        self.count
                            .map(|count| Text::caption(count.to_string()).faint().flex_none()),
                    ),
            )
            .children(self.add.map(|add| div().flex_none().child(add)));

        let hover_bg = theme.colors.control_hover;
        let pill = self.automation.map(|label| {
            let clickable = self.on_automation.is_some();
            div()
                .id(pill_id)
                .flex()
                .flex_none()
                .self_start()
                .items_center()
                .gap(theme.space.xxs)
                .h(theme.metrics.tile_chip_h)
                .px(theme.space.sm)
                .mx(theme.space.xs)
                .rounded(theme.radii.control)
                .bg(theme.colors.control)
                .border(theme.metrics.hairline)
                .border_color(theme.colors.control_border)
                .child(Icon::Zap.el().size(IconSize::Small).tone(Tone::Warning))
                .child(Text::caption(label).muted())
                .when(clickable, |el| {
                    el.cursor_pointer().hover(move |style| style.bg(hover_bg))
                })
                .when_some(self.on_automation, |el, handler| {
                    el.on_click(move |event, window, cx| handler(event, window, cx))
                })
        });

        let mut footer = self.footer;
        let slot = self.drop_slot;
        let hint = self.empty_hint.filter(|_| slot.is_none()).map(|hint| {
            div()
                .px(theme.space.sm)
                .py(theme.space.xs)
                .child(Text::caption(hint).faint())
        });
        let body = div()
            .id(body_id)
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full();
        let body = match self.rows {
            // The gap lives on each row, not on the body: a virtualized list lays its items out
            // itself, so a container gap would apply to nothing. The footer is the item after
            // the last tile, so it follows the cards instead of sinking to the column's floor.
            // The marker is absolute inside a relative card wrapper, so moving it neither
            // remeasures a row nor splices an item into the list.
            Some((list, count, mut render_row)) if count > 0 => body.child(
                gpui::list(list, move |index, window, cx| {
                    let item = if index < count {
                        render_row(index, window, cx)
                    } else {
                        footer.take().unwrap_or_else(|| div().into_any_element())
                    };
                    let before = slot
                        .as_ref()
                        .filter(|(at, _)| *at < count && *at == index)
                        .map(|(_, label)| {
                            let marker = DropSlot::new(label.clone());
                            if index == 0 {
                                marker.top()
                            } else {
                                marker.before()
                            }
                        });
                    let after = slot
                        .as_ref()
                        .filter(|(at, _)| index + 1 == count && *at == count)
                        .map(|(_, label)| DropSlot::new(label.clone()).after());
                    let tile = div()
                        .relative()
                        .w_full()
                        .child(item)
                        .children(before)
                        .children(after);
                    div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .pb(gap)
                        .child(tile)
                        .into_any_element()
                })
                .size_full(),
            ),
            Some(_) => body
                .relative()
                .gap(gap)
                .children(hint)
                .children(slot.map(|(_, label)| DropSlot::new(label)))
                .children(footer),
            None => {
                let count = self.tiles.len();
                let tiles = self.tiles.into_iter().enumerate().map(|(index, tile)| {
                    let before = slot
                        .as_ref()
                        .filter(|(at, _)| *at < count && *at == index)
                        .map(|(_, label)| DropSlot::new(label.clone()).before());
                    let after = slot
                        .as_ref()
                        .filter(|(at, _)| index + 1 == count && *at == count)
                        .map(|(_, label)| DropSlot::new(label.clone()).after());
                    div()
                        .relative()
                        .flex()
                        .flex_col()
                        .w_full()
                        .child(tile)
                        .children(before)
                        .children(after)
                });
                body.gap(gap)
                    .overflow_y_scroll()
                    .when_some(self.scroll, |el, scroll| el.track_scroll(&scroll))
                    .when(empty, |el| el.relative())
                    .when(empty, |el| el.children(hint))
                    .children(tiles)
                    .when(empty, |el| {
                        el.children(slot.map(|(_, label)| DropSlot::new(label)))
                    })
                    .children(footer)
            }
        };

        let content = div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .gap(gap)
            .p(pad)
            .child(header)
            .children(pill)
            .child(body);

        div()
            .id(self.id)
            .flex()
            .flex_col()
            .flex_none()
            .h_full()
            .w(width)
            .overflow_hidden()
            .rounded(theme.radii.dialog)
            .bg(theme.colors.surface)
            .border(theme.metrics.hairline)
            .border_color(if self.drop_target {
                theme.colors.accent
            } else {
                theme.colors.border
            })
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
