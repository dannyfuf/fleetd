//! `Row` — leading glyph slot, flex content, trailing columns.
//!
//! Every list in Fleet is made of these, at 30 px (44 px for a two-line job row, 34 px for a
//! palette row). The row owns its own selected / cursor / dimmed / disabled rendering so that a
//! state change is a glyph change **in place** and never a re-sort or a re-layout (§3.3
//! "cursor stability").
//!
//! Columns are the §2.9 ladder made concrete: build them from
//! [`ColumnLadder::resolve`](super::ColumnLadder::resolve) and
//! [`RowColumn::resolved`], so the pane width — never the window width — decides which columns
//! a row draws.

use gpui::{AnyElement, App, ElementId, Pixels, Window, div, prelude::*};

use crate::{
    components::{FocusRing, ResolvedColumn},
    theme::{ActiveTheme, ch},
};

/// The glyph column of every list, in `ch` (§2.9 column 1).
pub const GLYPH_COLUMN_CH: f32 = 2.0;

/// The opacity a dimmed or disabled row renders at (§3 row states).
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
}

impl RowColumn {
    /// A fixed-width column.
    pub fn fixed(width: Pixels, element: impl IntoElement) -> Self {
        Self {
            element: element.into_any_element(),
            width: Some(width),
            min_width: None,
            align: ColumnAlign::Left,
            flex: false,
        }
    }

    /// A fixed column whose width is stated in `ch`, the unit every ladder in §2.9 uses.
    pub fn fixed_ch(width_ch: f32, element: impl IntoElement) -> Self {
        Self::fixed(ch(width_ch), element)
    }

    /// A column that takes the remaining width.
    pub fn flex(element: impl IntoElement) -> Self {
        Self {
            element: element.into_any_element(),
            width: None,
            min_width: None,
            align: ColumnAlign::Left,
            flex: true,
        }
    }

    /// An auto-width column.
    pub fn auto(element: impl IntoElement) -> Self {
        Self {
            element: element.into_any_element(),
            width: None,
            min_width: None,
            align: ColumnAlign::Left,
            flex: false,
        }
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
}

/// One list row.
#[derive(IntoElement)]
pub struct Row {
    id: Option<ElementId>,
    leading: Option<AnyElement>,
    reserve_leading: bool,
    columns: Vec<RowColumn>,
    second_line: Option<AnyElement>,
    height: Option<Pixels>,
    selected: bool,
    cursor: bool,
    dimmed: bool,
    disabled: bool,
    hoverable: bool,
}

impl Row {
    /// An empty row.
    pub fn new() -> Self {
        Self {
            id: None,
            leading: None,
            reserve_leading: false,
            columns: Vec::new(),
            second_line: None,
            height: None,
            selected: false,
            cursor: false,
            dimmed: false,
            disabled: false,
            hoverable: true,
        }
    }

    /// A row with a stable id, required for hover and click.
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

    /// Turn the hover background off.
    pub fn hoverable(mut self, hoverable: bool) -> Self {
        self.hoverable = hoverable;
        self
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
        let height = self.height.unwrap_or(if two_line {
            theme.metrics.job_row_h
        } else {
            theme.metrics.row_h
        });
        let selected = self.selected;
        // Hover is pointer feedback only (§3): it never expresses state, and it never paints
        // over the selection background of the row the cursor is already on.
        let hoverable = self.hoverable && !self.disabled && !selected;
        let hover_bg = theme.colors.row_hover;
        let gap = theme.space.md;
        let pad = theme.space.md;

        let content = div()
            .flex()
            .items_center()
            .h_full()
            .w_full()
            .gap(gap)
            .px(pad)
            .children((self.reserve_leading || self.leading.is_some()).then(|| {
                div()
                    .flex_none()
                    .w(ch(GLYPH_COLUMN_CH))
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

        let base = div()
            .w_full()
            .h(height)
            .when(selected, |el| el.bg(theme.colors.row_selected))
            .when(self.dimmed || self.disabled, |el| {
                el.opacity(theme.metrics.dimmed_opacity)
            });
        let ring = FocusRing::cursor_row(self.cursor).content(body);

        match self.id {
            Some(id) => base
                .id(id)
                .when(hoverable, |el| el.hover(move |s| s.bg(hover_bg)))
                .child(ring)
                .into_any_element(),
            None => base.child(ring).into_any_element(),
        }
    }
}
