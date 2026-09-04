//! `Row` — leading glyph slot, flex content, trailing columns.
//!
//! Every list in Fleet is made of these, at 30 px (44 px for jobs, 34 px for the palette).
//! The row owns its own selected / dimmed / disabled rendering so that a state change is a
//! glyph change **in place** and never a re-sort or a re-layout (§3.3 "cursor stability").

use gpui::{AnyElement, App, ElementId, Pixels, Window, div, prelude::*};

use crate::{components::FocusRing, theme::ActiveTheme};

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
    align: ColumnAlign,
    flex: bool,
}

impl RowColumn {
    /// A fixed-width column.
    pub fn fixed(width: Pixels, element: impl IntoElement) -> Self {
        Self {
            element: element.into_any_element(),
            width: Some(width),
            align: ColumnAlign::Left,
            flex: false,
        }
    }

    /// A column that takes the remaining width.
    pub fn flex(element: impl IntoElement) -> Self {
        Self {
            element: element.into_any_element(),
            width: None,
            align: ColumnAlign::Left,
            flex: true,
        }
    }

    /// An auto-width column.
    pub fn auto(element: impl IntoElement) -> Self {
        Self {
            element: element.into_any_element(),
            width: None,
            align: ColumnAlign::Left,
            flex: false,
        }
    }

    /// Set the alignment.
    pub fn align(mut self, align: ColumnAlign) -> Self {
        self.align = align;
        self
    }
}

/// One list row.
#[derive(IntoElement)]
pub struct Row {
    id: Option<ElementId>,
    leading: Option<AnyElement>,
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
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
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

    /// The second line of a two-line row (job progress sub-line, clone descriptions).
    pub fn second_line(mut self, line: impl IntoElement) -> Self {
        self.second_line = Some(line.into_any_element());
        self
    }

    /// Override the row height.
    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    /// Paint the selection background.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Draw the 2 px accent cursor bar on the leading edge.
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
        let height = self.height.unwrap_or(theme.metrics.row_h);
        let selected = self.selected;
        let hoverable = self.hoverable && !self.disabled;
        let hover_bg = theme.colors.row_hover;

        let content = div()
            .flex()
            .items_center()
            .h_full()
            .w_full()
            .gap(theme.space.md)
            .px(theme.space.md)
            .children(
                self.leading
                    .map(|glyph| div().flex_none().w(crate::theme::ch(2.0)).flex().items_center().justify_center().child(glyph)),
            )
            .children(self.columns.into_iter().map(|column| {
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .when(column.flex, |el| el.flex_1())
                    .when_some(column.width, |el, width| el.w(width).flex_none())
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
                        .px(theme.space.md)
                        .pb(theme.space.xs)
                        .child(line),
                )
                .into_any_element(),
            None => content.into_any_element(),
        };

        let base = div()
            .w_full()
            .h(height)
            .when(selected, |el| el.bg(theme.colors.row_selected))
            .when(self.dimmed || self.disabled, |el| el.opacity(0.4));
        let ring = FocusRing::cursor_row(self.cursor).child(body);

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
