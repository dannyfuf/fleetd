//! `FuzzyList` — a debounced query's ranked, capped result rows.
//!
//! §3.8: a list **under a text field** moves with `ctrl-n` / `ctrl-p` or `↓` / `↑` and never
//! with `j` / `k`; a dialog with no text field (Assign, Settings, Confirm) does bind `j` / `k`
//! ([D-12]). [`FuzzyList::binds_jk`] states which case a given list is in, so the dialog does
//! not have to re-derive it.
//!
//! **Minimal render.** Matching, ranking and debouncing belong to the caller (they need the
//! domain's fields); this component renders already-ranked rows with the cap applied.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*};

use crate::{components::Row, text::Text, theme::ActiveTheme};

/// One result row.
pub struct FuzzyItem {
    primary: SharedString,
    secondary: Option<SharedString>,
    trailing: Option<SharedString>,
    leading: Option<AnyElement>,
    disabled: bool,
}

impl FuzzyItem {
    /// A row with a primary line.
    pub fn new(primary: impl Into<SharedString>) -> Self {
        Self {
            primary: primary.into(),
            secondary: None,
            trailing: None,
            leading: None,
            disabled: false,
        }
    }

    /// A muted second line. The row collapses to one line when this is absent (§3.8.2:
    /// an empty description must not leave a blank line).
    pub fn secondary(mut self, secondary: impl Into<SharedString>) -> Self {
        self.secondary = Some(secondary.into());
        self
    }

    /// A right-aligned muted value, e.g. a relative `updatedAt`.
    pub fn trailing(mut self, trailing: impl Into<SharedString>) -> Self {
        self.trailing = Some(trailing.into());
        self
    }

    /// A leading glyph.
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// Render the row as non-selectable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A capped list of ranked results.
#[derive(IntoElement)]
pub struct FuzzyList {
    items: Vec<FuzzyItem>,
    cursor: usize,
    cap: usize,
    under_text_field: bool,
    empty: Option<AnyElement>,
}

impl FuzzyList {
    /// A list over already-ranked items.
    pub fn new(items: impl IntoIterator<Item = FuzzyItem>) -> Self {
        Self {
            items: items.into_iter().collect(),
            cursor: 0,
            cap: 8,
            under_text_field: true,
            empty: None,
        }
    }

    /// Which item carries the cursor.
    pub fn cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor;
        self
    }

    /// How many rows to show. 8 for Clone, 6 for the Create base list, 10 for the palette.
    pub fn cap(mut self, cap: usize) -> Self {
        self.cap = cap;
        self
    }

    /// Whether a text field owns the keyboard above this list.
    pub fn under_text_field(mut self, under: bool) -> Self {
        self.under_text_field = under;
        self
    }

    /// What to show when there are no results.
    pub fn empty(mut self, empty: impl IntoElement) -> Self {
        self.empty = Some(empty.into_any_element());
        self
    }

    /// Whether this list may bind `j` / `k` ([D-12]).
    pub fn binds_jk(&self) -> bool {
        !self.under_text_field
    }
}

impl RenderOnce for FuzzyList {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        if self.items.is_empty() {
            return div().w_full().children(self.empty).into_any_element();
        }
        let cursor = self.cursor;
        let two_line_h = theme.metrics.job_row_h;
        div()
            .flex()
            .flex_col()
            .w_full()
            .children(self.items.into_iter().take(self.cap).enumerate().map(
                |(ix, item)| {
                    let selected = ix == cursor;
                    let has_secondary = item.secondary.is_some();
                    let mut row = Row::new()
                        .selected(selected)
                        .cursor(selected)
                        .disabled(item.disabled)
                        .height(if has_secondary {
                            two_line_h
                        } else {
                            theme.metrics.row_h
                        })
                        .column(crate::components::RowColumn::flex(
                            Text::ui(item.primary).ellipsize(),
                        ));
                    if let Some(leading) = item.leading {
                        row = row.leading(leading);
                    }
                    if let Some(trailing) = item.trailing {
                        row = row.column(
                            crate::components::RowColumn::auto(Text::ui(trailing).faint())
                                .align(crate::components::ColumnAlign::Right),
                        );
                    }
                    if let Some(secondary) = item.secondary {
                        row = row.second_line(Text::ui(secondary).muted().ellipsize());
                    }
                    row
                },
            ))
            .into_any_element()
    }
}
