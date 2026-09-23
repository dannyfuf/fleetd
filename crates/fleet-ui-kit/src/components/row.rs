//! `Row` — leading glyph slot, flex content, trailing columns.
//!
//! Every list in Fleet is made of these, at 30 px (44 px for a two-line job row or a
//! [`Row::comfortable`] Hub row, 34 px for a palette row). The row owns its own selected /
//! cursor / hover / dimmed / disabled rendering so that a state change is a glyph change
//! **in place** and never a re-sort or a re-layout (§3.3 "cursor stability").
//!
//! The row is pointer-first (ADR 0023): it hovers without an id, and
//! [`Row::on_click`] / [`Row::on_double_click`] / [`Row::on_secondary_click`] give it the
//! UX-SPEC §5.1 contract — a click selects, a double-click opens, a right click opens the
//! row's menu. A list that owns its rows' indices wires all three through one
//! [`super::ListPointer`] instead of three closures per row. [`Row::hover_actions`] is the
//! trailing slot for the buttons the pointer sees on hover; every one of them must also be in
//! the row's menu, so nothing lives only behind hover.
//!
//! Columns are the §2.9 ladder made concrete: build them from
//! [`ColumnLadder::resolve`](super::ColumnLadder::resolve) and
//! [`RowColumn::resolved`], so the pane width — never the window width — decides which columns
//! a row draws.

use gpui::{
    AnyElement, App, ElementId, MouseButton, MouseDownEvent, Pixels, Window, div, prelude::*,
};

use crate::{
    components::{FocusRing, ResolvedColumn},
    theme::{ActiveTheme, ch},
};

/// The glyph column of every list, in `ch` (§2.9 column 1).
pub const GLYPH_COLUMN_CH: f32 = 2.0;

/// The hover group every row names itself with, so a [`RowColumn::hover_only`] cell can reveal
/// itself while *its own* row is hovered. gpui resolves a group name to the innermost enclosing
/// element that declared it, so one name is enough for any number of sibling rows.
const ROW_GROUP: &str = "fleet-row";

/// A pointer handler on a row. It receives the raw press, so a view can hand it a
/// `cx.listener(..)` directly.
type PressHandler = Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// How a column's content sits in its box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColumnAlign {
    /// Left aligned. The default.
    #[default]
    Left,
    /// Centered. Only the glyph column.
    Center,
    /// Right aligned. Counts and ages.
    Right,
}

/// One trailing column of a row.
pub struct RowColumn {
    element: AnyElement,
    width: Option<Pixels>,
    min_width: Option<Pixels>,
    align: ColumnAlign,
    flex: bool,
    hover_only: bool,
}

impl RowColumn {
    fn with(element: AnyElement, width: Option<Pixels>, flex: bool) -> Self {
        Self {
            element,
            width,
            min_width: None,
            align: ColumnAlign::Left,
            flex,
            hover_only: false,
        }
    }

    /// A fixed-width column.
    pub fn fixed(width: Pixels, element: impl IntoElement) -> Self {
        Self::with(element.into_any_element(), Some(width), false)
    }

    /// A fixed column whose width is stated in `ch`, the unit every ladder in §2.9 uses.
    pub fn fixed_ch(width_ch: f32, element: impl IntoElement) -> Self {
        Self::fixed(ch(width_ch), element)
    }

    /// A column that takes the remaining width.
    pub fn flex(element: impl IntoElement) -> Self {
        Self::with(element.into_any_element(), None, true)
    }

    /// An auto-width column.
    pub fn auto(element: impl IntoElement) -> Self {
        Self::with(element.into_any_element(), None, false)
    }

    /// The column a [`ColumnLadder`](super::ColumnLadder) resolved for the current pane width,
    /// filled with `element`.
    ///
    /// This is the only correct way to build a worktrees or PR row: the ladder decides the
    /// width, the alignment and whether the column exists at all, so the breakpoints live in
    /// one place instead of in every view.
    pub fn resolved(column: &ResolvedColumn, element: impl IntoElement) -> Self {
        let mut out = match column.width {
            Some(width) => Self::fixed(width, element),
            None => Self::flex(element),
        };
        out.min_width = column.min_width;
        out.align = column.align;
        out
    }

    /// Set the alignment.
    pub fn align(mut self, align: ColumnAlign) -> Self {
        self.align = align;
        self
    }

    /// Set a minimum width. A flex column never shrinks below it (§2.9 "flex, min 24 ch").
    pub fn min_width(mut self, min_width: Pixels) -> Self {
        self.min_width = Some(min_width);
        self
    }

    /// Set a minimum width in `ch`.
    pub fn min_width_ch(self, min_width_ch: f32) -> Self {
        self.min_width(ch(min_width_ch))
    }

    /// Draw this column only while its row is hovered or selected, keeping its width reserved
    /// either way so revealing it never reflows the row.
    ///
    /// This is the column form of [`Row::hover_actions`], for a laddered list: resolve an
    /// actions column from the [`ColumnLadder`](super::ColumnLadder) and fill it with
    /// `RowColumn::resolved(actions, buttons).hover_only()`, so the row, its hover actions and
    /// the [`super::ListHeader`] above it all share one width.
    pub fn hover_only(mut self) -> Self {
        self.hover_only = true;
        self
    }
}

/// One list row.
#[derive(IntoElement)]
pub struct Row {
    id: Option<ElementId>,
    leading: Option<AnyElement>,
    reserve_leading: bool,
    leading_width: Option<Pixels>,
    columns: Vec<RowColumn>,
    second_line: Option<AnyElement>,
    details: Option<AnyElement>,
    height: Option<Pixels>,
    selected: bool,
    cursor: bool,
    dimmed: bool,
    disabled: bool,
    hoverable: bool,
    comfortable: bool,
    on_click: Option<PressHandler>,
    on_double_click: Option<PressHandler>,
    on_secondary_click: Option<PressHandler>,
}

impl Row {
    /// An empty row.
    pub fn new() -> Self {
        Self {
            id: None,
            leading: None,
            reserve_leading: false,
            leading_width: None,
            columns: Vec::new(),
            second_line: None,
            details: None,
            height: None,
            selected: false,
            cursor: false,
            dimmed: false,
            disabled: false,
            hoverable: true,
            comfortable: false,
            on_click: None,
            on_double_click: None,
            on_secondary_click: None,
        }
    }

    /// A row with a stable id.
    ///
    /// Hover and the pointer handlers work without one; the id is what an accessibility
    /// tree, a scroll-into-view or a keyed element state needs.
    pub fn with_id(id: impl Into<ElementId>) -> Self {
        Self::new().id(id)
    }

    /// Set the element id.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// The 2 ch glyph slot. Pass a [`super::StatusGlyph`]; pass nothing for "not applicable".
    ///
    /// Leaving it unset is the **blank cell** of §2.5 — "this column does not apply to this
    /// row" — and is not the same as [`super::StatusKind::NoSession`], which is a dim dot.
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// Reserve the glyph column even when this row has no glyph, so a list whose rows
    /// disagree about the leading slot still aligns its text. Off by default: a list where
    /// no row ever carries a glyph must not pay for the column.
    pub fn reserve_leading(mut self, reserve: bool) -> Self {
        self.reserve_leading = reserve;
        self
    }

    /// Widen the leading slot past its 2 ch glyph width, for a leading element that is not a
    /// glyph: the palette's icon tile. Every row of one list passes the same width, or the text
    /// columns stop lining up.
    pub fn leading_width(mut self, width: Pixels) -> Self {
        self.leading_width = Some(width);
        self
    }

    /// Append a column.
    pub fn column(mut self, column: RowColumn) -> Self {
        self.columns.push(column);
        self
    }

    /// Append several columns.
    pub fn columns(mut self, columns: impl IntoIterator<Item = RowColumn>) -> Self {
        self.columns.extend(columns);
        self
    }

    /// The second line of a two-line row (job progress sub-line, clone descriptions). The row
    /// grows to the 44 px two-line height unless [`Row::height`] says otherwise.
    pub fn second_line(mut self, line: impl IntoElement) -> Self {
        self.second_line = Some(line.into_any_element());
        self
    }

    /// A block under the row's line (or lines), aligned with the first text column: a job's
    /// progress bar, its inline error, the buttons that act on it.
    ///
    /// The row's line keeps its height and the row grows to fit the block, so a list holding
    /// such rows must measure each one (`gpui::list`), not assume a uniform height.
    pub fn details(mut self, details: impl IntoElement) -> Self {
        self.details = Some(details.into_any_element());
        self
    }

    /// Override the row height.
    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    /// Paint the selection background: this is the list's current item.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Draw the 2 px accent cursor bar on the leading edge: this pane has focus.
    ///
    /// Selection and cursor are separate flags on purpose — a list keeps its selected row while
    /// focus lives in another pane, and then the row keeps the background and loses the bar.
    pub fn cursor(mut self, cursor: bool) -> Self {
        self.cursor = cursor;
        self
    }

    /// Dim to 40 %: a row being deleted.
    pub fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self
    }

    /// Non-selectable. Dims and stops hover.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Turn the hover background off. A row with a pointer handler still shows the pointer
    /// cursor; this only drops the `row_hover` paint (a header row, a static fact).
    pub fn hoverable(mut self, hoverable: bool) -> Self {
        self.hoverable = hoverable;
        self
    }

    /// The 44 px comfortable density of the Hub lists (`metrics.row_h_comfortable`).
    ///
    /// The row sets the height; the cells carry the type. Fill the primary cell with
    /// `Text::ui_strong(..)` (body, weight 500) and every secondary cell with
    /// `Text::ui(..).muted()` (`text_secondary`), so a comfortable list reads as one name per
    /// row followed by its facts.
    pub fn comfortable(mut self) -> Self {
        self.comfortable = true;
        self
    }

    /// A single press of the primary button: **select** this row (UX-SPEC §5.1).
    ///
    /// Fires on the press, like every native list, so the cursor is already on the row by the
    /// time a second press of a double-click arrives. A handler implies the pointer cursor.
    pub fn on_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// The second press of a double-click: **open** this row, the pointer twin of `⏎`.
    ///
    /// Read from the press's click count, so the first press of the pair has already run
    /// [`Row::on_click`]. Without this handler a double-click is two selects.
    pub fn on_double_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_double_click = Some(Box::new(handler));
        self
    }

    /// A right click (a ctrl-click on macOS): open this row's menu at `event.position`.
    ///
    /// The row provides the hook, not the menu; the handler normally selects the row and then
    /// opens a context menu listing every row action, hover actions included.
    pub fn on_secondary_click(
        mut self,
        handler: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_secondary_click = Some(Box::new(handler));
        self
    }

    /// The trailing slot drawn only while the row is hovered or selected: the mockup's
    /// `Open ⏎` button and its `⋯` menu trigger.
    ///
    /// The slot keeps its width reserved while hidden, so revealing it never reflows the row.
    /// Every action placed here must also be reachable from the row's key and from its
    /// right-click menu (UX-SPEC §5.1). In a list with a [`super::ListHeader`], use a ladder
    /// column and [`RowColumn::hover_only`] instead, so the header reserves the same width.
    pub fn hover_actions(self, actions: impl IntoElement) -> Self {
        self.column(
            RowColumn::auto(actions)
                .align(ColumnAlign::Right)
                .hover_only(),
        )
    }

    /// Whether any pointer handler is attached.
    fn is_pressable(&self) -> bool {
        self.on_click.is_some()
            || self.on_double_click.is_some()
            || self.on_secondary_click.is_some()
    }
}

impl Default for Row {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for Row {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let two_line = self.second_line.is_some();
        let height = self.height.unwrap_or(if self.comfortable {
            theme.metrics.row_h_comfortable
        } else if two_line {
            theme.metrics.job_row_h
        } else {
            theme.metrics.row_h
        });
        let selected = self.selected;
        let live = !self.disabled;
        // Hover is pointer feedback only (§3): it never expresses state, and it never paints
        // over the selection background of the row the cursor is already on.
        let hoverable = self.hoverable && live && !selected;
        let pressable = live && self.is_pressable();
        let hover_bg = theme.colors.row_hover;
        let gap = theme.space.md;
        let pad = theme.space.md;
        let has_hover_only = self.columns.iter().any(|column| column.hover_only);
        let has_leading = self.reserve_leading || self.leading.is_some();
        let leading_w = self.leading_width.unwrap_or(ch(GLYPH_COLUMN_CH));

        let content = div()
            .flex()
            .items_center()
            .h_full()
            .w_full()
            .gap(gap)
            .px(pad)
            .children(has_leading.then(|| {
                div()
                    .flex_none()
                    .w(leading_w)
                    .flex()
                    .items_center()
                    .justify_center()
                    .children(self.leading)
            }))
            .children(self.columns.into_iter().map(|column| {
                div()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .min_w_0()
                    .when(column.flex, |el| el.flex_1())
                    .when_some(column.width, |el, width| el.w(width).flex_none())
                    .when_some(column.min_width, |el, min_width| el.min_w(min_width))
                    .map(|el| match column.align {
                        ColumnAlign::Left => el.justify_start(),
                        ColumnAlign::Center => el.justify_center(),
                        ColumnAlign::Right => el.justify_end(),
                    })
                    // Hidden, not removed: `invisible` keeps the layout box, so revealing the
                    // cell on hover never moves a column beside it.
                    .when(column.hover_only && !(selected && live), |el| {
                        el.invisible().when(live, |el| {
                            el.group_hover(ROW_GROUP, |style| style.visible())
                        })
                    })
                    .child(column.element)
            }));

        let body = match self.second_line {
            Some(line) => div()
                .flex()
                .flex_col()
                .size_full()
                .child(div().flex_1().min_h_0().child(content))
                .child(
                    div()
                        .flex_none()
                        .px(pad)
                        .pb(theme.space.xs)
                        .overflow_hidden()
                        .child(line),
                )
                .into_any_element(),
            None => content.into_any_element(),
        };
        // With a details block the line keeps its height and the row grows under it; the block
        // starts where the first text column starts, so it reads as part of the same item.
        let details_indent = if has_leading {
            pad + leading_w + gap
        } else {
            pad
        };
        let details = self.details.map(|details| {
            div()
                .flex()
                .flex_col()
                .w_full()
                .pl(details_indent)
                .pr(pad)
                .pb(theme.space.sm)
                .child(details)
        });
        let grows = details.is_some();
        let body = match details {
            Some(details) => div()
                .flex()
                .flex_col()
                .w_full()
                .child(div().w_full().h(height).child(body))
                .child(details)
                .into_any_element(),
            None => body,
        };

        let (on_click, on_double_click, on_secondary_click) = if live {
            (self.on_click, self.on_double_click, self.on_secondary_click)
        } else {
            (None, None, None)
        };
        let primary = (on_click.is_some() || on_double_click.is_some())
            .then_some((on_click, on_double_click));

        let base = div()
            .w_full()
            .when(!grows, |el| el.h(height))
            .when(selected, |el| el.bg(theme.colors.row_selected))
            .when(self.dimmed || self.disabled, |el| {
                el.opacity(theme.metrics.dimmed_opacity)
            })
            .when(has_hover_only, |el| el.group(ROW_GROUP))
            .when(hoverable, |el| el.hover(move |s| s.bg(hover_bg)))
            .when(pressable, |el| el.cursor_pointer())
            .when_some(primary, |el, (on_click, on_double_click)| {
                el.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    if event.click_count >= 2
                        && let Some(open) = &on_double_click
                    {
                        open(event, window, cx);
                    } else if let Some(select) = &on_click {
                        select(event, window, cx);
                    }
                })
            })
            .when_some(on_secondary_click, |el, menu| {
                el.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    menu(event, window, cx)
                })
            })
            .child(FocusRing::cursor_row(self.cursor).content(body));

        match self.id {
            Some(id) => base.id(id).into_any_element(),
            None => base.into_any_element(),
        }
    }
}
