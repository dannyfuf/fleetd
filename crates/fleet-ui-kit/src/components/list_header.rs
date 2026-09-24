//! `ListHeader` — the column heads above a laddered list.
//!
//! One line of sentence-case `caption` labels, aligned to the same
//! [`ColumnLadder`](super::ColumnLadder) resolution the rows below it use. It is built from a
//! [`Row`] on purpose: the leading glyph slot, the padding, the gap and the column boxes are
//! the row's own, so a head can never drift from its column by a pixel. Not a
//! [`super::SectionHeader`], which titles a group of rows rather than naming their columns.

use gpui::{App, SharedString, Window, prelude::*};

use crate::{
    components::{ResolvedColumn, Row, RowColumn},
    text::Text,
    theme::ActiveTheme,
};

/// The column heads of a list.
#[derive(IntoElement)]
pub struct ListHeader {
    reserve_leading: bool,
    columns: Vec<(ResolvedColumn, SharedString)>,
}

impl ListHeader {
    /// A header with no columns yet.
    pub fn new() -> Self {
        Self {
            reserve_leading: false,
            columns: Vec::new(),
        }
    }

    /// Reserve the glyph column, as the rows below do with [`Row::reserve_leading`] or a
    /// leading glyph. Must match the rows, or every head sits one glyph to the left.
    pub fn reserve_leading(mut self, reserve: bool) -> Self {
        self.reserve_leading = reserve;
        self
    }

    /// Head one resolved column with `label`, in sentence case (`Pull request`, not
    /// `PULL REQUEST`). Pass an empty label for a column with no head, such as the
    /// [`RowColumn::hover_only`] actions column: its width is still reserved.
    pub fn column(mut self, column: &ResolvedColumn, label: impl Into<SharedString>) -> Self {
        self.columns.push((column.clone(), label.into()));
        self
    }
}

impl Default for ListHeader {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for ListHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        gpui::div()
            .w_full()
            .flex_none()
            .pb(theme.space.xs)
            .border_b(theme.metrics.hairline)
            .border_color(theme.colors.border)
            .child(
                Row::new()
                    .hoverable(false)
                    .height(theme.metrics.section_header_h)
                    .reserve_leading(self.reserve_leading)
                    .columns(self.columns.into_iter().map(|(column, label)| {
                        RowColumn::resolved(&column, Text::caption(label).ellipsize())
                    })),
            )
    }
}
