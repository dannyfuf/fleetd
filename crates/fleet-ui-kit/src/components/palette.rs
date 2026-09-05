//! `Palette` — sectioned `GO` / `DO` / `CONTEXT` results with right-aligned key hints.
//!
//! §3.9. The palette is a [`TextField`] over a stack of [`FuzzyList`]s, one per section, sharing
//! a single flat cursor. Three rules are structural rather than stylistic:
//!
//! * `GO` (objects) always comes **first**, so a session is reachable from inside another
//!   session with `ctrl-s s : pay fix ⏎` and no list scan. The order is normalised on render,
//!   so a caller cannot get it wrong by pushing sections in the order it computed them;
//! * every `DO` row shows its bound key, right-aligned, so the palette trains itself out of
//!   the loop;
//! * the total cap is **10 rows across all sections**, so the top match never moves below the
//!   fold and `Enter` stays predictable. A command that is invalid here is **not listed at
//!   all** — never greyed, because a greyed row costs a `j`.
//!
//! Ranking, matching and the flat cursor belong to the caller; [`Palette::shown`] and
//! [`Palette::flat_len`] give it the two numbers it needs to move that cursor with
//! [`FuzzyList::next_cursor`] / [`FuzzyList::prev_cursor`] on `ctrl-n` / `ctrl-p`.

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*, px};

use crate::{
    components::{FuzzyItem, FuzzyList, KeyHintRow, TextField},
    icons::Icon,
    text::Text,
    theme::ActiveTheme,
    tone::Tone,
};

/// The three sections, in their fixed order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PaletteSectionKind {
    /// Objects: worktrees, PRs, repos. Ranked above commands.
    Go,
    /// Valid commands only, each with its bound key.
    Do,
    /// Context switches, with their digit.
    Context,
}

impl PaletteSectionKind {
    /// The section title.
    pub fn title(self) -> &'static str {
        match self {
            PaletteSectionKind::Go => "go",
            PaletteSectionKind::Do => "do",
            PaletteSectionKind::Context => "context",
        }
    }
}

/// One palette row.
pub struct PaletteRow {
    icon: Option<Icon>,
    label: SharedString,
    detail: Option<SharedString>,
    key: Option<SharedString>,
    destructive: bool,
    leading: Option<AnyElement>,
    matches: Vec<usize>,
}

impl PaletteRow {
    /// A row.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            icon: None,
            label: label.into(),
            detail: None,
            key: None,
            destructive: false,
            leading: None,
            matches: Vec::new(),
        }
    }

    /// The glyph. Use the same glyph the object or action uses elsewhere.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A custom leading element, e.g. a [`super::StatusGlyph`] for a worktree row.
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// The muted right-hand description (`session attached`, `PR · mine`, `repo`).
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The bound key, right-aligned.
    pub fn key(mut self, key: impl Into<SharedString>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// The character indices of the label the ranker matched (§3.9 allows a weight bump and
    /// nothing louder).
    pub fn matches(mut self, matches: impl IntoIterator<Item = usize>) -> Self {
        self.matches = matches.into_iter().collect();
        self
    }

    /// Prefix with `triangle-alert`. The action is still routed through its confirm dialog.
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// The [`FuzzyItem`] this row renders as.
    fn into_item(self, secondary: gpui::Hsla) -> FuzzyItem {
        let mut item = FuzzyItem::new(self.label).matches(self.matches);
        if self.destructive {
            // §3.9: "Destructive commands — prefixed with `triangle-alert`". Recolouring the
            // action's own glyph is not that prefix: a red `trash` still reads as "delete",
            // which is what the label already says, while the warning mark reads as "stop".
            // Leaving the leading slot empty is what lets `FuzzyList` draw that mark.
            item = item.destructive(true);
        } else if let Some(leading) = self.leading {
            item = item.leading(leading);
        } else if let Some(icon) = self.icon {
            item = item.leading(
                icon.el()
                    .size(crate::icons::IconSize::Large)
                    .color(secondary),
            );
        }
        if let Some(detail) = self.detail {
            item = item.trailing(detail);
        }
        if let Some(key) = self.key {
            item = item.key(key);
        }
        item
    }
}

/// One section of the palette.
pub struct PaletteSection {
    kind: PaletteSectionKind,
    rows: Vec<PaletteRow>,
}

impl PaletteSection {
    /// A section.
    pub fn new(kind: PaletteSectionKind, rows: impl IntoIterator<Item = PaletteRow>) -> Self {
        Self {
            kind,
            rows: rows.into_iter().collect(),
        }
    }

    /// How many rows this section holds before the palette-wide cap.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the section would render nothing.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// The command palette card. Wrap it in an [`super::Overlay`].
#[derive(IntoElement)]
pub struct Palette {
    query: SharedString,
    caret: Option<usize>,
    sections: Vec<PaletteSection>,
    cursor: usize,
    cap: usize,
    total: usize,
    empty: Option<SharedString>,
}

impl Palette {
    /// A palette over ranked sections.
    pub fn new(query: impl Into<SharedString>) -> Self {
        Self {
            query: query.into(),
            caret: None,
            sections: Vec::new(),
            cursor: 0,
            cap: 10,
            total: 0,
            empty: None,
        }
    }

    /// Append a section. Order is normalised on render.
    pub fn section(mut self, section: PaletteSection) -> Self {
        self.sections.push(section);
        self
    }

    /// Caret position in characters. Defaults to the end of the query.
    pub fn caret(mut self, caret: usize) -> Self {
        self.caret = Some(caret);
        self
    }

    /// The flat cursor index across all sections.
    pub fn cursor(mut self, cursor: usize) -> Self {
        self.cursor = cursor;
        self
    }

    /// The total row cap. 10 by the spec.
    pub fn cap(mut self, cap: usize) -> Self {
        self.cap = cap;
        self
    }

    /// How many candidates matched in total, for the `9 of 63` footer.
    pub fn total(mut self, total: usize) -> Self {
        self.total = total;
        self
    }

    /// The no-match line, e.g. `Nothing matches "gpu".`
    pub fn empty(mut self, empty: impl Into<SharedString>) -> Self {
        self.empty = Some(empty.into());
        self
    }

    /// How many rows exist across every section, before the cap.
    pub fn flat_len(&self) -> usize {
        self.sections.iter().map(PaletteSection::len).sum()
    }

    /// How many rows will actually render, after the cap. The left half of `9 of 63`, and the
    /// length `ctrl-n` / `ctrl-p` wrap around.
    pub fn shown(&self) -> usize {
        self.flat_len().min(self.cap)
    }
}

impl RenderOnce for Palette {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let shown = self.shown();
        let total = self.total.max(shown);
        let cursor = self.cursor;
        let cap = self.cap;
        let secondary = theme.colors.text_secondary;

        let mut sections = self.sections;
        sections.sort_by_key(|section| section.kind);

        // One flat cursor across every section: the caller counts rows, not sections.
        let mut consumed = 0usize;
        let mut blocks: Vec<AnyElement> = Vec::new();
        for section in sections {
            if consumed >= cap || section.is_empty() {
                continue;
            }
            let room = cap - consumed;
            let take = section.len().min(room);
            let items = section
                .rows
                .into_iter()
                .take(take)
                .map(|row| row.into_item(secondary));
            let local_cursor = cursor.checked_sub(consumed).unwrap_or(usize::MAX);

            blocks.push(
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(theme.metrics.section_header_h)
                            .px(theme.space.md)
                            .child(Text::label(section.kind.title())),
                    )
                    .child(
                        FuzzyList::new(items)
                            .cap(take)
                            .cursor(local_cursor)
                            .under_text_field(true)
                            .row_height(theme.metrics.palette_row_h),
                    )
                    .into_any_element(),
            );
            consumed += take;
        }

        let body: AnyElement = if blocks.is_empty() {
            div()
                .flex()
                .items_center()
                .h(theme.metrics.palette_row_h)
                .px(theme.space.md)
                .child(Text::ui(self.empty.unwrap_or_default()).muted())
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .w_full()
                .pb(theme.space.xs)
                .children(blocks)
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .w_full()
                    .px(theme.space.sm)
                    .border_b(px(1.0))
                    .border_color(theme.colors.border)
                    .child(
                        // The query line carries neither a preview nor a validation message,
                        // so it is the one field allowed to drop the 18 px status slot.
                        // §3.9's prompt is `:` — the key that opens the palette. A `command`
                        // glyph there advertises `⌘`, which Fleet binds nowhere.
                        TextField::new(self.query)
                            .placeholder("go to, or do")
                            .prefix(":")
                            .focused(true)
                            .height(theme.metrics.palette_input_h)
                            .hide_status_line(true)
                            .when_some(self.caret, TextField::caret),
                    ),
            )
            .child(body)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme.space.sm)
                    .w_full()
                    .h(theme.metrics.section_header_h)
                    .px(theme.space.md)
                    .py(theme.space.xs)
                    .border_t(px(1.0))
                    .border_color(theme.colors.border)
                    .child(Text::hint(format!("{shown} of {total}")).tone(Tone::Muted))
                    .child(Text::hint("\u{b7}").faint())
                    .child(
                        KeyHintRow::new()
                            .key("\u{23ce}", "run")
                            .key("\u{2303}n/\u{2303}p", "move")
                            .key("esc", "cancel"),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(kind: PaletteSectionKind, n: usize) -> PaletteSection {
        PaletteSection::new(kind, (0..n).map(|i| PaletteRow::new(format!("row {i}"))))
    }

    #[test]
    fn the_cap_bounds_the_whole_palette_not_each_section() {
        let palette = Palette::new("pay")
            .section(section(PaletteSectionKind::Go, 6))
            .section(section(PaletteSectionKind::Do, 8))
            .section(section(PaletteSectionKind::Context, 3));
        assert_eq!(palette.flat_len(), 17);
        assert_eq!(palette.shown(), 10);
    }

    #[test]
    fn a_short_palette_shows_everything() {
        let palette = Palette::new("").section(section(PaletteSectionKind::Do, 3));
        assert_eq!(palette.shown(), 3);
    }

    #[test]
    fn sections_have_a_fixed_order() {
        let mut kinds = vec![
            PaletteSectionKind::Context,
            PaletteSectionKind::Do,
            PaletteSectionKind::Go,
        ];
        kinds.sort();
        assert_eq!(
            kinds,
            vec![
                PaletteSectionKind::Go,
                PaletteSectionKind::Do,
                PaletteSectionKind::Context
            ]
        );
    }
}
