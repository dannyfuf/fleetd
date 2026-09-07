//! `FuzzyList` — a debounced query's ranked, capped result rows.
//!
//! §3.8: a list **under a text field** moves with `ctrl-n` / `ctrl-p` or `↓` / `↑` and never
//! with `j` / `k`; a dialog with no text field (Assign, Settings, Confirm) does bind `j` / `k`
//! ([D-12]). [`FuzzyList::binds_jk`] states which case a given list is in, so the dialog does
//! not have to re-derive it, and [`FuzzyList::next_cursor`] / [`FuzzyList::prev_cursor`]
//! implement the wrapping motion both key pairs share.
//!
//! Matching, ranking and the 150 ms debounce belong to the caller — they need the domain's
//! fields. What the component owns is the *rendering* of a match: [`FuzzyItem::matches`] takes
//! the character indices the ranker hit, and they render as a weight bump at full contrast.
//! §3.9 caps the treatment there deliberately ("no fuzzy-match highlighting beyond a subtle
//! weight bump"): a row painted in four colors stops reading as one label.
//!
//! Never render a list longer than its cap — a predictable `Enter` matters more than
//! completeness, and the footer says `9 of 63`.

use gpui::{AnyElement, App, FontWeight, Pixels, SharedString, Window, div, prelude::*};

use crate::{
    components::{ColumnAlign, Row, RowColumn},
    icons::{Icon, IconSize},
    text::{Text, TextRole},
    theme::{ActiveTheme, Theme, ch},
};

/// The `ch` budget of the right-aligned key column, wide enough for `S-⏎`.
const KEY_COLUMN_CH: f32 = 5.0;

/// One result row.
pub struct FuzzyItem {
    primary: SharedString,
    detail: Option<SharedString>,
    secondary: Option<SharedString>,
    trailing: Option<SharedString>,
    leading: Option<AnyElement>,
    key: Option<SharedString>,
    matches: Vec<usize>,
    destructive: bool,
    disabled: bool,
}

impl FuzzyItem {
    /// A row with a primary line.
    pub fn new(primary: impl Into<SharedString>) -> Self {
        Self {
            primary: primary.into(),
            detail: None,
            secondary: None,
            trailing: None,
            leading: None,
            key: None,
            matches: Vec::new(),
            destructive: false,
            disabled: false,
        }
    }

    /// A muted second line. The row collapses to one line when this is absent (§3.8.2:
    /// an empty description must not leave a blank line).
    pub fn secondary(mut self, secondary: impl Into<SharedString>) -> Self {
        self.secondary = Some(secondary.into());
        self
    }

    /// A muted qualifier drawn **on the same line**, after the primary label.
    ///
    /// Use it when the qualifier is what makes the row's identity (§3.8.5: a context's owners
    /// are the reason a repo belongs to it) — [`FuzzyItem::secondary`] grows the row to two
    /// lines, which is the right shape for a description and the wrong one for a qualifier the
    /// eye reads with the name.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A right-aligned muted value, e.g. a relative `updatedAt` or `session attached`.
    pub fn trailing(mut self, trailing: impl Into<SharedString>) -> Self {
        self.trailing = Some(trailing.into());
        self
    }

    /// A leading glyph.
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// The bound key, right-aligned in its own column — the §3.9 rule that makes the palette
    /// train itself out of the loop.
    pub fn key(mut self, key: impl Into<SharedString>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// The character indices of `primary` the ranker matched. Out-of-range indices are ignored,
    /// so a caller that matched against a pre-truncation string cannot panic the render.
    pub fn matches(mut self, matches: impl IntoIterator<Item = usize>) -> Self {
        self.matches = matches.into_iter().collect();
        self.matches.sort_unstable();
        self.matches.dedup();
        self
    }

    /// Prefix the row with `triangle-alert`. The action is still routed through its confirm.
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// Render the row as non-selectable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Coalesce matched character indices into UTF-8 ranges without copying the label.
fn match_ranges(text: &str, matches: &[usize]) -> Vec<std::ops::Range<usize>> {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    let mut hits = matches.iter().copied().peekable();
    for (index, (byte, ch)) in text.char_indices().enumerate() {
        while hits.peek().is_some_and(|hit| *hit < index) {
            hits.next();
        }
        if hits.peek() != Some(&index) {
            continue;
        }
        hits.next();
        if let Some(range) = ranges.last_mut()
            && range.end == byte
        {
            range.end += ch.len_utf8();
        } else {
            ranges.push(byte..byte + ch.len_utf8());
        }
    }
    ranges
}

fn highlighted(role: TextRole, text: SharedString, matches: &[usize], theme: &Theme) -> AnyElement {
    if matches.is_empty() {
        return Text::new(role, text).ellipsize().into_any_element();
    }
    let ranges = match_ranges(&text, matches);
    crate::styled_with(div(), role.style(theme), theme)
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_color(theme.colors.text_secondary)
        .font_weight(FontWeight::NORMAL)
        .child(
            gpui::StyledText::new(text).with_highlights(ranges.into_iter().map(|range| {
                (
                    range,
                    gpui::HighlightStyle {
                        color: Some(theme.colors.text),
                        font_weight: Some(theme.text.ui_strong.weight),
                        ..Default::default()
                    },
                )
            })),
        )
        .into_any_element()
}

/// A capped list of ranked results.
#[derive(IntoElement)]
pub struct FuzzyList {
    items: Vec<FuzzyItem>,
    cursor: usize,
    cap: usize,
    under_text_field: bool,
    row_height: Option<Pixels>,
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
            row_height: None,
            empty: None,
        }
    }

    /// Which item carries the cursor. An index past the end simply selects nothing, which is
    /// what a sectioned surface such as [`super::Palette`] relies on.
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

    /// Override the row height. 30 px one-line, 44 px two-line, 34 px inside the palette.
    pub fn row_height(mut self, height: Pixels) -> Self {
        self.row_height = Some(height);
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

    /// How many rows will actually render, after the cap.
    pub fn shown(&self) -> usize {
        self.items.len().min(self.cap)
    }

    /// `ctrl-n` / `↓` (and `j` when [`FuzzyList::binds_jk`]): the next row, wrapping.
    ///
    /// A fuzzy list wraps where a pane list clamps: the set is short, capped and re-ranked on
    /// every keystroke, so there is no scroll position for the user to lose.
    pub fn next_cursor(cursor: usize, len: usize) -> usize {
        super::navigation::next(cursor, len)
    }

    /// `ctrl-p` / `↑` (and `k` when [`FuzzyList::binds_jk`]): the previous row, wrapping.
    pub fn prev_cursor(cursor: usize, len: usize) -> usize {
        super::navigation::previous(cursor, len)
    }
}

impl RenderOnce for FuzzyList {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        if self.items.is_empty() {
            return div()
                .flex()
                .w_full()
                .items_center()
                .px(theme.space.md)
                .min_h(theme.metrics.row_h)
                .children(self.empty)
                .into_any_element();
        }

        let cursor = self.cursor;
        let one_line_h = self.row_height.unwrap_or(theme.metrics.row_h);
        let two_line_h = self.row_height.unwrap_or(theme.metrics.job_row_h);

        div()
            .flex()
            .flex_col()
            .w_full()
            .children(
                self.items
                    .into_iter()
                    .take(self.cap)
                    .enumerate()
                    .map(move |(ix, item)| {
                        let selected = ix == cursor && !item.disabled;
                        let has_secondary = item.secondary.is_some();
                        let mut row = Row::new()
                            // Only some items carry a glyph; the column is reserved so the
                            // primary text of every row starts at the same x.
                            .reserve_leading(true)
                            .selected(selected)
                            .cursor(selected)
                            .disabled(item.disabled)
                            .height(if has_secondary {
                                two_line_h
                            } else {
                                one_line_h
                            });

                        if let Some(leading) = item.leading {
                            row = row.leading(leading);
                        } else if item.destructive {
                            row = row.leading(
                                Icon::TriangleAlert
                                    .el()
                                    .size(IconSize::Large)
                                    .color(theme.colors.danger),
                            );
                        }

                        row = row.column(RowColumn::flex(highlighted(
                            TextRole::Ui,
                            item.primary,
                            &item.matches,
                            theme,
                        )));

                        if let Some(detail) = item.detail {
                            row = row.column(RowColumn::flex(Text::ui(detail).muted().ellipsize()));
                        }
                        if let Some(trailing) = item.trailing {
                            row = row.column(
                                RowColumn::auto(Text::ui(trailing).faint())
                                    .align(ColumnAlign::Right),
                            );
                        }
                        if let Some(key) = item.key {
                            row = row.column(
                                RowColumn::fixed(ch(KEY_COLUMN_CH), Text::hint(key))
                                    .align(ColumnAlign::Right),
                            );
                        }
                        if let Some(secondary) = item.secondary {
                            row = row.second_line(Text::ui(secondary).muted().ellipsize());
                        }
                        row
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_merge_adjacent_matches_and_keep_utf8_boundaries() {
        assert_eq!(match_ranges("payroll", &[0, 1, 2]), vec![0..3]);
        assert_eq!(match_ranges("ab", &[0, 9]), vec![0..1]);
        assert_eq!(match_ranges("héllo", &[1]), vec![1..3]);
    }

    #[test]
    fn cursor_motion_wraps_both_ways() {
        assert_eq!(FuzzyList::next_cursor(2, 3), 0);
        assert_eq!(FuzzyList::next_cursor(0, 3), 1);
        assert_eq!(FuzzyList::prev_cursor(0, 3), 2);
        assert_eq!(FuzzyList::prev_cursor(2, 3), 1);
    }

    #[test]
    fn cursor_motion_on_an_empty_list_stays_at_zero() {
        assert_eq!(FuzzyList::next_cursor(0, 0), 0);
        assert_eq!(FuzzyList::prev_cursor(0, 0), 0);
    }

    #[test]
    fn the_cap_bounds_what_renders() {
        let list = FuzzyList::new((0..20).map(|i| FuzzyItem::new(i.to_string()))).cap(10);
        assert_eq!(list.shown(), 10);
        assert!(!list.binds_jk());
    }
}
