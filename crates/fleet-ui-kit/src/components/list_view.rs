//! `ListView` — a virtualized list of fixed-height rows with a cursor.
//!
//! Built on `gpui::uniform_list`, which is the right primitive for "one element per item at a
//! known height". (It is **not** right for a terminal grid; see
//! [`super::TerminalGrid`].)
//!
//! Two invariants of §3.3 live here:
//!
//! * **Cursor stability.** Background events never re-sort, re-scroll or re-focus the list.
//!   [`ListCursor`] therefore only ever moves on an explicit call, and
//!   [`ListCursor::retain`] is how a view keeps the cursor on the same item across a data
//!   refresh.
//! * **Scrolloff 2.** The cursor never sits in the first or last two visible rows while there
//!   is more list to show; [`ListCursor::scroll_target`] computes the index to scroll to.

use gpui::{
    AnyElement, App, ElementId, Pixels, ScrollStrategy, UniformListScrollHandle, Window, div,
    prelude::*, uniform_list,
};
use std::rc::Rc;

use crate::theme::ActiveTheme;

/// Row renderer: `(index, is_cursor, window, cx) -> element`.
pub type RenderRow = Rc<dyn Fn(usize, bool, &mut Window, &mut App) -> AnyElement>;

/// A list cursor with `j`/`k`/`gg`/`G`/`ctrl-d`/`ctrl-u` semantics and scrolloff.
///
/// Pure logic: no gpui types, fully unit-testable, and owned by the view's entity rather than
/// by the element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ListCursor {
    index: usize,
    len: usize,
    scrolloff: usize,
    page: usize,
}

impl ListCursor {
    /// A cursor over `len` items, at index 0, with the spec's scrolloff of 2.
    pub fn new(len: usize) -> Self {
        Self {
            index: 0,
            len,
            scrolloff: 2,
            page: 10,
        }
    }

    /// Change the scrolloff.
    pub fn scrolloff(mut self, scrolloff: usize) -> Self {
        self.scrolloff = scrolloff;
        self
    }

    /// Change how far `ctrl-d` / `ctrl-u` jump.
    pub fn page(mut self, page: usize) -> Self {
        self.page = page.max(1);
        self
    }

    /// The current index. Always `< len`, or 0 when the list is empty.
    pub fn index(&self) -> usize {
        self.index
    }

    /// How many items the cursor spans.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Move to an index, clamped.
    pub fn set(&mut self, index: usize) {
        self.index = index.min(self.len.saturating_sub(1));
    }

    /// `j`.
    pub fn down(&mut self) {
        self.set(self.index.saturating_add(1));
    }

    /// `k`.
    pub fn up(&mut self) {
        self.index = self.index.saturating_sub(1);
    }

    /// `gg`.
    pub fn first(&mut self) {
        self.index = 0;
    }

    /// `G`.
    pub fn last(&mut self) {
        self.index = self.len.saturating_sub(1);
    }

    /// `ctrl-d`.
    pub fn page_down(&mut self) {
        self.set(self.index.saturating_add(self.page));
    }

    /// `ctrl-u`.
    pub fn page_up(&mut self) {
        self.index = self.index.saturating_sub(self.page);
    }

    /// Adopt a new length without moving the cursor off the item it was on.
    ///
    /// `find_previous` receives the new length and returns where the previously selected item
    /// went, if it is still present. This is the mechanism behind "a row that changes state
    /// changes its glyph in place": the data can change under the cursor without moving it.
    pub fn retain(&mut self, new_len: usize, find_previous: impl FnOnce(usize) -> Option<usize>) {
        let previous = self.index;
        self.len = new_len;
        self.index = find_previous(previous)
            .unwrap_or(previous)
            .min(new_len.saturating_sub(1));
    }

    /// Set the length, clamping the cursor.
    pub fn set_len(&mut self, len: usize) {
        self.len = len;
        self.index = self.index.min(len.saturating_sub(1));
    }

    /// The index a scroll handle should be told to reveal, honouring the scrolloff.
    ///
    /// Scrolling to `index + scrolloff` (clamped) keeps two rows of context below the cursor
    /// when moving down, and `index - scrolloff` does the same above.
    pub fn scroll_target(&self, moving_down: bool) -> usize {
        if moving_down {
            (self.index + self.scrolloff).min(self.len.saturating_sub(1))
        } else {
            self.index.saturating_sub(self.scrolloff)
        }
    }
}

/// A virtualized list.
#[derive(IntoElement)]
pub struct ListView {
    id: ElementId,
    item_count: usize,
    row_height: Option<Pixels>,
    cursor: Option<usize>,
    scroll: Option<UniformListScrollHandle>,
    render_row: RenderRow,
    empty: Option<AnyElement>,
}

impl ListView {
    /// A list of `item_count` rows.
    ///
    /// `render_row` receives the item index and whether that index is the cursor, so a view
    /// never has to compare indices itself.
    pub fn new(
        id: impl Into<ElementId>,
        item_count: usize,
        render_row: impl Fn(usize, bool, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            item_count,
            row_height: None,
            cursor: None,
            scroll: None,
            render_row: Rc::new(render_row),
            empty: None,
        }
    }

    /// Which index carries the cursor.
    pub fn cursor(mut self, index: usize) -> Self {
        self.cursor = Some(index);
        self
    }

    /// Override the row height. Only affects the empty-state box; `uniform_list` measures the
    /// first rendered row itself.
    pub fn row_height(mut self, height: Pixels) -> Self {
        self.row_height = Some(height);
        self
    }

    /// Track scrolling, so the view can call `scroll_to_item` after a cursor move.
    pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self {
        self.scroll = Some(handle.clone());
        self
    }

    /// What to show when `item_count` is 0. Normally a [`super::EmptyState`].
    pub fn empty(mut self, empty: impl IntoElement) -> Self {
        self.empty = Some(empty.into_any_element());
        self
    }

    /// Reveal `index` in `handle`, honouring the scrolloff of `cursor`.
    ///
    /// Call this from the action handler that moved the cursor, never from `render`.
    pub fn reveal(handle: &UniformListScrollHandle, cursor: &ListCursor, moving_down: bool) {
        handle.scroll_to_item(cursor.scroll_target(moving_down), ScrollStrategy::Top);
    }
}

impl RenderOnce for ListView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.item_count == 0 {
            let theme = cx.theme();
            return div()
                .size_full()
                .min_h(self.row_height.unwrap_or(theme.metrics.row_h))
                .children(self.empty)
                .into_any_element();
        }

        let render_row = self.render_row.clone();
        let cursor = self.cursor;
        let list = uniform_list(self.id, self.item_count, move |range, window, cx| {
            range
                .map(|ix| render_row(ix, cursor == Some(ix), window, cx))
                .collect::<Vec<_>>()
        })
        .size_full();

        match self.scroll {
            Some(handle) => list.track_scroll(&handle).into_any_element(),
            None => list.into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ListCursor;

    #[test]
    fn moves_and_clamps() {
        let mut cursor = ListCursor::new(3);
        cursor.down();
        cursor.down();
        cursor.down();
        assert_eq!(cursor.index(), 2);
        cursor.up();
        cursor.up();
        cursor.up();
        assert_eq!(cursor.index(), 0);
        cursor.last();
        assert_eq!(cursor.index(), 2);
    }

    #[test]
    fn empty_list_stays_at_zero() {
        let mut cursor = ListCursor::new(0);
        cursor.down();
        cursor.last();
        assert_eq!(cursor.index(), 0);
        assert!(cursor.is_empty());
    }

    #[test]
    fn retain_follows_the_item() {
        let mut cursor = ListCursor::new(5);
        cursor.set(3);
        cursor.retain(5, |previous| Some(previous + 1));
        assert_eq!(cursor.index(), 4);
        cursor.retain(2, |_| None);
        assert_eq!(cursor.index(), 1);
    }

    #[test]
    fn scrolloff_keeps_context() {
        let mut cursor = ListCursor::new(20);
        cursor.set(10);
        assert_eq!(cursor.scroll_target(true), 12);
        assert_eq!(cursor.scroll_target(false), 8);
        cursor.set(19);
        assert_eq!(cursor.scroll_target(true), 19);
    }
}
