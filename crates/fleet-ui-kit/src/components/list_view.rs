//! `ListView` — a virtualized list of fixed-height rows with a cursor.
//!
//! Built on `gpui::uniform_list`, which is the right primitive for "one element per item at a
//! known height". (It is **not** right for a terminal grid; see [`super::TerminalGrid`].)
//!
//! Three invariants of §3.3 live here:
//!
//! * **Cursor stability.** Background events never re-sort, re-scroll or re-focus the list.
//!   [`ListCursor`] therefore only ever moves on an explicit call, and [`ListCursor::retain`]
//!   is how a view keeps the cursor on the same item across a data refresh.
//! * **Scrolloff 2.** The cursor never sits in the last two visible rows while there is more
//!   list below it, nor in the first two while there is more above.
//!   [`ListCursor::scroll_target`] computes the index to reveal and [`ListView::reveal`]
//!   reveals it with [`gpui::ScrollStrategy::Nearest`], so the list scrolls by the minimum
//!   amount instead of jumping the cursor to an edge.
//! * **The pointer mirrors the keys (ADR 0023, UX-SPEC §5.1).** A press on a row moves the
//!   cursor there, a double-click opens it like `⏎`, a right click opens its menu. The list
//!   routes the press through one [`ListPointer`], so a screen writes only `on_select(ix)`,
//!   `on_open(ix)` and `on_menu(ix, position)`, never a per-row closure.
//! * **The parent owns the keys.** `j` / `k` / `gg` / `G` / `ctrl-d` / `ctrl-u` are gpui
//!   actions ([`ListDown`] and friends) that the view binds and dispatches; the element never
//!   listens for a key itself, because the same six motions drive lists that live in a pane,
//!   in a dialog and in the palette.
//!
//! ```ignore
//! // in the view's action handler, never in `render`:
//! fn page_down(&mut self, _: &ListPageDown, _window: &mut Window, cx: &mut Context<Self>) {
//!     let moving_down = self.cursor.motion(ListMotion::PageDown);
//!     ListView::reveal(&self.scroll, &self.cursor, moving_down);
//!     cx.notify();
//! }
//! ```

use gpui::{
    AnyElement, App, ElementId, KeyBinding, MouseButton, MouseDownEvent, Pixels, Point,
    ScrollStrategy, UniformListScrollHandle, Window, div, prelude::*, uniform_list,
};
use std::rc::Rc;

use crate::{
    components::{Row, SkeletonRows},
    theme::ActiveTheme,
};

gpui::actions!(
    fleet_list,
    [
        /// `j` / `↓`: move the cursor one row down.
        ListDown,
        /// `k` / `↑`: move the cursor one row up.
        ListUp,
        /// `gg`: move the cursor to the first row.
        ListFirst,
        /// `G`: move the cursor to the last row.
        ListLast,
        /// `ctrl-d`: move the cursor half a viewport down.
        ListPageDown,
        /// `ctrl-u`: move the cursor half a viewport up.
        ListPageUp,
    ]
);

/// The six motions a list cursor understands, one per action.
///
/// The enum exists so a view can route all six actions through one call site and get back the
/// direction [`ListView::reveal`] needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListMotion {
    /// `j`.
    Down,
    /// `k`.
    Up,
    /// `gg`.
    First,
    /// `G`.
    Last,
    /// `ctrl-d`.
    PageDown,
    /// `ctrl-u`.
    PageUp,
}

impl ListMotion {
    /// Whether this motion moves toward the end of the list, which decides on which side of the
    /// cursor the scrolloff margin is kept.
    pub fn moves_down(self) -> bool {
        matches!(
            self,
            ListMotion::Down | ListMotion::Last | ListMotion::PageDown
        )
    }
}

/// The default bindings for the six list actions, in the given gpui key context.
///
/// `context` is a key-context predicate such as `Some("Hub > Worktrees")`; `None` binds them
/// globally, which is only ever right in an example or a test bench. A list that lives **under
/// a text field** must not use these: §4 of the design system requires `ctrl-n` / `ctrl-p`
/// there, and [`super::FuzzyList::binds_jk`] states which case a list is in.
pub fn list_key_bindings(context: Option<&str>) -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("j", ListDown, context),
        KeyBinding::new("down", ListDown, context),
        KeyBinding::new("k", ListUp, context),
        KeyBinding::new("up", ListUp, context),
        KeyBinding::new("g g", ListFirst, context),
        KeyBinding::new("shift-g", ListLast, context),
        KeyBinding::new("ctrl-d", ListPageDown, context),
        KeyBinding::new("ctrl-u", ListPageUp, context),
    ]
}

/// Row renderer: `(index, is_cursor, window, cx) -> element`.
type RenderRow = Rc<dyn Fn(usize, bool, &mut Window, &mut App) -> AnyElement>;

/// The scrolloff every Fleet list uses (§3.3).
pub const SCROLLOFF: usize = 2;

/// How far `ctrl-d` / `ctrl-u` jump before the view has measured its viewport.
pub const DEFAULT_PAGE: usize = 10;

/// How many placeholder rows a cold load draws (§3.5: the PR list's cold fetch).
pub const SKELETON_ROWS: usize = 6;

/// A list cursor with `j`/`k`/`gg`/`G`/`ctrl-d`/`ctrl-u` semantics and scrolloff.
///
/// Pure logic: no gpui types, fully unit-testable, and owned by the view's entity rather than
/// by the element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListCursor {
    index: usize,
    len: usize,
    scrolloff: usize,
    page: usize,
}

impl Default for ListCursor {
    fn default() -> Self {
        Self::new(0)
    }
}

impl ListCursor {
    /// A cursor over `len` items, at index 0, with the spec's scrolloff of 2.
    pub fn new(len: usize) -> Self {
        Self {
            index: 0,
            len,
            scrolloff: SCROLLOFF,
            page: DEFAULT_PAGE,
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

    /// Set the page from the number of rows the pane can show: `ctrl-d` is a **half** page, so
    /// a 24-row viewport pages by 12. Call it when the pane is measured or resized.
    pub fn set_page_from_visible(&mut self, visible_rows: usize) {
        self.page = (visible_rows / 2).max(1);
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

    /// The scrolloff in effect.
    pub fn scrolloff_rows(&self) -> usize {
        self.scrolloff
    }

    /// How far one `ctrl-d` / `ctrl-u` jumps.
    pub fn page_rows(&self) -> usize {
        self.page
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

    /// Apply one motion and return whether it moved **down**, which is what
    /// [`ListView::reveal`] needs to know.
    ///
    /// This is the single call site a view needs for all six actions.
    pub fn motion(&mut self, motion: ListMotion) -> bool {
        match motion {
            ListMotion::Down => self.down(),
            ListMotion::Up => self.up(),
            ListMotion::First => self.first(),
            ListMotion::Last => self.last(),
            ListMotion::PageDown => self.page_down(),
            ListMotion::PageUp => self.page_up(),
        }
        motion.moves_down()
    }

    /// Adopt a new length without moving the cursor off the item it was on.
    ///
    /// `find_previous` receives the previously selected index and returns where that item went
    /// in the new data, if it is still present. This is the mechanism behind "a row that
    /// changes state changes its glyph in place": the data can change under the cursor without
    /// moving it.
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
    /// Revealing `index + scrolloff` when moving down keeps two rows of context below the
    /// cursor; `index - scrolloff` does the same above. Both are clamped to the list, so the
    /// margin collapses at the ends instead of refusing to scroll.
    pub fn scroll_target(&self, moving_down: bool) -> usize {
        if moving_down {
            (self.index + self.scrolloff).min(self.len.saturating_sub(1))
        } else {
            self.index.saturating_sub(self.scrolloff)
        }
    }
}

/// What one press on a list row asks for (UX-SPEC §5.1).
///
/// Pure: derived from the button and the click count alone, so the whole pointer contract is
/// unit-testable without a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowPress {
    /// A single primary press: move the cursor to the row.
    Select,
    /// The second press of a double-click: move the cursor there and open the row, like `⏎`.
    Open,
    /// A right click: move the cursor there and open the row's menu.
    Menu,
}

impl RowPress {
    /// Classify a press. `None` for a button rows do not answer (middle, back, forward).
    pub fn classify(button: MouseButton, click_count: usize) -> Option<Self> {
        match button {
            MouseButton::Left if click_count >= 2 => Some(Self::Open),
            MouseButton::Left => Some(Self::Select),
            MouseButton::Right => Some(Self::Menu),
            _ => None,
        }
    }
}

/// `(index, window, cx)`: a row-indexed pointer handler.
type IndexHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;
/// `(index, position, window, cx)`: the menu handler, with the window position to anchor at.
type MenuHandler = Rc<dyn Fn(usize, Point<Pixels>, &mut Window, &mut App)>;

/// The three pointer handlers of a list, shared by every row it draws.
///
/// Built once per frame by the view (or by [`ListView`]'s `on_*` builders) and cloned into each
/// visible row — the handlers are `Rc`, because one closure serves every row. Each press first
/// moves the cursor ([`ListPointer::on_select`]), then opens or shows the menu, so the view
/// never has to remember to select before acting. A view whose rows are not in a [`ListView`]
/// (a short sidebar, a board column) wires them with [`ListPointer::attach`].
#[derive(Clone, Default)]
pub struct ListPointer {
    select: Option<IndexHandler>,
    open: Option<IndexHandler>,
    menu: Option<MenuHandler>,
}

impl ListPointer {
    /// A pointer contract with no handlers: rows stay inert until one is set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Move the cursor to `ix`. Normally `ListCursor::set(ix)`, focus the list's pane, reveal,
    /// `cx.notify()` — exactly what `j`/`k` do, minus the motion.
    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.select = Some(Rc::new(handler));
        self
    }

    /// Open row `ix`: the same action `⏎` dispatches on the cursor row. Runs after
    /// [`ListPointer::on_select`], so the handler may read the cursor.
    pub fn on_open(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.open = Some(Rc::new(handler));
        self
    }

    /// Show row `ix`'s menu at `position` (window coordinates). Runs after
    /// [`ListPointer::on_select`], so the menu acts on the cursor row like its keys do.
    pub fn on_menu(
        mut self,
        handler: impl Fn(usize, Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.menu = Some(Rc::new(handler));
        self
    }

    /// Whether no handler is set, in which case rows get no pointer wiring at all.
    pub fn is_empty(&self) -> bool {
        self.select.is_none() && self.open.is_none() && self.menu.is_none()
    }

    /// Route one press on row `ix`: select, then open or menu as [`RowPress::classify`] says.
    pub fn press(&self, ix: usize, event: &MouseDownEvent, window: &mut Window, cx: &mut App) {
        let Some(press) = RowPress::classify(event.button, event.click_count) else {
            return;
        };
        if let Some(select) = &self.select {
            select(ix, window, cx);
        }
        match press {
            RowPress::Select => {}
            RowPress::Open => {
                if let Some(open) = &self.open {
                    open(ix, window, cx);
                }
            }
            RowPress::Menu => {
                if let Some(menu) = &self.menu {
                    menu(ix, event.position, window, cx);
                }
            }
        }
    }

    /// Wire row `ix` to these handlers through [`Row::on_click`], [`Row::on_double_click`] and
    /// [`Row::on_secondary_click`].
    pub fn attach(&self, ix: usize, row: Row) -> Row {
        let handler = |pointer: &Self| {
            let pointer = pointer.clone();
            move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                pointer.press(ix, event, window, cx)
            }
        };
        row.when(self.select.is_some() || self.open.is_some(), |row| {
            row.on_click(handler(self))
        })
        .when(self.open.is_some(), |row| {
            row.on_double_click(handler(self))
        })
        .when(self.menu.is_some(), |row| {
            row.on_secondary_click(handler(self))
        })
    }

    /// Wrap an already-built row element so a press anywhere on it routes to `ix`. This is how
    /// [`ListView`] serves rows it receives as `AnyElement`s; a view that builds its own
    /// [`Row`] should prefer [`ListPointer::attach`].
    fn wrap(&self, ix: usize, row: AnyElement) -> AnyElement {
        let primary = self.clone();
        let secondary = self.clone();
        div()
            .w_full()
            .cursor_pointer()
            .when(self.select.is_some() || self.open.is_some(), |el| {
                el.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    primary.press(ix, event, window, cx)
                })
            })
            .when(self.menu.is_some(), |el| {
                el.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    secondary.press(ix, event, window, cx)
                })
            })
            .child(row)
            .into_any_element()
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
    loading: bool,
    skeleton_rows: usize,
    pointer: ListPointer,
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
            loading: false,
            skeleton_rows: SKELETON_ROWS,
            pointer: ListPointer::new(),
        }
    }

    /// A press on row `ix` moves the cursor there. See [`ListPointer::on_select`].
    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.pointer = self.pointer.on_select(handler);
        self
    }

    /// A double-click on row `ix` opens it, after selecting it. See [`ListPointer::on_open`].
    pub fn on_open(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.pointer = self.pointer.on_open(handler);
        self
    }

    /// A right click on row `ix` opens its menu, after selecting it. See
    /// [`ListPointer::on_menu`].
    pub fn on_menu(
        mut self,
        handler: impl Fn(usize, Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.pointer = self.pointer.on_menu(handler);
        self
    }

    /// Use a [`ListPointer`] the view already built, e.g. one it also hands to a detail panel.
    pub fn pointer(mut self, pointer: ListPointer) -> Self {
        self.pointer = pointer;
        self
    }

    /// Which index carries the cursor.
    pub fn cursor(mut self, index: usize) -> Self {
        self.cursor = Some(index);
        self
    }

    /// Override the row height. Sizes the skeleton and the empty-state box too;
    /// `uniform_list` measures the first rendered row itself.
    pub fn row_height(mut self, height: Pixels) -> Self {
        self.row_height = Some(height);
        self
    }

    /// Track scrolling, so the view can call [`ListView::reveal`] after a cursor move.
    pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self {
        self.scroll = Some(handle.clone());
        self
    }

    /// What to show when `item_count` is 0. Normally a [`super::EmptyState`].
    pub fn empty(mut self, empty: impl IntoElement) -> Self {
        self.empty = Some(empty.into_any_element());
        self
    }

    /// Cold load: draw [`SkeletonRows`] instead of the list.
    ///
    /// **Cold only.** Everything in Fleet except a first PR fetch renders from `state.json`
    /// immediately, and a skeleton where cached truth exists is a lie (§6.3).
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// How many placeholder rows the loading state draws. 6 by default.
    pub fn skeleton_rows(mut self, rows: usize) -> Self {
        self.skeleton_rows = rows;
        self
    }

    /// Reveal the cursor in `handle`, honouring the scrolloff.
    ///
    /// Call this from the action handler that moved the cursor, never from `render`: scrolling
    /// during layout is how a background refresh ends up moving the viewport.
    /// [`gpui::ScrollStrategy::Nearest`] scrolls by the minimum amount, so a cursor that is
    /// already inside the margin does not move the list at all.
    pub fn reveal(handle: &UniformListScrollHandle, cursor: &ListCursor, moving_down: bool) {
        handle.scroll_to_item(cursor.scroll_target(moving_down), ScrollStrategy::Nearest);
    }
}

impl RenderOnce for ListView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let row_height = self.row_height.unwrap_or(theme.metrics.row_h);

        if self.loading {
            return div()
                .size_full()
                .child(SkeletonRows::new(self.skeleton_rows).row_height(row_height))
                .into_any_element();
        }

        if self.item_count == 0 {
            return div()
                .size_full()
                .min_h(row_height)
                .children(self.empty)
                .into_any_element();
        }

        let render_row = self.render_row;
        let cursor = self.cursor;
        // No handlers, no wrapper: a keyboard-only list keeps exactly the element tree it had.
        let pointer = (!self.pointer.is_empty()).then_some(self.pointer);
        let list = uniform_list(self.id, self.item_count, move |range, window, cx| {
            range
                .map(|ix| {
                    let row = render_row(ix, cursor == Some(ix), window, cx);
                    match &pointer {
                        Some(pointer) => pointer.wrap(ix, row),
                        None => row,
                    }
                })
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
    use super::{ListCursor, ListMotion, RowPress};
    use gpui::MouseButton;

    #[test]
    fn a_press_classifies_into_select_open_or_menu() {
        assert_eq!(
            RowPress::classify(MouseButton::Left, 1),
            Some(RowPress::Select)
        );
        assert_eq!(
            RowPress::classify(MouseButton::Left, 2),
            Some(RowPress::Open)
        );
        assert_eq!(
            RowPress::classify(MouseButton::Left, 3),
            Some(RowPress::Open)
        );
        assert_eq!(
            RowPress::classify(MouseButton::Right, 1),
            Some(RowPress::Menu)
        );
        assert_eq!(RowPress::classify(MouseButton::Middle, 1), None);
    }

    #[test]
    fn default_cursor_can_page_after_population() {
        let mut cursor = ListCursor::default();
        cursor.set_len(100);
        cursor.page_down();
        assert_eq!(cursor.index(), super::DEFAULT_PAGE);
        assert_eq!(cursor.scrolloff_rows(), super::SCROLLOFF);
    }

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
        cursor.set(1);
        assert_eq!(cursor.scroll_target(false), 0);
    }

    #[test]
    fn motions_report_their_direction() {
        let mut cursor = ListCursor::new(40).page(10);
        assert!(cursor.motion(ListMotion::PageDown));
        assert_eq!(cursor.index(), 10);
        assert!(!cursor.motion(ListMotion::PageUp));
        assert_eq!(cursor.index(), 0);
        assert!(cursor.motion(ListMotion::Last));
        assert_eq!(cursor.index(), 39);
        assert!(!cursor.motion(ListMotion::First));
        assert_eq!(cursor.index(), 0);
    }

    #[test]
    fn ctrl_d_is_half_a_viewport() {
        let mut cursor = ListCursor::new(100);
        cursor.set_page_from_visible(24);
        assert_eq!(cursor.page_rows(), 12);
        cursor.set_page_from_visible(1);
        assert_eq!(cursor.page_rows(), 1);
    }
}
